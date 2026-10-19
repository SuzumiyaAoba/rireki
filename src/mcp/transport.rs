//! HTTP の境界検査、受信上限と JSON 応答。

use super::protocol::{dispatch, error};
use super::{History, HttpResponse, MAX_BODY, VERSIONS};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use tokio::sync::Semaphore;

fn header<'a>(request: &'a Request<Incoming>, name: &str) -> Option<&'a str> {
    request.headers().get(name).and_then(|h| h.to_str().ok())
}

pub(super) async fn handle(
    mut request: Request<Incoming>,
    address: SocketAddr,
    history: Arc<RwLock<History>>,
    active: Arc<Semaphore>,
) -> HttpResponse {
    let authorities = [address.to_string(), format!("localhost:{}", address.port())];
    // Host と Origin を検査し、外部ドメインからの DNS rebinding を拒否する。
    if header(&request, "host")
        .is_none_or(|host| !authorities.iter().any(|a| a.eq_ignore_ascii_case(host)))
        || request.headers().get_all("origin").iter().any(|origin| {
            origin
                .to_str()
                .ok()
                .is_none_or(|origin| !authorities.iter().any(|a| origin == format!("http://{a}")))
        })
    {
        return respond(403, None);
    }
    if request
        .uri()
        .path_and_query()
        .is_none_or(|path| path.as_str() != "/mcp")
    {
        return respond(404, None);
    }
    if header(&request, "mcp-protocol-version").is_some_and(|v| !VERSIONS.contains(&v)) {
        return respond(
            400,
            Some(error(
                Value::Null,
                -32600,
                "Unsupported MCP-Protocol-Version",
            )),
        );
    }
    // サーバー発の通知や永続 SSE ストリームを使わない stateless HTTP モード。
    if request.method() != Method::POST {
        return respond(405, None);
    }
    if header(&request, "content-type").is_none_or(|v| {
        !v.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("application/json")
    }) {
        return respond(415, None);
    }
    if header(&request, "accept").is_none_or(|v| {
        !v.split(',')
            .any(|t| t.trim().split(';').next() == Some("application/json"))
    }) {
        return respond(406, None);
    }
    let Ok(permit) = active.try_acquire_owned() else {
        return respond(503, None);
    };
    if header(&request, "content-length")
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return respond(413, None);
    }
    let mut body = Vec::new();
    while let Some(frame) = request.body_mut().frame().await {
        let Ok(frame) = frame else {
            return respond(400, None);
        };
        if let Ok(data) = frame.into_data() {
            if body.len() + data.len() > MAX_BODY {
                return respond(413, None);
            }
            body.extend_from_slice(&data);
        }
    }
    let message: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return respond(400, Some(error(Value::Null, -32700, "Parse error"))),
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        dispatch(message, &history)
    })
    .await
    {
        Ok((code, result)) => respond(code, result),
        Err(_) => respond(500, Some(error(Value::Null, -32603, "Internal error"))),
    }
}

pub(super) fn respond(code: u16, body: Option<Value>) -> HttpResponse {
    let mut response = Response::builder()
        .status(code)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff");
    if code == 405 {
        response = response.header("Allow", "POST");
    }
    response
        .body(Full::new(Bytes::from(
            body.map(|v| v.to_string()).unwrap_or_default(),
        )))
        .unwrap()
}
