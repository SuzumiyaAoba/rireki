//! 表示期間の選択（カレンダーで日付の範囲を選ぶポップオーバー）と、期間全体のどこにイベントがあるかを示すミニマップ

use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{
    AnyElement, Bounds, ClickEvent, Corner, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Window,
    anchored, canvas, deferred, div, fill, point, prelude::*, px, size,
};

use super::header::weekday_ja;
use super::widgets::tip;
use super::{MINIMAP_H, Popover, Rireki, local, midnight};
use crate::icons::{self, Icon};
use crate::prefs::Mode;
use crate::theme::{self, c};

/// ミニマップの棒 1 本の幅（px）
const BIN_W: f32 = 2.0;

impl Rireki {
    /// 表示中のセッションのイベントを、`edges` で区切った区間ごとに数える（開始時刻で数える）
    fn count_bins(&self, edges: &[f64]) -> Vec<u32> {
        let mut out = vec![0u32; edges.len().saturating_sub(1)];
        let (Some(&a), Some(&b)) = (edges.first(), edges.last()) else {
            return out;
        };
        for &si in &self.order {
            let s = &self.store.sessions[si];
            if s.end < a || s.start >= b {
                continue;
            }
            for lane in &s.lanes {
                let evs = &lane.events;
                let mut prev = evs.partition_point(|e| e.start < a);
                for (i, &t) in edges[1..].iter().enumerate() {
                    let next = evs.partition_point(|e| e.start < t);
                    out[i] += (next - prev) as u32;
                    prev = next;
                }
            }
        }
        out
    }

