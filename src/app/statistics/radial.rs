//! 時計型の時間帯ヒートマップ。内側から月〜日の 7 本、時計回りに 0〜23 時の 24 区分。
//! 色ごとにパスをまとめ、描画と当たり判定で同じ寸法を使う。

use std::f32::consts::TAU;

use gpui::{
    Bounds, Context, Div, ElementId, FontWeight, MouseMoveEvent, Path, PathBuilder, Pixels, Point, Window, canvas, div,
    point, prelude::*, px,
};

use super::charts::WEEKDAYS;
use super::{Hover, Metric, MetricFormat, Rireki, card, card_title, percent};
use crate::app::widgets::tip;
use crate::statistics::{HOURS, Summary};
use crate::theme::{self, c};

const MAX_SIZE: f32 = 360.0;
const AXIS_PAD: f32 = 24.0;
const RING_GAP: f32 = 2.0;
const HOUR_ANGLE: f32 = TAU / 24.0;
const ANGLE_GAP: f32 = 0.028;
/// 使用量が 0 の区分も、時計の輪郭が読める暗い青で描く。
const EMPTY: u32 = 0x16233f;

/// 直前の塗りと選択枠だけを保持する。位置も含め、同じ座標のパスだけを再利用する。
#[derive(Default)]
pub(super) struct Cache {
    geometry: Option<Geometry>,
}

struct Geometry {
    bounds: [u32; 4],
    /// 存在しない区分は255。値が変わっても色の段階が同じなら塗りは同じ。
    levels: [u8; HOURS],
    fills: [Option<Path<Pixels>>; 6],
    outline: Option<(Option<Hover>, Option<Path<Pixels>>)>,
}

impl Cache {
    fn prepare(&mut self, b: Bounds<Pixels>, values: &[f64], peak: f64, hover: Option<Hover>) -> Option<&Geometry> {
        let Some(layout) = Layout::new(b) else {
            self.geometry = None;
            return None;
        };
        let bounds = [b.origin.x, b.origin.y, b.size.width, b.size.height].map(|v| f32::from(v).to_bits());
        let mut levels = [255; HOURS];
        for (slot, &value) in levels.iter_mut().zip(values) {
            *slot = if value > 0.0 && peak > 0.0 {
                ((value / peak * 5.0).ceil() as usize).clamp(1, 5) as u8
            } else {
                0
            };
        }
        if self.geometry.as_ref().is_none_or(|g| g.bounds != bounds || g.levels != levels) {
            // 古い頂点を先に解放し、差し替えの間に二組を保持しない。
            self.geometry = None;
            let mut fills: [PathBuilder; 6] = std::array::from_fn(|_| PathBuilder::fill());
            let mut used = [false; 6];
            for (i, &level) in levels.iter().take_while(|&&level| level != 255).enumerate() {
                layout.sector(&mut fills[level as usize], i / 24, i % 24);
                used[level as usize] = true;
            }
            let fills = std::array::from_fn({
                let mut paths = fills.into_iter();
                move |i| {
                    let path = paths.next().unwrap();
                    used[i].then(|| path.build().ok()).flatten()
                }
            });
            self.geometry = Some(Geometry { bounds, levels, fills, outline: None });
        }
        let geometry = self.geometry.as_mut().unwrap();
        let hover = match hover {
            Some(Hover::Hour(_) | Hover::Weekday(_) | Hover::HourOfDay(_)) => hover,
            _ => None,
        };
        if geometry.outline.as_ref().is_none_or(|(key, _)| *key != hover) {
            geometry.outline = None;
            let mut outline = PathBuilder::stroke(px(1.5));
            let mut outlined = false;
            for i in 0..values.len().min(HOURS) {
                let selected = match hover {
                    Some(Hover::Hour(cell)) => cell == i,
                    Some(Hover::Weekday(day)) => day == i / 24,
                    Some(Hover::HourOfDay(hour)) => hour == i % 24,
                    _ => false,
                };
                if selected {
                    layout.sector(&mut outline, i / 24, i % 24);
                    outlined = true;
                }
            }
            geometry.outline = Some((hover, outlined.then(|| outline.build().ok()).flatten()));
        }
        Some(geometry)
    }
}

