//! 右サイドパネル。1 / 2 / 3 / 4 で「概要」「履歴」「変更」「分析」を切り替える。
//!
//! 概要はセッション全体の指示・応答・エージェント構成を示す。
//! 履歴・変更・分析は選択範囲を対象にし、対象範囲を共通上部に明示する。
//! 履歴では指示と応答を読み、ツール実行と思考を必要なときに展開する。
//! 概要、履歴、変更、分析の一覧は可視行だけを描き、タブ切り替えで位置を保持する。

use std::cell::{Cell, RefCell};

use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Div, ElementId, FontWeight, ListAlignment, ListOffset,
    ListState, SharedString, Stateful, WeakEntity, Window, div, list, prelude::*, px,
};

use super::rich;
use super::widgets::{Marquee, dot, event_icon, project_tile, tip};
use super::{DETAIL_MAX, HEADER_H, INNER_RADIUS, Rireki, Selection, fmt_dur, fmt_n, is_live, local, tilde};
use crate::icons::{self, Icon};
use crate::model::{Event, Kind, Session, Source, Status, Usage, UsageSum, one_line};

mod list;
mod loading;
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

/// 詳細パネル。選択が変わったときだけ描き直すよう、別ビューとしてキャッシュする
pub struct DetailView {
    pub(super) app: WeakEntity<Rireki>,
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
    pub(super) fn detail_uses_list(&self) -> bool {
        self.shown_detail_selection().filter(|&sel| sel.first < sel.last)
            .is_some_and(|_| self.detail_tab == DetailTab::Events)
    }

    pub(super) fn detail_at_bottom(&self) -> bool {
        if self.detail_tab == DetailTab::Files && self.detail_cache.as_ref().is_some_and(|d| !d.files.is_empty()) {
            self.file_list.at_bottom()
        } else if self.detail_uses_list() {
            self.detail_list.at_bottom()
        } else {
            -self.detail_scroll.offset().y >= self.detail_scroll.max_offset().height - px(4.0)
        }
    }

    pub(super) fn detail_at_latest(&self) -> bool {
        if self.detail_uses_list() {
            self.detail_list.at_latest(self.detail_reversed)
        } else {
            self.detail_at_bottom()
        }
    }

