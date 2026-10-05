//! GUI を起動せず、読み取り専用の索引と決まった負荷で性能を計測する

use std::hash::{DefaultHasher, Hash, Hasher};
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use crate::layout::{self, Granularity};
use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Store};

#[derive(serde::Serialize, serde::Deserialize)]
struct IngestJob {
    kind: u8,
    path: std::path::PathBuf,
    mtime: f64,
    fingerprint: String,
}

fn items_hash(items: &[Loaded]) -> u64 {
    let mut hash = DefaultHasher::new();
    items.len().hash(&mut hash);
    for item in items {
        let lanes: &[Lane] = match item {
            Loaded::Session(s) => {
                (
                    0u8,
                    &s.id,
                    s.source as u8,
                    &s.cwd,
                    s.headless,
                    &s.title,
                    &s.branch,
                    &s.version,
                )
                    .hash(&mut hash);
                &s.lanes
            }
            Loaded::Child(parent, lane) => {
                (1u8, parent).hash(&mut hash);
                std::slice::from_ref(lane)
            }
        };
        lanes.len().hash(&mut hash);
        for lane in lanes {
            (&lane.name, &lane.spawned_by, &lane.model, &lane.effort).hash(&mut hash);
            lane.events.len().hash(&mut hash);
            for e in &lane.events {
                (e.start.to_bits(), e.end.to_bits(), e.kind as u8, e.status as u8).hash(&mut hash);
                (&e.name, &e.summary, &e.input, &e.output, &e.call_id, &e.model).hash(&mut hash);
                (
                    e.usage.input,
                    e.usage.cache_read,
                    e.usage.cache_write,
                    e.usage.cache_write_1h,
                    e.usage.output,
                    e.usage.reasoning,
                )
                    .hash(&mut hash);
            }
        }
    }
    hash.finish()
}

