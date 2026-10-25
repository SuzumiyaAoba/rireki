//! 右サイドパネル。1 / 2 / 3 / 4 で「概要」「履歴」「変更」「分析」を切り替える。
//!
//! 概要はセッション全体の指示・応答・エージェント構成を示す。
//! 履歴・変更・分析は選択範囲を対象にし、対象範囲を共通上部に明示する。
//! 履歴では指示と応答を読み、ツール実行と思考を必要なときに展開する。
//! 概要、履歴、変更、分析の一覧は可視行だけを描き、タブ切り替えで位置を保持する。

use std::cell::{Cell, RefCell};

use gpui::{
    AnyElement, ClickEvent, Context, Div, ElementId, FontWeight, ListAlignment, ListOffset,
    ListState, SharedString, Stateful, WeakEntity, Window, div, list, prelude::*, px,
};

use super::rich;
use super::widgets::{Marquee, dot, event_icon, project_tile, tip};
use super::{DETAIL_MAX, HEADER_H, INNER_RADIUS, Rireki, Selection, StyleRefinement, fmt_dur, fmt_n, is_live, local, tilde};
use crate::icons::{self, Icon};
use crate::model::{Event, Kind, Session, Source, Status, Usage, UsageSum, one_line};

mod controller;
mod list;
mod loading;
mod invalidation;
mod profile;
mod event;
mod file_render;
mod history;
mod usage;
use list::*;
use usage::*;
pub(super) use list::DetailList;
pub(super) use usage::{fmt_tokens, fmt_usd, usage_view};
mod files;
pub(super) mod analysis;
pub(super) mod replay;
pub(super) mod overview;
use crate::detail::{DetailCache, FileStat};
pub(super) use files::FileList;
use files::{FILE_MAX, FileListKey, FileRow, PER_EDIT};
use crate::prefs::DetailTab;
use crate::theme::{self, c};

/// 詳細パネルの枠と見出し。本文は独立したビューとして再利用する。
pub struct DetailView {
    pub(super) app: WeakEntity<Rireki>,
}

pub(super) struct DetailContentView {
    pub(super) app: WeakEntity<Rireki>,
}

pub(super) struct AnalysisContentView {
    pub(super) app: WeakEntity<Rireki>,
}

impl Render for DetailContentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.detail_content(window, cx)),
            None => div().into_any_element(),
        };
        div().size_full().flex().flex_col().child(content)
    }
}

impl Render for AnalysisContentView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| {
                let Some(sel) = app.shown_detail_selection() else {
                    return div().into_any_element();
                };
                app.analysis_view(sel, cx)
            }),
            None => div().into_any_element(),
        }
    }
}

impl Render for DetailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.detail(window, cx)),
            None => div().into_any_element(),
        }
    }
}

/// 詳細パネルのイベント一覧の絞り込み
#[derive(Clone, PartialEq, Debug)]
pub(super) enum RowFilter {
    /// ツール名（または User / Assistant など）
    Name(String),
    /// 失敗・拒否・中断
    Failed,
}

/// 結果を Markdown として表示するツール（サブエージェントの報告や取得したページ）
const MARKDOWN_OUTPUT: &[&str] = &["Agent", "Task", "WebFetch", "ExitPlanMode", "Skill"];
/// 長い本文を最初に出す行数（差分は行数で数える）
const FOLD_LINES: usize = 60;
const FOLD_DIFF: usize = 160;

/// セッションを再開するコマンド（作業ディレクトリに移ってから再開する）
fn resume_command(s: &Session) -> String {
    let cwd = format!("'{}'", s.cwd.replace('\'', "'\\''"));
    match s.source {
        Source::Claude => format!("cd {cwd} && claude --resume {}", s.id),
        Source::Codex => format!("cd {cwd} && codex resume {}", s.id),
    }
}

/// 結果の先頭に出る終了コード（Claude の `Exit code 1`、Codex の `[exit 1 · 0.2s]`）
fn exit_code(output: &str) -> Option<&str> {
    let first = output.lines().next()?;
    let rest = first.strip_prefix("Exit code ").or_else(|| first.strip_prefix("[exit "))?;
    let n = rest.bytes().take_while(|b| b.is_ascii_digit() || *b == b'-').count();
    (n > 0).then(|| &rest[..n])
}

