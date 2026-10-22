//! GUI・TUI・CLI が共有する履歴の読み込みと差分更新。
//!
//! [`History`] は索引と Store を所有する。起動時に [`History::load_cached`]、
//! 以後は [`History::plan`] と [`History::ingest`] を呼び、返されたスナップショットを
//! 一覧・検索・詳細・統計に渡す。実行スレッド、タイマー、結果の配送は呼び出し元が選ぶ。
//! 取り込みを中断するときはイテレータを破棄する。未処理のファイルは次回の探索で再計画される。
//!
//! ```no_run
//! use rireki::catalog::{Catalog, SourceFilter};
//! use rireki::history::History;
//!
//! let mut history = History::open();
//! let cached = history.load_cached();
//! let mut snapshot = cached.delta.snapshot;
//! let tasks = history.plan();
//! for batch in history.ingest(tasks) {
//!     // フロントエンドは進捗と共有スナップショットを受け取る。
//!     snapshot = batch.delta.snapshot;
//! }
//! let catalog = Catalog::new(&snapshot, SourceFilter::All, None);
//! for &si in &catalog.order {
//!     println!("{}", snapshot.sessions[si].title);
//! }
//! // ライブ更新では同じ History に対して plan → ingest を繰り返す。
//! ```

use std::iter::FusedIterator;
use std::path::Path;
use std::time::Duration;

use crate::index::{CHUNK, Indexer, Task, Update};
use crate::model::{Snapshot, Store};

/// フロントエンドがライブ更新を確認する標準間隔。コア自身はタイマーを起動しない。
pub const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// 一つの更新単位。同じ Snapshot の添字を検索・詳細の選択にも使う。
pub struct Delta {
    pub snapshot: Snapshot,
    pub touched: Vec<usize>,
}

/// 起動時のキャッシュ読み込み結果。
pub struct Cached {
    pub delta: Delta,
    pub files: usize,
}

/// 最大 [`CHUNK`] ファイルを反映した結果と、その取り込み中の進捗。
pub struct Batch {
    pub delta: Delta,
    pub done: usize,
    pub total: usize,
}

/// 履歴の書き込み側。読み取り側は [`Self::snapshot`] の共有された版を使う。
pub struct History {
    indexer: Indexer,
    store: Store,
}

impl History {
    pub fn open() -> Self {
        Self::open_with_live_cache(true)
    }

    /// 計測では同じ更新・保存経路を全体解析でも実行できる。
    pub fn open_with_live_cache(cache_live: bool) -> Self {
        Self::with_indexer(Indexer::open_with_live_cache(cache_live))
    }

    /// 保存なしの利用・テストでは [`Indexer::in_memory`] を渡せる。
    pub fn with_indexer(indexer: Indexer) -> Self {
        Self {
            indexer,
            store: Store::default(),
        }
    }

    fn apply(&mut self, updates: Vec<Update>) -> Delta {
        let touched = self
            .store
            .replace(updates.into_iter().map(|u| (u.origin, u.items)));
        Delta {
            snapshot: self.snapshot(),
            touched,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.store.snapshot()
    }

    /// 差分探索の前に、保存済みの履歴を読み込む。
    pub fn load_cached(&mut self) -> Cached {
        let updates = self.indexer.load_stable();
        let files = updates.len();
        Cached {
            delta: self.apply(updates),
            files,
        }
    }

    /// 既存の [`crate::index::ViewLoader`] 利用側向けの読み込み API。
    pub fn load_stable(&mut self) -> Delta {
        self.load_cached().delta
    }

    pub fn plan(&mut self) -> Vec<Task> {
        self.indexer.plan()
    }

    /// 実際のホームや環境変数を変えず、探索するログルートを明示する。
    pub fn plan_in(&mut self, claude: &Path, codex: &Path) -> Vec<Task> {
        self.indexer.plan_in(claude, codex)
    }

    /// 一つの処理単位を反映する。通常はバッチ上限を保つ [`Self::ingest`] を使う。
    pub fn process(&mut self, tasks: &[Task]) -> Delta {
        let updates = self.indexer.process(tasks);
        self.apply(updates)
    }

    /// 探索結果を遅延処理する。各 `next` は同期的に解析し、最大 64 ファイルを公開する。
    /// 空の探索結果では更新を返さず、Store と Snapshot の版を維持する。
    pub fn ingest(&mut self, tasks: Vec<Task>) -> Ingest<'_> {
        Ingest {
            history: self,
            total: tasks.len(),
            done: 0,
            tasks: tasks.into_iter(),
        }
    }
}

/// 実行環境に依存しない取り込みイテレータ。破棄した時点で次のバッチの解析を止める。
pub struct Ingest<'a> {
    history: &'a mut History,
    tasks: std::vec::IntoIter<Task>,
    done: usize,
    total: usize,
}

impl Iterator for Ingest<'_> {
    type Item = Batch;

    fn next(&mut self) -> Option<Self::Item> {
        if self.tasks.len() == 0 {
            return None;
        }
        let chunk: Vec<_> = self.tasks.by_ref().take(CHUNK).collect();
        self.done += chunk.len();
        Some(Batch {
            delta: self.history.process(&chunk),
            done: self.done,
            total: self.total,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.tasks.len().div_ceil(CHUNK);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Ingest<'_> {}
impl FusedIterator for Ingest<'_> {}
