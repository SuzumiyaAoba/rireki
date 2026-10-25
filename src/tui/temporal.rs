//! GUI と共通の履歴・集約規則を使う、端末の時間軸とカレンダー。
//! 料金集計と詳細読み込みは呼び出し元のワーカーに渡す。

mod input;
mod render;
mod rows;
pub use input::HELP;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use rireki::cost_timeline;
use rireki::detail::Selection;
use rireki::histogram::count_bins;
use rireki::layout::{self, Granularity};
use rireki::model::{Kind, Snapshot, UsageSum};
use rireki::time::{local, midnight};

const IDLE: f64 = 15.0 * 60.0;
const COST_BINS: usize = 64;
const PRESETS: [f64; 6] = [3600.0, 21600.0, 86400.0, 604800.0, 2592000.0, 7776000.0];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Timeline,
    Day,
    Week,
    Month,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::Timeline => "タイムライン",
            Self::Day => "日",
            Self::Week => "週",
            Self::Month => "月",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    pub granularity: Granularity,
    pub directory: bool,
    pub in_view_only: bool,
    pub cost: bool,
    pub collapsed: HashSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Timeline,
            granularity: Granularity::Auto,
            directory: false,
            in_view_only: false,
            cost: false,
            collapsed: HashSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    None,
    Changed,
    Open(Selection),
}

#[derive(Clone)]
struct Item {
    selection: Selection,
    start: f64,
    end: f64,
    label: String,
    kind: Kind,
    usage: Option<UsageSum>,
}

struct Row {
    cwd: String,
    label: String,
    header: bool,
    lane: Option<(usize, usize)>,
    range: (f64, f64),
    items: Vec<Item>,
}

struct Day {
    date: NaiveDate,
    active: f64,
    sessions: usize,
    intervals: Vec<(f64, f64)>,
}

pub struct Model {
    settings: Settings,
    day: NaiveDate,
    range: (f64, f64),
    bounds: Option<(f64, f64)>,
    selected: usize,
    item: usize,
    offset: usize,
    page: usize,
    plot_width: u16,
    hour_start: f64,
    hour_span: f64,
    date_focus: bool,
    initialized: bool,
    versions: Vec<(usize, usize)>,
    cache: layout::Cache,
    parents: HashMap<usize, (usize, usize, f64)>,
    rows: Vec<Row>,
    days: Vec<Day>,
    /// GUI と同じく統合済みセッション区間を加算。同時実行セッションの重なりも含む。
    projects: Vec<(String, f64)>,
    show_projects: bool,
    project_selected: usize,
    project_offset: usize,
    dirty: bool,
    cost: Option<Arc<cost_timeline::Summary>>,
    minimap: Vec<u32>,
    input: Option<String>,
    error: String,
}

impl Default for Model {
    fn default() -> Self {
        let now = Local::now().timestamp() as f64;
        Self {
            settings: Settings::default(),
            day: Local::now().date_naive(),
            range: (now - 86400.0, now),
            bounds: None,
            selected: 0,
            item: 0,
            offset: 0,
            page: 10,
            plot_width: 60,
            hour_start: 0.0,
            hour_span: 24.0,
            date_focus: false,
            initialized: false,
            versions: Vec::new(),
            cache: layout::Cache::default(),
            parents: HashMap::new(),
            rows: Vec::new(),
            days: Vec::new(),
            projects: Vec::new(),
            show_projects: false,
            project_selected: 0,
            project_offset: 0,
            dirty: true,
            cost: None,
            minimap: Vec::new(),
            input: None,
            error: String::new(),
        }
    }
}

impl Model {
    pub fn settings(&self) -> Settings {
        self.settings.clone()
    }

