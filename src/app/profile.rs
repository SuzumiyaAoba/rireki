//! ネイティブ画面で同じ操作を繰り返す計測。--profile-ui でだけ実行する
//! GPUI の描画・提示・要素解放を含む経過時間は ZED_MEASUREMENTS=1 で別途記録できる

use std::time::{Duration, Instant};
use std::cell::Cell;
use std::rc::Rc;

use gpui::{Context, ListOffset, Window, px};

use super::{Rireki, Selection, midnight};
use crate::layout::Granularity;
use crate::model::{Event, Kind, Lane, Session, Status};
use crate::prefs::{DetailTab, Mode};

const TICK: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Default, serde::Serialize)]
pub(super) struct TimelineMeasurements {
    pub(super) build_ms: f64,
    pub(super) rows_ms: f64,
    pub(super) aggregate_ms: f64,
    pub(super) description_ms: f64,
    pub(super) paint_ms: f64,
    pub(super) paint_cpu_ms: f64,
    pub(super) minimap_ms: f64,
    pub(super) minimap_cpu_ms: f64,
    pub(super) minimap_calls: usize,
    pub(super) minimap_quads: usize,
    pub(super) lane_map_ms: f64,
    pub(super) lane_map_cpu_ms: f64,
    pub(super) lane_map_calls: usize,
    pub(super) lane_map_quads: usize,
    pub(super) build_calls: usize,
    pub(super) aggregate_calls: usize,
    pub(super) paint_calls: usize,
    #[serde(skip)]
    pub(super) paint_hash: u64,
    #[serde(skip)]
    pub(super) minimap_hash: u64,
    #[serde(skip)]
    pub(super) lane_map_hash: u64,
}

