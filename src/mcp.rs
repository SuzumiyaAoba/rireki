//! localhost 限定、読み取り専用の MCP Streamable HTTP サーバー。
//! GUI のスナップショットを共有し、索引の読み込みやログの更新を重複実行しない。

mod schema;
mod query;
mod tools;
mod protocol;
mod transport;
use transport::{handle, respond};

use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::Response;
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::json;
use tokio::sync::{Semaphore, watch};

use crate::model::Snapshot;

pub const DEFAULT_PORT: u16 = 47821;
const VERSIONS: &[&str] = &["2025-03-26", "2025-06-18", "2025-11-25"];
const MAX_BODY: usize = 64 * 1024;
const MAX_REQUESTS: usize = 4;
const MAX_CONNECTIONS: usize = 64;

type HttpResponse = Response<Full<Bytes>>;

struct History {
    snapshot: Snapshot,
    revision: u64,
}

/// アプリが所有するサーバー。破棄すると接続と待受を停止する。
pub struct Server {
    history: Arc<RwLock<History>>,
    stop: watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
    address: SocketAddr,
}

impl Server {
    /// port=0 は OS に空きポートを選ばせる（テスト用）。通常は DEFAULT_PORT を使う。
    pub fn start(snapshot: Snapshot, port: u16) -> io::Result<Self> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        // Unix では既存の接続が残っていても停止後の同じポートへ再バインドできる。
        // Windows の SO_REUSEADDR は他プロセスの待受を奪えるため使用しない。
        #[cfg(unix)]
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())?;
        socket.listen(128)?;
        socket.set_nonblocking(true)?;
        let listener: TcpListener = socket.into();
        let address = listener.local_addr()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let history = Arc::new(RwLock::new(History {
            snapshot,
            revision: 1,
        }));
        let (stop, mut shutdown) = watch::channel(false);
        let worker_history = history.clone();
        let thread = thread::Builder::new().name("rireki-mcp".into()).spawn(move || {
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("nonblocking TCP listener");
                let active = Arc::new(Semaphore::new(MAX_REQUESTS));
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown.changed() => break,
                        Some(_) = connections.join_next(), if !connections.is_empty() => {},
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break; };
                            if connections.len() >= MAX_CONNECTIONS { continue; }
                            let (history, active) = (worker_history.clone(), active.clone());
                            connections.spawn(async move {
                                let service = service_fn(move |request| {
                                    let (history, active) = (history.clone(), active.clone());
                                    async move {
                                        let response = tokio::time::timeout(Duration::from_secs(10), handle(request, address, history, active))
                                            .await.unwrap_or_else(|_| respond(408, None));
                                        Ok::<_, Infallible>(response)
                                    }
                                });
                                let _ = hyper::server::conn::http1::Builder::new()
                                    .timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5))
                                    .max_buf_size(16 * 1024)
                                    .serve_connection(TokioIo::new(stream), service).await;
                            });
                        }
                    }
                }
                // リスナーと低速なクライアントを含む全接続を閉じてから停止を完了する。
                drop(listener);
                connections.abort_all();
                while connections.join_next().await.is_some() {}
            });
            // 検索中のブロッキング処理の終了は GUI の停止操作を待たせない。
            runtime.shutdown_background();
        })?;
        Ok(Self {
            history,
            stop,
            thread: Some(thread),
            address,
        })
    }

    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }

    /// スナップショットの交換だけを同期する。検索や本文の取得中に GUI の更新をロックしない。
    pub fn update(&self, snapshot: Snapshot) {
        let mut history = self.history.write().unwrap();
        history.snapshot = snapshot;
        history.revision += 1;
    }

    /// mcpServers 形式の Streamable HTTP クライアント向け設定。
    pub fn client_config(&self) -> String {
        serde_json::to_string_pretty(
            &json!({"mcpServers":{"rireki":{"type":"http","url":self.url()}}}),
        )
        .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.history.write().unwrap().snapshot = Snapshot::default();
    }
}
