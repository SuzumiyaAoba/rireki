//! ネイティブ画面で同じ操作を繰り返す計測。--profile-ui でだけ実行する
//! GPUI の描画・提示・要素解放を含む経過時間は ZED_MEASUREMENTS=1 で別途記録できる

use std::time::{Duration, Instant};

use gpui::{Context, ListOffset, Window, px};

use super::{Rireki, Selection, midnight};
use crate::layout::Granularity;
use crate::model::{Event, Kind, Lane, Session, Status};
use crate::prefs::{DetailTab, Mode};

const TICK: Duration = Duration::from_millis(5);

#[derive(Default)]
pub(super) struct State {
    first_frame_queued: bool,
    ready_frame_queued: bool,
    ready_frame: bool,
    rendered_generation: Option<u64>,
    manual: bool,
    eager_files: bool,
}

impl State {
    pub(super) fn new() -> Self {
        let args: Vec<_> = std::env::args().collect();
        Self {
            manual: args.iter().any(|a| a == "--profile-render"),
            eager_files: args.iter().any(|a| a == "--profile-eager-files"),
            ..Self::default()
        }
    }
}

/// 画面ロック中も構築・レイアウト・paint 命令生成を検証する。GPU への present は含まない
fn render_only(cx: &mut gpui::AsyncWindowContext, stage: &str) -> bool {
    let started = Instant::now();
    if cx.update(|window, cx| window.draw(cx).clear()).is_err() {
        return false;
    }
    println!(
        "{}",
        serde_json::json!({"ui_profile":"render", "stage":stage,
        "ms":started.elapsed().as_secs_f64()*1000.0})
    );
    true
}
const STAGES: [(&str, usize); 9] = [
    ("timeline_pan", 120),
    ("timeline_zoom", 120),
    ("timeline_calls", 80),
    ("calendar_scroll", 120),
    ("search_cold", 120),
    ("search_warm", 120),
    ("search_navigation", 120),
    ("selection", 240),
    ("event_navigation", 120),
];
const DETAIL_STAGES: [(&str, usize); 9] = [
    ("selection", 240),
    ("detail_scroll", 240),
    ("detail_filter", 240),
    ("detail_resize", 240),
    ("detail_row_select", 240),
    ("detail_tabs", 240),
    ("detail_follow", 240),
    ("detail_follow_cancel", 240),
    ("detail_live_scrolled", 240),
];
const RICH_STAGES: [(&str, usize); 4] = [
    ("detail_file_expand", 240),
    ("detail_long_single", 240),
    ("calendar_dense_day", 120),
    ("calendar_dense_week", 120),
];
const FILE_STAGES: [(&str, usize); 1] = [("detail_files_checks", 240)];

fn profile_event(index: usize, start: f64) -> Event {
    let (kind, name, input) = match index % 5 {
        0 => (Kind::User, "User", "変更を確認してください".to_string()),
        1 => (
            Kind::Assistant,
            "Assistant",
            "# 確認\n本文とコマンドの折り返しを確認します。".to_string(),
        ),
        3 => (
            Kind::Tool,
            "apply_patch",
            "*** Update File: profile.rs\n@@\n-before\n+after".to_string(),
        ),
        _ => (
            Kind::Tool,
            "exec_command",
            format!("$ printf 'event {index} with a command that wraps across multiple lines in the details panel'"),
        ),
    };
    Event {
        start: start + index as f64,
        end: start + index as f64 + 0.1,
        kind,
        name: name.into(),
        summary: format!("event {index}"),
        input,
        output: "[exit 1 · 0.1s]\nresult".into(),
        call_id: None,
        row: None,
        usage: Default::default(),
        model: None,
        status: if index.is_multiple_of(7) {
            Status::Error
        } else {
            Status::Ok
        },
    }
}

fn summary(stage: &str, metric: &str, values: &mut [f64]) {
    if values.is_empty() {
        return;
    }
    values.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"ui_profile":"summary", "stage":stage, "metric":metric,
        "samples":values.len(),"median_ms":values[values.len()/2],
        "p95_ms":values[(values.len()*95).div_ceil(100)-1],"max_ms":values[values.len()-1]})
    );
}

