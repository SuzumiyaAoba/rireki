//! 統計・詳細分析・MCP の実行器。画面から独立し、要求は種類ごとの最新分にまとめる。

mod jobs;
pub use jobs::{Generations, Job};
use jobs::{RequestKey, UpdateKey};

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
    fn key(&self) -> RequestKey {
        match self {
            Self::Inspect { .. } => RequestKey::Job(Job::Inspect),
            Self::Analysis { .. } => RequestKey::Job(Job::Analysis),
            Self::Metrics { .. } => RequestKey::Job(Job::Metrics),
            Self::Cost { .. } => RequestKey::Job(Job::Cost),
            Self::Mcp { .. } => RequestKey::Mcp,
            Self::Snapshot(_) => RequestKey::Snapshot,
        }
    }
    fn generation(&self) -> Option<(Job, u64)> {
        match self {
            Self::Inspect { generation, .. } => Some((Job::Inspect, *generation)),
            Self::Analysis { generation, .. } => Some((Job::Analysis, *generation)),
            Self::Metrics { generation, .. } => Some((Job::Metrics, *generation)),
            Self::Cost { generation, .. } => Some((Job::Cost, *generation)),
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
    fn key(&self) -> UpdateKey {
        match self {
            Self::Inspect { .. } => UpdateKey::Job(Job::Inspect),
            Self::Analysis { .. } => UpdateKey::Job(Job::Analysis),
            Self::Metrics { .. } => UpdateKey::Job(Job::Metrics),
            Self::Cost { .. } => UpdateKey::Job(Job::Cost),
            Self::Mcp { .. } => UpdateKey::Mcp,
            Self::Error(_) => UpdateKey::Error,
        }
    }
    fn generation(&self) -> Option<(Job, u64)> {
        match self {
            Self::Inspect { generation, .. } => Some((Job::Inspect, *generation)),
            Self::Analysis { generation, .. } => Some((Job::Analysis, *generation)),
            Self::Metrics { generation, .. } => Some((Job::Metrics, *generation)),
            Self::Cost { generation, .. } => Some((Job::Cost, *generation)),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Shared {
    stopped: AtomicBool,
    generations: Generations<AtomicU64>,
    mcp_generation: AtomicU64,
    pending: Mutex<BTreeMap<RequestKey, Request>>,
    completed: Mutex<BTreeMap<UpdateKey, Update>>,
    wake: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

impl Shared {
    fn current(&self, kind: Job, generation: u64) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && self.generations[kind].load(Ordering::Acquire) == generation
    }
    fn publish(&self, update: Update) {
        let mut completed = lock(&self.completed);
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        if let Some((job, generation)) = update.generation()
            && !self.current(job, generation)
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
        if let Some((job, generation)) = request.generation() {
            self.shared.generations[job].store(generation, Ordering::Release);
        }
        if matches!(request, Request::Mcp { .. }) {
            self.shared.mcp_generation.fetch_add(1, Ordering::AcqRel);
        }
        // 旧結果が UI にまだ渡されていない場合も、最新要求で無効にする。
        if let Some(result) = key.result() {
            lock(&self.shared.completed).remove(&result);
        }
        pending.insert(key, request);
        self.shared.wake.notify_all();
    }
    pub fn cancel(&self, kind: Job, generation: u64) {
        let current = &self.shared.generations[kind];
        let mut pending = lock(&self.shared.pending);
        current.store(generation, Ordering::Release);
        pending.remove(&RequestKey::Job(kind));
        lock(&self.shared.completed).remove(&UpdateKey::Job(kind));
    }
    pub fn try_recv(&self) -> Option<Update> {
        let mut completed = lock(&self.shared.completed);
        while let Some((_, update)) = completed.pop_first() {
            if update
                .generation()
                .is_none_or(|(job, generation)| self.shared.current(job, generation))
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
                    let current = || shared.current(Job::Inspect, generation);
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
                    let current = || shared.current(Job::Analysis, generation);
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
                    let current = || shared.current(Job::Metrics, generation);
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
                    let current = || shared.current(Job::Cost, generation);
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
        if let Some(Request::Snapshot(next)) = requests.remove(&RequestKey::Snapshot) {
            snapshot = next;
            if let Some(server) = &server {
                server.update(snapshot.clone());
            }
        }
        if let Some(Request::Mcp { port }) = requests.remove(&RequestKey::Mcp) {
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

fn take_requests(shared: &Shared, controls: bool) -> Option<(BTreeMap<RequestKey, Request>, u64)> {
    let mut pending = lock(&shared.pending);
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return None;
        }
        if pending.keys().any(|key| key.is_control() == controls) {
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
        .filter(|key| key.is_control() == controls)
        .collect();
    let requests = keys
        .into_iter()
        .filter_map(|key| pending.remove(&key).map(|request| (key, request)))
        .collect();
    Some((requests, shared.mcp_generation.load(Ordering::Acquire)))
}

#[cfg(test)]
mod tests;
