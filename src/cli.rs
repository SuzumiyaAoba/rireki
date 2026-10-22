//! ウィンドウを開かずに動く計測・統計コマンドの振り分け。

use crate::{db, model, perf};
use rireki::history::History;
use std::path::Path;

/// CLI コマンドを実行したときは true を返す。画面計測は GUI の起動側で扱う。
pub(super) fn run(args: &[String]) -> bool {
    match args.get(1).map(String::as_str) {
        Some("--help" | "-h") => print_help(),
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

fn print_help() {
    println!(
        "rireki — セッション履歴の閲覧・集計\n\n\
使い方: rireki [コマンド]\n\n\
  --help, -h                   このヘルプを表示\n\
  --tui                        ターミナルで履歴を閲覧（詳細は --tui --help）\n\
  --stats                      履歴を取り込み、統計を表示\n\
  --bench [索引DB]             読み込み・検索・集計を計測\n\
  --bench-parse <manifest>      ログ解析を計測\n\
  --bench-search <索引DB>       検索を計測\n\
  --bench-db <索引DB>           索引DBの読み込みを計測\n\
  --bench-details <索引DB> <本文DB>  本文の読み込みを計測\n\
  --bench-summary <索引DB>      本文の集計を計測\n\
  --bench-range <索引DB>        表示範囲の集計を計測\n\
  --bench-statistics <索引DB>   統計の集計を計測\n\
  --bench-discover [Claudeルート Codexルート]  ログ探索を計測\n\
  --bench-view [索引DB]         表示用の索引を計測\n\
  --bench-ingest <manifest>     ログの取り込みを計測\n\
  --verify-logs                 ログの整合性を検証"
    );
    #[cfg(target_os = "macos")]
    println!("  --bench-db-memory <索引DB>    索引DBのメモリー使用量を計測");
    #[cfg(feature = "gui")]
    println!("\nコマンドなしで GUI を起動します。画面計測には --profile-ui を指定します。");
    #[cfg(all(not(feature = "gui"), feature = "tui"))]
    println!(
        "\nコマンドなしで TUI を起動します。GUI を使うには gui feature を有効にしてください。"
    );
    #[cfg(not(any(feature = "gui", feature = "tui")))]
    println!("\nGUI / TUI を含まないビルドです。--stats などのコマンドを指定してください。");
    #[cfg(not(feature = "tui"))]
    println!("このビルドに TUI は含まれていません。tui feature を有効にしてください。");
    println!("GUI 依存なしで TUI をビルド: cargo build --no-default-features --features tui");
    #[cfg(all(feature = "tui", target_os = "windows"))]
    println!("Windows の端末では rireki-tui.exe を実行してください（--tui は不要です）。");
}

fn required_path<'a>(args: &'a [String], index: usize, message: &str) -> &'a Path {
    Path::new(args.get(index).expect(message))
}

/// GUI を起動せずに、索引 DB の読み込みと差分取り込みを行って結果を表示する
fn print_stats() {
    let t = std::time::Instant::now();
    let mut history = History::open();
    let cached = history.load_cached();
    let n_stable = cached.files;
    drop(cached);
    let t_db = t.elapsed();
    let tasks = history.plan();
    let n_live = tasks.iter().filter(|t| t.live).count();
    let n_tasks = tasks.len();
    for _ in history.ingest(tasks) {}
    let store = history.snapshot();
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
