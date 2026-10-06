//! 詳細パネル。タブ（1 / 2 / 3 / 4）で「イベント」「ファイル」「セッション」「分析」を切り替える
//!
//! - 見出し: イベント名と結果の状態（失敗・拒否・中断など）、レーン内の位置と前後への移動、選択の履歴
//! - 文脈の帯: 読み込み元 › プロジェクト › セッション（押すとセッションのタブへ）› サブエージェント（起動元へ）
//! - ファイル: 選択の中で編集したファイルと追加・削除の割合。押すとその場で開き、編集の時系列の帯と編集ごとの差分を出す
//! - セッション: 集計のタイル（失敗は押すと一覧へ）・操作・サブエージェントの一覧・トークン・属性
//! - 1 イベント: 時刻・トークン数・きっかけになった指示と、入力・結果（差分・Markdown・ANSI の色・行番号）
//! - 複数: 集計と、名前・失敗での絞り込み、ユーザーの指示で区切ったイベントの一覧
//! - 分析: 選択全体の検索・絞り込み・実行時間とトークンでの比較。全画面表示でも同じ選択を保つ

use std::cell::{Cell, RefCell};

use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Div, ElementId, FontWeight, ListAlignment, ListOffset,
    ListState, SharedString, Stateful, WeakEntity, Window, div, list, prelude::*, px,
};

use super::rich;
use super::widgets::{dot, event_icon, project_tile, tip};
use super::{DETAIL_MAX, HEADER_H, INNER_RADIUS, Rireki, Selection, fmt_dur, fmt_n, is_live, local, tilde};
use crate::icons::{self, Icon};
use crate::model::{Event, Kind, Session, Source, Status, Usage, UsageSum, one_line};

mod files;
pub(super) mod analysis;
pub(super) mod replay;
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

#[derive(Clone, Copy, PartialEq, Debug)]
enum DetailRow {
    Summary,
    Event(usize),
    Empty,
    Limit,
}

#[derive(Clone, PartialEq)]
struct DetailListKey {
    sel: Selection,
    generation: u64,
    filter: Option<RowFilter>,
    reversed: bool,
    width: u32,
    live: bool,
    replay_visible: Option<usize>,
}

struct DetailListData {
    key: DetailListKey,
    rows: Vec<DetailRow>,
}

/// 高さの違う行を可視範囲だけ描く。更新時は同じイベントを上端に保つ
pub(super) struct DetailList {
    pub(super) state: ListState,
    data: RefCell<Option<DetailListData>>,
    pub(super) rendered: Cell<usize>,
}

fn detail_rows(events: &[Event], filter: Option<&RowFilter>, reversed: bool) -> Vec<DetailRow> {
    let mut rows = vec![DetailRow::Summary];
    for (i, e) in events.iter().take(DETAIL_MAX).enumerate() {
        let keep = match filter {
            None => true,
            Some(RowFilter::Name(n)) => e.name.as_ref() == n,
            Some(RowFilter::Failed) => e.status.failed(),
        };
        if keep { rows.push(DetailRow::Event(i)); }
    }
    if reversed { rows[1..].reverse(); }
    if rows.len() == 1 { rows.push(DetailRow::Empty); }
    if events.len() > DETAIL_MAX { rows.push(DetailRow::Limit); }
    rows
}

fn detail_anchor(old: &[DetailRow], new: &[DetailRow], offset: ListOffset) -> ListOffset {
    old.get(offset.item_ix)
        .and_then(|row| new.iter().position(|next| next == row))
        .map(|item_ix| ListOffset { item_ix, ..offset })
        .unwrap_or_default()
}

fn replay_at_top(offset: ListOffset) -> bool {
    offset.item_ix == 0 && offset.offset_in_item <= px(4.0)
}

impl DetailList {
    pub(super) fn new() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(200.0)),
            data: RefCell::new(None),
            rendered: Cell::new(0),
        }
    }

    pub(super) fn reset(&self) {
        *self.data.borrow_mut() = None;
        self.rendered.set(0);
        self.state.reset(0);
    }

    fn prepare(&self, key: DetailListKey, events: &[Event]) {
        let mut data = self.data.borrow_mut();
        if data.as_ref().is_some_and(|d| d.key == key) { return; }
        // 再生が進んだときは追加された行だけを挿入し、既存行の高さと位置を保つ。
        if let Some(d) = data.as_mut()
            && let (Some(before), Some(after)) = (d.key.replay_visible, key.replay_visible)
            && after >= before
            && key == (DetailListKey { replay_visible: Some(after), ..d.key.clone() })
        {
            let mut added = replay::matching_rows(events, before, after, key.filter.as_ref());
            if key.reversed { added.reverse(); }
            if !added.is_empty() {
                if d.rows == [DetailRow::Empty] {
                    self.state.splice(0..1, added.len());
                    d.rows = added;
                } else if key.reversed {
                    let offset = self.state.logical_scroll_top();
                    let count = added.len();
                    self.state.splice(0..0, count);
                    d.rows.splice(0..0, added);
                    self.state.scroll_to(ListOffset { item_ix: offset.item_ix + count, ..offset });
                } else {
                    self.state.splice(d.rows.len()..d.rows.len(), added.len());
                    d.rows.extend(added);
                }
            }
            d.key = key;
            return;
        }
        let rows = match key.replay_visible {
            Some(visible) => replay::rows(events, visible, key.filter.as_ref(), key.reversed),
            None => detail_rows(events, key.filter.as_ref(), key.reversed),
        };
        let offset = data.as_ref().filter(|d| {
            d.key.sel.session == key.sel.session && d.key.sel.lane == key.sel.lane && d.key.sel.first == key.sel.first
        }).map(|d| detail_anchor(&d.rows, &rows, self.state.logical_scroll_top())).unwrap_or_default();
        self.state.splice(0..self.state.item_count(), rows.len());
        self.state.scroll_to(offset);
        *data = Some(DetailListData { key, rows });
    }

    fn at_bottom(&self) -> bool {
        let bounds = self.state.viewport_bounds();
        bounds.size.height > px(0.0) && self.state.item_count().checked_sub(1)
            .and_then(|i| self.state.bounds_for_item(i))
            .is_some_and(|last| last.bottom() <= bounds.bottom() + px(4.0))
    }

    fn at_latest(&self, reversed: bool) -> bool {
        if !reversed { return self.at_bottom(); }
        let offset = self.state.logical_scroll_top();
        if self.data.borrow().as_ref().is_some_and(|d| d.key.replay_visible.is_some()) {
            return self.state.viewport_bounds().size.height > px(0.0) && replay_at_top(offset);
        }
        self.state.viewport_bounds().size.height > px(0.0)
            && (offset.item_ix == 0 || (offset.item_ix == 1 && offset.offset_in_item <= px(4.0)))
    }

    fn scroll_to_latest(&self, reversed: bool) {
        self.state.scroll_to(if reversed {
            ListOffset::default()
        } else {
            ListOffset { item_ix: self.state.item_count(), offset_in_item: px(0.0) }
        });
    }
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

impl Rireki {
    pub(super) fn detail_profile_open_file(&mut self, cx: &mut Context<Self>) {
        self.file_open = self.detail_cache.as_ref().and_then(|d| d.files.first()).map(|f| f.path.clone());
        self.detail_changed(cx);
    }

    pub(super) fn detail_profile_scroll_files(&self, delta: gpui::Pixels, cx: &mut Context<Self>) {
        if self.profile_eager_files() {
            let h = &self.detail_scroll;
            h.set_offset(gpui::point(h.offset().x, (h.offset().y - delta).max(-h.max_offset().height)));
        } else {
            self.file_list.state.scroll_by(delta);
        }
        self.detail_changed(cx);
    }

    pub(super) fn detail_profile_reorder_files(&mut self, cx: &mut Context<Self>) {
        if let Some(cache) = &mut self.detail_cache && cache.files.len() > 1 {
            cache.files.swap(0, 1);
            let generation = self.detail_gen.fetch_add(1, super::Ordering::Relaxed) + 1;
            if let Some((_, shown)) = &mut self.detail_snapshot { *shown = generation; }
            self.detail_changed(cx);
        }
    }

