//! 統計・詳細分析・MCP の実行器。画面から独立し、要求は種類ごとの最新分にまとめる。

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use rireki::cost_timeline;
use rireki::detail::{DetailCache, DetailLoader, Selection, analysis};
use rireki::model::{Session, Snapshot};

use super::metrics;

pub enum Request {
    Inspect {
        generation: u64,
        selection: Selection,
        session: Arc<Session>,
    },
    Analysis {
        generation: u64,
        selection: Selection,
        session: Arc<Session>,
        filter: analysis::Filter,
        order: analysis::Order,
    },
    Metrics {
        generation: u64,
        snapshot: Snapshot,
        scope: Vec<usize>,
        params: metrics::Params,
    },
    Cost {
        generation: u64,
        snapshot: Snapshot,
        scope: Vec<usize>,
        view: cost_timeline::View,
        in_view_only: bool,
    },
    Mcp {
        port: Option<u16>,
    },
    Snapshot(Snapshot),
}

impl Request {
    fn key(&self) -> u8 {
        match self {
            Self::Inspect { .. } => 0,
            Self::Analysis { .. } => 1,
            Self::Metrics { .. } => 2,
            Self::Cost { .. } => 3,
            Self::Mcp { .. } => 4,
            Self::Snapshot(_) => 5,
        }
    }
    fn generation(&self) -> Option<u64> {
        match self {
            Self::Inspect { generation, .. }
            | Self::Analysis { generation, .. }
            | Self::Metrics { generation, .. }
            | Self::Cost { generation, .. } => Some(*generation),
            _ => None,
        }
    }
}

pub enum Update {
    Inspect {
        generation: u64,
        detail: Option<Box<DetailCache>>,
    },
    Analysis {
        generation: u64,
        analysis: Option<Arc<analysis::Analysis>>,
    },
    Metrics {
        generation: u64,
        computed: Box<metrics::Computed>,
    },
    Cost {
        generation: u64,
        summary: Arc<cost_timeline::Summary>,
    },
    Mcp {
        url: Option<String>,
        config: String,
        error: Option<String>,
    },
    Error(String),
}