/// 結果の状態の表示 (文言, 色)。成功なら None
fn status_label(e: &Event, live: bool, output: &str) -> Option<(String, u32)> {
    match e.status {
        Status::Ok => None,
        Status::Error => Some((exit_code(output).map_or("失敗".into(), |c| format!("失敗 · exit {c}")), theme::ERR)),
        Status::Rejected => Some(("拒否".into(), theme::WARN)),
        Status::Interrupted => Some(("中断".into(), theme::WARN)),
        Status::Pending if live => Some(("実行中".into(), theme::LIVE)),
        Status::Pending => Some(("結果なし".into(), theme::SUBTLE)),
    }
}

fn badge(label: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .bg(c(color).opacity(0.15))
        .text_xs()
        .text_color(c(color))
        .whitespace_nowrap()
        .child(label.into())
}

/// 小見出し（「入力」「結果」など）
fn label(text: impl Into<SharedString>) -> Div {
    div().flex_none().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::MUTED)).child(text.into())
}

/// 集計の 1 項目。並べたときに行の幅を等分する
fn stat(name: &'static str, value: String, color: Option<u32>) -> Div {
    div()
        .flex_1()
        .min_w(px(88.0))
        .flex()
        .flex_col()
        .gap_0p5()
        .whitespace_nowrap()
        .px_2p5()
        .py_2()
        .rounded_lg()
        .bg(c(theme::BG))
        .border_1()
        .border_color(c(theme::BORDER))
        .child(
            div()
                .text_lg()
                .line_height(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(color.unwrap_or(theme::TEXT)))
                .child(value),
        )
        .child(div().truncate().text_xs().text_color(c(theme::SUBTLE)).child(name))
}

fn detail_tab_index(tab: DetailTab) -> usize {
    match tab { DetailTab::Session => 0, DetailTab::Events => 1, DetailTab::Files => 2, DetailTab::Analysis => 3 }
}

impl Rireki {
    pub(super) fn detail(&self, _: &Window, cx: &mut Context<Self>) -> AnyElement {
        let panel = div().id("detail-panel").size_full().flex().flex_col()
            .when(self.detail_fullscreen, |d| d.rounded(px(INNER_RADIUS)))
            .when(!self.detail_fullscreen, |d| d.rounded_r(px(INNER_RADIUS)).border_l_1())
            .bg(c(theme::PANEL)).border_color(c(theme::BORDER)).text_sm().text_color(c(theme::TEXT))
            .on_scroll_wheel(cx.listener(|app, _: &gpui::ScrollWheelEvent, _, _| app.cancel_detail_follow()));
        let Some(sel) = self.shown_detail_selection() else { return panel.into_any_element(); };
        let files = self.cache(sel).map_or(0, |d| d.files.len());
        let panel = panel.child(self.detail_header(sel, cx)).child(self.context_bar(sel, cx))
            .child(self.detail_tabs(sel, self.detail_tab, files, cx)).child(self.detail_scope(sel, cx))
            .when(self.detail_tab == DetailTab::Events && self.cache(sel).is_some(), |d| {
                d.child(div().flex_none().px_3().pt_3().flex().flex_col().gap_2()
                    .child(self.history_heading(sel, cx))
                    .when(sel.first == sel.last, |d| d.child(self.replay_controls(sel, cx)))
                    .when(sel.first != sel.last && self.replay.is_some(), |d| d.child(self.event_list_header(sel, cx))))
            });
        // 親を cached にすると、見出しの更新時に GPUI が子も強制描画してしまう。
        // 本文だけを独立してキャッシュし、件数などの更新では再利用する。
        panel.child(gpui::AnyView::from(self.detail_content_view.clone()).cached(
            StyleRefinement::default().w_full().flex_1().min_h_0(),
        )).into_any_element()
    }

