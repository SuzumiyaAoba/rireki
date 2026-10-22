#![cfg_attr(
    all(target_os = "windows", not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]

mod app;
mod cli;
mod icons;
mod input;
mod perf;
mod pinch;
mod platform;
mod prefs;
mod theme;

use rireki::{
    cost_timeline, db, detail, histogram, index, jsmock, layout, loader, mcp, model, patch,
    pricing, search, statistics, time, view_index,
};

use gpui::{
    App, Application, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, point,
    prelude::*, px, size,
};

static PROCESS_STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    PROCESS_STARTED.set(std::time::Instant::now()).unwrap();
    let args: Vec<_> = std::env::args().collect();
    #[cfg(target_os = "windows")]
    if args.len() > 1 {
        platform::attach_parent_console();
    }
    if cli::run(&args) {
        return;
    }
    let profile_ui = args.iter().any(|a| a == "--profile-ui");
    if args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--profile-startup"
                | "--profile-ingest"
                | "--profile-idle"
                | "--profile-render"
                | "--profile-eager-files"
                | "--profile-detail"
                | "--profile-analysis"
                | "--profile-rich"
                | "--profile-files"
                | "--profile-statistics"
                | "--profile-summary"
                | "--profile-calendar"
                | "--profile-timeline"
                | "--profile-dense-timeline"
                | "--profile-components"
                | "--profile-exact-group-scale"
                | "--profile-unbatched-minimap"
                | "--profile-paired-minimap"
                | "--profile-full-grouping"
                | "--profile-uncached-rows"
                | "--profile-sidebar"
                | "--profile-eager-sidebar"
        )
    }) && !profile_ui
    {
        eprintln!("画面計測には --profile-ui も指定してください");
        std::process::exit(1);
    }
    if args.iter().any(|a| a == "--profile-paired-minimap")
        && !(args.iter().any(|a| a == "--profile-render")
            && args.iter().any(|a| a == "--profile-dense-timeline"))
    {
        eprintln!(
            "--profile-paired-minimap は --profile-render と --profile-dense-timeline も指定してください"
        );
        std::process::exit(1);
    }
    if profile_ui
        && (std::env::var_os("RIREKI_CACHE_DIR").is_none()
            || std::env::var_os("RIREKI_CONFIG_DIR").is_none())
    {
        eprintln!(
            "--profile-ui は RIREKI_CACHE_DIR と RIREKI_CONFIG_DIR に隔離した計測用ディレクトリを指定してください"
        );
        std::process::exit(1);
    }
    let profile_tasks = args.iter().position(|a| a == "--profile-ingest").map(|i| {
        perf::ingest_tasks(std::path::Path::new(
            args.get(i + 1).expect("manifest を指定してください"),
        ))
    });
    Application::new()
        .with_assets(icons::Assets)
        .run(move |cx: &mut App| {
            app::init(cx);
            let bounds = Bounds::centered(None, size(px(1500.0), px(920.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // macOS は透明なタイトルバー、Linux / Windows は OS のタイトルバーとウィンドウ操作を使う。
                    titlebar: Some(TitlebarOptions {
                        title: Some("rireki".into()),
                        appears_transparent: cfg!(target_os = "macos"),
                        traffic_light_position: cfg!(target_os = "macos")
                            .then(|| point(px(14.0), px((app::STRIP_H - 14.0) / 2.0))),
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
