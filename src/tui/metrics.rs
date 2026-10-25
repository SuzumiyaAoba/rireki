//! 統計と API 換算料金の端末表示。集計は worker の Cache で行い、描画は結果だけを読む。

mod compute;
mod render;
pub(super) use compute::{Cache, Computed, Params};
pub(super) use render::draw;

use std::collections::HashSet;

use chrono::{Duration, Local};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

use rireki::cost_timeline;
use rireki::detail::Selection;
use rireki::statistics::{Metric, Period, Summary};
use rireki::time::midnight;

const BINS: usize = 48;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Tab {
    #[default]
    Overview,
    Trend,
    Hours,
    Sources,
    Models,
    Projects,
    Sessions,
    Tools,
    Costs,
}

impl Tab {
    const ALL: [Self; 9] = [
        Self::Overview,
        Self::Trend,
        Self::Hours,
        Self::Sources,
        Self::Models,
        Self::Projects,
        Self::Sessions,
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
            Self::Sessions => "セッション",
            Self::Tools => "ツール",
            Self::Costs => "API 料金",
        }
    }
}

pub(super) enum Action {
    None,
    Recompute,
    Timeline { start: f64, end: f64 },
    Project(String),
    Event(Selection),
    /// セッションを開く。`failures` なら失敗・拒否・中断したイベントへ移る
    Session { index: usize, failures: bool },
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
                let n = Metric::ALL.len();
                self.metric =
                    Metric::ALL[(current + if key.code == KeyCode::Char('M') { n - 1 } else { 1 }) % n];
                self.selected = 0;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let current = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                let n = Tab::ALL.len();
                self.set_tab(
                    Tab::ALL[(current + if key.code == KeyCode::BackTab { n - 1 } else { 1 }) % n],
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
            Tab::Sessions => self
                .session_indices()
                .get(self.selected)
                .map_or(Action::None, |&i| Action::Session {
                    index: summary.sessions[i].index,
                    failures: self.metric == Metric::Failures,
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
            Tab::Overview => 4 + Metric::ALL.len() * 2 + 7,
            Tab::Trend => s.activity.len(),
            Tab::Hours => 168,
            Tab::Sources => 3,
            Tab::Models => s.models.len(),
            Tab::Projects => s.projects.len(),
            Tab::Sessions => self.session_indices().len(),
            Tab::Tools => self.tool_indices().len(),
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

    /// 指標の多い順のセッション。値のないセッションは並べず、同じ値なら開始の新しい順にする。
    /// セッション数の指標はどれも 1 件なので、開始の新しい順になる
    fn session_indices(&self) -> Vec<usize> {
        let Some(summary) = &self.summary else {
            return Vec::new();
        };
        let rows = &summary.sessions;
        let value = |i: usize| self.metric.total(&rows[i].total);
        let mut indices: Vec<usize> = (0..rows.len()).filter(|&i| value(i) > 0.0).collect();
        indices.sort_by(|&a, &b| {
            value(b).total_cmp(&value(a)).then(rows[b].start.total_cmp(&rows[a].start)).then(a.cmp(&b))
        });
        indices
    }

    /// ツールは呼び出しの多い順。失敗の指標では失敗・拒否・中断の多い順にする
    fn tool_indices(&self) -> Vec<usize> {
        let Some(summary) = &self.summary else {
            return Vec::new();
        };
        let mut indices: Vec<usize> = (0..summary.tools.len()).collect();
        if self.metric == Metric::Failures {
            indices.sort_by_key(|&i| std::cmp::Reverse(summary.tools[i].total().failures));
        }
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

/// 統計の順位から開くセッションの位置。`failures` なら失敗・拒否・中断の最も多いレーンの最初の失敗、
/// そうでなければ main レーン全体
pub(super) fn session_selection(snapshot: &rireki::model::Snapshot, index: usize, failures: bool) -> Option<Selection> {
    let session = snapshot.sessions.get(index)?;
    if failures {
        let failed = |l: &rireki::model::Lane| l.events.iter().filter(|e| e.status.failed()).count();
        let (lane, l) = session
            .lanes
            .iter()
            .enumerate()
            .max_by(|a, b| failed(a.1).cmp(&failed(b.1)).then(b.0.cmp(&a.0)))?;
        if let Some(event) = l.events.iter().position(|e| e.status.failed()) {
            return Some(Selection { session: index, lane, first: event, last: event });
        }
    }
    let n = session.lanes.first()?.events.len();
    (n > 0).then(|| Selection { session: index, lane: 0, first: 0, last: n - 1 })
}

#[derive(Clone, Copy)]
enum CostRow {
    Directory(usize),
    Session(usize, usize),
}

#[cfg(test)]
mod tests;
