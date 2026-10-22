//! ファイル単位の更新を Store に反映し、画面へ共有するスナップショットを作る。

use crate::model::{Snapshot, Store};

use super::{Indexer, Task, Update};

pub struct Delta {
    pub snapshot: Snapshot,
    pub touched: Vec<usize>,
}

/// データの結合・親子関係と時刻の索引化まで背景で済ませ、画面へ共有する
pub struct ViewLoader {
    indexer: Indexer,
    store: Store,
}

impl ViewLoader {
    pub fn open() -> Self {
        Self::open_with_live_cache(true)
    }

    /// 計測では同じ更新・保存経路を全体解析でも実行できる。
    pub fn open_with_live_cache(cache_live: bool) -> Self {
        Self {
            indexer: Indexer::open_with_live_cache(cache_live),
            store: Store::default(),
        }
    }

    fn apply(&mut self, updates: Vec<Update>) -> Delta {
        let touched = self
            .store
            .replace(updates.into_iter().map(|u| (u.origin, u.items)));
        Delta {
            snapshot: self.store.snapshot(),
            touched,
        }
    }

    pub fn load_stable(&mut self) -> Delta {
        let updates = self.indexer.load_stable();
        self.apply(updates)
    }

    pub fn plan(&mut self) -> Vec<Task> {
        self.indexer.plan()
    }

    pub fn plan_in(&mut self, claude: &std::path::Path, codex: &std::path::Path) -> Vec<Task> {
        self.indexer.plan_in(claude, codex)
    }

    pub fn process(&mut self, tasks: &[Task]) -> Delta {
        let updates = self.indexer.process(tasks);
        self.apply(updates)
    }
}