impl Update {
    fn key(&self) -> u8 {
        match self {
            Self::Inspect { .. } => 0,
            Self::Analysis { .. } => 1,
            Self::Metrics { .. } => 2,
            Self::Cost { .. } => 3,
            Self::Mcp { .. } => 4,
            Self::Error(_) => 5,
        }
    }
    fn generation(&self) -> Option<u64> {
        match self {
            Self::Inspect { generation, .. }
            | Self::Analysis { generation, .. }
            | Self::Metrics { generation, .. }
            | Self::Cost { generation, .. } => Some(*generation),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Shared {
    stopped: AtomicBool,
    generations: [AtomicU64; 4],
    mcp_generation: AtomicU64,
    pending: Mutex<BTreeMap<u8, Request>>,
    completed: Mutex<BTreeMap<u8, Update>>,
    wake: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

impl Shared {
    fn current(&self, kind: usize, generation: u64) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && self.generations[kind].load(Ordering::Acquire) == generation
    }
    fn publish(&self, update: Update) {
        let mut completed = lock(&self.completed);
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        if let Some(generation) = update.generation()
            && !self.current(usize::from(update.key()), generation)
        {
            return;
        }
        completed.insert(update.key(), update);
    }
    fn publish_mcp(&self, generation: u64, update: Update) {
        let mut completed = lock(&self.completed);
        if !self.stopped.load(Ordering::Acquire)
            && self.mcp_generation.load(Ordering::Acquire) == generation
        {
            completed.insert(update.key(), update);
        }
    }
    fn stop(&self) {
        let _pending = lock(&self.pending);
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_all();
    }
}

pub struct Operations {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Operations {
    pub fn start(persistent: bool) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let mut operations = Self {
            shared,
            threads: Vec::new(),
        };
        operations.spawn("rireki-tui-operations", move |shared| {
            run(shared, persistent)
        })?;
        operations.spawn("rireki-tui-mcp-control", control)?;
        Ok(operations)
    }
    fn spawn(&mut self, name: &str, work: impl FnOnce(&Shared) + Send + 'static) -> io::Result<()> {
        let worker = self.shared.clone();
        let name = name.to_owned();
        let thread = thread::Builder::new().name(name.clone()).spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&worker))).is_err() {
                worker.publish(Update::Error(format!(
                    "{name}: 背景処理が停止しました。再起動してください"
                )));
                worker.stop();
            }
        })?;
        self.threads.push(thread);
        Ok(())
    }
    pub fn request(&self, request: Request) {
        let mut pending = lock(&self.shared.pending);
        if self.shared.stopped.load(Ordering::Acquire) {
            return;
        }
        let key = request.key();
        if let Some(generation) = request.generation() {
            self.shared.generations[usize::from(key)].store(generation, Ordering::Release);
        }
        if matches!(request, Request::Mcp { .. }) {
            self.shared.mcp_generation.fetch_add(1, Ordering::AcqRel);
        }
        // 旧結果が UI にまだ渡されていない場合も、最新要求で無効にする。
        if key < 5 {
            lock(&self.shared.completed).remove(&key);
        }
        pending.insert(key, request);
        self.shared.wake.notify_all();
    }
    pub fn cancel(&self, kind: usize, generation: u64) {
        let Some(current) = self.shared.generations.get(kind) else {
            return;
        };
        let mut pending = lock(&self.shared.pending);
        current.store(generation, Ordering::Release);
        pending.remove(&(kind as u8));
        lock(&self.shared.completed).remove(&(kind as u8));
    }
    pub fn try_recv(&self) -> Option<Update> {
        let mut completed = lock(&self.shared.completed);
        while let Some((key, update)) = completed.pop_first() {
            if update
                .generation()
                .is_none_or(|generation| self.shared.current(usize::from(key), generation))
            {
                return Some(update);
            }
        }
        None
    }
    pub fn stop(&self) {
        self.shared.stop();
    }
    pub fn finish(mut self) {
        self.stop();
        for thread in std::mem::take(&mut self.threads) {
            let _ = thread.join();
        }
    }
}

