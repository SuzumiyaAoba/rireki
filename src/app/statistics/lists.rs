//! 統計の一覧: プロジェクト・モデルとトークン・読み込み元の比較。棒は読み込み元ごとに積む

use gpui::{AnyElement, ClickEvent, Context, Div, ElementId, FontWeight, div, point, prelude::*, px, relative};

use super::{Metric, MetricFormat, Rireki, card, card_title, cost_label, fmt_tokens, fmt_usd, percent, series, swatch};
use crate::app::widgets::{project_tile, tip};
use crate::app::{fmt_n, tilde};
use crate::icons::{self, Icon};
use crate::model::{Source, UsageSum};
use crate::statistics::{Summary, Totals};
use crate::theme::{self, c};

/// 一覧に並べる件数。残りは「その他」にまとめる
const PROJECTS: usize = 9;
const MODELS: usize = 6;
/// 数字の列の幅
pub(super) const NUM_W: f32 = 64.0;
pub(super) const SHARE_W: f32 = 44.0;
const COST_W: f32 = 76.0;

impl Rireki {
    /// プロジェクトごとの選んだ指標。押すとそのプロジェクトに絞り込む
    pub(super) fn projects_card(&self, s: &Summary, cx: &mut Context<Self>) -> Div {
        let metric = self.statistics_state.metric;
        let mut projects: Vec<(&String, &[Totals; 2], [f64; 2])> =
            s.projects.iter().map(|(cwd, t)| (cwd, t, [metric.total(&t[0]), metric.total(&t[1])])).collect();
        // 集計はトークンの多い順。選んだ指標の多い順に並べ直す（同じなら元の順）
        projects.sort_by(|a, b| (b.2[0] + b.2[1]).total_cmp(&(a.2[0] + a.2[1])));
        let total = metric.total(&s.total).max(f64::MIN_POSITIVE);
        let max = projects.first().map_or(1.0, |p| p.2[0] + p.2[1]).max(f64::MIN_POSITIVE);
        let all = self.statistics_state.all_projects;
        let shown = if all { projects.len() } else { PROJECTS.min(projects.len()) };
        let rows = projects.iter().take(shown).enumerate().map(|(i, &(cwd, t, v))| {
            let path = cwd.clone();
            let mut sum = t[0].clone();
            sum.merge(&t[1]);
            let detail = format!(
                "{}\n{} セッション · {} ターン · {} トークン · {}",
                tilde(cwd),
                fmt_n(sum.sessions),
                fmt_n(sum.turns),
                fmt_tokens(sum.usage.total()),
                cost_label(&sum.usage)
            );
            row(
                project_tile(cwd, 16.0).into_any_element(),
                self.project_name(cwd),
                bar([v[0] / max, v[1] / max]),
                metric.format(v[0] + v[1]),
                percent((v[0] + v[1]) / total),
                SHARE_W,
            )
            .id(ElementId::NamedInteger("statistics-project".into(), i as u64))
            .mx(px(-6.0))
            .px(px(6.0))
            .rounded_md()
            .cursor_pointer()
            .hover(|d| d.bg(c(theme::HOVER)))
            .tooltip(tip(detail, None))
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                t.set_focus(Some(path.clone()), cx);
                t.statistics_state.all_projects = false;
                t.statistics_state.scroll.set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            }))
        });
        let more = projects.len() > PROJECTS;
        let note = if self.focus.is_some() {
            format!("{}の多い順 · 絞り込み中", metric.label())
        } else {
            format!("{} 件 · {}の多い順 · 押すと絞り込み", fmt_n(projects.len()), metric.label())
        };
        card()
            .child(card_title("プロジェクト", note))
            .child(list(self.columns(metric.label(), "割合", SHARE_W)).children(rows))
            .when(more, |d| {
                d.child(
                    div()
                        .id("statistics-all-projects")
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(c(theme::ACCENT))
                        .cursor_pointer()
                        .hover(|d| d.text_color(c(theme::TEXT)))
                        .child(icons::icon(if all { Icon::ChevronUp } else { Icon::ChevronDown }, theme::ACCENT))
                        .child(if all {
                            "上位だけを表示".to_string()
                        } else {
                            format!("すべて表示（残り {} 件）", fmt_n(projects.len() - PROJECTS))
                        })
                        .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.statistics_state.all_projects = !t.statistics_state.all_projects;
                            cx.notify();
                        })),
                )
            })
    }

    /// モデルごとのトークンと料金、キャッシュ読込の割合
    pub(super) fn models_card(&self, s: &Summary) -> Div {
        let by_cost = self.statistics_state.metric == Metric::Cost;
        let value = |u: &[UsageSum; 2], i: usize| if by_cost { u[i].cost } else { u[i].total() as f64 };
        let mut models: Vec<_> = s.models.iter().collect();
        if by_cost {
            models.sort_by(|a, b| (b.1[0].cost + b.1[1].cost).total_cmp(&(a.1[0].cost + a.1[1].cost)));
        }
        let max = models.first().map_or(1.0, |m| value(&m.1, 0) + value(&m.1, 1)).max(f64::MIN_POSITIVE);
        let rows = models.iter().take(MODELS).map(|(name, u)| {
            let mut sum = u[0];
            sum.merge(&u[1]);
            let source = if u[0].total() >= u[1].total() { Source::Claude } else { Source::Codex };
            let (logo, color) = icons::source_icon(source);
            row(
                icons::icon(logo, color).size(px(14.0)).into_any_element(),
                name.to_string(),
                bar([value(u, 0) / max, value(u, 1) / max]),
                fmt_tokens(sum.total()),
                cost_label(&sum),
                COST_W,
            )
        });
        let rest = models.get(MODELS..).unwrap_or_default();
        let other = (!rest.is_empty()).then(|| {
            let mut sum = UsageSum::default();
            for (_, u) in rest {
                sum.merge(&u[0]);
                sum.merge(&u[1]);
            }
            row(
                blank(),
                format!("その他 {} モデル", rest.len()),
                div(),
                fmt_tokens(sum.total()),
                cost_label(&sum),
                COST_W,
            )
            .text_color(c(theme::MUTED))
        });
        let usage = &s.total.usage;
        let input = usage.input + usage.cache_read + usage.cache_write;
        let cached = usage.cache_read as f64 / input.max(1) as f64;
        card()
            .child(card_title("モデル", if by_cost { "推定料金の多い順" } else { "トークンの多い順" }))
            .child(if s.models.is_empty() {
                empty("この期間のログにはトークン使用量が記録されていません")
            } else {
                list(self.columns("トークン", "料金", COST_W)).children(rows).children(other)
            })
            .when(usage.calls > 0, |d| {
                d.child(div().h(px(1.0)).bg(c(theme::BORDER))).child(
                    div()
                        .id("statistics-cache")
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_xs()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child("キャッシュ読込率"))
                                .child(div().flex_1().child(meter(cached)))
                                .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(percent(cached))),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_x_3()
                                .gap_y_0p5()
                                .text_color(c(theme::MUTED))
                                .child(format!("入力 {}", fmt_tokens(usage.input)))
                                .child(format!("キャッシュ読込 {}", fmt_tokens(usage.cache_read)))
                                .child(format!("キャッシュ書込 {}", fmt_tokens(usage.cache_write)))
                                .child(format!("出力 {}", fmt_tokens(usage.output)))
                                .when(usage.reasoning > 0, |d| {
                                    d.child(format!("（うち推論 {}）", fmt_tokens(usage.reasoning)))
                                }),
                        )
                        .tooltip(tip(
                            "入力（キャッシュなし・キャッシュ読込・キャッシュ書込の合計）のうち、キャッシュから読んだ割合",
                            None,
                        )),
                )
            })
    }

    /// 読み込み元ごとの合計と平均。割合の帯の数字を表で読む
    pub(super) fn sources_card(&self, s: &Summary, inner: f32) -> Div {
        let shown = self.shown_sources();
        // 狭いときは平均の列を省く。名前の最小幅 90・数値 6 列 × 72・平均 3 列 × 92・間隔 9 × 8 と、
        // カードの余白と枠 34 が収まる幅で判定する
        let full = inner >= 904.0;
        let per_turn = |t: &Totals, f: &dyn Fn(f64) -> String, v: f64| {
            if t.turns > 0 { f(v / t.turns as f64) } else { "—".into() }
        };
        let cells = |t: &Totals| -> Vec<String> {
            let mut out = vec![
                fmt_tokens(t.usage.total()),
                cost_label(&t.usage),
                fmt_n(t.turns),
                fmt_n(t.sessions),
                fmt_n(t.tools),
                if t.tools > 0 { percent(t.failures as f64 / t.tools as f64) } else { "—".into() },
            ];
            if full {
                out.push(per_turn(t, &|v| fmt_tokens(v.round() as u64), t.usage.total() as f64));
                out.push(if t.usage.calls > t.usage.unpriced {
                    per_turn(t, &fmt_usd, t.usage.cost)
                } else {
                    "—".into()
                });
                out.push(per_turn(t, &|v| format!("{v:.1}"), t.tools as f64));
            }
            out
        };
        let mut heads = vec!["トークン", "推定料金", "ターン", "セッション", "ツール", "失敗率"];
        if full {
            heads.extend(["トークン/ターン", "料金/ターン", "ツール/ターン"]);
        }
        let widths: Vec<f32> = heads.iter().map(|h| if h.chars().count() > 5 { 92.0 } else { 72.0 }).collect();
        let line = |name: Div, values: Vec<String>, strong: bool| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(30.0))
                .border_t_1()
                .border_color(c(theme::GRID))
                .text_xs()
                .when(strong, |d| d.font_weight(FontWeight::SEMIBOLD))
                .child(name.flex_1().min_w(px(90.0)))
                .children(
                    values
                        .into_iter()
                        .zip(&widths)
                        .map(|(v, &w)| div().flex_none().w(px(w)).flex().justify_end().whitespace_nowrap().child(v)),
                )
        };
        let mut sum = s.sources[0].clone();
        sum.merge(&s.sources[1]);
        card()
            .child(card_title(
                if shown.len() > 1 { "読み込み元の比較" } else { "合計と平均" },
                "サブエージェントを含む · 失敗率はツール呼び出しに対する失敗・拒否・中断の割合",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .pb_1()
                            .text_size(px(11.0))
                            .text_color(c(theme::MUTED))
                            .child(div().flex_1().min_w(px(90.0)))
                            .children(heads.iter().zip(&widths).map(|(h, &w)| {
                                div().flex_none().w(px(w)).flex().justify_end().whitespace_nowrap().child(*h)
                            })),
                    )
                    .children(shown.iter().map(|&i| {
                        let (logo, color) = icons::source_icon([Source::Claude, Source::Codex][i]);
                        line(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(swatch(series(i)))
                                .child(icons::icon(logo, color).size(px(14.0)))
                                .child(["Claude Code", "Codex"][i]),
                            cells(&s.sources[i]),
                            false,
                        )
                    }))
                    .when(shown.len() > 1, |d| d.child(line(div().pl(px(16.0)).child("合計"), cells(&sum), true))),
            )
    }
}

