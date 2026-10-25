//! ツールの呼び出し・失敗・所要時間を 1 行にまとめた表
//!
//! - 呼び出し: 読み込み元ごとに積んだ横棒と件数
//! - 失敗率: 失敗（エラーの結果）と拒否・中断を状態の色で分けた帯。長さは表示中の最大に合わせた目盛り
//! - 所要時間: 桁が大きく違うので共通の対数軸に置き、中央値（●）から P95（●）までを線で結ぶ。数値も列に出す
//!
//! 帯と所要時間の図は行ごとに 1 つの canvas に描き、表の要素を行数 × 印の数に増やさない。

use gpui::{
    AnyElement, BorderStyle, Bounds, Div, FontWeight, Pixels, canvas, div, fill, point, prelude::*, px, quad, relative,
    size,
};

use super::lists::{NUM_W, bar, empty};
use super::{Metric, Rireki, card, card_title, percent, series, swatch};
use crate::app::widgets::lazy_tip;
use crate::app::{fmt_dur, fmt_n};
use crate::icons::{self, Icon};
use crate::statistics::{Calls, Durations, Summary, Tool};
use crate::theme::{self, c};

/// 並べるツールの数。残りは「その他」にまとめる
const TOOLS: usize = 10;
const ROW_H: f32 = 28.0;
const RATE_W: f32 = 48.0;
const FAIL_W: f32 = 72.0;
const TIME_W: f32 = 60.0;
/// 所要時間の目盛り（秒）。表記は詳細パネルの所要時間と同じ単位にする
const TICKS: [(f64, &str); 9] = [
    (0.001, "1ms"),
    (0.01, "10ms"),
    (0.1, "100ms"),
    (1.0, "1s"),
    (10.0, "10s"),
    (60.0, "1m"),
    (600.0, "10m"),
    (3600.0, "1h"),
    (36000.0, "10h"),
];
/// 失敗率の帯の目盛りの上限の候補
const RATE_SCALES: [f64; 7] = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0];
/// 中央値と P95 の印。量を表す青の段階のうち、主に読む中央値を明るくする
const MEDIAN: u32 = theme::HEAT[3];
const P95: u32 = theme::HEAT[1];

/// 表の幅で決める列の構成
struct Layout {
    name_w: f32,
    fail_bar: bool,
    p95: bool,
    axis: Option<Axis>,
}

