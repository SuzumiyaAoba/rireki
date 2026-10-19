//! 使用量、料金、モデル別集計の表示と共通フォーマット。

use super::{
    Div, ElementId, FontWeight, Stateful, Usage, UsageScale, UsageSum, c, div, fmt_n, px, stat,
    theme, tilde, tip,
};
use gpui::prelude::*;

pub(super) fn usage_total_label(usage: &UsageSum) -> String {
    if usage.calls == 0 {
        "不明".into()
    } else {
        fmt_tokens(usage.total())
    }
}

pub(super) fn usage_cost_label(usage: &UsageSum) -> String {
    if usage.calls == 0 || usage.calls == usage.unpriced {
        "不明".into()
    } else if usage.unpriced > 0 {
        format!("{}（一部）", fmt_usd(usage.cost))
    } else {
        fmt_usd(usage.cost)
    }
}

/// トークン数を短く（1,234 / 12.3k / 1.23M / 13.2B）
pub(in crate::app) fn fmt_tokens(n: u64) -> String {
    match n {
        0..10_000 => fmt_n(n as usize),
        10_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.2}M", n as f64 / 1e6),
        _ => format!("{:.2}B", n as f64 / 1e9),
    }
}

/// 使用量の内訳の色: 入力・キャッシュ読込・キャッシュ書込・出力
pub(super) const USAGE_COLORS: [u32; 4] = [0x5b9cf5, 0x3f6a9e, 0xd29a4a, 0x7fd18b];

pub(super) const CACHE_HIT_TOOLTIP: &str = "入力全体（キャッシュなし・キャッシュ読込・キャッシュ書込の合計）のうち、キャッシュから読んだ割合。入力トークンの記録がない場合は —";

pub(super) fn fmt_cache_hit_rate(rate: Option<f64>) -> String {
    rate.map_or_else(|| "—".into(), |r| format!("{:.1}%", r * 100.0))
}

pub(super) fn cache_hit_stat(id: &'static str, usage: &UsageSum) -> Stateful<Div> {
    stat(
        "キャッシュヒット率",
        fmt_cache_hit_rate(usage.cache_hit_rate()),
        None,
    )
    .id(id)
    .tooltip(tip(CACHE_HIT_TOOLTIP, None))
}

/// 使用量の合計・内訳の帯・凡例
pub(in crate::app) fn usage_view(u: &UsageSum) -> Div {
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
        .child(div().flex().gap(px(2.0)).h(px(8.0)).w_full().children(
            parts.iter().filter(|p| p.1 > 0).map(|&(_, n, col)| {
                div()
                    .h_full()
                    .flex_basis(gpui::relative(n as f32 / total))
                    .flex_shrink()
                    .min_w(px(3.0))
                    .rounded(px(2.0))
                    .bg(c(col))
            }),
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_y_1()
                .children(parts.iter().filter(|p| p.1 > 0).map(|&(label, n, col)| {
                    div()
                        .w(gpui::relative(0.5))
                        .pr_3()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(div().size(px(8.0)).flex_none().rounded(px(2.0)).bg(c(col)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(c(theme::MUTED))
                                .child(label),
                        )
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
                })),
        )
        .when(u.reasoning > 0, |d| {
            d.child(
                div()
                    .text_color(c(theme::SUBTLE))
                    .child(format!("出力のうち推論 {}", fmt_tokens(u.reasoning))),
            )
        })
        .child(cache_hit_stat("usage-cache-hit", u))
}

/// 料金（USD）。1 ドル以上は桁区切りを入れる
pub(in crate::app) fn fmt_usd(v: f64) -> String {
    if v < 0.01 && v > 0.0 {
        format!("${v:.4}")
    } else {
        let cents = (v * 100.0).round() as u64;
        format!("${}.{:02}", fmt_n((cents / 100) as usize), cents % 100)
    }
}

/// モデルごとのトークン数と料金
pub(super) fn model_table(models: &[(String, UsageSum)]) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .children(models.iter().map(|(m, u)| {
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().min_w_0().truncate().child(if m.is_empty() {
                    "(不明)".to_string()
                } else {
                    m.clone()
                }))
                .child(
                    div()
                        .flex_none()
                        .text_color(c(theme::MUTED))
                        .child(fmt_tokens(u.total())),
                )
                .child(div().flex_none().w(px(96.0)).flex().justify_end().child(
                    if u.unpriced > 0 && u.cost == 0.0 {
                        div().text_color(c(theme::SUBTLE)).child("不明")
                    } else {
                        div().text_color(c(theme::OK)).child(usage_cost_label(u))
                    },
                ))
        }))
}

pub(super) fn usage_input(u: &Usage) -> u64 {
    u.input_total()
}

/// 使用量の内訳・モデル・料金。数値表示と比較バーで同じツールチップを使う。
pub(super) fn usage_tooltip(u: &Usage, model: Option<&str>) -> String {
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
    detail.push_str(&format!(
        "\nキャッシュヒット率 {}\n{CACHE_HIT_TOOLTIP}",
        fmt_cache_hit_rate(u.cache_hit_rate())
    ));
    if let Some(m) = model {
        detail.push_str(&format!("\n{m}"));
        if let Some(p) = crate::pricing::price(m) {
            detail.push_str(&format!(" · {}", fmt_usd(p.cost(u))));
        }
    }
    detail
}

/// 1 イベントの入力と出力を別々の列で比較する。入力はキャッシュ込み。
pub(super) fn usage_meter(
    id: impl Into<ElementId>,
    u: &Usage,
    model: Option<&str>,
    scale: UsageScale,
) -> Stateful<Div> {
    let parts = [
        ("入力", usage_input(u), scale.input, USAGE_COLORS[0]),
        ("出力", u.output as u64, scale.output, USAGE_COLORS[3]),
    ];
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .mt_1()
        .flex()
        .flex_col()
        .gap_1()
        .text_xs()
        .child(div().flex().gap_3().children(parts.into_iter().map(
            |(label, value, max, color)| {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .text_color(c(color))
                            .child(label)
                            .child(fmt_tokens(value)),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded_full()
                            .bg(c(theme::GRID))
                            .when(value > 0, |d| {
                                d.child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(
                                            (value as f64 / max.max(1) as f64).min(1.0) as f32,
                                        ))
                                        .min_w(px(1.0))
                                        .rounded_full()
                                        .bg(c(color)),
                                )
                            }),
                    )
            },
        )))
        .child(div().text_color(c(theme::MUTED)).child(format!(
            "キャッシュヒット率 {}",
            fmt_cache_hit_rate(u.cache_hit_rate())
        )))
        .tooltip(tip(usage_tooltip(u, model), None))
}
