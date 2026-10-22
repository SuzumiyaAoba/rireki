//! GUI を起動せず、読み取り専用の索引と決まった負荷で性能を計測する

mod db;
mod fingerprint;
mod ingest;
mod verification;
mod view;

pub use db::{db_load, details};
use fingerprint::items_hash;
pub use ingest::ingest;
pub(crate) use ingest::ingest_tasks;
use verification::log_sample;
pub use verification::verify_logs;
pub use view::view;
use view::view_order;

#[cfg(target_os = "macos")]
mod memory;
mod parse;
mod range;
mod search;
mod statistics;
mod summary;
#[cfg(target_os = "macos")]
pub use memory::db as db_memory;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use crate::layout::{self, Granularity};
use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Store};

pub(crate) fn measure<T>(name: &str, repetitions: usize, mut f: impl FnMut() -> T) {
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
        (
            job.kind as u8,
            &job.path,
            job.mtime.to_bits(),
            &job.fingerprint,
        )
            .hash(&mut stamps);
    }
    println!(
        "{}",
        serde_json::json!({"claude":counts[0],"legacy":counts[1],"codex":counts[2],
        "paths_hash":format!("{:016x}",paths.finish()),"stamps_hash":format!("{:016x}",stamps.finish())})
    );
    drop(jobs);
    measure("discover", 31, scan);
}

fn synthetic_store(n: usize) -> Store {
    let mut lane = Lane::new("main");
    lane.events = (0..n)
        .map(|i| Event {
            start: i as f64,
            end: i as f64 + 0.5,
            kind: if i % 1000 == 0 {
                Kind::User
            } else {
                Kind::Tool
            },
            name: if i % 1000 == 0 {
                "User"
            } else {
                "exec_command"
            }
            .into(),
            summary: "cargo test 日本語の検索対象".into(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
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

/// 表示分布の集計を同じ索引と大量イベントで測る。
pub fn range(index: &Path) {
    let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
    let mut store = Store::default();
    store.replace(db.load_all().unwrap());
    let order = view_order(&store);
    println!(
        "{}",
        serde_json::json!({"dataset":"real", "sessions":order.len()})
    );
    range::benchmark(&store.snapshot(), &order);
    let store = synthetic_store(200_000);
    println!("{}", serde_json::json!({"dataset":"dense", "sessions":1}));
    range::benchmark(&store.snapshot(), &[0]);
}

/// 統計画面と同じ集計を、固定日・読み取り専用 DB と密な履歴で測る。
pub fn statistics(index: &Path) {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
    let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
    let mut store = Store::default();
    store.replace(db.load_all().unwrap());
    let order = view_order(&store);
    println!(
        "{}",
        serde_json::json!({"dataset":"real", "sessions":order.len(),
        "events":order.iter().flat_map(|&i| &store.sessions[i].lanes).map(|l| l.events.len()).sum::<usize>()})
    );
    statistics::benchmark(&store.snapshot(), &order, today);
    let mut store = synthetic_store(200_000);
    let base = chrono::TimeZone::from_local_datetime(
        &chrono::Local,
        &(today - chrono::Duration::days(119))
            .and_hms_opt(0, 0, 0)
            .unwrap(),
    )
    .earliest()
    .unwrap()
    .timestamp() as f64;
    let session = std::sync::Arc::make_mut(&mut store.sessions[0]);
    for (i, event) in session.lanes[0].events.iter_mut().enumerate() {
        event.start = base + i as f64 * (119.0 * 86400.0 / 200_000.0);
        event.end = event.start + 0.5;
        event.usage.input = 1000;
        event.usage.output = 100;
        event.model = Some("gpt-6-sol".into());
    }
    session.finalize();
    println!(
        "{}",
        serde_json::json!({"dataset":"dense", "sessions":1, "events":200_000})
    );
    statistics::benchmark(&store.snapshot(), &[0], today);
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
        let scope: Vec<_> = (0..store.sessions.len())
            .filter(|&i| store.visible(i))
            .collect();
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
            measure(&format!("parse_sample_{i}"), 5, || {
                crate::loader::parse(job)
            });
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

pub fn detail_summary(index: &Path) {
    let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
    let mut store = Store::default();
    store.replace(db.load_all().unwrap());
    if let Some(session) = store
        .sessions
        .iter()
        .filter(|s| !s.lanes.is_empty())
        .max_by_key(|s| s.lanes.iter().map(|l| l.events.len()).sum::<usize>())
    {
        let lane = (0..session.lanes.len())
            .max_by_key(|&li| session.lanes[li].events.len())
            .unwrap();
        println!("{}", serde_json::json!({"dataset":"real"}));
        summary::benchmark(session, lane);
    }
    let mut store = synthetic_store(200_000);
    let session = std::sync::Arc::make_mut(&mut store.sessions[0]);
    for (i, event) in session.lanes[0].events.iter_mut().enumerate() {
        event.usage.input = 1000 + i as u32;
        event.usage.output = 100;
        event.model = Some("gpt-6-sol".into());
    }
    println!("{}", serde_json::json!({"dataset":"dense"}));
    summary::benchmark(&store.sessions[0], 0);
}

pub fn search(index: &Path) {
    search::benchmark(index);
}

pub fn parse(manifest: &Path) {
    parse::benchmark(manifest);
}
