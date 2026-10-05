//! 全履歴の統計画面。本文を読み込まず、集計は背景で行いデータと期間が変わったときだけ更新する。

use std::collections::HashMap;

use chrono::{Duration, Local, NaiveDate};
use gpui::{Context, Div, ElementId, FontWeight, ScrollHandle, Window, div, point, prelude::*, px};

use super::detail::{fmt_tokens, fmt_usd, usage_view};
use super::widgets::tip;
use super::{Rireki, fmt_n, local, midnight, tilde};
use crate::icons::{self, Icon};
use crate::model::{Event, Kind, Snapshot, Source, UsageSum};
use crate::prefs::SourceFilter;
use crate::theme::{self, c};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Period {
    Week,
    Month,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Tokens,
    Turns,
    Sessions,
}

impl Metric {
    fn label(self) -> &'static str {
        match self {
            Self::Tokens => "トークン",
            Self::Turns => "ターン",
            Self::Sessions => "セッション",
        }
    }

    fn value(self, a: &Activity) -> u64 {
        match self {
            Self::Tokens => a.tokens,
            Self::Turns => a.turns,
            Self::Sessions => a.sessions,
        }
    }
}

pub(super) struct State {
    period: Period,
    metric: Metric,
    scroll: ScrollHandle,
    key: Option<(u64, Period, NaiveDate)>,
    task: Option<gpui::Task<()>>,
    summary: Option<Summary>,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            period: Period::Month,
            metric: Metric::Tokens,
            scroll: ScrollHandle::new(),
            key: None,
            task: None,
            summary: None,
        }
    }
}

#[derive(Default)]
struct Totals {
    sessions: usize,
    turns: usize,
    tools: usize,
    failures: usize,
    usage: UsageSum,
}

impl Totals {
    fn add(&mut self, e: &Event) {
        self.turns += usize::from(e.kind == Kind::User);
        self.tools += usize::from(matches!(e.kind, Kind::Tool | Kind::Agent));
        self.failures += usize::from(e.status.failed());
        self.usage.add(&e.usage, e.model.as_deref());
    }
}

struct Activity {
    from: NaiveDate,
    to: NaiveDate,
    tokens: u64,
    turns: u64,
    sessions: u64,
}

struct Summary {
    from: NaiveDate,
    to: NaiveDate,
    total: Totals,
    sources: [Totals; 2],
    models: Vec<(String, UsageSum)>,
    projects: Vec<(String, Totals)>,
    activity: Vec<Activity>,
    recorded_sessions: usize,
}

