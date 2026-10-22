use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use rireki::mcp::Server;
use rireki::model::{Event, Kind, Lane, Session, Snapshot, Source, Status, Usage};
use serde_json::{Value, json};

fn event(kind: Kind, name: &str, start: f64, status: Status) -> Event {
    Event {
        start,
        end: start + 2.0,
        kind,
        name: name.into(),
        summary: format!("{name} 日本語").into(),
        input: "あいうえお input".into(),
        output: "tool output".into(),
        call_id: Some("call-1".into()),
        text_ref: Default::default(),
        usage: Usage::default(),
        model: None,
        status,
    }
}

fn history() -> Snapshot {
    let mut main = Lane::new("main");
    main.events = vec![
        event(Kind::User, "User", 10.0, Status::Ok),
        event(Kind::Tool, "exec_command", 20.0, Status::Error),
        event(Kind::Assistant, "Assistant", 30.0, Status::Ok),
    ];
    main.events[2].usage = Usage {
        input: 100,
        cache_read: 50,
        output: 20,
        reasoning: 5,
        ..Usage::default()
    };
    main.events[2].model = Some("unknown-model".into());
    let mut child = Lane::new("child");
    child.spawned_by = Some("call-1".into());
    child.events = vec![event(Kind::Tool, "exec_command", 25.0, Status::Pending)];
    let mut a = Session {
        id: "codex-session".into(),
        source: Source::Codex,
        cwd: "/projects/demo".into(),
        headless: false,
        title: "MCP 開発".into(),
        branch: "master".into(),
        version: "1".into(),
        lanes: vec![main, child],
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    };
    a.finalize();
    let mut b = a.clone();
    b.id = "claude-session".into();
    b.source = Source::Claude;
    b.title = "Other project".into();
    b.cwd = "/projects/other".into();
    b.lanes.truncate(1);
    b.lanes[0].events = vec![event(Kind::Tool, "Read", 50.0, Status::Ok)];
    b.finalize();
    Snapshot {
        sessions: vec![Arc::new(a), Arc::new(b)],
    }
}

fn address(server: &Server) -> SocketAddr {
    server
        .url()
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/mcp")
        .unwrap()
        .parse()
        .unwrap()
}

fn request(server: &Server, method: &str, path: &str, headers: &str, body: &str) -> (u16, String) {
    let address = address(server);
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        // 未読の過大なリクエスト本文を送信中に拒否されると、応答後の TCP 終了が RST になる。
        assert!(
            error.kind() == std::io::ErrorKind::ConnectionReset && response.contains("\r\n\r\n"),
            "{error}"
        );
    }
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut remaining = body;
        let mut decoded = String::new();
        loop {
            let (size, rest) = remaining.split_once("\r\n").unwrap();
            let size = usize::from_str_radix(size.split(';').next().unwrap(), 16).unwrap();
            if size == 0 {
                break;
            }
            decoded.push_str(&rest[..size]);
            remaining = &rest[size + 2..];
        }
        decoded
    } else {
        if let Some(length) = head.lines().find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap())
        }) {
            assert_eq!(body.len(), length, "HTTP response must be complete");
        }
        body.to_string()
    };
    (status, body)
}

fn rpc(server: &Server, message: Value) -> Value {
    let (status, body) = request(server, "POST", "/mcp", "", &message.to_string());
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

fn call_result(server: &Server, name: &str, arguments: Value) -> Value {
    rpc(server, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}))["result"].clone()
}

fn call(server: &Server, name: &str, arguments: Value) -> Value {
    let result = call_result(server, name, arguments);
    assert_eq!(result["isError"], false, "{result}");
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
    text
}

