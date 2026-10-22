//! 統計と API 換算料金の端末表示。集計は worker の Cache で行い、描画は結果だけを読む。

use std::collections::HashSet;

use chrono::{Duration, Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use rireki::cost_timeline;
use rireki::detail::Selection;
use rireki::model::{Snapshot, UsageSum};
use rireki::statistics::{self, Amount, Period, Summary, Totals};
use rireki::time::{local, midnight};

const BINS: usize = 48;
const SOURCE_COLORS: [Color; 2] = [Color::Magenta, Color::Cyan];
const WEEKDAYS: [&str; 7] = ["月", "火", "水", "木", "金", "土", "日"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Metric {
    Tokens,
    Cost,
    Turns,
    Sessions,
    Tools,
}

impl Metric {
    const ALL: [Self; 5] = [
        Self::Tokens,
        Self::Cost,
        Self::Turns,
        Self::Sessions,
        Self::Tools,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Tokens => "トークン",
            Self::Cost => "推定料金",
            Self::Turns => "ターン",
            Self::Sessions => "セッション",
            Self::Tools => "ツール呼び出し",
        }
    }

    fn total(self, value: &Totals) -> f64 {
        match self {
            Self::Tokens => value.usage.total() as f64,
            Self::Cost => value.usage.cost,
            Self::Turns => value.turns as f64,
            Self::Sessions => value.sessions as f64,
            Self::Tools => value.tools as f64,
        }
    }

    fn amount(self, value: &Amount) -> f64 {
        match self {
            Self::Tokens => value.tokens as f64,
            Self::Cost => value.cost,
            Self::Turns => value.turns as f64,
            Self::Sessions => value.sessions as f64,
            Self::Tools => value.tools as f64,
        }
    }

    fn format(self, value: f64) -> String {
        if self == Self::Cost {
            usd(value)
        } else {
            number(value.round() as u64)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Tab {
    #[default]
    Overview,
    Trend,
    Hours,
    Sources,
    Models,
    Projects,
    Tools,
    Costs,
}

impl Tab {
    const ALL: [Self; 8] = [
        Self::Overview,
        Self::Trend,
        Self::Hours,
        Self::Sources,
        Self::Models,
        Self::Projects,
        Self::Tools,
        Self::Costs,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "概要",
            Self::Trend => "推移",
            Self::Hours => "時間帯",
            Self::Sources => "読み込み元",
            Self::Models => "モデル",
            Self::Projects => "プロジェクト",
            Self::Tools => "ツール",
            Self::Costs => "API 料金",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Params {
    pub period: Period,
    pub today: NaiveDate,
    pub cost_view: Option<cost_timeline::View>,
    pub in_view_only: bool,
}

pub(super) struct Computed {
    pub summary: Summary,
    pub costs: cost_timeline::Summary,
}

#[derive(Default)]
pub(super) struct Cache {
    statistics: statistics::Cache,
    costs: cost_timeline::Cache,
}

impl Cache {
    pub(super) fn build(
        &mut self,
        snapshot: &Snapshot,
        scope: &[usize],
        params: Params,
        keep_going: impl Fn() -> bool,
    ) -> Option<Computed> {
        let summary = self.statistics.build_while(
            snapshot,
            scope,
            params.period,
            params.today,
            &keep_going,
        )?;
        let view = params.cost_view.or_else(|| {
            cost_timeline::View::new(
                midnight(summary.from),
                midnight(summary.to + Duration::days(1)),
                BINS,
            )
        })?;
        let costs =
            self.costs
                .build_while(snapshot, scope, view, params.in_view_only, keep_going)?;
        Some(Computed { summary, costs })
    }
}

pub(super) enum Action {
    None,
    Recompute,
    Timeline { start: f64, end: f64 },
    Project(String),
    Event(Selection),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Settings {
    /// 0 は全期間。未知の値は30日間として読む。
    pub period_days: u8,
    pub metric: Metric,
    pub tab: Tab,
    pub in_view_only: bool,
    pub directories_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            period_days: 30,
            metric: Metric::Cost,
            tab: Tab::Overview,
            in_view_only: true,
            directories_only: false,
        }
    }
}

pub(super) struct State {
    pub period: Period,
    pub metric: Metric,
    pub tab: Tab,
    pub loading: bool,
    pub summary: Option<Summary>,
    pub costs: Option<cost_timeline::Summary>,
    pub in_view_only: bool,
    selected: usize,
    page: usize,
    horizontal: u16,
    cost_view: Option<cost_timeline::View>,
    cost_bin: usize,
    directories_only: bool,
    collapsed: HashSet<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            period: Period::Month,
            metric: Metric::Cost,
            tab: Tab::Overview,
            loading: false,
            summary: None,
            costs: None,
            in_view_only: true,
            selected: 0,
            page: 10,
            horizontal: 0,
            cost_view: None,
            cost_bin: 0,
            directories_only: false,
            collapsed: HashSet::new(),
        }
    }
}

impl State {
    pub(super) fn settings(&self) -> Settings {
        Settings {
            period_days: self.period.days().unwrap_or(0) as u8,
            metric: self.metric,
            tab: self.tab,
            in_view_only: self.in_view_only,
            directories_only: self.directories_only,
        }
    }

    pub(super) fn apply_settings(&mut self, settings: Settings) {
        self.period = match settings.period_days {
            7 => Period::Week,
            90 => Period::Quarter,
            0 => Period::All,
            _ => Period::Month,
        };
        self.metric = settings.metric;
        self.tab = settings.tab;
        self.in_view_only = settings.in_view_only;
        self.directories_only = settings.directories_only;
        self.selected = 0;
        self.cost_view = None;
    }

    pub(super) fn params(&self) -> Params {
        Params {
            period: self.period,
            today: Local::now().date_naive(),
            cost_view: self.cost_view,
            in_view_only: self.in_view_only,
        }
    }

    pub(super) fn apply(&mut self, computed: Computed) {
        self.summary = Some(computed.summary);
        self.costs = Some(computed.costs);
        self.loading = false;
        self.clamp_selection();
        if let Some(costs) = &self.costs {
            self.cost_bin = self.cost_bin.min(costs.view.bins.saturating_sub(1));
        }
    }

    pub(super) fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = 0;
        self.horizontal = 0;
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Left => self.horizontal = self.horizontal.saturating_sub(4),
                KeyCode::Right => self.horizontal = self.horizontal.saturating_add(4),
                _ => {}
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let periods = [Period::Week, Period::Month, Period::Quarter, Period::All];
                let current = periods.iter().position(|p| *p == self.period).unwrap_or(1);
                let step = if key.code == KeyCode::Char('[') { 3 } else { 1 };
                self.period = periods[(current + step) % 4];
                self.cost_view = None;
                self.selected = 0;
                return Action::Recompute;
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                let current = Metric::ALL
                    .iter()
                    .position(|m| *m == self.metric)
                    .unwrap_or(0);
                self.metric =
                    Metric::ALL[(current + if key.code == KeyCode::Char('M') { 4 } else { 1 }) % 5];
                self.selected = 0;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let current = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                self.set_tab(
                    Tab::ALL[(current + if key.code == KeyCode::BackTab { 7 } else { 1 }) % 8],
                );
            }
            KeyCode::Char('c') => self.set_tab(Tab::Costs),
            KeyCode::Char('0') if self.tab == Tab::Costs => {
                self.cost_view = None;
                self.cost_bin = 0;
                return Action::Recompute;
            }
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-')
                if self.tab == Tab::Costs =>
            {
                if let Some(costs) = &self.costs {
                    let factor = if key.code == KeyCode::Char('-') {
                        2.0
                    } else {
                        0.5
                    };
                    let start = costs.view.start;
                    let end = costs.view.end;
                    let center = (start + end) * 0.5;
                    let span = ((end - start) * factor).clamp(1.0, 3_155_760_000.0);
                    self.cost_view =
                        cost_timeline::View::new(center - span / 2.0, center + span / 2.0, BINS);
                    return Action::Recompute;
                }
            }
            KeyCode::Char('H') | KeyCode::Char('L') if self.tab == Tab::Costs => {
                if let Some(costs) = &self.costs {
                    let delta = (costs.view.end - costs.view.start) / 2.0
                        * if key.code == KeyCode::Char('H') {
                            -1.0
                        } else {
                            1.0
                        };
                    self.cost_view = cost_timeline::View::new(
                        costs.view.start + delta,
                        costs.view.end + delta,
                        BINS,
                    );
                    return Action::Recompute;
                }
            }
            KeyCode::Char('v') if self.tab == Tab::Costs => {
                self.in_view_only = !self.in_view_only;
                return Action::Recompute;
            }
            KeyCode::Char('d') if self.tab == Tab::Costs => {
                self.directories_only = !self.directories_only;
                self.clamp_selection();
            }
            KeyCode::Char(' ') if self.tab == Tab::Costs => {
                if let Some(CostRow::Directory(di)) = self.cost_rows().get(self.selected).copied()
                    && let Some(directory) = self.costs.as_ref().and_then(|s| s.directories.get(di))
                {
                    let cwd = directory.cwd.clone();
                    if !self.collapsed.remove(&cwd) {
                        self.collapsed.insert(cwd);
                    }
                    self.clamp_selection();
                }
            }
            KeyCode::Left | KeyCode::Char('h') if self.tab == Tab::Costs => {
                self.cost_bin = self.cost_bin.saturating_sub(1)
            }
            KeyCode::Right | KeyCode::Char('l') if self.tab == Tab::Costs => {
                self.cost_bin = (self.cost_bin + 1).min(
                    self.costs
                        .as_ref()
                        .map_or(BINS, |s| s.view.bins)
                        .saturating_sub(1),
                );
            }
            KeyCode::Left | KeyCode::Char('h') if self.tab == Tab::Hours => {
                self.selected = self.selected.saturating_sub(1)
            }
            KeyCode::Right | KeyCode::Char('l') if self.tab == Tab::Hours => {
                self.selected = (self.selected + 1).min(167)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected =
                    self.selected
                        .saturating_sub(if self.tab == Tab::Hours { 24 } else { 1 })
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected =
                    self.selected
                        .saturating_add(if self.tab == Tab::Hours { 24 } else { 1 })
            }
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(self.page),
            KeyCode::PageDown => self.selected = self.selected.saturating_add(self.page),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.row_count().saturating_sub(1),
            KeyCode::Left | KeyCode::Char('h') => {
                self.horizontal = self.horizontal.saturating_sub(4)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.horizontal = self.horizontal.saturating_add(4)
            }
            KeyCode::Enter => return self.activate(),
            _ => {}
        }
        self.clamp_selection();
        Action::None
    }

    fn activate(&self) -> Action {
        let Some(summary) = &self.summary else {
            return Action::None;
        };
        match self.tab {
            Tab::Overview => Action::Timeline {
                start: midnight(summary.from),
                end: midnight(summary.to + Duration::days(1)),
            },
            Tab::Trend => summary
                .activity
                .get(self.selected)
                .map_or(Action::None, |bucket| Action::Timeline {
                    start: midnight(bucket.from),
                    end: midnight(bucket.to + Duration::days(1)),
                }),
            Tab::Projects => self
                .project_indices()
                .get(self.selected)
                .map_or(Action::None, |&i| {
                    Action::Project(summary.projects[i].0.clone())
                }),
            Tab::Costs => self
                .selected_cost_bucket()
                .map_or(Action::None, |b| Action::Event(b.sample)),
            _ => Action::None,
        }
    }

    fn row_count(&self) -> usize {
        let Some(s) = &self.summary else {
            return 0;
        };
        match self.tab {
            Tab::Overview => 26,
            Tab::Trend => s.activity.len(),
            Tab::Hours => 168,
            Tab::Sources => 3,
            Tab::Models => s.models.len(),
            Tab::Projects => s.projects.len(),
            Tab::Tools => s.tools.len(),
            Tab::Costs => self.cost_rows().len(),
        }
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
    }

    fn project_indices(&self) -> Vec<usize> {
        let Some(summary) = &self.summary else {
            return Vec::new();
        };
        let mut indices: Vec<usize> = (0..summary.projects.len()).collect();
        indices.sort_by(|&a, &b| {
            let value = |i: usize| {
                summary.projects[i]
                    .1
                    .iter()
                    .map(|t| self.metric.total(t))
                    .sum::<f64>()
            };
            value(b).total_cmp(&value(a))
        });
        indices
    }

    fn model_indices(&self) -> Vec<usize> {
        let Some(summary) = &self.summary else {
            return Vec::new();
        };
        let mut indices: Vec<usize> = (0..summary.models.len()).collect();
        if self.metric == Metric::Cost {
            indices.sort_by(|&a, &b| {
                let cost = |i: usize| summary.models[i].1.iter().map(|u| u.cost).sum::<f64>();
                cost(b).total_cmp(&cost(a))
            });
        }
        indices
    }

    fn cost_rows(&self) -> Vec<CostRow> {
        let Some(summary) = &self.costs else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (di, directory) in summary.directories.iter().enumerate() {
            rows.push(CostRow::Directory(di));
            if !self.directories_only && !self.collapsed.contains(&directory.cwd) {
                rows.extend((0..directory.sessions.len()).map(|si| CostRow::Session(di, si)));
            }
        }
        rows
    }

    fn selected_cost_bucket(&self) -> Option<&cost_timeline::Bucket> {
        let costs = self.costs.as_ref()?;
        let row = self.cost_rows().get(self.selected).copied()?;
        let buckets = match row {
            CostRow::Directory(di) => &costs.directories[di].buckets,
            CostRow::Session(di, si) => &costs.directories[di].sessions[si].buckets,
        };
        buckets.iter().find(|b| b.index == self.cost_bin)
    }
}