impl Rireki {
    /// 数字の列の見出し。両方の読み込み元を表示しているときは、棒の色の凡例を左に置く
    pub(super) fn columns(&self, value: &'static str, note: &'static str, note_w: f32) -> Div {
        let sources = self.shown_sources();
        div()
            .flex()
            .gap_2()
            .text_size(px(11.0))
            .text_color(c(theme::MUTED))
            .child(div().flex_1().flex().items_center().gap_3().when(sources.len() > 1, |d| {
                d.children(sources.iter().map(|&i| {
                    div().flex().items_center().gap_1p5().child(swatch(series(i))).child(["Claude", "Codex"][i])
                }))
            }))
            .child(div().flex_none().w(px(NUM_W)).flex().justify_end().whitespace_nowrap().child(value))
            .child(div().flex_none().w(px(note_w)).flex().justify_end().whitespace_nowrap().child(note))
    }
}

/// 列見出しと行を縦に並べる
pub(super) fn list(columns: Div) -> Div {
    div().flex().flex_col().gap_1().child(columns)
}

/// 一覧の 1 行: 目印・名前・棒・数・右端の補足（割合・料金など）
fn row(mark: AnyElement, name: String, bar: Div, value: String, note: String, note_w: f32) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .h(px(26.0))
        .text_xs()
        .child(div().flex_none().w(px(16.0)).flex().justify_center().child(mark))
        .child(div().flex_none().w(relative(0.3)).min_w(px(90.0)).truncate().child(name))
        .child(div().flex_1().min_w(px(40.0)).flex().items_center().child(bar))
        .child(
            div()
                .flex_none()
                .w(px(NUM_W))
                .flex()
                .justify_end()
                .whitespace_nowrap()
                .font_weight(FontWeight::MEDIUM)
                .child(value),
        )
        .child(
            div()
                .flex_none()
                .w(px(note_w))
                .flex()
                .justify_end()
                .whitespace_nowrap()
                .text_color(c(theme::MUTED))
                .child(note),
        )
}

/// 目印のない行の空き
fn blank() -> AnyElement {
    div().size(px(14.0)).into_any_element()
}

pub(super) fn empty(text: &'static str) -> Div {
    div().text_xs().text_color(c(theme::MUTED)).child(text)
}

/// 読み込み元ごとに積んだ横棒。長さは一覧の最大に対する割合
pub(super) fn bar(parts: [f64; 2]) -> Div {
    div().w_full().h(px(8.0)).flex().gap(px(2.0)).children(
        parts
            .into_iter()
            .enumerate()
            .filter(|(_, r)| *r > 0.0)
            .map(|(i, r)| div().h_full().w(relative(r as f32)).min_w(px(2.0)).rounded(px(2.0)).bg(c(series(i)))),
    )
}

/// 一色の割合の帯。全体を同じ色の暗い溝に入れる
fn meter(r: f64) -> Div {
    div().w_full().h(px(8.0)).rounded(px(2.0)).bg(c(theme::HEAT[0]).opacity(0.35)).child(
        div()
            .h_full()
            .w(relative(r as f32))
            .min_w(px(if r > 0.0 { 2.0 } else { 0.0 }))
            .rounded(px(2.0))
            .bg(c(theme::HEAT[2])),
    )
}