#[derive(Clone, Copy)]
struct Layout {
    center: Point<Pixels>,
    inner: f32,
    outer: f32,
    pitch: f32,
}

impl Layout {
    fn new(b: Bounds<Pixels>) -> Option<Self> {
        let w = f32::from(b.size.width);
        let h = f32::from(b.size.height);
        let outer = w.min(h) / 2.0 - AXIS_PAD;
        let inner = outer * 0.32;
        let pitch = (outer - inner) / 7.0;
        (pitch > RING_GAP).then_some(Self {
            center: b.origin + point(px(w / 2.0), px(h / 2.0)),
            inner,
            outer,
            pitch,
        })
    }

    fn radii(self, day: usize) -> (f32, f32) {
        let inner = self.inner + day as f32 * self.pitch;
        (inner + RING_GAP / 2.0, inner + self.pitch - RING_GAP / 2.0)
    }

    /// 上端が 0 時。角度が増えると時計回りに進む。
    fn polar(self, radius: f32, angle: f32) -> Point<Pixels> {
        self.center + point(px(radius * angle.sin()), px(-radius * angle.cos()))
    }

    fn at(self, p: Point<Pixels>) -> Option<usize> {
        let x = f32::from(p.x - self.center.x);
        let y = f32::from(p.y - self.center.y);
        let radius = x.hypot(y);
        if !(self.inner..self.outer).contains(&radius) {
            return None;
        }
        let day = ((radius - self.inner) / self.pitch) as usize;
        let (inner, outer) = self.radii(day);
        if !(inner..outer).contains(&radius) {
            return None;
        }
        let angle = x.atan2(-y).rem_euclid(TAU);
        let hour = (angle / HOUR_ANGLE).floor() as usize;
        let offset = angle - hour as f32 * HOUR_ANGLE;
        (ANGLE_GAP / 2.0..HOUR_ANGLE - ANGLE_GAP / 2.0)
            .contains(&offset)
            .then_some(day * 24 + hour)
    }

    fn sector(self, path: &mut PathBuilder, day: usize, hour: usize) {
        let (inner, outer) = self.radii(day);
        let start = hour as f32 * HOUR_ANGLE + ANGLE_GAP / 2.0;
        let end = (hour + 1) as f32 * HOUR_ANGLE - ANGLE_GAP / 2.0;
        path.move_to(self.polar(outer, start));
        path.arc_to(
            point(px(outer), px(outer)),
            px(0.0),
            false,
            true,
            self.polar(outer, end),
        );
        path.line_to(self.polar(inner, end));
        path.arc_to(
            point(px(inner), px(inner)),
            px(0.0),
            false,
            false,
            self.polar(inner, start),
        );
        path.close();
    }
}

