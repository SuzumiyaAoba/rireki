//! ログファイルを「安定した履歴」と「リアルタイム（書き込み中）」に分けて扱う
//!
//! - 安定: 最終更新から `LIVE_SECS` 以上経ったファイル。パース結果を索引 DB に保存し、
//!   以後は指紋（mtime + サイズ）が変わらない限り読み直さない。
//! - リアルタイム: 最近更新されたファイル。DB には書かず、変更のたびにメモリ上で読み直す。
//!   更新が止まれば次の同期で安定側に移る。

use std::collections::HashMap;

use std::sync::mpsc;
use std::thread::JoinHandle;

use crate::db::{self, Db};
use crate::loader::{self, Job};
use crate::model::{Loaded, Snapshot, Store};

/// これより新しく更新されたファイルはリアルタイム扱い
pub const LIVE_SECS: f64 = 10.0 * 60.0;
/// 一度に処理するファイル数（進捗表示と UI 反映の単位）
pub const CHUNK: usize = 64;

pub struct Indexer {
    db: Option<Db>,
    /// DB への書き込みは専用スレッドで行い、次のパースと重ねる
    writer: Option<(mpsc::SyncSender<Vec<db::Packed>>, JoinHandle<()>)>,
    /// DB に保存済みのファイルの指紋
    stable: HashMap<String, String>,
    /// メモリ上で読んだリアルタイムファイルの指紋
    live: HashMap<String, String>,
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
        Self { indexer: Indexer::open(), store: Store::default() }
    }

    fn apply(&mut self, updates: Vec<Update>) -> Delta {
        let touched = self.store.replace(updates.into_iter().map(|u| (u.origin, u.items)));
        Delta { snapshot: self.store.snapshot(), touched }
    }

    pub fn load_stable(&mut self) -> Delta {
        let updates = self.indexer.load_stable();
        self.apply(updates)
    }

    pub fn plan(&self) -> Vec<Task> {
        self.indexer.plan()
    }

    pub fn plan_in(&self, claude: &std::path::Path, codex: &std::path::Path) -> Vec<Task> {
        self.indexer.plan_in(claude, codex)
    }

    pub fn process(&mut self, tasks: &[Task]) -> Delta {
        let updates = self.indexer.process(tasks);
        self.apply(updates)
    }
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl Indexer {
    pub fn open() -> Self {
        let db = Db::open().map_err(|e| eprintln!("rireki: 索引 DB を開けません: {e}")).ok();
        let stable = db.as_ref().and_then(|d| d.fingerprints().ok()).unwrap_or_default();
        let writer = db.is_some().then(|| Db::open().ok()).flatten().map(|mut w| {
            // 保存がパースに追いつかない場合も、待ち行列に大量の本文を溜めない
            let (tx, rx) = mpsc::sync_channel::<Vec<db::Packed>>(1);
            let h = std::thread::spawn(move || {
                for batch in rx {
                    if let Err(e) = w.put(&batch) {
                        eprintln!("rireki: 索引 DB への保存に失敗: {e}");
                    }
                }
            });
            (tx, h)
        });
        Self { db, writer, stable, live: HashMap::new() }
    }

    /// DB に保存済みの履歴を読む
    pub fn load_stable(&self) -> Vec<Update> {
        let Some(db) = &self.db else {
            return Vec::new();
        };
        match db.load_all() {
            Ok(v) => v.into_iter().map(|(origin, items)| Update { origin, items }).collect(),
            Err(e) => {
                eprintln!("rireki: 索引 DB の読み込みに失敗: {e}");
                Vec::new()
            }
        }
    }

    /// 読み直しが必要なファイルを列挙する（リアルタイムを先に）
    pub fn plan(&self) -> Vec<Task> {
        self.plan_jobs(loader::discover())
    }

    /// 計測用の固定した探索先でも、同じ更新判定を使う
    pub fn plan_in(&self, claude: &std::path::Path, codex: &std::path::Path) -> Vec<Task> {
        self.plan_jobs(loader::discover_in(claude, codex))
    }

    fn plan_jobs(&self, jobs: Vec<Job>) -> Vec<Task> {
        let t = now();
        let mut live = Vec::new();
        let mut stable = Vec::new();
        for job in jobs {
            let key = job.key();
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
        live.extend(stable);
        live
    }

    /// タスクをパースし、安定ファイルは DB に保存する
    pub fn process(&mut self, tasks: &[Task]) -> Vec<Update> {
        let jobs: Vec<Job> = tasks.iter().map(|t| t.job.clone()).collect();
        let parsed = loader::load(&jobs);

        let mut to_store: Vec<(String, String, &[Loaded])> = Vec::new();
        for (t, items) in tasks.iter().zip(&parsed) {
            let key = t.job.key();
            if t.live {
                self.live.insert(key, t.job.fingerprint.clone());
            } else {
                self.live.remove(&key);
                to_store.push((key, t.job.fingerprint.clone(), items.as_slice()));
            }
        }
        if let Some((tx, _)) = &self.writer
            && !to_store.is_empty()
        {
            let _ = tx.send(db::pack_files(&to_store));
        }
        for (key, fp, _) in &to_store {
            self.stable.insert(key.clone(), fp.clone());
        }

        tasks.iter().zip(parsed).map(|(t, items)| Update { origin: t.job.key(), items }).collect()
    }
}

impl Drop for Indexer {
    /// 書き込み待ちのデータを DB に書き終えてから閉じる
    fn drop(&mut self) {
        if let Some((tx, h)) = self.writer.take() {
            drop(tx);
            let _ = h.join();
        }
    }
}