impl Summary {
    fn build(store: &Snapshot, order: &[usize], period: Period, today: NaiveDate) -> Self {
        let from = match period {
            Period::Week => today - Duration::days(6),
            Period::Month => today - Duration::days(29),
            Period::All => order
                .iter()
                .map(|&si| store.sessions[si].start)
                .filter(|t| t.is_finite())
                .map(|t| local(t).date_naive())
                .min()
                .unwrap_or(today)
                .min(today),
        };
        let end = today + Duration::days(1);
        let (start_t, end_t) = (midnight(from), midnight(end));
        let days = (end - from).num_days();
        // 長い全期間でもグラフの列数を 30 以下に保つ。日付の差で区切るので夏時間でも日がずれない。
        let bin_days = (days + 29) / 30;
        let mut out = Self {
            from,
            to: today,
            total: Totals::default(),
            sources: Default::default(),
            models: Vec::new(),
            projects: Vec::new(),
            activity: (0..days)
                .step_by(bin_days as usize)
                .map(|offset| Activity {
                    from: from + Duration::days(offset),
                    to: (from + Duration::days(offset + bin_days - 1)).min(today),
                    tokens: 0,
                    turns: 0,
                    sessions: 0,
                })
                .collect(),
            recorded_sessions: 0,
        };
        let mut models: HashMap<&str, UsageSum> = HashMap::new();
        let mut projects: HashMap<&str, Totals> = HashMap::new();
        for &si in order {
            let session = &store.sessions[si];
            let source = usize::from(session.source == Source::Codex);
            let mut active = false;
            let mut recorded = false;
            let mut bins = vec![false; out.activity.len()];
            for lane in &session.lanes {
                // events は開始時刻順。期間外を二分探索で飛ばす。
                let first = lane.events.partition_point(|e| e.start < start_t);
                let last = lane.events.partition_point(|e| e.start < end_t);
                for e in &lane.events[first..last] {
                    if !e.start.is_finite() {
                        continue;
                    }
                    active = true;
                    recorded |= !e.usage.is_empty();
                    out.total.add(e);
                    out.sources[source].add(e);
                    projects.entry(&session.cwd).or_default().add(e);
                    if !e.usage.is_empty() {
                        models
                            .entry(e.model.as_deref().unwrap_or("モデル不明"))
                            .or_default()
                            .add(&e.usage, e.model.as_deref());
                    }
                    let day = local(e.start).date_naive();
                    let bin = ((day - from).num_days() / bin_days) as usize;
                    if let Some(a) = out.activity.get_mut(bin) {
                        a.tokens += e.usage.total();
                        a.turns += u64::from(e.kind == Kind::User);
                        bins[bin] = true;
                    }
                }
            }
            if active {
                out.total.sessions += 1;
                out.sources[source].sessions += 1;
                projects.entry(&session.cwd).or_default().sessions += 1;
                out.recorded_sessions += usize::from(recorded);
            }
            for (a, seen) in out.activity.iter_mut().zip(bins) {
                a.sessions += u64::from(seen);
            }
        }
        out.models = models.into_iter().map(|(m, u)| (m.to_string(), u)).collect();
        out.models
            .sort_by(|a, b| b.1.total().cmp(&a.1.total()).then_with(|| a.0.cmp(&b.0)));
        out.projects = projects.into_iter().map(|(p, t)| (p.to_string(), t)).collect();
        out.projects.sort_by(|a, b| {
            b.1.usage
                .total()
                .cmp(&a.1.usage.total())
                .then_with(|| b.1.turns.cmp(&a.1.turns))
                .then_with(|| a.0.cmp(&b.0))
        });
        out
    }
}

