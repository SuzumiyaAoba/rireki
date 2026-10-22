//! 履歴と読み取り処理を端末スレッドから分離する。要求・結果は種類ごとに最新一件だけ保持する。

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::detail::{DetailCache, DetailLoader, Selection};
use crate::history::{History, POLL_INTERVAL};
use crate::index::Indexer;
use crate::model::{Session, Snapshot};
use crate::search::{self, Results};

#[derive(Default)]
pub struct Config {
    /// 指定時は既定ログ・索引 DB を開かず、この二つのログルートだけを読む。
    pub roots: Option<(PathBuf, PathBuf)>,
}

pub enum Request {
    Search {
        generation: u64,
        snapshot: Snapshot,
        scope: Vec<usize>,
        query: String,
    },
    Detail {
        generation: u64,
        selection: Option<(Selection, Arc<Session>)>,
    },
    Refresh,
}

pub enum Update {
    History {
        snapshot: Snapshot,
        loading: bool,
        progress: Option<(usize, usize)>,
    },
    Search {
        generation: u64,
        results: Results,
    },
    Detail {
        generation: u64,
        detail: Option<Box<DetailCache>>,
    },
    Error(String),
}

#[derive(Default)]
struct Pending {
    search: Option<Request>,
    detail: Option<Request>,
    refresh: bool,
}

#[derive(Default)]
struct Completed {
    history: Option<Update>,
    search: Option<Update>,
    detail: Option<Update>,
    error: Option<Update>,
}

#[derive(Default)]
struct Shared {
    stopped: AtomicBool,
    search_generation: AtomicU64,
    detail_generation: AtomicU64,
    pending: Mutex<Pending>,
    completed: Mutex<Completed>,
    wake: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    fn stop(&self) {
        // wait の条件確認と同じロックを取って、停止通知が wait の直前に失われるのを防ぐ。
        let _pending = lock(&self.pending);
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    fn publish(&self, update: Update) {
        if self.stopped() {
            return;
        }
        let mut completed = lock(&self.completed);
        match update {
            Update::History { .. } => completed.history = Some(update),
            Update::Search { .. } => completed.search = Some(update),
            Update::Detail { .. } => completed.detail = Some(update),
            Update::Error(_) => completed.error = Some(update),
        }
    }
}

pub struct Worker {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Worker {
    pub fn start(config: Config) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let mut worker = Self {
            shared,
            threads: Vec::new(),
        };
        let persistent = config.roots.is_none();
        worker.spawn("rireki-tui-history", move |shared| {
            history_loop(shared, config)
        })?;
        worker.spawn("rireki-tui-query", move |shared| {
            query_loop(shared, persistent)
        })?;
        Ok(worker)
    }

