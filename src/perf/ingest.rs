//! 固定したログの取り込みと、索引・本文・スナップショットの照合。

use super::fingerprint::{event_hash, items_hash};
#[cfg(target_os = "macos")]
use super::memory;
use crate::model::{EventText, Lane, Loaded, Store};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::time::Instant;

#[derive(serde::Serialize, serde::Deserialize)]
struct IngestJob {
    kind: u8,
    path: std::path::PathBuf,
    mtime: f64,
    fingerprint: String,
}

/// 空の隔離キャッシュを要求し、非公開の manifest で計測対象を固定する
pub(crate) fn ingest_tasks(manifest: &Path) -> Vec<crate::index::Task> {
    let cache = std::env::var_os("RIREKI_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .expect("RIREKI_CACHE_DIR が必要です");
    assert!(
        !cache.exists()
            || std::fs::read_dir(&cache).is_ok_and(|mut entries| entries.next().is_none()),
        "取り込みの計測には空の RIREKI_CACHE_DIR が必要です"
    );
    let discovered = match std::env::var_os("RIREKI_BENCH_LOG_ROOT") {
        Some(root) => {
            let root = std::path::PathBuf::from(root);
            crate::loader::discover_in(&root.join("claude"), &root.join("codex"))
        }
        None => crate::loader::discover(),
    };
    if !manifest.exists() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let jobs: Vec<_> = discovered
            .iter()
            .filter(|j| now - j.mtime >= crate::index::LIVE_SECS)
            .map(|j| IngestJob {
                kind: j.kind as u8,
                path: j.path.clone(),
                mtime: j.mtime,
                fingerprint: j.fingerprint.clone(),
            })
            .collect();
        std::fs::write(manifest, serde_json::to_vec(&jobs).unwrap()).unwrap();
    }
    let jobs: Vec<IngestJob> = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let current: std::collections::HashMap<_, _> =
        discovered.iter().map(|j| (&j.path, j)).collect();
    for job in &jobs {
        assert!(
            current
                .get(&job.path)
                .is_some_and(|j| j.fingerprint == job.fingerprint && j.kind as u8 == job.kind),
            "計測対象のファイルが変更されています。新しい manifest で測り直してください"
        );
    }
    jobs.into_iter()
        .map(|j| crate::index::Task {
            live: false,
            job: crate::loader::Job {
                kind: match j.kind {
                    0 => crate::loader::JobKind::Claude,
                    1 => crate::loader::JobKind::ClaudeLegacyAgent,
                    2 => crate::loader::JobKind::Codex,
                    _ => panic!("未知のログ形式"),
                },
                path: j.path,
                mtime: j.mtime,
                fingerprint: j.fingerprint,
            },
        })
        .collect()
}