/// 空の隔離キャッシュを要求し、非公開の manifest で計測対象を固定する
pub(crate) fn ingest_tasks(manifest: &Path) -> Vec<crate::index::Task> {
    let cache = std::env::var_os("RIREKI_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .expect("RIREKI_CACHE_DIR が必要です");
    assert!(
        !cache.exists() || std::fs::read_dir(&cache).is_ok_and(|mut entries| entries.next().is_none()),
        "取り込みの計測には空の RIREKI_CACHE_DIR が必要です"
    );
    let discovered = crate::loader::discover();
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
    let current: std::collections::HashMap<_, _> = discovered.iter().map(|j| (&j.path, j)).collect();
    for job in &jobs {
        assert!(
            current
                .get(&job.path)
                .is_some_and(|j| j.fingerprint == job.fingerprint && j.kind as u8 == job.kind),
            "計測対象のファイルが変更されています。新しい manifest で測り直してください"
        );
    }
    jobs
        .into_iter()
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
    let started = Instant::now();
    let mut indexer = crate::index::Indexer::open();
    let mut store = Store::default();
    for chunk in tasks.chunks(crate::index::CHUNK) {
        let updates = indexer.process(chunk);
        for update in &updates {
            if !update.items.is_empty() {
                expected.insert(update.origin.clone(), items_hash(&update.items));
            }
        }
        store.replace(updates.into_iter().map(|u| (u.origin, u.items)));
    }
    drop(indexer); // 書き込み待ちをすべて処理する時間も含める
    let ingest_ms = started.elapsed().as_secs_f64() * 1000.0;
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
    for (path, items) in &mut files {
        for item in items.iter_mut() {
            let lanes: &mut [Lane] = match item {
                Loaded::Session(s) => &mut s.lanes,
                Loaded::Child(_, lane) => std::slice::from_mut(lane),
            };
            for lane in lanes {
                let ids: Vec<_> = lane.events.iter().filter_map(|e| e.row).collect();
                let mut texts = detail_db.details(&ids);
                for event in &mut lane.events {
                    if let Some((input, output)) = event.row.and_then(|id| texts.remove(&id)) {
                        event.input = input;
                        event.output = output;
                    }
                }
            }
        }
        actual.insert(path.clone(), items_hash(items));
        // 検証で本文の二つ目の全件コピーを保持せず、ファイルごとに解放する
        items.clear();
    }
    assert_eq!(expected.len(), actual.len(), "保存ファイル数が一致しません");
    assert!(expected == actual, "索引への保存・再読み込みで内容が変わりました");
    let mut digest = DefaultHasher::new();
    expected.hash(&mut digest);
    println!(
        "{}",
        serde_json::json!({"files":tasks.len(), "sessions":store.sessions.len(),
        "events":store.sessions.iter().flat_map(|s| &s.lanes).map(|l| l.events.len()).sum::<usize>(),
        "ingest_ms":ingest_ms, "roundtrip_ms":verify.elapsed().as_secs_f64()*1000.0,
        "content_hash":format!("{:016x}",digest.finish()), "roundtrip_equal":true})
    );
}

fn log_sample() -> Vec<(u64, crate::loader::Job)> {
    let mut sample: Vec<_> = crate::loader::discover()
        .into_iter()
        .filter_map(|job| std::fs::metadata(&job.path).ok().map(|m| (m.len(), job)))
        .collect();
    sample.sort_by_key(|&(bytes, _)| std::cmp::Reverse(bytes));
    sample.truncate(3);
    sample
}

/// 解析結果の全内容を比較できるよう、本文やパスを出さずに決定的なハッシュを返す
pub fn verify_logs() {
    let mut sample = log_sample();
    let jobs = crate::loader::discover();
    for kind in [
        crate::loader::JobKind::Claude,
        crate::loader::JobKind::ClaudeLegacyAgent,
    ] {
        if let Some(job) = jobs
            .iter()
            .filter(|j| j.kind == kind)
            .max_by_key(|j| std::fs::metadata(&j.path).map_or(0, |m| m.len()))
        {
            sample.push((std::fs::metadata(&job.path).map_or(0, |m| m.len()), job.clone()));
        }
    }
    for (i, (bytes, job)) in sample.into_iter().enumerate() {
        let items = crate::loader::parse(&job);
        let mut hash = DefaultHasher::new();
        let mut events = 0;
        let mut hash_lane = |lane: &Lane| {
            (&lane.name, &lane.spawned_by, &lane.model, &lane.effort).hash(&mut hash);
            lane.events.len().hash(&mut hash);
            for e in &lane.events {
                (e.start.to_bits(), e.end.to_bits(), e.kind as u8, e.status as u8).hash(&mut hash);
                (&e.name, &e.summary, &e.input, &e.output, &e.call_id, &e.row, &e.model).hash(&mut hash);
                (
                    e.usage.input,
                    e.usage.cache_read,
                    e.usage.cache_write,
                    e.usage.cache_write_1h,
                    e.usage.output,
                    e.usage.reasoning,
                )
                    .hash(&mut hash);
            }
            events += lane.events.len();
        };
        // レーンのハッシュとメタデータのハッシュを別々に作り、借用と順序を固定する
        let mut meta = DefaultHasher::new();
        items.len().hash(&mut meta);
        for item in &items {
            match item {
                Loaded::Session(s) => {
                    (
                        0u8,
                        &s.id,
                        s.source as u8,
                        &s.cwd,
                        s.headless,
                        &s.title,
                        &s.branch,
                        &s.version,
                    )
                        .hash(&mut meta);
                    s.lanes.len().hash(&mut meta);
                    for lane in &s.lanes {
                        hash_lane(lane);
                    }
                }
                Loaded::Child(parent, lane) => {
                    (1u8, parent).hash(&mut meta);
                    hash_lane(lane);
                }
            }
        }
        println!(
            "{}",
            serde_json::json!({"sample":i,"kind":format!("{:?}",job.kind),"bytes":bytes,"items":items.len(),"events":events,
            "metadata_hash":format!("{:016x}",meta.finish()),"event_hash":format!("{:016x}",hash.finish())})
        );
    }
}

fn measure<T>(name: &str, repetitions: usize, mut f: impl FnMut() -> T) {
    let mut times = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        let t = Instant::now();
        black_box(f());
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({
            "name": name, "repetitions": repetitions,
            "median_ms": times[repetitions / 2],
            "p95_ms": times[(repetitions * 95).div_ceil(100).saturating_sub(1)],
        })
    );
}

