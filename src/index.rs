//! ログファイルを「安定した履歴」と「リアルタイム（書き込み中）」に分けて扱う
//!
//! - 安定: 最終更新から `LIVE_SECS` 以上経ったファイル。パース結果を索引 DB に保存し、
//!   以後は指紋（mtime + サイズ）が変わらない限り読み直さない。
//! - リアルタイム: 最近更新されたファイル。DB には書かず、変更のたびにメモリ上で読み直す。
//!   更新が止まれば次の同期で安定側に移る。

use std::collections::HashMap;

use std::sync::mpsc;
use std::thread::JoinHandle;

use rayon::prelude::*;

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
    writer: Option<Writer>,
    /// DB に保存済みのファイルの指紋
    stable: HashMap<String, String>,
    /// メモリ上で読んだリアルタイムファイルの指紋
    live: HashMap<String, String>,
    pending: HashMap<String, String>,
}

struct Completion {
    files: Vec<(String, String)>,
    saved: bool,
}

struct Writer {
    sender: mpsc::SyncSender<Vec<db::Packed>>,
    completed: mpsc::Receiver<Completion>,
    handle: JoinHandle<()>,
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

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl Indexer {
    pub fn open() -> Self {
        let db = Db::open().map_err(|e| eprintln!("rireki: 索引 DB を開けません: {e}")).ok();
        let writer = db.is_some().then(|| Db::open().ok()).flatten();
        Self::with_db(db, writer)
    }

    fn with_db(db: Option<Db>, writer: Option<Db>) -> Self {
        let stable = db.as_ref().and_then(|d| d.fingerprints().ok()).unwrap_or_default();
        let writer = writer.map(|mut w| {
            // 保存がパースに追いつかない場合も、待ち行列に大量の本文を溜めない
            let (tx, rx) = mpsc::sync_channel::<Vec<db::Packed>>(1);
            let (done, completed) = mpsc::channel();
            let h = std::thread::spawn(move || {
                for batch in rx {
                    let result = w.put(&batch);
                    if let Err(e) = &result {
                        eprintln!("rireki: 索引 DB への保存に失敗: {e}");
                    }
                    let _ = done.send(Completion {
                        files: batch.iter().map(|p| (p.path.clone(), p.fingerprint.clone())).collect(),
                        saved: result.is_ok(),
                    });
                }
            });
            Writer { sender: tx, completed, handle: h }
        });
        Self { db, writer, stable, live: HashMap::new(), pending: HashMap::new() }
    }

    fn complete_writes(&mut self) {
        let mut disconnected = false;
        while let Some(result) = self.writer.as_ref().map(|w| w.completed.try_recv()) {
                let completion = match result {
                    Ok(completion) => completion,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => { disconnected = true; break; }
                };
                self.finish_write(completion);
        }
        if disconnected {
            self.pending.clear();
            if let Some(writer) = self.writer.take() {
                drop(writer.sender);
                let _ = writer.handle.join();
            }
        }
    }

    fn finish_write(&mut self, completion: Completion) {
        for (path, fp) in completion.files {
            if self.pending.get(&path) == Some(&fp) {
                self.pending.remove(&path);
                if completion.saved { self.stable.insert(path, fp); }
            }
        }
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
    pub fn plan(&mut self) -> Vec<Task> {
        self.complete_writes();
        self.plan_jobs(loader::discover())
    }

    /// 計測用の固定した探索先でも、同じ更新判定を使う
    pub fn plan_in(&mut self, claude: &std::path::Path, codex: &std::path::Path) -> Vec<Task> {
        self.complete_writes();
        self.plan_jobs(loader::discover_in(claude, codex))
    }

    fn plan_jobs(&self, jobs: Vec<Job>) -> Vec<Task> {
        let t = now();
        let mut live = Vec::new();
        let mut stable = Vec::new();
        for job in jobs {
            let key = job.key();
            if self.pending.get(&key) == Some(&job.fingerprint) { continue; }
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
        self.complete_writes();
        let save = self.writer.is_some();
        let parsed: Vec<_> = tasks.par_iter().map(|t| {
            let mut items = loader::parse(&t.job);
            let origin = t.job.key();
            let packed = (save && !t.live).then(|| {
                let packed = db::pack_file(&origin, &t.job.fingerprint, &items);
                packed.release_texts(&mut items);
                packed
            });
            (Update { origin, items }, packed)
        }).collect();

        let mut updates = Vec::with_capacity(tasks.len());
        let mut packed = Vec::new();
        for (t, (update, file)) in tasks.iter().zip(parsed) {
            if t.live {
                self.pending.remove(&update.origin);
                self.live.insert(update.origin.clone(), t.job.fingerprint.clone());
            } else {
                self.live.remove(&update.origin);
            }
            if let Some(file) = file { packed.push(file); }
            updates.push(update);
        }
        if let Some(writer) = &self.writer
            && !packed.is_empty()
        {
            let files: Vec<_> = packed.iter().map(|p| (p.path.clone(), p.fingerprint.clone())).collect();
            if writer.sender.send(packed).is_ok() { self.pending.extend(files); }
        }
        updates
    }

}

impl Drop for Indexer {
    /// 書き込み待ちのデータを DB に書き終えてから閉じる
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            drop(writer.sender);
            let _ = writer.handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fixtures::Fixture;
    use crate::model::EventText;

    fn task(fixture: &Fixture) -> Task {
        let path = fixture.index.with_file_name("log.jsonl");
        std::fs::write(&path, concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session\"}}\n",
            "{\"type\":\"response_item\",\"timestamp\":\"2026-10-05T00:00:00Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"日本語の入力\"}]}}"
        )).unwrap();
        Task { live: false, job: Job { kind: loader::JobKind::Codex, path, mtime: 0.0, fingerprint: "first".into() } }
    }

    fn completion(indexer: &mut Indexer) {
        let done = indexer.writer.as_ref().unwrap().completed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        indexer.finish_write(done);
    }

    #[test]
    fn mixed_live_and_stable_tasks_keep_order_and_retry_only_failed_saves() {
        let fixture = Fixture::new();
        let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
        let tasks: Vec<_> = (0..3).map(|i| {
            let path = fixture.index.with_file_name(format!("{i}.jsonl"));
            std::fs::write(&path, format!("{}\n{}\n",
                serde_json::json!({"type":"session_meta", "payload":{"id":format!("session-{i}")}}),
                serde_json::json!({"type":"response_item", "timestamp":"2026-10-05T00:00:00Z",
                    "payload":{"type":"message", "role":"user", "content":[{"text":format!("入力 {i}")}]}}))).unwrap();
            Task { live: i == 1, job: Job { kind: loader::JobKind::Codex, path,
                mtime: if i == 1 { now() } else { 0.0 }, fingerprint: format!("fp-{i}") } }
        }).collect();
        fixture.reject_writes(true);
        let updates = indexer.process(&tasks);
        completion(&mut indexer);
        assert!(indexer.stable.is_empty() && indexer.pending.is_empty());
        for (i, (t, u)) in tasks.iter().zip(&updates).enumerate() {
            assert_eq!(u.origin, t.job.key());
            let Loaded::Session(s) = &u.items[0] else { unreachable!() };
            assert_eq!(s.id, format!("session-{i}"));
            let e = &s.lanes[0].events[0];
            let (input, output) = match e.text() {
                EventText::Inline(i, o) if t.live => (i.to_owned(), o.to_owned()),
                EventText::Pending(i, o) if !t.live => (i, o),
                _ => panic!("unexpected body state"),
            };
            assert_eq!((input, output), (format!("入力 {i}"), String::new()));
        }
        assert_eq!(indexer.live.get(&tasks[1].job.key()), Some(&tasks[1].job.fingerprint));
        let retry = indexer.plan_jobs(tasks.iter().map(|t| t.job.clone()).collect());
        assert_eq!(retry.iter().map(|t| t.job.key()).collect::<Vec<_>>(), vec![tasks[0].job.key(), tasks[2].job.key()]);
        fixture.reject_writes(false);
        let saved = indexer.process(&retry);
        completion(&mut indexer);
        assert_eq!(indexer.stable.len(), 2);
        assert!(indexer.plan_jobs(tasks.iter().map(|t| t.job.clone()).collect()).is_empty());
        let reader = Db::open_index_read(&fixture.texts).unwrap();
        for (u, i) in saved.iter().zip([0, 2]) {
            let Loaded::Session(s) = &u.items[0] else { unreachable!() };
            let EventText::Row(id) = s.lanes[0].events[0].text() else { panic!("saved row expected") };
            assert_eq!(reader.details(&[id])[&id], (format!("入力 {i}"), String::new()));
        }
    }

    #[test]
    fn failed_writes_keep_texts_retry_and_mark_only_successful_fingerprints() {
        let fixture = Fixture::new();
        let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
        let task = task(&fixture);
        fixture.reject_writes(true);
        let updates = indexer.process(std::slice::from_ref(&task));
        completion(&mut indexer);
        assert!(indexer.stable.is_empty() && indexer.pending.is_empty());
        assert_eq!(indexer.plan_jobs(vec![task.job.clone()]).len(), 1);
        let Loaded::Session(s) = &updates[0].items[0] else { unreachable!() };
        let EventText::Pending(i, o) = s.lanes[0].events[0].text() else { panic!("failed save must keep body") };
        assert_eq!((i.as_str(), o.as_str()), ("日本語の入力", ""));
        fixture.reject_writes(false);
        let updates = indexer.process(std::slice::from_ref(&task));
        completion(&mut indexer);
        assert!(indexer.plan_jobs(vec![task.job.clone()]).is_empty());
        assert_eq!(indexer.stable.get(&task.job.key()).map(String::as_str), Some("first"));
        let Loaded::Session(s) = &updates[0].items[0] else { unreachable!() };
        assert!(matches!(s.lanes[0].events[0].text(), EventText::Row(_)));
    }

    #[test]
    fn late_save_completion_does_not_replace_new_live_or_pending_version() {
        let fixture = Fixture::new();
        let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
        let mut task = task(&fixture);
        let _ = indexer.process(std::slice::from_ref(&task));
        let old = indexer.writer.as_ref().unwrap().completed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        task.live = true;
        task.job.fingerprint = "live".into();
        let updates = indexer.process(std::slice::from_ref(&task));
        indexer.finish_write(old);
        assert!(indexer.stable.is_empty());
        assert_eq!(indexer.live.get(&task.job.key()).map(String::as_str), Some("live"));
        let Loaded::Session(s) = &updates[0].items[0] else { unreachable!() };
        assert!(matches!(s.lanes[0].events[0].text(), EventText::Inline(_, _)));
        assert_eq!(s.lanes[0].events[0].input, "日本語の入力");
        indexer.pending.insert(task.job.key(), "newer".into());
        indexer.finish_write(Completion { files: vec![(task.job.key(), "older".into())], saved: true });
        assert_eq!(indexer.pending.get(&task.job.key()).map(String::as_str), Some("newer"));
    }

    #[test]
    fn no_writer_keeps_original_texts_and_does_not_claim_saved() {
        let fixture = Fixture::new();
        let mut indexer = Indexer::with_db(None, None);
        let task = task(&fixture);
        let updates = indexer.process(std::slice::from_ref(&task));
        let Loaded::Session(s) = &updates[0].items[0] else { unreachable!() };
        assert!(matches!(s.lanes[0].events[0].text(), EventText::Inline(_, _)));
        assert_eq!(s.lanes[0].events[0].input, "日本語の入力");
        assert!(indexer.stable.is_empty());
        assert_eq!(indexer.plan_jobs(vec![task.job.clone()]).len(), 1);
    }

    #[test]
    fn disconnected_writer_retries_without_losing_pending_body() {
        let fixture = Fixture::new();
        let task = task(&fixture);
        let mut indexer = Indexer::with_db(None, None);
        let (sender, receiver) = mpsc::sync_channel(1);
        let (done, completed) = mpsc::channel::<Completion>();
        let handle = std::thread::spawn(move || {
            let _ = receiver.recv().unwrap();
            drop(done);
        });
        indexer.writer = Some(Writer { sender, completed, handle });
        let updates = indexer.process(std::slice::from_ref(&task));
        assert!(matches!(indexer.writer.as_ref().unwrap().completed.recv_timeout(std::time::Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)));
        indexer.complete_writes();
        assert!(indexer.writer.is_none() && indexer.pending.is_empty() && indexer.stable.is_empty());
        let Loaded::Session(s) = &updates[0].items[0] else { unreachable!() };
        let EventText::Pending(i, o) = s.lanes[0].events[0].text() else { panic!("terminated writer must keep body") };
        assert_eq!((i.as_str(), o.as_str()), ("日本語の入力", ""));
        assert_eq!(indexer.plan_jobs(vec![task.job.clone()]).len(), 1);
    }
}
