//! 期間の表記から開くポップオーバー（直近の期間の一覧と、日付の範囲を選ぶカレンダー。カレンダーの表示では日付へ移動）と、
//! 期間全体のどこにイベントがあるかを示すミニマップ

use std::rc::Rc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{
    AnyElement, Bounds, ClickEvent, Corner, Div, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Window, anchored, canvas, deferred, div, fill, point, prelude::*, px, size,
};

use super::header::{PRESETS, weekday_ja};
use super::widgets::tip;
use super::{MINIMAP_H, Popover, Rireki, local, midnight};
use crate::icons::{self, Icon};
use crate::histogram::count_bins;
use crate::prefs::Mode;
use crate::theme::{self, c};

/// ミニマップの棒 1 本の幅（px）
const BIN_W: f32 = 2.0;


impl Rireki {
    /// 表示中のセッションのイベントを、`edges` で区切った区間ごとに数える（開始時刻で数える）
    fn count_bins(&self, edges: &[f64]) -> Vec<u32> {
        count_bins(&self.store, &self.order, edges)
    }

    /// ミニマップが表す期間。選んだ期間があればそれ、なければ表示中のセッション全体
    fn minimap_span(&self) -> Option<(f64, f64)> {
        self.pick.or_else(|| self.order_bounds().and_then(|(a, b)| {
            (b > a).then(|| {
                let pad = (b - a) * 0.02;
                (a - pad, b + pad)
            })
        }))
    }

    /// 日付の範囲を表示期間にする（`a`〜`b` の両日を含む）
    pub(super) fn pick_days(&mut self, a: NaiveDate, b: NaiveDate, window: &Window) {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let (start, end) = (midnight(a), midnight(b + Duration::days(1)));
        self.mode = Mode::Timeline;
        self.pick = Some((start, end));
        self.set_range(start, end, window);
        self.persist();
    }

    /// 期間を選ぶポップオーバーを開く。今見ている月から始める
    pub(super) fn open_picker(&mut self) {
        let base = if self.mode == Mode::Timeline {
            self.pick.map_or_else(|| local(self.t0).date_naive(), |(a, _)| local(a).date_naive())
        } else {
            self.day0
        };
        self.picker_month = base.with_day(1).unwrap_or(base);
        self.picker_from = None;
        self.popover = Some(Popover::Range);
    }