/// 空の隔離キャッシュで全取り込みを測る。対象ファイルは非公開の manifest で固定する
pub fn ingest(manifest: &Path) {
    let tasks = ingest_tasks(manifest);
    let mut expected = std::collections::BTreeMap::new();
    // 比較用の元ログの全件走査は取り込み時間に含めず、本文は一ファイルずつ解放する。
    for task in &tasks {
        let items = crate::loader::parse(&task.job);
        if !items.is_empty() {
            expected.insert(task.job.key(), items_hash(&items));
        }
    }
    #[cfg(target_os = "macos")]
    let memory = std::env::args()
        .any(|a| a == "--measure-memory")
        .then(memory::Monitor::start);
    let started = Instant::now();
    let mut indexer = crate::index::Indexer::open();
    let mut store = Store::default();
    for chunk in tasks.chunks(crate::index::CHUNK) {
        let updates = indexer.process(chunk);
        store.replace(updates.into_iter().map(|u| (u.origin, u.items)));
    }
    drop(indexer); // 書き込み待ちをすべて処理する時間も含める
    let ingest_ms = started.elapsed().as_secs_f64() * 1000.0;
    #[cfg(target_os = "macos")]
    let memory = memory.map(memory::Monitor::finish);
    let resident_text_bytes: usize = store
        .sessions
        .iter()
        .flat_map(|s| &s.lanes)
        .flat_map(|l| &l.events)
        .map(|e| e.input.len() + e.output.len())
        .sum();
    let pending_text_bytes = crate::model::pending_text_bytes(
        store
            .sessions
            .iter()
            .flat_map(|s| &s.lanes)
            .flat_map(|l| &l.events),
    );
    let verify = Instant::now();
    let db = crate::db::Db::open_index_read(&crate::db::path()).unwrap();
    let detail_db = crate::db::Db::open_read().unwrap();
    assert_eq!(
        db.fingerprints().unwrap().len(),
        tasks.len(),
        "指紋の保存数が一致しません"
    );
    let mut files = db.load_all().unwrap();
    let mut actual = std::collections::BTreeMap::new();
    let mut row_hashes = std::collections::HashMap::new();
    let mut bodies = std::collections::HashMap::<u64, usize>::new();
    for (path, items) in &mut files {
        for item in items.iter_mut() {
            let lanes: &mut [Lane] = match item {
                Loaded::Session(s) => &mut s.lanes,
                Loaded::Child(_, lane) => std::slice::from_mut(lane),
            };
            for lane in lanes {
                let ids: Vec<_> = lane.events.iter().filter_map(|e| e.row()).collect();
                let mut texts = detail_db.details(&ids);
                for event in &mut lane.events {
                    if let Some((input, output)) = event.row().and_then(|id| texts.remove(&id)) {
                        event.input = input;
                        event.output = output;
                    }
                    let hash = event_hash(event, &event.input, &event.output);
                    row_hashes.insert(event.row().unwrap(), hash);
                    *bodies.entry(hash).or_default() += 1;
                }
            }
        }
        actual.insert(path.clone(), items_hash(items));
        // 検証で本文の二つ目の全件コピーを保持せず、ファイルごとに解放する
        items.clear();
    }
    assert_eq!(expected.len(), actual.len(), "保存ファイル数が一致しません");
    assert!(
        expected == actual,
        "索引への保存・再読み込みで内容が変わりました"
    );
    // Storeの共有された版からも、保存後の本文をイベントと正しく結び付けて取得できることを確認する。
    for lane in store.sessions.iter().flat_map(|s| &s.lanes) {
        let sources: Vec<_> = lane.events.iter().map(crate::model::Event::text).collect();
        let ids: Vec<_> = sources
            .iter()
            .filter_map(|t| match t {
                EventText::Row(id) => Some(*id),
                _ => None,
            })
            .collect();
        let mut texts = detail_db.details(&ids);
        for (event, source) in lane.events.iter().zip(sources) {
            let hash = match source {
                EventText::Inline(i, o) => event_hash(event, i, o),
                EventText::Pending(i, o) => event_hash(event, &i, &o),
                EventText::Row(id) => {
                    let (i, o) = texts.remove(&id).expect("保存後の本文がありません");
                    let hash = event_hash(event, &i, &o);
                    assert_eq!(
                        row_hashes[&id], hash,
                        "イベントと保存本文の対応が変わりました"
                    );
                    hash
                }
            };
            let count = bodies
                .get_mut(&hash)
                .expect("Storeの本文が保存内容と異なります");
            assert!(*count > 0);
            *count -= 1;
        }
    }
    let mut digest = DefaultHasher::new();
    expected.hash(&mut digest);
    let result = serde_json::json!({"files":tasks.len(), "sessions":store.sessions.len(),
        "events":store.sessions.iter().flat_map(|s| &s.lanes).map(|l| l.events.len()).sum::<usize>(),
        "ingest_ms":ingest_ms, "resident_text_bytes":resident_text_bytes,
        "pending_text_bytes":pending_text_bytes, "snapshot_texts_equal":true,
        "roundtrip_ms":verify.elapsed().as_secs_f64()*1000.0,
        "content_hash":format!("{:016x}",digest.finish()), "roundtrip_equal":true});
    #[cfg(target_os = "macos")]
    let result = match memory {
        Some(memory) => {
            let mut result = result;
            result["memory"] = serde_json::to_value(memory).unwrap();
            result
        }
        None => result,
    };
    println!("{result}");
}