/// パースせずに探索だけを繰り返す。指定ルートも既定ルートも読み取り専用
pub fn discover(claude: Option<&Path>, codex: Option<&Path>) {
    let scan = || match (claude, codex) {
        (Some(claude), Some(codex)) => crate::loader::discover_in(claude, codex),
        (None, None) => crate::loader::discover(),
        _ => panic!("Claude の projects と Codex の sessions を両方指定してください"),
    };
    let mut jobs = scan();
    let counts = [
        crate::loader::JobKind::Claude,
        crate::loader::JobKind::ClaudeLegacyAgent,
        crate::loader::JobKind::Codex,
    ]
    .map(|kind| jobs.iter().filter(|job| job.kind == kind).count());
    jobs.sort_by(|a, b| a.path.cmp(&b.path));
    let mut paths = DefaultHasher::new();
    let mut stamps = DefaultHasher::new();
    for job in &jobs {
        (job.kind as u8, &job.path).hash(&mut paths);
        (job.kind as u8, &job.path, job.mtime.to_bits(), &job.fingerprint).hash(&mut stamps);
    }
    println!(
        "{}",
        serde_json::json!({"claude":counts[0],"legacy":counts[1],"codex":counts[2],
        "paths_hash":format!("{:016x}",paths.finish()),"stamps_hash":format!("{:016x}",stamps.finish())})
    );
    drop(jobs);
    measure("discover", 31, scan);
}

fn view_order(store: &Store) -> Vec<usize> {
    let mut order: Vec<_> = (0..store.sessions.len()).filter(|&i| store.visible(i)).collect();
    order.sort_by(|&a, &b| store.sessions[b].start.total_cmp(&store.sessions[a].start));
    order
}

