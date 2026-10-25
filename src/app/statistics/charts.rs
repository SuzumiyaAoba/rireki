//! 統計の推移（読み込み元ごとに積んだ列）と時間帯（曜日 × 時刻の濃淡）。
//! 列とマスは 1 つの canvas に描き、ポインタの位置から当たり判定する（要素を数百に増やさない）

use std::cell::Cell;
use std::rc::Rc;

use chrono::Datelike;
use gpui::{
    Bounds, ClickEvent, Context, Corners, Div, FontWeight, MouseMoveEvent, Pixels, Point, canvas, div, fill, point,
    prelude::*, px, relative, size,
};

use super::{HoursView, Hover, Metric, MetricFormat, Rireki, card, card_title, percent, series, swatch};
use crate::app::header::{Choice, weekday_ja};
use crate::prefs::SourceFilter;
use crate::statistics::{Summary, Unit};
use crate::theme::{self, c};

/// 推移の描画領域の高さと、その下の日付の帯の高さ
const PLOT_H: f32 = 180.0;
const AXIS_H: f32 = 22.0;
/// 目盛りの数字の列の幅
const GUTTER: f32 = 48.0;
/// 列の最大の太さ。細い列が多いときは隙間を詰める
const BAR_MAX: f32 = 24.0;
/// 日付の表示 1 つに取る幅
const LABEL_W: f32 = 56.0;
/// 時間帯の 1 マスの高さの範囲。幅に合わせてほぼ正方形にする
const CELL_H: (f32, f32) = (16.0, 26.0);
/// 時間帯の曜日の列、曜日ごとの合計の列の幅、時ごとの合計の高さ、マスの間隔
const DAY_W: f32 = 16.0;
const TOTAL_W: f32 = 48.0;
const MARGIN_H: f32 = 24.0;
const GRID_GAP: f32 = 3.0;
pub(super) const WEEKDAYS: [&str; 7] = ["月", "火", "水", "木", "金", "土", "日"];

/// canvas に描いた位置（ウィンドウ座標）。描画時に覚え、ポインタの当たり判定に使う
pub(super) type Area = Rc<Cell<Bounds<Pixels>>>;

impl Rireki {
    pub(super) fn set_statistics_hover(&mut self, hover: Option<Hover>, cx: &mut Context<Self>) {
        if self.statistics_state.hover != hover {
            self.statistics_state.hover = hover;
            cx.notify();
        }
    }

    /// ポインタがグラフから出たら、そのグラフのホバーだけを消す（隣のグラフに入った後で届いても消さない）
    pub(super) fn leave_statistics_chart(&mut self, owns: fn(&Hover) -> bool, cx: &mut Context<Self>) {
        if self.statistics_state.hover.as_ref().is_some_and(owns) {
            self.set_statistics_hover(None, cx);
        }
    }