#[test]
fn streamable_http_initialization_discovery_and_notifications() {
    let server = Server::start(history(), 0).unwrap();
    for version in ["2025-03-26", "2025-06-18", "2025-11-25", "unknown"] {
        let result = rpc(
            &server,
            json!({"jsonrpc":"2.0","id":"init","method":"initialize", "params":{
            "protocolVersion":version,"capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        );
        assert_eq!(result["id"], "init");
        assert_eq!(
            result["result"]["protocolVersion"],
            if version == "unknown" {
                "2025-11-25"
            } else {
                version
            }
        );
        assert_eq!(
            result["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
    }
    let (status, body) = request(
        &server,
        "POST",
        "/mcp",
        "",
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
    );
    assert_eq!((status, body.as_str()), (202, ""));
    let result = rpc(
        &server,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let tools = result["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
    assert_eq!(
        rpc(&server, json!({"jsonrpc":"2.0","id":3,"method":"ping"}))["result"],
        json!({})
    );
    assert_eq!(request(&server, "GET", "/mcp", "", "").0, 405);
    assert_eq!(request(&server, "DELETE", "/mcp", "", "").0, 405);
}

#[test]
fn sessions_filters_parent_lanes_and_pagination() {
    let server = Server::start(history(), 0).unwrap();
    let first = call(&server, "list_sessions", json!({"limit":1}));
    assert_eq!(first["total"], 2);
    assert_eq!(first["sessions"][0]["id"], "claude-session");
    assert_eq!(first["next_offset"], 1);
    let second = call(
        &server,
        "list_sessions",
        json!({"offset":1,"limit":1,"expected_revision":first["revision"]}),
    );
    assert_eq!(second["sessions"][0]["id"], "codex-session");
    assert_eq!(second["next_offset"], Value::Null);
    let filtered = call(
        &server,
        "list_sessions",
        json!({"source":"codex","cwd":"/projects/demo","query":"mcp 開発","after":15,"before":35}),
    );
    assert_eq!(filtered["total"], 1);
    let session = call(
        &server,
        "get_session",
        json!({"session_id":"codex-session"}),
    );
    assert_eq!(session["event_count"], 4);
    assert_eq!(session["lanes"][1]["parent"], 0);
    assert_eq!(session["analysis"]["events"], 4);
}

#[test]
fn event_order_search_body_limits_and_indices() {
    let server = Server::start(history(), 0).unwrap();
    let result = call(
        &server,
        "get_events",
        json!({"session_id":"codex-session","limit":2}),
    );
    assert_eq!(result["total"], 4);
    assert_eq!(result["events"][0]["kind"], "user");
    assert!(result["events"][0].get("input").is_none());
    let page = call(
        &server,
        "get_events",
        json!({"session_id":"codex-session","offset":2,"include_text":true,"text_limit":3}),
    );
    assert_eq!(page["events"][0]["lane"], 1);
    assert_eq!(page["events"][0]["input"], "あいう");
    assert_eq!(page["events"][0]["input_truncated"], true);
    assert_eq!(page["events"][0]["text_available"], true);
    let search = call(
        &server,
        "search_events",
        json!({"query":"EXEC 日本語","order":"newest","status":"pending"}),
    );
    assert_eq!(search["total"], 1);
    assert_eq!(search["events"][0]["start"], 25.0);
    let exact = call(
        &server,
        "get_events",
        json!({"session_id":"codex-session","lane":0,"event_index":1,"include_text":true}),
    );
    assert_eq!(exact["total"], 1);
    assert_eq!(exact["events"][0]["status"], "error");
    assert_eq!(exact["events"][0]["output"], "tool output");
    let time = call(
        &server,
        "search_events",
        json!({"query":"日本語","kind":"tool","after":20,"before":25}),
    );
    assert_eq!(time["total"], 2);
}

#[test]
fn bounded_event_heap_matches_full_sort_in_both_orders() {
    let mut snapshot = history();
    let s = Arc::make_mut(&mut snapshot.sessions[0]);
    s.lanes[0].events = (0..500)
        .map(|i| event(Kind::User, "message", (i / 2) as f64, Status::Ok))
        .collect();
    s.lanes[1].events = (0..500)
        .map(|i| event(Kind::Assistant, "reply", (i / 3) as f64, Status::Ok))
        .collect();
    s.finalize();
    let mut expected: Vec<_> = s
        .lanes
        .iter()
        .enumerate()
        .flat_map(|(li, l)| {
            l.events
                .iter()
                .enumerate()
                .map(move |(ei, e)| (e.start, li, ei))
        })
        .collect();
    let server = Server::start(snapshot, 0).unwrap();
    for newest in [false, true] {
        expected.sort_by(|a, b| {
            (if newest {
                b.0.total_cmp(&a.0)
            } else {
                a.0.total_cmp(&b.0)
            })
            .then_with(|| (a.1, a.2).cmp(&(b.1, b.2)))
        });
        let result = call(
            &server,
            "get_events",
            json!({"session_id":"codex-session","offset":450,"limit":100,"order":if newest { "newest" } else { "oldest" }}),
        );
        assert_eq!(result["total"], 1000);
        let actual: Vec<_> = result["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["start"].as_f64().unwrap(),
                    e["lane"].as_u64().unwrap() as usize,
                    e["event_index"].as_u64().unwrap() as usize,
                )
            })
            .collect();
        assert_eq!(actual, expected[450..550]);
    }
}

#[test]
fn analysis_counts_all_lanes_and_respects_event_filters() {
    let server = Server::start(history(), 0).unwrap();
    let all = call(&server, "analyze_history", json!({"source":"codex"}));
    assert_eq!(all["sessions"], 1);
    assert_eq!(all["events"], 4);
    assert_eq!(all["turns"], 1);
    assert_eq!(all["tool_calls"], 2);
    assert_eq!(all["errors"], 1);
    assert_eq!(all["tools"][0]["pending"], 1);
    assert_eq!(all["tools"][0]["completed_seconds"], 2.0);
    assert_eq!(all["usage"]["total"], 170);
    assert_eq!(all["usage"]["reasoning"], 5);
    assert_eq!(all["usage"]["unpriced_calls"], 1);
    let filtered = call(
        &server,
        "analyze_history",
        json!({"status":"failed","query":"exec","before":25}),
    );
    assert_eq!(filtered["events"], 1);
    assert_eq!(filtered["tool_calls"], 1);
}

#[test]
fn search_and_analysis_share_filters_across_sessions_and_lanes() {
    let server = Server::start(history(), 0).unwrap();
    for (filters, expected) in [
        (json!({}), 5),
        (json!({"source":"codex"}), 4),
        (json!({"cwd":"/projects/other"}), 1),
        (json!({"kind":"tool","after":20,"before":25}), 2),
        (json!({"status":"failed"}), 1),
        (json!({"status":"pending","lane":1}), 1),
        (json!({"lane":0,"event_index":2}), 1),
        (json!({"query":"ASSISTANT unknown-MODEL"}), 1),
        (json!({"query":"missing"}), 0),
    ] {
        let mut arguments = json!({"query":"日本語"});
        arguments
            .as_object_mut()
            .unwrap()
            .extend(filters.as_object().unwrap().clone());
        let events = call(&server, "search_events", arguments.clone());
        let analysis = call(&server, "analyze_history", arguments.clone());
        assert_eq!(events["total"], expected, "{arguments}");
        assert_eq!(analysis["events"], expected, "{arguments}");
        let sessions: std::collections::HashSet<_> = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["session_id"].as_str().unwrap())
            .collect();
        assert_eq!(analysis["sessions"], sessions.len(), "{arguments}");
    }
}

#[test]
fn live_snapshot_updates_and_revision_mismatches() {
    let server = Server::start(Snapshot::default(), 0).unwrap();
    let initial = call(&server, "list_sessions", json!({}));
    assert_eq!(initial["total"], 0);
    server.update(history());
    let current = call(&server, "list_sessions", json!({}));
    assert_eq!(current["total"], 2);
    assert_ne!(initial["revision"], current["revision"]);
    assert_eq!(
        call_result(
            &server,
            "list_sessions",
            json!({"expected_revision":initial["revision"]})
        )["isError"],
        true
    );
}

#[test]
fn rejects_remote_origins_host_rebinding_bad_versions_and_oversized_requests() {
    let server = Server::start(history(), 0).unwrap();
    assert!(address(&server).ip().is_loopback());
    assert_eq!(
        request(
            &server,
            "POST",
            "/mcp",
            "Origin: https://example.com\r\n",
            "{}"
        )
        .0,
        403
    );
    // 同じポートでも localhost を含むように見せかけた origin は許可しない。
    let forged = format!(
        "Origin: http://localhost:{}.example.com\r\n",
        address(&server).port()
    );
    assert_eq!(request(&server, "POST", "/mcp", &forged, "{}").0, 403);
    assert_eq!(
        request(
            &server,
            "POST",
            "/mcp",
            "MCP-Protocol-Version: invalid\r\n",
            "{}"
        )
        .0,
        400
    );
    assert_eq!(request(&server, "POST", "/other", "", "{}").0, 404);
    let mut stream = TcpStream::connect(address(&server)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(stream, "POST /mcp HTTP/1.1\r\nHost: attacker.example\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 403"));
    assert_eq!(
        request(&server, "POST", "/mcp", "", &"x".repeat(65537)).0,
        413
    );
    let (status, body) = request(&server, "POST", "/mcp", "", "{");
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],
        -32700
    );
}

#[test]
fn invalid_tool_arguments_and_rpc_requests_return_actionable_errors() {
    let server = Server::start(history(), 0).unwrap();
    for (tool, args) in [
        ("get_session", json!({})),
        ("get_events", json!({"session_id":"missing"})),
        ("search_events", json!({"query":"  "})),
        ("list_sessions", json!({"limit":0})),
        ("list_sessions", json!({"offset":10001})),
        ("list_sessions", json!({"source":"invalid"})),
        (
            "get_events",
            json!({"session_id":"codex-session","text_limit":0}),
        ),
        (
            "get_events",
            json!({"session_id":"codex-session","lane":10}),
        ),
        (
            "get_events",
            json!({"session_id":"codex-session","lane":0,"event_index":999}),
        ),
        (
            "get_events",
            json!({"session_id":"codex-session","event_index":1}),
        ),
        ("analyze_history", json!({"after":30,"before":10})),
        ("list_sessions", json!({"typo":true})),
    ] {
        let result = call_result(&server, tool, args.clone());
        assert_eq!(result["isError"], true, "{tool}: {args}: {result}");
        assert!(!result["content"][0]["text"].as_str().unwrap().is_empty());
    }
    assert_eq!(
        rpc(&server, json!({"jsonrpc":"2.0","id":1,"method":"unknown"}))["error"]["code"],
        -32601
    );
    assert_eq!(
        rpc(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"unknown"}})
        )["error"]["code"],
        -32602
    );
    assert_eq!(request(&server, "POST", "/mcp", "", "[]").0, 400);
}

#[test]
fn stop_releases_listener_and_restart_works_even_with_partial_client_body() {
    let server = Server::start(history(), 0).unwrap();
    let address = address(&server);
    assert!(Server::start(Snapshot::default(), address.port()).is_err());
    let mut slow = TcpStream::connect(address).unwrap();
    write!(slow, "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: 2000\r\n\r\n{{").unwrap();
    // 別の接続も応答でき、停止は遅いクライアントの送信完了を待たない。
    assert_eq!(call(&server, "list_sessions", json!({}))["total"], 2);
    drop(server);
    let restarted = Server::start(history(), address.port()).unwrap();
    assert_eq!(call(&restarted, "list_sessions", json!({}))["total"], 2);
    drop(slow);
}