    pub(super) fn detail_profile_open_file_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sel) = self.selected && let Some(event) = self.file_list.top_event() {
            self.open_edit(sel.session, sel.lane, sel.first + event, window, cx);
        }
    }

    pub(super) fn detail_profile_state(&self) -> serde_json::Value {
        let offset = self.detail_list.state.logical_scroll_top();
        let data = self.detail_list.data.borrow();
        let top_event = data.as_ref().and_then(|d| d.rows.get(offset.item_ix).map(|r| (d.key.sel, r)))
            .and_then(|(sel, row)| match row { DetailRow::Event(i) => Some(sel.first + i), _ => None });
        let opened = self.detail_cache.as_ref().and_then(|d| {
            let path = self.file_open.as_ref()?;
            let file = d.files.iter().find(|f| &f.path == path)?;
            let rows: usize = file.events.iter().filter_map(|i| d.patches.get(i))
                .flat_map(|ps| ps.iter().filter(|p| &p.path == path)).map(|p| p.rows.len()).sum();
            Some((file.events.len(), rows))
        });
        serde_json::json!({
            "list_count":self.detail_list.state.item_count(), "rendered":self.detail_list.rendered.get(),
            "item_ix":offset.item_ix, "offset_px":f32::from(offset.offset_in_item), "top_event":top_event,
            "at_bottom":self.detail_at_bottom(), "tab":format!("{:?}", self.detail_tab),
            "following":self.detail_following, "follow_pending":self.detail_follow,
            "files":self.detail_cache.as_ref().map(|d| d.files.len()),
            "summary":self.detail_cache.as_ref().map(|d| serde_json::json!({
                "selection_tools":d.selection_summary.total.stats.tools,
                "selection_turns":d.selection_summary.total.stats.turns,
                "selection_tokens":d.selection_summary.total.usage.total(),
                "session_tools":d.session_summary.total.stats.tools,
                "session_turns":d.session_summary.total.stats.turns,
                "session_tokens":d.session_summary.total.usage.total(),
                "models":d.session_summary.models.len(), "lanes":d.session_summary.lanes.len(),
            })),
            "file_list_count":self.file_list.state.item_count(), "file_rendered":self.file_list.rendered.get(),
            "file_item_ix":self.file_list.state.logical_scroll_top().item_ix,
            "file_offset_px":f32::from(self.file_list.state.logical_scroll_top().offset_in_item),
            "file_top_event":self.file_list.top_event(),
            "file_open":self.file_open.is_some(), "open_edits_rows":opened,
            "expanded_in":self.expanded.contains("in"), "expanded_out":self.expanded.contains("out"),
            "expanded_diff":self.expanded.contains("file-diff"),
            "text_chars":self.detail_cache.as_ref().and_then(|d| d.texts.first()).map(|(a,b)| (a.chars().count(),b.chars().count())),
            "selected_count":self.selected.map(|s| s.last - s.first + 1),
            "selected_kind":self.selected.filter(|&s| self.valid(s)).map(|s| format!("{:?}", self.store.sessions[s.session].lanes[s.lane].events[s.first].kind)),
            "row_filter":match &self.row_filter { None => "none", Some(RowFilter::Failed) => "failed", Some(RowFilter::Name(_)) => "name" },
        })
    }

    pub(super) fn detail_profile_click(&self) -> Option<(gpui::Point<gpui::Pixels>, usize)> {
        let list = &self.detail_list.state;
        let viewport = list.viewport_bounds();
        let data = self.detail_list.data.borrow();
        let data = data.as_ref()?;
        data.rows.iter().enumerate().skip(list.logical_scroll_top().item_ix).find_map(|(row, kind)| {
            let DetailRow::Event(i) = kind else { return None; };
            let bounds = list.bounds_for_item(row)?;
            let center = bounds.center();
            (center.y >= viewport.top() && center.y < viewport.bottom()).then_some((center, data.key.sel.first + i))
        })
    }

    pub(super) fn detail_uses_list(&self) -> bool {
        self.shown_detail_selection().filter(|&sel| sel.first < sel.last)
            .is_some_and(|sel| self.detail_tab == DetailTab::Events || (self.detail_tab == DetailTab::Files && self.cache(sel).is_none_or(|cache| cache.files.is_empty())))
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
    pub(super) fn animate_list_to_bottom(&mut self, cx: &mut Context<Self>) {
        const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
        const MAX_FRAMES: usize = 40;
        self.detail_following = true;
        self.follow_anim = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FRAME).await;
            for frame in 0..MAX_FRAMES {
                let Ok(keep_going) = this.update(cx, |app, cx| {
                    if !app.detail_uses_list() || app.detail_list.at_bottom() { return None; }
                    let list = &app.detail_list.state;
                    if frame + 1 == MAX_FRAMES {
                        list.scroll_to(ListOffset { item_ix: list.item_count(), offset_in_item: px(0.0) });
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

    pub(super) fn cancel_detail_load(&mut self) {
        self.reset_replay();
        self.detail_gen.fetch_add(1, super::Ordering::Relaxed);
        self.detail_task = None;
        self.cancel_detail_follow();
    }

    /// 大きな編集履歴と旧版セッションの解放で、選択や閉じる操作を待たせない。
    pub(super) fn clear_detail_cache(&mut self, cx: &mut Context<Self>) {
        let old = (self.detail_cache.take(), self.detail_snapshot.take());
        if old.0.is_some() || old.1.is_some() {
            cx.background_executor().spawn(async move { drop(old); }).detach();
        }
    }

    /// DB 読み込みと JS のモック実行を背景で行い、最新の選択だけを反映する
    pub(super) fn request_detail(&mut self, sel: Selection, cx: &mut Context<Self>) {
        self.clear_detail_cache(cx);
        self.refresh_detail(sel, cx);
    }

    /// 同じ選択の更新では表示中の詳細を保持し、セッションの版ごと差し替える
    pub(super) fn refresh_detail(&mut self, sel: Selection, cx: &mut Context<Self>) {
        self.cancel_detail_load();
        self.detail_changed(cx);
        let generation = self.detail_gen.load(super::Ordering::Relaxed);
        let gen_counter = self.detail_gen.clone();
        let loader = self.detail_loader.clone();
        let session = self.store.sessions[sel.session].clone();
        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx.background_executor().spawn(async move {
                let is_current = || gen_counter.load(super::Ordering::Relaxed) == generation;
                if !is_current() { return None; }
                let mut loader = loader.lock().unwrap();
                if !is_current() { return None; }
                loader.prepare();
                loader.load_session(sel, &session, is_current).map(|cache| (cache, session))
            }).await;
            let Some((cache, session)) = loaded else { return };
            let _ = this.update(cx, |app, cx| {
                if app.detail_gen.load(super::Ordering::Relaxed) == generation && app.selected == Some(sel) && app.valid(sel) {
                    let old_cache = app.detail_cache.replace(cache);
                    let old_snapshot = app.detail_snapshot.replace((session, generation));
                    cx.background_executor().spawn(async move { drop((old_cache, old_snapshot)); }).detach();
                    app.request_analysis(cx);
                    if app.replay_autostart.take() == Some(sel) && app.detail_tab == DetailTab::Events && !app.statistics_open {
                        app.begin_replay(sel, cx);
                    }
                    app.detail_changed(cx);
                    if std::mem::take(&mut app.detail_follow) && app.detail_tab == DetailTab::Events {
                        app.animate_to_latest(cx);
                    }
                }
            });
        }));
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
            cx.notify();
        }
        self.reveal(sel, false, window);
    }

    pub(super) fn detail(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let panel = div()
            .id("detail-panel")
            .size_full()
            .flex()
            .flex_col()
            .when(self.detail_fullscreen, |d| d.rounded(px(INNER_RADIUS)))
            .when(!self.detail_fullscreen, |d| d.rounded_r(px(INNER_RADIUS)).border_l_1())
            .bg(c(theme::PANEL))
            .border_color(c(theme::BORDER))
            .text_sm()
            .text_color(c(theme::TEXT))
            .on_scroll_wheel(cx.listener(|app, _: &gpui::ScrollWheelEvent, _, _| app.cancel_detail_follow()));
        let Some(sel) = self.shown_detail_selection() else {
            return panel.into_any_element();
        };
        let files = self.cache(sel).map_or(0, |d| d.files.len());
        let tab = match self.detail_tab {
            DetailTab::Files if files == 0 => DetailTab::Events,
            t => t,
        };
        let mut body = div()
            .id("detail-scroll")
            .track_scroll(&self.detail_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_3();
        body.style().restrict_scroll_to_axis = Some(true);
        match tab {
            DetailTab::Events => {
                if self.cache(sel).is_none() {
                    body = body.child(div().text_xs().text_color(c(theme::MUTED)).child("詳細を読み込み中…"));
                } else if sel.first == sel.last {
                    body = body.child(self.replay_controls(sel, cx)).child(self.event_view(sel, cx));
                } else {
                    return panel
                        .child(self.detail_header(sel, cx))
                        .child(self.context_bar(sel, cx))
                        .child(self.detail_tabs(sel, tab, files, cx))
                        .child(
                            div().flex_1().min_h_0().p_3().flex().flex_col().gap_2()
                                .when(self.replay.is_some(), |d| d.child(self.event_list_header(sel, cx)))
                                .child(self.event_list(sel, window, cx)),
                        )
                        .into_any_element();
                }
            }
            DetailTab::Files => {
                if let Some(d) = self.cache(sel) {
                    return panel
                        .child(self.detail_header(sel, cx))
                        .child(self.context_bar(sel, cx))
                        .child(self.detail_tabs(sel, tab, files, cx))
                        .child(div().flex_1().min_h_0().p_3().flex().flex_col().child(self.files_list(sel, d, window, cx)))
                        .into_any_element();
                }
            }
            DetailTab::Session => {
                if self.cache(sel).is_some() {
                    body = body.child(self.session_card(sel, cx));
                    if self.detail_session(sel).lanes.len() > 1 {
                        body = body.child(self.subagent_list(sel, cx));
                    }
                } else {
                    body = body.child(div().text_xs().text_color(c(theme::MUTED)).child("詳細を読み込み中…"));
                }
            }
            DetailTab::Analysis => {
                return panel
                    .child(self.detail_header(sel, cx))
                    .child(self.context_bar(sel, cx))
                    .child(self.detail_tabs(sel, tab, files, cx))
                    .child(self.analysis_view(sel, cx))
                    .into_any_element();
            }
        }
        panel
            .child(self.detail_header(sel, cx))
            .child(self.context_bar(sel, cx))
            .child(self.detail_tabs(sel, tab, files, cx))
            .child(body)
            .into_any_element()
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
                .child(self.tab_button(DetailTab::Events, "イベント", n_events, "1", tab, true, cx))
                .child(self.tab_button(DetailTab::Files, "ファイル", Some(files), "2", tab, files > 0, cx))
                .child(self.tab_button(DetailTab::Session, "セッション", None, "3", tab, true, cx))
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
            self.detail_tab = tab;
            self.detail_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            self.detail_list.state.scroll_to(ListOffset::default());
            self.file_list.state.scroll_to(ListOffset::default());
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

    /// 見出し。イベント名と状態、前後のイベント・選択の履歴の移動、閉じる
    fn detail_header(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[sel.lane];
        let evs = &lane.events[sel.first..=sel.last];
        let single = evs.len() == 1;
        let (title, color) = if single {
            (evs[0].name.to_string(), Some((theme::event_color(&evs[0]), theme::event_icon(&evs[0]))))
        } else {
            (format!("{} 件", fmt_n(evs.len())), None)
        };
        let output = self.cache(sel).and_then(|d| d.texts.first()).map_or("", |t| t.1.as_str());
        let status = if single { status_label(&evs[0], is_live(s.end), output) } else { None };
        let n = lane.events.len();
        let (prev, next) = if single { (sel.first > 0, sel.first + 1 < n) } else { (true, true) };
        let (prev, next) = if self.detail_reversed { (next, prev) } else { (prev, next) };
        let (tile_col, tile_icon) = color.unwrap_or((theme::MUTED, Icon::Layers));
        let pos = if single {
            format!("{} / {}", fmt_n(sel.first + 1), fmt_n(n))
        } else {
            format!("{}–{} / {}", fmt_n(sel.first + 1), fmt_n(sel.last + 1), fmt_n(n))
        };
        div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pl_2()
            .pr_2()
            .border_b_1()
            .border_color(c(theme::BORDER))
            // 選択の履歴（ブラウザの戻る・進むと同じ位置）
            .child(
                div()
                    .flex_none()
                    .flex()
                    .child(self.ghost_button(
                        "d-back",
                        Icon::ArrowLeft,
                        "戻る",
                        Some("⌘["),
                        !self.back.is_empty(),
                        cx,
                        |t, w, cx| t.go_back(w, cx),
                    ))
                    .child(self.ghost_button(
                        "d-forward",
                        Icon::ArrowRight,
                        "進む",
                        Some("⌘]"),
                        !self.forward.is_empty(),
                        cx,
                        |t, w, cx| t.go_forward(w, cx),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .size(px(26.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(c(tile_col).opacity(0.16))
                            .border_1()
                            .border_color(c(tile_col).opacity(0.35))
                            .child(event_icon(tile_icon, tile_col, 14.0)),
                    )
                    .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when_some(status, |d, (l, col)| d.child(badge(l, col))),
            )
            // レーン内の位置と前後の移動をひとまとまりにする
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .bg(c(theme::BG))
                    .child(
                        self.ghost_button(
                            "d-prev",
                            Icon::ChevronUp,
                            "前のイベント",
                            Some("K"),
                            prev,
                            cx,
                            |t, w, cx| t.step_event(-1, w, cx),
                        )
                        .size(px(22.0)),
                    )
                    .child(
                        div()
                            .id("d-pos")
                            .px_1()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(pos)
                            .tooltip(tip("レーン内の位置", None)),
                    )
                    .child(
                        self.ghost_button(
                            "d-next",
                            Icon::ChevronDown,
                            "次のイベント",
                            Some("J"),
                            next,
                            cx,
                            |t, w, cx| t.step_event(1, w, cx),
                        )
                        .size(px(22.0)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .child(self.ghost_button(
                        "d-locate",
                        Icon::Locate,
                        "選択位置を表示",
                        None,
                        true,
                        cx,
                        |t, w, _| {
                            t.detail_fullscreen = false;
                            if let Some(sel) = t.selected {
                                t.reveal(sel, true, w);
                            }
                        },
                    ))
                    .child(self.ghost_button(
                        "d-fullscreen", if self.detail_fullscreen { Icon::Minimize } else { Icon::Maximize },
                        if self.detail_fullscreen { "通常のパネルに戻す" } else { "詳細を全画面表示" },
                        Some("⇧⌘↵"), true, cx, |t, window, cx| t.toggle_detail_fullscreen(window, cx),
                    ))
                    .child(self.ghost_button("d-close", Icon::X, "閉じる", None, true, cx, |t, window, cx| {
                        t.close_detail(cx);
                        window.focus(&t.focus_handle);
                    })),
            )
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
                    .truncate()
                    .text_color(c(theme::TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|d| d.underline())
                    .child(title)
                    .tooltip(tip("セッションの詳細", Some("3")))
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
                                if let Some(i) =
                                    s.lanes[0].events.iter().position(|e| e.call_id.as_deref() == Some(id.as_str()))
                                {
                                    let sel = Selection { session, lane: 0, first: i, last: i };
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
    fn open_lane_filtered(
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

    /// セッションの詳細。概要の数字 → 操作 → トークン → 属性の順に、よく見るものを上に置く
    fn session_card(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[sel.lane];
        let summary = &self.cache(sel).unwrap().session_summary;
        let usage = summary.total.usage;
        let title = if s.title.is_empty() { "(無題)".to_string() } else { s.title.clone() };
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
            (true, false) => Some(format!("エフォート {}", lane.effort)),
            (false, false) => Some(format!("{} · エフォート {}", lane.model, lane.effort)),
        };
        let version = (!s.version.is_empty()).then(|| match s.source {
            Source::Claude => format!("Claude Code {}", s.version),
            Source::Codex => format!("Codex CLI {}", s.version),
        });
        let st = summary.total.stats;
        let session = sel.session;
        let mut tiles = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(stat("期間", fmt_dur(s.end - s.start), None))
            .child(stat("ターン", fmt_n(st.turns), None))
            .child(stat("ツール", fmt_n(st.tools), None));
        if st.failures() > 0 {
            tiles = tiles.child(
                stat("失敗", fmt_n(st.failures()), Some(theme::ERR))
                    .id("tile-failed")
                    .cursor_pointer()
                    .hover(|d| d.border_color(c(theme::ERR)))
                    .tooltip(tip("メインの失敗したイベントを一覧", None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        t.open_lane_filtered(session, 0, Some(RowFilter::Failed), w, cx)
                    }))
                    .into_any_element(),
            );
        } else {
            tiles = tiles.child(stat("失敗", "0".into(), None));
        }
        if usage.calls > usage.unpriced {
            tiles = tiles.child(stat("料金", fmt_usd(usage.cost), Some(theme::OK)));
        }
        if st.peak > 0 {
            tiles = tiles.child(stat("最大コンテキスト", fmt_tokens(st.peak), None));
        }
        let models = &summary.models;
        let lane_usage = (s.lanes.len() > 1).then(|| summary.lanes[sel.lane].usage).filter(|u| u.calls > 0);
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
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_xs().text_color(c(theme::MUTED)).child(format!(
                        "{} – {}",
                        local(s.start).format("%Y/%m/%d %H:%M"),
                        local(s.end).format(if local(s.start).date_naive() == local(s.end).date_naive() {
                            "%H:%M"
                        } else {
                            "%m/%d %H:%M"
                        }),
                    ))),
            )
            .child(tiles)
            .child(actions)
            .when(usage.calls > 0, |d| {
                d.child(
                    section("トークン")
                        .child(div().text_xs().child(usage_view(&usage)))
                        .when_some(lane_usage, |d, u| {
                            d.child(div().text_xs().text_color(c(theme::SUBTLE)).child("このレーン"))
                                .child(usage_view(&u))
                        })
                        .when(models.len() > 1 || models.first().is_some_and(|m| m.1.unpriced > 0), |d| {
                            d.child(div().text_xs().child(model_table(models)))
                        }),
                )
            })
            .child(
                section("情報").child(
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
                        .child(meta(
                            "セッション",
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().min_w_0().truncate().font_family(rich::MONO).child(s.id.clone()))
                                .child(self.copy_button("copy-id", s.id.clone(), cx).flex_none())
                                .into_any_element(),
                        ))
                        .when_some(version, |d, v| d.child(meta("バージョン", div().child(v).into_any_element())))
                        .when(s.headless, |d| d.child(meta("起動", div().child("非対話").into_any_element()))),
                ),
            )
    }

    /// サブエージェントの一覧。押すとそのレーン全体を選ぶ
    fn subagent_list(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let mut list = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .pt_2()
            .border_t_1()
            .border_color(c(theme::BORDER))
            .child(div().pb_1().child(label(format!("サブエージェント {}", s.lanes.len() - 1))));
        let summary = &self.cache(sel).unwrap().session_summary;
        for (li, l) in s.lanes.iter().enumerate().skip(1) {
            let st = summary.lanes[li].stats;
            let (a, b) = l.range().unwrap_or((0.0, 0.0));
            let session = sel.session;
            let on = sel.lane == li;
            list = list.child(
                div()
                    .id(ElementId::NamedInteger("sub".into(), li as u64))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_1p5()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .when(on, |d| d.bg(c(theme::ACCENT_BG)))
                    .hover(|d| d.bg(c(theme::HOVER)))
                    .child(icons::icon(Icon::Bot, theme::SUB).size_3().flex_none())
                    .child(div().flex_1().min_w_0().truncate().child(l.name.clone()))
                    .when(st.failures() > 0, |d| d.child(badge(format!("失敗 {}", st.failures()), theme::ERR)))
                    .child(div().flex_none().text_color(c(theme::SUBTLE)).child(format!(
                        "{} 件 · {}",
                        fmt_n(l.events.len()),
                        fmt_dur(b - a)
                    )))
                    .on_click(
                        cx.listener(move |t, _: &ClickEvent, w, cx| t.open_lane_filtered(session, li, None, w, cx)),
                    ),
            );
        }
        list
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

    /// 1 イベントの詳細。時刻・トークン数・きっかけの指示と、入力・結果
    fn event_view(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[sel.lane];
        let e = &lane.events[sel.first];
        let cache = self.cache(sel);
        let (input, output) = cache.and_then(|d| d.texts.first()).map_or(("", ""), |t| (t.0.as_str(), t.1.as_str()));
        let raw = cache.and_then(|d| d.raws.first()).and_then(|r| r.as_deref());
        let mut col = div().flex().flex_col().gap_3().min_w_0();

        // 時刻・所要時間・トークン数（位置は見出しに出す）
        let when = if e.end > e.start {
            format!("{} → {}", local(e.start).format("%m/%d %H:%M:%S"), local(e.end).format("%H:%M:%S"))
        } else {
            local(e.start).format("%m/%d %H:%M:%S").to_string()
        };
        col = col.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x_3()
                .gap_y_0p5()
                .text_xs()
                .text_color(c(theme::MUTED))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icons::icon(Icon::Clock, theme::SUBTLE).size_3())
                        .child(when),
                )
                .when(e.end > e.start, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(icons::icon(Icon::Hourglass, theme::SUBTLE).size_3())
                            .child(fmt_dur(e.end - e.start)),
                    )
                })
                .when(!e.usage.is_empty(), |d| d.child(usage_chip("usage-ev", &e.usage, e.model.as_deref()))),
        );

        // きっかけになったユーザーの指示
        let turns = &self.cache(sel).unwrap().session_summary.turns[sel.lane];
        let previous_turn = turns.partition_point(|turn| turn.index < sel.first).checked_sub(1)
            .map(|index| turns[index].index);
        if e.kind != Kind::User
            && let Some(ui) = previous_turn
        {
            let u = &lane.events[ui];
            let (session, lane_i) = (sel.session, sel.lane);
            col = col.child(
                div()
                    .id("turn-prompt")
                    .flex()
                    .items_start()
                    .gap_1p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(c(theme::event_color(u)).opacity(0.07))
                    .border_l_2()
                    .border_color(c(theme::event_color(u)).opacity(0.7))
                    .text_xs()
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::event_color(u)).opacity(0.12)))
                    .child(div().flex_none().text_color(c(theme::MUTED)).child("指示"))
                    .child(div().flex_1().min_w_0().line_clamp(2).child(one_line(&u.summary, 200)))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(local(u.start).format("%H:%M").to_string()),
                    )
                    .tooltip(tip("このイベントのきっかけになった指示を開く", None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        let sel = Selection { session, lane: lane_i, first: ui, last: ui };
                        t.navigate(sel, w, cx);
                        t.reveal(sel, false, w);
                    })),
            );
        }

        match e.kind {
            Kind::User | Kind::Assistant | Kind::Thinking => {
                let color = if e.kind == Kind::Thinking { theme::MUTED } else { theme::TEXT };
                col = col.child(self.titled(
                    None,
                    "copy-in",
                    input,
                    rich::markdown(input, color).text_size(px(13.0)).into_any_element(),
                    cx,
                ));
            }
            Kind::Tool | Kind::Agent => {
                col = col.child(self.tool_input(e, input, raw, &s.cwd, cx));
                if e.kind == Kind::Agent
                    && let Some(li) = e.call_id.as_deref().and_then(|id| s.lane_spawned_by(id))
                {
                    col = col.child(self.subagent_button(sel, li, cx));
                }
                let lines = cache.and_then(|d| d.out_lines.first()).copied().unwrap_or(0);
                col = col.child(self.tool_output(e, output, lines, is_live(s.end), &s.cwd, cx));
            }
        }
        col
    }

    /// 見出しとコピーボタンを付けた本文
    fn titled(
        &self,
        head: Option<Div>,
        copy_id: &'static str,
        copy: &str,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_h(px(22.0))
                    .child(head.unwrap_or_else(div))
                    .child(div().flex_1())
                    .when(!copy.is_empty(), |d| d.child(self.copy_button(copy_id, copy.to_string(), cx).flex_none())),
            )
            .child(div().id(SharedString::from(format!("{copy_id}-body"))).min_w_0().child(body))
    }

    /// 「残り N 行を表示」。押すと `id` の本文を全部出す
    fn expand_button(&self, id: &'static str, hidden: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("more-{id}")))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .text_xs()
            .text_color(c(theme::ACCENT))
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(Icon::ChevronDown, theme::ACCENT).size_3())
            .child(format!("残り {} 行を表示", fmt_n(hidden)))
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                t.expanded.insert(id);
                t.detail_changed(cx);
            }))
    }

    /// 長い本文を `FOLD_LINES` 行で畳んだ等幅の箱
    fn folded_mono(&self, id: &'static str, text: &str, flow: rich::TextFlow, cx: &mut Context<Self>) -> Div {
        let (shown, hidden) = if self.expanded.contains(id) { (text, 0) } else { rich::head_lines(text, FOLD_LINES) };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(rich::mono_box(id, shown, flow))
            .when(hidden > 0, |d| d.child(self.expand_button(id, hidden, cx)))
    }

    /// ツールの入力。差分・コマンド・Markdown（計画やサブエージェントへの指示）・引数で見せ方を変える
    fn tool_input(&self, e: &Event, input: &str, raw: Option<&str>, cwd: &str, cx: &mut Context<Self>) -> Div {
        // 要約が入力に含まれていなければ（コマンドの説明など）見出しの下に添える。
        // exec の要約は呼び出しを並べたものなので、整形した入力と重なる
        let key = e.summary.trim_end_matches('…');
        let caption =
            (raw.is_none() && !key.is_empty() && !one_line(input, usize::MAX).contains(key)).then(|| e.summary.clone());
        let markdown = matches!(e.name.as_ref(), "ExitPlanMode" | "Agent" | "Task");
        let (title, body): (&str, AnyElement) = if let Some(raw) = raw {
            // exec は整形した呼び出しと JS 本文を切り替える
            let show = self.show_raw;
            let toggle =
                self.segmented("rawmode", &[(false, "整形"), (true, "JS")], Some(show), cx, |t, v, _, cx| {
                    t.show_raw = v;
                    t.detail_changed(cx);
                });
            let text = if show { raw } else { input };
            let body = div().flex().flex_col().gap_1p5().min_w_0()
                .child(toggle).child(self.folded_mono("in", text, rich::TextFlow::Scroll, cx));
            ("入力", body.into_any_element())
        } else if rich::is_patch(input) {
            let files = rich::parse_patch(input);
            let limit = if self.expanded.contains("in") { usize::MAX } else { FOLD_DIFF };
            let (view, hidden) = rich::diff_view(&files, cwd, limit);
            let body = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(view)
                .when(hidden > 0, |d| d.child(self.expand_button("in", hidden, cx)));
            ("変更", body.into_any_element())
        } else if markdown {
            ("内容", rich::markdown(input, theme::TEXT).text_size(px(13.0)).into_any_element())
        } else if input.starts_with("$ ") {
            ("コマンド", self.folded_mono("in", input, rich::TextFlow::Scroll, cx).into_any_element())
        } else {
            ("入力", self.folded_mono("in", input, rich::TextFlow::Wrap, cx).into_any_element())
        };
        let copy = if self.show_raw { raw.unwrap_or(input) } else { input };
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .min_w_0()
            .when_some(caption, |d, cap| d.child(div().text_sm().child(SharedString::from(cap))))
            .child(self.titled(Some(label(title)), "copy-in", copy, body, cx))
    }

    /// ツールの結果。失敗なら枠を赤くし、内容に合わせて Markdown・行番号付き・差分・ANSI の色で出す
    fn tool_output(&self, e: &Event, output: &str, lines: usize, live: bool, cwd: &str, cx: &mut Context<Self>) -> Div {
        let status = status_label(e, live, output);
        let head = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(label("結果"))
            .when(lines > 1, |d| {
                d.child(div().text_xs().text_color(c(theme::SUBTLE)).child(format!("{} 行", fmt_n(lines))))
            })
            .when_some(status.clone(), |d, (l, col)| d.child(badge(l, col)));
        let body: AnyElement = if output.is_empty() {
            let note = match e.status {
                Status::Pending if live => "結果はまだありません",
                Status::Pending => "結果は記録されていません",
                _ => "出力なし",
            };
            div().text_xs().text_color(c(theme::SUBTLE)).child(note).into_any_element()
        } else if MARKDOWN_OUTPUT.contains(&e.name.as_ref()) {
            rich::markdown(output, theme::TEXT).text_size(px(13.0)).into_any_element()
        } else if rich::is_patch(output) {
            let files = rich::parse_patch(output);
            let limit = if self.expanded.contains("out") { usize::MAX } else { FOLD_DIFF };
            let (view, hidden) = rich::diff_view(&files, cwd, limit);
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(view)
                .when(hidden > 0, |d| d.child(self.expand_button("out", hidden, cx)))
                .into_any_element()
        } else if let Some(rows) = rich::numbered(output) {
            let all = self.expanded.contains("out");
            let hidden = if all { 0 } else { rows.len().saturating_sub(FOLD_LINES) };
            let shown = if all { &rows[..] } else { &rows[..rows.len().min(FOLD_LINES)] };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(rich::numbered_view(shown))
                .when(hidden > 0, |d| d.child(self.expand_button("out", hidden, cx)))
                .into_any_element()
        } else {
            self.folded_mono("out", output, rich::TextFlow::Scroll, cx).into_any_element()
        };
        let failed = e.status.failed();
        self.titled(Some(head), "copy-out", output, body, cx).when(failed && !output.is_empty(), |d| {
            d.pl_2().border_l_2().border_color(c(status.map_or(theme::ERR, |s| s.1)).opacity(0.6))
        })
    }

    /// サブエージェントのレーンを開くボタン（件数・期間・失敗を添える）
    fn subagent_button(&self, sel: Selection, li: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let session = sel.session;
        let l = &self.detail_session(sel).lanes[li];
        let n = l.events.len();
        let (a, b) = l.range().unwrap_or((0.0, 0.0));
        let st = self.detail_cache.as_ref().unwrap().session_summary.lanes[li].stats;
        let mut text = format!("サブエージェントを開く · {} 件 · {}", fmt_n(n), fmt_dur(b - a));
        if st.failures() > 0 {
            text.push_str(&format!(" · 失敗 {}", st.failures()));
        }
        self.icon_button(
            SharedString::from(format!("to-sub-{li}")),
            Some(Icon::Bot),
            text,
            false,
            cx,
            move |t, w, cx| {
                let sel = Selection { session, lane: li, first: 0, last: n - 1 };
                if !t.valid(sel) { return; }
                t.navigate(sel, w, cx);
                t.reveal(sel, true, w);
            },
        )
    }

    // ------------------------------------------------------------ 複数

    /// 選択範囲の期間・件数・集計と、絞り込み（名前・失敗）、編集したファイル
    fn selection_summary(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        let (start, end) = (evs[0].start, self.cache(sel).unwrap().end());
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

        let mut tiles = div().flex().flex_wrap().gap_2().child(stat("期間", fmt_dur(end - start), None)).child(stat(
            "件数",
            fmt_n(evs.len()),
            None,
        ));
        if st.turns > 0 {
            tiles = tiles.child(stat("指示", fmt_n(st.turns), None));
        }
        tiles = tiles.child(stat("ツール", fmt_n(st.tools), None));
        if st.failures() > 0 {
            let on = filter == Some(RowFilter::Failed);
            tiles = tiles.child(
                stat("失敗", fmt_n(st.failures()), Some(theme::ERR))
                    .id("tile-failed")
                    .cursor_pointer()
                    .when(on, |d| d.border_color(c(theme::ERR)).bg(c(theme::ERR).opacity(0.08)))
                    .hover(|d| d.border_color(c(theme::ERR)))
                    .tooltip(tip(if on { "絞り込みを外す" } else { "失敗したイベントだけを表示" }, None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                        t.set_row_filter(if on { None } else { Some(RowFilter::Failed) }, cx);
                    }))
                    .into_any_element(),
            );
        }
        let usage = summary.total.usage;
        let col = div()
            .flex()
            .flex_col()
            .gap_2()
            .text_xs()
            .child(div().text_color(c(theme::MUTED)).child(format!(
                "{} – {}",
                local(start).format("%m/%d %H:%M:%S"),
                local(end).format(if local(start).date_naive() == local(end).date_naive() {
                    "%H:%M:%S"
                } else {
                    "%m/%d %H:%M:%S"
                }),
            )))
            .child(tiles)
            .when(usage.calls > 0, |d| d.child(usage_view(&usage)))
            .child(chips);
        col.pb_2().border_b_1().border_color(c(theme::BORDER))
    }

    /// 選択の中で編集したファイル。押すとその場で開き、編集の時系列と、編集ごとの差分を出す
    fn files_list(&self, sel: Selection, cache: &DetailCache, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        self.file_list.prepare(FileListKey {
            sel,
            generation: self.shown_detail_generation(),
            width: self.detail_content_width(window).to_bits(),
            open: self.file_open.clone(),
            all_files: self.expanded.contains("files"),
            all_diffs: self.expanded.contains("file-diff"),
        }, cache);
        self.file_list.rendered.set(0);
        if self.profile_eager_files() {
            return div().id("profile-eager-files").track_scroll(&self.detail_scroll)
                .size_full().overflow_y_scroll().flex().flex_col()
                .children((0..self.file_list.state.item_count()).map(|i| div().flex_none().min_w_0().child(self.file_list_item(sel, i, cx))))
                .into_any_element();
        }
        let app = cx.entity().downgrade();
        gpui::list(self.file_list.state.clone(), move |index, _, cx| {
            app.update(cx, |app, cx| app.file_list_item(sel, index, cx)).unwrap_or_else(|_| div().into_any_element())
        }).size_full().into_any_element()
    }

    fn file_list_item(&self, sel: Selection, index: usize, cx: &mut Context<Self>) -> AnyElement {
        self.file_list.rendered.set(self.file_list.rendered.get() + 1);
        let Some(row) = self.file_list.row(index) else { return div().into_any_element(); };
        let Some(cache) = self.cache(sel) else { return div().into_any_element(); };
        let cwd = &self.detail_session(sel).cwd;
        match row {
            FileRow::Summary | FileRow::File(_) | FileRow::More => self.file_row_view(cache, cwd, row, cx),
            FileRow::History(file) | FileRow::Edit { file, .. } | FileRow::Empty(file) | FileRow::Expand(file) => {
                let Some(file) = cache.files.get(file) else { return div().into_any_element(); };
                self.file_history(sel, cache, file, cwd, row, cx)
            }
        }.into_any_element()
    }

    fn file_row_view(&self, d: &DetailCache, cwd: &str, row: FileRow, cx: &mut Context<Self>) -> Div {
        let all = self.expanded.contains("files");
        let (added, removed) = d.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        let mut list = div().flex().flex_col().gap_1().when(row == FileRow::Summary, |list| list.child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .pb_1()
                .child(label(format!("編集したファイル {}", d.files.len())))
                .child(div().text_xs().text_color(c(theme::OK)).child(format!("+{}", fmt_n(added))))
                .child(div().text_xs().text_color(c(theme::ERR)).child(format!("−{}", fmt_n(removed))))
                .child(div().flex_1())
                .child(div().text_xs().text_color(c(theme::SUBTLE)).child("押すと差分を表示")),
        ));
        for (i, f) in d.files.iter().enumerate().take(if all { usize::MAX } else { FILE_MAX }) {
            if row != FileRow::File(i) { continue; }
            let open = self.file_open.as_deref() == Some(f.path.as_str());
            let path = rich::rel_path(&f.path, cwd);
            let (dir, base) = match path.rsplit_once('/') {
                Some((d, b)) => (format!("{d}/"), b.to_string()),
                None => (String::new(), path.to_string()),
            };
            let key = f.path.clone();
            let row = div()
                .id(ElementId::NamedInteger("file".into(), i as u64))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(open, |d| d.bg(c(theme::ACCENT_BG)))
                .when(!open, |d| d.hover(|s| s.bg(c(theme::HOVER))))
                .child(
                    icons::icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::SUBTLE)
                        .size_3()
                        .flex_none(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .child(div().min_w_0().truncate().text_color(c(theme::SUBTLE)).child(dir))
                        .child(
                            div()
                                .flex_none()
                                .max_w_full()
                                .truncate()
                                .text_color(c(theme::TEXT))
                                .when(open, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(base),
                        ),
                )
                .child(div().flex_none().text_color(c(theme::SUBTLE)).child(format!("×{}", f.edits)))
                .child(div().flex_none().text_color(c(theme::OK)).child(format!("+{}", f.added)))
                .child(div().flex_none().text_color(c(theme::ERR)).child(format!("−{}", f.removed)))
                .child(diff_bar(f.added, f.removed))
                .tooltip(tip(f.path.clone(), None))
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    t.file_open = if t.file_open.as_deref() == Some(key.as_str()) { None } else { Some(key.clone()) };
                    t.expanded.remove("file-diff");
                    t.detail_changed(cx);
                }));
            list = list.child(row);
        }
        if row == FileRow::More && !all && d.files.len() > FILE_MAX {
            list = list.child(
                div()
                    .id("more-files")
                    .px_1p5()
                    .text_xs()
                    .text_color(c(theme::ACCENT))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(format!("ほか {} ファイル", d.files.len() - FILE_MAX))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        t.expanded.insert("files");
                        t.detail_changed(cx);
                    })),
            );
        }
        list.pb_1()
    }

    /// 1 イベントを選んでイベントのタブを開く
    fn open_edit(&mut self, session: usize, lane: usize, idx: usize, w: &mut Window, cx: &mut Context<Self>) {
        let sel = Selection { session, lane, first: idx, last: idx };
        self.navigate(sel, w, cx);
        self.set_detail_tab(DetailTab::Events, cx);
        self.reveal(sel, false, w);
    }

    /// 開いたファイルの編集の履歴。選択の期間の上に編集を印で置いた帯と、編集ごとの差分
    fn file_history(&self, sel: Selection, d: &DetailCache, f: &FileStat, cwd: &str, row: FileRow, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        let (start, end) = (evs[0].start, d.end());
        let span = (end - start).max(1.0);
        // 各編集でこのファイルに加えた変更
        let edits: Vec<(usize, Vec<&rich::FilePatch>)> = f
            .events
            .iter()
            .filter_map(|&i| {
                let ps: Vec<&rich::FilePatch> =
                    d.patches.get(&i)?.iter().filter(|p| p.path == f.path).collect();
                (!ps.is_empty()).then_some((i, ps))
            })
            .collect();
        let max_lines =
            edits.iter().map(|(_, ps)| ps.iter().map(|p| p.added + p.removed).sum::<usize>()).max().unwrap_or(1).max(1);
        let (session, lane) = (sel.session, sel.lane);

        // 編集の帯: 横は時刻、印の高さは変更行数、色は追加と削除の割合
        let fmt = if local(start).date_naive() == local(end).date_naive() { "%H:%M" } else { "%m/%d %H:%M" };
        let mut strip = div()
            .relative()
            .h(px(34.0))
            .w_full()
            .rounded_md()
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .child(div().absolute().left_0().right_0().bottom(px(6.0)).h(px(1.0)).bg(c(theme::GRID)));
        for (n, (i, ps)) in edits.iter().enumerate().filter(|_| matches!(row, FileRow::History(_))) {
            let e = &evs[*i];
            let (a, r) = ps.iter().fold((0, 0), |(a, r), p| (a + p.added, r + p.removed));
            let frac = (((e.start - start) / span) as f32).clamp(0.0, 1.0);
            let h = 6.0 + 18.0 * ((a + r) as f32 / max_lines as f32).sqrt();
            let col = if a >= r { theme::OK } else { theme::ERR };
            strip = strip.child(
                div()
                    .id(ElementId::NamedInteger("edit-mark".into(), *i as u64))
                    .absolute()
                    .bottom(px(6.0))
                    .left(gpui::relative(frac * 0.96 + 0.01))
                    .w(px(5.0))
                    .h(px(h))
                    .rounded(px(2.0))
                    .bg(c(col).opacity(0.85))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::ACCENT)))
                    .tooltip(tip(
                        format!("#{} {} · {} · +{a} −{r}", n + 1, local(e.start).format("%H:%M:%S"), e.name),
                        None,
                    ))
                    .on_click({
                        let idx = sel.first + i;
                        cx.listener(move |t, _: &ClickEvent, w, cx| t.open_edit(session, lane, idx, w, cx))
                    }),
            );
        }
        let axis = div()
            .flex()
            .text_xs()
            .text_color(c(theme::SUBTLE))
            .child(local(start).format(fmt).to_string())
            .child(div().flex_1())
            .child(local(end).format(fmt).to_string());

        // 編集ごとの差分
        let all = self.expanded.contains("file-diff");
        let mut diffs = div().flex().flex_col().gap_3();
        let mut hidden_total = if matches!(row, FileRow::Expand(_)) {
            edits.iter().map(|(_, ps)| ps.iter().map(|p| p.rows.len()).sum::<usize>().saturating_sub(PER_EDIT)).sum()
        } else { 0 };
        for (n, (i, ps)) in edits.iter().enumerate() {
            if !matches!(row, FileRow::Edit { event, .. } if event == *i) { continue; }
            let e = &evs[*i];
            let (a, r) = ps.iter().fold((0, 0), |(a, r), p| (a + p.added, r + p.removed));
            let (view, hidden) = rich::diff_view(ps.iter().copied(), cwd, if all { usize::MAX } else { PER_EDIT });
            hidden_total += hidden;
            let color = theme::event_color(e);
            diffs = diffs.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .id(ElementId::NamedInteger("edit-head".into(), *i as u64))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_1()
                            .py_0p5()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .hover(|d| d.bg(c(theme::HOVER)))
                            .child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .rounded_full()
                                    .bg(c(theme::GRID))
                                    .text_color(c(theme::MUTED))
                                    .child(format!("#{}", n + 1)),
                            )
                            .child(event_icon(theme::event_icon(e), color, 12.0))
                            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(e.name.clone())))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(c(theme::SUBTLE))
                                    .child(local(e.start).format("%m/%d %H:%M:%S").to_string()),
                            )
                            .child(div().flex_1())
                            .child(div().flex_none().text_color(c(theme::OK)).child(format!("+{a}")))
                            .child(div().flex_none().text_color(c(theme::ERR)).child(format!("−{r}")))
                            .child(icons::icon(Icon::ArrowRight, theme::SUBTLE).size_3())
                            .tooltip(tip("このイベントを開く", None))
                            .on_click({
                                let idx = sel.first + i;
                                cx.listener(move |t, _: &ClickEvent, w, cx| t.open_edit(session, lane, idx, w, cx))
                            }),
                    )
                    .child(view)
                    .when(hidden > 0, |d| {
                        d.child(
                            div()
                                .px_1()
                                .text_xs()
                                .text_color(c(theme::SUBTLE))
                                .child(format!("… {} 行を省略", fmt_n(hidden))),
                        )
                    }),
            );
        }
        if matches!(row, FileRow::Expand(_)) && hidden_total > 0 {
            diffs = diffs.child(
                div()
                    .id("file-diff-all")
                    .text_xs()
                    .text_color(c(theme::ACCENT))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(format!("省略した {} 行をすべて表示", fmt_n(hidden_total)))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        t.expanded.insert("file-diff");
                        t.detail_changed(cx);
                    })),
            );
        }
        if matches!(row, FileRow::Empty(_)) {
            diffs = diffs.child(div().text_xs().text_color(c(theme::SUBTLE)).child("差分は記録されていません"));
        }
        div()
            .ml(px(10.0))
            .pb_3()
            .pl_3()
            .when(matches!(row, FileRow::History(_)), |d| d.pt_1())
            .border_l_2()
            .border_color(c(theme::ACCENT).opacity(0.5))
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .when(matches!(row, FileRow::History(_)), |d| d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(label(format!("編集の時系列 · {} 回", edits.len())))
                    .child(strip)
                    .child(axis),
            ))
            .child(diffs)
    }

    /// イベントの一覧。絞り込みがなければユーザーの指示ごとに区切って並べる
    fn event_list(&self, sel: Selection, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let s = self.detail_session(sel);
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        self.detail_list.prepare(DetailListKey {
            sel,
            generation: self.shown_detail_generation(),
            filter: self.row_filter.clone(),
            reversed: self.detail_reversed,
            width: self.detail_content_width(window).to_bits(),
            live: is_live(s.end),
            replay_visible: self.replay.as_ref().map(|replay| replay.visible(evs)),
        }, evs);
        if self.replay.as_ref().is_some_and(|replay| replay.scroll_pending.replace(false)) {
            self.detail_list.scroll_to_latest(self.detail_reversed);
        }
        self.detail_list.rendered.set(0);
        let app = cx.entity().downgrade();
        let scrolling = app.clone();
        self.detail_list.state.set_scroll_handler(move |_, window, cx| {
            let _ = scrolling.update(cx, |app, cx| {
                app.cancel_detail_follow();
                if let Some(replay) = &mut app.replay {
                    replay.following = false;
                    replay.check_follow = true;
                    replay.scroll_pending.set(false);
                    app.detail_changed(cx);
                    // ListState はこのコールバック中に借用されているので、端の判定を遅延する。
                    cx.defer_in(window, move |app, _, cx| {
                        if app.selected == Some(sel) && app.replay.is_some() {
                            app.check_replay_follow(cx);
                        }
                    });
                }
            });
        });
        list(self.detail_list.state.clone(), move |row, _, cx| {
            app.update(cx, |app, cx| app.detail_list_item(sel, row, cx)).unwrap_or_else(|_| div().into_any_element())
        }).w_full().min_w_0().flex_1().min_h_0().into_any_element()
    }

    /// 集計の下、イベント行の直前に見出し・並び順・再生操作をまとめる
    fn event_list_header(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(label("イベント一覧"))
                    .child(
                        div()
                            .id("detail-order")
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                self.icon_button(
                                    "detail-order-oldest", Some(Icon::ArrowDown), "古い順", !self.detail_reversed, cx,
                                    |t, _, cx| t.set_detail_order(false, cx),
                                ).tooltip(tip("時刻の古いイベントから表示", None)),
                            )
                            .child(
                                self.icon_button(
                                    "detail-order-newest", Some(Icon::ArrowUp), "新しい順", self.detail_reversed, cx,
                                    |t, _, cx| t.set_detail_order(true, cx),
                                ).tooltip(tip("時刻の新しいイベントから表示", None)),
                            ),
                    ),
            )
            .child(self.replay_controls(sel, cx))
    }

    fn detail_list_item(&self, sel: Selection, row: usize, cx: &mut Context<Self>) -> AnyElement {
        if !self.valid(sel) || self.cache(sel).is_none() { return div().into_any_element(); }
        let item = self.detail_list.data.borrow().as_ref().and_then(|d| d.rows.get(row)).copied();
        self.detail_list.rendered.set(self.detail_list.rendered.get() + 1);
        let i = match item {
            Some(DetailRow::Summary) => return div().w_full().min_w_0().flex().flex_col().gap_2().pb_2()
                .child(self.selection_summary(sel, cx))
                .child(self.event_list_header(sel, cx)).into_any_element(),
            Some(DetailRow::Empty) => return div().py_2().text_xs().text_color(c(theme::SUBTLE))
                .child(if self.replay.as_ref().is_some_and(|replay| !replay.is_finished()) {
                    "次のイベントを待っています…"
                } else { "該当するイベントはありません" }).into_any_element(),
            Some(DetailRow::Limit) => return div().pt_1().text_xs().text_color(c(theme::MUTED)).child(format!(
                "先頭 {} 件を表示しています（全 {} 件）", fmt_n(DETAIL_MAX), fmt_n(sel.last - sel.first + 1)
            )).into_any_element(),
            Some(DetailRow::Event(i)) => i,
            None => return div().into_any_element(),
        };
        let s = self.detail_session(sel);
        let e = &s.lanes[sel.lane].events[sel.first + i];
        let cache = self.cache(sel);
        let touched = cache.and_then(|d| d.touched.get(i)).map_or(&[][..], Vec::as_slice);
        let (preview, output) = cache.and_then(|d| d.texts.get(i)).map_or(("", ""), |t| (t.0.as_str(), t.1.as_str()));
        if e.kind == Kind::User && self.row_filter.is_none() {
            self.turn_row(sel, i, preview, cx).into_any_element()
        } else {
            self.event_row(sel, i, (preview, output), touched, is_live(s.end), cx).into_any_element()
        }
    }

    /// ユーザーの指示の行。次の指示までの件数・時間・失敗を添え、本文を大きめに出す（ターンの区切り）
    fn turn_row(&self, sel: Selection, i: usize, text: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let lane = &self.detail_session(sel).lanes[sel.lane];
        let idx = sel.first + i;
        let e = &lane.events[idx];
        let turns = &self.cache(sel).unwrap().session_summary.turns[sel.lane];
        let turn = &turns[turns.binary_search_by_key(&idx, |turn| turn.index).unwrap()];
        let until = turn.end;
        let mut facts = vec![format!("{} 件", fmt_n(turn.events))];
        if until > e.start {
            facts.push(fmt_dur(until - e.start));
        }
        let color = theme::event_color(e);
        let text = if text.is_empty() { e.summary.as_ref() } else { text };
        let content = div()
            .my_1()
            .px_2p5()
            .py_2()
            .rounded_lg()
            .bg(c(color).opacity(0.08))
            .border_1()
            .border_color(c(color).opacity(0.25))
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .whitespace_normal()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(c(color)).child("指示"))
                    .child(div().flex_1())
                    .when(turn.failures > 0, |d| d.child(badge(format!("失敗 {}", turn.failures), theme::ERR)))
                    .child(div().flex_none().text_color(c(theme::SUBTLE)).child(facts.join(" · "))),
            )
            .child(div().text_sm().line_clamp(3).child(one_line(text, 400)));
        self.rail_row(sel, idx, e, true, content, cx)
    }

    /// 一覧の 1 行。1 行目に名前と数値（失敗・差分・所要時間・トークン）を右に揃え、
    /// 2 行目に要約（メッセージなら本文の先頭）を出す
    fn event_row(
        &self,
        sel: Selection,
        i: usize,
        (text, output): (&str, &str),
        touched: &[(String, usize, usize)],
        live: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let idx = sel.first + i;
        let s = self.detail_session(sel);
        let e = &s.lanes[sel.lane].events[idx];
        let message = matches!(e.kind, Kind::User | Kind::Assistant | Kind::Thinking);
        let status = status_label(e, live, output);
        let (added, removed) = touched.iter().fold((0, 0), |(a, r), t| (a + t.1, r + t.2));
        // ファイルのパスは作業ディレクトリからの相対にして短くする
        let commands = self.cache(sel).and_then(|d| d.commands.get(i)).map_or(&[][..], Vec::as_slice);
        let body: SharedString = if message {
            if text.is_empty() { e.summary.clone().into() } else { one_line(text, 400).into() }
        } else {
            rich::rel_path(&e.summary, &s.cwd).to_string().into()
        };
        let num = |t: String, col: u32| div().flex_none().text_color(c(col)).child(t);
        let content = div()
            .flex_1()
            .min_w_0()
            .py_1p5()
            .pr_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .flex_shrink()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(if message { theme::MUTED } else { theme::TEXT }))
                            .child(SharedString::from(e.name.clone())),
                    )
                    .when_some(status, |d, (l, col)| d.child(badge(l, col)))
                    .child(div().flex_1())
                    .when(added + removed > 0, |d| {
                        d.child(num(format!("+{added}"), theme::OK)).child(num(format!("−{removed}"), theme::ERR))
                    })
                    .when(e.end > e.start, |d| d.child(num(fmt_dur(e.end - e.start), theme::SUBTLE)))
                    .when(!e.usage.is_empty(), |d| {
                        d.child(usage_chip(
                            ElementId::NamedInteger("usage".into(), idx as u64),
                            &e.usage,
                            e.model.as_deref(),
                        ))
                    }),
            )
            // コマンドは元の行を保ち、横スクロールで全文を出す
            .when(!commands.is_empty(), |d| {
                d.child(rich::horizontal_scroll("commands").child(
                    div().flex_none().min_w_full().flex().flex_col().gap_0p5()
                        .children(commands.iter().map(|cmd| {
                            div()
                                .flex()
                                .gap_1p5()
                                .text_xs()
                                .font_family(rich::MONO)
                                .child(div().flex_none().text_color(c(theme::SUBTLE)).child("$"))
                                .child(div().flex_none().text_color(c(theme::MUTED)).child(cmd.clone()))
                        })),
                ))
            })
            .when(commands.is_empty() && !body.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .whitespace_normal()
                        .line_clamp(if e.kind == Kind::Thinking { 2 } else { 3 })
                        .when(!message, |d| d.font_family(rich::MONO))
                        .text_color(c(match e.kind {
                            Kind::Assistant => theme::TEXT,
                            _ => theme::MUTED,
                        }))
                        .child(body),
                )
            });
        self.rail_row(sel, idx, e, false, content, cx)
    }

    /// 一覧の行の外枠。左に時刻、縦の線の上にイベントの種類のアイコン、右に内容を並べる。
    /// 押すとそのイベントだけを選ぶ
    fn rail_row(
        &self,
        sel: Selection,
        idx: usize,
        e: &Event,
        turn: bool,
        content: Div,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (session, lane) = (sel.session, sel.lane);
        let color = theme::event_color(e);
        let node = if turn { 22.0 } else { 18.0 };
        div()
            .id(ElementId::NamedInteger("ev".into(), idx as u64))
            // List は行を独立して計測するため、幅を指定しないと本文の長さまで広がる。
            .w_full()
            .min_w_0()
            .flex()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|d| d.bg(c(theme::HOVER)))
            .child(
                div()
                    .flex_none()
                    .w(px(58.0))
                    .whitespace_nowrap()
                    .pt(px(if turn { 12.0 } else { 7.0 }))
                    .pl_1()
                    .text_xs()
                    .text_color(c(theme::SUBTLE))
                    .child(local(e.start).format("%H:%M:%S").to_string()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(22.0))
                    .relative()
                    .flex()
                    .justify_center()
                    // 行をまたいでつながる縦の線
                    .child(div().absolute().top_0().bottom_0().left(px(10.5)).w(px(1.0)).bg(c(theme::BORDER)))
                    .child(
                        div()
                            .mt(px(if turn { 8.0 } else { 5.0 }))
                            .size(px(node))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(c(theme::PANEL))
                            .border_1()
                            .border_color(c(color).opacity(0.5))
                            .child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(c(color).opacity(0.16))
                                    .child(event_icon(theme::event_icon(e), color, if turn { 12.0 } else { 10.0 })),
                            ),
                    ),
            )
            .child(div().flex_1().min_w_0().flex().child(content.flex_1().min_w_0()))
            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                t.navigate(Selection { session, lane, first: idx, last: idx }, w, cx);
            }))
    }
}