    /// 読み込み元の並び（Claude, Codex）のうち、絞り込みで表示しているもの
    pub(super) fn shown_sources(&self) -> &'static [usize] {
        match self.source {
            SourceFilter::All => &[0, 1],
            SourceFilter::Claude => &[0],
            SourceFilter::Codex => &[1],
        }
    }

    /// 期間内の推移。列を押すとその日（週・月）をタイムラインで開く
    pub(super) fn trend_card(&self, s: &Summary, inner: f32, cx: &mut Context<Self>) -> Div {
        let metric = self.statistics_state.metric;
        let sources = self.shown_sources();
        let values: Vec<[f64; 2]> =
            s.activity.iter().map(|b| [metric.amount(&b.sources[0]), metric.amount(&b.sources[1])]).collect();
        let n = values.len().max(1);
        let peak = values.iter().map(|v| v[0] + v[1]).fold(0.0, f64::max);
        let ticks = ticks(peak, metric.count());
        let top = *ticks.last().unwrap();
        let hover = match self.statistics_state.hover {
            Some(Hover::Bucket(i)) if i < values.len() => Some(i),
            _ => None,
        };
        let unit = match s.unit {
            Unit::Day => "日ごと",
            Unit::Week => "週ごと",
            Unit::Month => "月ごと",
        };
        // ポインタを重ねた列の値。重ねていなければ期間の合計を凡例に添える
        let (head, shown) = match hover {
            Some(i) => (Some(bucket_label(s, i)), values[i]),
            None => (None, [metric.total(&s.sources[0]), metric.total(&s.sources[1])]),
        };
        let legend = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap_x_3()
            .gap_y_1()
            .text_xs()
            .when_some(head, |d, head| d.child(div().font_weight(FontWeight::SEMIBOLD).child(head)))
            .children(sources.iter().map(|&i| {
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(swatch(series(i)))
                    .child(div().text_color(c(theme::MUTED)).child(["Claude", "Codex"][i]))
                    .child(metric.format(shown[i]))
            }))
            .when(sources.len() > 1, |d| {
                d.child(
                    div()
                        .flex()
                        .gap_1p5()
                        .child(div().text_color(c(theme::MUTED)).child("計"))
                        .child(metric.format(shown[0] + shown[1])),
                )
            });
        // 日付は右端（今日）からそろえて間引く
        let slot = (inner - 34.0 - GUTTER) / n as f32;
        let every = (LABEL_W / slot).ceil().max(1.0) as usize;
        let y = |v: f64| PLOT_H - (v / top) as f32 * PLOT_H;
        let gutter =
            div().relative().flex_none().w(px(GUTTER)).h(px(PLOT_H + AXIS_H)).children(ticks.iter().map(|&t| {
                div()
                    .absolute()
                    .top(px(y(t) - 7.0))
                    .right(px(8.0))
                    .text_size(px(11.0))
                    .line_height(px(14.0))
                    .text_color(c(theme::MUTED))
                    .whitespace_nowrap()
                    .child(metric.tick(t))
            }));
        let count = values.len();
        let labels = (0..count).filter(|i| (count - 1 - i).is_multiple_of(every)).map(|i| {
            div()
                .absolute()
                .top(px(PLOT_H + 6.0))
                .left(relative(i as f32 / n as f32))
                .w(relative(1.0 / n as f32))
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .text_size(px(11.0))
                .text_color(c(theme::MUTED))
                .child(axis_label(s, i, slot >= 72.0))
        });
        let area = self.statistics_state.trend_area.clone();
        let plot = canvas(
            {
                let area = area.clone();
                move |b, _, _| area.set(b)
            },
            move |b, _, window, _| paint_trend(b, &values, &ticks, top, hover, window),
        )
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(PLOT_H));
        let ranges: Vec<_> = s.activity.iter().map(|b| (b.from, b.to)).collect();
        let at = move |area: &Area, x: Pixels| {
            let b = area.get();
            let w = f32::from(b.size.width);
            let f = f32::from(x - b.origin.x) / w;
            (w > 0.0 && (0.0..1.0).contains(&f)).then(|| ((f * n as f32) as usize).min(n - 1))
        };
        let plot_area = div()
            .id("statistics-trend")
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(PLOT_H + AXIS_H))
            .cursor_pointer()
            .child(plot)
            .children(labels)
            .on_mouse_move({
                let area = area.clone();
                cx.listener(move |t, e: &MouseMoveEvent, _, cx| {
                    t.set_statistics_hover(at(&area, e.position.x).map(Hover::Bucket), cx)
                })
            })
            .on_hover(cx.listener(|t, on: &bool, _, cx| {
                if !on {
                    t.leave_statistics_chart(|h| matches!(h, Hover::Bucket(_)), cx);
                }
            }))
            .on_click(cx.listener(move |t, e: &ClickEvent, w, cx| {
                let Some(&(from, to)) = at(&area, e.position().x).and_then(|i| ranges.get(i)) else {
                    return;
                };
                t.statistics_open = false;
                t.statistics_state.hover = None;
                t.pick_days(from, to, w);
                cx.notify();
            }));
        card()
            .child(
                card_title(
                    format!("{}の推移", metric.label()),
                    format!("{unit} · 指標は上のタイルで切り替え · 列を押すとタイムラインで開く"),
                )
                .flex_wrap()
                .child(legend),
            )
            .child(div().flex().child(gutter).child(plot_area))
    }

    /// 曜日 × 時刻の濃淡を円形・表形式で表示。期間内の日をローカル時間で重ねる
    pub(super) fn hours_card(&self, s: &Summary, width: f32, cx: &mut Context<Self>) -> Div {
        if self.statistics_state.hours_view == HoursView::Radial {
            return self.radial_hours_card(s, width, cx);
        }
        let metric = self.statistics_state.metric;
        let cell_w = (width - 34.0 - DAY_W - TOTAL_W - 25.0 * GRID_GAP) / 24.0;
        let cell_h = (cell_w * 1.2).clamp(CELL_H.0, CELL_H.1);
        let grid_h = MARGIN_H + GRID_GAP + 7.0 * cell_h + 6.0 * GRID_GAP;
        let values: Vec<f64> = s.hours.iter().map(|a| metric.amount(a)).collect();
        let days = s.by_weekday.map(|a| metric.amount(&a));
        let hours = s.by_hour.map(|a| metric.amount(&a));
        let total = metric.total(&s.total);
        let hover = self.statistics_state.hover;
        let name = |i: usize| format!("{}曜 {}時台", WEEKDAYS[i / 24], i % 24);
        let value = |v: f64| {
            let share = if metric == Metric::Sessions || total <= 0.0 {
                String::new()
            } else {
                format!("（{}）", percent(v / total))
            };
            format!("{}{}{share}", metric.format(v), metric.unit())
        };
        let (head, shown) = match hover {
            Some(Hover::Hour(i)) if i < values.len() => (name(i), Some(values[i])),
            Some(Hover::Weekday(d)) if d < 7 => (format!("{}曜（終日）", WEEKDAYS[d]), Some(days[d])),
            Some(Hover::HourOfDay(h)) if h < 24 => (format!("{h}時台（全曜日）"), Some(hours[h])),
            _ => {
                let top =
                    (0..values.len()).filter(|&i| values[i] > 0.0).max_by(|&a, &b| values[a].total_cmp(&values[b]));
                (top.map(|i| format!("最多 {}", name(i))).unwrap_or_default(), top.map(|i| values[i]))
            }
        };
        let readout = div()
            .flex()
            .gap_1p5()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(head))
            .when_some(shown, |d, v| d.child(value(v)));
        let label = |text: String| div().text_size(px(11.0)).text_color(c(theme::MUTED)).child(text);
        let weekdays = div()
            .flex_none()
            .w(px(DAY_W))
            .pt(px(MARGIN_H + GRID_GAP))
            .flex()
            .flex_col()
            .gap(px(GRID_GAP))
            .children(WEEKDAYS.map(|d| label(d.into()).h(px(cell_h)).line_height(px(cell_h))));
        let area = self.statistics_state.hours_area.clone();
        let grid = canvas(
            {
                let area = area.clone();
                move |b, _, _| area.set(b)
            },
            move |b, _, window, _| paint_hours(b, cell_h, &values, &days, &hours, hover, window),
        )
        .size_full();
        let plot = div()
            .id("statistics-hours")
            .flex_1()
            .min_w_0()
            .h(px(grid_h))
            .child(grid)
            .on_mouse_move(cx.listener(move |t, e: &MouseMoveEvent, _, cx| {
                t.set_statistics_hover(hour_at(area.get(), cell_h, e.position), cx)
            }))
            .on_hover(cx.listener(|t, on: &bool, _, cx| {
                if !on {
                    t.leave_statistics_chart(
                        |h| matches!(h, Hover::Hour(_) | Hover::Weekday(_) | Hover::HourOfDay(_)),
                        cx,
                    );
                }
            }));
        // 時の目盛りはマスの左端にそろえる
        let axis = div().flex().gap(px(GRID_GAP)).child(div().flex_none().w(px(DAY_W))).child(
            div().relative().flex_1().h(px(14.0)).mr(px(TOTAL_W + GRID_GAP)).children(
                (0..24)
                    .step_by(3)
                    .map(|h| div().absolute().top_0().left(relative(h as f32 / 24.0)).child(label(h.to_string()))),
            ),
        );
        let scale = div()
            .flex()
            .items_center()
            .justify_end()
            .gap_1()
            .text_xs()
            .text_color(c(theme::MUTED))
            .child("少")
            .children(theme::HEAT.iter().map(|&h| div().size(px(10.0)).rounded(px(2.0)).bg(c(h))))
            .child("多");
        card()
            .child(
                card_title("時間帯", format!("{} · 曜日 × 時刻（ローカル時間）", metric.label()))
                    .flex_wrap()
                    .child(self.hours_view_switch(cx)),
            )
            .child(div().text_xs().child(readout))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().flex().gap(px(GRID_GAP)).child(weekdays).child(plot))
                    .child(axis),
            )
            .child(scale)
    }

    pub(super) fn hours_view_switch(&self, cx: &mut Context<Self>) -> Div {
        self.tool_segmented(
            "statistics-hours-view",
            &[
                Choice { value: HoursView::Radial, label: "円形", tip: "時計のように時間帯を表示", key: None },
                Choice { value: HoursView::Grid, label: "表", tip: "曜日 × 時刻の表で表示", key: None },
            ],
            self.statistics_state.hours_view,
            cx,
            |t, view, _, _| {
                t.statistics_state.hours_view = view;
                t.statistics_state.hover = None;
            },
        )
    }
}

