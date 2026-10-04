mod app;
mod db;
mod icons;
mod index;
mod input;
mod jsmock;
mod layout;
mod loader;
mod model;
mod pinch;
mod prefs;
mod search;
mod theme;

use gpui::{
    App, Application, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size,
};

fn main() {
    if std::env::args().any(|a| a == "--stats") {
        return print_stats();
    }
    Application::new().with_assets(icons::Assets).run(|cx: &mut App| {
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
            |window, cx| {
                let view = cx.new(|cx| app::Rireki::new(window, cx));
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
    println!("DB: {n_stable} files in {t_db:.2?} ({})", db::path().display());
    println!("parsed: {n_tasks} files ({n_live} live)");
    println!("{} sessions, {lanes} lanes, {events} events in {:.2?}", visible.len(), t.elapsed());
}
