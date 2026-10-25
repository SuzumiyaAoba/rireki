//! 統計の端末描画と表示形式。数値の精度・文字幅は端末の規則に従う。

use super::{BINS, CostRow, Metric, Period, State, Summary, Tab, cost_timeline};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use rireki::model::{Snapshot, UsageSum, format_number as number};
use rireki::time::local;
use unicode_segmentation::UnicodeSegmentation;

const SOURCE_COLORS: [Color; 2] = [Color::Magenta, Color::Cyan];
const WEEKDAYS: [&str; 7] = ["月", "火", "水", "木", "金", "土", "日"];

trait MetricFormat {
    fn format(self, value: f64) -> String;
}

impl MetricFormat for Metric {
    fn format(self, value: f64) -> String {
        if self == Self::Cost {
            usd(value)
        } else {
            number(value.round() as u64)
        }
    }
}

pub(in crate::tui) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut State,
    snapshot: &Snapshot,
) {
    if area.is_empty() {
        return;
    }
    let [heading, tabs, content] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let range = state
        .summary
        .as_ref()
        .map_or_else(String::new, |s| format!("{}〜{}", s.from, s.to));
    frame.render_widget(
        Line::from(vec![
            Span::styled(
                format!(
                    " {} · {} · {} ",
                    period_label(state.period),
                    state.metric.label(),
                    range
                ),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(if state.loading {
                "集計中…"
            } else {
                "[ ] 期間  m 指標"
            }),
        ]),
        heading,
    );
    frame.render_widget(
        Line::from(
            Tab::ALL
                .iter()
                .map(|&tab| {
                    Span::styled(
                        format!(" {} ", tab.label()),
                        if state.tab == tab {
                            Style::default().bg(Color::Blue).fg(Color::White)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    )
                })
                .collect::<Vec<_>>(),
        ),
        tabs,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} · Tab:切替 ", state.tab.label()));
    let inner = block.inner(content);
    frame.render_widget(block, content);
    if inner.is_empty() {
        return;
    }
    state.page = usize::from(inner.height).saturating_sub(5).max(1);
    let Some(summary) = &state.summary else {
        frame.render_widget(
            Paragraph::new(if state.loading {
                "統計を集計しています…"
            } else {
                "統計はまだありません"
            }),
            inner,
        );
        return;
    };
    match state.tab {
        Tab::Overview => draw_overview(frame, inner, state, summary),
        Tab::Trend => draw_trend(frame, inner, state, summary),
        Tab::Hours => draw_hours(frame, inner, state, summary),
        Tab::Sources => draw_sources(frame, inner, state, summary),
        Tab::Models => draw_models(frame, inner, state, summary),
        Tab::Projects => draw_projects(frame, inner, state, summary),
        Tab::Tools => draw_tools(frame, inner, state, summary),
        Tab::Costs => draw_costs(frame, inner, state, snapshot),
    }
}

fn draw_overview(frame: &mut Frame<'_>, area: Rect, state: &State, summary: &Summary) {
    let mut lines = vec![Line::from(
        "指標                    今期                  前期比（同じ日数）",
    )];
    for metric in Metric::ALL {
        let value = metric.total(&summary.total);
        let delta = match &summary.previous {
            Some(previous) => format!(
                "{} (前期 {})",
                comparison(value, Some(metric.total(previous))),
                metric.format(metric.total(previous))
            ),
            None => format!(
                "稼働日あたり {}",
                metric.format(value / summary.active_days.max(1) as f64)
            ),
        };
        lines.push(Line::from(vec![Span::styled(
            format!(
                "{} {} {:>18}   {}",
                if metric == state.metric { "›" } else { " " },
                padded(metric.label(), 14),
                metric.format(value),
                delta
            ),
            if metric == state.metric {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            },
        )]));
        let sources = summary.sources.each_ref().map(|t| metric.total(t));
        lines.push(Line::from(format!(
            "   Claude {} ({}) · Codex {} ({})",
            metric.format(sources[0]),
            percent(sources[0], value),
            metric.format(sources[1]),
            percent(sources[1], value)
        )));
    }
    lines.push(Line::from(""));
    lines.extend(usage_lines(&summary.total.usage));
    lines.push(Line::from(format!(
        "ツール失敗・拒否・中断 {} 回 ({})",
        number(summary.total.failures as u64),
        percent(summary.total.failures as f64, summary.total.tools as f64)
    )));
    lines.push(Line::from(format!(
        "稼働 {} / {} 日 · 使用量の記録 {} / {} セッション",
        summary.active_days,
        summary.days(),
        summary.recorded_sessions,
        summary.total.sessions
    )));
    lines.push(Line::from(
        "料金はログの使用量と料金表による API 換算 (USD)。契約の請求額とは異なります。",
    ));
    lines.push(Line::from("Enter: この期間をタイムラインで表示"));
    let max_scroll = lines.len().saturating_sub(usize::from(area.height));
    frame.render_widget(
        Paragraph::new(lines).scroll((state.selected.min(max_scroll) as u16, state.horizontal)),
        area,
    );
}

fn draw_trend(frame: &mut Frame<'_>, area: Rect, state: &State, summary: &Summary) {
    let max = summary
        .activity
        .iter()
        .map(|b| {
            b.sources
                .iter()
                .map(|a| state.metric.amount(a))
                .sum::<f64>()
        })
        .fold(0.0, f64::max)
        .max(f64::MIN_POSITIVE);
    let width = usize::from(area.width).saturating_sub(42).min(48);
    let lines = summary
        .activity
        .iter()
        .map(|bucket| {
            let values = bucket.sources.each_ref().map(|a| state.metric.amount(a));
            let label = if bucket.from == bucket.to {
                bucket.from.to_string()
            } else {
                format!("{}〜{}", bucket.from, bucket.to)
            };
            let mut spans = vec![Span::raw(format!(
                "{label:<23} {:>14} ",
                state.metric.format(values[0] + values[1])
            ))];
            spans.extend(stacked(values, max, width));
            spans.push(Span::raw(format!(
                " C:{} D:{}",
                state.metric.format(values[0]),
                state.metric.format(values[1])
            )));
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    let note = summary
        .activity
        .get(state.selected)
        .map_or_else(String::new, |b| {
            format!(
                "{}〜{} · Claude {} / Codex {} · Enter:期間を開く",
                b.from,
                b.to,
                state.metric.format(state.metric.amount(&b.sources[0])),
                state.metric.format(state.metric.amount(&b.sources[1]))
            )
        });
    list_with_detail(
        frame,
        area,
        &lines,
        state.selected,
        state.horizontal,
        vec![Line::from(note)],
    );
}

fn draw_hours(frame: &mut Frame<'_>, area: Rect, state: &State, summary: &Summary) {
    let cell = state.selected.min(167);
    let day = cell / 24;
    let hour = cell % 24;
    let max = summary
        .hours
        .iter()
        .map(|a| state.metric.amount(a))
        .fold(0.0, f64::max)
        .max(f64::MIN_POSITIVE);
    let symbols = ['·', '░', '▒', '▓', '█'];
    let mut lines = vec![Line::from(
        "    00    03    06    09    12    15    18    21",
    )];
    for (weekday, label) in WEEKDAYS.iter().enumerate() {
        let mut spans = vec![Span::raw(format!("{label}  "))];
        for h in 0..24 {
            let i = weekday * 24 + h;
            let value = state.metric.amount(&summary.hours[i]);
            let level = if value == 0.0 {
                0
            } else {
                (1.0 + 3.0 * value / max).round().clamp(1.0, 4.0) as usize
            };
            let style = if i == cell {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default().fg(Color::Cyan)
            };
            spans.push(Span::styled(format!("{} ", symbols[level]), style));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "選択: {}曜日 {:02}:00–{:02}:00  {} {}",
        WEEKDAYS[day],
        hour,
        hour + 1,
        state.metric.label(),
        state
            .metric
            .format(state.metric.amount(&summary.hours[cell]))
    )));
    lines.push(Line::from(format!(
        "{}曜日の合計 {} · 毎日 {:02}時台の合計 {}",
        WEEKDAYS[day],
        state
            .metric
            .format(state.metric.amount(&summary.by_weekday[day])),
        hour,
        state
            .metric
            .format(state.metric.amount(&summary.by_hour[hour]))
    )));
    lines.push(Line::from(
        "↑↓:曜日  ←→/h/l:時刻  · 選択した期間のローカル時刻で集計",
    ));
    lines.push(Line::from("曜日別合計"));
    for (i, amount) in summary.by_weekday.iter().enumerate() {
        lines.push(Line::from(format!(
            " {} {}",
            WEEKDAYS[i],
            state.metric.format(state.metric.amount(amount))
        )));
    }
    // 小さい端末でも選択中のマスの数値を先頭に置き、値が見えない状態を避ける。
    if area.height < 12 || area.width < 52 {
        let mut compact = vec![lines[9].clone(), lines[10].clone(), lines[11].clone()];
        compact.extend((0..24).map(|h| {
            Line::from(format!(
                "{} {:02}:00 {}",
                if h == hour { "›" } else { " " },
                h,
                state
                    .metric
                    .format(state.metric.amount(&summary.hours[day * 24 + h]))
            ))
        }));
        frame.render_widget(Paragraph::new(compact).wrap(Wrap { trim: false }), area);
    } else {
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn draw_sources(frame: &mut Frame<'_>, area: Rect, state: &State, summary: &Summary) {
    let sources = [&summary.sources[0], &summary.sources[1], &summary.total];
    let lines = sources
        .iter()
        .enumerate()
        .map(|(i, totals)| {
            Line::from(vec![Span::styled(
                format!(
                    "{:<7}  {} セッション · {} ターン · {} ツール · {} tokens · {}",
                    ["Claude", "Codex", "合計"][i],
                    number(totals.sessions as u64),
                    number(totals.turns as u64),
                    number(totals.tools as u64),
                    number(totals.usage.total()),
                    cost_label(&totals.usage)
                ),
                Style::default().fg([Color::Magenta, Color::Cyan, Color::White][i]),
            )])
        })
        .collect::<Vec<_>>();
    let selected = sources[state.selected.min(2)];
    let mut details = usage_lines(&selected.usage);
    details.push(Line::from(format!(
        "失敗・拒否・中断 {} 回 ({})",
        selected.failures,
        percent(selected.failures as f64, selected.tools as f64)
    )));
    if selected.turns > 0 {
        let turns = selected.turns as f64;
        let cost = if selected.usage.calls > selected.usage.unpriced {
            usd(selected.usage.cost / turns)
        } else {
            "—".into()
        };
        details.push(Line::from(format!(
            "1ターンあたり: {} tokens · {} · {:.1} ツール",
            number((selected.usage.total() as f64 / turns).round() as u64),
            cost,
            selected.tools as f64 / turns
        )));
    } else {
        details.push(Line::from("1ターンあたり: — (発話記録なし)"));
    }
    list_with_detail(
        frame,
        area,
        &lines,
        state.selected,
        state.horizontal,
        details,
    );
}

fn draw_models(frame: &mut Frame<'_>, area: Rect, state: &State, summary: &Summary) {
    let indices = state.model_indices();
    let lines = indices
        .iter()
        .map(|&i| {
            let (model, sources) = &summary.models[i];
            let mut usage = sources[0];
            usage.merge(&sources[1]);
            Line::from(format!(
                "{}  {} tokens · {} · キャッシュ読込 {}",
                model_name(model),
                number(usage.total()),
                cost_label(&usage),
                rate(usage.cache_hit_rate())
            ))
        })
        .collect::<Vec<_>>();
    let details = summary
        .models
        .get(indices.get(state.selected).copied().unwrap_or(usize::MAX))
        .map_or_else(Vec::new, |(model, sources)| {
            let mut usage = sources[0];
            usage.merge(&sources[1]);
            let mut details = vec![Line::from(format!(
                "{} · Claude {} / Codex {} tokens",
                model_name(model),
                number(sources[0].total()),
                number(sources[1].total())
            ))];
            details.extend(usage_lines(&usage));
            details
        });
    list_with_detail(
        frame,
        area,
        &lines,
        state.selected,
        state.horizontal,
        details,
    );
}

fn draw_projects(frame: &mut Frame<'_>, area: Rect, state: &State, summary: &Summary) {
    let indices = state.project_indices();
    let total = state.metric.total(&summary.total);
    let lines = indices
        .iter()
        .map(|&i| {
            let (project, sources) = &summary.projects[i];
            let value = sources.iter().map(|t| state.metric.total(t)).sum::<f64>();
            Line::from(format!(
                "{}  {} ({}) · C:{} D:{}",
                safe(project),
                state.metric.format(value),
                percent(value, total),
                state.metric.format(state.metric.total(&sources[0])),
                state.metric.format(state.metric.total(&sources[1]))
            ))
        })
        .collect::<Vec<_>>();
    let details = indices.get(state.selected).map_or_else(Vec::new, |&i| {
        let (project, sources) = &summary.projects[i];
        let mut total = sources[0].clone();
        total.merge(&sources[1]);
        let mut lines = vec![Line::from(format!(
            "{} · {} セッション · {} ターン · Enter:絞り込む",
            safe(project),
            total.sessions,
            total.turns
        ))];
        lines.extend(usage_lines(&total.usage));
        lines
    });
    list_with_detail(
        frame,
        area,
        &lines,
        state.selected,
        state.horizontal,
        details,
    );
}

fn draw_tools(frame: &mut Frame<'_>, area: Rect, state: &State, summary: &Summary) {
    let lines = summary
        .tools
        .iter()
        .map(|tool| {
            let calls = tool.sources[0].calls + tool.sources[1].calls;
            let failed = tool.sources[0].failures + tool.sources[1].failures;
            Line::from(format!(
                "{}{} · {} 回 · 失敗 {} ({}) · Claude {} / Codex {}",
                if tool.agent { "[agent] " } else { "" },
                safe(&tool.name),
                number(calls as u64),
                failed,
                percent(failed as f64, calls as f64),
                tool.sources[0].calls,
                tool.sources[1].calls
            ))
        })
        .collect::<Vec<_>>();
    let details = summary
        .tools
        .get(state.selected)
        .map_or_else(Vec::new, |tool| {
            vec![
                Line::from(format!(
                    "{} · 失敗には拒否・中断を含みます",
                    safe(&tool.name)
                )),
                Line::from(format!(
                    "Claude: {} 回 / 失敗 {} · Codex: {} 回 / 失敗 {}",
                    tool.sources[0].calls,
                    tool.sources[0].failures,
                    tool.sources[1].calls,
                    tool.sources[1].failures
                )),
            ]
        });
    list_with_detail(
        frame,
        area,
        &lines,
        state.selected,
        state.horizontal,
        details,
    );
}

fn draw_costs(frame: &mut Frame<'_>, area: Rect, state: &State, snapshot: &Snapshot) {
    let Some(costs) = &state.costs else {
        return;
    };
    let rows = state.cost_rows();
    let spark_width = usize::from(area.width).saturating_sub(55).min(BINS);
    let max = costs
        .directories
        .iter()
        .flat_map(|d| &d.buckets)
        .map(|b| b.usage.cost)
        .fold(0.0, f64::max)
        .max(f64::MIN_POSITIVE);
    let lines = rows
        .iter()
        .map(|row| {
            let (name, usage, buckets) = match *row {
                CostRow::Directory(di) => {
                    let d = &costs.directories[di];
                    (
                        format!(
                            "{} {}",
                            if state.collapsed.contains(&d.cwd) || state.directories_only {
                                "▸"
                            } else {
                                "▾"
                            },
                            safe(&d.cwd)
                        ),
                        &d.usage,
                        &d.buckets,
                    )
                }
                CostRow::Session(di, si) => {
                    let s = &costs.directories[di].sessions[si];
                    let title = snapshot
                        .sessions
                        .get(s.session)
                        .map_or("(削除済み)", |s| s.title.as_str());
                    (format!("  {}", safe(title)), &s.usage, &s.buckets)
                }
            };
            let mut spans = vec![Span::raw(format!(
                "{} {:>14} ",
                padded(&name, 28),
                cost_label(usage)
            ))];
            spans.extend(cost_spark(
                buckets,
                costs.view.bins,
                spark_width,
                max,
                state.cost_bin,
            ));
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    let mut details = vec![Line::from(format!(
        "{}〜{} · {} · {} セッション",
        local(costs.view.start).format("%m/%d %H:%M"),
        local(costs.view.end).format("%m/%d %H:%M"),
        cost_label(&costs.usage),
        costs.sessions
    ))];
    let index = state.cost_bin.min(costs.view.bins.saturating_sub(1));
    details.push(Line::from(format!(
        "選択区間 {}〜{} · ←→:区間  Enter:最大料金の呼び出し",
        local(costs.edges[index]).format("%m/%d %H:%M"),
        local(costs.edges[index + 1]).format("%m/%d %H:%M")
    )));
    match state.selected_cost_bucket() {
        Some(bucket) => {
            details.push(Line::from(format!(
                "{} · {} tokens · API {} 回 · 料金不明 {} 回",
                cost_label(&bucket.usage),
                number(bucket.usage.total()),
                bucket.usage.calls,
                bucket.usage.unpriced
            )));
            details.push(Line::from(format!(
                "入力 {} / 読込 {} / 作成 {} / 出力 {} (推論 {})",
                number(bucket.usage.input),
                number(bucket.usage.cache_read),
                number(bucket.usage.cache_write),
                number(bucket.usage.output),
                number(bucket.usage.reasoning)
            )));
        }
        None => details.push(Line::from("この区間の API 使用量はありません")),
    }
    details.push(Line::from(format!(
        "+/-:ズーム H/L:期間移動 0:期間を戻す d:行集約 Space:展開 v:期間内のみ({})",
        if state.in_view_only { "ON" } else { "OFF" }
    )));
    list_with_detail(
        frame,
        area,
        &lines,
        state.selected,
        state.horizontal,
        details,
    );
}

fn list_with_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    lines: &[Line<'static>],
    selected: usize,
    horizontal: u16,
    detail: Vec<Line<'static>>,
) {
    let detail_height = detail
        .len()
        .min(6)
        .min(usize::from(area.height).saturating_sub(1)) as u16;
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(detail_height)]).areas(area);
    if lines.is_empty() {
        frame.render_widget(
            Paragraph::new("この期間に記録はありません")
                .style(Style::default().fg(Color::DarkGray)),
            list_area,
        );
    } else {
        let height = usize::from(list_area.height).max(1);
        let start = selected
            .saturating_sub(height - 1)
            .min(lines.len().saturating_sub(height));
        let visible = lines
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(i, line)| {
                let mut spans = vec![Span::raw(if i == selected { "› " } else { "  " })];
                spans.extend(line.spans.clone());
                Line::from(spans).style(if i == selected {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default()
                })
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(visible).scroll((0, horizontal)), list_area);
    }
    frame.render_widget(
        Paragraph::new(detail)
            .style(Style::default().fg(Color::Cyan))
            .scroll((0, horizontal)),
        detail_area,
    );
}

fn usage_lines(usage: &UsageSum) -> Vec<Line<'static>> {
    vec![
        Line::from(format!(
            "トークン合計 {} · 入力 {} · 出力 {} (うち推論 {})",
            number(usage.total()),
            number(usage.input),
            number(usage.output),
            number(usage.reasoning)
        )),
        Line::from(format!(
            "キャッシュ読込 {} · 作成 {} · 読込率 {}",
            number(usage.cache_read),
            number(usage.cache_write),
            rate(usage.cache_hit_rate())
        )),
        Line::from(format!(
            "API {} 回 · 推定料金 {} · 料金不明 {} 回",
            number(usage.calls),
            cost_label(usage),
            number(usage.unpriced)
        )),
    ]
}

fn stacked(values: [f64; 2], max: f64, width: usize) -> Vec<Span<'static>> {
    let a = (values[0] / max * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    let b = (values[1] / max * width as f64)
        .round()
        .clamp(0.0, width.saturating_sub(a) as f64) as usize;
    vec![
        Span::styled("█".repeat(a), Style::default().fg(SOURCE_COLORS[0])),
        Span::styled("█".repeat(b), Style::default().fg(SOURCE_COLORS[1])),
    ]
}

fn cost_spark(
    buckets: &[cost_timeline::Bucket],
    bins: usize,
    width: usize,
    max: f64,
    selected: usize,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut values = vec![0.0; width];
    let mut present = vec![false; width];
    for bucket in buckets {
        let column = (bucket.index * width / bins.max(1)).min(width - 1);
        values[column] += bucket.usage.cost;
        present[column] = true;
    }
    let max = values.iter().copied().fold(max, f64::max);
    let glyphs = ['·', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let selected_column = selected * width / bins.max(1);
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            let level = if present[i] {
                ((value / max * 7.0).round() as usize + 1).min(8)
            } else {
                0
            };
            Span::styled(
                glyphs[level].to_string(),
                if i == selected_column {
                    Style::default().fg(Color::White).bg(Color::Magenta)
                } else {
                    Style::default().fg(Color::Cyan)
                },
            )
        })
        .collect()
}

fn period_label(period: Period) -> &'static str {
    match period {
        Period::Week => "7日間",
        Period::Month => "30日間",
        Period::Quarter => "90日間",
        Period::All => "全期間",
    }
}

pub(super) fn comparison(current: f64, previous: Option<f64>) -> String {
    match previous {
        None => "比較なし".into(),
        Some(0.0) if current == 0.0 => "±0.0%".into(),
        Some(0.0) => "新規".into(),
        Some(previous) => format!("{:+.1}%", (current - previous) / previous * 100.0),
    }
}

fn usd(value: f64) -> String {
    if value > 0.0 && value < 0.0001 {
        "<$0.0001".into()
    } else if value < 0.01 {
        format!("${value:.4}")
    } else {
        format!("${value:.2}")
    }
}

pub(super) fn cost_label(usage: &UsageSum) -> String {
    if usage.calls == 0 {
        "—".into()
    } else if usage.calls == usage.unpriced {
        "料金不明".into()
    } else if usage.unpriced > 0 {
        format!("{} + 不明", usd(usage.cost))
    } else {
        usd(usage.cost)
    }
}

fn percent(value: f64, total: f64) -> String {
    if total == 0.0 {
        "—".into()
    } else {
        format!("{:.1}%", value / total * 100.0)
    }
}

fn rate(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |v| format!("{:.1}%", v * 100.0))
}
fn model_name(value: &str) -> String {
    if value.is_empty() {
        "(モデル不明)".into()
    } else {
        safe(value)
    }
}
fn safe(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}
pub(super) fn padded(value: &str, width: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for grapheme in value.graphemes(true) {
        let columns = Span::raw(grapheme).width();
        if used + columns > width {
            break;
        }
        out.push_str(grapheme);
        used += columns;
    }
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}