/// 推移の目盛り線・ホバーの帯・列。列は Claude を基線に置き、Codex を上に積む
fn paint_trend(
    b: Bounds<Pixels>,
    values: &[[f64; 2]],
    ticks: &[f64],
    top: f64,
    hover: Option<usize>,
    window: &mut gpui::Window,
) {
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let n = values.len().max(1) as f32;
    let slot = w / n;
    let gap = if slot >= 10.0 { 2.0 } else { 1.0 };
    let bar = (slot - gap).clamp(1.0, BAR_MAX);
    let rect = |x: f32, y: f32, w: f32, h: f32| Bounds::new(b.origin + point(px(x), px(y)), size(px(w), px(h)));
    let rounded_top =
        |r: f32| Corners { top_left: px(r), top_right: px(r), bottom_left: px(0.0), bottom_right: px(0.0) };
    if let Some(i) = hover {
        let band = (bar + 16.0).min(slot);
        let x = i as f32 * slot + (slot - band) / 2.0;
        window.paint_quad(fill(rect(x, 0.0, band, h), c(theme::HOVER)).corner_radii(rounded_top(4.0)));
    }
    for &t in ticks {
        let y = (h - (t / top) as f32 * h).min(h - 1.0);
        window.paint_quad(fill(rect(0.0, y, w, 1.0), c(if t == 0.0 { theme::BORDER } else { theme::GRID })));
    }
    for (i, v) in values.iter().enumerate() {
        let x = i as f32 * slot + (slot - bar) / 2.0;
        let heights: Vec<(usize, f32)> =
            [0, 1].into_iter().filter(|&s| v[s] > 0.0).map(|s| (s, ((v[s] / top) as f32 * h).max(2.0))).collect();
        // 境目は面の色の隙間で分ける。隙間は上の区分から差し引き、全体の高さを保つ
        let mut base = h - 1.0;
        for (k, &(src, height)) in heights.iter().enumerate() {
            let (bottom, height) = if k == 0 { (base, height) } else { (base - 2.0, (height - 2.0).max(1.0)) };
            let q = fill(rect(x, bottom - height, bar, height), c(series(src)));
            window.paint_quad(if k + 1 == heights.len() { q.corner_radii(rounded_top(3.0)) } else { q });
            base = bottom - height;
        }
    }
}