impl Rireki {
    pub(super) fn radial_hours_card(&self, s: &Summary, width: f32, cx: &mut Context<Self>) -> Div {
        let metric = self.statistics_state.metric;
        let values: Vec<f64> = s.hours.iter().map(|a| metric.amount(a)).collect();
        let peak = values.iter().copied().fold(0.0, f64::max);
        let total = metric.total(&s.total);
        let busiest = values
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0.0)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i);
        let hover = self.statistics_state.hover;
        let highlighted = match hover {
            Some(Hover::Hour(_) | Hover::Weekday(_) | Hover::HourOfDay(_)) => hover,
            _ => busiest.map(Hover::Hour),
        };
        let (label, shown) = match highlighted {
            Some(Hover::Hour(i)) if i < values.len() => (
                format!(
                    "{}{}曜 {}:00–{:02}:00",
                    if hover == highlighted { "" } else { "最多 · " },
                    WEEKDAYS[i / 24],
                    i % 24,
                    i % 24 + 1
                ),
                Some(values[i]),
            ),
            Some(Hover::Weekday(day)) if day < 7 => (
                format!("{}曜 · 終日", WEEKDAYS[day]),
                Some(metric.amount(&s.by_weekday[day])),
            ),
            Some(Hover::HourOfDay(hour)) if hour < 24 => {
                (format!("{hour}時台 · 全曜日"), Some(metric.amount(&s.by_hour[hour])))
            }
            _ => (format!("この期間に{}の記録はありません", metric.label()), None),
        };
        let readout = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap_x_2()
            .text_xs()
            .line_height(px(18.0))
            .min_h(px(36.0))
            .child(div().text_color(c(theme::MUTED)).child(label))
            .when_some(shown, |d, value| {
                d.child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                    "{}{}",
                    metric.format(value),
                    metric.unit()
                )))
                .when(total > 0.0 && metric != Metric::Sessions, |d| {
                    d.child(
                        div()
                            .text_color(c(theme::MUTED))
                            .child(format!("全体の {}", percent(value / total))),
                    )
                })
            });
        let chart_size = (width - 34.0).min(MAX_SIZE);
        let area = self.statistics_state.hours_area.clone();
        let cache = self.statistics_state.radial_cache.borrow_mut().get_or_insert_with(Default::default).clone();
        let plot = canvas(
            {
                let area = area.clone();
                move |b, _, _| area.set(b)
            },
            move |b, _, window, _| paint_radial(b, &values, peak, highlighted, &cache, window),
        )
        .absolute()
        .size_full();
        // 時の目盛りは区分の境界に置き、0 時・6 時・12 時・18 時が時計の四方に来る。
        let radius = chart_size / 2.0 - AXIS_PAD + 13.0;
        let labels = (0..24).step_by(3).map(|hour| {
            let angle = hour as f32 * HOUR_ANGLE;
            div()
                .absolute()
                .left(px(chart_size / 2.0 + radius * angle.sin() - 16.0))
                .top(px(chart_size / 2.0 - radius * angle.cos() - 7.0))
                .w(px(32.0))
                .text_center()
                .text_size(px(11.0))
                .line_height(px(14.0))
                .text_color(c(theme::MUTED))
                .child(format!("{hour}時"))
        });
        let center_size = (chart_size / 2.0 - AXIS_PAD) * 0.64;
        let chart = div()
            .id("statistics-radial-hours")
            .relative()
            .flex_none()
            .size(px(chart_size))
            .child(plot)
            .children(labels)
            .child(
                div()
                    .id("statistics-radial-total")
                    .absolute()
                    .left(px((chart_size - center_size) / 2.0))
                    .top(px((chart_size - center_size) / 2.0))
                    .size(px(center_size))
                    .rounded_full()
                    .bg(c(theme::BACKDROP))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .child(div().text_size(px(10.0)).text_color(c(theme::MUTED)).child("期間合計"))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if total >= 1000.0 {
                                metric.tick(total)
                            } else {
                                metric.format(total)
                            }),
                    )
                    .tooltip(tip(format!("{}{}", metric.format(total), metric.unit()), None)),
            )
            .on_mouse_move(cx.listener(move |t, e: &MouseMoveEvent, _, cx| {
                let cell = Layout::new(area.get()).and_then(|layout| layout.at(e.position));
                t.set_statistics_hover(cell.map(Hover::Hour), cx);
            }))
            .on_hover(cx.listener(|t, on: &bool, _, cx| {
                if !on {
                    t.leave_statistics_chart(
                        |h| matches!(h, Hover::Hour(_) | Hover::Weekday(_) | Hover::HourOfDay(_)),
                        cx,
                    );
                }
            }));
        // 数字で内側からの順序を示す。曜日にポインタを重ねると、その輪と終日の合計を示す。
        let weekdays =
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap_2()
                .children(WEEKDAYS.into_iter().enumerate().map(|(day, name)| {
                    let selected = highlighted == Some(Hover::Weekday(day));
                    div()
                        .id(ElementId::NamedInteger("statistics-radial-weekday".into(), day as u64))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .py_0p5()
                        .rounded_sm()
                        .text_xs()
                        .text_color(c(if selected { theme::TEXT } else { theme::MUTED }))
                        .when(selected, |d| d.bg(c(theme::HOVER)))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(c(theme::SUBTLE))
                                .child((day + 1).to_string()),
                        )
                        .child(name)
                        .tooltip(tip(
                            format!(
                                "内側から {} 本目 · {}曜 {}{}",
                                day + 1,
                                name,
                                metric.format(metric.amount(&s.by_weekday[day])),
                                metric.unit()
                            ),
                            None,
                        ))
                        .on_hover(cx.listener(move |t, on: &bool, _, cx| {
                            if *on {
                                t.set_statistics_hover(Some(Hover::Weekday(day)), cx);
                            } else if t.statistics_state.hover == Some(Hover::Weekday(day)) {
                                t.set_statistics_hover(None, cx);
                            }
                        }))
                }));
        let scale = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap_1()
            .text_xs()
            .text_color(c(theme::MUTED))
            .child("0")
            .child(div().size(px(10.0)).rounded(px(2.0)).bg(c(EMPTY)))
            .child("少")
            .children(theme::HEAT.map(|color| div().size(px(10.0)).rounded(px(2.0)).bg(c(color))))
            .child("多")
            .when(peak > 0.0, |d| {
                d.child(format!("（最大 {}{}）", metric.format(peak), metric.unit()))
            });
        card()
            .child(
                card_title("時間帯", format!("{} · 曜日 × 時刻（ローカル時間）", metric.label()))
                    .flex_wrap()
                    .child(self.hours_view_switch(cx)),
            )
            .child(div().flex().justify_center().child(chart))
            .child(readout)
            .child(weekdays)
            .child(
                div()
                    .text_center()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child("内側から月〜日 · 上が0時、時計回りに24時間"),
            )
            .child(scale)
            .when(metric == Metric::Cost, |d| {
                d.child(
                    div()
                        .text_center()
                        .text_xs()
                        .text_color(c(theme::SUBTLE))
                        .child("API換算の推定額（USD） · 料金不明の記録は除外"),
                )
            })
    }
}