impl Rireki {
    pub(super) fn tools_card(&self, s: &Summary, inner: f32) -> Div {
        let by_failures = self.statistics_state.metric == Metric::Failures;
        let mut tools: Vec<&Tool> = s.tools.iter().collect();
        if by_failures {
            // 同じ件数なら呼び出しの多い順（集計の並び）を保つ
            tools.sort_by_key(|t| std::cmp::Reverse(t.total().failures));
        }
        let shown = &tools[..tools.len().min(TOOLS)];
        let rest = &tools[shown.len()..];
        // カードの左右の余白と枠を除いた幅
        let content = inner - 34.0;
        let layout = Layout {
            name_w: (content * 0.2).clamp(96.0, 180.0),
            fail_bar: content >= 640.0,
            p95: content >= 420.0,
            axis: (content >= 880.0).then(|| Axis::new(shown, (content * 0.24).clamp(180.0, 320.0))).flatten(),
        };
        let max = shown.iter().map(|t| t.total().calls).max().unwrap_or(1).max(1) as f64;
        let scale = rate_scale(shown.iter().map(|t| t.total()));
        let rows = shown.iter().map(|t| self.tool_row(t, max, scale, &layout).into_any_element());
        let other = (!rest.is_empty()).then(|| {
            let mut calls = Calls::default();
            for t in rest {
                calls.merge(&t.total());
            }
            other_row(rest.len(), &calls, scale, &layout).into_any_element()
        });
        let total = &s.total;
        let note = format!(
            "{} 回 · 失敗・拒否・中断 {} · {}",
            fmt_n(total.tools),
            percent(total.failures as f64 / total.tools.max(1) as f64),
            if by_failures { "失敗・拒否・中断の多い順" } else { "呼び出しの多い順" }
        );
        card()
            .child(card_title("ツール", note).flex_wrap().child(self.tools_legend(&layout)))
            .child(if s.tools.is_empty() {
                empty("この期間にツール呼び出しはありません")
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(columns(&layout, scale))
                    .children(rows)
                    .children(other)
            })
            .when(!s.tools.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(c(theme::SUBTLE))
                        .child("失敗率は呼び出しに対する割合。所要時間は結果のある呼び出しの区間分布から求めた近似値（誤差 6.25% 以内）で、サブエージェントは配下の処理を含みます"),
                )
            })
    }

    /// 棒・帯・印の色の凡例。両方の読み込み元を表示しているときだけ読み込み元の色を出す
    fn tools_legend(&self, layout: &Layout) -> Div {
        let sources = self.shown_sources();
        let item = |mark: Div, text: &'static str| {
            div().flex().items_center().gap_1p5().child(mark).child(div().text_color(c(theme::MUTED)).child(text))
        };
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap_x_3()
            .gap_y_1()
            .text_xs()
            .when(sources.len() > 1, |d| {
                d.children(sources.iter().map(|&i| item(swatch(series(i)), ["Claude", "Codex"][i])))
            })
            .child(item(swatch(theme::ERR), "失敗"))
            .child(item(swatch(theme::WARN), "拒否・中断"))
            .when(layout.axis.is_some(), |d| {
                d.child(item(marker_swatch(MEDIAN), "中央値")).child(item(marker_swatch(P95), "P95"))
            })
    }

    fn tool_row(&self, t: &Tool, max: f64, scale: Option<f64>, layout: &Layout) -> gpui::Stateful<Div> {
        let calls = t.total();
        let (icon, color) =
            if t.agent { (Icon::Bot, 0xa77bf3) } else { (theme::tool_icon(&t.name), theme::tool_color(&t.name)) };
        let d = &t.durations;
        // 説明はツールチップを開いたときに作る
        let (name, agent, by_source) = (t.name.clone(), t.agent, [t.sources[0].calls, t.sources[1].calls]);
        let (median, p95, longest, timed) = (d.percentile(50), d.percentile(95), d.max(), d.count());
        let detail = move || {
            let time = |v: Option<f64>| v.map_or_else(|| "—".to_string(), fmt_dur);
            format!(
                "{}{}\n呼び出し {} 回（Claude {} · Codex {}）\n失敗 {} · 拒否 {} · 中断 {} · 結果なし {}\n{}",
                name,
                if agent { "（サブエージェント）" } else { "" },
                fmt_n(calls.calls),
                fmt_n(by_source[0]),
                fmt_n(by_source[1]),
                fmt_n(calls.errors()),
                fmt_n(calls.rejected),
                fmt_n(calls.interrupted),
                fmt_n(calls.pending),
                if timed > 0 {
                    format!(
                        "所要時間 中央値 {} · P95 {} · 最長 {}（結果のある {} 回）",
                        time(median),
                        time(p95),
                        time(longest),
                        fmt_n(timed as usize)
                    )
                } else {
                    "所要時間の記録はありません".to_string()
                }
            )
        };
        cells(
            icons::icon(icon, color).size(px(14.0)).into_any_element(),
            t.name.clone(),
            bar([t.sources[0].calls as f64 / max, t.sources[1].calls as f64 / max]),
            &calls,
            scale,
            Some(d),
            layout,
        )
        .id(gpui::SharedString::from(format!("statistics-tool-{}-{}", t.name, t.agent)))
        .tooltip(lazy_tip(detail))
    }
}

/// 一覧の 1 行。名前・呼び出し・失敗率・所要時間の列を表の構成に合わせて並べる
fn cells(
    mark: AnyElement,
    name: String,
    calls_bar: Div,
    calls: &Calls,
    scale: Option<f64>,
    durations: Option<&Durations>,
    layout: &Layout,
) -> Div {
    let rate = if calls.calls > 0 && calls.failures > 0 {
        percent(calls.failures as f64 / calls.calls as f64)
    } else {
        "—".to_string()
    };
    let time = |p| durations.and_then(|d| d.percentile(p)).map_or_else(|| "—".to_string(), fmt_dur);
    div()
        .flex()
        .items_center()
        .gap_2()
        .h(px(ROW_H))
        .text_xs()
        .child(div().flex_none().w(px(16.0)).flex().justify_center().child(mark))
        .child(div().flex_none().w(px(layout.name_w)).truncate().child(name))
        .child(div().flex_1().min_w(px(48.0)).flex().items_center().child(calls_bar))
        .child(number(NUM_W, fmt_n(calls.calls)).font_weight(FontWeight::MEDIUM))
        .when(layout.fail_bar, |d| d.child(div().flex_none().w(px(FAIL_W)).child(failure_bar(calls, scale))))
        .child(number(RATE_W, rate).text_color(c(if calls.failures > 0 { theme::TEXT } else { theme::MUTED })))
        .when_some(layout.axis.as_ref(), |d, axis| d.child(axis.plot(durations)))
        .child(number(TIME_W, time(50)))
        .when(layout.p95, |d| d.child(number(TIME_W, time(95)).text_color(c(theme::MUTED))))
}

/// 上位以外のツールをまとめた行。所要時間は種類ごとに桁が違うので合算しない
fn other_row(kinds: usize, calls: &Calls, scale: Option<f64>, layout: &Layout) -> Div {
    cells(
        div().size(px(14.0)).into_any_element(),
        format!("その他 {} 種類", fmt_n(kinds)),
        div(),
        calls,
        scale,
        None,
        layout,
    )
    .text_color(c(theme::MUTED))
}