    /// 未計測の行も少しずつ描きながら追う。ユーザー操作・絞り込み・選択変更で止める
    pub(super) fn animate_list_to_latest(&mut self, cx: &mut Context<Self>) {
        const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
        const MAX_FRAMES: usize = 40;
        let reversed = self.detail_reversed;
        self.detail_following = true;
        self.follow_anim = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FRAME).await;
            for frame in 0..MAX_FRAMES {
                let Ok(keep_going) = this.update(cx, |app, cx| {
                    if !app.detail_uses_list() || app.detail_reversed != reversed { return None; }
                    let list = &app.detail_list.state;
                    let offset = list.logical_scroll_top();
                    if reversed && offset.item_ix == 0 && offset.offset_in_item <= px(1.0) {
                        app.detail_list.scroll_to_latest(true);
                        app.detail_changed(cx);
                        return None;
                    }
                    if !reversed && app.detail_list.at_bottom() { return None; }
                    if frame + 1 == MAX_FRAMES {
                        app.detail_list.scroll_to_latest(reversed);
                    } else if reversed {
                        // 上端へ近づくほど減速する。移動量を先読み範囲内に抑え、未計測の行を飛ばさない。
                        let distance = (-list.scroll_px_offset_for_scrollbar().y * 0.2).clamp(px(1.0), px(100.0));
                        list.scroll_by(-distance);
                    } else {
                        let bounds = list.viewport_bounds();
                        let distance = list.item_count().checked_sub(1).and_then(|i| list.bounds_for_item(i))
                            .map(|last| ((last.bottom() - bounds.bottom()) * 0.2).max(px(1.0)))
                            .unwrap_or_else(|| (bounds.size.height * 0.15).clamp(px(40.0), px(100.0)));
                        list.scroll_by(distance);
                    }
                    app.detail_changed(cx);
                    Some(())
                }) else { return; };
                if keep_going.is_none() { break; }
                cx.background_executor().timer(FRAME).await;
            }
            let _ = this.update(cx, |app, _| app.detail_following = false);
        }));
    }

    pub(super) fn cancel_detail_follow(&mut self) {
        self.detail_follow = false;
        self.detail_following = false;
        self.follow_anim = None;
    }

    pub(super) fn set_row_filter(&mut self, filter: Option<RowFilter>, cx: &mut Context<Self>) {
        self.row_filter = filter;
        self.cancel_detail_follow();
        self.detail_changed(cx);
    }

    fn set_detail_order(&mut self, reversed: bool, cx: &mut Context<Self>) {
        if self.detail_reversed == reversed { return; }
        let following = self.replay.as_ref().is_some_and(|replay| replay.following)
            || self.detail_list.at_latest(self.detail_reversed);
        self.detail_reversed = reversed;
        if let Some(replay) = &mut self.replay {
            replay.following = following;
            replay.check_follow = false;
            replay.scroll_pending.set(following);
        }
        self.cancel_detail_follow();
        self.persist();
        self.detail_changed(cx);
    }


    /// 表示順で同じレーンの次（`dir` = 1）/ 前（-1）を選ぶ。複数を選んでいるときは、表示上の先頭 / 末尾を選ぶ。
    /// 1 イベントずつ進むときは履歴に積まない（「戻る」で一覧に戻れるようにする）
    pub(super) fn step_event(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cur) = self.selected.filter(|&s| self.valid(s)) else {
            return;
        };
        let n = self.store.sessions[cur.session].lanes[cur.lane].events.len();
        let dir = if self.detail_reversed { -dir } else { dir };
        let i = if cur.first != cur.last {
            if dir > 0 { cur.first } else { cur.last }
        } else {
            match cur.first.checked_add_signed(dir).filter(|&i| i < n) {
                Some(i) => i,
                None => return,
            }
        };
        let sel = Selection { session: cur.session, lane: cur.lane, first: i, last: i };
        if cur.first != cur.last {
            self.navigate(sel, window, cx);
        } else {
            self.select(sel, cx);
            self.set_detail_tab(DetailTab::Events, cx);
            cx.notify();
        }
        self.reveal(sel, false, window);
    }

    pub(super) fn detail(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let panel = div().id("detail-panel").size_full().flex().flex_col()
            .when(self.detail_fullscreen, |d| d.rounded(px(INNER_RADIUS)))
            .when(!self.detail_fullscreen, |d| d.rounded_r(px(INNER_RADIUS)).border_l_1())
            .bg(c(theme::PANEL)).border_color(c(theme::BORDER)).text_sm().text_color(c(theme::TEXT))
            .on_scroll_wheel(cx.listener(|app, _: &gpui::ScrollWheelEvent, _, _| app.cancel_detail_follow()));
        let Some(sel) = self.shown_detail_selection() else { return panel.into_any_element(); };
        let files = self.cache(sel).map_or(0, |d| d.files.len());
        let tab = self.detail_tab;
        let panel = panel.child(self.detail_header(sel, cx)).child(self.context_bar(sel, cx))
            .child(self.detail_tabs(sel, tab, files, cx)).child(self.detail_scope(sel, cx));
        if self.cache(sel).is_none() {
            return panel.child(div().p_3().text_xs().text_color(c(theme::MUTED))
                .child("詳細を読み込み中…")).into_any_element();
        }
        let content = match tab {
            DetailTab::Session => self.overview_view(sel, window, cx),
            DetailTab::Analysis => self.analysis_view(sel, cx),
            DetailTab::Files if files == 0 => div().p_3().text_xs().text_color(c(theme::MUTED))
                .child("選択範囲に記録されたファイル編集はありません").into_any_element(),
            DetailTab::Files => div().flex_1().min_h_0().p_3().flex().flex_col()
                .child(self.files_list(sel, self.cache(sel).unwrap(), window, cx)).into_any_element(),
            DetailTab::Events if sel.first == sel.last => div().id("detail-scroll")
                .track_scroll(&self.detail_scroll).flex_1().min_h_0().overflow_y_scroll()
                .p_3().flex().flex_col().gap_3().child(self.history_heading(sel, cx))
                .child(self.replay_controls(sel, cx)).child(self.event_view(sel, cx)).into_any_element(),
            DetailTab::Events => {
                let events = self.event_list(sel, window, cx);
                div().flex_1().min_h_0().p_3().flex().flex_col().gap_2()
                    .child(self.history_heading(sel, cx))
                    .when(self.replay.is_some(), |d| d.child(self.event_list_header(sel, cx)))
                    .child(events).into_any_element()
            }
        };
        panel.child(content).into_any_element()
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

    /// 概要末尾の基本情報と再開・ログの操作。モデルはメインレーンの属性を表示する。
    fn overview_information(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[0];
        let meta = |k: &'static str, v: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(div().w(px(76.0)).flex_none().text_color(c(theme::SUBTLE)).child(k))
                .child(div().flex_1().min_w_0().child(v))
        };
        let model = match (lane.model.is_empty(), lane.effort.is_empty()) {
            (true, true) => None,
            (false, true) => Some(lane.model.clone()),
            (true, false) => Some(lane.effort.clone()),
            (false, false) => Some(format!("{} {}", lane.model, lane.effort)),
        };
        let version = (!s.version.is_empty()).then(|| match s.source {
            Source::Claude => format!("Claude Code {}", s.version),
            Source::Codex => format!("Codex CLI {}", s.version),
        });
        let session = sel.session;
        let resume = resume_command(s);
        let resumed = self.copied == Some("copy-resume");
        let origin = s.lanes.first().map(|l| l.origin.clone()).unwrap_or_default();
        let main_n = s.lanes[0].events.len();
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_1p5()
            .child(
                self.action_button(
                    "resume",
                    if resumed { Icon::Check } else { Icon::Terminal },
                    if resumed { "コピーしました" } else { "再開コマンドをコピー" },
                )
                .tooltip(tip(resume.clone(), None))
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(resume.clone()));
                    t.flash_copied("copy-resume", cx);
                })),
            )
            .when(main_n > 0, |d| {
                d.child(
                    self.action_button("all-events", Icon::Layers, "全イベントを一覧")
                        .tooltip(tip("メインのレーン全体を選ぶ", None))
                        .on_click(
                            cx.listener(move |t, _: &ClickEvent, w, cx| t.open_lane_filtered(session, 0, None, w, cx)),
                        ),
                )
            })
            .when(!origin.is_empty(), |d| {
                d.child(
                    self.action_button("reveal-log", Icon::FolderOpen, "ログを Finder で表示")
                        .tooltip(tip(origin.clone(), None))
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.reveal_path(std::path::Path::new(&origin));
                        })),
                )
            });
        let section = |name: &'static str| {
            div().flex().flex_col().gap_1p5().pt_2().border_t_1().border_color(c(theme::BORDER)).child(label(name))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                section("基本情報").child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_xs()
                        .when_some(model, |d, m| d.child(meta("モデル", div().child(m).into_any_element())))
                        .when(!s.branch.is_empty(), |d| {
                            d.child(meta("ブランチ", div().child(s.branch.clone()).into_any_element()))
                        })
                        .child(meta("ディレクトリ", div().child(tilde(&s.cwd)).into_any_element()))
                        .when(self.expanded.contains("overview-meta"), |d| d.child(meta(
                            "セッション",
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().min_w_0().truncate().font_family(rich::MONO).child(s.id.clone()))
                                .child(self.copy_button("copy-id", s.id.clone(), cx).flex_none())
                                .into_any_element(),
                        )))
                        .when(self.expanded.contains("overview-meta"), |d| d.when_some(version, |d, v| d.child(meta("バージョン", div().child(v).into_any_element()))))
                        .when(s.headless, |d| d.child(meta("起動", div().child("非対話").into_any_element()))),
                ),
            )
            .child(self.icon_button("overview-meta-toggle", Some(Icon::ChevronDown),
                if self.expanded.contains("overview-meta") { "詳細情報を閉じる" } else { "ID・バージョンを表示" },
                false, cx, |t, _, cx| {
                    if !t.expanded.remove("overview-meta") { t.expanded.insert("overview-meta"); }
                    t.overview_list.invalidate_information();
                    t.detail_changed(cx);
                }))
            .child(actions)
    }

    fn action_button(&self, id: &'static str, icon: Icon, label: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .text_xs()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(icon, if icon == Icon::Check { theme::OK } else { theme::MUTED }))
            .child(label)
    }

    // ------------------------------------------------------------ 1 イベント


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
mod tests {
    use super::*;