    fn detail_content(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(profile) = &self.profile {
            profile.detail_renders.set(profile.detail_renders.get() + 1);
        }
        let Some(sel) = self.shown_detail_selection() else { return div().into_any_element(); };
        let files = self.cache(sel).map_or(0, |d| d.files.len());
        if self.cache(sel).is_none() {
            return div().p_3().text_xs().text_color(c(theme::MUTED))
                .child("詳細を読み込み中…").into_any_element();
        }
        self.detail_live.set(Some(is_live(self.detail_session(sel).end)));
        match self.detail_tab {
            DetailTab::Session => self.overview_view(sel, window, cx),
            DetailTab::Analysis => gpui::AnyView::from(self.analysis_content_view.clone())
                .cached(StyleRefinement::default().w_full().flex_1().min_h_0())
                .into_any_element(),
            DetailTab::Files if files == 0 => div().p_3().text_xs().text_color(c(theme::MUTED))
                .child("選択範囲に記録されたファイル編集はありません").into_any_element(),
            DetailTab::Files => div().flex_1().min_h_0().p_3().flex().flex_col()
                .child(self.files_list(sel, self.cache(sel).unwrap(), window, cx)).into_any_element(),
            DetailTab::Events if sel.first == sel.last => div().id("detail-scroll")
                .track_scroll(&self.detail_scroll).flex_1().min_h_0().overflow_y_scroll()
                .p_3().flex().flex_col().gap_3().child(self.event_view(sel, cx)).into_any_element(),
            DetailTab::Events => {
                let events = self.event_list(sel, window, cx);
                div().flex_1().min_h_0().p_3().flex().flex_col().gap_2()
                    .child(events).into_any_element()
            }
        }
    }

    /// 対象はタブを切り替えても常に読める位置に示す。
    fn detail_scope(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[sel.lane];
        let whole = sel.first == 0 && sel.last + 1 == lane.events.len();
        let overview = self.detail_tab == DetailTab::Session;
        let events = &lane.events[sel.first..=sel.last];
        let (start, end) = if overview { (s.start, s.end) } else {
            (events[0].start, self.cache(sel).map_or(events.last().unwrap().end, |d| d.end()))
        };
        let title = if overview { "セッション全体（全エージェント）".to_string() } else {
            format!("{} · {} 件", if whole { "レーン全体" } else { "選択範囲" }, fmt_n(events.len()))
        };
        div().flex_none().px_3().py_1p5().flex().flex_col().gap_1().text_xs()
            .border_b_1().border_color(c(theme::BORDER)).text_color(c(theme::MUTED))
            .child(div().flex().items_center().gap_2().child(title).child(div().flex_1())
                .when(!overview && !whole, |d| d.child(self.icon_button(
                    "scope-lane", Some(Icon::Layers), "レーン全体を見る", false, cx,
                    move |app, w, cx| {
                        let tab = app.detail_tab;
                        let n = app.store.sessions[sel.session].lanes[sel.lane].events.len();
                        if n == 0 { return; }
                        app.navigate(Selection { first: 0, last: n - 1, ..sel }, w, cx);
                        app.set_detail_tab(tab, cx);
                    },
                ))))
            .child(format!("{} – {}", local(start).format("%Y/%m/%d %H:%M:%S"),
                local(end).format(if local(start).date_naive() == local(end).date_naive() {
                    "%H:%M:%S"
                } else { "%Y/%m/%d %H:%M:%S" })))
    }

    fn detail_tabs(&self, sel: Selection, tab: DetailTab, files: usize, cx: &mut Context<Self>) -> Div {
        let n_events = (sel.first != sel.last).then(|| sel.last - sel.first + 1);
        div().flex_none().px_3().py_2().border_b_1().border_color(c(theme::BORDER)).child(
            div()
                .flex()
                .gap_0p5()
                .p(px(2.0))
                .rounded_lg()
                .bg(c(theme::BG))
                .border_1()
                .border_color(c(theme::BORDER))
                .child(self.tab_button(DetailTab::Session, "概要", None, "1", tab, true, cx))
                .child(self.tab_button(DetailTab::Events, "履歴", n_events, "2", tab, true, cx))
                .child(self.tab_button(DetailTab::Files, "変更", Some(files), "3", tab, true, cx))
                .child(self.tab_button(DetailTab::Analysis, "分析", None, "4", tab, true, cx)),
        )
    }