#[cfg(target_os = "macos")]
pub(super) fn thread_cpu_ns() -> Option<u64> {
    unsafe extern "C" { fn clock_gettime_nsec_np(clock: i32) -> u64; }
    // Apple SDK の CLOCK_THREAD_CPUTIME_ID は 16。引数は clockid_t、戻り値はナノ秒。
    let value = unsafe { clock_gettime_nsec_np(16) };
    (value != 0).then_some(value)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn thread_cpu_ns() -> Option<u64> { None }

pub(super) fn cpu_delta_ms(start: Option<u64>) -> Option<f64> {
    start.zip(thread_cpu_ns()).and_then(|(a, b)| b.checked_sub(a)).map(|ns| ns as f64 / 1_000_000.0)
}

#[derive(Default)]
pub(super) struct State {
    first_frame_queued: bool,
    ready_frame_queued: bool,
    ready_frame: bool,
    rendered_generation: Option<u64>,
    manual: bool,
    eager_files: bool,
    pub(super) timeline: bool,
    pub(super) uncached_rows: bool,
    pub(super) sidebar: bool,
    pub(super) eager_sidebar: bool,
    pub(super) unbatched_minimap: bool,
    pub(super) unbatched_lane_map: bool,
    dense_timeline: bool,
    pub(super) measurements: Option<Rc<Cell<TimelineMeasurements>>>,
}

impl State {
    pub(super) fn new() -> Self {
        let args: Vec<_> = std::env::args().collect();
        Self {
            manual: args.iter().any(|a| a == "--profile-render"),
            eager_files: args.iter().any(|a| a == "--profile-eager-files"),
            timeline: args.iter().any(|a| matches!(a.as_str(), "--profile-timeline" | "--profile-dense-timeline" | "--profile-lane-map")),
            uncached_rows: args.iter().any(|a| a == "--profile-uncached-rows"),
            sidebar: args.iter().any(|a| a == "--profile-sidebar"),
            eager_sidebar: args.iter().any(|a| a == "--profile-eager-sidebar"),
            unbatched_minimap: args.iter().any(|a| a == "--profile-unbatched-minimap"),
            unbatched_lane_map: args.iter().any(|a| a == "--profile-unbatched-lane-map"),
            dense_timeline: args.iter().any(|a| a == "--profile-dense-timeline"),
            measurements: args.iter().any(|a| a == "--profile-components").then(Default::default),
            ..Self::default()
        }
    }
}

/// 画面ロック中も構築・レイアウト・paint 命令生成を検証する。GPU への present は含まない
fn render_only(cx: &mut gpui::AsyncWindowContext, stage: &str, measurements: Option<&Rc<Cell<TimelineMeasurements>>>) -> bool {
    if let Some(measurements) = measurements { measurements.set(TimelineMeasurements::default()); }
    let started = Instant::now();
    let Ok(cpu_ms) = cx.update(|window, cx| {
        let cpu_started = measurements.and_then(|_| thread_cpu_ns());
        window.draw(cx).clear();
        measurements.and_then(|_| cpu_delta_ms(cpu_started))
    }) else { return false; };
    println!(
        "{}",
        serde_json::json!({"ui_profile":"render", "stage":stage,
        "ms":started.elapsed().as_secs_f64()*1000.0, "cpu_ms":cpu_ms,
        "components":measurements.map(|m| m.get())})
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
const DETAIL_STAGES: [(&str, usize); 10] = [
    ("selection", 240),
    ("detail_scroll", 240),
    ("detail_filter", 240),
    ("detail_resize", 240),
    ("detail_row_select", 240),
    ("detail_tabs", 240),
    ("detail_follow", 240),
    ("detail_follow_cancel", 240),
    ("detail_live_scrolled", 240),
    ("detail_refresh", 240),
];
const RICH_STAGES: [(&str, usize); 4] = [
    ("detail_file_expand", 240),
    ("detail_long_single", 240),
    ("calendar_dense_day", 120),
    ("calendar_dense_week", 120),
];
const CALENDAR_STAGES: [(&str, usize); 5] = [
    ("calendar_scroll", 120),
    ("calendar_scroll_all", 120),
    ("calendar_zoom", 120),
    ("calendar_dates", 40),
    ("calendar_day_scroll", 120),
];
const TIMELINE_STAGES: [(&str, usize); 8] = [
    ("timeline_lane_scroll", 120),
    ("timeline_lane_scroll_all", 120),
    ("timeline_directory_scroll", 120),
    ("timeline_directory_pan", 80),
    ("timeline_directory_zoom", 80),
    ("timeline_collapse", 80),
    ("timeline_scope", 80),
    ("timeline_update", 80),
];
const SIDEBAR_STAGES: [(&str, usize); 8] = [
    ("sidebar_refresh", 120), ("sidebar_all", 120), ("sidebar_scroll", 120),
    ("sidebar_more", 80), ("sidebar_fold", 80), ("sidebar_scope", 80),
    ("sidebar_resize", 80), ("sidebar_update", 80),
];
const LANE_MAP_STAGES: [(&str, usize); 5] = [
    ("lane_map_all_scroll", 80), ("lane_map_directory_scroll", 80),
    ("lane_map_selection", 80), ("lane_map_collapse", 80), ("lane_map_zoom", 80),
];
const DENSE_TIMELINE_STAGES: [(&str, usize); 8] = [
    ("timeline_dense_pan", 80), ("timeline_dense_zoom", 80),
    ("timeline_dense_same_tool", 80), ("timeline_dense_calls", 80),
    ("timeline_dense_turns", 80), ("timeline_dense_directory_pan", 80),
    ("timeline_dense_directory_zoom", 80), ("timeline_dense_update", 80),
];
const FILE_STAGES: [(&str, usize); 1] = [("detail_files_checks", 240)];
const ANALYSIS_STAGES: [(&str, usize); 2] = [
    ("detail_fullscreen", 240),
    ("detail_analysis", 280),
];
const SUMMARY_STAGES: [(&str, usize); 2] = [
    ("detail_summary_selection", 120),
    ("detail_summary_session", 120),
];
const STATISTICS_STAGES: [(&str, usize); 6] = [
    ("statistics_open", 120),
    ("statistics_period", 120),
    ("statistics_scope", 120),
    ("statistics_source", 120),
    ("statistics_update", 120),
    ("statistics_cancel", 120),
];

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
        summary: format!("event {index}").into(),
        input,
        output: "[exit 1 · 0.1s]\nresult".into(),
        call_id: None,
        text_ref: Default::default(),
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
        let measurements = self.profile.as_ref().and_then(|p| p.measurements.clone());
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
                if manual && !render_only(cx, "startup", measurements.as_ref()) { return; }
                let Ok(ready) = this.update_in(cx, |app, window, cx| {
                    // 前回の模擬プロジェクトは索引に保存しない。残った絞り込みを解除して起動待ちを進める。
                    if app.loading.is_none() && app.order.is_empty() && app.profile.as_ref().is_some_and(|p| p.dense_timeline) {
                        app.set_source(crate::prefs::SourceFilter::All, cx);
                        app.set_focus(None, cx);
                    }
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
            let paired_minimap = args.iter().any(|a| a == "--profile-paired-minimap");
            let paired_lane_map = args.iter().any(|a| a == "--profile-paired-lane-map");
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
            if args.iter().any(|arg| arg == "--profile-dense-timeline") {
                let _ = this.update_in(cx, |app, _, _| app.profile_dense_sessions(anchor));
            }
            let stages: &[(&str, usize)] = if args.iter().any(|arg| arg == "--profile-lane-map") {
                &LANE_MAP_STAGES
            } else if args.iter().any(|arg| arg == "--profile-dense-timeline") {
                &DENSE_TIMELINE_STAGES
            } else if args.iter().any(|arg| arg == "--profile-analysis") {
                &ANALYSIS_STAGES
            } else if args.iter().any(|arg| arg == "--profile-statistics") {
                &STATISTICS_STAGES
            } else if args.iter().any(|arg| arg == "--profile-sidebar") {
                &SIDEBAR_STAGES
            } else if args.iter().any(|arg| arg == "--profile-calendar") {
                &CALENDAR_STAGES
            } else if args.iter().any(|arg| arg == "--profile-timeline") {
                &TIMELINE_STAGES
            } else if args.iter().any(|arg| arg == "--profile-summary") {
                &SUMMARY_STAGES
            } else if args.iter().any(|arg| arg == "--profile-files") {
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
                    if paired_minimap || paired_lane_map {
                        // 同じ位置を二回描き、初回と再利用の影響が片方に偏らないよう順番を交互にする。
                        for unbatched in if step.is_multiple_of(2) { [true, false] } else { [false, true] } {
                            if this.update_in(cx, |app, _, cx| {
                                if paired_minimap { app.profile.as_mut().unwrap().unbatched_minimap = unbatched; }
                                if paired_lane_map { app.profile.as_mut().unwrap().unbatched_lane_map = unbatched; }
                                cx.notify();
                            }).is_err() { return; }
                            let variant = format!("{stage}_{}", if unbatched { "unbatched" } else { "batched" });
                            if !render_only(cx, &variant, measurements.as_ref()) { return; }
                            if this.update_in(cx, |app, window, _| app.profile_timeline_frame(&variant, step, window)).is_err() { return; }
                        }
                        continue;
                    }
                    if manual && !render_only(cx, stage, measurements.as_ref()) { return; }
                    if manual && args.iter().any(|arg| arg == "--profile-calendar") {
                        let _ = this.update_in(cx, |app, _, _| app.profile_calendar_frame(stage, step));
                    }
                    if manual && args.iter().any(|arg| matches!(arg.as_str(), "--profile-timeline" | "--profile-dense-timeline" | "--profile-lane-map")) {
                        let _ = this.update_in(cx, |app, window, _| app.profile_timeline_frame(stage, step, window));
                    }
                    if manual && args.iter().any(|arg| arg == "--profile-sidebar") {
                        let _ = this.update_in(cx, |app, window, _| app.profile_sidebar_frame(stage, step, window));
                    }
                    if (step == 0 || step + 1 == samples) && stage.starts_with("statistics_") {
                        let wait = Instant::now();
                        loop {
                            bg.timer(TICK).await;
                            if manual && !render_only(cx, stage, measurements.as_ref()) { return; }
                            let Ok(ready) = this.update_in(cx, |app, _, _| app.statistics_profile_ready()) else { return; };
                            if ready { break; }
                            if wait.elapsed() > Duration::from_secs(10) {
                                eprintln!("rireki: 統計計測の集計が完了しません");
                                let _ = this.update_in(cx, |_, _, cx| cx.quit());
                                return;
                            }
                        }
                    }
                    if step == 0 && stage.starts_with("detail_") {
                        let wait = Instant::now();
                        loop {
                            bg.timer(TICK).await;
                            if manual && !render_only(cx, stage, measurements.as_ref()) { return; }
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
                    if stage == "detail_analysis" && step.is_multiple_of(40) {
                        let wait = Instant::now();
                        loop {
                            bg.timer(TICK).await;
                            if manual && !render_only(cx, stage, measurements.as_ref()) { return; }
                            let Ok(ready) = this.update_in(cx, |app, _, _| app.analysis_profile_ready()) else { return; };
                            if ready { break; }
                            assert!(wait.elapsed() < Duration::from_secs(10), "イベント分析が完了しません");
                        }
                        let _ = this.update_in(cx, |app, _, _| app.analysis_profile_check());
                    }
                }
                summary(stage, "dispatch", &mut dispatch);
                summary(stage, "foreground_interval", &mut gaps);
                let _ = this.update_in(cx, |app, window, _| {
                    println!("{}", serde_json::json!({"ui_profile":"state", "stage":stage,
                        "query_set":!app.query.is_empty(), "results":app.results.as_ref().map(|r| r.total),
                        "detail_ready":app.detail_cache.is_some(), "painted_blocks":app.hits.len(), "active":window.is_window_active(), "cursor":app.result_cursor,
                        "detail":app.detail_profile_state(), "statistics":app.statistics_profile_state()}));
                });
                println!("{}", serde_json::json!({"ui_profile":"stage_end","stage":stage}));
                bg.timer(Duration::from_millis(200)).await;
            }
            println!("{}", serde_json::json!({"ui_profile":"done"}));
            let _ = this.update_in(cx, |_, _, cx| cx.quit());
        }).detach();
    }

    fn profile_step(&mut self, stage: &str, step: usize, anchor: usize, window: &mut Window, cx: &mut Context<Self>) {
        if stage.starts_with("lane_map_") {
            self.profile_lane_map_step(stage, step, anchor, window, cx);
            cx.notify();
            return;
        }
        if stage.starts_with("timeline_dense_") {
            self.profile_dense_step(stage, step, anchor, window, cx);
            cx.notify();
            return;
        }
        if stage.starts_with("sidebar_") {
            self.profile_sidebar_step(stage, step, anchor, window, cx);
            cx.notify();
            return;
        }
        if stage.starts_with("statistics_") {
            self.statistics_profile_step(stage, step, anchor, window, cx);
            cx.notify();
            return;
        }
        if step == 0 {
            window.activate_window();
            cx.activate(true);
            self.search_input.update(cx, |input, cx| input.set_text("", cx));
            self.close_detail(cx);
            self.detail_t = 0.0;
            self.mode = Mode::Timeline;
            self.gran = Granularity::Auto;
            self.in_view_only = true;
            if self.profile.as_ref().is_some_and(|p| p.timeline) {
                self.directory_timeline = false;
                self.collapsed.clear();
                self.set_source(crate::prefs::SourceFilter::All, cx);
                self.set_focus(None, cx);
            }
            let s = &self.store.sessions[anchor];
            self.t0 = s.start;
            self.spp = ((s.end - s.start) / self.plot_width(window) as f64).max(0.01);
            self.day0 = super::local(s.start).date_naive();
            self.row_scroll = 0.0;
            self.cal_scroll = 8.0 * self.px_per_hour;
            if stage.starts_with("detail_") {
                let session = if stage == "detail_long_single" {
                    self.profile_rich_session(anchor)
                } else if matches!(stage, "detail_fullscreen" | "detail_analysis") {
                    let si = self.profile_session(anchor);
                    let start = self.store.sessions[si].start;
                    let session = std::sync::Arc::make_mut(&mut self.store.sessions[si]);
                    session.lanes[0].events = (0..420).map(|i| {
                        let mut event = profile_event(i, start);
                        event.end = event.start + (i % 30 + 1) as f64;
                        if event.kind == Kind::Assistant {
                            event.usage.output = (i * 10) as u32;
                            event.model = Some("analysis-model".into());
                        }
                        event
                    }).collect();
                    session.finalize();
                    self.rebuild_order();
                    si
                } else if matches!(stage, "detail_follow" | "detail_follow_cancel" | "detail_live_scrolled" | "detail_refresh") {
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
            "detail_analysis" => self.analysis_profile_step(step, window, cx),
            "detail_fullscreen" => match step {
                20 => {
                    self.detail_list.state.scroll_to(ListOffset { item_ix:80, offset_in_item:px(17.0) });
                    self.detail_changed(cx);
                }
                40 => self.toggle_detail_fullscreen(window, cx),
                60 => {
                    assert!(self.detail_fullscreen);
                    assert_eq!(self.detail_list.state.logical_scroll_top().item_ix, 80);
                    assert!(self.detail_content_width(window) > self.detail_w);
                }
                80 => self.set_detail_tab(DetailTab::Files, cx),
                120 => self.set_detail_tab(DetailTab::Session, cx),
                160 => self.set_detail_tab(DetailTab::Events, cx),
                200 => {
                    let old = (self.detail_w, self.sidebar_w, self.label_w, self.selected);
                    self.dismiss(window, cx);
                    assert!(!self.detail_fullscreen);
                    assert_eq!((self.detail_w, self.sidebar_w, self.label_w, self.selected), old);
                }
                _ => {},
            },
            "detail_summary_selection" | "detail_summary_session" => {
                if stage == "detail_summary_session" {
                    self.set_detail_tab(DetailTab::Session, cx);
                }
                self.detail_w = 420.0 + (step % 20) as f32 * 2.0;
                self.detail_changed(cx);
            }
            "timeline_pan" => self.t0 += self.spp * 2.0,
            "timeline_zoom" => self.spp *= if step < 60 { 0.985 } else { 1.0 / 0.985 },
            "timeline_calls" => {
                self.gran = Granularity::Call;
                self.row_scroll += 2.0;
            }
            "timeline_lane_scroll" | "timeline_lane_scroll_all" | "timeline_directory_scroll" => {
                self.in_view_only = stage == "timeline_lane_scroll";
                self.directory_timeline = stage == "timeline_directory_scroll";
                self.row_scroll = (step % 60) as f32 * super::ROW_H;
            }
            "timeline_directory_pan" | "timeline_directory_zoom" => {
                self.in_view_only = false;
                self.directory_timeline = true;
                if stage == "timeline_directory_pan" {
                    self.t0 += self.spp * 2.0;
                } else {
                    self.spp *= if step < 40 { 0.99 } else { 1.0 / 0.99 };
                }
            }
            "timeline_collapse" => {
                self.in_view_only = false;
                if step.is_multiple_of(10) {
                    let cwd = self.store.sessions[anchor].cwd.clone();
                    if !self.collapsed.remove(&cwd) { self.collapsed.insert(cwd); }
                }
                self.row_scroll = (step % 10) as f32 * super::ROW_H;
            }
            "timeline_scope" => {
                self.in_view_only = false;
                if step.is_multiple_of(10) {
                    let source = match (step / 10) % 3 {
                        0 => crate::prefs::SourceFilter::All,
                        1 => crate::prefs::SourceFilter::Claude,
                        _ => crate::prefs::SourceFilter::Codex,
                    };
                    self.set_source(source, cx);
                    let focus = (step / 10).is_multiple_of(2).then(|| self.store.sessions[anchor].cwd.clone());
                    self.set_focus(focus, cx);
                }
                self.row_scroll = (step % 10) as f32 * super::ROW_H;
            }
            "timeline_update" => {
                self.in_view_only = false;
                if step == 0 {
                    self.profile_timeline_session(anchor);
                } else if step.is_multiple_of(10) {
                    let si = self.store.sessions.iter().position(|s| s.id == "rireki-timeline-profile").unwrap();
                    let session = std::sync::Arc::make_mut(&mut self.store.sessions[si]);
                    let n = session.lanes[0].events.len();
                    let start = session.start;
                    session.lanes[0].events.extend((n..n + 10).map(|i| profile_event(i, start)));
                    session.finalize();
                    self.layout_cache.borrow_mut().invalidate(&[si]);
                    self.rebuild_order();
                }
                self.row_scroll = (step % 10) as f32 * super::ROW_H;
            }
            "calendar_scroll" | "calendar_scroll_all" | "calendar_day_scroll" => {
                self.in_view_only = stage == "calendar_scroll";
                self.mode = if stage == "calendar_day_scroll" { Mode::Day } else { Mode::Week };
                self.px_per_hour = 60.0;
                self.cal_scroll = step as f32 * 3.0;
            }
            "calendar_zoom" => {
                self.in_view_only = false;
                self.mode = Mode::Week;
                self.px_per_hour = 40.0 + (step % 20) as f32 * 2.0;
                self.cal_scroll = step as f32 * 3.0;
            }
            "calendar_dates" => {
                self.in_view_only = false;
                self.mode = Mode::Week;
                self.px_per_hour = 60.0;
                self.day0 = super::local(self.store.sessions[anchor].start).date_naive()
                    - chrono::Duration::days((step % 3) as i64 * 7);
                self.cal_scroll = 180.0;
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
            "detail_refresh" => {
                if step % 40 == 10 {
                    self.set_detail_tab([DetailTab::Events, DetailTab::Files, DetailTab::Session][step / 40 % 3], cx);
                    self.file_open = self.detail_cache.as_ref().and_then(|d| d.files.first()).map(|f| f.path.clone());
                    self.expanded.insert("file-diff");
                    self.set_row_filter(Some(super::RowFilter::Name("exec_command".into())), cx);
                }
                if step % 40 == 20 {
                    self.profile_append(10, cx);
                }
                if step % 40 == 39 {
                    let sel = self.selected.unwrap();
                    assert_eq!(self.detail_cache.as_ref().unwrap().sel, sel);
                    assert!(std::ptr::eq(self.detail_session(sel), &*self.store.sessions[sel.session]));
                    assert_eq!(self.detail_cache.as_ref().unwrap().selection_summary.total.stats.tools,
                        self.store.sessions[sel.session].lanes[sel.lane].events[sel.first..=sel.last].iter()
                            .filter(|e| matches!(e.kind, Kind::Tool | Kind::Agent)).count());
                    assert!(self.file_open.is_some() && self.expanded.contains("file-diff") && self.row_filter.is_some());
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

    fn profile_lane_map_step(&mut self, stage: &str, step: usize, anchor: usize, window: &mut Window, cx: &mut Context<Self>) {
        if step == 0 {
            window.activate_window();
            cx.activate(true);
            self.search_input.update(cx, |input, cx| input.set_text("", cx));
            self.close_detail(cx);
            self.detail_t = 0.0;
            self.mode = Mode::Timeline;
            self.gran = Granularity::Auto;
            self.in_view_only = false;
            self.directory_timeline = stage.contains("directory");
            self.collapsed.clear();
            self.set_source(crate::prefs::SourceFilter::All, cx);
            self.set_focus(None, cx);
            let (start, end) = self.order.iter().map(|&si| &self.store.sessions[si])
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| (a.min(s.start), b.max(s.end)));
            assert!(start.is_finite() && end > start, "lane map profile has no history");
            self.pick = Some((start, end));
            self.set_range(start, end, window);
        }
        match stage {
            "lane_map_selection" => {
                self.selected = step.is_multiple_of(2).then_some(Selection { session: anchor, lane: 0, first: 0, last: 0 });
                // 選択に伴う詳細パネルのアニメーションで、比較する二画面の幅が変わらないようにする。
                self.detail_t = if self.selected.is_some() { 1.0 } else { 0.0 };
                let (start, end) = self.pick.unwrap();
                self.set_range(start, end, window);
            }
            "lane_map_collapse" if step.is_multiple_of(10) => {
                let cwd = self.store.sessions[anchor].cwd.clone();
                if !self.collapsed.remove(&cwd) { self.collapsed.insert(cwd); }
            }
            "lane_map_zoom" => self.spp *= if step < 40 { 0.99 } else { 1.0 / 0.99 },
            _ => {}
        }
        let rows = self.rows_in(self.t0, self.t0 + self.spp * self.plot_width(window) as f64);
        let total = rows.rows.len() as f32 * super::ROW_H;
        self.row_scroll = (total - self.plot_height(window) + super::ROW_H).max(0.0)
            * (step % 20) as f32 / 19.0;
    }

    fn profile_dense_sessions(&mut self, anchor: usize) {
        let source = self.store.sessions[anchor].source;
        let start = self.store.sessions[anchor].start;
        let names: Vec<std::sync::Arc<str>> = ["User", "read_file", "exec_command", "apply_patch", "Assistant", "spawn_agent"]
            .into_iter().map(Into::into).collect();
        for i in 0..24 {
            let mut lane = Lane::new("main");
            lane.events = (0..12000).map(|j| {
                let mut event = profile_event(j, start + i as f64 * 0.013);
                event.name = names[j % names.len()].clone();
                event.kind = if j.is_multiple_of(200) { Kind::Agent } else { event.kind };
                event.end = event.start + if j.is_multiple_of(257) { 12.0 } else { 0.1 };
                event.input.clear();
                event.output.clear();
                event
            }).collect();
            let mut session = Session {
                id: format!("rireki-dense-profile-{i}"), source, cwd: "rireki-timeline-dense".into(),
                headless: false, title: format!("Dense timeline {i}"), branch: String::new(), version: String::new(),
                lanes: vec![lane], start, end: start, parents: Vec::new(), tree: Vec::new(),
            };
            session.finalize();
            self.store.sessions.push(std::sync::Arc::new(session));
        }
        self.rebuild_order();
    }

    fn profile_dense_step(&mut self, stage: &str, step: usize, anchor: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.hovered = None;
        if step == 0 {
            window.activate_window();
            cx.activate(true);
            self.search_input.update(cx, |input, cx| input.set_text("", cx));
            self.close_detail(cx);
            self.detail_t = 0.0;
            self.sidebar_open = false;
            self.side_t = 0.0;
            self.mode = Mode::Timeline;
            self.gran = Granularity::Auto;
            self.in_view_only = false;
            self.directory_timeline = stage.contains("directory");
            self.collapsed.clear();
            self.set_source(crate::prefs::SourceFilter::All, cx);
            self.set_focus(Some("rireki-timeline-dense".into()), cx);
            self.t0 = self.store.sessions[anchor].start + 6000.0;
            self.spp = 0.03;
            self.row_scroll = 0.0;
        }
        match stage {
            "timeline_dense_pan" | "timeline_dense_directory_pan" => self.t0 += self.spp * 2.0,
            "timeline_dense_calls" => {
                self.gran = Granularity::Call;
                self.t0 += self.spp * 2.0;
            }
            "timeline_dense_turns" => {
                self.gran = Granularity::Turn;
                self.t0 += self.spp * 2.0;
            }
            "timeline_dense_same_tool" => {
                self.gran = Granularity::SameTool;
                self.spp *= if step < 40 { 0.99 } else { 1.0 / 0.99 };
            }
            "timeline_dense_update" => {
                if step.is_multiple_of(10) {
                    let si = self.store.sessions.iter().position(|s| s.id == "rireki-dense-profile-0").unwrap();
                    let start = self.store.sessions[anchor].start + 6003.0;
                    let session = std::sync::Arc::make_mut(&mut self.store.sessions[si]);
                    let mut event = profile_event(step, start);
                    event.name = "read_file".into();
                    session.lanes[0].events.push(event);
                    session.finalize();
                    self.layout_cache.borrow_mut().invalidate(&[si]);
                    self.rebuild_order();
                }
                self.spp *= if step < 40 { 0.99 } else { 1.0 / 0.99 };
            }
            _ => self.spp *= if step < 40 { 0.99 } else { 1.0 / 0.99 },
        }
    }

    fn profile_sidebar_step(&mut self, stage: &str, step: usize, anchor: usize, window: &mut Window, cx: &mut Context<Self>) {
        if step == 0 {
            window.activate_window();
            cx.activate(true);
            self.search_input.update(cx, |input, cx| input.set_text("", cx));
            self.close_detail(cx);
            self.detail_t = 0.0;
            self.mode = Mode::Timeline;
            self.sidebar_open = true;
            self.side_t = 1.0;
            self.sidebar_w = 320.0;
            self.in_view_only = true;
            self.directory_timeline = false;
            for key in ["live", "projects", "sessions"] { self.folded.remove(key); }
            self.set_source(crate::prefs::SourceFilter::All, cx);
            self.set_focus(None, cx);
            self.sidebar_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            self.navigator_list.state.scroll_to(ListOffset::default());
            let session = &self.store.sessions[anchor];
            self.t0 = session.start;
            self.spp = ((session.end - session.start) / self.plot_width(window) as f64).max(0.01);
        }
        match stage {
            "sidebar_all" => self.session_limit = usize::MAX,
            "sidebar_scroll" => {
                self.session_limit = usize::MAX;
                self.sidebar_scroll.set_offset(gpui::point(px(0.0), px(-((step % 60) as f32) * 28.0)));
                if step.is_multiple_of(60) {
                    self.navigator_list.state.scroll_to(ListOffset::default());
                } else {
                    self.navigator_list.state.scroll_by(px(28.0));
                }
            }
            "sidebar_more" if step.is_multiple_of(10) => self.session_limit += 100,
            "sidebar_fold" if step.is_multiple_of(10) => {
                let key = ["projects", "sessions", "live"][(step / 10) % 3];
                if !self.folded.remove(key) { self.folded.insert(key.into()); }
            }
            "sidebar_scope" if step.is_multiple_of(10) => {
                let source = match (step / 10) % 3 {
                    0 => crate::prefs::SourceFilter::All,
                    1 => crate::prefs::SourceFilter::Claude,
                    _ => crate::prefs::SourceFilter::Codex,
                };
                self.set_source(source, cx);
                let focus = (step / 10).is_multiple_of(2).then(|| self.store.sessions[anchor].cwd.clone());
                self.set_focus(focus, cx);
            }
            "sidebar_resize" => self.sidebar_w = 260.0 + (step % 20) as f32 * 6.0,
            "sidebar_update" => {
                if step == 0 {
                    self.profile_session(anchor);
                } else if step.is_multiple_of(10) {
                    let si = self.store.sessions.iter().position(|s| s.id == "rireki-ui-profile").unwrap();
                    let session = std::sync::Arc::make_mut(&mut self.store.sessions[si]);
                    let start = session.start;
                    let n = session.lanes[0].events.len();
                    session.lanes[0].events.extend((n..n + 10).map(|i| profile_event(i, start)));
                    session.finalize();
                    self.layout_cache.borrow_mut().invalidate(&[si]);
                    self.rebuild_order();
                }
            }
            _ => {}
        }
        self.sidebar_changed(cx);
    }

    fn profile_sidebar_frame(&self, stage: &str, step: usize, window: &Window) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        use super::sidebar::NavRow;
        let rows = self.navigator_rows();
        assert_eq!(&**self.navigator_list.rows().unwrap(), &rows, "navigator changed in {stage} step {step}");
        let eager = !self.navigator_virtual.get();
        let viewport = if eager { self.sidebar_scroll.bounds() } else { self.navigator_list.state.viewport_bounds() };
        let rem = f32::from(window.rem_size());
        let mut geometry = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let bounds = if eager {
                self.sidebar_scroll.bounds_for_item(i).map(|b| gpui::Bounds::new(b.origin + self.sidebar_scroll.offset(), b.size))
            } else {
                self.navigator_list.state.bounds_for_item(i).map(|mut b| {
                    // List の外枠に含めた余白を除き、従来のクリック対象の行を照合する。
                    let (top, bottom) = match row {
                        NavRow::Section(_, _) => (
                            rem * if i > 0 && matches!(rows[i - 1], NavRow::Section(_, _)) { 0.625 } else { 0.75 }, rem * 0.125,
                        ),
                        NavRow::More(_) => (
                            rem * if i > 0 && matches!(rows[i - 1], NavRow::Section(_, _)) { 0.125 } else { 0.25 }, 0.0,
                        ),
                        _ => (0.0, 0.0),
                    };
                    b.origin.x += px(rem * 0.5);
                    b.origin.y += px(top);
                    b.size.width -= px(rem);
                    b.size.height -= px(top + bottom);
                    b
                })
            };
            if let Some(b) = bounds.filter(|b| b.bottom() > viewport.top() && b.top() < viewport.bottom()) {
                let coords = [b.left(), b.top(), b.size.width, b.size.height]
                    .map(|p| (f32::from(p) * 1000.0).round() as i64);
                geometry.push((i, coords));
            }
        }
        let mut hash = DefaultHasher::new();
        for row in &rows {
            match row {
                NavRow::Live(si) | NavRow::Session(si) => {
                    std::mem::discriminant(row).hash(&mut hash);
                    self.store.sessions[*si].id.hash(&mut hash);
                }
                NavRow::Project(i) => {
                    std::mem::discriminant(row).hash(&mut hash);
                    let p = &self.projects[*i];
                    (&p.cwd, &p.name, p.sessions).hash(&mut hash);
                }
                _ => row.hash(&mut hash),
            }
        }
        println!("{}", serde_json::json!({"ui_profile":"sidebar_frame", "stage":stage, "step":step,
            "rows":rows.len(), "row_hash":format!("{:016x}",hash.finish()),
            "built":self.navigator_rendered.get(), "limit":self.session_limit,
            "live":rows.iter().filter(|r| matches!(r, NavRow::Live(_))).count(), "geometry":geometry}));
    }

    fn profile_calendar_frame(&self, stage: &str, step: usize) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hash = DefaultHasher::new();
        for &(rect, sel) in &self.hits {
            [rect.0.to_bits(), rect.1.to_bits(), rect.2.to_bits(), rect.3.to_bits()].hash(&mut hash);
            self.store.sessions[sel.session].id.hash(&mut hash);
            (sel.lane, sel.first, sel.last).hash(&mut hash);
        }
        println!("{}", serde_json::json!({"ui_profile":"calendar_frame", "stage":stage,
            "step":step, "blocks":self.hits.len(), "counts":self.counts,
            "hit_hash":format!("{:016x}", hash.finish())}));
    }

    fn profile_timeline_frame(&self, stage: &str, step: usize, window: &Window) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        if self.profile.as_ref().is_some_and(|p| p.dense_timeline) {
            assert!(self.hits.len() >= 10, "dense timeline has no visible workload in {stage} step {step}");
        }
        let v1 = self.t0 + self.spp * self.plot_width(window) as f64;
        let actual = self.rows_in(self.t0, v1);
        let reference = self.build_rows_in(self.t0, v1);
        let rows_hash = self.profile_rows_hash(&actual.rows);
        assert_eq!(rows_hash, self.profile_rows_hash(&reference), "timeline rows changed in {stage} step {step}");
        let mut hash = DefaultHasher::new();
        for &(rect, sel) in &self.hits {
            [rect.0.to_bits(), rect.1.to_bits(), rect.2.to_bits(), rect.3.to_bits()].hash(&mut hash);
            self.store.sessions[sel.session].id.hash(&mut hash);
            (sel.lane, sel.first, sel.last).hash(&mut hash);
        }
        let mut frame = serde_json::json!({"ui_profile":"timeline_frame", "stage":stage,"step":step,
            "rows":actual.rows.len(), "row_hash":format!("{rows_hash:016x}"),
            "blocks":self.hits.len(), "counts":self.counts,"hit_hash":format!("{:016x}",hash.finish())});
        if let Some(measurements) = self.profile.as_ref().and_then(|p| p.measurements.as_ref()) {
            frame["paint_hash"] = format!("{:016x}", measurements.get().paint_hash).into();
            frame["minimap_hash"] = format!("{:016x}", measurements.get().minimap_hash).into();
            frame["minimap_quads"] = measurements.get().minimap_quads.into();
            frame["lane_map_hash"] = format!("{:016x}", measurements.get().lane_map_hash).into();
            frame["lane_map_quads"] = measurements.get().lane_map_quads.into();
        }
        println!("{frame}");
    }

    fn profile_rows_hash(&self, rows: &[super::Row]) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hash = DefaultHasher::new();
        rows.len().hash(&mut hash);
        for row in rows {
            match row {
                super::Row::Header { cwd, sessions, collapsed, active, .. } => {
                    (0u8, cwd, collapsed, active).hash(&mut hash);
                    sessions.len().hash(&mut hash);
                    for &si in sessions { self.store.sessions[si].id.hash(&mut hash); }
                }
                super::Row::Lane(track) => {
                    (1u8, &self.store.sessions[track.session].id, track.lane, track.depth, track.active).hash(&mut hash);
                    track.link.map(|(si, li, time)| (&self.store.sessions[si].id, li, time.to_bits())).hash(&mut hash);
                }
                super::Row::Packed(_) => 2u8.hash(&mut hash),
            }
            row.packed_blocks().len().hash(&mut hash);
            for block in row.packed_blocks() {
                (&self.store.sessions[block.session].id, block.block.start.to_bits(), block.block.end.to_bits(),
                    block.block.first, block.block.last, block.x.to_bits(), block.width.to_bits()).hash(&mut hash);
            }
        }
        hash.finish()
    }

    fn profile_timeline_session(&mut self, anchor: usize) {
        let source = &self.store.sessions[anchor];
        let start = source.start;
        let mut lane = Lane::new("main");
        lane.origin = "rireki-timeline-profile".into();
        lane.events = (0..60).map(|i| profile_event(i, start)).collect();
        let mut session = Session {
            id: "rireki-timeline-profile".into(), source: source.source, cwd: source.cwd.clone(),
            headless: false, title: "Timeline profile".into(), branch: String::new(), version: String::new(),
            lanes: vec![lane], start, end: start, parents: Vec::new(), tree: Vec::new(),
        };
        session.finalize();
        self.store.sessions.push(std::sync::Arc::new(session));
        self.rebuild_order();
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
        let shown = self.shown_detail_selection().unwrap();
        let generation = self.shown_detail_generation();
        let offset = self.detail_list.state.logical_scroll_top();
        let shown_snapshot = self.detail_snapshot.as_ref().unwrap().0.clone();
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
        // 背景処理が完了する前にも、旧版の行・集計・位置をそのまま表示できる。
        assert_eq!(self.shown_detail_selection(), Some(shown));
        assert_eq!(self.shown_detail_generation(), generation);
        assert!(std::ptr::eq(self.detail_session(shown), &*shown_snapshot));
        let next = self.detail_list.state.logical_scroll_top();
        assert_eq!(next.item_ix, offset.item_ix);
        assert_eq!(next.offset_in_item, offset.offset_in_item);
    }
}