/// 列見出し。所要時間の列には対数軸の目盛りを置く
fn columns(layout: &Layout, scale: Option<f64>) -> Div {
    let head = |w: f32, text: &'static str| number(w, text.to_string());
    div()
        .flex()
        .items_end()
        .gap_2()
        .pb_1()
        .text_size(px(11.0))
        .text_color(c(theme::MUTED))
        .child(div().flex_none().w(px(16.0)))
        .child(div().flex_none().w(px(layout.name_w)))
        .child(div().flex_1().min_w(px(48.0)))
        .child(head(NUM_W, "呼び出し"))
        .when(layout.fail_bar, |d| {
            // 帯の右端が表す割合
            d.child(
                div()
                    .flex_none()
                    .w(px(FAIL_W))
                    .flex()
                    .justify_between()
                    .text_color(c(theme::SUBTLE))
                    .child("0")
                    .child(scale.map(percent).unwrap_or_default()),
            )
        })
        .child(head(RATE_W, "失敗率"))
        .when_some(layout.axis.as_ref(), |d, axis| d.child(axis.labels()))
        .child(head(TIME_W, "中央値"))
        .when(layout.p95, |d| d.child(head(TIME_W, "P95")))
}

fn number(w: f32, text: String) -> Div {
    div().flex_none().w(px(w)).flex().justify_end().whitespace_nowrap().child(text)
}

/// 表示中のツールの最大の失敗率を収める目盛りの上限。失敗がなければ None
fn rate_scale(calls: impl Iterator<Item = Calls>) -> Option<f64> {
    let max = calls
        .filter(|c| c.calls > 0)
        .map(|c| c.failures as f64 / c.calls as f64)
        .fold(0.0, f64::max);
    (max > 0.0).then(|| RATE_SCALES.into_iter().find(|&s| s >= max).unwrap_or(1.0))
}

/// 失敗と拒否・中断を分けて積んだ帯。溝の長さが目盛りの上限。境目は面の色の隙間で分ける
fn failure_bar(calls: &Calls, scale: Option<f64>) -> impl IntoElement {
    let parts = match scale {
        Some(scale) if calls.calls > 0 => {
            let n = calls.calls as f64 * scale;
            [
                (calls.errors() as f64 / n, theme::ERR),
                ((calls.rejected + calls.interrupted) as f64 / n, theme::WARN),
            ]
        }
        _ => [(0.0, theme::ERR), (0.0, theme::WARN)],
    };
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            window.paint_quad(fill(b, c(theme::GRID)).corner_radii(px(2.0)));
            let width = f32::from(b.size.width);
            let mut x = 0.0;
            for (fraction, color) in parts {
                if fraction <= 0.0 || x >= width {
                    continue;
                }
                let w = ((fraction.min(1.0) as f32) * width).max(2.0).min(width - x);
                window.paint_quad(fill(rect(b, x, 0.0, w, f32::from(b.size.height)), c(color)).corner_radii(px(2.0)));
                x += w + 2.0;
            }
        },
    )
    .w_full()
    .h(px(6.0))
}

/// `b` の左上からの相対位置の矩形
fn rect(b: Bounds<Pixels>, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(b.origin + point(px(x), px(y)), size(px(w), px(h)))
}

fn marker_swatch(color: u32) -> Div {
    div().flex_none().size(px(8.0)).rounded_full().bg(c(color))
}

/// 所要時間の共通の対数軸
struct Axis {
    width: f32,
    /// 両端の目盛りの値（秒）と、その常用対数
    first: f64,
    last: f64,
    lo: f64,
    hi: f64,
}

impl Axis {
    /// 表示中のツールの中央値と P95 を収める目盛りの範囲。所要時間がなければ None
    fn new(tools: &[&Tool], width: f32) -> Option<Self> {
        let (mut lo, mut hi) = (f64::INFINITY, 0.0_f64);
        for t in tools {
            if let (Some(a), Some(b)) = (t.durations.percentile(50), t.durations.percentile(95)) {
                lo = lo.min(a);
                hi = hi.max(b);
            }
        }
        if !lo.is_finite() {
            return None;
        }
        // 1ms 未満は左端に置く。少なくとも 1 桁の幅をとる
        let first = TICKS.iter().rev().find(|t| t.0 <= lo).map_or(TICKS[0].0, |t| t.0);
        let last = TICKS.iter().find(|t| t.0 >= hi.max(first * 10.0)).map_or(TICKS[TICKS.len() - 1].0, |t| t.0);
        Some(Self { width, first, last, lo: first.log10(), hi: last.log10() })
    }