    fn list_event(name: &str, status: Status) -> Event {
        Event {
            start: 1.0, end: 2.0, kind: Kind::Tool, name: name.into(), summary: Default::default(),
            input: String::new(), output: String::new(), call_id: None, text_ref: Default::default(),
            usage: Default::default(), model: None, status,
        }
    }

    #[test]
    fn detail_usage_scale_tracks_filters_display_limit_and_updates() {
        let mut events: Vec<_> = (0..=DETAIL_MAX).map(|_| list_event("Read", Status::Ok)).collect();
        events[0].usage = Usage { input: 100, cache_read: 50, cache_write: 25, output: 2, ..Default::default() };
        events[1] = list_event("Edit", Status::Error);
        events[1].usage = Usage { input: 10, output: 80, ..Default::default() };
        events[DETAIL_MAX].usage = Usage { input: 10_000, output: 10_000, ..Default::default() };
        let state = DetailList::new();
        let key = |generation, filter, reversed| DetailListKey {
            sel: Selection { session: 1, lane: 0, first: 0, last: DETAIL_MAX },
            generation, filter, reversed, width: 400, live: true, replay_visible: None,
        };
        state.prepare(key(1, None, false), &events);
        assert_eq!(state.usage_scale(), UsageScale { input: 175, output: 80 });
        state.prepare(key(1, None, true), &events);
        assert_eq!(state.usage_scale(), UsageScale { input: 175, output: 80 });
        state.prepare(key(1, Some(RowFilter::Name("Read".into())), false), &events);
        assert_eq!(state.usage_scale(), UsageScale { input: 175, output: 2 });
        state.prepare(key(1, Some(RowFilter::Failed), false), &events);
        assert_eq!(state.usage_scale(), UsageScale { input: 10, output: 80 });
        state.prepare(key(1, Some(RowFilter::Name("Missing".into())), false), &events);
        assert_eq!(state.usage_scale(), UsageScale::default());
        events[0].usage = Usage {
            input: u32::MAX, cache_read: u32::MAX, cache_write: u32::MAX, output: 1, ..Default::default()
        };
        state.prepare(key(2, None, false), &events);
        assert_eq!(state.usage_scale(), UsageScale { input: u32::MAX as u64 * 3, output: 80 });
    }