impl Rireki {
    pub(super) fn show_statistics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.statistics_open = true;
        self.popover = None;
        self.hovered = None;
        self.drag = None;
        self.resize = None;
        window.focus(&self.focus_handle);
        self.persist();
        cx.notify();
    }

    fn request_statistics(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        let key = (self.data_gen, self.statistics_state.period, today);
        if self.statistics_state.key == Some(key) {
            return;
        }
        self.statistics_state.key = Some(key);
        self.statistics_state.summary = None;
        let snapshot = self.store.clone();
        let order = self.order.clone();
        self.statistics_state.task = Some(cx.spawn(async move |this, cx| {
            let summary = cx
                .background_executor()
                .spawn(async move { Summary::build(&snapshot, &order, key.1, today) })
                .await;
            let _ = this.update(cx, |t, cx| {
                if t.statistics_state.key == Some(key) {
                    t.statistics_state.summary = Some(summary);
                    cx.notify();
                }
            });
        }));
    }

    pub(super) fn statistics(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        self.request_statistics(cx);
        let narrow = self.main_width(window) < 800.0;
        let source = match self.source {
            SourceFilter::All => "すべての読み込み元",
            SourceFilter::Claude => "Claude Code",
            SourceFilter::Codex => "Codex",
        };
        let scope = self.focus.as_ref().map_or_else(
            || format!("{source} · すべてのプロジェクト"),
            |cwd| format!("{source} · {}", self.project_name(cwd)),
        );
        let periods = self.segmented(
            "statistics-period",
            &[
                (Period::Week, "7日間"),
                (Period::Month, "30日間"),
                (Period::All, "全期間"),
            ],
            Some(self.statistics_state.period),
            cx,
            |t, period, _, _| {
                t.statistics_state.period = period;
                t.statistics_state.scroll.set_offset(point(px(0.0), px(0.0)));
            },
        );
        let header = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(icons::icon(Icon::Chart, theme::ACCENT).size(px(20.0)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("統計"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .truncate()
                            .child(scope),
                    ),
            )
            .when(self.focus.is_some(), |d| {
                d.child(self.ghost_button(
                    "statistics-clear-project",
                    Icon::X,
                    "すべてのプロジェクトに戻る",
                    None,
                    true,
                    cx,
                    |t, _, cx| t.set_focus(None, cx),
                ))
            })
            .child(periods);
        let mut content = div().flex().flex_col().gap_4().p_4();
        if let Some(summary) = &self.statistics_state.summary {
            let total = &summary.total;
            let usage = &total.usage;
            let cost = cost_label(usage);
            content = content
                .child(div().text_xs().text_color(c(theme::MUTED)).child(format!(
                    "{} – {} · ローカル時間 · サブエージェントを含む",
                    summary.from.format("%Y/%m/%d"),
                    summary.to.format("%Y/%m/%d")
                )))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_3()
                        .child(metric_card(
                            "statistics-tokens",
                            "総トークン",
                            fmt_tokens(usage.total()),
                            format!(
                                "{} トークン（入力・キャッシュ・出力の合計）",
                                fmt_n(usage.total() as usize)
                            ),
                            Icon::Layers,
                            theme::ACCENT,
                            narrow,
                        ))
                        .child(metric_card(
                            "statistics-turns",
                            "ターン",
                            fmt_n(total.turns),
                            "期間内のユーザー発話の数（全レーン）".into(),
                            Icon::MessageSquare,
                            theme::OK,
                            narrow,
                        ))
                        .child(metric_card(
                            "statistics-sessions",
                            "セッション",
                            fmt_n(total.sessions),
                            "期間内にイベントのあるセッション。複数レーンは 1 件として数える".into(),
                            Icon::Timeline,
                            theme::SUB,
                            narrow,
                        ))
                        .child(metric_card(
                            "statistics-cost",
                            "推定料金 (USD)",
                            cost,
                            "ログの使用量と料金表による推定。料金不明のモデルは含まない".into(),
                            Icon::Activity,
                            theme::WARN,
                            narrow,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_x_4()
                        .gap_y_1()
                        .text_xs()
                        .text_color(c(theme::MUTED))
                        .child(format!("ツール呼び出し {} 回", fmt_n(total.tools)))
                        .child(format!("失敗・拒否・中断 {} 回", fmt_n(total.failures)))
                        .child(format!(
                            "使用量の記録 {} / {} セッション",
                            fmt_n(summary.recorded_sessions),
                            fmt_n(total.sessions)
                        ))
                        .when(total.turns > 0 && usage.calls > 0, |d| {
                            d.child(format!(
                                "平均 {} トークン / ターン",
                                fmt_tokens(usage.total() / total.turns as u64)
                            ))
                        }),
                );
            if total.sessions == 0 {
                content = content.child(
                    section("この期間の履歴はありません").child(
                        div()
                            .text_sm()
                            .text_color(c(theme::MUTED))
                            .child(if self.loading.is_some() {
                                "履歴を読み込んでいます…"
                            } else {
                                "期間や左の読み込み元を切り替えると、ほかの履歴を集計できます。"
                            }),
                    ),
                );
            } else {
                let chart = self.activity_chart(summary, narrow, cx);
                let tokens = section("トークンの内訳")
                    .when(usage.calls > 0, |d| d.child(usage_view(usage)))
                    .when(usage.calls == 0, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(c(theme::MUTED))
                                .child("この期間のログにはトークン使用量が記録されていません。"),
                        )
                    })
                    .when(summary.recorded_sessions < total.sessions, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(c(theme::MUTED))
                                .child("トークンと料金は、使用量が記録されているイベントのみの集計です。"),
                        )
                    })
                    .when(usage.unpriced > 0, |d| {
                        d.child(div().text_xs().text_color(c(theme::WARN)).child(format!(
                            "料金不明の {} 回は推定料金に含まれていません。",
                            fmt_n(usage.unpriced as usize)
                        )))
                    });
                let sources = section("読み込み元別").children(
                    [Source::Claude, Source::Codex]
                        .into_iter()
                        .zip(&summary.sources)
                        .map(|(source, total)| {
                            let (icon, color) = icons::source_icon(source);
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .py_2()
                                .child(icons::icon(icon, color).size_4())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .child(div().text_sm().child(source.label()))
                                        .child(div().text_xs().text_color(c(theme::MUTED)).child(format!(
                                            "{} セッション · {} ターン",
                                            fmt_n(total.sessions),
                                            fmt_n(total.turns)
                                        ))),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .flex_none()
                                        .flex()
                                        .flex_col()
                                        .items_end()
                                        .gap_0p5()
                                        .child(fmt_tokens(total.usage.total()))
                                        .child(
                                            div().text_color(c(theme::MUTED)).child(cost_label(&total.usage)),
                                        ),
                                )
                        }),
                );
                let models = section("モデル別トークン")
                    .when(summary.models.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(c(theme::MUTED))
                                .child("モデルの使用量の記録はありません。"),
                        )
                    })
                    .children(summary.models.iter().take(10).map(|(model, u)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .child(div().flex_1().min_w_0().truncate().child(model.clone()))
                                    .child(div().flex_none().child(fmt_tokens(u.total())))
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(76.0))
                                            .flex()
                                            .justify_end()
                                            .text_color(c(theme::MUTED))
                                            .child(cost_label(u)),
                                    ),
                            )
                            .child(
                                div().h(px(4.0)).rounded_full().bg(c(theme::GRID)).child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .bg(c(theme::ACCENT))
                                        .w(gpui::relative(u.total() as f32 / usage.total().max(1) as f32)),
                                ),
                            )
                    }));
                let projects = section("プロジェクト別")
                    .child(div().text_xs().text_color(c(theme::MUTED)).child(format!(
                        "{} プロジェクト · 上位10件 · クリックで絞り込み",
                        fmt_n(summary.projects.len())
                    )))
                    .children(
                        summary
                            .projects
                            .iter()
                            .take(10)
                            .enumerate()
                            .map(|(i, (cwd, total))| {
                                let path = cwd.clone();
                                div()
                                    .id(ElementId::NamedInteger("statistics-project".into(), i as u64))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .p_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|d| d.bg(c(theme::HOVER)))
                                    .tooltip(tip(tilde(cwd), None))
                                    .child(icons::icon(Icon::Folder, theme::project_color(cwd)).size_4())
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .gap_0p5()
                                            .child(div().text_sm().truncate().child(self.project_name(cwd)))
                                            .child(div().text_xs().text_color(c(theme::MUTED)).child(
                                                format!(
                                                    "{} セッション · {} ターン",
                                                    fmt_n(total.sessions),
                                                    fmt_n(total.turns)
                                                ),
                                            )),
                                    )
                                    .child(div().flex_none().text_xs().child(fmt_tokens(total.usage.total())))
                                    .on_click(cx.listener(move |t, _, _, cx| {
                                        t.set_focus(Some(path.clone()), cx);
                                        t.statistics_state.scroll.set_offset(point(px(0.0), px(0.0)));
                                        cx.notify();
                                    }))
                            }),
                    );
                content = content.child(chart).child(
                    div()
                        .flex()
                        .gap_4()
                        .when(narrow, |d| d.flex_col())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .child(tokens)
                                .child(models),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .child(sources)
                                .child(projects),
                        ),
                );
            }
        } else {
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .py_8()
                    .text_sm()
                    .text_color(c(theme::MUTED))
                    .child(icons::icon(Icon::Refresh, theme::ACCENT).size_4())
                    .child("統計を集計中…"),
            );
        }
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .id("statistics-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.statistics_state.scroll)
                    .child(content),
            )
    }

    fn activity_chart(&self, summary: &Summary, narrow: bool, cx: &mut Context<Self>) -> Div {
        let metric = self.statistics_state.metric;
        let max = summary
            .activity
            .iter()
            .map(|a| metric.value(a))
            .max()
            .unwrap_or(0);
        let stride = if narrow {
            summary.activity.len().max(1) / 2
        } else {
            5
        };
        let chart = div().flex().items_end().gap(px(3.0)).h(px(168.0)).children(
            summary.activity.iter().enumerate().map(|(i, a)| {
                let value = metric.value(a);
                let height = if value == 0 {
                    2.0
                } else {
                    (value as f32 / max.max(1) as f32 * 140.0).max(3.0)
                };
                let label = if a.from == a.to {
                    a.from.format("%m/%d").to_string()
                } else {
                    format!("{} – {}", a.from.format("%m/%d"), a.to.format("%m/%d"))
                };
                div()
                    .id(ElementId::NamedInteger("statistics-bar".into(), i as u64))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .gap_1()
                    .tooltip(tip(
                        format!("{label} · {} {}", fmt_n(value as usize), metric.label()),
                        None,
                    ))
                    .child(
                        div()
                            .w_full()
                            .h(px(height))
                            .rounded_t(px(3.0))
                            .bg(c(if value > 0 { theme::ACCENT } else { theme::GRID })),
                    )
                    .child(
                        div()
                            .h(px(16.0))
                            .text_size(px(9.0))
                            .text_color(c(theme::SUBTLE))
                            .whitespace_nowrap()
                            .when(
                                i == 0 || i + 1 == summary.activity.len() || i.is_multiple_of(stride.max(1)),
                                |d| d.child(a.from.format("%-m/%-d").to_string()),
                            ),
                    )
            }),
        );
        section("期間内の推移")
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(format!(
                                "{} · 最大 {}{}",
                                if summary.activity.first().is_some_and(|a| a.from != a.to) {
                                    "複数日ごとの集計"
                                } else {
                                    "日別"
                                },
                                fmt_tokens(max),
                                metric.label()
                            )),
                    )
                    .child(self.segmented(
                        "statistics-metric",
                        &[
                            (Metric::Tokens, "トークン"),
                            (Metric::Turns, "ターン"),
                            (Metric::Sessions, "セッション"),
                        ],
                        Some(metric),
                        cx,
                        |t, metric, _, _| t.statistics_state.metric = metric,
                    )),
            )
            .child(chart)
    }
}