    fn x(&self, seconds: f64) -> f32 {
        ((seconds.max(self.first).log10() - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0) as f32
    }

    fn ticks(&self) -> impl Iterator<Item = (f32, &'static str)> + '_ {
        TICKS
            .iter()
            .filter(|t| (self.first..=self.last).contains(&t.0))
            .map(|&(v, label)| (self.x(v), label))
    }

    /// 目盛りの表記。重ならない間隔に間引き、両端は内側にそろえる
    fn labels(&self) -> Div {
        let ticks: Vec<_> = self.ticks().collect();
        let fit = ((self.width / 44.0).floor() as usize).max(2);
        let step = ticks.len().div_ceil(fit).max(1);
        let last = ticks.len().saturating_sub(1);
        div().flex_none().w(px(self.width)).h(px(14.0)).relative().children(
            ticks.into_iter().enumerate().filter(|(i, _)| i % step == 0 || *i == last).map(move |(i, (x, label))| {
                let d = div().absolute().top_0().whitespace_nowrap().text_color(c(theme::SUBTLE)).child(label);
                if i == 0 {
                    d.left_0()
                } else if i == last {
                    d.right_0()
                } else {
                    d.left(relative(x)).ml(px(-16.0)).w(px(32.0)).flex().justify_center()
                }
            }),
        )
    }

    /// 1 行の所要時間。目盛りの縦線の上に、中央値から P95 までの線と 2 つの印を置く
    fn plot(&self, durations: Option<&Durations>) -> impl IntoElement {
        let mut ticks = [0.0_f32; TICKS.len()];
        let mut n = 0;
        for (x, _) in self.ticks() {
            ticks[n] = x;
            n += 1;
        }
        let points = durations.and_then(|d| Some((self.x(d.percentile(50)?), self.x(d.percentile(95)?))));
        canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                for &x in &ticks[..n] {
                    window.paint_quad(fill(rect(b, (x * w).min(w - 1.0), 0.0, 1.0, h), c(theme::GRID)));
                }
                let Some((a, z)) = points else {
                    return;
                };
                let (a, z) = (a * w, z * w);
                window.paint_quad(fill(rect(b, a, h / 2.0 - 1.0, (z - a).max(0.0), 2.0), c(P95)));
                // 面の色の輪を付けた 8px の印。重なっても見分けられるようにする
                for (x, color) in [(z, P95), (a, MEDIAN)] {
                    window.paint_quad(quad(
                        rect(b, x - 6.0, h / 2.0 - 6.0, 12.0, 12.0),
                        px(6.0),
                        c(color),
                        px(2.0),
                        c(theme::PANEL),
                        BorderStyle::Solid,
                    ));
                }
            },
        )
        .flex_none()
        .w(px(self.width))
        .h(px(16.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_scale_rounds_up_to_a_readable_limit() {
        let calls = |calls, failures| Calls { calls, failures, ..Calls::default() };
        assert_eq!(rate_scale([calls(100, 0)].into_iter()), None);
        assert_eq!(rate_scale([calls(100, 3), calls(10, 1)].into_iter()), Some(0.1));
        assert_eq!(rate_scale([calls(100, 11)].into_iter()), Some(0.2));
        assert_eq!(rate_scale([calls(2, 2)].into_iter()), Some(1.0));
        assert_eq!(rate_scale([calls(0, 0)].into_iter()), None);
    }

    #[test]
    fn log_axis_covers_medians_and_p95_with_tick_aligned_ends() {
        let tool = |seconds: &[f64]| Tool {
            name: "t".into(),
            agent: false,
            sources: Default::default(),
            durations: Durations::from_seconds(seconds.iter().copied()),
        };
        let fast = tool(&[0.05, 0.06, 0.07]);
        let slow = tool(&[5.0, 30.0, 200.0]);
        let axis = Axis::new(&[&fast, &slow], 200.0).unwrap();
        // 中央値 60ms 付近から P95 200s 付近まで → 10ms〜10m
        assert_eq!((axis.first, axis.last), (0.01, 600.0));
        assert_eq!(axis.x(0.01), 0.0);
        assert_eq!(axis.x(600.0), 1.0);
        assert!(axis.x(1.0) > axis.x(0.1) && axis.x(10.0) > axis.x(1.0));
        assert_eq!(axis.ticks().map(|t| t.1).collect::<Vec<_>>(), ["10ms", "100ms", "1s", "10s", "1m", "10m"]);
        // 1ms 未満だけでも 1 桁の幅をとる
        let zero = tool(&[0.0, 0.0]);
        let axis = Axis::new(&[&zero], 200.0).unwrap();
        assert_eq!((axis.first, axis.last), (0.001, 0.01));
        assert!(Axis::new(&[&tool(&[])], 200.0).is_none());
    }
}
