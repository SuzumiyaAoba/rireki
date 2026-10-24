//! イベント分析タブ。集計は背景で行い、一覧は可視行だけを描く。

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use gpui::{
    ClickEvent, ClipboardItem, Context, Div, Focusable, FontWeight, ListAlignment, ListState,
    Window, div, list, prelude::*, px,
};

use super::{badge, cache_hit_stat, fmt_tokens, label, stat, usage_view};
use crate::app::widgets::{event_icon, tip};
use crate::app::{Rireki, fmt_dur, fmt_n, local};
use crate::detail::{
    Selection,
    analysis::{Analysis, Filter, KindFilter, Order, StatusFilter, duration, status_name},
};
use crate::icons::{self, Icon};
use crate::input::{InputEvent, TextInput};
use crate::prefs::DetailTab;
use crate::theme::{self, c};

#[derive(Clone, PartialEq)]
struct Key {
    selection: Selection,
    generation: u64,
    filter: Filter,
    order: Order,
}

pub(in crate::app) struct State {
    input: gpui::Entity<TextInput>,
    filter: Filter,
    order: Order,
    key: Option<Key>,
    result: Option<(Key, Arc<Analysis>)>,
    generation: Arc<AtomicU64>,
    task: Option<gpui::Task<()>>,
    export_task: Option<gpui::Task<()>>,
    exporting: bool,
    busy: bool,
    list: ListState,
}

