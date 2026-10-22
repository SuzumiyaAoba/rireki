//! ビューの初期化と背景での履歴読み込み。

use super::{
    Arc, AtomicU64, Bounds, Cell, Context, DetailList, DetailLoader, DetailView, FileList, HashSet,
    Local, MAX_SIDEBAR_W, MIN_DETAIL_W, MIN_PANE_W, MIN_SIDEBAR_W, Mode, Mutex, POLL, Point, Rc,
    RefCell, Rireki, ScrollHandle, SidebarView, Snapshot, TextInput, ViewLoader, Window, calendar,
    cost, detail, index, now, point, prefs, profile, px, sidebar, statistics,
};
use gpui::prelude::*;

impl Rireki {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        mut profile_tasks: Option<Vec<index::Task>>,
    ) -> Self {
        // トラックパッドのピンチでズームする（溜まった分はまとめて反映）
        let pinch = crate::pinch::listen();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(mut m) = pinch.recv().await {
                while let Ok(more) = pinch.try_recv() {
                    m += more;
                }
                if this
                    .update_in(cx, |t, window, cx| t.on_pinch(m, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let profile_ui = std::env::args().any(|a| a == "--profile-ui");
        let args: Vec<_> = std::env::args().collect();
        let profile_roots = profile_ui
            .then(|| args.iter().position(|a| a == "--profile-idle"))
            .flatten()
            .map(|i| {
                (
                    std::path::PathBuf::from(
                        args.get(i + 1).expect("Claude の探索先を指定してください"),
                    ),
                    std::path::PathBuf::from(
                        args.get(i + 2).expect("Codex の探索先を指定してください"),
                    ),
                )
            });
        cx.spawn(async move |this, cx| {
            let bg = cx.background_executor().clone();
            // 1. 索引 DB から安定した履歴を読む
            let indexer = bg
                .spawn(async { Arc::new(Mutex::new(ViewLoader::open())) })
                .await;
            let ix = indexer.clone();
            let stable = bg
                .spawn(async move { ix.lock().unwrap().load_stable() })
                .await;
            let alive = this
                .update(cx, |app, cx| {
                    app.apply(stable, cx);
                    cx.notify();
                })
                .is_ok();
            if !alive {
                return;
            }
            // 2. 未索引・更新されたファイルを取り込み、以後はリアルタイムファイルを監視する
            let mut first = true;
            let mut minute = 0;
            loop {
                let ix = indexer.clone();
                let planned = profile_ui.then(std::time::Instant::now);
                let tasks = if let Some(tasks) = profile_tasks.take() {
                    tasks
                } else if let Some((claude, codex)) = &profile_roots {
                    let (claude, codex) = (claude.clone(), codex.clone());
                    bg.spawn(async move { ix.lock().unwrap().plan_in(&claude, &codex) })
                        .await
                } else if profile_ui {
                    Vec::new()
                } else {
                    bg.spawn(async move { ix.lock().unwrap().plan() }).await
                };
                let total = tasks.len();
                assert!(
                    profile_roots.is_none() || total == 0,
                    "アイドル計測の固定ログはすべて索引済みである必要があります"
                );
                if let Some(planned) = planned {
                    println!(
                        "{}",
                        serde_json::json!({"ui_profile":"poll", "tasks":total,
                        "ms":planned.elapsed().as_secs_f64()*1000.0})
                    );
                }
                let mut done = 0;
                let mut tasks = tasks.into_iter().peekable();
                while tasks.peek().is_some() {
                    let chunk: Vec<_> = tasks.by_ref().take(index::CHUNK).collect();
                    done += chunk.len();
                    let ix = indexer.clone();
                    let updates = bg
                        .spawn(async move { ix.lock().unwrap().process(&chunk) })
                        .await;
                    let alive = this
                        .update(cx, |app, cx| {
                            let loading = (first && done < total).then_some((done, total));
                            // 変化がなければ描き直さない（3 秒ごとのポーリングで毎回全体を描かない）
                            if app.apply(updates, cx) || app.loading != loading {
                                app.loading = loading;
                                cx.notify();
                            }
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
                // 「3分前」などの表示と進行中の印、現在時刻の線を分が変わるごとに更新する
                let m = (now() / 60.0) as i64;
                if first || m != minute {
                    minute = m;
                    let was_first = std::mem::replace(&mut first, false);
                    let alive = this
                        .update(cx, |app, cx| {
                            if was_first {
                                app.loading = None;
                            }
                            app.live_sources = app.find_live_sources();
                            app.sidebar_changed(cx);
                            app.detail_changed(cx);
                            cx.notify();
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
                bg.timer(POLL).await;
            }
        })
        .detach();

        let search_state = super::search_controller::State::new(cx);

        let p = prefs::load();
        let today = Local::now().date_naive();
        let weak = cx.weak_entity();
        let detail_view = cx.new(|_| DetailView { app: weak.clone() });
        let sidebar_view = cx.new(|_| SidebarView { app: weak });
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let analysis_state = detail::analysis::State::new(cx);
        let mcp_port = cx.new(|cx| {
            let mut input = TextInput::new("ポート番号", cx);
            input.set_text(&crate::mcp::DEFAULT_PORT.to_string(), cx);
            input
        });
        let mut this = Self {
            store: Snapshot::default(),
            mcp_server: None,
            mcp_busy: false,
            mcp_error: None,
            mcp_port,
            layout_cache: RefCell::new(
                if profile_ui && std::env::args().any(|a| a == "--profile-full-grouping") {
                    crate::layout::Cache::with_full_history()
                } else if profile_ui && std::env::args().any(|a| a == "--profile-exact-group-scale")
                {
                    crate::layout::Cache::with_exact_scales()
                } else {
                    crate::layout::Cache::default()
                },
            ),
            calendar_cache: calendar::Cache::default(),
            scope: Vec::new(),
            order: Vec::new(),
            order_extent: Cell::new(None),
            projects: Vec::new(),
            exec_parent: Default::default(),
            track_index: Default::default(),
            loading: Some((0, 0)),
            profile: profile_ui.then(profile::State::new),
            detail_loader: Arc::new(Mutex::new(DetailLoader::default())),
            detail_gen: Arc::new(AtomicU64::new(0)),
            detail_task: None,
            detail_follow: false,
            detail_following: false,
            focus_handle,
            search_state,
            sidebar_view,
            sidebar_scroll: ScrollHandle::new(),
            navigator_rendered: Cell::new(0),
            navigator_virtual: Cell::new(false),
            navigator_list: sidebar::NavigatorList::new(),
            detail_view,
            detail_cache: None,
            detail_snapshot: None,
            show_raw: false,
            row_filter: None,
            expanded: HashSet::new(),
            file_open: None,
            detail_tab: p.detail_tab,
            detail_reversed: p.detail_reversed,
            detail_fullscreen: false,
            analysis_state,
            detail_scroll: ScrollHandle::new(),
            detail_scroll_offsets: [point(px(0.0), px(0.0)); 4],
            overview_list: detail::overview::OverviewList::new(),
            history_expanded: HashSet::new(),
            detail_list: DetailList::new(),
            replay: None,
            replay_speed: 1.0,
            replay_task: None,
            replay_autostart: None,
            file_list: FileList::new(),
            follow_anim: None,
            copied: None,
            hits: Vec::new(),
            hit_origin: Rc::new(Cell::new(Point::default())),
            hovered: None,
            hover_at: Point::default(),
            mode: p.mode,
            statistics_open: p.statistics,
            statistics_state: statistics::State::new(),
            calendar: if p.calendar == Mode::Timeline {
                Mode::Week
            } else {
                p.calendar
            },
            t0: now(),
            spp: 60.0,
            row_scroll: 0.0,
            day0: today,
            px_per_hour: 60.0,
            cal_scroll: 8.0 * 60.0,
            gran: p.gran,
            in_view_only: p.in_view_only,
            directory_timeline: p.directory_timeline,
            cost_timeline: p.cost_timeline,
            cost_state: cost::State::new(),
            source: p.source,
            live_sources: (false, false),
            selected: None,
            back: Vec::new(),
            forward: Vec::new(),
            drag: None,
            sidebar_open: p.sidebar,
            sidebar_w: p.sidebar_w.clamp(MIN_SIDEBAR_W, MAX_SIDEBAR_W),
            label_w: p.label_w.max(MIN_PANE_W),
            detail_w: p.detail_w.max(MIN_DETAIL_W),
            side_t: if p.sidebar { 1.0 } else { 0.0 },
            detail_t: 0.0,
            anim_at: None,
            resize: None,
            collapsed: p.collapsed.iter().cloned().collect(),
            focus: p.focus.clone(),
            folded: p.folded.iter().cloned().collect(),
            session_limit: 50,
            popover: None,
            display_anchor: Rc::new(Cell::new(Bounds::default())),
            pick: None,
            picker_month: today,
            picker_from: None,
            picker_anchor: Rc::new(Cell::new(Bounds::default())),
            minimap_bounds: Rc::new(Cell::new(Bounds::default())),
            minimap_drag: false,
            lane_map_bounds: Rc::new(Cell::new(Bounds::default())),
            lane_map_drag: false,
            minimap_cache: None,
            data_gen: 0,
            timeline_cache: RefCell::default(),
            counts: String::new(),
            saved: p,
        };
        // 現在時刻が右寄りに見えるように始める
        this.go_today(window);
        // SHOT-HOOK: 一時的なスクリーンショット用
        if let Ok(d) = std::env::var("RIREKI_SHOT_DAY") {
            this.day0 = d.parse().unwrap();
        }
        if let Ok(v) = std::env::var("RIREKI_SHOT_SCROLL") {
            this.cal_scroll = v.parse().unwrap();
        }
        if let Ok(v) = std::env::var("RIREKI_SHOT_PPH") {
            this.px_per_hour = v.parse().unwrap();
        }
        if std::env::args().any(|a| a == "--profile-ui") {
            this.start_profile(window, cx);
        }
        this
    }
}