    /// タブ。区切られた選択肢の形にし、選んでいるものを浮かせる。件数は小さい丸の中に出す
    #[allow(clippy::too_many_arguments)]
    fn tab_button(
        &self,
        tab: DetailTab,
        text: &'static str,
        count: Option<usize>,
        key: &'static str,
        cur: DetailTab,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = tab == cur;
        let fg = if on {
            theme::TEXT
        } else if enabled {
            theme::MUTED
        } else {
            theme::SUBTLE
        };
        div()
            .id(SharedString::from(format!("tab-{tab:?}")))
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .gap_1p5()
            .h(px(26.0))
            .rounded_md()
            .text_xs()
            .text_color(c(fg))
            .when(on, |d| {
                d.bg(c(theme::ELEVATED)).border_1().border_color(c(theme::BORDER)).font_weight(FontWeight::SEMIBOLD)
            })
            .child(text)
            .when_some(count.filter(|&n| n > 0), |d, n| {
                d.child(
                    div()
                        .px_1p5()
                        .rounded_full()
                        .bg(c(if on { theme::ACCENT } else { theme::GRID }).opacity(if on { 0.25 } else { 1.0 }))
                        .text_color(c(if on { theme::ACCENT } else { theme::MUTED }))
                        .text_size(px(10.0))
                        .child(fmt_n(n)),
                )
            })
            .tooltip(tip(if enabled { text } else { "編集したファイルはありません" }, Some(key)))
            .when(enabled && !on, |d| {
                d.cursor_pointer()
                    .hover(|s| s.text_color(c(theme::TEXT)).bg(c(theme::HOVER)))
                    .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| {
                        t.set_detail_tab(tab, cx);
                        window.focus(&t.focus_handle);
                    }))
            })
    }

    pub(super) fn set_detail_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        if self.detail_tab != tab {
            self.pause_replay(cx);
            self.replay_autostart = None;
            self.detail_scroll_offsets[detail_tab_index(self.detail_tab)] = self.detail_scroll.offset();
            self.detail_tab = tab;
            self.detail_scroll.set_offset(self.detail_scroll_offsets[detail_tab_index(tab)]);
            self.cancel_detail_follow();
            self.persist();
        }
        if tab == DetailTab::Analysis {
            self.analysis_content_view.update(cx, |_, cx| cx.notify());
        }
        self.request_analysis(cx);
        self.detail_changed(cx);
    }

    /// 通常のパネルと全画面で実際に本文を描く幅。可変高リストの計測を幅に合わせる。
    pub(super) fn detail_content_width(&self, window: &Window) -> f32 {
        if self.detail_fullscreen {
            (f32::from(window.viewport_size().width) - super::GAP * 2.0 - 2.0).max(0.0)
        } else {
            self.pane_full_widths(window).1
        }
    }

    pub(super) fn toggle_detail_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.statistics_open || self.selected.is_none_or(|s| !self.valid(s)) { return; }
        self.detail_fullscreen = !self.detail_fullscreen;
        self.popover = None;
        self.hovered = None;
        self.drag = None;
        self.resize = None;
        self.cancel_detail_follow();
        window.focus(&self.focus_handle);
        self.detail_changed(cx);
        cx.notify();
    }

    fn cache(&self, sel: Selection) -> Option<&DetailCache> {
        self.detail_cache.as_ref().filter(|d| d.sel == sel)
    }

    pub(super) fn shown_detail_selection(&self) -> Option<Selection> {
        self.selected.filter(|&sel| self.valid(sel)).map(|sel| {
            // 末尾へ追従する選択が伸びても、完了前に未読み込みの行を混ぜない。
            self.detail_cache.as_ref().map_or(sel, |d| d.sel)
        })
    }

    pub(super) fn detail_session(&self, sel: Selection) -> &Session {
        self.detail_snapshot.as_ref().filter(|_| self.cache(sel).is_some())
            .map_or(&self.store.sessions[sel.session], |(session, _)| session)
    }

    pub(super) fn shown_detail_generation(&self) -> u64 {
        self.detail_snapshot.as_ref().map_or_else(
            || self.detail_gen.load(super::Ordering::Relaxed),
            |(_, generation)| *generation,
        )
    }

    pub(super) fn shown_selection_generation(&self) -> u64 {
        if self.detail_cache.is_some() { self.detail_selection_gen } else {
            self.detail_gen.load(super::Ordering::Relaxed)
        }
    }

    pub(super) fn detail_live_changed(&self) -> bool {
        self.detail_live.get().is_some_and(|before| self.shown_detail_selection()
            .is_some_and(|sel| before != is_live(self.detail_session(sel).end)))
    }

    /// 共通見出しにはセッションと選択の履歴、表示操作を置く。
    fn detail_header(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        div().h(px(HEADER_H)).flex_none().flex().items_center().gap_1().px_2()
            .border_b_1().border_color(c(theme::BORDER))
            .child(self.ghost_button("d-back", Icon::ArrowLeft, "戻る", Some(crate::platform::shortcut("⌘[", "Ctrl+[")),
                !self.back.is_empty(), cx, |t, w, cx| t.go_back(w, cx)))
            .child(self.ghost_button("d-forward", Icon::ArrowRight, "進む", Some(crate::platform::shortcut("⌘]", "Ctrl+]")),
                !self.forward.is_empty(), cx, |t, w, cx| t.go_forward(w, cx)))
            .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD)
                .child(Marquee::new("session-title", if s.title.is_empty() { "(無題)".to_string() }
                    else { one_line(&s.title, usize::MAX) })))
            .child(self.ghost_button("d-locate", Icon::Locate, "選択位置を表示", None, true, cx,
                |t, w, cx| { t.detail_fullscreen = false;
                    if let Some(sel) = t.selected { t.reveal(sel, true, w); }
                    t.detail_changed(cx); cx.notify(); }))
            .child(self.ghost_button("d-fullscreen", if self.detail_fullscreen { Icon::Minimize } else { Icon::Maximize },
                if self.detail_fullscreen { "通常のパネルに戻す" } else { "詳細を全画面表示" },
                Some(crate::platform::shortcut("⇧⌘↵", "Ctrl+Shift+Enter")), true, cx, |t, w, cx| t.toggle_detail_fullscreen(w, cx)))
            .child(self.ghost_button("d-close", Icon::X, "詳細を閉じる", Some("esc"), true, cx,
                |t, w, cx| { t.close_detail(cx); w.focus(&t.focus_handle); }))
    }

    fn history_heading(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[sel.lane];
        let single = sel.first == sel.last;
        let e = &lane.events[sel.first];
        let title = if single { e.name.to_string() } else { "イベントの履歴".into() };
        let pos = if single { fmt_n(sel.first + 1) } else {
            format!("{}–{}", fmt_n(sel.first + 1), fmt_n(sel.last + 1))
        };
        let output = self.cache(sel).and_then(|d| d.texts.first()).map_or("", |t| t.1.as_str());
        div().flex_none().flex().items_center().gap_1().text_xs()
            .child(label(title))
            .when(single, |d| d.when_some(status_label(e, is_live(s.end), output),
                |d, (text, color)| d.child(badge(text, color))))
            .child(div().flex_1())
            .child(self.ghost_button("d-prev", Icon::ChevronUp, "前のイベント", Some("K"),
                !single || if self.detail_reversed { sel.first + 1 < lane.events.len() } else { sel.first > 0 },
                cx, |t, w, cx| t.step_event(-1, w, cx)).size(px(22.0)))
            .child(div().text_color(c(theme::SUBTLE)).child(format!("{pos} / {}", fmt_n(lane.events.len()))))
            .child(self.ghost_button("d-next", Icon::ChevronDown, "次のイベント", Some("J"),
                !single || if self.detail_reversed { sel.first > 0 } else { sel.first + 1 < lane.events.len() },
                cx, |t, w, cx| t.step_event(1, w, cx)).size(px(22.0)))
    }

    /// 見出しの下の文脈。読み込み元・プロジェクト・セッション・サブエージェントをパンくずで出す。
    /// セッション名を押すとセッションのタブへ、サブエージェントの矢印で起動元へ移る
    fn context_bar(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[sel.lane];
        let (logo, logo_color) = icons::source_icon(s.source);
        let title = if s.title.is_empty() { "(無題)".to_string() } else { s.title.clone() };
        let sep = || icons::icon(Icon::ChevronRight, theme::SUBTLE).size_3().flex_none();
        let mut bar = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_1p5()
            .text_xs()
            .text_color(c(theme::MUTED))
            .bg(c(theme::BG))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .id("ctx-src")
                    .flex_none()
                    .child(icons::icon(logo, logo_color))
                    .tooltip(tip(s.source.label(), None)),
            )
            .child(project_tile(&s.cwd, 14.0))
            .child(
                div()
                    .id("ctx-project")
                    .flex_shrink()
                    .min_w(px(24.0))
                    .max_w(px(120.0))
                    .truncate()
                    .child(self.project_name(&s.cwd))
                    .tooltip(tip(tilde(&s.cwd), None)),
            )
            .child(sep())
            .child(
                div()
                    .id("ctx-session")
                    .flex_1()
                    .min_w_0()
                    .text_color(c(theme::TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|d| d.underline())
                    .child(Marquee::new("title", one_line(&title, usize::MAX)))
                    .tooltip(tip("セッションの概要", Some("1")))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.set_detail_tab(DetailTab::Session, cx))),
            );
        if sel.lane != 0 {
            let (session, parent_id) = (sel.session, lane.spawned_by.clone());
            bar = bar
                .child(sep())
                .child(icons::icon(Icon::Bot, theme::SUB).size_3().flex_none())
                .child(
                    div()
                        .flex_shrink()
                        .min_w(px(24.0))
                        .max_w(px(120.0))
                        .truncate()
                        .text_color(c(theme::SUB))
                        .child(lane.name.clone()),
                )
                .when_some(parent_id, |d, id| {
                    d.child(
                        self.ghost_button(
                            "to-parent",
                            Icon::ArrowUp,
                            "起動元のイベントへ",
                            None,
                            true,
                            cx,
                            move |t, w, cx| {
                                let s = &t.store.sessions[session];
                                let parent = s.parents.get(sel.lane).copied().flatten().unwrap_or(0);
                                if let Some(i) =
                                    s.lanes[parent].events.iter().position(|e| e.call_id.as_deref() == Some(id.as_str()))
                                {
                                    let sel = Selection { session, lane: parent, first: i, last: i };
                                    t.navigate(sel, w, cx);
                                    t.reveal(sel, false, w);
                                }
                            },
                        )
                        .size(px(20.0)),
                    )
                });
        }
        bar.when(is_live(s.end), |d| {
            d.child(div().id("ctx-live").child(dot(theme::LIVE, 7.0)).tooltip(tip("進行中", None)))
        })
    }

    /// レーン全体を選び、イベントのタブで `filter` を掛ける（集計から該当イベントへ移る）
    pub(super) fn open_lane_filtered(
        &mut self,
        session: usize,
        lane: usize,
        filter: Option<RowFilter>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(n) = self.store.sessions.get(session).and_then(|s| s.lanes.get(lane)).map(|l| l.events.len()) else {
            return;
        };
        if n == 0 {
            return;
        }
        let sel = Selection { session, lane, first: 0, last: n - 1 };
        self.navigate(sel, w, cx);
        self.set_row_filter(filter, cx);
        self.set_detail_tab(DetailTab::Events, cx);
        self.reveal(sel, true, w);
    }

    /// イベントの絞り込み（名前・失敗）
    fn selection_summary(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        let summary = &self.cache(sel).unwrap().selection_summary;
        let st = summary.total.stats;
        let counts = &summary.names;
        let filter = self.row_filter.clone();
        let chip = |id: SharedString, text: String, col: Option<(u32, Icon)>, on: bool, f: Option<RowFilter>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .py(px(1.0))
                .rounded_md()
                .border_1()
                .border_color(c(if on { theme::ACCENT } else { theme::BORDER }))
                .bg(c(if on { theme::ACCENT_BG } else { theme::BG }))
                .text_xs()
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::HOVER)))
                .when_some(col, |d, (col, ic)| d.child(event_icon(ic, col, 11.0)))
                .child(text)
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    t.set_row_filter(if t.row_filter == f { None } else { f.clone() }, cx);
                }))
        };
        let mut chips = div().flex().flex_wrap().gap_1().child(chip(
            "chip-all".into(),
            format!("すべて {}", fmt_n(evs.len())),
            None,
            filter.is_none(),
            None,
        ));
        for (i, (name, n, first)) in counts.iter().take(12).enumerate() {
            let f = RowFilter::Name(name.to_string());
            let on = filter.as_ref() == Some(&f);
            chips = chips.child(chip(
                SharedString::from(format!("chip-{i}")),
                format!("{name} ×{n}"),
                Some((theme::event_color(&evs[*first]), theme::event_icon(&evs[*first]))),
                on,
                Some(f),
            ));
        }

        if st.failures() > 0 {
            chips = chips.child(chip("chip-failed".into(), format!("失敗等 {}", st.failures()),
                None, filter == Some(RowFilter::Failed), Some(RowFilter::Failed)));
        }
        chips.pb_2().border_b_1().border_color(c(theme::BORDER))
    }


}

/// 追加・削除の割合を 5 マスで表す（GitHub の差分の表示と同じ）
fn diff_bar(added: usize, removed: usize) -> Div {
    let total = added + removed;
    let green = if total == 0 { 0 } else { ((added * 5) as f32 / total as f32).round() as usize };
    let red = if total == 0 { 0 } else { 5 - green };
    div().flex_none().flex().gap(px(1.0)).children((0..5).map(move |i| {
        let col = if i < green {
            c(theme::OK)
        } else if i < green + red {
            c(theme::ERR)
        } else {
            c(theme::GRID)
        };
        div().size(px(6.0)).rounded(px(1.0)).bg(col)
    }))
}

#[cfg(test)]
mod tests;