impl State {
    pub(in crate::app) fn new(cx: &mut Context<Rireki>) -> Self {
        let input = cx.new(|cx| TextInput::new("名前・要約・モデルを検索", cx));
        cx.subscribe(&input, |app, input, _: &InputEvent, cx| {
            app.analysis_state.filter.query = input.read(cx).text().trim().to_string();
            app.request_analysis(cx);
        })
        .detach();
        Self {
            input,
            filter: Filter::default(),
            order: Order::default(),
            key: None,
            result: None,
            generation: Arc::new(AtomicU64::new(0)),
            task: None,
            export_task: None,
            exporting: false,
            busy: false,
            list: ListState::new(0, ListAlignment::Top, px(200.0)),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl Rireki {
    pub(in crate::app) fn analysis_profile_step(
        &mut self,
        step: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match step {
            0 => {
                self.analysis_state.filter = Filter::default();
                self.analysis_state.order = Order::Oldest;
                self.analysis_state
                    .input
                    .update(cx, |i, cx| i.set_text("", cx));
                self.set_detail_tab(DetailTab::Analysis, cx);
            }
            40 => self
                .analysis_state
                .input
                .update(cx, |i, cx| i.set_text("event 3", cx)),
            80 => self.analysis_state.filter.status = StatusFilter::Failed,
            120 => self.analysis_state.order = Order::Tokens,
            160 => {
                self.analysis_state.filter.kind = KindFilter::Tools;
                self.analysis_state
                    .input
                    .update(cx, |i, cx| i.set_text("", cx));
            }
            200 => self
                .analysis_state
                .input
                .update(cx, |i, cx| i.set_text("__no_analysis_matches__", cx)),
            240 => {
                self.analysis_state.filter = Filter::default();
                self.analysis_state
                    .input
                    .update(cx, |i, cx| i.set_text("", cx));
            }
            260 => {
                let sel = self.shown_detail_selection().unwrap();
                assert!(
                    self.analysis_state
                        .result
                        .as_ref()
                        .unwrap()
                        .1
                        .indices
                        .contains(&350)
                );
                self.open_analysis_event(sel, 350, window, cx);
                assert_eq!(self.detail_tab, DetailTab::Events);
            }
            279 => {
                let sel = self.shown_detail_selection().unwrap();
                assert_eq!((sel.first, sel.last), (350, 350));
                let cache = self
                    .cache(sel)
                    .expect("分析から開いたイベントの読み込みが完了する");
                assert_eq!(cache.texts.len(), 1);
                assert!(!cache.texts[0].0.is_empty());
                println!(
                    "{}",
                    serde_json::json!({"ui_profile":"analysis_open_check", "event":351, "has_input":true})
                );
            }
            _ => return,
        }
        self.request_analysis(cx);
    }

    pub(in crate::app) fn analysis_profile_ready(&self) -> bool {
        !self.analysis_state.busy
            && self.analysis_state.result.as_ref().is_some_and(|(key, _)| {
                Some(key.selection) == self.shown_detail_selection()
                    && key.generation == self.shown_selection_generation()
                    && key.filter == self.analysis_state.filter
                    && key.order == self.analysis_state.order
            })
    }

    pub(in crate::app) fn analysis_profile_check(&self) {
        let (key, actual) = self.analysis_state.result.as_ref().unwrap();
        let sel = key.selection;
        let events = &self.detail_session(sel).lanes[sel.lane].events[sel.first..=sel.last];
        let expected_count = match (
            key.filter.query.as_str(),
            key.filter.status,
            key.filter.kind,
        ) {
            ("", StatusFilter::All, KindFilter::All) => 420,
            ("event 3", StatusFilter::All, KindFilter::All) => 159,
            ("event 3", StatusFilter::Failed, KindFilter::All) => 23,
            ("", StatusFilter::Failed, KindFilter::Tools) => 36,
            ("__no_analysis_matches__", _, _) => 0,
            _ => panic!("未定義の分析検証条件"),
        };
        assert_eq!(actual.indices.len(), expected_count);
        if key.order == Order::Tokens {
            assert!(
                actual
                    .indices
                    .windows(2)
                    .all(|pair| events[pair[0]].usage.total() >= events[pair[1]].usage.total())
            );
        }
        assert_eq!(
            self.analysis_state.list.item_count(),
            actual.indices.len() + 1
        );
        if key.filter == Filter::default() {
            assert_eq!(actual.indices.len(), 420);
            assert_eq!(actual.usage.total(), 175140);
        }
        if key.filter.query == "__no_analysis_matches__" {
            assert!(actual.indices.is_empty());
        }
        println!(
            "{}",
            serde_json::json!({"ui_profile":"analysis_check", "count":actual.indices.len(),
            "order":format!("{:?}", key.order), "status":format!("{:?}", key.filter.status),
            "query":key.filter.query, "list_count":self.analysis_state.list.item_count()})
        );
    }

    pub(in crate::app) fn reset_analysis(&mut self) {
        let state = &mut self.analysis_state;
        state.generation.fetch_add(1, Ordering::Relaxed);
        state.task = None;
        state.export_task = None;
        state.exporting = false;
        state.key = None;
        state.result = None;
        state.busy = false;
        state.list.reset(0);
    }

    pub(in crate::app) fn request_analysis(&mut self, cx: &mut Context<Self>) {
        if self.detail_tab != DetailTab::Analysis {
            return;
        }
        let Some(sel) = self
            .shown_detail_selection()
            .filter(|&sel| self.cache(sel).is_some())
        else {
            return;
        };
        let key = Key {
            selection: sel,
            generation: self.shown_selection_generation(),
            filter: self.analysis_state.filter.clone(),
            order: self.analysis_state.order,
        };
        if self.analysis_state.key.as_ref() == Some(&key) {
            return;
        }
        let session = self
            .detail_snapshot
            .as_ref()
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| self.store.sessions[sel.session].clone());
        let state = &mut self.analysis_state;
        if state
            .result
            .as_ref()
            .is_some_and(|(old, _)| old.selection != sel || old.generation != key.generation)
        {
            state.result = None;
            state.list.reset(0);
        }
        state.key = Some(key.clone());
        state.export_task = None;
        state.exporting = false;
        state.busy = true;
        let generation = state.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let counter = state.generation.clone();
        state.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    Analysis::build(
                        &session.lanes[sel.lane].events[sel.first..=sel.last],
                        &key.filter,
                        key.order,
                        || counter.load(Ordering::Relaxed) == generation,
                    )
                    .map(|result| (key, Arc::new(result)))
                })
                .await;
            let Some((key, result)) = loaded else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                let state = &mut app.analysis_state;
                if state.generation.load(Ordering::Relaxed) != generation {
                    return;
                }
                state.list.reset(result.indices.len() + 1);
                state.result = Some((key, result));
                state.busy = false;
                app.detail_changed(cx);
            });
        }));
        self.detail_changed(cx);
    }

    pub(in crate::app) fn focus_analysis_search(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.analysis_state.list.scroll_to(gpui::ListOffset::default());
        window.focus(&self.analysis_state.input.focus_handle(cx));
        self.analysis_state
            .input
            .update(cx, |input, cx| input.select_all_text(cx));
        self.detail_changed(cx);
    }

    pub(in crate::app) fn cancel_analysis_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.analysis_state.input.read(cx).text().is_empty() {
            window.focus(&self.focus_handle);
        } else {
            self.analysis_state
                .input
                .update(cx, |input, cx| input.set_text("", cx));
        }
        self.detail_changed(cx);
    }

    pub(super) fn analysis_view(&self, sel: Selection, cx: &mut Context<Self>) -> gpui::AnyElement {
        let state = &self.analysis_state;
        let mut body = div()
            .id("event-analysis")
            .key_context("EventAnalysis")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_3();
        if state.result.is_some() {
            let app = cx.entity().downgrade();
            body = body.child(
                list(state.list.clone(), move |row, _, cx| {
                    app.update(cx, |app, cx| app.analysis_item(sel, row, cx))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .w_full()
                .min_w_0()
                .flex_1()
                .min_h_0()
                .when(state.busy, |d| d.opacity(0.5)),
            );
        } else {
            body = body.child(div().text_xs().text_color(c(theme::MUTED)).child("集計中…"));
        }
        body.into_any_element()
    }

    /// 検索と並べ替えは指標・内訳の後、比較一覧の直前に置く。
    fn analysis_controls(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let state = &self.analysis_state;
        let mut body = div().flex().flex_col().gap_2()
            .child(div().flex().items_center().gap_2().child(label("イベントの比較"))
                .child(div().flex_1()).child(self.ghost_button("analysis-export",
                    if self.copied == Some("analysis-export") { Icon::Check } else { Icon::Copy },
                    "絞り込んだ全イベントのメタデータを JSON でコピー", None,
                    state.result.is_some() && !state.busy && !state.exporting, cx,
                    |app, _, cx| app.copy_analysis(cx))));
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(32.0))
                .flex_none()
                .rounded_md()
                .bg(c(theme::BG))
                .border_1()
                .border_color(c(theme::BORDER))
                .child(icons::icon(Icon::Search, theme::MUTED).size_3p5())
                .child(div().flex_1().min_w_0().child(state.input.clone()))
                .child(self.ghost_button(
                    "analysis-search-focus",
                    Icon::TextSearch,
                    "分析内を検索",
                    Some(crate::platform::shortcut("⌘F", "Ctrl+F")),
                    true,
                    cx,
                    |app, window, cx| app.focus_analysis_search(window, cx),
                )),
        );
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(self.segmented(
                    "analysis-kind",
                    &[
                        (KindFilter::All, "すべて"),
                        (KindFilter::Tools, "ツール"),
                        (KindFilter::Messages, "発話・思考"),
                    ],
                    Some(state.filter.kind),
                    cx,
                    |app, kind, _, cx| {
                        app.analysis_state.filter.kind = kind;
                        app.analysis_state.filter.name = None;
                        app.request_analysis(cx);
                    },
                ))
                .child(self.segmented(
                    "analysis-status",
                    &[
                        (StatusFilter::All, "全状態"),
                        (StatusFilter::Failed, "失敗・拒否・中断"),
                        (StatusFilter::Pending, "結果なし"),
                    ],
                    Some(state.filter.status),
                    cx,
                    |app, status, _, cx| {
                        app.analysis_state.filter.status = status;
                        app.request_analysis(cx);
                    },
                )),
        );
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(self.segmented(
                    "analysis-order",
                    &[
                        (Order::Oldest, "古い順"),
                        (Order::Newest, "新しい順"),
                        (Order::Duration, "時間順"),
                        (Order::Tokens, "トークン順"),
                    ],
                    Some(state.order),
                    cx,
                    |app, order, _, cx| {
                        app.analysis_state.order = order;
                        app.request_analysis(cx);
                    },
                ))
                .when(state.filter != Filter::default(), |d| {
                    d.child(self.icon_button(
                        "analysis-clear",
                        Some(Icon::X),
                        "絞り込み解除",
                        false,
                        cx,
                        |app, _, cx| {
                            app.analysis_state.filter = Filter::default();
                            app.analysis_state
                                .input
                                .update(cx, |input, cx| input.set_text("", cx));
                            app.request_analysis(cx);
                        },
                    ))
                }),
        );
        if let Some(name) = &state.filter.name {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(badge(name.clone(), theme::ACCENT))
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child("のイベントに絞り込み中"),
                    ),
            );
        }
        body = body.child(
            div()
                .flex_none()
                .text_xs()
                .text_color(c(theme::MUTED))
                .child(if state.busy {
                    "集計中…".into()
                } else if let Some((_, result)) = &state.result {
                    format!(
                        "{} / {} 件 · 名前・要約・モデルが検索対象",
                        fmt_n(result.indices.len()),
                        fmt_n(sel.last - sel.first + 1)
                    )
                } else {
                    "詳細を読み込み中…".into()
                }),
        );
        body
    }

    fn copy_analysis(&mut self, cx: &mut Context<Self>) {
        let Some((key, analysis)) = &self.analysis_state.result else {
            return;
        };
        if self.analysis_state.busy || self.analysis_state.exporting {
            return;
        }
        let sel = key.selection;
        let analysis = analysis.clone();
        let session = self
            .detail_snapshot
            .as_ref()
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| self.store.sessions[sel.session].clone());
        self.analysis_state.exporting = true;
        self.analysis_state.export_task = Some(cx.spawn(async move |this, cx| {
            let json = cx
                .background_executor()
                .spawn(async move {
                    let lane = &session.lanes[sel.lane];
                    analysis.export_json(
                        &lane.events[sel.first..=sel.last],
                        sel.first,
                        &session.id,
                        &lane.name,
                    )
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.analysis_state.exporting = false;
                cx.write_to_clipboard(ClipboardItem::new_string(json));
                app.flash_copied("analysis-export", cx);
            });
        }));
        self.detail_changed(cx);
    }

    fn analysis_item(
        &self,
        sel: Selection,
        row: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some((key, analysis)) = &self.analysis_state.result else {
            return div().into_any_element();
        };
        if key.selection != sel || !self.valid(sel) {
            return div().into_any_element();
        }
        if row == 0 {
            return div().w_full().min_w_0().flex().flex_col().gap_2().pb_2()
                .child(self.analysis_summary(analysis, cx)).child(self.analysis_controls(sel, cx)).into_any_element();
        }
        let Some(&index) = analysis.indices.get(row - 1) else {
            return div().into_any_element();
        };
        let i = sel.first + index;
        let e = &self.detail_session(sel).lanes[sel.lane].events[i];
        let color = theme::event_color(e);
        let status_color = if e.status.failed() {
            theme::ERR
        } else {
            theme::MUTED
        };
        let status = if crate::detail::analysis::is_tool(e) {
            status_name(e.status)
        } else {
            "発話・思考"
        };
        div()
            .id(("analysis-event", i))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_xs()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(event_icon(theme::event_icon(e), color, 12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(e.name.to_string()),
                    )
                    .child(div().text_color(c(status_color)).child(status))
                    .child(
                        div()
                            .text_color(c(theme::MUTED))
                            .child(fmt_dur(duration(e))),
                    ),
            )
            .child(
                div()
                    .truncate()
                    .text_color(c(theme::MUTED))
                    .child(e.summary.to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(c(theme::SUBTLE))
                    .child(format!(
                        "#{} · {}",
                        fmt_n(i + 1),
                        local(e.start).format("%m/%d %H:%M:%S")
                    ))
                    .child(div().flex_1())
                    .when(e.usage.total() > 0, |d| {
                        d.child(format!("{} tokens", fmt_tokens(e.usage.total())))
                    }),
            )
            .when(!self.analysis_state.busy, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(c(theme::HOVER)))
                    .on_click(cx.listener(move |app, _: &ClickEvent, window, cx| {
                        app.open_analysis_event(sel, index, window, cx);
                    }))
            })
            .into_any_element()
    }

    fn open_analysis_event(
        &mut self,
        sel: Selection,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let i = sel.first + index;
        let event = Selection {
            first: i,
            last: i,
            ..sel
        };
        if !self.valid(event) {
            return;
        }
        self.navigate(event, window, cx);
        self.set_detail_tab(DetailTab::Events, cx);
        window.focus(&self.focus_handle);
    }

    fn analysis_summary(&self, analysis: &Analysis, cx: &mut Context<Self>) -> Div {
        let has_tools = analysis.completed_tools > 0;
        let rate = analysis
            .failure_rate()
            .map_or("—".into(), |r| format!("{:.1}%", r * 100.0));
        let tool_time = |seconds| {
            if has_tools {
                fmt_dur(seconds)
            } else {
                "—".into()
            }
        };
        let mut out =
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_3()
                .pb_3()
                .child(label(if self.analysis_state.filter == Filter::default() {
                    "選択範囲の集計"
                } else { "絞り込んだイベントの集計" }))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(stat("選択期間", fmt_dur(analysis.span), None))
                        .child(stat("イベント", fmt_n(analysis.indices.len()), None))
                        .child(stat("失敗・拒否・中断", fmt_n(analysis.failures),
                            (analysis.failures > 0).then_some(theme::ERR)))
                        .child(stat("API換算料金", super::usage_cost_label(&analysis.usage), None))
                        .child(cache_hit_stat("analysis-cache-hit", &analysis.usage))
                        .child(
                            stat(
                                "ツール失敗率",
                                rate,
                                (analysis.failed_tools > 0).then_some(theme::ERR),
                            )
                            .id("analysis-rate")
                            .tooltip(tip(
                                "失敗・拒否・中断 / 結果のあるツール呼び出し（結果なしは除外）",
                                None,
                            )),
                        )
                        .child(stat("トークン", super::usage_total_label(&analysis.usage), None)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(stat("平均ツール時間", tool_time(analysis.average()), None))
                        .child(
                            stat("P95 ツール時間", tool_time(analysis.p95_seconds), None)
                                .id("analysis-p95")
                                .tooltip(tip("完了したツールの95%がこの時間以下です", None)),
                        )
                        .child(stat(
                            "最長ツール時間",
                            tool_time(analysis.max_seconds),
                            None,
                        )),
                )
                .child(div().text_xs().text_color(c(theme::MUTED)).child(format!(
                    "範囲 {} · ツール合計 {} · 失敗等 {} 件 · 結果なし {} 件",
                    fmt_dur(analysis.span),
                    fmt_dur(analysis.tool_seconds),
                    fmt_n(analysis.failures),
                    fmt_n(analysis.pending)
                )))
                .child(div().text_xs().text_color(c(theme::SUBTLE)).child(
                    "時間集計は結果のあるツールが対象です。合計は同時実行の重なりを含みます。",
                ));
        if !analysis.tools.is_empty() {
            let peak = analysis.tools.first().map_or(0.0, |tool| tool.seconds);
            let mut tools = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(label("ツール別の時間 · 上位8件（クリックで絞り込み）"));
            for tool in analysis.tools.iter().take(8) {
                let name = tool.name.clone();
                let on = self.analysis_state.filter.name.as_ref() == Some(&name);
                let share = if peak > 0.0 {
                    (tool.seconds / peak) as f32
                } else {
                    0.0
                };
                tools = tools.child(
                    div()
                        .id(gpui::SharedString::from(format!("analysis-tool-{name}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .when(on, |d| d.bg(c(theme::ACCENT_BG)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(name.clone()),
                                )
                                .child(format!("{}回", fmt_n(tool.calls)))
                                .when(tool.failures > 0, |d| {
                                    d.child(
                                        div()
                                            .text_color(c(theme::ERR))
                                            .child(format!("失敗等{}", tool.failures)),
                                    )
                                })
                                .child(fmt_dur(tool.seconds)),
                        )
                        .child(
                            div()
                                .mt_1()
                                .h(px(3.0))
                                .w_full()
                                .rounded_full()
                                .bg(c(theme::GRID))
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(share))
                                        .rounded_full()
                                        .bg(c(theme::ACCENT)),
                                ),
                        )
                        .tooltip(tip(
                            format!(
                                "平均 {} · 最長 {} · 結果なし {}回",
                                fmt_dur(tool.average()),
                                fmt_dur(tool.max_seconds),
                                fmt_n(tool.pending)
                            ),
                            None,
                        ))
                        .when(!self.analysis_state.busy, |d| {
                            d.cursor_pointer()
                                .hover(|s| s.bg(c(theme::HOVER)))
                                .on_click(cx.listener(move |app, _: &ClickEvent, _, cx| {
                                    app.analysis_state.filter.name =
                                        if on { None } else { Some(name.clone()) };
                                    app.request_analysis(cx);
                                }))
                        }),
                );
            }
            out = out.child(tools);
        }
        if analysis.usage.calls > 0 {
            out = out.child(div().flex().flex_col().gap_1().text_xs()
                .child(label("モデル別のトークン・API換算料金"))
                .child(super::model_table(&analysis.models)))
                .child(self.icon_button("analysis-usage-toggle", Some(Icon::ChevronDown),
                    if self.expanded.contains("analysis-usage") { "キャッシュ内訳を閉じる" } else { "キャッシュ内訳を表示" },
                    false, cx, |app, _, cx| {
                        if !app.expanded.remove("analysis-usage") { app.expanded.insert("analysis-usage"); }
                        let offset = app.analysis_state.list.logical_scroll_top();
                        let count = app.analysis_state.list.item_count();
                        app.analysis_state.list.splice(0..count, count);
                        app.analysis_state.list.scroll_to(offset);
                        app.detail_changed(cx);
                    }))
                .when(self.expanded.contains("analysis-usage"), |d| d.child(usage_view(&analysis.usage))
                    .child(stat("最大コンテキスト", fmt_tokens(analysis.peak_context), None)))
                .child(div().text_xs().text_color(c(theme::SUBTLE)).child("トークン・料金は記録された使用量を集計しています。"));
        }
        if analysis.indices.is_empty() {
            out = out.child(
                div()
                    .py_3()
                    .text_color(c(theme::MUTED))
                    .child("条件に一致するイベントはありません"),
            );
        }
        out
    }
}