fn section(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_lg()
        .bg(c(theme::PANEL))
        .border_1()
        .border_color(c(theme::BORDER))
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
}

#[allow(clippy::too_many_arguments)]
fn metric_card(
    id: &'static str,
    label: &'static str,
    value: String,
    description: String,
    icon: Icon,
    color: u32,
    narrow: bool,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_1()
        .flex_basis(gpui::relative(if narrow { 0.47 } else { 0.23 }))
        .min_w_0()
        .p_4()
        .rounded_lg()
        .bg(c(theme::PANEL))
        .border_1()
        .border_color(c(theme::BORDER))
        .flex()
        .flex_col()
        .gap_3()
        .tooltip(tip(description, None))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(c(theme::MUTED))
                .child(icons::icon(icon, color).size_4())
                .child(label),
        )
        .child(
            div()
                .text_size(px(if narrow { 26.0 } else { 30.0 }))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(color))
                .child(value),
        )
}

fn cost_label(u: &UsageSum) -> String {
    if u.calls > u.unpriced {
        fmt_usd(u.cost)
    } else {
        "—".into()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::model::{Lane, Session, Status, Usage};

    fn event(start: f64, kind: Kind, usage: Usage) -> Event {
        Event {
            start,
            end: start,
            kind,
            usage,
            name: String::new(),
            summary: String::new(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            row: None,
            model: Some("statistics-test-unknown-model".into()),
            status: Status::Ok,
        }
    }

    fn session(source: Source, cwd: &str, lanes: Vec<Vec<Event>>) -> Arc<Session> {
        let mut s = Session {
            id: cwd.into(),
            source,
            cwd: cwd.into(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
            lanes: lanes
                .into_iter()
                .enumerate()
                .map(|(i, events)| {
                    let mut lane = Lane::new(if i == 0 { "main" } else { "agent" });
                    lane.events = events;
                    lane.finish();
                    lane
                })
                .collect(),
        };
        s.finalize();
        Arc::new(s)
    }

    #[test]
    fn aggregates_all_lanes_without_double_counting_sessions_or_reasoning() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let t = midnight(today) + 3600.0;
        let mut tool = event(
            t + 1.0,
            Kind::Tool,
            Usage {
                input: 100,
                cache_read: 40,
                cache_write: 20,
                output: 30,
                reasoning: 10,
                ..Usage::default()
            },
        );
        tool.status = Status::Error;
        let store = Snapshot {
            sessions: vec![
                session(
                    Source::Claude,
                    "/a",
                    vec![
                        vec![event(t, Kind::User, Usage::default()), tool],
                        vec![event(
                            t + 2.0,
                            Kind::User,
                            Usage {
                                input: 5,
                                output: 10,
                                ..Usage::default()
                            },
                        )],
                    ],
                ),
                session(
                    Source::Codex,
                    "/b",
                    vec![vec![event(t, Kind::User, Usage::default())]],
                ),
            ],
        };
        let s = Summary::build(&store, &[0, 1], Period::Week, today);
        assert_eq!(
            (s.total.sessions, s.total.turns, s.total.tools, s.total.failures),
            (2, 3, 1, 1)
        );
        assert_eq!(
            (
                s.total.usage.total(),
                s.total.usage.reasoning,
                s.total.usage.calls
            ),
            (205, 10, 2)
        );
        assert_eq!(s.recorded_sessions, 1);
        assert_eq!(
            (s.sources[0].sessions, s.sources[0].turns, s.sources[1].sessions),
            (1, 2, 1)
        );
        assert_eq!(s.models[0].1.total(), 205);
        assert_eq!(s.projects.len(), 2);
        assert_eq!(s.projects[0].1.usage.total(), 205);
        let day = s.activity.last().unwrap();
        assert_eq!((day.tokens, day.turns, day.sessions), (205, 3, 2));
        assert_eq!(s.total.usage.unpriced, 2);
        assert_eq!(cost_label(&s.total.usage), "—");
    }

    #[test]
    fn period_uses_event_start_and_half_open_local_day_boundaries() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let start = midnight(today - Duration::days(6));
        let end = midnight(today + Duration::days(1));
        let store = Snapshot {
            sessions: vec![session(
                Source::Codex,
                "/a",
                vec![vec![
                    event(
                        start - 0.001,
                        Kind::User,
                        Usage {
                            input: 100,
                            ..Usage::default()
                        },
                    ),
                    event(start, Kind::User, Usage::default()),
                    event(end - 0.001, Kind::User, Usage::default()),
                    event(
                        end,
                        Kind::User,
                        Usage {
                            input: 200,
                            ..Usage::default()
                        },
                    ),
                ]],
            )],
        };
        let s = Summary::build(&store, &[0], Period::Week, today);
        assert_eq!(
            (s.total.sessions, s.total.turns, s.total.usage.total()),
            (1, 2, 0)
        );
        assert_eq!(s.activity.len(), 7);
        assert_eq!(s.activity[0].turns, 1);
        assert_eq!(s.activity[6].turns, 1);
        assert!(
            s.activity[1..6]
                .iter()
                .all(|a| a.tokens == 0 && a.turns == 0 && a.sessions == 0)
        );
    }

    #[test]
    fn respects_scope_and_bounds_large_all_time_charts() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let past = today - Duration::days(3000);
        let store = Snapshot {
            sessions: vec![
                session(
                    Source::Claude,
                    "/excluded",
                    vec![vec![event(midnight(past), Kind::User, Usage::default())]],
                ),
                session(
                    Source::Codex,
                    "/included",
                    vec![vec![
                        event(midnight(past + Duration::days(1)), Kind::User, Usage::default()),
                        event(midnight(today), Kind::User, Usage::default()),
                    ]],
                ),
            ],
        };
        let s = Summary::build(&store, &[1], Period::All, today);
        assert_eq!(s.from, past + Duration::days(1));
        assert_eq!((s.total.sessions, s.total.turns, s.projects.len()), (1, 2, 1));
        assert_eq!(s.sources[0].sessions, 0);
        assert!(s.activity.len() <= 30);
        assert_eq!(s.activity.first().unwrap().from, s.from);
        assert_eq!(s.activity.last().unwrap().to, today);
        assert_eq!(s.activity.iter().map(|a| a.turns).sum::<u64>(), 2);
        assert_eq!(s.activity.iter().map(|a| a.sessions).sum::<u64>(), 2);
    }

    #[test]
    fn empty_scope_and_usage_are_valid() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        for period in [Period::Week, Period::Month, Period::All] {
            let s = Summary::build(&Snapshot::default(), &[], period, today);
            assert_eq!(
                (s.total.sessions, s.total.turns, s.total.usage.total()),
                (0, 0, 0)
            );
            assert!(s.models.is_empty() && s.projects.is_empty());
            assert!(
                s.activity
                    .iter()
                    .all(|a| a.tokens == 0 && a.turns == 0 && a.sessions == 0)
            );
        }
    }
}