impl Rireki {
    pub(super) fn profile_eager_files(&self) -> bool {
        self.profile.as_ref().is_some_and(|p| p.eager_files)
    }
    /// 次のフレーム要求で確認するため、render の構築時間だけを起動時間にしない
    pub(super) fn profile_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = &mut self.profile else { return };
        if profile.manual {
            profile.rendered_generation = Some(self.data_gen);
            return;
        }
        let ready = self.loading.is_none();
        let first = !profile.first_frame_queued;
        if !first && (!ready || profile.ready_frame_queued) {
            return;
        }
        profile.first_frame_queued = true;
        profile.ready_frame_queued |= ready;
        let generation = self.data_gen;
        let weak = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            let _ = weak.update(cx, |app, _| {
                let ready = ready && app.loading.is_none() && app.data_gen == generation && window.is_window_active();
                if let Some(profile) = &mut app.profile {
                    profile.ready_frame |= ready;
                    if !ready {
                        profile.ready_frame_queued = false;
                    }
                }
                println!(
                    "{}",
                    serde_json::json!({"ui_profile":"frame_ready", "first":first, "ready":ready,
                    "main_ms":crate::PROCESS_STARTED.get().unwrap().elapsed().as_secs_f64()*1000.0,
                    "generation":generation, "active":window.is_window_active()})
                );
            });
        });
    }

    pub(super) fn start_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let started = Instant::now();
            let manual = std::env::args().any(|a| a == "--profile-render");
            let mut startup_gaps = Vec::new();
            let mut previous = started;
            let bg = cx.background_executor().clone();
            let anchor = loop {
                bg.timer(TICK).await;
                let now = Instant::now();
                startup_gaps.push(now.duration_since(previous).as_secs_f64()*1000.0);
                previous = now;
                if manual && !render_only(cx, "startup") { return; }
                let Ok(ready) = this.update_in(cx, |app, window, cx| {
                    if !window.is_window_active() {
                        window.activate_window();
                        cx.activate(true);
                    }
                    if app.loading.is_none() && app.profile.as_ref().is_some_and(|p| !p.ready_frame && !p.ready_frame_queued) {
                        cx.notify();
                    }
                    (app.profile.as_ref().is_some_and(|p| if manual {
                        app.loading.is_none() && p.rendered_generation == Some(app.data_gen)
                    } else { p.ready_frame })).then(|| app.order.iter().copied().max_by_key(|&si| {
                        app.store.sessions[si].lanes.first().map_or(0, |l| l.events.len())
                    })).flatten()
                }) else { return };
                if let Some(si) = ready { break si; }
                if started.elapsed() > Duration::from_secs(60) {
                    eprintln!("rireki: UI 計測のデータ読み込みが完了しません");
                    let _ = this.update_in(cx, |_, _, cx| cx.quit());
                    return;
                }
            };
            summary("startup", "foreground_interval", &mut startup_gaps);
            let _ = this.update_in(cx, |app, _, _| {
                println!("{}", serde_json::json!({"ui_profile":"ready", "startup_ms":started.elapsed().as_secs_f64()*1000.0,
                    "main_to_ready_ms":crate::PROCESS_STARTED.get().unwrap().elapsed().as_secs_f64()*1000.0,
                    "render_only":manual,
                    "sessions":app.store.sessions.len(),
                    "events":app.store.sessions.iter().flat_map(|s| &s.lanes).map(|l| l.events.len()).sum::<usize>(),
                    "anchor_events":app.store.sessions[anchor].lanes[0].events.len()}));
            });
            let args: Vec<_> = std::env::args().collect();
            if let Some(index) = args.iter().position(|a| a == "--profile-idle") {
                let seconds: u64 = args.get(index + 3).map_or(15, |s| s.parse().expect("待機秒数を指定してください"));
                let generation = this.update_in(cx, |app, _, _| app.data_gen).ok();
                let idle = Instant::now();
                bg.timer(Duration::from_secs(seconds)).await;
                let _ = this.update_in(cx, |app, _, _| {
                    println!("{}", serde_json::json!({"ui_profile":"idle_end", "seconds":idle.elapsed().as_secs_f64(),
                        "unchanged":generation == Some(app.data_gen), "loading":app.loading.is_some(),
                        "sessions":app.store.sessions.len()}));
                });
                println!("{}", serde_json::json!({"ui_profile":"done"}));
                let _ = this.update_in(cx, |_, _, cx| cx.quit());
                return;
            }
            if std::env::args().any(|arg| arg == "--profile-startup") {
                bg.timer(Duration::from_millis(200)).await;
                println!("{}", serde_json::json!({"ui_profile":"done"}));
                let _ = this.update_in(cx, |_, _, cx| cx.quit());
                return;
            }
            let stages: &[(&str, usize)] = if args.iter().any(|arg| arg == "--profile-files") {
                &FILE_STAGES
            } else if args.iter().any(|arg| arg == "--profile-rich") {
                &RICH_STAGES
            } else if args.iter().any(|arg| arg == "--profile-detail") {
                &DETAIL_STAGES
            } else {
                &STAGES
            };
            for &(stage, samples) in stages {
                println!("{}", serde_json::json!({"ui_profile":"stage_start","stage":stage}));
                let mut dispatch = Vec::with_capacity(samples);
                let mut gaps = Vec::with_capacity(samples);
                let mut previous = Instant::now();
                for step in 0..samples {
                    bg.timer(TICK).await;
                    let now = Instant::now();
                    gaps.push(now.duration_since(previous).as_secs_f64()*1000.0);
                    previous = now;
                    let t = Instant::now();
                    if this.update_in(cx, |app, window, cx| app.profile_step(stage, step, anchor, window, cx)).is_err() { return; }
                    dispatch.push(t.elapsed().as_secs_f64()*1000.0);
                    if manual && !render_only(cx, stage) { return; }
                    if step == 0 && stage.starts_with("detail_") {
                        let wait = Instant::now();
                        loop {
                            bg.timer(TICK).await;
                            if manual && !render_only(cx, stage) { return; }
                            let Ok(ready) = this.update_in(cx, |app, _, _| {
                                app.detail_cache.is_some() && (!app.detail_uses_list()
                                    || (app.detail_list.state.item_count() > 0
                                        && app.detail_list.state.viewport_bounds().size.height > px(0.0)))
                            }) else { return; };
                            if ready { break; }
                            if wait.elapsed() > Duration::from_secs(10) {
                                eprintln!("rireki: 詳細計測の初回描画が完了しません");
                                let _ = this.update_in(cx, |_, _, cx| cx.quit());
                                return;
                            }
                        }
                    }
                }
                summary(stage, "dispatch", &mut dispatch);
                summary(stage, "foreground_interval", &mut gaps);
                let _ = this.update_in(cx, |app, window, _| {
                    println!("{}", serde_json::json!({"ui_profile":"state", "stage":stage,
                        "query_set":!app.query.is_empty(), "results":app.results.as_ref().map(|r| r.total),
                        "detail_ready":app.detail_cache.is_some(), "painted_blocks":app.hits.len(), "active":window.is_window_active(), "cursor":app.result_cursor,
                        "detail":app.detail_profile_state()}));
                });
                println!("{}", serde_json::json!({"ui_profile":"stage_end","stage":stage}));
                bg.timer(Duration::from_millis(200)).await;
            }
            println!("{}", serde_json::json!({"ui_profile":"done"}));
            let _ = this.update_in(cx, |_, _, cx| cx.quit());
        }).detach();
    }

    fn profile_step(&mut self, stage: &str, step: usize, anchor: usize, window: &mut Window, cx: &mut Context<Self>) {
        if step == 0 {
            window.activate_window();
            cx.activate(true);
            self.search_input.update(cx, |input, cx| input.set_text("", cx));
            self.close_detail(cx);
            self.detail_t = 0.0;
            self.mode = Mode::Timeline;
            self.gran = Granularity::Auto;
            self.in_view_only = true;
            let s = &self.store.sessions[anchor];
            self.t0 = s.start;
            self.spp = ((s.end - s.start) / self.plot_width(window) as f64).max(0.01);
            self.day0 = super::local(s.start).date_naive();
            self.row_scroll = 0.0;
            self.cal_scroll = 8.0 * self.px_per_hour;
            if stage.starts_with("detail_") {
                let session = if stage == "detail_long_single" {
                    self.profile_rich_session(anchor)
                } else if matches!(stage, "detail_follow" | "detail_follow_cancel" | "detail_live_scrolled") {
                    self.profile_session(anchor)
                } else {
                    anchor
                };
                self.detail_tab = DetailTab::Events;
                let last = if stage == "detail_long_single" {
                    0
                } else {
                    self.store.sessions[session].lanes[0].events.len() - 1
                };
                self.navigate(
                    Selection {
                        session,
                        lane: 0,
                        first: 0,
                        last,
                    },
                    window,
                    cx,
                );
                self.detail_t = 1.0;
            }
        }
        match stage {
            "timeline_pan" => self.t0 += self.spp * 2.0,
            "timeline_zoom" => self.spp *= if step < 60 { 0.985 } else { 1.0 / 0.985 },
            "timeline_calls" => {
                self.gran = Granularity::Call;
                self.row_scroll += 2.0;
            }
            "calendar_scroll" => {
                self.mode = Mode::Week;
                self.cal_scroll = step as f32 * 3.0;
            }
            "calendar_dense_day" | "calendar_dense_week" => {
                self.mode = if stage.ends_with("week") { Mode::Week } else { Mode::Day };
                self.px_per_hour = self.plot_height(window) / 24.0 + (step % 20) as f32 * 0.05;
                self.cal_scroll = 0.0;
                self.gran = Granularity::Call;
            }
            "search_cold" | "search_warm" | "search_navigation" if step == 1 => {
                self.search_input.update(cx, |input, cx| input.set_text("cargo", cx));
            }
            "search_navigation" if step >= 40 => self.move_cursor(1, cx),
            "selection" if step == 0 => {
                self.detail_tab = DetailTab::Events;
                let last = self.store.sessions[anchor].lanes[0].events.len() - 1;
                self.navigate(
                    Selection {
                        session: anchor,
                        lane: 0,
                        first: 0,
                        last,
                    },
                    window,
                    cx,
                );
                self.detail_t = 1.0;
            }
            "event_navigation" if step.is_multiple_of(10) => {
                self.detail_tab = DetailTab::Events;
                let event = (step / 10).min(self.store.sessions[anchor].lanes[0].events.len() - 1);
                self.navigate(
                    Selection {
                        session: anchor,
                        lane: 0,
                        first: event,
                        last: event,
                    },
                    window,
                    cx,
                );
                self.detail_t = 1.0;
                self.t0 = midnight(self.day0);
            }
            "detail_scroll" if (40..220).contains(&step) && step.is_multiple_of(3) => {
                self.detail_list
                    .state
                    .scroll_by(px(if step < 160 { 80.0 } else { -80.0 }));
                self.detail_changed(cx);
            }
            "detail_filter" => match step {
                40 => self.set_row_filter(Some(super::RowFilter::Name("exec_command".into())), cx),
                80 => self.set_row_filter(Some(super::RowFilter::Failed), cx),
                120 => self.set_row_filter(None, cx),
                160 => self.set_row_filter(Some(super::RowFilter::Name("__profile_no_matches__".into())), cx),
                _ => {}
            },
            "detail_resize" => {
                if step == 40 {
                    self.detail_list.state.scroll_to(ListOffset {
                        item_ix: 80,
                        offset_in_item: px(17.0),
                    });
                }
                if (60..180).contains(&step) {
                    self.detail_w = 320.0 + (step % 60) as f32 * 3.0;
                    self.detail_changed(cx);
                }
            }
            "detail_row_select" => {
                if step == 40 {
                    self.detail_list.state.scroll_to(ListOffset {
                        item_ix: 80,
                        offset_in_item: px(0.0),
                    });
                    self.detail_changed(cx);
                }
                if step == 80
                    && let Some((_, target)) = self.detail_profile_click()
                    && let Some(sel) = self.selected
                {
                    println!(
                        "{}",
                        serde_json::json!({"ui_profile":"row_target","stage":stage,"event":target})
                    );
                    self.navigate(
                        Selection {
                            first: target,
                            last: target,
                            ..sel
                        },
                        window,
                        cx,
                    );
                }
            }
            "detail_tabs" => match step {
                40 => self.set_detail_tab(DetailTab::Files, cx),
                80 => self.set_detail_tab(DetailTab::Session, cx),
                120 => self.set_detail_tab(DetailTab::Events, cx),
                _ => {}
            },
            "detail_file_expand" => {
                if step == 40 {
                    self.set_detail_tab(DetailTab::Files, cx);
                    self.detail_profile_open_file(cx);
                }
                if step == 80 {
                    self.expanded.insert("file-diff");
                    self.detail_changed(cx);
                }
                if (100..200).contains(&step) && step.is_multiple_of(3) {
                    self.detail_profile_scroll_files(px(80.0), cx);
                }
                if step == 220 {
                    self.file_open = None;
                    self.expanded.remove("file-diff");
                    self.detail_changed(cx);
                }
            }
            "detail_files_checks" => match step {
                40 => {
                    self.set_detail_tab(DetailTab::Files, cx);
                    self.detail_profile_open_file(cx);
                }
                60 => {
                    self.file_list.state.scroll_to(ListOffset {
                        item_ix: 23,
                        offset_in_item: px(17.0),
                    });
                    self.detail_changed(cx);
                }
                80 => {
                    self.expanded.insert("file-diff");
                    self.detail_changed(cx);
                }
                120..=159 => {
                    self.detail_w = 320.0 + (step % 40) as f32 * 3.0;
                    self.detail_changed(cx);
                }
                160 => self.detail_profile_reorder_files(cx),
                200 => self.detail_profile_open_file_edit(window, cx),
                _ => {}
            },
            "detail_long_single" => {
                let event = match step {
                    60 => Some(1),
                    140 => Some(3),
                    220 => Some(2),
                    _ => None,
                };
                if let Some(event) = event
                    && let Some(sel) = self.selected
                {
                    self.navigate(
                        Selection {
                            first: event,
                            last: event,
                            ..sel
                        },
                        window,
                        cx,
                    );
                    self.detail_t = 1.0;
                }
                if matches!(step, 100 | 180 | 230) {
                    self.expanded.insert("in");
                    self.expanded.insert("out");
                    self.detail_changed(cx);
                }
                if step % 10 == 9 {
                    self.detail_scroll.set_offset(gpui::point(
                        px(0.0),
                        -self.detail_scroll.max_offset().height * (step % 40) as f32 / 40.0,
                    ));
                    self.detail_changed(cx);
                }
            }
            "detail_follow" | "detail_follow_cancel" | "detail_live_scrolled" => {
                if step == 40 {
                    self.detail_list.state.scroll_to(ListOffset {
                        item_ix: if stage == "detail_live_scrolled" {
                            15
                        } else {
                            self.detail_list.state.item_count()
                        },
                        offset_in_item: if stage == "detail_live_scrolled" {
                            px(17.0)
                        } else {
                            px(0.0)
                        },
                    });
                    self.detail_changed(cx);
                }
                if step == 60 || (stage == "detail_follow" && step == 100) {
                    self.profile_append(if stage == "detail_follow_cancel" { 40 } else { 10 }, cx);
                }
                if stage == "detail_follow_cancel" && step == 70 {
                    self.cancel_detail_follow();
                    self.detail_list.state.scroll_by(px(-80.0));
                    self.detail_changed(cx);
                }
            }
            _ => {}
        }
        if stage.starts_with("detail_") && step % 40 == 39 {
            println!(
                "{}",
                serde_json::json!({"ui_profile":"checkpoint","stage":stage,"step":step,"detail":self.detail_profile_state(),
                "selected_first":self.selected.map(|s| s.first),"selected_last":self.selected.map(|s| s.last)})
            );
        }
        cx.notify();
    }

    fn profile_session(&mut self, anchor: usize) -> usize {
        let source = &self.store.sessions[anchor];
        let mut lane = Lane::new("main");
        lane.origin = "rireki-ui-profile".into();
        let start = super::now();
        lane.events = (0..60).map(|i| profile_event(i, start)).collect();
        let mut session = Session {
            id: "rireki-ui-profile".into(),
            source: source.source,
            cwd: source.cwd.clone(),
            headless: false,
            title: "UI profile".into(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start,
            end: start,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        self.store.sessions.push(std::sync::Arc::new(session));
        self.rebuild_order();
        self.store.sessions.len() - 1
    }

    fn profile_rich_session(&mut self, anchor: usize) -> usize {
        let index = self.profile_session(anchor);
        let session = std::sync::Arc::make_mut(&mut self.store.sessions[index]);
        session.lanes[0].events.truncate(4);
        let events = &mut session.lanes[0].events;
        events[0].input = "# 長文の確認\n\n- **項目**と `code`\n\n> 引用した文章\n\n".repeat(90);
        events[1].kind = Kind::Tool;
        events[1].name = "exec_command".into();
        events[1].input = format!("$ printf '{}'", "日本語 command ".repeat(350));
        events[1].output = "\u{1b}[31merror 日本語\u{1b}[0m\n".repeat(200);
        events[2].name = "apply_patch".into();
        events[2].input = format!(
            "*** Update File: profile.rs\n@@ -1,150 +1,150 @@\n{}",
            "-before\n+after\n".repeat(150)
        );
        events[3].name = "read_file".into();
        events[3].input = "$ cat -n profile.rs".into();
        events[3].output = (1..=250).map(|i| format!("{i:6}\tline 日本語 text\n")).collect();
        session.finalize();
        self.rebuild_order();
        index
    }

    fn profile_append(&mut self, count: usize, cx: &mut Context<Self>) {
        let Some(sel) = self.selected else {
            return;
        };
        let mut snapshot = self.store.clone();
        let session = std::sync::Arc::make_mut(&mut snapshot.sessions[sel.session]);
        let start = session.start;
        let events = &mut session.lanes[sel.lane].events;
        let n = events.len();
        events.extend((n..n + count).map(|i| profile_event(i, start)));
        session.finalize();
        self.apply(
            super::Delta {
                snapshot,
                touched: vec![sel.session],
            },
            cx,
        );
    }
}