    /// 期間の表記から開くポップオーバー。タイムラインでは直近の期間の一覧と、日付の範囲を選ぶカレンダーを並べる。
    /// カレンダーの表示では、押した日を含む日・週・月へ移動する
    pub(super) fn range_menu(&self, window: &Window, cx: &mut gpui::Context<Self>) -> AnyElement {
        let b = self.picker_anchor.get();
        let timeline = self.mode == Mode::Timeline;
        let month = self.picker_month;
        let first = month - Duration::days(month.weekday().num_days_from_monday() as i64);
        let days: Vec<NaiveDate> = (0..42).map(|i| first + Duration::days(i)).collect();
        let edges: Vec<f64> = (0..=42).map(|i| midnight(first + Duration::days(i))).collect();
        let counts = self.count_bins(&edges);
        let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
        let today = Local::now().date_naive();
        // 強調する範囲。タイムラインでは選んだ期間、カレンダーでは表示中の日・週（月は表全体なので強調しない）
        let picked = match self.mode {
            Mode::Timeline => self.pick.map(|(a, b)| (local(a).date_naive(), local(b - 1.0).date_naive())),
            Mode::Day => Some((self.day0, self.day0)),
            Mode::Week => Some((self.week_start(), self.week_start() + Duration::days(6))),
            Mode::Month => None,
        };
        let from = self.picker_from.filter(|_| timeline);

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
                // 範囲の内側は角を丸めず、続いた帯に見せる
                .when(inside && !ends, |s| s.rounded_none().bg(c(theme::ACCENT_BG)))
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
                    if t.mode != Mode::Timeline {
                        t.day0 = d;
                        t.popover = None;
                    } else {
                        match t.picker_from.take() {
                            Some(a) => {
                                t.pick_days(a, d, w);
                                t.popover = None;
                            }
                            None => t.picker_from = Some(d),
                        }
                    }
                    cx.notify();
                }))
        };
        let grid = div()
            .flex()
            .flex_col()
            .children((0..6).map(|r| div().flex().children((0..7).map(|k| cell(r * 7 + k, days[r * 7 + k])))));
        let hint = match (self.mode, from, picked) {
            (Mode::Day, ..) => "日付を選ぶと、その日へ移動".to_string(),
            (Mode::Week, ..) => "日付を選ぶと、その週へ移動".to_string(),
            (Mode::Month, ..) => "日付を選ぶと、その月へ移動".to_string(),
            (Mode::Timeline, Some(a), _) => format!("{} から（終了日を選ぶ）", a.format("%Y/%-m/%-d")),
            (Mode::Timeline, None, Some((a, b))) => {
                format!("{} – {}", a.format("%Y/%-m/%-d"), b.format("%Y/%-m/%-d"))
            }
            (Mode::Timeline, None, None) => "開始日と終了日を選ぶ".to_string(),
        };
        let clear = (timeline && self.pick.is_some()).then(|| {
            div()
                .id("pick-clear-menu")
                .flex_none()
                .px_2()
                .py(px(2.0))
                .rounded_md()
                .border_1()
                .border_color(c(theme::BORDER))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::HOVER)))
                .child("解除")
                .tooltip(tip("選んだ期間の指定を解除（ミニマップを全期間に戻す）", None))
                .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                    t.pick = None;
                    t.picker_from = None;
                    cx.notify();
                }))
        });
        let foot = div()
            .flex()
            .items_center()
            .gap_2()
            .pt_1p5()
            .mt_1()
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_xs()
            .child(div().flex_1().text_color(c(theme::MUTED)).child(hint))
            .children(clear);
        let calendar = div().flex().flex_col().child(head).child(week_head).child(grid).child(foot);
        // 直近の期間。押すと現在時刻までのその長さを表示する
        let presets = timeline.then(|| {
            let active = self.active_preset(window);
            div()
                .w(px(132.0))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .pr_2()
                .mr_2()
                .border_r_1()
                .border_color(c(theme::BORDER))
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .pb_1p5()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(theme::MUTED))
                        .child("表示する期間"),
                )
                .children(PRESETS.iter().enumerate().map(|(i, &(label, _))| {
                    let on = active == Some(i);
                    div()
                        .id(("preset", i))
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(28.0))
                        .px_2()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::HOVER)))
                        // 全期間は直近の期間と分けて置く
                        .when(i + 1 == PRESETS.len(), |d| d.mt_1())
                        .when(on, |d| d.bg(c(theme::ACCENT_BG)).font_weight(FontWeight::SEMIBOLD))
                        .child(div().flex_1().child(label))
                        .when(on, |d| d.child(icons::icon(Icon::Check, theme::ACCENT)))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            t.apply_preset(i, w);
                            t.popover = None;
                            cx.notify();
                        }))
                }))
        });
        deferred(
            anchored()
                .position(point(b.left(), b.bottom() + px(4.0)))
                .anchor(Corner::TopLeft)
                .snap_to_window_with_margin(px(8.0))
                .child(
                    div()
                        .id("range-menu")
                        .occlude()
                        .p_2()
                        .flex()
                        .rounded_lg()
                        .bg(c(theme::ELEVATED))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .shadow_lg()
                        .text_color(c(theme::TEXT))
                        .children(presets)
                        .child(calendar),
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
        self.clamp_timeline(window);
    }

    /// タイムラインの上に置く帯。期間全体のイベントの分布と、今見ている範囲を示す
    pub(super) fn minimap(&mut self, window: &Window, cx: &mut gpui::Context<Self>) -> Div {
        let span = self.minimap_span();
        let plot_w = self.plot_width(window);
        let bins = ((plot_w - 16.0) / BIN_W).max(1.0) as usize;
        let (counts, max) = match span {
            Some((a, e)) => {
                let key = (a.to_bits(), e.to_bits(), bins, self.data_gen);
                match &self.minimap_cache {
                    Some((k, v, max)) if *k == key => (Some(v.clone()), *max),
                    _ => {
                        let edges: Vec<f64> = (0..=bins).map(|i| a + (e - a) * i as f64 / bins as f64).collect();
                        let v = Rc::new(self.count_bins(&edges));
                        let max = v.iter().copied().max().unwrap_or(0).max(1) as f32;
                        self.minimap_cache = Some((key, v.clone(), max));
                        (Some(v), max)
                    }
                }
            }
            None => (None, 1.0),
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
        let measurements = self.profile.as_ref().and_then(|p| p.measurements.clone());
        let unbatched = self.profile.as_ref().is_some_and(|p| p.unbatched_minimap);
        let map = canvas(
            move |b, _, _| bounds_cell.set(b),
            move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                use std::hash::{DefaultHasher, Hash, Hasher};
                let counts = counts.as_ref().map_or(&[][..], |v| v.as_slice());
                let started = measurements.as_ref().map(|_| std::time::Instant::now());
                let cpu_started = measurements.as_ref().and_then(|_| super::profile::thread_cpu_ns());
                let mut hash = measurements.as_ref().map(|_| DefaultHasher::new());
                let mut backgrounds = Vec::new();
                let mut quads = 0usize;
                let mut paint = |window: &mut Window, quad: gpui::PaintQuad| {
                    if let Some(hash) = &mut hash {
                        quad.bounds.hash(hash);
                        [quad.corner_radii.top_left, quad.corner_radii.top_right,
                            quad.corner_radii.bottom_right, quad.corner_radii.bottom_left].hash(hash);
                        [quad.border_widths.top, quad.border_widths.right,
                            quad.border_widths.bottom, quad.border_widths.left].hash(hash);
                        quad.border_color.hash(hash);
                        quad.border_style.hash(hash);
                        let index = backgrounds.iter().position(|b| *b == quad.background).unwrap_or_else(|| {
                            serde_json::to_vec(&quad.background).unwrap().hash(hash);
                            backgrounds.push(quad.background);
                            backgrounds.len() - 1
                        });
                        index.hash(hash);
                    }
                    quads += 1;
                    window.paint_quad(quad);
                };
                let mut draw = |window: &mut Window| {
                    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    paint(window, fill(bounds, c(theme::PANEL)).corner_radii(px(4.0)));
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
                        paint(window, fill(r, c(theme::ACCENT).opacity(0.75)));
                    }
                    // 今見ている範囲
                    if let Some((a, e)) = span {
                        let x0 = ((view.0 - a) / (e - a)) as f32 * w;
                        let x1 = ((view.1 - a) / (e - a)) as f32 * w;
                        let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
                        if x1 > x0 || x0 > 0.0 && x0 < w {
                            let x1 = x1.max(x0 + 2.0).min(w);
                            let r = Bounds::new(bounds.origin + point(px(x0), px(0.0)), size(px(x1 - x0), px(h)));
                            paint(window,
                                fill(r, gpui::white().opacity(0.08))
                                    .corner_radii(px(3.0))
                                    .border_widths(px(1.0))
                                    .border_color(gpui::white().opacity(0.7)),
                            );
                        }
                    }
                };
                if unbatched {
                    draw(window);
                } else {
                    // 背景・棒・選択範囲はすべてQuad。GPUIの安定ソートで挿入順を保ち、
                    // 数百本の棒の境界木への登録を一つにまとめる。
                    window.paint_layer(bounds, draw);
                }
                if let Some((measurements, started)) = measurements.as_ref().zip(started) {
                    let mut value = measurements.get();
                    value.minimap_ms += started.elapsed().as_secs_f64() * 1000.0;
                    value.minimap_cpu_ms += super::profile::cpu_delta_ms(cpu_started).unwrap_or(0.0);
                    value.minimap_calls += 1;
                    value.minimap_quads += quads;
                    value.minimap_hash = hash.map_or(0, |mut hash| { quads.hash(&mut hash); hash.finish() });
                    measurements.set(value);
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
                            let before = t.t0;
                            t.minimap_seek(e.position.x, w);
                            cx.stop_propagation();
                            if t.t0 != before { cx.notify(); }
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
                        let before = t.t0;
                        t.minimap_seek(e.position.x, w);
                        if t.t0 != before { cx.notify(); }
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|t, _, _, _| t.minimap_drag = false)),
            )
    }
}