    #[test]
    fn replay_usage_scale_includes_new_rows_and_resets_on_rewind() {
        let mut events = vec![list_event("Read", Status::Ok), list_event("Edit", Status::Ok), list_event("Read", Status::Ok)];
        events[0].usage = Usage { input: 100, output: 2, ..Default::default() };
        events[1].usage = Usage { input: 50, cache_read: 100, ..Default::default() };
        events[2].usage = Usage { output: 40, ..Default::default() };
        for reversed in [false, true] {
            let state = DetailList::new();
            let key = |visible, filter| DetailListKey {
                sel: Selection { session: 1, lane: 0, first: 0, last: 2 },
                generation: 1, filter, reversed, width: 400, live: false, replay_visible: Some(visible),
            };
            state.prepare(key(0, None), &events);
            assert_eq!(state.usage_scale(), UsageScale::default());
            state.prepare(key(1, None), &events);
            assert_eq!(state.usage_scale(), UsageScale { input: 100, output: 2 });
            state.prepare(key(2, None), &events);
            assert_eq!(state.usage_scale(), UsageScale { input: 150, output: 2 });
            state.prepare(key(3, None), &events);
            assert_eq!(state.usage_scale(), UsageScale { input: 150, output: 40 });
            state.prepare(key(1, None), &events);
            assert_eq!(state.usage_scale(), UsageScale { input: 100, output: 2 });
            let filter = Some(RowFilter::Name("Read".into()));
            state.prepare(key(1, filter.clone()), &events);
            state.prepare(key(3, filter), &events);
            assert_eq!(state.usage_scale(), UsageScale { input: 100, output: 40 });
        }
    }