    /// ミニマップが表す期間。選んだ期間があればそれ、なければ表示中のセッション全体
    fn minimap_span(&self) -> Option<(f64, f64)> {
        self.pick.or_else(|| {
            let ss = self.order.iter().map(|&si| &self.store.sessions[si]);
            let (a, b) = ss.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| (a.min(s.start), b.max(s.end)));
            (a.is_finite() && b > a).then(|| {
                let pad = (b - a) * 0.02;
                (a - pad, b + pad)
            })
        })
    }

    /// 日付の範囲を表示期間にする（`a`〜`b` の両日を含む）
    fn pick_days(&mut self, a: NaiveDate, b: NaiveDate, window: &Window) {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let (start, end) = (midnight(a), midnight(b + Duration::days(1)));
        self.mode = Mode::Timeline;
        self.pick = Some((start, end));
        self.set_range(start, end, window);
        self.persist();
    }

    pub(super) fn open_picker(&mut self) {
        let base = self.pick.map(|(a, _)| local(a).date_naive()).unwrap_or_else(|| Local::now().date_naive());
        self.picker_month = base.with_day(1).unwrap_or(base);
        self.picker_from = None;
        self.popover = Some(Popover::Range);
    }

    /// 見出しに置く、期間を選ぶボタン
    pub(super) fn range_button(&self, cx: &mut gpui::Context<Self>) -> gpui::Stateful<Div> {
        let anchor = self.picker_anchor.clone();
        let open = self.popover == Some(Popover::Range);
        div()
            .id("range")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.0))
            .rounded_md()
            .border_1()
            .border_color(c(if open || self.pick.is_some() {
                theme::ACCENT
            } else {
                theme::BORDER
            }))
            .when(open, |d| d.bg(c(theme::ACCENT_BG)))
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(
                Icon::Calendar,
                if self.pick.is_some() {
                    theme::ACCENT
                } else {
                    theme::MUTED
                },
            ))
            .child(canvas(move |b, _, _| anchor.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .tooltip(tip("カレンダーで期間を選ぶ", None))
            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                if t.popover == Some(Popover::Range) {
                    t.popover = None;
                } else {
                    t.open_picker();
                }
                cx.notify();
            }))
    }

    /// 期間を選ぶカレンダー。1 回目のクリックで開始日、2 回目で終了日を決める
    pub(super) fn range_menu(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let b = self.picker_anchor.get();
        let month = self.picker_month;
        let first = month - Duration::days(month.weekday().num_days_from_monday() as i64);
        let days: Vec<NaiveDate> = (0..42).map(|i| first + Duration::days(i)).collect();
        let edges: Vec<f64> = (0..=42).map(|i| midnight(first + Duration::days(i))).collect();
        let counts = self.count_bins(&edges);
        let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
        let today = Local::now().date_naive();
        let picked = self.pick.map(|(a, b)| (local(a).date_naive(), local(b - 1.0).date_naive()));
        let from = self.picker_from;

        let shift = |months: i32| {
            move |t: &mut Self, _: &ClickEvent, _: &mut Window, cx: &mut gpui::Context<Self>| {
                let m = t.picker_month;
                let total = m.year() * 12 + m.month0() as i32 + months;
                t.picker_month =
                    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1).unwrap_or(m);
                cx.notify();
            }
        };
        let nav = |id: &'static str, icon: Icon, label: &'static str, f| {
            div()
                .id(id)
                .p(px(4.0))
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::HOVER)))
                .child(icons::icon(icon, theme::MUTED))
                .tooltip(tip(label, None))
                .on_click(cx.listener(f))
        };
        let head = div()
            .flex()
            .items_center()
            .px_1()
            .pb_1()
            .child(nav("pick-prev", Icon::ChevronLeft, "前の月", shift(-1)))
            .child(div().flex_1().text_center().text_sm().font_weight(FontWeight::SEMIBOLD).child(format!(
                "{}年{}月",
                month.year(),
                month.month()
            )))
            .child(nav("pick-next", Icon::ChevronRight, "次の月", shift(1)));

        let week_head = div().flex().children(
            [
                chrono::Weekday::Mon,
                chrono::Weekday::Tue,
                chrono::Weekday::Wed,
                chrono::Weekday::Thu,
                chrono::Weekday::Fri,
                chrono::Weekday::Sat,
                chrono::Weekday::Sun,
            ]
            .into_iter()
            .map(|w| div().w(px(34.0)).text_center().text_xs().text_color(c(theme::SUBTLE)).child(weekday_ja(w))),
        );
        let cell = |i: usize, d: NaiveDate| {
            let n = counts[i];
            let in_month = d.month() == month.month();
            let ends = from == Some(d) || picked.is_some_and(|(a, b)| from.is_none() && (d == a || d == b));
            let inside = from.is_none() && picked.is_some_and(|(a, b)| a <= d && d <= b);
            let fg = if ends {
                theme::TEXT
            } else if !in_month {
                theme::SUBTLE
            } else if d == today {
                theme::ACCENT
            } else {
                theme::TEXT
            };
            div()
                .id(("pick-day", i))
                .w(px(34.0))
                .h(px(32.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(2.0))
                .rounded_md()
                .cursor_pointer()
                .when(inside && !ends, |s| s.bg(c(theme::ACCENT_BG)))
                .when(ends, |s| s.bg(c(theme::SELECT)))
                .hover(|s| s.border_1().border_color(c(theme::ACCENT)))
                .text_xs()
                .text_color(c(fg))
                .when(d == today, |s| s.font_weight(FontWeight::SEMIBOLD))
                .child(d.day().to_string())
                // その日のイベントの多さを、下の小さな棒の長さで示す
                .child(
                    div()
                        .h(px(3.0))
                        .w(px(if n == 0 {
                            0.0
                        } else {
                            4.0 + 18.0 * (n as f32 / max).sqrt()
                        }))
                        .rounded_sm()
                        .bg(c(theme::ACCENT).opacity(if in_month { 0.9 } else { 0.4 })),
                )
                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    match t.picker_from.take() {
                        Some(a) => {
                            t.pick_days(a, d, w);
                            t.popover = None;
                        }
                        None => t.picker_from = Some(d),
                    }
                    cx.notify();
                }))
        };
        let grid = div()
            .flex()
            .flex_col()
            .children((0..6).map(|r| div().flex().children((0..7).map(|k| cell(r * 7 + k, days[r * 7 + k])))));
        let hint = match (from, picked) {
            (Some(a), _) => format!("{} から（終了日を選ぶ）", a.format("%Y/%-m/%-d")),
            (None, Some((a, b))) => {
                format!("{} – {}", a.format("%Y/%-m/%-d"), b.format("%Y/%-m/%-d"))
            }
            (None, None) => "開始日と終了日を選ぶ".to_string(),
        };
        let quick = |id: &'static str, label: &'static str, back: i64| {
            div()
                .id(id)
                .px_2()
                .py(px(2.0))
                .rounded_md()
                .border_1()
                .border_color(c(theme::BORDER))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::HOVER)))
                .child(label)
                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    let today = Local::now().date_naive();
                    t.pick_days(today - Duration::days(back), today, w);
                    t.popover = None;
                    cx.notify();
                }))
        };
        let foot = div()
            .flex()
            .flex_col()
            .gap_1p5()
            .pt_1p5()
            .mt_1()
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_xs()
            .child(div().text_color(c(theme::MUTED)).child(hint))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(quick("pick-7", "直近 7 日", 6))
                    .child(quick("pick-30", "直近 30 日", 29))
                    .child(quick("pick-90", "直近 90 日", 89)),
            );
        deferred(
            anchored().position(point(b.right(), b.bottom() + px(4.0))).anchor(Corner::TopRight).child(
                div()
                    .id("range-menu")
                    .occlude()
                    .p_2()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .bg(c(theme::ELEVATED))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .shadow_lg()
                    .text_color(c(theme::TEXT))
                    .child(head)
                    .child(week_head)
                    .child(grid)
                    .child(foot),
            ),
        )
        .with_priority(2)
        .into_any_element()
    }

    /// ミニマップの x 座標（ウィンドウ座標）の時刻を表示範囲の中央にする
    fn minimap_seek(&mut self, x: Pixels, window: &Window) {
        let b = self.minimap_bounds.get();
        let Some((a, e)) = self.minimap_span() else {
            return;
        };
        let w = f32::from(b.size.width);
        if w <= 0.0 {
            return;
        }
        let f = ((f32::from(x - b.origin.x)) / w).clamp(0.0, 1.0) as f64;
        let span = self.spp * self.plot_width(window) as f64;
        self.t0 = a + (e - a) * f - span / 2.0;
    }

    /// タイムラインの上に置く帯。期間全体のイベントの分布と、今見ている範囲を示す
    pub(super) fn minimap(&mut self, window: &Window, cx: &mut gpui::Context<Self>) -> Div {
        let span = self.minimap_span();
        let plot_w = self.plot_width(window);
        let bins = ((plot_w - 16.0) / BIN_W).max(1.0) as usize;
        let counts = match span {
            Some((a, e)) => {
                let key = (a.to_bits(), e.to_bits(), bins, self.data_gen);
                match &self.minimap_cache {
                    Some((k, v)) if *k == key => v.clone(),
                    _ => {
                        let edges: Vec<f64> = (0..=bins).map(|i| a + (e - a) * i as f64 / bins as f64).collect();
                        let v = self.count_bins(&edges);
                        self.minimap_cache = Some((key, v.clone()));
                        v
                    }
                }
            }
            None => Vec::new(),
        };
        let view = (self.t0, self.t0 + self.spp * plot_w as f64);
        let label = match (self.pick, span) {
            (Some((a, b)), _) => {
                let (a, b) = (local(a), local(b - 1.0));
                format!("{} – {}", a.format("%Y/%-m/%-d"), b.format("%Y/%-m/%-d"))
            }
            (None, Some(_)) => "全期間".to_string(),
            (None, None) => String::new(),
        };
        let bounds_cell = self.minimap_bounds.clone();
        let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
        let map = canvas(
            move |b, _, _| bounds_cell.set(b),
            move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                window.paint_quad(fill(bounds, c(theme::PANEL)).corner_radii(px(4.0)));
                let n = counts.len().max(1) as f32;
                let bw = w / n;
                for (i, &k) in counts.iter().enumerate() {
                    if k == 0 {
                        continue;
                    }
                    let bh = ((k as f32 / max).sqrt() * (h - 4.0)).max(2.0);
                    let r = Bounds::new(
                        bounds.origin + point(px(i as f32 * bw), px(h - 2.0 - bh)),
                        size(px(bw.max(1.0)), px(bh)),
                    );
                    window.paint_quad(fill(r, c(theme::ACCENT).opacity(0.75)));
                }
                // 今見ている範囲
                if let Some((a, e)) = span {
                    let x0 = ((view.0 - a) / (e - a)) as f32 * w;
                    let x1 = ((view.1 - a) / (e - a)) as f32 * w;
                    let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
                    if x1 > x0 || x0 > 0.0 && x0 < w {
                        let x1 = x1.max(x0 + 2.0).min(w);
                        let r = Bounds::new(bounds.origin + point(px(x0), px(0.0)), size(px(x1 - x0), px(h)));
                        window.paint_quad(
                            fill(r, gpui::white().opacity(0.08))
                                .corner_radii(px(3.0))
                                .border_widths(px(1.0))
                                .border_color(gpui::white().opacity(0.7)),
                        );
                    }
                }
            },
        )
        .size_full();
        let clear = self.pick.is_some().then(|| {
            div()
                .id("pick-clear")
                .flex_none()
                .p(px(3.0))
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::HOVER)))
                .child(icons::icon(Icon::X, theme::MUTED).size_3())
                .tooltip(tip("選んだ期間を解除", None))
                .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.pick = None;
                    cx.notify();
                }))
        });
        div()
            .h(px(MINIMAP_H))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .w(px(self.label_w))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .children(clear),
            )
            .child(
                div()
                    .id("minimap")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px_2()
                    .py(px(4.0))
                    .cursor_pointer()
                    .child(map)
                    .tooltip(tip("クリック・ドラッグで表示位置を移動", None))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|t, e: &MouseDownEvent, w, cx| {
                            t.minimap_drag = true;
                            t.minimap_seek(e.position.x, w);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|t, e: &MouseMoveEvent, w, cx| {
                        if !t.minimap_drag {
                            return;
                        }
                        if e.pressed_button != Some(MouseButton::Left) {
                            t.minimap_drag = false;
                            return;
                        }
                        t.minimap_seek(e.position.x, w);
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|t, _, _, _| t.minimap_drag = false)),
            )
    }
}