/// 時間帯のマス目の (幅, マスと間隔を合わせた送り幅, 上端)。上に時ごと、右に曜日ごとの合計の帯を置く
fn hours_layout(b: Bounds<Pixels>) -> (f32, f32, f32) {
    let cells = f32::from(b.size.width) - TOTAL_W - GRID_GAP;
    (cells, (cells + GRID_GAP) / 24.0, MARGIN_H + GRID_GAP)
}

/// ポインタの位置にあるマス・時ごとの合計・曜日ごとの合計
fn hour_at(b: Bounds<Pixels>, cell_h: f32, p: Point<Pixels>) -> Option<Hover> {
    let (x, y) = (f32::from(p.x - b.origin.x), f32::from(p.y - b.origin.y));
    if x < 0.0 || y < 0.0 || x >= f32::from(b.size.width) || y >= f32::from(b.size.height) {
        return None;
    }
    let (cells, pitch, grid_top) = hours_layout(b);
    let col = ((x / pitch) as usize).min(23);
    if y < grid_top {
        return (x < cells).then_some(Hover::HourOfDay(col));
    }
    let day = (((y - grid_top) / (cell_h + GRID_GAP)) as usize).min(6);
    Some(if x < cells { Hover::Hour(day * 24 + col) } else { Hover::Weekday(day) })
}