impl Drop for Operations {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(shared: &Shared, persistent: bool) {
    let mut loader = DetailLoader::default();
    if persistent {
        loader.prepare();
    }
    let mut metrics = metrics::Cache::default();
    let mut costs = cost_timeline::Cache::default();
    loop {
        let Some((requests, _)) = take_requests(shared, false) else {
            break;
        };
        for (_, request) in requests {
            if shared.stopped.load(Ordering::Acquire) {
                break;
            }
            match request {
                Request::Inspect {
                    generation,
                    selection,
                    session,
                } => {
                    let current = || shared.current(0, generation);
                    let detail = loader.load_session(selection, &session, current);
                    if current() {
                        shared.publish(Update::Inspect {
                            generation,
                            detail: detail.map(Box::new),
                        });
                    }
                }
                Request::Analysis {
                    generation,
                    selection,
                    session,
                    filter,
                    order,
                } => {
                    let current = || shared.current(1, generation);
                    let analysis = session
                        .lanes
                        .get(selection.lane)
                        .and_then(|lane| lane.events.get(selection.first..=selection.last))
                        .and_then(|events| {
                            analysis::Analysis::build(events, &filter, order, current)
                        })
                        .map(Arc::new);
                    if current() {
                        shared.publish(Update::Analysis {
                            generation,
                            analysis,
                        });
                    }
                }
                Request::Metrics {
                    generation,
                    snapshot,
                    scope,
                    params,
                } => {
                    let current = || shared.current(2, generation);
                    if let Some(computed) = metrics.build(&snapshot, &scope, params, current)
                        && current()
                    {
                        shared.publish(Update::Metrics {
                            generation,
                            computed: Box::new(computed),
                        });
                    }
                }
                Request::Cost {
                    generation,
                    snapshot,
                    scope,
                    view,
                    in_view_only,
                } => {
                    let current = || shared.current(3, generation);
                    if let Some(summary) =
                        costs.build_while(&snapshot, &scope, view, in_view_only, current)
                        && current()
                    {
                        shared.publish(Update::Cost {
                            generation,
                            summary: Arc::new(summary),
                        });
                    }
                }
                Request::Mcp { .. } | Request::Snapshot(_) => unreachable!(),
            }
        }
    }
}

/// 統計の走査中も MCP の公開・停止・スナップショット交換を待たせない。
fn control(shared: &Shared) {
    let mut snapshot = Snapshot::default();
    let mut server: Option<rireki::mcp::Server> = None;
    while let Some((mut requests, generation)) = take_requests(shared, true) {
        if let Some(Request::Snapshot(next)) = requests.remove(&5) {
            snapshot = next;
            if let Some(server) = &server {
                server.update(snapshot.clone());
            }
        }
        if let Some(Request::Mcp { port }) = requests.remove(&4) {
            if shared.mcp_generation.load(Ordering::Acquire) != generation {
                continue;
            }
            server.take();
            let error = match port {
                Some(port) => match rireki::mcp::Server::start(snapshot.clone(), port) {
                    Ok(started) => {
                        server = Some(started);
                        None
                    }
                    Err(error) => Some(format!("MCP を起動できません: {error}")),
                },
                None => None,
            };
            // 起動中に停止/別ポートへ変更された場合は古い待受と結果を公開しない。
            if shared.stopped.load(Ordering::Acquire)
                || shared.mcp_generation.load(Ordering::Acquire) != generation
            {
                server.take();
                continue;
            }
            shared.publish_mcp(
                generation,
                Update::Mcp {
                    url: server.as_ref().map(|server| server.url()),
                    config: server
                        .as_ref()
                        .map_or_else(String::new, |server| server.client_config()),
                    error,
                },
            );
        }
    }
}

fn take_requests(shared: &Shared, controls: bool) -> Option<(BTreeMap<u8, Request>, u64)> {
    let mut pending = lock(&shared.pending);
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return None;
        }
        if pending.keys().any(|key| (*key >= 4) == controls) {
            break;
        }
        pending = shared
            .wake
            .wait(pending)
            .unwrap_or_else(|error| error.into_inner());
    }
    let keys: Vec<_> = pending
        .keys()
        .copied()
        .filter(|key| (*key >= 4) == controls)
        .collect();
    let requests = keys
        .into_iter()
        .filter_map(|key| pending.remove(&key).map(|request| (key, request)))
        .collect();
    Some((requests, shared.mcp_generation.load(Ordering::Acquire)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rireki::model::{Lane, Source};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    fn session(title: &str) -> Arc<Session> {
        Arc::new(Session {
            id: "operations-fixture".into(),
            source: Source::Codex,
            cwd: "/tmp/operations".into(),
            title: title.into(),
            branch: String::new(),
            version: String::new(),
            headless: false,
            lanes: vec![Lane::new("main")],
            start: 100.0,
            end: 101.0,
            parents: vec![None],
            tree: vec![(0, 0)],
        })
    }

    fn idle() -> Operations {
        Operations {
            shared: Arc::default(),
            threads: Vec::new(),
        }
    }

    fn inspect(generation: u64) -> Request {
        Request::Inspect {
            generation,
            selection: Selection {
                session: 0,
                lane: 0,
                first: 0,
                last: 0,
            },
            session: session("fixture"),
        }
    }

    #[test]
    fn coalescing_cancellation_and_receive_discard_obsolete_results() {
        let operations = idle();
        operations.request(inspect(1));
        operations.request(inspect(2));
        assert_eq!(lock(&operations.shared.pending).len(), 1);
        assert_eq!(
            lock(&operations.shared.pending)
                .get(&0)
                .unwrap()
                .generation(),
            Some(2)
        );
        operations.shared.publish(Update::Inspect {
            generation: 1,
            detail: None,
        });
        assert!(operations.try_recv().is_none());
        operations.shared.publish(Update::Inspect {
            generation: 2,
            detail: None,
        });
        operations.cancel(0, 3);
        assert!(operations.try_recv().is_none());
        assert!(lock(&operations.shared.pending).is_empty());
        operations.shared.publish(Update::Inspect {
            generation: 2,
            detail: None,
        });
        assert!(operations.try_recv().is_none());
        operations.request(inspect(4));
        // 世代照合後に届いた結果も、受信時の照合で落とす。
        lock(&operations.shared.completed).insert(
            0,
            Update::Inspect {
                generation: 3,
                detail: None,
            },
        );
        assert!(operations.try_recv().is_none());
        operations.shared.publish(Update::Inspect {
            generation: 4,
            detail: None,
        });
        assert!(matches!(
            operations.try_recv(),
            Some(Update::Inspect { generation: 4, .. })
        ));
    }

    #[test]
    fn control_queue_keeps_latest_snapshot_and_never_accepts_old_mcp_status() {
        let operations = idle();
        operations.request(inspect(1));
        operations.request(Request::Snapshot(Snapshot {
            sessions: vec![session("old")],
        }));
        operations.request(Request::Snapshot(Snapshot {
            sessions: vec![session("latest")],
        }));
        operations.request(Request::Mcp { port: Some(0) });
        operations.request(Request::Mcp { port: None });
        let (requests, generation) = take_requests(&operations.shared, true).unwrap();
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            requests.get(&4),
            Some(Request::Mcp { port: None })
        ));
        assert!(
            matches!(requests.get(&5), Some(Request::Snapshot(snapshot)) if snapshot.sessions[0].title == "latest")
        );
        assert_eq!(lock(&operations.shared.pending).len(), 1);
        operations.shared.publish_mcp(
            generation - 1,
            Update::Mcp {
                url: Some("old".into()),
                config: String::new(),
                error: None,
            },
        );
        assert!(operations.try_recv().is_none());
        operations.shared.publish_mcp(
            generation,
            Update::Mcp {
                url: None,
                config: String::new(),
                error: None,
            },
        );
        assert!(matches!(
            operations.try_recv(),
            Some(Update::Mcp { url: None, .. })
        ));
        operations.stop();
        assert!(take_requests(&operations.shared, false).is_none());
    }

    fn mcp_status(operations: &Operations) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(update) = operations.try_recv() {
                match update {
                    Update::Mcp { url, config, error } => {
                        assert!(error.is_none(), "{error:?}");
                        if let Some(url) = &url {
                            let config: serde_json::Value = serde_json::from_str(&config).unwrap();
                            assert_eq!(config["mcpServers"]["rireki"]["url"], *url);
                        }
                        return url;
                    }
                    Update::Error(error) => panic!("{error}"),
                    _ => {}
                }
            }
            assert!(Instant::now() < deadline, "MCP 制御が完了しませんでした");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn sessions(url: &str) -> String {
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/mcp")
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#;
        write!(stream, "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        response
    }

    #[test]
    fn mcp_publishes_live_snapshots_and_stops_without_waiting_for_computation_queue() {
        let mut operations = idle();
        // 計算キューを意図的に処理しない。制御ワーカーだけでも公開と停止が完了する。
        operations.spawn("rireki-tui-mcp-test", control).unwrap();
        operations.request(inspect(1));
        operations.request(Request::Snapshot(Snapshot {
            sessions: vec![session("before-refresh")],
        }));
        operations.request(Request::Mcp { port: Some(0) });
        let url = mcp_status(&operations).unwrap();
        assert!(sessions(&url).contains("before-refresh"));
        operations.request(Request::Snapshot(Snapshot {
            sessions: vec![session("after-refresh")],
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sessions(&url).contains("after-refresh") {
            assert!(
                Instant::now() < deadline,
                "MCP に更新が反映されませんでした"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(lock(&operations.shared.pending).len(), 1);
        operations.request(Request::Mcp { port: None });
        assert!(mcp_status(&operations).is_none());
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/mcp")
            .unwrap();
        assert!(TcpStream::connect(address).is_err());
        operations.finish();
    }
}