    fn spawn(&mut self, name: &str, run: impl FnOnce(&Shared) + Send + 'static) -> io::Result<()> {
        let shared = self.shared.clone();
        let name = name.to_string();
        let handle = thread::Builder::new().name(name.clone()).spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&shared))).is_err() {
                shared.publish(Update::Error(format!(
                    "{name}: バックグラウンド処理が停止しました。再起動してください"
                )));
                shared.stop();
            }
        })?;
        self.threads.push(handle);
        Ok(())
    }

    pub fn request(&self, request: Request) {
        if self.shared.stopped() {
            return;
        }
        let mut pending = lock(&self.shared.pending);
        match request {
            Request::Search { generation, .. } => {
                self.shared
                    .search_generation
                    .store(generation, Ordering::Release);
                pending.search = Some(request);
            }
            Request::Detail { generation, .. } => {
                self.shared
                    .detail_generation
                    .store(generation, Ordering::Release);
                pending.detail = Some(request);
            }
            Request::Refresh => pending.refresh = true,
        }
        self.shared.wake.notify_all();
    }

    pub fn try_recv(&self) -> Option<Update> {
        let mut completed = lock(&self.shared.completed);
        completed
            .history
            .take()
            .or_else(|| completed.search.take())
            .or_else(|| completed.detail.take())
            .or_else(|| completed.error.take())
    }

    /// 現在のバッチ終了時に停止する。端末の復元を join で待たせない。
    pub fn stop(&self) {
        self.shared.stop();
    }

    /// 端末を復元した後に呼び、履歴の書き込みスレッドの後処理まで完了させる。
    pub fn finish(mut self) {
        self.stop();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn history_loop(shared: &Shared, config: Config) {
    let mut history = if config.roots.is_some() {
        History::with_indexer(Indexer::in_memory(true))
    } else {
        History::open()
    };
    if shared.stopped() {
        return;
    }
    let snapshot = if config.roots.is_none() {
        history.load_cached().delta.snapshot
    } else {
        history.snapshot()
    };
    shared.publish(Update::History {
        snapshot,
        loading: true,
        progress: None,
    });
    while !shared.stopped() {
        let tasks = match &config.roots {
            Some((claude, codex)) => history.plan_in(claude, codex),
            None => history.plan(),
        };
        let total = tasks.len();
        if total > 0 {
            shared.publish(Update::History {
                snapshot: history.snapshot(),
                loading: true,
                progress: Some((0, total)),
            });
        }
        let mut batches = history.ingest(tasks);
        while !shared.stopped() {
            let Some(batch) = batches.next() else { break };
            shared.publish(Update::History {
                snapshot: batch.delta.snapshot,
                loading: batch.done < batch.total,
                progress: Some((batch.done, batch.total)),
            });
        }
        if shared.stopped() {
            break;
        }
        shared.publish(Update::History {
            snapshot: history.snapshot(),
            loading: false,
            progress: Some((total, total)),
        });
        let deadline = Instant::now() + POLL_INTERVAL;
        let mut pending = lock(&shared.pending);
        while !shared.stopped() && !pending.refresh {
            let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let (next, _) = shared
                .wake
                .wait_timeout(pending, wait)
                .unwrap_or_else(|e| e.into_inner());
            pending = next;
        }
        pending.refresh = false;
    }
}

fn query_loop(shared: &Shared, persistent: bool) {
    let mut index = search::Index::default();
    let mut loader = DetailLoader::default();
    if persistent {
        loader.prepare();
    }
    loop {
        let (detail, search) = {
            let mut pending = lock(&shared.pending);
            while !shared.stopped() && pending.search.is_none() && pending.detail.is_none() {
                pending = shared.wake.wait(pending).unwrap_or_else(|e| e.into_inner());
            }
            if shared.stopped() {
                return;
            }
            (pending.detail.take(), pending.search.take())
        };
        if let Some(Request::Detail {
            generation,
            selection,
        }) = detail
        {
            let current = || {
                !shared.stopped() && shared.detail_generation.load(Ordering::Acquire) == generation
            };
            if current() {
                let detail = selection.and_then(|(selection, session)| {
                    loader.load_session(selection, &session, current)
                });
                if current() {
                    shared.publish(Update::Detail {
                        generation,
                        detail: detail.map(Box::new),
                    });
                }
            }
        }
        if let Some(Request::Search {
            generation,
            snapshot,
            scope,
            query,
        }) = search
        {
            let current = || {
                !shared.stopped() && shared.search_generation.load(Ordering::Acquire) == generation
            };
            if let Some(results) =
                index.search_while(&snapshot.sessions, &scope, &query, 500, current)
                && current()
            {
                shared.publish(Update::Search {
                    generation,
                    results,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[test]
    fn pending_and_completed_are_coalesced_without_losing_other_kinds() {
        let worker = Worker {
            shared: Arc::default(),
            threads: Vec::new(),
        };
        for generation in 1..=1000 {
            worker.request(Request::Search {
                generation,
                snapshot: Snapshot::default(),
                scope: vec![],
                query: generation.to_string(),
            });
            worker.request(Request::Detail {
                generation,
                selection: None,
            });
            worker.shared.publish(Update::Search {
                generation,
                results: Results::default(),
            });
            worker.shared.publish(Update::Detail {
                generation,
                detail: None,
            });
        }
        worker.request(Request::Refresh);
        let pending = lock(&worker.shared.pending);
        assert!(pending.refresh);
        assert!(matches!(
            pending.search,
            Some(Request::Search {
                generation: 1000,
                ..
            })
        ));
        assert!(matches!(
            pending.detail,
            Some(Request::Detail {
                generation: 1000,
                ..
            })
        ));
        drop(pending);
        assert!(matches!(
            worker.try_recv(),
            Some(Update::Search {
                generation: 1000,
                ..
            })
        ));
        assert!(matches!(
            worker.try_recv(),
            Some(Update::Detail {
                generation: 1000,
                ..
            })
        ));
        assert!(worker.try_recv().is_none());
    }

    #[test]
    fn explicit_roots_support_live_refresh_search_and_detail_without_database() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rireki-tui-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let claude = root.join("claude");
        let codex = root.join("codex");
        std::fs::create_dir_all(claude.join("project")).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        let path = claude.join("project/session.jsonl");
        let row = serde_json::json!({"type":"user","timestamp":"2026-10-05T00:00:05Z","sessionId":"tui-fixture","cwd":"/workspace/test","message":{"role":"user","content":"日本語 TUI fixture"}});
        std::fs::write(&path, format!("{row}\n")).unwrap();
        let worker = Worker::start(Config {
            roots: Some((claude, codex)),
        })
        .unwrap();
        let wait = |accept: &dyn Fn(&Update) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(update) = worker.try_recv()
                    && accept(&update)
                {
                    break update;
                }
                assert!(Instant::now() < deadline, "worker result timed out");
                thread::sleep(Duration::from_millis(5));
            }
        };
        let Update::History { snapshot, .. } = wait(
            &|u| matches!(u, Update::History { snapshot, loading: false, .. } if !snapshot.sessions.is_empty()),
        ) else {
            unreachable!()
        };
        assert_eq!(snapshot.sessions[0].id, "tui-fixture");
        worker.request(Request::Search {
            generation: 7,
            snapshot: snapshot.clone(),
            scope: vec![0],
            query: "日本語".into(),
        });
        let Update::Search { results, .. } =
            wait(&|u| matches!(u, Update::Search { generation: 7, .. }))
        else {
            unreachable!()
        };
        assert_eq!(results.total, 1);
        worker.request(Request::Detail {
            generation: 9,
            selection: Some((
                Selection {
                    session: 0,
                    lane: 0,
                    first: 0,
                    last: 0,
                },
                snapshot.sessions[0].clone(),
            )),
        });
        let Update::Detail {
            detail: Some(detail),
            ..
        } = wait(&|u| matches!(u, Update::Detail { generation: 9, .. }))
        else {
            unreachable!()
        };
        assert!(
            detail.texts[0].0.contains("日本語 TUI fixture")
                || detail.texts[0].1.contains("日本語 TUI fixture")
        );
        let next = serde_json::json!({"type":"user","timestamp":"2026-10-05T00:00:06Z","sessionId":"tui-fixture","cwd":"/workspace/test","message":{"role":"user","content":"追記されたイベント"}});
        std::fs::write(&path, format!("{row}\n{next}\n")).unwrap();
        // 手動更新を送らず、通常の 3 秒ポーリングによる追記の反映を確認する。
        let Update::History { snapshot, .. } = wait(
            &|u| matches!(u, Update::History { snapshot, .. } if snapshot.sessions.first().is_some_and(|s| s.lanes[0].events.len() == 2)),
        ) else {
            unreachable!()
        };
        assert_eq!(
            snapshot.sessions[0].lanes[0].events[1].summary.as_ref(),
            "追記されたイベント"
        );
        worker.finish();
        std::fs::remove_dir_all(root).unwrap();
    }
}