fn paint_hours(
    b: Bounds<Pixels>,
    cell_h: f32,
    values: &[f64],
    days: &[f64; 7],
    hours: &[f64; 24],
    hover: Option<Hover>,
    window: &mut gpui::Window,
) {
    let peak = |v: &[f64]| v.iter().copied().fold(0.0, f64::max).max(f64::MIN_POSITIVE);
    let (max, day_max, hour_max) = (peak(values), peak(days), peak(hours));
    let (cells, pitch, grid_top) = hours_layout(b);
    let cell_w = pitch - GRID_GAP;
    let rect = |x: f32, y: f32, w: f32, h: f32| Bounds::new(b.origin + point(px(x), px(y)), size(px(w), px(h)));
    let mark = |on: bool| c(if on { theme::HEAT[4] } else { theme::HEAT[1] });
    // 時ごとの合計（上端の小さな列）
    for (h, &v) in hours.iter().enumerate() {
        let height = (v / hour_max) as f32 * MARGIN_H;
        if height > 0.0 {
            let on = hover == Some(Hover::HourOfDay(h));
            let q = fill(rect(h as f32 * pitch, MARGIN_H - height, cell_w, height), mark(on));
            window.paint_quad(q.corner_radii(Corners {
                top_left: px(2.0),
                top_right: px(2.0),
                bottom_left: px(0.0),
                bottom_right: px(0.0),
            }));
        }
    }
    for d in 0..7 {
        let y = grid_top + d as f32 * (cell_h + GRID_GAP);
        for h in 0..24 {
            let v = values[d * 24 + h];
            // 最大に対する割合を 5 段階に分ける。0 は空のマス
            let color =
                if v > 0.0 { theme::HEAT[((v / max * 5.0).ceil() as usize).clamp(1, 5) - 1] } else { theme::GRID };
            let q = fill(rect(h as f32 * pitch, y, cell_w, cell_h), c(color)).corner_radii(px(3.0));
            window.paint_quad(if hover == Some(Hover::Hour(d * 24 + h)) {
                q.border_widths(px(1.0)).border_color(c(theme::TEXT))
            } else {
                q
            });
        }
        // 曜日ごとの合計（右端の小さな棒）
        let width = (days[d] / day_max) as f32 * (TOTAL_W - 4.0);
        if width > 0.0 {
            let on = hover == Some(Hover::Weekday(d));
            let q = fill(rect(cells + GRID_GAP + 4.0, y + (cell_h - 8.0) / 2.0, width, 8.0), mark(on));
            window.paint_quad(q.corner_radii(Corners {
                top_left: px(0.0),
                top_right: px(2.0),
                bottom_left: px(0.0),
                bottom_right: px(2.0),
            }));
        }
    }
}

