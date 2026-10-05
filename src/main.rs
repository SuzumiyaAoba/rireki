mod app;
mod icons;
mod input;
mod pinch;
mod perf;
mod prefs;
mod theme;

use rireki::{
    db, detail, histogram, index, jsmock, layout, loader, model, patch, pricing, search, statistics, time,
    view_index,
};

use gpui::{
    App, Application, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size,
};

static PROCESS_STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    PROCESS_STARTED.set(std::time::Instant::now()).unwrap();
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--bench-search") {
        return perf::search(std::path::Path::new(args.get(2).expect("索引 DB を指定してください")));
    }
    if args.get(1).is_some_and(|a| a == "--bench-db") {
        return perf::db_load(std::path::Path::new(args.get(2).expect("索引 DB を指定してください")));
    }
    #[cfg(target_os = "macos")]
    if args.get(1).is_some_and(|a| a == "--bench-db-memory") {
        return perf::db_memory(std::path::Path::new(args.get(2).expect("索引 DB を指定してください")));
    }
    if args.get(1).is_some_and(|a| a == "--bench-details") {
        return perf::details(
            std::path::Path::new(args.get(2).expect("索引 DB を指定してください")),
            std::path::Path::new(args.get(3).expect("本文 DB を指定してください")),
        );
    }
    if args.get(1).is_some_and(|a| a == "--bench-summary") {
        return perf::detail_summary(std::path::Path::new(args.get(2).expect("索引 DB を指定してください")));
    }
    if args.get(1).is_some_and(|a| a == "--bench-range") {
        return perf::range(std::path::Path::new(args.get(2).expect("索引 DB を指定してください")));
    }
    if args.get(1).is_some_and(|a| a == "--bench-statistics") {
        return perf::statistics(std::path::Path::new(args.get(2).expect("索引 DB を指定してください")));
    }
    if args.get(1).is_some_and(|a| a == "--bench-discover") {
        return perf::discover(args.get(2).map(std::path::Path::new), args.get(3).map(std::path::Path::new));
    }
    if args.get(1).is_some_and(|a| a == "--bench-view") {
        return perf::view(args.get(2).map(std::path::Path::new));
    }
    if args.get(1).is_some_and(|a| a == "--bench-ingest") {
        return perf::ingest(std::path::Path::new(args.get(2).expect("manifest の保存先を指定してください")));
    }
    if args.get(1).is_some_and(|a| a == "--verify-logs") {
        return perf::verify_logs();
    }
    if args.get(1).is_some_and(|a| a == "--bench") {
        return perf::run(args.get(2).map(std::path::Path::new));
    }
    if std::env::args().any(|a| a == "--stats") {
        return print_stats();
    }
    let profile_ui = args.iter().any(|a| a == "--profile-ui");
    if args.iter().any(|a| matches!(a.as_str(), "--profile-startup" | "--profile-ingest" | "--profile-idle" | "--profile-render" | "--profile-eager-files" | "--profile-detail" | "--profile-rich" | "--profile-files" | "--profile-statistics" | "--profile-summary")) && !profile_ui {
        eprintln!("画面計測には --profile-ui も指定してください");
        std::process::exit(1);
    }
    if profile_ui
        && (std::env::var_os("RIREKI_CACHE_DIR").is_none() || std::env::var_os("RIREKI_CONFIG_DIR").is_none())
    {
        eprintln!("--profile-ui は RIREKI_CACHE_DIR と RIREKI_CONFIG_DIR に隔離した計測用ディレクトリを指定してください");
        std::process::exit(1);
    }
    let profile_tasks = args.iter().position(|a| a == "--profile-ingest").map(|i| {
        perf::ingest_tasks(std::path::Path::new(args.get(i + 1).expect("manifest を指定してください")))
    });
    Application::new().with_assets(icons::Assets).run(move |cx: &mut App| {
        app::init(cx);
        let bounds = Bounds::centered(None, size(px(1500.0), px(920.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // タイトルバーを透明にして、上端の帯とアイコンレールを地の色でつなげる（信号機ボタンは帯の中央に置く）
                titlebar: Some(TitlebarOptions {
                    title: Some("rireki".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.0), px((app::STRIP_H - 14.0) / 2.0))),
                }),
                window_min_size: Some(size(px(960.0), px(600.0))),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| app::Rireki::new(window, cx, profile_tasks));
                window.focus(&view.focus_handle(cx));
                view
            },
        )
        .unwrap();
        cx.activate(true);
    });
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
    let visible: Vec<_> = (0..store.sessions.len()).filter(|&i| store.visible(i)).map(|i| &store.sessions[i]).collect();
    let lanes: usize = visible.iter().map(|s| s.lanes.len()).sum();
    let events: usize = visible.iter().flat_map(|s| &s.lanes).map(|l| l.events.len()).sum();
    let st = model::Stats::of(visible.iter().flat_map(|s| &s.lanes).flat_map(|l| &l.events));
    println!("DB: {n_stable} files in {t_db:.2?} ({})", db::path().display());
    println!("parsed: {n_tasks} files ({n_live} live)");
    println!("{} sessions, {lanes} lanes, {events} events in {:.2?}", visible.len(), t.elapsed());
    println!("{} tool calls ({} errors, {} rejected, {} interrupted)", st.tools, st.errors, st.rejected, st.interrupted);
}