/// 描画せずに表示用の順序とセッション間の起動元を測る
pub fn view(index: Option<&Path>) {
    if let Some(index) = index {
        let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
        let mut store = Store::default();
        store.replace(db.load_all().unwrap());
        let order = view_order(&store);
        let links = crate::app::view_index::exec_parents(&store.sessions, &order);
        let mut sorted: Vec<_> = links
            .iter()
            .map(|(&child, &(parent, lane, start))| (child, parent, lane, start.to_bits()))
            .collect();
        sorted.sort_unstable();
        let mut hash = DefaultHasher::new();
        sorted.hash(&mut hash);
        println!(
            "{}",
            serde_json::json!({"sessions":order.len(),"headless":order.iter().filter(|&&i| store.sessions[i].headless).count(),
            "links":links.len(),"links_hash":format!("{:016x}", hash.finish())})
        );
        measure("view_order", 31, || view_order(&store));
        measure("exec_parents_real", 31, || {
            crate::app::view_index::exec_parents(&store.sessions, &order)
        });
    }
    let mut sessions = Vec::new();
    for si in 0..128 {
        let mut lane = Lane::new("main");
        let events = if si < 64 { 2000 } else { 1 };
        for ei in 0..events {
            let start = if si < 64 { ei as f64 } else { 5000.0 + si as f64 };
            lane.events.push(Event {
                start,
                end: if si < 64 && ei == 0 { 100_000.0 } else { start + 0.5 },
                kind: if si >= 64 {
                    Kind::Assistant
                } else if ei == 0 {
                    Kind::Agent
                } else {
                    Kind::Tool
                },
                name: "exec_command".into(),
                summary: String::new(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                row: None,
                usage: Default::default(),
                model: None,
                status: Default::default(),
            });
        }
        let mut session = Session {
            id: format!("session-{si}"),
            source: Source::Codex,
            cwd: "/synthetic".into(),
            headless: si >= 64,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        sessions.push(std::sync::Arc::new(session));
    }
    let order: Vec<_> = (0..sessions.len()).collect();
    assert!(crate::app::view_index::exec_parents(&sessions, &order).is_empty());
    measure("exec_parents_overlapping_agent", 7, || {
        crate::app::view_index::exec_parents(&sessions, &order)
    });
}

fn synthetic_store(n: usize) -> Store {
    let mut lane = Lane::new("main");
    lane.events = (0..n)
        .map(|i| Event {
            start: i as f64,
            end: i as f64 + 0.5,
            kind: if i % 1000 == 0 { Kind::User } else { Kind::Tool },
            name: if i % 1000 == 0 { "User" } else { "exec_command" }.into(),
            summary: "cargo test 日本語の検索対象".into(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            row: None,
            usage: Default::default(),
            model: None,
            status: Default::default(),
        })
        .collect();
    let mut store = Store::default();
    store.replace([(
        "synthetic".into(),
        vec![Loaded::Session(Session {
            id: "synthetic".into(),
            source: Source::Codex,
            cwd: "/synthetic".into(),
            headless: false,
            title: "性能計測".into(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        })],
    )]);
    store
}

pub fn run(index: Option<&Path>) {
    println!(
        "{}",
        serde_json::json!({"profile": if cfg!(debug_assertions) { "dev" } else { "release" },
        "parallelism": std::thread::available_parallelism().map_or(1, |n| n.get())})
    );
    if let Some(index) = index {
        let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
        measure("db_load", 7, || db.load_all().unwrap());
        let mut store = Store::default();
        let files = db.load_all().unwrap();
        store.replace(files);
        println!(
            "{}",
            serde_json::json!({"sessions": store.sessions.len(),
            "events": store.sessions.iter().flat_map(|s| &s.lanes).map(|l| l.events.len()).sum::<usize>()})
        );
        let scope: Vec<_> = (0..store.sessions.len()).filter(|&i| store.visible(i)).collect();
        let mut search = crate::search::Index::default();
        measure("real_search_cold", 1, || {
            search.search(&store.sessions, &scope, "exec", 500)
        });
        measure("real_search_warm", 15, || {
            search.search(&store.sessions, &scope, "exec", 500)
        });
        let sample = log_sample();
        for (i, (bytes, job)) in sample.iter().enumerate() {
            println!(
                "{}",
                serde_json::json!({"parse_sample": i, "bytes": bytes, "kind": format!("{:?}", job.kind)})
            );
            measure(&format!("parse_sample_{i}"), 5, || crate::loader::parse(job));
        }
        measure("discover", 7, crate::loader::discover);
    }
    let store = synthetic_store(200_000);
    let lane = &store.sessions[0].lanes[0];
    for gran in Granularity::ALL {
        measure(&format!("aggregate_{gran:?}"), 31, || {
            layout::aggregate(lane, (199_900.0, 199_950.0), 1.0, gran)
        });
        let mut cache = layout::Cache::default();
        measure(&format!("aggregate_cached_cold_{gran:?}"), 1, || {
            cache.aggregate((0, 0), lane, (199_900.0, 199_950.0), 1.0, gran)
        });
        measure(&format!("aggregate_cached_warm_{gran:?}"), 31, || {
            cache.aggregate((0, 0), lane, (199_900.0, 199_950.0), 1.0, gran)
        });
    }
    let b = layout::Block {
        start: 0.0,
        end: 200_000.0,
        first: 0,
        last: lane.events.len() - 1,
    };
    measure("block_label_and_dominant", 15, || {
        layout::describe(&lane.events, &b, true)
    });
    let mut cache = layout::Cache::default();
    measure("block_description_cached_cold", 1, || {
        cache.describe((0, 0), lane, &b, true)
    });
    measure("block_description_cached_warm", 31, || {
        cache.describe((0, 0), lane, &b, true)
    });
    let mut search = crate::search::Index::default();
    measure("dense_search_cold", 1, || {
        search.search(&store.sessions, &[0], "cargo", 500)
    });
    measure("dense_search_warm", 15, || {
        search.search(&store.sessions, &[0], "cargo", 500)
    });
    let text = "日本語の長い本文 cargo test\n".repeat(50_000);
    measure("short_summary_long_text", 31, || {
        crate::model::one_line(black_box(&text), 160)
    });
    measure("truncate_long_text", 31, || {
        crate::model::truncate(black_box(&text), 6000)
    });
}