#[derive(Clone, Copy)]
enum CostRow {
    Directory(usize),
    Session(usize, usize),
}

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, state: &mut State, snapshot: &Snapshot) {
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

fn comparison(current: f64, previous: Option<f64>) -> String {
    match previous {
        None => "比較なし".into(),
        Some(0.0) if current == 0.0 => "±0.0%".into(),
        Some(0.0) => "新規".into(),
        Some(previous) => format!("{:+.1}%", (current - previous) / previous * 100.0),
    }
}

fn number(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
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

fn cost_label(usage: &UsageSum) -> String {
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
fn padded(value: &str, width: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rireki::model::{Event, Kind, Lane, Session, Source, Status, TextRef, Usage};
    use std::sync::Arc;

    fn fixture() -> (Snapshot, NaiveDate) {
        let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        let mut lane = Lane::new("main");
        for (offset, model, kind) in [
            (0, "claude-sonnet-4-5", Kind::Assistant),
            (-8, "unknown-model", Kind::Assistant),
            (0, "", Kind::Tool),
            (0, "", Kind::User),
        ] {
            let start = midnight(today + Duration::days(offset)) + 3600.0;
            lane.events.push(Event {
                start,
                end: start + 1.0,
                kind,
                name: "日本語ツール\u{1b}".into(),
                summary: "summary".into(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: TextRef::default(),
                usage: if kind == Kind::Assistant {
                    Usage {
                        input: 100,
                        cache_read: 200,
                        cache_write: 40,
                        output: 50,
                        reasoning: 20,
                        ..Usage::default()
                    }
                } else {
                    Usage::default()
                },
                model: (!model.is_empty()).then(|| model.into()),
                status: if kind == Kind::Tool {
                    Status::Error
                } else {
                    Status::Ok
                },
            });
        }
        lane.finish();
        let mut session = Session {
            id: "one".into(),
            source: Source::Claude,
            cwd: "/日本語/project".into(),
            headless: false,
            title: "日本語 title".into(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        (
            Snapshot {
                sessions: vec![Arc::new(session)],
            },
            today,
        )
    }

    fn computed(period: Period) -> (Snapshot, Computed) {
        let (snapshot, today) = fixture();
        let params = Params {
            period,
            today,
            cost_view: None,
            in_view_only: true,
        };
        let result = Cache::default()
            .build(&snapshot, &[0], params, || true)
            .unwrap();
        (snapshot, result)
    }

    #[test]
    fn metrics_use_core_scope_previous_period_and_unknown_prices() {
        let (snapshot, result) = computed(Period::Week);
        assert_eq!(result.summary.total.usage.total(), 390);
        assert_eq!(result.summary.total.failures, 1);
        assert_eq!(result.summary.previous.as_ref().unwrap().usage.total(), 390);
        assert_eq!(result.summary.previous.as_ref().unwrap().usage.unpriced, 1);
        assert_eq!(
            result.costs.usage.total(),
            result.summary.total.usage.total()
        );
        let params = Params {
            period: Period::All,
            today: result.summary.to,
            cost_view: None,
            in_view_only: false,
        };
        let empty = Cache::default()
            .build(&snapshot, &[], params, || true)
            .unwrap();
        assert_eq!(empty.summary.total.sessions, 0);
        assert!(empty.costs.directories.is_empty());
        assert!(
            Cache::default()
                .build(&snapshot, &[0], params, || false)
                .is_none()
        );
    }

    #[test]
    fn period_navigation_drills_down_to_exclusive_local_day_end() {
        let (_, result) = computed(Period::Week);
        let today = result.summary.to;
        let mut state = State::default();
        state.apply(result);
        state.set_tab(Tab::Trend);
        state.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        let Action::Timeline { start, end } =
            state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("timeline action");
        };
        assert_eq!(start, midnight(today));
        assert_eq!(end, midnight(today + Duration::days(1)));
        state.set_tab(Tab::Projects);
        assert!(
            matches!(state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Action::Project(p) if p == "/日本語/project")
        );
        assert!(matches!(
            state.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)),
            Action::Recompute
        ));
        assert_eq!(state.period, Period::Quarter);
    }

    #[test]
    fn cost_bucket_opens_actual_call_and_zoom_preserves_valid_range() {
        let (_, result) = computed(Period::Week);
        let mut state = State::default();
        state.apply(result);
        state.set_tab(Tab::Costs);
        state.cost_bin = state.costs.as_ref().unwrap().directories[0].buckets[0].index;
        assert!(matches!(
            state.activate(),
            Action::Event(Selection {
                session: 0,
                lane: 0,
                ..
            })
        ));
        let original = state.costs.as_ref().unwrap().view;
        assert!(matches!(
            state.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE)),
            Action::Recompute
        ));
        let zoomed = state.params().cost_view.unwrap();
        assert_eq!(
            zoomed.end - zoomed.start,
            (original.end - original.start) / 2.0
        );
        assert!(matches!(
            state.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE)),
            Action::Recompute
        ));
        assert!(state.params().cost_view.is_none());
    }

    #[test]
    fn metrics_render_every_tab_in_tiny_and_wide_terminals_without_control_text() {
        let (snapshot, result) = computed(Period::All);
        let mut state = State::default();
        state.apply(result);
        for (width, height) in [(1, 1), (24, 7), (80, 24), (160, 45)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for tab in Tab::ALL {
                state.set_tab(tab);
                terminal
                    .draw(|frame| draw(frame, frame.area(), &mut state, &snapshot))
                    .unwrap();
                // 全角文字の後続セルは空白なので、表示幅ぶん進めて画面を復元する。
                let buffer = terminal.backend().buffer();
                let mut text = String::new();
                for y in buffer.area.top()..buffer.area.bottom() {
                    let mut x = buffer.area.left();
                    while x < buffer.area.right() {
                        let symbol = buffer[(x, y)].symbol();
                        text.push_str(symbol);
                        x += Span::raw(symbol).width().max(1) as u16;
                    }
                    text.push('\n');
                }
                assert!(!text.contains('\u{1b}'));
                if width >= 80 && height >= 24 {
                    assert!(
                        text.contains(tab.label()),
                        "{width}x{height} {tab:?}: {text}"
                    );
                }
            }
        }
    }

    #[test]
    fn saved_metrics_preferences_round_trip_and_unicode_columns_keep_graphemes() {
        let mut state = State {
            period: Period::All,
            metric: Metric::Tokens,
            ..State::default()
        };
        state.set_tab(Tab::Hours);
        state.in_view_only = false;
        let saved = serde_json::to_string(&state.settings()).unwrap();
        let mut restored = State::default();
        restored.apply_settings(serde_json::from_str(&saved).unwrap());
        assert_eq!(restored.period, Period::All);
        assert_eq!(restored.metric, Metric::Tokens);
        assert_eq!(restored.tab, Tab::Hours);
        assert!(!restored.in_view_only);
        let missing: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.period_days, 30);
        assert_eq!(Span::raw(padded("日本語", 4)).width(), 4);
        assert_eq!(padded("e\u{301}x", 1), "e\u{301}");
    }

    #[test]
    fn unknown_prices_and_zero_previous_are_not_reported_as_free_or_infinite() {
        assert_eq!(
            cost_label(&UsageSum {
                calls: 1,
                unpriced: 1,
                ..UsageSum::default()
            }),
            "料金不明"
        );
        assert_eq!(comparison(20.0, Some(0.0)), "新規");
        assert_eq!(comparison(0.0, Some(0.0)), "±0.0%");
        assert_eq!(comparison(20.0, None), "比較なし");
        assert_eq!(number(1_234_567), "1,234,567");
    }
}
