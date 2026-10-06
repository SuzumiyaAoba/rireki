//! API 換算コストのタイムライン。集計は背景で行い、可視行だけを描く。

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use gpui::{
    AnyElement, ClickEvent, Context, Div, ElementId, FontWeight, Window, canvas, div, prelude::*, px,
};

use super::detail::{fmt_tokens, fmt_usd};
use super::widgets::{project_tile, tip};
use super::{AXIS_H, FOOTER_H, ROW_H, Rireki, Selection, local, now};
use crate::cost_timeline::{Bucket, Cache, Summary, View};
use crate::icons::{self, Icon};
use crate::model::{UsageSum, one_line};
use crate::theme::{self, c};

type Key = (u64, u64, u64, usize, bool);

#[derive(Clone, Copy)]
enum Row {
    Directory(usize),
    Session(usize, usize),
}

struct Rows {
    directory: bool,
    collapsed: HashSet<String>,
    rows: Rc<Vec<Row>>,
}

pub(super) struct State {
    key: Option<Key>,
    shown: Option<Key>,
    summary: Option<Arc<Summary>>,
    rows: Option<Rows>,
    task: Option<gpui::Task<()>>,
    cache: Arc<Mutex<Cache>>,
    generation: Arc<AtomicU64>,
    pub(super) max_bucket: f64,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            key: None,
            shown: None,
            summary: None,
            rows: None,
            task: None,
            cache: Arc::new(Mutex::new(Cache::default())),
            generation: Arc::new(AtomicU64::new(0)),
            max_bucket: 0.0,
        }
    }

    fn request(&mut self, key: Key) -> Option<u64> {
        if self.key == Some(key) {
            return None;
        }
        self.key = Some(key);
        Some(self.generation.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn publish(&mut self, key: Key, version: u64, summary: Summary) -> bool {
        if self.key != Some(key) || self.generation.load(Ordering::Relaxed) != version {
            return false;
        }
        self.summary = Some(Arc::new(summary));
        self.shown = Some(key);
        self.rows = None;
        true
    }

    /// 再集計中も直前の結果を描き、新しい集計が完成してから差し替える。
    fn summary(&self) -> Option<Arc<Summary>> {
        self.summary.clone()
    }

    fn rows(&mut self, summary: &Summary, directory: bool, collapsed: &HashSet<String>) -> Rc<Vec<Row>> {
        if let Some(rows) = &self.rows
            && rows.directory == directory
            && (directory || rows.collapsed == *collapsed)
        {
            return rows.rows.clone();
        }
        let mut rows = Vec::new();
        self.max_bucket = 0.0;
        for (di, group) in summary.directories.iter().enumerate() {
            rows.push(Row::Directory(di));
            if directory || collapsed.contains(&group.cwd) {
                for b in &group.buckets {
                    self.max_bucket = self.max_bucket.max(b.usage.cost);
                }
            } else {
                for (si, session) in group.sessions.iter().enumerate() {
                    rows.push(Row::Session(di, si));
                    for b in &session.buckets {
                        self.max_bucket = self.max_bucket.max(b.usage.cost);
                    }
                }
            }
        }
        let rows = Rc::new(rows);
        self.rows = Some(Rows {
            directory,
            collapsed: if directory { HashSet::new() } else { collapsed.clone() },
            rows: rows.clone(),
        });
        rows
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

fn cost_amount(cost: f64) -> String {
    if cost > 0.0 && cost < 0.0001 { "<$0.0001".to_string() } else { fmt_usd(cost) }
}

pub(super) fn cost_label(usage: &UsageSum) -> String {
    if usage.calls == 0 {
        "—".into()
    } else if usage.calls == usage.unpriced {
        "料金不明".into()
    } else if usage.unpriced > 0 {
        format!("{} + 不明", cost_amount(usage.cost))
    } else {
        cost_amount(usage.cost)
    }
}

fn usage_tip(usage: &UsageSum) -> String {
    let mut text = format!(
        "API 換算 {}（USD）\n{} トークン · API {} 回",
        cost_label(usage),
        fmt_tokens(usage.total()),
        usage.calls
    );
    if usage.unpriced > 0 {
        text.push_str(&format!("\n料金不明 {} 回の金額は含まれていません", usage.unpriced));
    }
    if usage.calls == 0 {
        text.push_str("\nこの期間に記録された API 使用量はありません");
    }
    text
}

impl Rireki {
    pub(super) fn toggle_cost_timeline(&mut self, cx: &mut Context<Self>) {
        self.cost_timeline = !self.cost_timeline;
        self.row_scroll = 0.0;
        self.hovered = None;
        self.hits.clear();
        self.popover = None;
        if !self.cost_timeline {
            self.cost_state.generation.fetch_add(1, Ordering::Relaxed);
            self.cost_state.task = None;
            self.cost_state.key = None;
        }
        self.persist();
        cx.notify();
    }

    fn request_cost_timeline(&mut self, window: &Window, cx: &mut Context<Self>) {
        let width = self.plot_width(window) as f64;
        let Some(view) = View::new(self.t0, self.t0 + self.spp * width, (width / 36.0).ceil() as usize)
        else {
            return;
        };
        let key = (self.data_gen, view.start.to_bits(), view.end.to_bits(), view.bins, self.in_view_only);
        let Some(version) = self.cost_state.request(key) else {
            return;
        };
        let store = self.store.clone();
        let order = self.order.clone();
        let cache = self.cost_state.cache.clone();
        let generation = self.cost_state.generation.clone();
        self.cost_state.task = Some(cx.spawn(async move |this, cx| {
            let summary = cx
                .background_executor()
                .spawn(async move {
                    cache.lock().unwrap().build_while(&store, &order, view, key.4, || {
                        generation.load(Ordering::Relaxed) == version
                    })
                })
                .await;
            let _ = this.update(cx, |t, cx| {
                if let Some(summary) = summary
                    && t.cost_state.publish(key, version, summary)
                {
                    cx.notify();
                }
            });
        }));
    }

    pub(super) fn cost_timeline_view(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        self.request_cost_timeline(window, cx);
        self.hits.clear();
        self.hovered = None;
        let pw = self.plot_width(window);
        let ph = self.plot_height(window);
        let v0 = self.t0;
        let v1 = v0 + self.spp * pw as f64;
        let spp = self.spp;
        let x_of = |t: f64| ((t - v0) / spp) as f32;
        let mut axis = div().relative().h(px(AXIS_H)).w(px(pw)).overflow_hidden();
        let mut grid = div().absolute().top_0().left_0().size_full();
        for (t, label, major) in super::timeline::ticks(v0, v1, spp) {
            let x = x_of(t);
            if x < 0.0 {
                continue;
            }
            axis = axis.child(
                div()
                    .absolute()
                    .left(px(x + 3.0))
                    .top(px(if major { 2.0 } else { 14.0 }))
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(c(if major { theme::TEXT } else { theme::MUTED }))
                    .child(label),
            );
            grid = grid.child(div().absolute().left(px(x)).top_0().bottom_0().w(px(1.0)).bg(c(if major {
                0x353b47
            } else {
                theme::GRID
            })));
        }
        let nx = x_of(now());
        if (0.0..pw).contains(&nx) {
            grid = grid.child(div().absolute().left(px(nx)).top_0().bottom_0().w(px(1.0)).bg(c(theme::NOW)));
        }
        let origin = self.hit_origin.clone();
        let mut plot = div().relative().w(px(pw)).h(px(ph)).overflow_hidden().child(grid).child(
            canvas(move |bounds, _, _| origin.set(bounds.origin), |_, _, _, _| {}).absolute().size_full(),
        );
        let mut labels = div().relative().w(px(self.label_w)).h(px(ph)).overflow_hidden();
        if let Some(summary) = self.cost_state.summary() {
            let rows = self.cost_state.rows(&summary, self.directory_timeline, &self.collapsed);
            self.row_scroll = self.row_scroll.min((rows.len() as f32 * ROW_H - ph + ROW_H).max(0.0));
            let first = (self.row_scroll / ROW_H) as usize;
            let visible = (ph / ROW_H) as usize + 2;
            self.counts = format!(
                "表示期間 {} · {} ディレクトリ · {} セッション",
                cost_label(&summary.usage),
                summary.directories.len(),
                summary.sessions
            );
            let max = self.cost_state.max_bucket;
            for (ri, row) in rows.iter().enumerate().skip(first).take(visible) {
                let y = ri as f32 * ROW_H - self.row_scroll;
                let (group, session) = match *row {
                    Row::Directory(di) => (&summary.directories[di], None),
                    Row::Session(di, si) => {
                        (&summary.directories[di], Some(&summary.directories[di].sessions[si]))
                    }
                };
                let directory = session.is_none();
                let usage = session.map_or(&group.usage, |s| &s.usage);
                let active = session.map_or_else(|| group.sessions.iter().any(|s| s.active), |s| s.active);
                let collapsed = self.collapsed.contains(&group.cwd);
                let selected =
                    self.selected.is_some_and(|s| session.is_some_and(|row| row.session == s.session));
                let mut label = div()
                    .id(ElementId::NamedInteger("cost-label".into(), ri as u64))
                    .absolute()
                    .top(px(y))
                    .left_0()
                    .w(px(self.label_w))
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .text_xs()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .when(directory, |d| d.bg(c(0x1f232b)).border_t_1().border_color(c(0x3a4150)))
                    .when(selected, |d| d.bg(c(theme::ACCENT_BG)))
                    .when(!active, |d| d.opacity(0.4))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::HOVER)));
                let mut detail = usage_tip(usage);
                if let Some(row) = session {
                    let s = &self.store.sessions[row.session];
                    let title = if s.title.is_empty() { "(無題)" } else { &s.title };
                    let (icon, color) = icons::source_icon(s.source);
                    label = label
                        .pl(px(24.0))
                        .child(icons::icon(icon, color))
                        .child(div().flex_1().min_w_0().truncate().child(one_line(title, 80)));
                    detail =
                        format!("{title}\n{}\n{}\n{detail}\n全レーン・サブエージェントを含む", s.cwd, s.id);
                    let sample =
                        row.buckets.first().map(|b| b.sample).or_else(|| {
                            s.lanes.iter().enumerate().find(|(_, l)| !l.events.is_empty()).map(|(li, _)| {
                                Selection { session: row.session, lane: li, first: 0, last: 0 }
                            })
                        });
                    let data_gen = self.cost_state.shown.map(|key| key.0);
                    label = label.on_click(cx.listener(move |t, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        if let Some(sel) = sample.filter(|s| data_gen == Some(t.data_gen) && t.valid(*s)) {
                            t.navigate(sel, window, cx);
                        }
                    }));
                } else {
                    let cwd = group.cwd.clone();
                    let directory_mode = self.directory_timeline;
                    label = label
                        .when(!directory_mode, |d| {
                            d.child(icons::icon(
                                if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                                theme::MUTED,
                            ))
                        })
                        .child(project_tile(&group.cwd, 14.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.project_name(&group.cwd)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(c(theme::SUBTLE))
                                .child(group.sessions.len().to_string()),
                        );
                    detail = format!(
                        "{}\n{detail}\n{} セッション（全レーンを含む）\n{}",
                        group.cwd,
                        group.sessions.len(),
                        if directory_mode {
                            "クリックでこのディレクトリのセッション別コストを表示"
                        } else {
                            "クリックで折りたたみ・展開"
                        }
                    );
                    label = label.on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if t.directory_timeline {
                            t.directory_timeline = false;
                            t.set_focus(Some(cwd.clone()), cx);
                        } else if !t.collapsed.remove(&cwd) {
                            t.collapsed.insert(cwd.clone());
                        }
                        t.persist();
                        cx.notify();
                    }));
                }
                labels = labels.child(
                    label
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(if usage.unpriced > 0 { theme::WARN } else { theme::OK }))
                                .child(cost_label(usage)),
                        )
                        .tooltip(tip(detail, None)),
                );
                plot = plot.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(y))
                        .h(px(ROW_H))
                        .border_b_1()
                        .border_color(c(theme::GRID))
                        .when(directory, |d| d.bg(c(0x1b1e25).opacity(0.6)))
                        .when(selected, |d| d.bg(c(theme::ACCENT).opacity(0.07))),
                );
                let buckets = session.map_or_else(
                    || {
                        if self.directory_timeline || collapsed { group.buckets.as_slice() } else { &[] }
                    },
                    |s| s.buckets.as_slice(),
                );
                for bucket in buckets {
                    let x = x_of(summary.edges[bucket.index]);
                    let width = (x_of(summary.edges[bucket.index + 1]) - x - 2.0).max(1.0);
                    plot = plot.child(self.cost_bar(bucket, &summary, ri, (x, y, width), max, cx));
                }
            }
            if rows.is_empty() {
                plot = plot.child(self.empty_state(cx));
            }
        } else {
            self.counts = "API コストを集計中…".into();
            plot = plot.child(
                div()
                    .absolute()
                    .top(px(32.0))
                    .left(px(24.0))
                    .text_sm()
                    .text_color(c(theme::MUTED))
                    .child("API コストを集計中…"),
            );
        }
        div().flex().flex_col().child(div().flex().border_b_1().border_color(c(theme::BORDER))
            .child(div().w(px(self.label_w)).h(px(AXIS_H)).flex_none().px_2().flex().items_center()
                .text_xs().text_color(c(theme::MUTED)).border_r_1().border_color(c(theme::BORDER))
                .child("表示期間の合計 · API 換算 USD")
                .id("cost-explanation").tooltip(tip("記録されたトークン使用量をモデル別の API 料金で換算。サブスクリプションの請求額とは異なります。使用量のない呼び出しと料金不明のモデルは金額に含みません。", None)))
            .child(axis))
            .child(div().flex().child(labels.border_r_1().border_color(c(theme::BORDER))).child(plot))
            .into_any_element()
    }

    fn cost_bar(
        &self,
        bucket: &Bucket,
        summary: &Summary,
        row: usize,
        rect: (f32, f32, f32),
        max: f64,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let (x, y, width) = rect;
        let height = if max > 0.0 && bucket.usage.cost > 0.0 {
            ((bucket.usage.cost / max) as f32 * (ROW_H - 6.0)).max(3.0)
        } else {
            3.0
        };
        let sel = bucket.sample;
        let data_gen = self.cost_state.shown.map(|key| key.0);
        let session = &self.store.sessions[sel.session];
        let title = if session.title.is_empty() { "(無題)" } else { &session.title };
        let precise = summary.edges[bucket.index + 1] - summary.edges[bucket.index] < 1.0;
        let detail = format!(
            "{} – {}\n{}\n代表呼び出し: {}\nクリックで詳細を開く",
            local(summary.edges[bucket.index]).format(if precise {
                "%Y/%m/%d %H:%M:%S%.3f"
            } else {
                "%Y/%m/%d %H:%M:%S"
            }),
            local(summary.edges[bucket.index + 1]).format(if precise {
                "%m/%d %H:%M:%S%.3f"
            } else {
                "%m/%d %H:%M:%S"
            }),
            usage_tip(&bucket.usage),
            one_line(title, 80)
        );
        div()
            .id(ElementId::NamedInteger(format!("cost-bin-{row}").into(), bucket.index as u64))
            .absolute()
            .left(px(x + 1.0))
            .top(px(y + 3.0))
            .w(px(width))
            .h(px(ROW_H - 6.0))
            .cursor_pointer()
            .hover(|d| d.bg(c(theme::OK).opacity(0.12)))
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(height))
                    .rounded(px(2.0))
                    .bg(c(if bucket.usage.calls == bucket.usage.unpriced { theme::WARN } else { theme::OK })
                        .opacity(0.8))
                    .when(bucket.usage.unpriced > 0, |d| {
                        d.child(div().absolute().top_0().left_0().right_0().h(px(2.0)).bg(c(theme::WARN)))
                    }),
            )
            .tooltip(tip(detail, None))
            .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                // ログ更新中は旧集計のイベント位置を新しい版に適用しない。
                if data_gen == Some(t.data_gen) && t.valid(sel) {
                    t.navigate(sel, window, cx);
                }
            }))
    }

    pub(super) fn scroll_to_cost_selection(&mut self, sel: Selection, window: &Window) {
        let Some(summary) = self.cost_state.summary() else {
            return;
        };
        let cwd = &self.store.sessions[sel.session].cwd;
        if !self.directory_timeline && self.collapsed.remove(cwd) {
            self.persist();
        }
        let rows = self.cost_state.rows(&summary, self.directory_timeline, &self.collapsed);
        let row = rows.iter().position(|row| match *row {
            Row::Directory(di) => {
                self.directory_timeline && summary.directories[di].cwd == self.store.sessions[sel.session].cwd
            }
            Row::Session(di, si) => summary.directories[di].sessions[si].session == sel.session,
        });
        if let Some(row) = row {
            let y = row as f32 * ROW_H;
            let ph = self.plot_height(window);
            if y < self.row_scroll || y + ROW_H > self.row_scroll + ph {
                self.row_scroll = (y - ph / 3.0).max(0.0);
            }
        }
    }

    pub(super) fn cost_footer(&self, window: &Window) -> Div {
        let pending = self.cost_state.shown != self.cost_state.key;
        let summary = self.cost_state.summary();
        let detail = summary.as_ref()
            .map_or_else(|| self.counts.clone(), |s| usage_tip(&s.usage));
        div().h(px(FOOTER_H)).flex_none().flex().items_center().gap_3().px_3()
            .border_t_1().border_color(c(theme::BORDER)).text_xs().text_color(c(theme::MUTED))
            .child(div().id("cost-total").flex_1().min_w_0().truncate().child(self.counts.clone()).tooltip(tip(detail, None)))
            .when(summary.is_some() && self.main_width(window) >= 900.0, |d| d.child(
                div().id("cost-scale").flex_none().whitespace_nowrap()
                    .child(format!("高さの上限 {} / 区間", cost_amount(self.cost_state.max_bucket)))
                    .tooltip(tip("すべての行を同じ金額の縮尺で表示。表示範囲と幅に合わせて区間を分割します。", None))))
            .when(pending && summary.is_some(), |d| d.child("集計中…"))
            .child(div().id("cost-legend").flex_none().flex().items_center().gap_1()
                .child(div().size(px(7.0)).rounded_sm().bg(c(theme::OK))).child("API 換算")
                .child(div().size(px(7.0)).rounded_sm().bg(c(theme::WARN))).child("料金不明")
                .tooltip(tip("緑の棒は区間の API 換算料金。オレンジの印は料金不明の呼び出し。実際の請求額とは異なります。", None)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(cwd: &str) -> Summary {
        Summary {
            view: View::new(10.0, 20.0, 2).unwrap(),
            edges: vec![10.0, 15.0, 20.0],
            directories: vec![crate::cost_timeline::Directory {
                cwd: cwd.into(), sessions: Vec::new(), usage: UsageSum::default(), buckets: Vec::new(),
            }],
            usage: UsageSum::default(),
            sessions: 0,
        }
    }

    #[test]
    fn recalculation_keeps_the_displayed_summary_and_rows_until_completion() {
        let mut state = State::new();
        let key = (1, 10.0f64.to_bits(), 20.0f64.to_bits(), 2, false);
        let version = state.request(key).unwrap();
        assert!(state.summary().is_none());
        assert!(state.publish(key, version, summary("/old")));
        let displayed = state.summary().unwrap();
        let collapsed = HashSet::new();
        let rows = state.rows(&displayed, false, &collapsed);
        assert_eq!(rows.len(), 1);

        // ログ更新、横移動、ズーム、幅変更、期間内への絞り込みでも表示を消さない。
        let changes = [
            (2, key.1, key.2, key.3, key.4),
            (2, 11.0f64.to_bits(), 21.0f64.to_bits(), key.3, key.4),
            (2, 11.0f64.to_bits(), 16.0f64.to_bits(), key.3, key.4),
            (2, 11.0f64.to_bits(), 16.0f64.to_bits(), 4, key.4),
            (2, 11.0f64.to_bits(), 16.0f64.to_bits(), 4, true),
        ];
        let mut version = version;
        for changed in changes {
            version = state.request(changed).unwrap();
            assert!(state.request(changed).is_none());
            let retained = state.summary().unwrap();
            assert!(Arc::ptr_eq(&displayed, &retained));
            assert!(Rc::ptr_eq(&rows, &state.rows(&retained, false, &collapsed)));
            assert_eq!(state.shown, Some(key));
        }
        assert!(state.publish(changes[4], version, summary("/new")));
        let updated = state.summary().unwrap();
        assert_eq!(updated.directories[0].cwd, "/new");
        assert!(!Arc::ptr_eq(&displayed, &updated));
        assert!(!Rc::ptr_eq(&rows, &state.rows(&updated, false, &collapsed)));
        assert_eq!(state.shown, state.key);
    }

    #[test]
    fn superseded_results_cannot_replace_the_display_even_after_returning_to_the_same_view() {
        let mut state = State::new();
        let key = (1, 10.0f64.to_bits(), 20.0f64.to_bits(), 2, false);
        let old_version = state.request(key).unwrap();
        assert!(state.publish(key, old_version, summary("/displayed")));
        let displayed = state.summary().unwrap();
        let other = (key.0, 20.0f64.to_bits(), 30.0f64.to_bits(), key.3, key.4);
        let other_version = state.request(other).unwrap();
        let latest_version = state.request(key).unwrap();
        assert!(!state.publish(other, other_version, summary("/superseded")));
        assert!(!state.publish(key, old_version, summary("/old-version")));
        assert!(Arc::ptr_eq(&displayed, &state.summary().unwrap()));
        assert!(state.publish(key, latest_version, summary("/latest")));
        assert_eq!(state.summary().unwrap().directories[0].cwd, "/latest");
    }

    #[test]
    fn cost_labels_distinguish_missing_unpriced_free_and_small_paid_usage() {
        assert_eq!(cost_label(&UsageSum::default()), "—");
        assert_eq!(cost_label(&UsageSum { calls: 1, unpriced: 1, ..UsageSum::default() }), "料金不明");
        assert_eq!(cost_label(&UsageSum { calls: 1, ..UsageSum::default() }), "$0.00");
        assert_eq!(cost_label(&UsageSum { calls: 1, cost: 0.0000002, ..UsageSum::default() }), "<$0.0001");
        assert_eq!(
            cost_label(&UsageSum { calls: 2, unpriced: 1, cost: 12.5, ..UsageSum::default() }),
            "$12.50 + 不明"
        );
    }
}