/// トークン数を短く（1,234 / 12.3k / 1.23M / 13.2B）
pub(super) fn fmt_tokens(n: u64) -> String {
    match n {
        0..10_000 => fmt_n(n as usize),
        10_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.2}M", n as f64 / 1e6),
        _ => format!("{:.2}B", n as f64 / 1e9),
    }
}

/// 使用量の内訳の色: 入力・キャッシュ読込・キャッシュ書込・出力
const USAGE_COLORS: [u32; 4] = [0x5b9cf5, 0x3f6a9e, 0xd29a4a, 0x7fd18b];

/// 使用量の合計・内訳の帯・凡例
pub(super) fn usage_view(u: &UsageSum) -> Div {
    let total = u.total().max(1) as f32;
    let parts = [
        ("入力", u.input, USAGE_COLORS[0]),
        ("キャッシュ読込", u.cache_read, USAGE_COLORS[1]),
        ("キャッシュ書込", u.cache_write, USAGE_COLORS[2]),
        ("出力", u.output, USAGE_COLORS[3]),
    ];
    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2p5()
        .rounded_lg()
        .bg(c(theme::BG))
        .border_1()
        .border_color(c(theme::BORDER))
        .text_xs()
        .child(
            div()
                .flex()
                .items_end()
                .gap_1p5()
                .child(
                    div()
                        .text_lg()
                        .line_height(px(22.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(fmt_tokens(u.total())),
                )
                .child(
                    div()
                        .pb(px(2.0))
                        .text_color(c(theme::SUBTLE))
                        .child(format!("トークン · {} 回", fmt_n(u.calls as usize))),
                )
                .child(div().flex_1())
                .when(u.unpriced > 0, |d| {
                    d.child(
                        div()
                            .id("unpriced")
                            .pb(px(2.0))
                            .text_color(c(theme::SUBTLE))
                            .child(format!("料金不明 {} 回", fmt_n(u.unpriced as usize)))
                            .tooltip(tip(
                                format!(
                                    "料金表にないモデル。{} に追加できます",
                                    tilde(&crate::pricing::path().to_string_lossy())
                                ),
                                None,
                            )),
                    )
                })
                .when(u.calls > u.unpriced, |d| {
                    d.child(
                        div()
                            .text_lg()
                            .line_height(px(22.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(theme::OK))
                            .child(fmt_usd(u.cost)),
                    )
                }),
        )
        .child(div().flex().gap(px(2.0)).h(px(8.0)).w_full().children(parts.iter().filter(|p| p.1 > 0).map(
            |&(_, n, col)| {
                div()
                    .h_full()
                    .flex_basis(gpui::relative(n as f32 / total))
                    .flex_shrink()
                    .min_w(px(3.0))
                    .rounded(px(2.0))
                    .bg(c(col))
            },
        )))
        .child(div().flex().flex_wrap().gap_y_1().children(parts.iter().filter(|p| p.1 > 0).map(|&(label, n, col)| {
            div()
                .w(gpui::relative(0.5))
                .pr_3()
                .flex()
                .items_center()
                .gap_1p5()
                .child(div().size(px(8.0)).flex_none().rounded(px(2.0)).bg(c(col)))
                .child(div().flex_1().min_w_0().truncate().text_color(c(theme::MUTED)).child(label))
                .child(div().flex_none().child(fmt_tokens(n)))
                .child(
                    div()
                        .flex_none()
                        .w(px(30.0))
                        .flex()
                        .justify_end()
                        .text_color(c(theme::SUBTLE))
                        .child(format!("{:.0}%", n as f32 / total * 100.0)),
                )
        })))
        .when(u.reasoning > 0, |d| {
            d.child(div().text_color(c(theme::SUBTLE)).child(format!("出力のうち推論 {}", fmt_tokens(u.reasoning))))
        })
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

/// 料金（USD）。1 ドル以上は桁区切りを入れる
pub(super) fn fmt_usd(v: f64) -> String {
    if v < 0.01 && v > 0.0 {
        format!("${v:.4}")
    } else {
        let cents = (v * 100.0).round() as u64;
        format!("${}.{:02}", fmt_n((cents / 100) as usize), cents % 100)
    }
}

/// モデルごとのトークン数と料金
fn model_table(models: &[(String, UsageSum)]) -> Div {
    div().flex().flex_col().gap_0p5().children(models.iter().map(|(m, u)| {
        div()
            .flex()
            .gap_2()
            .child(div().flex_1().min_w_0().truncate().child(if m.is_empty() {
                "(不明)".to_string()
            } else {
                m.clone()
            }))
            .child(div().flex_none().text_color(c(theme::MUTED)).child(fmt_tokens(u.total())))
            .child(div().flex_none().w(px(64.0)).flex().justify_end().child(if u.unpriced > 0 && u.cost == 0.0 {
                div().text_color(c(theme::SUBTLE)).child("—")
            } else {
                div().text_color(c(theme::OK)).child(fmt_usd(u.cost))
            }))
    }))
}

/// イベントに添える使用量（入力はキャッシュ込み）。内訳・モデル・料金はツールチップに出す
fn usage_chip(id: impl Into<ElementId>, u: &Usage, model: Option<&str>) -> Stateful<Div> {
    let input = u.input as u64 + u.cache_read as u64 + u.cache_write as u64;
    let mut detail = format!(
        "入力 {} / キャッシュ読込 {} / キャッシュ書込 {} / 出力 {}",
        fmt_n(u.input as usize),
        fmt_n(u.cache_read as usize),
        fmt_n(u.cache_write as usize),
        fmt_n(u.output as usize)
    );
    if u.reasoning > 0 {
        detail.push_str(&format!("（うち推論 {}）", fmt_n(u.reasoning as usize)));
    }
    if let Some(m) = model {
        detail.push_str(&format!("\n{m}"));
        if let Some(p) = crate::pricing::price(m) {
            detail.push_str(&format!(" · {}", fmt_usd(p.cost(u))));
        }
    }
    div()
        .id(id)
        .flex_none()
        .text_xs()
        .text_color(c(theme::SUBTLE))
        .child(format!("↑{} ↓{}", fmt_tokens(input), fmt_tokens(u.output as u64)))
        .tooltip(tip(detail, None))
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
        let events = vec![list_event("Read", Status::Ok); 5];
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
