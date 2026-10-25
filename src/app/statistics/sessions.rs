//! 期間内のセッションを、選んだ指標の多い順に並べる。押すとタイムラインと詳細パネルで開く
//!
//! 全件の並べ替えはせず、上位だけを選んでから並べる。セッション数の指標は 1 件ずつなので開始の新しい順にする。
//! 集計に渡した表示順に依存しないよう、同じ値の順序も開始時刻で決める。

use gpui::{ClickEvent, Context, Div, ElementId, div, prelude::*, px, relative};

use super::lists::{NUM_W, SHARE_W, bar, empty, list};
use super::{Metric, MetricFormat, Rireki, card, card_title, cost_label, fmt_tokens, percent};
use crate::app::widgets::{dot, lazy_tip};
use crate::app::{fmt_n, is_live, rel_time, tilde};
use crate::icons::{self, Icon};
use crate::model::one_line;
use crate::statistics::{SessionRow, Summary, source_index};
use crate::theme::{self, c};

/// 最初に並べる件数と、すべて表示したときの上限
const SESSIONS: usize = 8;
const SESSIONS_ALL: usize = 50;

impl Rireki {
    pub(super) fn sessions_card(&self, s: &Summary, cx: &mut Context<Self>) -> Div {
        let metric = self.statistics_state.metric;
        let newest = metric == Metric::Sessions;
        let failures = metric == Metric::Failures;
        let all = self.statistics_state.all_sessions;
        let (rows, matched) = ranked(&s.sessions, metric, if all { SESSIONS_ALL } else { SESSIONS });
        let total = metric.total(&s.total).max(f64::MIN_POSITIVE);
        let max = rows.first().map_or(1.0, |r| metric.total(&r.total)).max(f64::MIN_POSITIVE);
        let items = rows.iter().map(|row| {
            let index = row.index;
            let session = &self.store.sessions[index];
            let value = metric.total(&row.total);
            let src = source_index(session.source);
            let mut parts = [0.0; 2];
            parts[src] = value / max;
            let (logo, logo_color) = icons::source_icon(session.source);
            let title = if session.title.is_empty() { "(無題)".to_string() } else { one_line(&session.title, 200) };
            // 説明はツールチップを開いたときに作る
            let (full, t) = (session.clone(), row.total.clone());
            let detail = move || {
                format!(
                    "{}\n{}\n期間内: {} ターン · ツール {} 回 · 失敗・拒否・中断 {} · {} トークン · {}\n{}",
                    if full.title.is_empty() { "(無題)".to_string() } else { one_line(&full.title, 400) },
                    tilde(&full.cwd),
                    fmt_n(t.turns),
                    fmt_n(t.tools),
                    fmt_n(t.failures),
                    fmt_tokens(t.usage.total()),
                    cost_label(&t.usage),
                    if failures { "押すと失敗したイベントを開く" } else { "押すとタイムラインと詳細で開く" }
                )
            };
            div()
                .id(ElementId::NamedInteger("statistics-session".into(), index as u64))
                .flex()
                .items_center()
                .gap_2()
                .h(px(40.0))
                .mx(px(-6.0))
                .px(px(6.0))
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .hover(|d| d.bg(c(theme::HOVER)))
                .child(
                    div()
                        .flex_none()
                        .w(px(16.0))
                        .flex()
                        .justify_center()
                        .child(icons::icon(logo, logo_color).size(px(14.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().truncate().child(title))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_color(c(theme::MUTED))
                                .child(div().min_w_0().truncate().child(self.project_name(&session.cwd)))
                                .child(div().flex_none().child(format!("· {}", rel_time(session.end))))
                                .when(is_live(session.end), |d| d.child(dot(theme::LIVE, 6.0))),
                        ),
                )
                .when(!newest, |d| {
                    d.child(div().flex_none().w(relative(0.24)).min_w(px(40.0)).flex().items_center().child(bar(parts)))
                        .child(
                            div()
                                .flex_none()
                                .w(px(NUM_W))
                                .flex()
                                .justify_end()
                                .whitespace_nowrap()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(metric.format(value)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(px(SHARE_W))
                                .flex()
                                .justify_end()
                                .whitespace_nowrap()
                                .text_color(c(theme::MUTED))
                                .child(percent(value / total)),
                        )
                })
                .tooltip(lazy_tip(detail))
                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    if failures {
                        t.open_failures(index, w, cx);
                    } else {
                        t.open_session(index, w, cx);
                    }
                }))
        });
        let note = if newest {
            format!("期間内の {} 件 · 開始の新しい順 · 押すと開く", fmt_n(s.sessions.len()))
        } else if failures {
            format!("{} 件 · 失敗・拒否・中断の多い順 · 押すと失敗したイベントを開く", fmt_n(matched))
        } else {
            format!("{} 件 · {}の多い順 · 押すと開く", fmt_n(matched), metric.label())
        };
        let more = matched > SESSIONS;
        card()
            .child(card_title("セッション", note))
            .child(if rows.is_empty() {
                empty(if failures {
                    "この期間に失敗・拒否・中断したツール呼び出しはありません"
                } else {
                    "この期間に該当するセッションはありません"
                })
            } else if newest {
                div().flex().flex_col().gap_1().children(items)
            } else {
                list(self.columns(metric.label(), "割合", SHARE_W)).children(items)
            })
            .when(more, |d| {
                d.child(
                    div()
                        .id("statistics-all-sessions")
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
                            format!("上位 {} 件まで表示", fmt_n(matched.min(SESSIONS_ALL)))
                        })
                        .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.statistics_state.all_sessions = !t.statistics_state.all_sessions;
                            cx.notify();
                        })),
                )
            })
    }
}