fn paint_radial(
    b: Bounds<Pixels>, values: &[f64], peak: f64, hover: Option<Hover>,
    cache: &std::cell::RefCell<Cache>, window: &mut Window,
) {
    let mut cache = cache.borrow_mut();
    let Some(geometry) = cache.prepare(b, values, peak, hover) else { return };
    let colors = [
        EMPTY,
        theme::HEAT[0],
        theme::HEAT[1],
        theme::HEAT[2],
        theme::HEAT[3],
        theme::HEAT[4],
    ];
    for (level, path) in geometry.fills.iter().enumerate() {
        if let Some(path) = path {
            window.paint_path(path.clone(), c(colors[level]));
        }
    }
    if let Some((_, Some(path))) = &geometry.outline {
        window.paint_path(path.clone(), c(theme::NOW));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn every_weekday_and_hour_is_hit_at_multiple_sizes_and_origins() {
        for diameter in [240.0, 300.0, 360.0] {
            let layout = Layout::new(Bounds::new(
                point(px(137.0), px(59.0)),
                size(px(diameter), px(diameter)),
            ))
            .unwrap();
            for day in 0..7 {
                let (inner, outer) = layout.radii(day);
                for hour in 0..24 {
                    let p = layout.polar((inner + outer) / 2.0, (hour as f32 + 0.5) * HOUR_ANGLE);
                    assert_eq!(layout.at(p), Some(day * 24 + hour));
                }
            }
        }
    }

    #[test]
    fn hole_edges_and_gaps_do_not_select_a_cell() {
        let layout = Layout::new(Bounds::new(point(px(40.0), px(80.0)), size(px(360.0), px(360.0)))).unwrap();
        assert_eq!(layout.at(layout.center), None);
        assert_eq!(layout.at(layout.polar(layout.inner - 1.0, HOUR_ANGLE / 2.0)), None);
        assert_eq!(layout.at(layout.polar(layout.outer + 1.0, HOUR_ANGLE / 2.0)), None);
        for day in 0..7 {
            let (inner, outer) = layout.radii(day);
            let radius = (inner + outer) / 2.0;
            assert_eq!(layout.at(layout.polar(inner - 0.25, HOUR_ANGLE / 2.0)), None);
            assert_eq!(layout.at(layout.polar(outer + 0.25, HOUR_ANGLE / 2.0)), None);
            for hour in 0..24 {
                assert_eq!(layout.at(layout.polar(radius, hour as f32 * HOUR_ANGLE)), None);
                assert_eq!(
                    layout.at(layout.polar(radius, (hour as f32 + 1.0) * HOUR_ANGLE - ANGLE_GAP / 4.0)),
                    None
                );
            }
        }
        assert!(Layout::new(Bounds::new(point(px(0.0), px(0.0)), size(px(50.0), px(50.0)))).is_none());
    }
}