    #[test]
    fn detail_list_filters_within_original_display_limit() {
        let mut events: Vec<_> = (0..DETAIL_MAX + 3).map(|i| {
            list_event(if i % 2 == 0 { "Read" } else { "Edit" }, match i % 5 {
                0 => Status::Error, 1 => Status::Rejected, 2 => Status::Interrupted,
                3 => Status::Pending, _ => Status::Ok,
            })
        }).collect();
        let all = detail_rows(&events, None, false);
        assert_eq!(all.len(), DETAIL_MAX + 2);
        assert_eq!(all[0], DetailRow::Summary);
        assert_eq!(all[DETAIL_MAX], DetailRow::Event(DETAIL_MAX - 1));
        assert_eq!(all[DETAIL_MAX + 1], DetailRow::Limit);
        for filter in [RowFilter::Name("Edit".into()), RowFilter::Failed] {
            let rows = detail_rows(&events, Some(&filter), false);
            let expected: Vec<_> = events.iter().take(DETAIL_MAX).enumerate().filter(|(_, e)| match &filter {
                RowFilter::Name(name) => e.name.as_ref() == name,
                RowFilter::Failed => matches!(e.status, Status::Error | Status::Rejected | Status::Interrupted),
            }).map(|(i, _)| DetailRow::Event(i)).collect();
            assert_eq!(&rows[1..rows.len() - 1], &expected);
        }
        events[DETAIL_MAX].name = "AfterLimit".into();
        assert_eq!(detail_rows(&events, Some(&RowFilter::Name("AfterLimit".into())), false),
            [DetailRow::Summary, DetailRow::Empty, DetailRow::Limit]);
        assert_eq!(detail_rows(&[], None, false), [DetailRow::Summary, DetailRow::Empty]);
    }

    #[test]
    fn newest_history_orders_events_by_time_across_prompts() {
        let events: Vec<_> = [Kind::Tool, Kind::User, Kind::Tool, Kind::Assistant,
            Kind::User, Kind::Thinking, Kind::Assistant].into_iter().enumerate().map(|(i, kind)| {
                Event { start: i as f64, end: i as f64 + 1.0, kind, ..list_event("event", Status::Ok) }
            }).collect();
        assert_eq!(detail_rows(&events, None, true), [DetailRow::Summary,
            DetailRow::Event(6), DetailRow::Event(5), DetailRow::Event(4),
            DetailRow::Event(3), DetailRow::Event(2), DetailRow::Event(1), DetailRow::Event(0)]);
        assert_eq!(replay::rows(&events, 6, None, true), [DetailRow::Event(5), DetailRow::Event(4),
            DetailRow::Event(3), DetailRow::Event(2), DetailRow::Event(1), DetailRow::Event(0)]);
    }

    #[test]
    fn single_prompt_history_changes_order_in_normal_and_replay_lists() {
        let events: Vec<_> = [Kind::User, Kind::Thinking, Kind::Tool, Kind::Assistant]
            .into_iter().enumerate().map(|(i, kind)| {
                Event { start: i as f64, end: i as f64 + 1.0, kind, ..list_event("event", Status::Ok) }
            }).collect();
        for replay_visible in [None, Some(events.len())] {
            let state = DetailList::new();
            let key = |reversed| DetailListKey {
                sel: Selection { session: 1, lane: 0, first: 10, last: 13 },
                generation: 1, filter: None, reversed, width: 400, live: false, replay_visible,
            };
            let shown_events = || state.data.borrow().as_ref().unwrap().rows.iter().filter_map(|row| {
                if let DetailRow::Event(i) = row { Some(*i) } else { None }
            }).collect::<Vec<_>>();
            state.prepare(key(false), &events);
            assert_eq!(shown_events(), [0, 1, 2, 3]);
            state.prepare(key(true), &events);
            assert_eq!(shown_events(), [3, 2, 1, 0]);
            state.prepare(key(false), &events);
            assert_eq!(shown_events(), [0, 1, 2, 3]);
        }
    }

