//! ログファイルを「安定した履歴」と「リアルタイム（書き込み中）」に分けて扱う
//!
//! - 安定: 最終更新から `LIVE_SECS` 以上経ったファイル。パース結果を索引 DB に保存し、
//!   以後は指紋（mtime + サイズ）が変わらない限り読み直さない。
//! - リアルタイム: 最近更新されたファイル。既読部分を照合し、追記分だけを解析する。
//!   更新が止まれば次の同期で安定側に移る。
//!
//! このモジュールは探索・取り込みと指紋の更新を調整する。非同期保存は `writer`、
//! 解析状態の容量管理は `live_cache`、Store とスナップショットの更新は [`crate::history`] が担当する。

use std::collections::HashMap;

use std::sync::mpsc;

use rayon::prelude::*;

use crate::db::{self, Db};
use crate::loader::{self, Job};
use crate::model::Loaded;

mod live_cache;
#[cfg(test)]
mod tests;
mod writer;

use live_cache::{BatchBudget, LiveCache};
// 既存の利用側との互換性を保つ。新しいフロントエンドは history::History を使う。
pub use crate::history::{Delta, History as ViewLoader};
use writer::{Completion, Writer};

/// これより新しく更新されたファイルはリアルタイム扱い
pub const LIVE_SECS: f64 = 10.0 * 60.0;
/// 一度に処理するファイル数（進捗表示と UI 反映の単位）
pub const CHUNK: usize = 64;
pub struct Indexer {
    db: Option<Db>,
    /// DB への書き込みは専用スレッドで行い、次のパースと重ねる
    writer: Option<Writer>,
    /// DB に保存済みのファイルの指紋
    stable: HashMap<String, String>,
    /// メモリ上で読んだリアルタイムファイルの指紋
    live: HashMap<String, String>,
    /// 保存完了を待つ指紋。成功通知が同じ版を指す場合だけ stable へ移す。
    pending: HashMap<String, String>,
    live_cache: LiveCache,
    cache_live: bool,
}

pub struct Task {
    pub job: Job,
    pub live: bool,
}