/// 指標の多い順の上位 `limit` 件と、値のあるセッションの数。同じ値なら開始の新しいセッションを先にする。
/// セッション数の指標はどのセッションも 1 件なので、開始の新しい順になる
fn ranked(rows: &[SessionRow], metric: Metric, limit: usize) -> (Vec<&SessionRow>, usize) {
    let mut out: Vec<(usize, f64)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i, metric.total(&r.total)))
        .filter(|&(_, v)| v > 0.0)
        .collect();
    let matched = out.len();
    let order = |a: &(usize, f64), b: &(usize, f64)| {
        b.1.total_cmp(&a.1).then(rows[b.0].start.total_cmp(&rows[a.0].start)).then(a.0.cmp(&b.0))
    };
    if out.len() > limit {
        // 全件を並べ替えず、上位だけを選んでから並べる
        out.select_nth_unstable_by(limit, order);
        out.truncate(limit);
    }
    out.sort_unstable_by(order);
    (out.into_iter().map(|(i, _)| &rows[i]).collect(), matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::Totals;

    /// 番号が大きいほど新しいセッション。表示順は新しい順とは限らない
    fn row(index: usize, turns: usize, failures: usize) -> SessionRow {
        SessionRow {
            index,
            start: index as f64 * 60.0,
            total: Totals { sessions: 1, turns, failures, ..Totals::default() },
        }
    }

    #[test]
    fn ranking_selects_the_top_rows_with_newer_sessions_first_on_ties() {
        let mut rows: Vec<_> = (0..20).map(|i| row(i, i % 5, usize::from(i % 7 == 0))).collect();
        rows.swap(3, 17);
        let (top, matched) = ranked(&rows, Metric::Turns, 4);
        assert_eq!(matched, 16);
        assert_eq!(top.iter().map(|r| r.index).collect::<Vec<_>>(), [19, 14, 9, 4]);
        let (all, _) = ranked(&rows, Metric::Turns, 50);
        assert_eq!(all.len(), 16);
        assert!(all.windows(2).all(|w| w[0].total.turns >= w[1].total.turns));
        // 失敗のないセッションは並べない
        let (failed, matched) = ranked(&rows, Metric::Failures, 8);
        assert_eq!((failed.iter().map(|r| r.index).collect::<Vec<_>>(), matched), (vec![14, 7, 0], 3));
        // セッション数はどれも 1 件なので、開始の新しい順
        let (newest, matched) = ranked(&rows, Metric::Sessions, 3);
        assert_eq!((newest.iter().map(|r| r.index).collect::<Vec<_>>(), matched), (vec![19, 18, 17], 20));
        assert!(ranked(&[], Metric::Cost, 8).0.is_empty());
    }
}
