//! ログファイルを「安定した履歴」と「リアルタイム（書き込み中）」に分けて扱う
//!
//! - 安定: 最終更新から `LIVE_SECS` 以上経ったファイル。パース結果を索引 DB に保存し、
//!   以後は指紋（mtime + サイズ）が変わらない限り読み直さない。
//! - リアルタイム: 最近更新されたファイル。既読部分を照合し、追記分だけを解析する。
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
/// 生の解析状態を保持する上限。古いものから破棄し、次回は全体解析に戻す。
const LIVE_CACHE_BYTES: usize = 64 * 1024 * 1024;

struct CachedLive {
    log: loader::LiveLog,
    bytes: usize,
    used: u64,
}

pub struct Indexer {
    db: Option<Db>,
    /// DB への書き込みは専用スレッドで行い、次のパースと重ねる
    writer: Option<Writer>,
    /// DB に保存済みのファイルの指紋
    stable: HashMap<String, String>,
    /// メモリ上で読んだリアルタイムファイルの指紋
    live: HashMap<String, String>,
    pending: HashMap<String, String>,
    live_parsers: HashMap<String, CachedLive>,
    live_bytes: usize,
    live_tick: u64,
    cache_live: bool,
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
        Self::open_with_live_cache(true)
    }

    /// 計測では同じ更新・保存経路を全体解析でも実行できる。
    pub fn open_with_live_cache(cache_live: bool) -> Self {
        Self { indexer: Indexer::open_with_live_cache(cache_live), store: Store::default() }
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
        Self::open_with_live_cache(true)
    }

    pub fn open_with_live_cache(cache_live: bool) -> Self {
        let db = Db::open().map_err(|e| eprintln!("rireki: 索引 DB を開けません: {e}")).ok();
        let writer = db.is_some().then(|| Db::open().ok()).flatten();
        let mut indexer = Self::with_db(db, writer);
        indexer.cache_live = cache_live;
        indexer
    }

    pub fn live_cache_bytes(&self) -> usize { self.live_bytes }

    pub fn live_update_stats(&self, job: &Job) -> Option<loader::LiveStats> {
        self.live_parsers.get(&job.key()).map(|cache| cache.log.last_update())
    }

    /// 保存を伴わない計測・テストでも、同じ探索と解析経路を使う。
    pub fn in_memory(cache_live: bool) -> Self {
        let mut indexer = Self::with_db(None, None);
        indexer.cache_live = cache_live;
        indexer
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
        Self { db, writer, stable, live: HashMap::new(), pending: HashMap::new(),
            live_parsers: HashMap::new(), live_bytes: 0, live_tick: 0, cache_live: true }
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

    fn plan_jobs(&mut self, jobs: Vec<Job>) -> Vec<Task> {
        let t = now();
        let mut live = Vec::new();
        let mut stable = Vec::new();
        let mut retained = std::collections::HashSet::new();
        for job in jobs {
            let key = job.key();
            if self.live_parsers.contains_key(&key) { retained.insert(key.clone()); }
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
        self.live_parsers.retain(|key, cache| {
            if retained.contains(key) { true } else { self.live_bytes -= cache.bytes; false }
        });
        live.extend(stable);
        live
    }

    /// タスクをパースし、安定ファイルは DB に保存する
    pub fn process(&mut self, tasks: &[Task]) -> Vec<Update> {
        self.complete_writes();
        let save = self.writer.is_some();
        // 並列で解析した状態を collect に大量に溜めない。既存の保持分は後でLRUで整理する。
        let batch_bytes = std::sync::atomic::AtomicUsize::new(0);
        let states: Vec<_> = tasks.iter().map(|t| {
            let cached = self.live_parsers.remove(&t.job.key()).map(|cache| {
                self.live_bytes -= cache.bytes;
                cache.log
            });
            cached.or_else(|| (t.live && self.cache_live).then(loader::LiveLog::default))
        }).collect();
        let parsed: Vec<_> = tasks.par_iter().zip(states.into_par_iter()).map(|(t, mut state)| {
            let mut items = state.as_mut().map_or_else(|| loader::parse(&t.job), |log| log.parse(&t.job));
            let origin = t.job.key();
            let packed = (save && !t.live).then(|| {
                let packed = db::pack_file(&origin, &t.job.fingerprint, &items);
                packed.release_texts(&mut items);
                packed
            });
            let state = t.live.then_some(state).flatten().and_then(|log| {
                let bytes = log.retained_bytes();
                use std::sync::atomic::Ordering::Relaxed;
                batch_bytes.try_update(Relaxed, Relaxed, |held| {
                    held.checked_add(bytes).filter(|total| *total <= LIVE_CACHE_BYTES)
                }).ok().map(|_| (log, bytes))
            });
            (Update { origin, items }, packed, state)
        }).collect();

        let mut updates = Vec::with_capacity(tasks.len());
        let mut packed = Vec::new();
        for (t, (update, file, state)) in tasks.iter().zip(parsed) {
            if t.live {
                self.pending.remove(&update.origin);
                self.live.insert(update.origin.clone(), t.job.fingerprint.clone());
            } else {
                self.live.remove(&update.origin);
            }
            if let Some(file) = file { packed.push(file); }
            if let Some((log, bytes)) = state { self.cache_live_log(update.origin.clone(), log, bytes); }
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

    fn cache_live_log(&mut self, origin: String, log: loader::LiveLog, bytes: usize) {
        if let Some(old) = self.live_parsers.remove(&origin) { self.live_bytes -= old.bytes; }
        if bytes > LIVE_CACHE_BYTES { return; }
        while self.live_bytes + bytes > LIVE_CACHE_BYTES {
            let Some(oldest) = self.live_parsers.iter().min_by_key(|(_, c)| c.used).map(|(key, _)| key.clone()) else { break };
            self.live_bytes -= self.live_parsers.remove(&oldest).unwrap().bytes;
        }
        self.live_tick = self.live_tick.wrapping_add(1);
        self.live_parsers.insert(origin, CachedLive { log, bytes, used: self.live_tick });
        self.live_bytes += bytes;
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
                EventText::Pending(i, o) => (i, o),
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
        let EventText::Pending(input, output) = s.lanes[0].events[0].text() else { panic!("live text expected") };
        assert_eq!((input, output), ("日本語の入力".into(), String::new()));
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

    #[test]
    fn live_to_stable_keeps_compressed_bodies_on_failed_and_successful_writes() {
        let fixture = Fixture::new();
        let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
        let mut task = task(&fixture);
        task.live = true;
        let live = indexer.process(std::slice::from_ref(&task));
        let Loaded::Session(old) = &live[0].items[0] else { unreachable!() };
        assert!(indexer.live_bytes > 0);
        assert!(indexer.stable.is_empty());
        let expected = ("日本語の入力".to_owned(), String::new());
        let EventText::Pending(i, o) = old.lanes[0].events[0].text() else { panic!("shared live text expected") };
        assert_eq!((i, o), expected);
        fixture.reject_writes(true);
        task.live = false;
        let failed = indexer.process(std::slice::from_ref(&task));
        completion(&mut indexer);
        assert!(indexer.live_parsers.is_empty() && indexer.live_bytes == 0);
        let Loaded::Session(s) = &failed[0].items[0] else { unreachable!() };
        let EventText::Pending(i, o) = s.lanes[0].events[0].text() else { panic!("failed save must keep shared text") };
        assert_eq!((i, o), expected);
        fixture.reject_writes(false);
        let saved = indexer.process(std::slice::from_ref(&task));
        completion(&mut indexer);
        let Loaded::Session(s) = &saved[0].items[0] else { unreachable!() };
        let EventText::Row(id) = s.lanes[0].events[0].text() else { panic!("committed row expected") };
        assert_eq!(Db::open_index_read(&fixture.texts).unwrap().details(&[id])[&id], expected);
        let EventText::Pending(i, o) = old.lanes[0].events[0].text() else { panic!("old snapshot must remain readable") };
        assert_eq!((i, o), expected);
    }

    #[test]
    fn live_cache_budget_eviction_and_removed_files_fall_back_to_full_parsing() {
        let fixture = Fixture::new();
        let mut indexer = Indexer::with_db(None, None);
        let mut first = task(&fixture);
        first.live = true;
        let _ = indexer.process(std::slice::from_ref(&first));
        let old = indexer.live_parsers.remove(&first.job.key()).unwrap();
        indexer.live_bytes -= old.bytes;
        // 小さいログの容量見積もりだけを大きくして、実メモリを大量に確保せず上限到達を再現する。
        indexer.cache_live_log(first.job.key(), old.log, LIVE_CACHE_BYTES / 2 + 1);
        let second_path = fixture.index.with_file_name("second.jsonl");
        std::fs::copy(&first.job.path, &second_path).unwrap();
        let second = Task { live: true, job: Job { path: second_path, ..first.job.clone() } };
        let mut second_cache = loader::LiveLog::default();
        second_cache.parse(&second.job);
        indexer.cache_live_log(second.job.key(), second_cache, LIVE_CACHE_BYTES / 2 + 1);
        assert!(!indexer.live_parsers.contains_key(&first.job.key()));
        assert!(indexer.live_bytes <= LIVE_CACHE_BYTES);
        first.job.fingerprint = "changed".into();
        let updates = indexer.process(std::slice::from_ref(&first));
        let Loaded::Session(s) = &updates[0].items[0] else { unreachable!() };
        let EventText::Pending(i, o) = s.lanes[0].events[0].text() else { panic!("reloaded body expected") };
        assert_eq!((i, o), ("日本語の入力".into(), String::new()));
        indexer.plan_jobs(vec![first.job.clone()]);
        assert!(!indexer.live_parsers.contains_key(&second.job.key()));
        indexer.plan_jobs(Vec::new());
        assert!(indexer.live_parsers.is_empty() && indexer.live_bytes == 0);
        indexer.cache_live_log(first.job.key(), loader::LiveLog::default(), LIVE_CACHE_BYTES + 1);
        assert!(indexer.live_parsers.is_empty());
    }
}