    pub fn apply_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.changed();
    }

    pub fn mode(&self) -> Mode {
        self.settings.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.settings.mode == mode {
            return;
        }
        if self.settings.mode == Mode::Timeline {
            self.day = local((self.range.0 + self.range.1) / 2.0).date_naive();
        } else if mode == Mode::Timeline {
            self.range = (midnight(self.day), midnight(self.day + Duration::days(1)));
        }
        self.settings.mode = mode;
        self.selected = 0;
        self.item = 0;
        self.offset = 0;
        self.changed();
    }

    pub fn in_view_only(&self) -> bool {
        self.settings.in_view_only
    }

    pub fn range(&self) -> (f64, f64) {
        if self.settings.mode == Mode::Timeline {
            self.range
        } else {
            let (first, count) = self.calendar_span();
            (
                midnight(first),
                midnight(first + Duration::days(count as i64)),
            )
        }
    }

    pub fn set_range(&mut self, start: f64, end: f64) {
        if !start.is_finite() || !end.is_finite() || start >= end {
            return;
        }
        self.settings.mode = Mode::Timeline;
        self.range = (start, end);
        self.date_focus = false;
        self.initialized = true;
        self.selected = 0;
        self.item = 0;
        self.offset = 0;
        self.changed();
    }

    pub fn is_editing(&self) -> bool {
        self.input.is_some()
    }

    /// 同じスナップショットと絞り込みには無処理。
    pub fn sync(&mut self, snapshot: &Snapshot, order: &[usize]) {
        let versions: Vec<_> = order
            .iter()
            .filter_map(|&i| {
                snapshot
                    .sessions
                    .get(i)
                    .map(|s| (i, Arc::as_ptr(s) as usize))
            })
            .collect();
        if versions == self.versions {
            return;
        }
        self.versions = versions;
        self.bounds = order_bounds(snapshot, order);
        self.parents = rireki::view_index::exec_parents(&snapshot.sessions, order);
        self.cache = layout::Cache::default();
        if !self.initialized
            && let Some((_, end)) = self.bounds
        {
            self.range = (end - 86400.0 * 0.85, end + 86400.0 * 0.15);
            self.day = local(end).date_naive();
            self.initialized = true;
        }
        self.minimap = self
            .bounds
            .map(|(a, b)| {
                let edges: Vec<_> = (0..=64).map(|i| a + (b - a) * i as f64 / 64.0).collect();
                count_bins(snapshot, order, &edges)
            })
            .unwrap_or_default();
        self.changed();
    }

    /// 同じ要求が処理中なら呼び出し元でまとめ、結果の世代もそこで確認する。
    pub fn cost_request(&self) -> Option<cost_timeline::View> {
        if self.settings.mode != Mode::Timeline || !self.settings.cost {
            return None;
        }
        let view = cost_timeline::View::new(self.range.0, self.range.1, COST_BINS)?;
        self.cost
            .as_ref()
            .is_none_or(|summary| summary.view != view)
            .then_some(view)
    }

    pub fn apply_cost(&mut self, summary: Arc<cost_timeline::Summary>) {
        if self.settings.mode == Mode::Timeline
            && self.settings.cost
            && summary.view.start == self.range.0
            && summary.view.end == self.range.1
        {
            self.cost = Some(summary);
            self.dirty = true;
        }
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.cost = None;
    }

    fn calendar_span(&self) -> (NaiveDate, usize) {
        match self.settings.mode {
            Mode::Week => (
                self.day - Duration::days(self.day.weekday().num_days_from_monday() as i64),
                7,
            ),
            Mode::Month => {
                let first = self.day.with_day(1).unwrap_or(self.day);
                (
                    first - Duration::days(first.weekday().num_days_from_monday() as i64),
                    42,
                )
            }
            _ => (self.day, 1),
        }
    }

    fn current_item(&self) -> Option<&Item> {
        self.rows.get(self.selected)?.items.get(self.item)
    }
}

fn order_bounds(snapshot: &Snapshot, order: &[usize]) -> Option<(f64, f64)> {
    let mut start = f64::INFINITY;
    let mut end = f64::NEG_INFINITY;
    for &i in order {
        if let Some(s) = snapshot.sessions.get(i)
            && s.start.is_finite()
            && s.end.is_finite()
        {
            start = start.min(s.start);
            end = end.max(s.end);
        }
    }
    // 集計は半開区間なので、末尾の瞬間イベントも全期間・ミニマップに含める。
    (start.is_finite() && end.is_finite()).then_some((start, end.max(start) + 1.0))
}

fn tokens(usage: &UsageSum) -> u64 {
    usage.input + usage.cache_read + usage.cache_write + usage.output
}

fn title(session: &rireki::model::Session) -> String {
    if session.title.is_empty() {
        "(無題)".into()
    } else {
        clean(&session.title)
    }
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn duration(seconds: f64) -> String {
    let minutes = (seconds.max(0.0) / 60.0).round() as u64;
    if minutes >= 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn weekday(date: NaiveDate) -> &'static str {
    ["月", "火", "水", "木", "金", "土", "日"][date.weekday().num_days_from_monday() as usize]
}

#[cfg(test)]
mod tests;