/// 0 から `max` までを 4 つ前後に分ける、きりのよい目盛り
fn ticks(max: f64, count: bool) -> Vec<f64> {
    let max = if max > 0.0 { max } else { 1.0 };
    let raw = max / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let mut step = [1.0, 2.0, 2.5, 5.0, 10.0].into_iter().map(|m| m * mag).find(|&s| s >= raw).unwrap_or(10.0 * mag);
    if count {
        // 件数は整数の目盛りにする（2.5 刻みは 3 刻みにしない）
        step = if step < 1.0 {
            1.0
        } else if step.fract() != 0.0 {
            step * 2.0
        } else {
            step
        };
    }
    let n = (max / step - 1e-9).ceil().max(1.0) as usize;
    (0..=n).map(|i| i as f64 * step).collect()
}

/// 日付の帯の短い表示。列が広ければ曜日を添える
fn axis_label(s: &Summary, i: usize, wide: bool) -> String {
    let b = &s.activity[i];
    match s.unit {
        Unit::Day if i + 1 == s.activity.len() => "今日".into(),
        Unit::Day if wide => format!("{}（{}）", b.from.format("%-m/%-d"), weekday_ja(b.from.weekday())),
        Unit::Day | Unit::Week => b.from.format("%-m/%-d").to_string(),
        Unit::Month if i == 0 || b.from.month() == 1 => b.from.format("%Y/%-m").to_string(),
        Unit::Month => b.from.format("%-m月").to_string(),
    }
}

/// 読み取り値に出す列の期間
fn bucket_label(s: &Summary, i: usize) -> String {
    let b = &s.activity[i];
    match s.unit {
        Unit::Day => format!("{}（{}）", b.from.format("%-m月%-d日"), weekday_ja(b.from.weekday())),
        Unit::Week if b.from == b.to => b.from.format("%-m/%-d").to_string(),
        Unit::Week => format!("{} – {}", b.from.format("%-m/%-d"), b.to.format("%-m/%-d")),
        Unit::Month => b.from.format("%Y年%-m月").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_cover_the_peak_with_round_steps() {
        assert_eq!(ticks(0.0, true), [0.0, 1.0]);
        assert_eq!(ticks(3.0, true), [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(ticks(9.0, true), [0.0, 5.0, 10.0]);
        assert_eq!(ticks(1_116_190_000.0, true), [0.0, 5e8, 1e9, 1.5e9]);
        assert_eq!(ticks(40.0, true), [0.0, 10.0, 20.0, 30.0, 40.0]);
        assert_eq!(ticks(1.8, false), [0.0, 0.5, 1.0, 1.5, 2.0]);
        assert_eq!(ticks(250.0, false), [0.0, 100.0, 200.0, 300.0]);
    }

    #[test]
    fn hour_grid_hit_testing_matches_the_painted_layout() {
        // 送り幅がちょうど 20px になる幅
        let b = Bounds::new(point(px(100.0), px(50.0)), size(px(TOTAL_W + 24.0 * 20.0), px(300.0)));
        let cell_h = 20.0;
        let at = |x: f32, y: f32| hour_at(b, cell_h, point(px(100.0 + x), px(50.0 + y)));
        let top = MARGIN_H + GRID_GAP;
        assert_eq!(at(1.0, 1.0), Some(Hover::HourOfDay(0)));
        assert_eq!(at(20.0 * 23.0 + 1.0, MARGIN_H - 1.0), Some(Hover::HourOfDay(23)));
        assert_eq!(at(20.0 * 9.0 + 5.0, top + 1.0), Some(Hover::Hour(9)));
        assert_eq!(at(20.0 * 14.0 + 5.0, top + (cell_h + GRID_GAP) * 4.0 + 5.0), Some(Hover::Hour(4 * 24 + 14)));
        assert_eq!(at(24.0 * 20.0 + 10.0, top + (cell_h + GRID_GAP) * 6.0 + 5.0), Some(Hover::Weekday(6)));
        assert_eq!(at(24.0 * 20.0 + 10.0, 2.0), None);
        assert_eq!(at(-1.0, 10.0), None);
    }
}
