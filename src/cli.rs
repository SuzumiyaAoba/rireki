//! ウィンドウを開かずに動く計測・統計コマンドの振り分け。

use crate::{db, index, model, perf};
use std::path::Path;

/// CLI コマンドを実行したときは true を返す。画面計測は GUI の起動側で扱う。
pub(super) fn run(args: &[String]) -> bool {
    match args.get(1).map(String::as_str) {
        Some("--bench-parse") => perf::parse(required_path(args, 2, "manifest を指定してください")),
        Some("--bench-search") => {
            perf::search(required_path(args, 2, "索引 DB を指定してください"))
        }
        Some("--bench-db") => perf::db_load(required_path(args, 2, "索引 DB を指定してください")),
        #[cfg(target_os = "macos")]
        Some("--bench-db-memory") => {
            perf::db_memory(required_path(args, 2, "索引 DB を指定してください"))
        }
        Some("--bench-details") => perf::details(
            required_path(args, 2, "索引 DB を指定してください"),
            required_path(args, 3, "本文 DB を指定してください"),
        ),
        Some("--bench-summary") => {
            perf::detail_summary(required_path(args, 2, "索引 DB を指定してください"))
        }
        Some("--bench-range") => perf::range(required_path(args, 2, "索引 DB を指定してください")),
        Some("--bench-statistics") => {
            perf::statistics(required_path(args, 2, "索引 DB を指定してください"))
        }
        Some("--bench-discover") => {
            perf::discover(args.get(2).map(Path::new), args.get(3).map(Path::new))
        }
        Some("--bench-view") => perf::view(args.get(2).map(Path::new)),
        Some("--bench-ingest") => perf::ingest(required_path(
            args,
            2,
            "manifest の保存先を指定してください",
        )),
        Some("--verify-logs") => perf::verify_logs(),
        Some("--bench") => perf::run(args.get(2).map(Path::new)),
        _ if args.iter().any(|a| a == "--stats") => print_stats(),
        _ => return false,
    }
    true
}

fn required_path<'a>(args: &'a [String], index: usize, message: &str) -> &'a Path {
    Path::new(args.get(index).expect(message))
}

/// GUI を起動せずに、索引 DB の読み込みと差分取り込みを行って結果を表示する
fn print_stats() {
    let t = std::time::Instant::now();
    let mut ix = index::Indexer::open();
    let mut store = model::Store::default();
    let stable = ix.load_stable();
    let n_stable = stable.len();
    store.replace(stable.into_iter().map(|u| (u.origin, u.items)));
    let t_db = t.elapsed();
    let tasks = ix.plan();
    let n_live = tasks.iter().filter(|t| t.live).count();
    let n_tasks = tasks.len();
    for chunk in tasks.chunks(index::CHUNK) {
        store.replace(ix.process(chunk).into_iter().map(|u| (u.origin, u.items)));
    }
    let visible: Vec<_> = (0..store.sessions.len())
        .filter(|&i| store.visible(i))
        .map(|i| &store.sessions[i])
        .collect();
    let lanes: usize = visible.iter().map(|s| s.lanes.len()).sum();
    let events: usize = visible
        .iter()
        .flat_map(|s| &s.lanes)
        .map(|l| l.events.len())
        .sum();
    let st = model::Stats::of(
        visible
            .iter()
            .flat_map(|s| &s.lanes)
            .flat_map(|l| &l.events),
    );
    println!(
        "DB: {n_stable} files in {t_db:.2?} ({})",
        db::path().display()
    );
    println!("parsed: {n_tasks} files ({n_live} live)");
    println!(
        "{} sessions, {lanes} lanes, {events} events in {:.2?}",
        visible.len(),
        t.elapsed()
    );
    println!(
        "{} tool calls ({} errors, {} rejected, {} interrupted)",
        st.tools, st.errors, st.rejected, st.interrupted
    );
}
