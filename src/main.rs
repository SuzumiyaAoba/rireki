#![cfg_attr(
    all(
        feature = "gui",
        target_os = "windows",
        not(debug_assertions),
        not(test)
    ),
    windows_subsystem = "windows"
)]

#[cfg(feature = "gui")]
mod app;
#[cfg(feature = "gui")]
mod icons;
#[cfg(feature = "gui")]
mod input;
#[cfg(feature = "gui")]
mod pinch;
#[cfg(feature = "gui")]
mod platform;
#[cfg(feature = "gui")]
mod prefs;
#[cfg(feature = "gui")]
mod theme;

#[cfg(feature = "gui")]
use rireki::{
    cost_timeline, detail, histogram, index, jsmock, layout, mcp, model, patch, pricing, search,
    statistics, time, view_index,
};

#[cfg(feature = "gui")]
use gpui::{
    App, Application, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, point,
    prelude::*, px, size,
};

#[cfg(feature = "gui")]
static PROCESS_STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    #[cfg(feature = "gui")]
    PROCESS_STARTED.set(std::time::Instant::now()).unwrap();
    let args: Vec<_> = std::env::args().collect();
    #[cfg(all(feature = "gui", target_os = "windows"))]
    if args.len() > 1 {
        platform::attach_parent_console();
    }
    #[cfg(feature = "gui")]
    run_gui(&args);
    #[cfg(not(feature = "gui"))]
    {
        let _ = args;
        eprintln!(
            "このバイナリには GUI が含まれていません。gui feature を有効にしてビルドしてください。"
        );
        std::process::exit(1);
    }
}

#[cfg(feature = "gui")]
fn run_gui(args: &[String]) {
    let profile_ui = args.iter().any(|a| a == "--profile-ui");
    if args.iter().any(|a| {
        matches!(
            a.as_str(),
            "--profile-startup"
                | "--profile-idle"
                | "--profile-render"
                | "--profile-eager-files"
                | "--profile-detail"
                | "--profile-detail-updates"
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
                    let view = cx.new(|cx| app::Rireki::new(window, cx));
                    window.focus(&view.focus_handle(cx));
                    view
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