    #[test]
    fn detail_list_keeps_event_anchor_on_filter_width_and_live_updates() {
        let events = vec![list_event("Read", Status::Ok), list_event("Edit", Status::Error), list_event("Edit", Status::Ok)];
        let state = DetailList::new();
        let key = |generation, filter, width, last| DetailListKey {
            sel: Selection { session: 1, lane: 2, first: 10, last }, generation, filter, reversed: false, width, live: false,
            replay_visible: None,
        };
        state.prepare(key(1, None, 400, 12), &events);
        state.state.scroll_to(ListOffset { item_ix: 2, offset_in_item: px(17.0) });
        state.prepare(key(1, Some(RowFilter::Name("Edit".into())), 400, 12), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
        state.prepare(key(2, Some(RowFilter::Name("Edit".into())), 320, 13), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
        state.prepare(key(2, Some(RowFilter::Name("Read".into())), 320, 13), &events);
        assert_eq!(state.state.logical_scroll_top().item_ix, 0);
        state.state.scroll_to(ListOffset { item_ix: 1, offset_in_item: px(20.0) });
        state.reset();
        state.prepare(key(3, None, 400, 12), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (0, px(0.0)));
    }

    #[test]
    fn reversed_detail_list_keeps_display_limit_and_filters() {
        let mut events: Vec<_> = (0..DETAIL_MAX + 1).map(|_| list_event("Read", Status::Ok)).collect();
        events[0] = list_event("Edit", Status::Error);
        events[DETAIL_MAX - 1] = list_event("Edit", Status::Ok);
        events[DETAIL_MAX] = list_event("Edit", Status::Error);
        let rows = detail_rows(&events, None, true);
        assert_eq!(rows.len(), DETAIL_MAX + 2);
        assert_eq!(rows[0], DetailRow::Summary);
        assert_eq!(rows[1], DetailRow::Event(DETAIL_MAX - 1));
        assert_eq!(rows[DETAIL_MAX], DetailRow::Event(0));
        assert_eq!(rows[DETAIL_MAX + 1], DetailRow::Limit);
        assert_eq!(detail_rows(&events, Some(&RowFilter::Name("Edit".into())), true),
            [DetailRow::Summary, DetailRow::Event(DETAIL_MAX - 1), DetailRow::Event(0), DetailRow::Limit]);
        assert_eq!(detail_rows(&events, Some(&RowFilter::Failed), true),
            [DetailRow::Summary, DetailRow::Event(0), DetailRow::Limit]);
        assert_eq!(detail_rows(&events, Some(&RowFilter::Name("Missing".into())), true),
            [DetailRow::Summary, DetailRow::Empty, DetailRow::Limit]);
        assert_eq!(detail_rows(&[], None, true), [DetailRow::Summary, DetailRow::Empty]);
    }

    #[test]
    fn detail_list_keeps_event_anchor_when_reversed_and_extended() {
        let mut events = vec![list_event("Read", Status::Ok), list_event("Edit", Status::Error), list_event("Edit", Status::Ok)];
        events[0].kind = Kind::User;
        let state = DetailList::new();
        let key = |generation, last, reversed| DetailListKey {
            sel: Selection { session: 1, lane: 2, first: 10, last }, generation, filter: None, reversed,
            width: 400, live: false,
            replay_visible: None,
        };
        state.prepare(key(1, 12, false), &events);
        state.state.scroll_to(ListOffset { item_ix: 1, offset_in_item: px(17.0) });
        state.prepare(key(1, 12, true), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (3, px(17.0)));
        assert_eq!(state.data.borrow().as_ref().unwrap().rows[offset.item_ix], DetailRow::Event(0));
        events.push(list_event("Read", Status::Ok));
        state.prepare(key(2, 13, true), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (4, px(17.0)));
        assert_eq!(state.data.borrow().as_ref().unwrap().rows[offset.item_ix], DetailRow::Event(0));
        state.prepare(key(2, 13, false), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
    }

    #[test]
    fn replay_list_appends_filtered_events_keeps_scroll_and_rewinds() {
        let events = vec![list_event("Read", Status::Ok), list_event("Edit", Status::Error), list_event("Edit", Status::Ok)];
        let state = DetailList::new();
        let key = |visible, filter| DetailListKey {
            sel: Selection { session: 1, lane: 2, first: 10, last: 12 },
            generation: 1, filter, reversed: false, width: 400, live: false,
            replay_visible: Some(visible),
        };
        let edit = Some(RowFilter::Name("Edit".into()));
        state.prepare(key(1, edit.clone()), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Empty]);
        state.prepare(key(2, edit.clone()), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Event(1)]);
        state.state.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(17.0) });
        state.prepare(key(3, edit.clone()), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Event(1), DetailRow::Event(2)]);
        assert_eq!(state.state.logical_scroll_top().offset_in_item, px(17.0));
        state.prepare(key(1, edit), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Empty]);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (0, px(0.0)));
        state.prepare(key(1, None), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Event(0)]);
        state.prepare(key(3, None), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Event(0), DetailRow::Event(1), DetailRow::Event(2)]);
    }

    #[test]
    fn reversed_replay_prepends_new_events_and_preserves_the_event_being_read() {
        let events: Vec<_> = [Kind::User, Kind::Tool, Kind::Assistant, Kind::User, Kind::Tool]
            .into_iter().enumerate().map(|(i, kind)| {
                Event { start: i as f64, end: i as f64 + 1.0, kind, ..list_event("event", Status::Ok) }
            }).collect();
        let state = DetailList::new();
        let key = |visible, reversed| DetailListKey {
            sel: Selection { session: 1, lane: 2, first: 10, last: 14 },
            generation: 1, filter: None, reversed, width: 400, live: false,
            replay_visible: Some(visible),
        };
        state.prepare(key(3, true), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Event(2), DetailRow::Event(1), DetailRow::Event(0)]);
        state.state.scroll_to(ListOffset { item_ix: 1, offset_in_item: px(17.0) });
        state.prepare(key(5, true), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (3, px(17.0)));
        assert_eq!(state.data.borrow().as_ref().unwrap().rows[offset.item_ix], DetailRow::Event(1));
        assert_eq!(state.data.borrow().as_ref().unwrap().rows[0], DetailRow::Event(4));
        state.prepare(key(5, false), &events);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
        assert_eq!(state.data.borrow().as_ref().unwrap().rows[offset.item_ix], DetailRow::Event(1));
        state.scroll_to_latest(false);
        assert_eq!(state.state.logical_scroll_top().item_ix, 5);
        state.prepare(key(5, true), &events);
        state.scroll_to_latest(true);
        let offset = state.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (0, px(0.0)));
        state.prepare(key(1, true), &events);
        assert_eq!(state.data.borrow().as_ref().unwrap().rows, [DetailRow::Event(0)]);
    }

    #[test]
    fn replay_top_edge_excludes_scrolling_inside_the_first_event() {
        assert!(replay_at_top(ListOffset::default()));
        assert!(replay_at_top(ListOffset { item_ix: 0, offset_in_item: px(4.0) }));
        assert!(!replay_at_top(ListOffset { item_ix: 0, offset_in_item: px(17.0) }));
        assert!(!replay_at_top(ListOffset { item_ix: 1, offset_in_item: px(0.0) }));
    }

    #[test]
    fn resume_quotes_cwd() {
        let s = Session {
            id: "abc".into(),
            source: Source::Claude,
            cwd: "/w/it's here".into(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: Vec::new(),
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        assert_eq!(resume_command(&s), r#"cd '/w/it'\''s here' && claude --resume abc"#);
        let s = Session { source: Source::Codex, cwd: "/w".into(), ..s };
        assert_eq!(resume_command(&s), "cd '/w' && codex resume abc");
    }

    #[test]
    fn short_numbers() {
        assert_eq!(fmt_tokens(9_999), "9,999");
        assert_eq!(fmt_tokens(85_234), "85.2k");
        assert_eq!(fmt_tokens(12_345_678), "12.35M");
        assert_eq!(fmt_tokens(13_230_930_000), "13.23B");
        assert_eq!(fmt_usd(7435.094), "$7,435.09");
        assert_eq!(fmt_usd(0.995), "$1.00");
        assert_eq!(fmt_usd(0.0012), "$0.0012");
        assert_eq!(fmt_usd(0.0), "$0.00");
        assert_eq!(exit_code("Exit code 127\nnot found"), Some("127"));
        assert_eq!(exit_code("[exit 1 · 0.2s]\nerr"), Some("1"));
        assert_eq!(exit_code("[0.2s]\nok"), None);
    }
}