/// 1 ファイル分の更新。Store::replace にそのまま渡す
pub struct Update {
    pub origin: String,
    pub items: Vec<Loaded>,
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Indexer {
    pub fn open() -> Self {
        Self::open_with_live_cache(true)
    }

    pub fn open_with_live_cache(cache_live: bool) -> Self {
        let db = Db::open()
            .map_err(|e| eprintln!("rireki: 索引 DB を開けません: {e}"))
            .ok();
        let writer = db.is_some().then(|| Db::open().ok()).flatten();
        let mut indexer = Self::with_db(db, writer);
        indexer.cache_live = cache_live;
        indexer
    }

    pub fn live_cache_bytes(&self) -> usize {
        self.live_cache.bytes()
    }

    pub fn live_update_stats(&self, job: &Job) -> Option<loader::LiveStats> {
        self.live_cache.stats(&job.key())
    }

    /// 保存を伴わない計測・テストでも、同じ探索と解析経路を使う。
    pub fn in_memory(cache_live: bool) -> Self {
        let mut indexer = Self::with_db(None, None);
        indexer.cache_live = cache_live;
        indexer
    }

    fn with_db(db: Option<Db>, writer: Option<Db>) -> Self {
        let stable = db
            .as_ref()
            .and_then(|d| d.fingerprints().ok())
            .unwrap_or_default();
        Self {
            db,
            writer: writer.map(Writer::start),
            stable,
            live: HashMap::new(),
            pending: HashMap::new(),
            live_cache: LiveCache::default(),
            cache_live: true,
        }
    }

    fn complete_writes(&mut self) {
        let mut disconnected = false;
        while let Some(result) = self.writer.as_ref().map(|w| w.completed.try_recv()) {
            let completion = match result {
                Ok(completion) => completion,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            };
            self.finish_write(completion);
        }
        if disconnected {
            self.pending.clear();
            if let Some(writer) = self.writer.take() {
                writer.finish();
            }
        }
    }

    fn finish_write(&mut self, completion: Completion) {
        for (path, fp) in completion.files {
            if self.pending.get(&path) == Some(&fp) {
                self.pending.remove(&path);
                if completion.saved {
                    self.stable.insert(path, fp);
                }
            }
        }
    }

    /// DB に保存済みの履歴を読む
    pub fn load_stable(&self) -> Vec<Update> {
        let Some(db) = &self.db else {
            return Vec::new();
        };
        match db.load_all() {
            Ok(v) => v
                .into_iter()
                .map(|(origin, items)| Update { origin, items })
                .collect(),
            Err(e) => {
                eprintln!("rireki: 索引 DB の読み込みに失敗: {e}");
                Vec::new()
            }
        }
    }

    /// 読み直しが必要なファイルを列挙する（リアルタイムを先に）
    pub fn plan(&mut self) -> Vec<Task> {
        self.complete_writes();
        self.plan_jobs(loader::discover())
    }

    /// 計測用の固定した探索先でも、同じ更新判定を使う
    pub fn plan_in(&mut self, claude: &std::path::Path, codex: &std::path::Path) -> Vec<Task> {
        self.complete_writes();
        self.plan_jobs(loader::discover_in(claude, codex))
    }

    fn plan_jobs(&mut self, jobs: Vec<Job>) -> Vec<Task> {
        let t = now();
        let mut live = Vec::new();
        let mut stable = Vec::new();
        let mut retained = std::collections::HashSet::new();
        for job in jobs {
            let key = job.key();
            if self.live_cache.contains(&key) {
                retained.insert(key.clone());
            }
            if self.pending.get(&key) == Some(&job.fingerprint) {
                continue;
            }
            if t - job.mtime < LIVE_SECS {
                if self.live.get(&key) != Some(&job.fingerprint) {
                    live.push(Task { job, live: true });
                }
            } else if self.stable.get(&key) != Some(&job.fingerprint) {
                stable.push(Task { job, live: false });
            } else if self.live.contains_key(&key) {
                // 安定側に移ったが DB の内容が最新なので、DB 版に差し替えるだけでよい
                stable.push(Task { job, live: false });
            }
        }
        self.live_cache.retain(&retained);
        live.extend(stable);
        live
    }

    /// タスクをパースし、安定ファイルは DB に保存する
    pub fn process(&mut self, tasks: &[Task]) -> Vec<Update> {
        self.complete_writes();
        let save = self.writer.is_some();
        // 並列で解析した状態を collect に大量に溜めない。既存の保持分は後でLRUで整理する。
        let batch_budget = BatchBudget::default();
        let states: Vec<_> = tasks
            .iter()
            .map(|t| {
                let cached = self.live_cache.take(&t.job.key());
                cached.or_else(|| (t.live && self.cache_live).then(loader::LiveLog::default))
            })
            .collect();
        let parsed: Vec<_> = tasks
            .par_iter()
            .zip(states.into_par_iter())
            .map(|(t, mut state)| {
                let mut items = state
                    .as_mut()
                    .map_or_else(|| loader::parse(&t.job), |log| log.parse(&t.job));
                let origin = t.job.key();
                let packed = (save && !t.live).then(|| {
                    let packed = db::pack_file(&origin, &t.job.fingerprint, &items);
                    packed.release_texts(&mut items);
                    packed
                });
                let state = t
                    .live
                    .then_some(state)
                    .flatten()
                    .and_then(|log| batch_budget.retain(log));
                (Update { origin, items }, packed, state)
            })
            .collect();

        let mut updates = Vec::with_capacity(tasks.len());
        let mut packed = Vec::new();
        for (t, (update, file, state)) in tasks.iter().zip(parsed) {
            if t.live {
                self.pending.remove(&update.origin);
                self.live
                    .insert(update.origin.clone(), t.job.fingerprint.clone());
            } else {
                self.live.remove(&update.origin);
            }
            if let Some(file) = file {
                packed.push(file);
            }
            if let Some((log, bytes)) = state {
                self.live_cache.insert(update.origin.clone(), log, bytes);
            }
            updates.push(update);
        }
        if let Some(writer) = &self.writer
            && !packed.is_empty()
        {
            let files: Vec<_> = packed
                .iter()
                .map(|p| (p.path.clone(), p.fingerprint.clone()))
                .collect();
            if writer.sender.send(packed).is_ok() {
                self.pending.extend(files);
            }
        }
        updates
    }
}

impl Drop for Indexer {
    /// 書き込み待ちのデータを DB に書き終えてから閉じる
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            writer.finish();
        }
    }
}
