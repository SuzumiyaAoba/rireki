//! GUI と共通の履歴・集約規則を使う、端末の時間軸とカレンダー。
//! 料金集計と詳細読み込みは呼び出し元のワーカーに渡す。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{Datelike, Duration, Local, Months, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use serde::{Deserialize, Serialize};

use rireki::cost_timeline;
use rireki::detail::Selection;
use rireki::histogram::count_bins;
use rireki::layout::{self, Granularity};
use rireki::model::{Kind, Snapshot, UsageSum};
use rireki::time::{local, midnight};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
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

    pub fn paste(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.extend(
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(64usize.saturating_sub(input.chars().count())),
            );
        }
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

    pub fn handle_key(&mut self, key: KeyEvent, snapshot: &Snapshot, order: &[usize]) -> Action {
        if self.input.is_some() {
            return self.edit(key);
        }
        self.ensure_rows(snapshot, order);
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Char(c @ '1'..='6') => {
                    self.preset(PRESETS[c as usize - '1' as usize]);
                    return Action::Changed;
                }
                KeyCode::Left | KeyCode::Right => {
                    self.day += Duration::days(if key.code == KeyCode::Left { -1 } else { 1 });
                    self.date_focus = true;
                    self.changed();
                    return Action::Changed;
                }
                _ => return Action::None,
            }
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Home | KeyCode::End => {
                    if let Some((a, b)) = self.bounds {
                        self.seek(if key.code == KeyCode::Home { a } else { b });
                    }
                    return Action::Changed;
                }
                KeyCode::Up | KeyCode::Down => {
                    let dir = if key.code == KeyCode::Up { -1.0 } else { 1.0 };
                    self.hour_start = (self.hour_start + dir * self.hour_span / 4.0)
                        .clamp(0.0, 24.0 - self.hour_span);
                    return Action::Changed;
                }
                _ => return Action::None,
            }
        }
        if self.settings.mode != Mode::Timeline {
            if key.code == KeyCode::Char('u') {
                self.show_projects = !self.show_projects;
                self.date_focus = false;
                return Action::Changed;
            }
            if self.show_projects {
                let next = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        Some(self.project_selected.saturating_sub(1))
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        Some(self.project_selected.saturating_add(1))
                    }
                    KeyCode::PageUp => Some(self.project_selected.saturating_sub(self.page.max(1))),
                    KeyCode::PageDown => {
                        Some(self.project_selected.saturating_add(self.page.max(1)))
                    }
                    KeyCode::Home => Some(0),
                    KeyCode::End => Some(self.projects.len().saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some((cwd, _)) = self.projects.get(self.project_selected)
                            && let Some(row) = self
                                .rows
                                .iter()
                                .position(|row| row.cwd == *cwd && !row.items.is_empty())
                        {
                            self.selected = row;
                            self.item = 0;
                            self.show_projects = false;
                            self.date_focus = false;
                        }
                        return Action::Changed;
                    }
                    _ => None,
                };
                if let Some(next) = next {
                    self.project_selected = next.min(self.projects.len().saturating_sub(1));
                    return Action::Changed;
                }
            }
        }
        match key.code {
            KeyCode::Char('d') => self.set_mode(Mode::Day),
            KeyCode::Char('w') => self.set_mode(Mode::Week),
            KeyCode::Char('m') => self.set_mode(Mode::Month),
            KeyCode::Char('c') => {
                self.settings.cost = !self.settings.cost;
                self.changed();
            }
            KeyCode::Char('o') => {
                self.settings.directory = !self.settings.directory;
                self.changed();
            }
            KeyCode::Char('v') => {
                self.settings.in_view_only = !self.settings.in_view_only;
                self.changed();
            }
            KeyCode::Char('a') => {
                let i = Granularity::ALL
                    .iter()
                    .position(|g| *g == self.settings.granularity)
                    .unwrap_or(0);
                self.settings.granularity = Granularity::ALL[(i + 1) % Granularity::ALL.len()];
                self.changed();
            }
            KeyCode::Char(' ') => self.toggle_group(),
            KeyCode::Left | KeyCode::Char('h') => self.step(-1),
            KeyCode::Right | KeyCode::Char('l') => self.step(1),
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom(0.5),
            KeyCode::Char('-') => self.zoom(2.0),
            KeyCode::Char('t') => self.seek(Local::now().timestamp() as f64),
            KeyCode::Char('L') => {
                if let Some((_, end)) = self.bounds {
                    self.seek(end);
                }
            }
            KeyCode::Char('g') => {
                self.input = Some(String::new());
                self.error.clear();
            }
            KeyCode::Char('0') => {
                if let Some((a, b)) = self.bounds {
                    self.set_range(a, b);
                }
            }
            KeyCode::Char('{') => self.minimap_step(-1),
            KeyCode::Char('}') => self.minimap_step(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
            KeyCode::PageUp => self.move_row(-(self.page.max(1) as isize)),
            KeyCode::PageDown => self.move_row(self.page.max(1) as isize),
            KeyCode::Home => {
                self.selected = 0;
                self.item = 0;
            }
            KeyCode::End => {
                self.selected = self.rows.len().saturating_sub(1);
                self.item = 0;
            }
            KeyCode::Char('<') | KeyCode::Char(',') => self.move_item(-1),
            KeyCode::Char('>') | KeyCode::Char('.') => self.move_item(1),
            KeyCode::Char('[') => self.related_lane(snapshot, false),
            KeyCode::Char(']') => self.related_lane(snapshot, true),
            KeyCode::Char('z') => {
                if self.date_focus && self.settings.mode != Mode::Timeline {
                    self.set_range(midnight(self.day), midnight(self.day + Duration::days(1)));
                    self.date_focus = false;
                } else if let Some(item) = self.current_item() {
                    let span = (item.end - item.start).max(30.0);
                    self.set_range(
                        item.start - span * 0.05,
                        item.end.max(item.start + 30.0) + span * 0.05,
                    );
                } else if self.settings.mode != Mode::Timeline {
                    self.set_range(midnight(self.day), midnight(self.day + Duration::days(1)));
                }
            }
            KeyCode::Enter => {
                if self.date_focus && self.settings.mode != Mode::Timeline {
                    self.set_mode(Mode::Day);
                    self.date_focus = false;
                    return Action::Changed;
                }
                if let Some(item) = self.current_item() {
                    return Action::Open(item.selection);
                }
                if self.rows.get(self.selected).is_some_and(|row| row.header) {
                    self.toggle_group();
                }
            }
            _ => return Action::None,
        }
        Action::Changed
    }

    fn toggle_group(&mut self) {
        if let Some(row) = self.rows.get(self.selected)
            && !row.cwd.is_empty()
        {
            let cwd = row.cwd.clone();
            if !self.settings.collapsed.remove(&cwd) {
                self.settings.collapsed.insert(cwd);
            }
            self.changed();
        }
    }

    fn edit(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.input = None;
                self.error.clear();
            }
            KeyCode::Enter => match parse_range(self.input.as_deref().unwrap_or_default().trim()) {
                Ok(InputRange::Days(a, b)) => {
                    if self.settings.mode == Mode::Timeline || a != b {
                        self.set_range(midnight(a), midnight(b + Duration::days(1)));
                    } else {
                        self.day = a;
                        self.date_focus = true;
                        self.changed();
                    }
                    self.input = None;
                    self.error.clear();
                }
                Ok(InputRange::Preset(seconds)) => {
                    if seconds == 0.0 {
                        if let Some((a, b)) = self.bounds {
                            self.set_range(a, b);
                        }
                    } else {
                        self.preset(seconds);
                    }
                    self.input = None;
                    self.error.clear();
                }
                Err(message) => self.error = message.into(),
            },
            KeyCode::Backspace => {
                self.input.as_mut().unwrap().pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.as_mut().unwrap().clear()
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.input.as_ref().unwrap().len() < 64
                    && !c.is_control() =>
            {
                self.input.as_mut().unwrap().push(c);
            }
            _ => {}
        }
        Action::Changed
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

    fn step(&mut self, direction: i64) {
        match self.settings.mode {
            Mode::Timeline => {
                let shift = (self.range.1 - self.range.0) * direction as f64;
                self.range = (self.range.0 + shift, self.range.1 + shift);
            }
            Mode::Day => self.day += Duration::days(direction),
            Mode::Week => self.day += Duration::days(direction * 7),
            Mode::Month => {
                let first = self.day.with_day(1).unwrap_or(self.day);
                self.day = if direction < 0 {
                    first - Months::new(1)
                } else {
                    first + Months::new(1)
                };
            }
        }
        self.selected = 0;
        self.offset = 0;
        self.changed();
    }

    fn zoom(&mut self, factor: f64) {
        if self.settings.mode == Mode::Timeline {
            let center = (self.range.0 + self.range.1) / 2.0;
            let span = ((self.range.1 - self.range.0) * factor).clamp(1.0, 86400.0 * 3650.0);
            self.range = (center - span / 2.0, center + span / 2.0);
            self.changed();
        } else if self.settings.mode != Mode::Month {
            let center = self.hour_start + self.hour_span / 2.0;
            self.hour_span = (self.hour_span * factor).clamp(0.25, 24.0);
            self.hour_start = (center - self.hour_span / 2.0).clamp(0.0, 24.0 - self.hour_span);
        }
    }

    fn seek(&mut self, time: f64) {
        self.day = local(time).date_naive();
        if self.settings.mode == Mode::Timeline {
            let span = self.range.1 - self.range.0;
            self.range = (time - span * 0.85, time + span * 0.15);
        }
        self.selected = 0;
        self.offset = 0;
        self.changed();
    }

    fn preset(&mut self, seconds: f64) {
        let now = Local::now().timestamp() as f64;
        self.set_range(now - seconds, now);
    }

    fn minimap_step(&mut self, direction: isize) {
        let Some((a, b)) = self.bounds else {
            return;
        };
        let middle = (self.range().0 + self.range().1) / 2.0;
        let current = (((middle - a) / (b - a) * 64.0) as isize).clamp(0, 63);
        let target = if direction < 0 {
            (0..current as usize)
                .rev()
                .find(|&i| self.minimap.get(i).is_some_and(|&v| v > 0))
        } else {
            ((current + 1) as usize..64).find(|&i| self.minimap.get(i).is_some_and(|&v| v > 0))
        };
        if let Some(i) = target {
            let time = a + (b - a) * (i as f64 + 0.5) / 64.0;
            if self.settings.mode == Mode::Timeline {
                let span = self.range.1 - self.range.0;
                self.range = (time - span / 2.0, time + span / 2.0);
                self.changed();
            } else {
                self.seek(time);
            }
        }
    }

    fn move_row(&mut self, delta: isize) {
        self.date_focus = false;
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.rows.len().saturating_sub(1));
        self.item = 0;
    }

    fn move_item(&mut self, delta: isize) {
        self.date_focus = false;
        if let Some(row) = self.rows.get(self.selected) {
            self.item = self
                .item
                .saturating_add_signed(delta)
                .min(row.items.len().saturating_sub(1));
        }
    }

    fn related_lane(&mut self, snapshot: &Snapshot, child: bool) {
        let Some((si, li)) = self.rows.get(self.selected).and_then(|row| row.lane) else {
            return;
        };
        let Some(session) = snapshot.sessions.get(si) else {
            return;
        };
        let target = if child {
            session
                .parents
                .iter()
                .position(|&parent| parent == Some(li))
                .map(|lane| (si, lane))
                .or_else(|| {
                    self.parents
                        .iter()
                        .filter(|(_, (ps, pl, _))| *ps == si && *pl == li)
                        .map(|(&si, _)| (si, 0))
                        .min()
                })
        } else {
            session
                .parents
                .get(li)
                .copied()
                .flatten()
                .map(|lane| (si, lane))
                .or_else(|| self.parents.get(&si).map(|&(ps, pl, _)| (ps, pl)))
        };
        if let Some(target) = target
            && let Some(i) = self.rows.iter().position(|row| row.lane == Some(target))
        {
            self.selected = i;
            self.item = 0;
        }
    }

    fn current_item(&self) -> Option<&Item> {
        self.rows.get(self.selected)?.items.get(self.item)
    }

    fn ensure_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        if !self.dirty {
            return;
        }
        let selected = self.current_item().map(|item| item.selection);
        self.rows.clear();
        self.days.clear();
        self.projects.clear();
        if self.settings.mode != Mode::Timeline {
            self.calendar_rows(snapshot, order);
        } else if self.settings.cost {
            self.cost_rows(snapshot);
        } else if self.settings.directory {
            self.directory_rows(snapshot, order);
        } else {
            self.lane_rows(snapshot, order);
        }
        if let Some(selection) = selected
            && let Some((row, item)) = self.rows.iter().enumerate().find_map(|(row, r)| {
                r.items
                    .iter()
                    .position(|item| item.selection == selection)
                    .map(|item| (row, item))
            })
        {
            self.selected = row;
            self.item = item;
        }
        self.project_selected = self
            .project_selected
            .min(self.projects.len().saturating_sub(1));
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.item = self
            .rows
            .get(self.selected)
            .map_or(0, |row| self.item.min(row.items.len().saturating_sub(1)));
        self.dirty = false;
    }

    fn header_row(&mut self, cwd: &str, label: String) {
        self.rows.push(Row {
            cwd: cwd.into(),
            label,
            header: true,
            lane: None,
            range: self.range,
            items: Vec::new(),
        });
    }

    fn lane_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
        let mut by_cwd = HashMap::new();
        for &si in order {
            let Some(s) = snapshot.sessions.get(si) else {
                continue;
            };
            let i = *by_cwd.entry(s.cwd.as_str()).or_insert_with(|| {
                groups.push((&s.cwd, Vec::new()));
                groups.len() - 1
            });
            groups[i].1.push(si);
        }
        let spp = (self.range.1 - self.range.0) / self.plot_width.max(1) as f64;
        for (cwd, sessions) in groups {
            let header = self.rows.len();
            self.header_row(
                cwd,
                format!("{} ({} セッション)", clean(cwd), sessions.len()),
            );
            if self.settings.collapsed.contains(cwd) {
                continue;
            }
            for si in sessions {
                let session = &snapshot.sessions[si];
                for &(li, depth) in &session.tree {
                    let lane = &session.lanes[li];
                    let blocks = self.cache.aggregate(
                        (si, li),
                        lane,
                        self.range,
                        spp,
                        self.settings.granularity,
                    );
                    if blocks.is_empty() && self.settings.in_view_only {
                        continue;
                    }
                    let label = if li == 0 {
                        format!(
                            "{}{} {}",
                            if self.parents.contains_key(&si) {
                                "↳ "
                            } else {
                                ""
                            },
                            session.source.label(),
                            title(session)
                        )
                    } else {
                        format!("{}↳ {}", " ".repeat(depth.min(8)), clean(&lane.name))
                    };
                    let items = blocks
                        .into_iter()
                        .map(|block| {
                            let (dominant, label) =
                                self.cache.describe((si, li), lane, &block, true);
                            Item {
                                selection: Selection {
                                    session: si,
                                    lane: li,
                                    first: block.first,
                                    last: block.last,
                                },
                                start: block.start,
                                end: block.end,
                                label: clean(&label.unwrap_or_default()),
                                kind: lane.events[dominant].kind,
                                usage: None,
                            }
                        })
                        .collect();
                    self.rows.push(Row {
                        cwd: cwd.into(),
                        label,
                        header: false,
                        lane: Some((si, li)),
                        range: self.range,
                        items,
                    });
                }
            }
            if self.settings.in_view_only && self.rows.len() == header + 1 {
                self.rows.pop();
            }
        }
    }

    fn directory_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        let spp = (self.range.1 - self.range.0) / self.plot_width.max(1) as f64;
        let groups = layout::directory::groups(
            &mut self.cache,
            &snapshot.sessions,
            order,
            &self.parents,
            layout::directory::View {
                range: self.range,
                spp,
                gran: self.settings.granularity,
                min_width: 1.0,
                in_view_only: self.settings.in_view_only,
            },
        );
        for group in groups {
            self.header_row(
                &group.cwd,
                format!(
                    "{} ({} セッション)",
                    clean(&group.cwd),
                    group.sessions.len()
                ),
            );
            if self.settings.collapsed.contains(&group.cwd) {
                continue;
            }
            for (i, blocks) in group.rows.into_iter().enumerate() {
                let items = blocks
                    .into_iter()
                    .map(|b| {
                        let lane = &snapshot.sessions[b.session].lanes[0];
                        let (dominant, label) =
                            self.cache.describe((b.session, 0), lane, &b.block, true);
                        Item {
                            selection: Selection {
                                session: b.session,
                                lane: 0,
                                first: b.block.first,
                                last: b.block.last,
                            },
                            start: b.block.start,
                            end: b.block.end,
                            label: format!(
                                "{} · {}",
                                title(&snapshot.sessions[b.session]),
                                clean(&label.unwrap_or_default())
                            ),
                            kind: lane.events[dominant].kind,
                            usage: None,
                        }
                    })
                    .collect();
                self.rows.push(Row {
                    cwd: group.cwd.clone(),
                    label: format!("並列行 {}", i + 1),
                    header: false,
                    lane: None,
                    range: self.range,
                    items,
                });
            }
        }
    }

    fn cost_rows(&mut self, snapshot: &Snapshot) {
        let Some(cost) = self.cost.clone() else {
            return;
        };
        for directory in &cost.directories {
            self.header_row(
                &directory.cwd,
                format!(
                    "{} · USD {:.4} · {} tokens",
                    clean(&directory.cwd),
                    directory.usage.cost,
                    tokens(&directory.usage)
                ),
            );
            if self.settings.collapsed.contains(&directory.cwd) {
                continue;
            }
            if self.settings.directory {
                self.rows.push(Row {
                    cwd: directory.cwd.clone(),
                    label: "API 換算料金".into(),
                    header: false,
                    lane: None,
                    range: self.range,
                    items: cost_items(&directory.buckets, &cost.edges),
                });
            } else {
                for session in &directory.sessions {
                    let Some(s) = snapshot.sessions.get(session.session) else {
                        continue;
                    };
                    self.rows.push(Row {
                        cwd: directory.cwd.clone(),
                        label: title(s),
                        header: false,
                        lane: Some((session.session, 0)),
                        range: self.range,
                        items: cost_items(&session.buckets, &cost.edges),
                    });
                }
            }
        }
    }

    fn calendar_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        let (first, count) = self.calendar_span();
        let mut projects: HashMap<String, f64> = HashMap::new();
        for day in 0..count {
            let date = first + Duration::days(day as i64);
            let range = (midnight(date), midnight(date + Duration::days(1)));
            let mut by_root: HashMap<usize, Vec<Item>> = HashMap::new();
            for &si in order {
                let Some(session) = snapshot.sessions.get(si) else {
                    continue;
                };
                if session.start >= range.1 || session.end < range.0 {
                    continue;
                }
                let mut root = si;
                while let Some(&(parent, _, _)) = self.parents.get(&root) {
                    root = parent;
                }
                for (li, lane) in session.lanes.iter().enumerate() {
                    let hi = lane.events.partition_point(|e| e.start < range.1);
                    let lo = lane.cum_end.partition_point(|&end| end < range.0).min(hi);
                    let mut current: Option<Item> = None;
                    for (ei, event) in lane.events.iter().enumerate().take(hi).skip(lo) {
                        if event.end < range.0 {
                            continue;
                        }
                        let (start, end) = (event.start.max(range.0), event.end.min(range.1));
                        if let Some(item) = &mut current
                            && start <= item.end + IDLE
                        {
                            item.end = item.end.max(end);
                            item.selection.last = ei;
                        } else {
                            if let Some(item) = current.take() {
                                by_root.entry(root).or_default().push(item);
                            }
                            current = Some(Item {
                                selection: Selection {
                                    session: si,
                                    lane: li,
                                    first: ei,
                                    last: ei,
                                },
                                start,
                                end,
                                label: title(&snapshot.sessions[root]),
                                kind: event.kind,
                                usage: None,
                            });
                        }
                    }
                    if let Some(item) = current {
                        by_root.entry(root).or_default().push(item);
                    }
                }
            }
            let mut groups = Vec::new();
            let sessions = by_root.len();
            for (root, mut items) in by_root {
                items.sort_by(|a, b| {
                    a.start
                        .total_cmp(&b.start)
                        .then(a.selection.lane.cmp(&b.selection.lane))
                });
                let mut merged: Option<Item> = None;
                for item in items {
                    if let Some(previous) = &mut merged
                        && item.start <= previous.end + IDLE
                    {
                        previous.end = previous.end.max(item.end);
                        // 詳細は一つのレーン内の選択。重なるメインレーンを代表にする。
                        if item.selection.session == root && item.selection.lane == 0 {
                            if previous.selection.session == root && previous.selection.lane == 0 {
                                previous.selection.first =
                                    previous.selection.first.min(item.selection.first);
                                previous.selection.last =
                                    previous.selection.last.max(item.selection.last);
                            } else {
                                previous.selection = item.selection;
                            }
                        }
                    } else {
                        if let Some(item) = merged.take() {
                            groups.push((root, item));
                        }
                        merged = Some(item);
                    }
                }
                if let Some(item) = merged {
                    groups.push((root, item));
                }
            }
            groups.sort_by(|a, b| a.1.start.total_cmp(&b.1.start).then(a.0.cmp(&b.0)));
            let intervals: Vec<_> = groups
                .iter()
                .map(|(_, item)| (item.start, item.end))
                .collect();
            let active = union_duration(&intervals);
            self.days.push(Day {
                date,
                active,
                sessions,
                intervals,
            });
            if groups.is_empty() {
                continue;
            }
            self.rows.push(Row {
                cwd: String::new(),
                label: format!(
                    "{} ({}) · {} セッション · 稼働 {}",
                    date,
                    weekday(date),
                    sessions,
                    duration(active)
                ),
                header: true,
                lane: None,
                range,
                items: Vec::new(),
            });
            for (root, item) in groups {
                let session = &snapshot.sessions[root];
                *projects.entry(session.cwd.clone()).or_default() += item.end - item.start;
                let label = format!(
                    "{}–{} {} {}",
                    local(item.start).format("%H:%M"),
                    local(item.end).format("%H:%M"),
                    session.source.label(),
                    title(session)
                );
                self.rows.push(Row {
                    cwd: session.cwd.clone(),
                    label,
                    header: false,
                    lane: Some((item.selection.session, item.selection.lane)),
                    range,
                    items: vec![item],
                });
            }
        }
        self.projects = projects.into_iter().collect();
        self.projects
            .sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    }

    pub fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        snapshot: &Snapshot,
        order: &[usize],
    ) {
        if area.is_empty() {
            return;
        }
        let label_width = (area.width / 3)
            .clamp(12, 42)
            .min(area.width.saturating_sub(1));
        let plot_width = area.width.saturating_sub(label_width + 3).max(1);
        if self.plot_width != plot_width {
            self.plot_width = plot_width;
            self.dirty = true;
        }
        self.ensure_rows(snapshot, order);
        let [heading, overview, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(if area.height >= 12 {
                if self.settings.mode == Mode::Timeline {
                    2
                } else if self.settings.mode == Mode::Month {
                    8
                } else {
                    10
                }
            } else {
                0
            }),
            Constraint::Min(1),
            Constraint::Length(if area.height >= 7 { 3 } else { 1 }),
        ])
        .areas(area);
        self.draw_heading(frame, heading);
        if self.settings.mode == Mode::Timeline {
            self.draw_minimap(frame, overview);
        } else if self.settings.mode == Mode::Month {
            self.draw_month(frame, overview);
        } else {
            self.draw_calendar(frame, overview);
        }
        if self.settings.mode != Mode::Timeline && self.show_projects {
            self.draw_projects(frame, body);
        } else {
            self.draw_rows(frame, body, label_width);
        }
        self.draw_footer(frame, footer, snapshot);
        if self.input.is_some() {
            self.draw_input(frame, area);
        }
    }

    fn draw_heading(&self, frame: &mut Frame<'_>, area: Rect) {
        let (a, b) = self.range();
        let period = if self.settings.mode == Mode::Month {
            self.day.format("%Y年%m月").to_string()
        } else {
            format!(
                "{} — {}",
                local(a).format("%Y-%m-%d %H:%M"),
                local(b).format("%Y-%m-%d %H:%M")
            )
        };
        let options = format!(
            "{} · {} · {} · {}",
            if self.settings.cost {
                "API換算料金"
            } else {
                "イベント"
            },
            if self.settings.directory {
                "ディレクトリ"
            } else {
                "レーン"
            },
            self.settings.granularity.label(),
            if self.settings.in_view_only {
                "範囲内のみ"
            } else {
                "すべての行"
            }
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{}  ", self.settings.mode.label()),
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(period),
                ]),
                Line::styled(options, Style::default().fg(MUTED)),
            ]),
            area,
        );
    }

    fn draw_minimap(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        let Some((a, b)) = self.bounds else {
            return;
        };
        let width = area.width.saturating_sub(2) as usize;
        if width == 0 {
            return;
        }
        let max = self.minimap.iter().copied().max().unwrap_or(1).max(1) as f64;
        let bars = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let mut spans = vec![Span::raw("│")];
        for i in 0..width {
            let bin = i * 64 / width;
            let value = self.minimap.get(bin).copied().unwrap_or(0);
            let time = a + (b - a) * i as f64 / width as f64;
            let active = time >= self.range.0 && time <= self.range.1;
            let style = Style::default().fg(if active { ACCENT } else { MUTED });
            spans.push(Span::styled(
                bars[if value == 0 {
                    0
                } else {
                    (value as f64 / max * 8.0).ceil() as usize
                }]
                .to_string(),
                if active {
                    style.bg(Color::DarkGray)
                } else {
                    style
                },
            ));
        }
        spans.push(Span::raw("│"));
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(spans),
                Line::styled(
                    format!(
                        "{}  時間分布  {{/}} 前/次の活動 · Ctrl+Home/End 最初/最新  {}",
                        local(a).format("%Y-%m-%d"),
                        local(b).format("%Y-%m-%d")
                    ),
                    Style::default().fg(MUTED),
                ),
            ]),
            area,
        );
    }

    fn draw_month(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        let width = (area.width / 7).max(1);
        let max_active = self
            .days
            .iter()
            .map(|day| day.active)
            .fold(0.0, f64::max)
            .max(1.0);
        for (i, name) in ["月", "火", "水", "木", "金", "土", "日"]
            .iter()
            .enumerate()
        {
            let x = area.x + i as u16 * width;
            if x >= area.right() {
                continue;
            }
            frame.render_widget(
                Paragraph::new(*name).style(Style::default().fg(ACCENT)),
                Rect::new(x, area.y, width.min(area.right() - x), 1),
            );
        }
        for (i, day) in self.days.iter().enumerate() {
            let x = area.x + i as u16 % 7 * width;
            let y = area.y + 1 + i as u16 / 7;
            if x >= area.right() || y >= area.bottom() {
                continue;
            }
            let text = if width >= 13 {
                format!(
                    "{:>2} {} {}",
                    day.date.day(),
                    day.sessions,
                    duration(day.active)
                )
            } else if width >= 6 {
                format!("{:>2} {:>2}", day.date.day(), day.sessions)
            } else {
                format!("{:>2}", day.date.day())
            };
            let color = if day.active > max_active * 0.66 {
                Color::Green
            } else if day.sessions > 0 {
                Color::Cyan
            } else {
                MUTED
            };
            let mut style = Style::default().fg(color);
            if day.date.month() != self.day.month() {
                style = style.add_modifier(Modifier::DIM);
            }
            if day.date == self.day {
                style = style.bg(Color::Blue);
            }
            frame.render_widget(
                Paragraph::new(text).style(style),
                Rect::new(x, y, width.min(area.right() - x), 1),
            );
        }
        if area.height >= 8 {
            frame.render_widget(
                Paragraph::new(
                    "日 / セッション数 / 稼働時間 · Alt+←/→ 日を選択 · g 日付指定 · z 時間軸で開く",
                )
                .style(Style::default().fg(MUTED)),
                Rect::new(area.x, area.bottom() - 1, area.width, 1),
            );
        }
    }

    fn draw_calendar(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() || self.days.is_empty() {
            return;
        }
        let hour_width = 6u16.min(area.width);
        let width = ((area.width - hour_width) / self.days.len() as u16).max(1);
        let rows = area.height.saturating_sub(2);
        if rows == 0 {
            return;
        }
        for (i, day) in self.days.iter().enumerate() {
            let x = area.x + hour_width + i as u16 * width;
            if x >= area.right() {
                break;
            }
            let title = if width > 17 {
                format!(
                    "{} {} · {}",
                    day.date.format("%m/%d"),
                    weekday(day.date),
                    duration(day.active)
                )
            } else {
                format!("{} {}", day.date.format("%m/%d"), weekday(day.date))
            };
            frame.render_widget(
                Paragraph::new(title).style(Style::default().fg(ACCENT)),
                Rect::new(x, area.y, width.min(area.right() - x), 1),
            );
            let start = midnight(day.date);
            for row in 0..rows {
                let lo =
                    start + (self.hour_start + self.hour_span * row as f64 / rows as f64) * 3600.0;
                let hi = start
                    + (self.hour_start + self.hour_span * (row + 1) as f64 / rows as f64) * 3600.0;
                let count = day
                    .intervals
                    .iter()
                    .filter(|&&(a, b)| a < hi && b >= lo)
                    .count();
                let cell = if count == 0 {
                    "·".into()
                } else {
                    format!("{} {}件", "█".repeat((width as usize / 3).max(1)), count)
                };
                frame.render_widget(
                    Paragraph::new(cell).style(Style::default().fg(if count > 0 {
                        Color::Green
                    } else {
                        MUTED
                    })),
                    Rect::new(x, area.y + row + 1, width.min(area.right() - x), 1),
                );
            }
        }
        for row in 0..rows {
            let hour = self.hour_start + self.hour_span * row as f64 / rows as f64;
            frame.render_widget(
                Paragraph::new(format!(
                    "{:02}:{:02}",
                    hour as u32,
                    (hour.fract() * 60.0) as u32
                ))
                .style(Style::default().fg(MUTED)),
                Rect::new(area.x, area.y + row + 1, hour_width, 1),
            );
        }
        frame.render_widget(
            Paragraph::new("稼働区間（15分以上の空白で分割） · +/− 時間目盛 · Ctrl+↑/↓ 時刻移動")
                .style(Style::default().fg(MUTED)),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }

    fn draw_projects(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        self.page = usize::from(area.height).saturating_sub(1).max(1);
        frame.render_widget(
            Paragraph::new("プロジェクト別の期間内稼働時間 · 長い順 · 同時実行セッションは加算")
                .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if body.is_empty() {
            return;
        }
        if self.projects.is_empty() {
            frame.render_widget(
                Paragraph::new("この期間に記録はありません").style(Style::default().fg(MUTED)),
                body,
            );
            return;
        }
        if self.project_selected < self.project_offset {
            self.project_offset = self.project_selected;
        }
        if self.project_selected >= self.project_offset + self.page {
            self.project_offset = self.project_selected + 1 - self.page;
        }
        self.project_offset = self
            .project_offset
            .min(self.projects.len().saturating_sub(self.page));
        let peak = self
            .projects
            .first()
            .map_or(1.0, |(_, seconds)| *seconds)
            .max(1.0);
        let bar_width = usize::from(area.width).saturating_sub(62).min(20);
        for (i, (cwd, seconds)) in self
            .projects
            .iter()
            .enumerate()
            .skip(self.project_offset)
            .take(self.page)
        {
            let selected = i == self.project_selected;
            let style = if selected {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };
            let bar = "█".repeat((seconds / peak * bar_width as f64).round() as usize);
            let text = format!(
                "{} {:>7}  {:<bar_width$} {}",
                if selected { "›" } else { " " },
                duration(*seconds),
                bar,
                clean(cwd)
            );
            frame.render_widget(
                Paragraph::new(text).style(style),
                Rect::new(
                    body.x,
                    body.y + (i - self.project_offset) as u16,
                    body.width,
                    1,
                ),
            );
        }
    }

    fn draw_rows(&mut self, frame: &mut Frame<'_>, area: Rect, label_width: u16) {
        if area.is_empty() {
            return;
        }
        self.page = area.height as usize;
        if self.selected < self.offset {
            self.offset = self.selected;
        }
        if self.selected >= self.offset + self.page {
            self.offset = self.selected + 1 - self.page;
        }
        self.offset = self.offset.min(self.rows.len().saturating_sub(self.page));
        if self.rows.is_empty() {
            let message = if self.settings.mode == Mode::Timeline
                && self.settings.cost
                && self.cost.is_none()
            {
                "料金を集計中…"
            } else {
                "この期間に表示する履歴はありません。L:最新の作業  0:全期間  g:期間指定"
            };
            frame.render_widget(
                Paragraph::new(message).style(Style::default().fg(MUTED)),
                area,
            );
            return;
        }
        for (row_index, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(self.page)
        {
            let selected = row_index == self.selected;
            let y = area.y + (row_index - self.offset) as u16;
            let rect = Rect::new(area.x, y, area.width, 1);
            let base = Style::default().bg(if selected { Color::Blue } else { Color::Reset });
            if row.header {
                let icon = if self.settings.collapsed.contains(&row.cwd) {
                    "▶"
                } else {
                    "▼"
                };
                frame.render_widget(
                    Paragraph::new(format!("{icon} {}", row.label))
                        .style(base.fg(ACCENT).add_modifier(Modifier::BOLD)),
                    rect,
                );
                continue;
            }
            let [label, plot] =
                Layout::horizontal([Constraint::Length(label_width), Constraint::Min(0)])
                    .areas(rect);
            frame.render_widget(
                Paragraph::new(row.label.clone()).style(base.fg(if row.items.is_empty() {
                    MUTED
                } else {
                    Color::White
                })),
                label,
            );
            let width = plot.width as usize;
            let mut cells: Vec<Span<'static>> = vec![Span::styled("·", base.fg(MUTED)); width];
            let max_cost = row
                .items
                .iter()
                .filter_map(|item| item.usage.map(|u| u.cost))
                .fold(0.0, f64::max)
                .max(f64::MIN_POSITIVE);
            for (ii, item) in row.items.iter().enumerate() {
                let span = (row.range.1 - row.range.0).max(1.0);
                let left = (((item.start - row.range.0) / span * width as f64)
                    .floor()
                    .max(0.0) as usize)
                    .min(width);
                let right = (((item.end - row.range.0) / span * width as f64)
                    .ceil()
                    .max(0.0) as usize)
                    .max(left + 1)
                    .min(width);
                let color = if item.usage.is_some() {
                    Color::Green
                } else {
                    kind_color(item.kind)
                };
                let chosen = selected && ii == self.item;
                let symbol = item.usage.map_or("━", |usage| {
                    let levels = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
                    levels[((usage.cost / max_cost * 7.0) as usize).min(7)]
                });
                for cell in &mut cells[left..right] {
                    *cell = Span::styled(
                        if chosen { "█" } else { symbol },
                        base.fg(color).add_modifier(if chosen {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    );
                }
            }
            frame.render_widget(Paragraph::new(Line::from(cells)), plot);
        }
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect, snapshot: &Snapshot) {
        if self.settings.mode != Mode::Timeline && self.show_projects {
            let selected = self
                .projects
                .get(self.project_selected)
                .map_or_else(String::new, |(cwd, seconds)| {
                    format!("{seconds:.1} 秒 · {}", clean(cwd))
                });
            frame.render_widget(Paragraph::new(vec![
                Line::raw(selected),
                Line::styled(format!("プロジェクト {}/{} · ↑↓/PgUp/PgDn/Home/End 選択 · Enter 活動一覧へ · u 一覧を切替", self.project_selected + usize::from(!self.projects.is_empty()), self.projects.len()), Style::default().fg(ACCENT)),
                Line::styled("←/→ 期間移動 · d/w/m 日/週/月 · s/p/P 読み込み元/プロジェクトの絞り込み", Style::default().fg(MUTED)),
            ]), area);
            return;
        }
        let selected = if self.date_focus && self.settings.mode != Mode::Timeline {
            format!(
                "選択日 {} · Enter 日表示 · z 選択日を時間軸で開く · ↑/↓ セッション一覧へ",
                self.day
            )
        } else {
            self.current_item()
                .map(|item| {
                    let session = &snapshot.sessions[item.selection.session];
                    let lane = &session.lanes[item.selection.lane];
                    format!(
                        "{} — {} · {} [{}] · {}",
                        local(item.start).format("%m/%d %H:%M:%S"),
                        local(item.end).format("%H:%M:%S"),
                        clean(&lane.name),
                        session.source.label(),
                        item.label
                    )
                })
                .unwrap_or_else(|| {
                    "↑/↓ 行  </> ブロック  Enter 詳細  Space プロジェクト折畳".into()
                })
        };
        let counts = if let Some(item) = self.current_item()
            && let Some(usage) = item.usage
        {
            format!(
                "入力 {} · キャッシュ読取 {} / 書込 {} · 出力 {} (推論 {}) · {} calls · USD {:.6} · 未換算 {} calls",
                usage.input,
                usage.cache_read,
                usage.cache_write,
                usage.output,
                usage.reasoning,
                usage.calls,
                usage.cost,
                usage.unpriced
            )
        } else if let Some(cost) = &self.cost {
            format!(
                "合計 USD {:.4} · {} tokens · {} calls · 未換算 {} calls （契約の請求額ではありません）",
                cost.usage.cost,
                tokens(&cost.usage),
                cost.usage.calls,
                cost.usage.unpriced
            )
        } else {
            format!(
                "行 {}/{} · ←/→ 前後  +/− 拡縮  t 今日  L 最新  g 期間  z 選択を拡大  [/] 親/子",
                self.selected + usize::from(!self.rows.is_empty()),
                self.rows.len()
            )
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(selected),
                Line::styled(counts, Style::default().fg(MUTED)),
                Line::styled(
                    if self.settings.mode != Mode::Timeline { "u プロジェクト別稼働時間 · Alt+←/→ 日を選択 · Ctrl+↑/↓ 時刻移動 · 0 全期間" }
                    else { "c 値  o 行  a 集約  v 範囲内のみ · Alt+1..6 1h/6h/24h/7d/30d/90d · 0 全期間" },
                    Style::default().fg(MUTED),
                ),
            ]),
            area,
        );
    }

    fn draw_input(&self, frame: &mut Frame<'_>, area: Rect) {
        let width = area.width.min(76);
        let height = area.height.min(7);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw("YYYY-MM-DD または YYYY-MM-DD..YYYY-MM-DD"),
                Line::raw("直近: 1h / 6h / 24h / 7d / 30d / 90d / all"),
                Line::styled(
                    format!("> {}", self.input.as_deref().unwrap_or_default()),
                    Style::default().fg(ACCENT),
                ),
                Line::styled(self.error.clone(), Style::default().fg(Color::Red)),
                Line::styled("Enter 適用 · Esc 取消", Style::default().fg(MUTED)),
            ])
            .block(Block::default().borders(Borders::ALL).title("期間・日付")),
            popup,
        );
    }
}

pub const HELP: &[(&str, &str)] = &[
    ("←/→ · h/l", "前後の期間"),
    ("+/−", "拡大・縮小"),
    ("t / L / 0", "今日 / 最新 / 全期間"),
    ("g", "日付・期間指定"),
    ("Alt+1..6", "直近 1h/6h/24h/7d/30d/90d"),
    ("{/}", "時間分布の前/次の活動"),
    ("↑/↓ · j/k", "行を移動"),
    ("</>", "前/次のブロック"),
    ("Enter", "ブロックの詳細"),
    ("z", "選択範囲をタイムラインで拡大"),
    ("c / o / a / v", "値 / 行 / まとめ方 / 範囲内だけ"),
    ("Space", "プロジェクト折畳"),
    ("[/]", "親/子レーン"),
    ("Alt+←/→", "カレンダーの日付選択"),
    ("Ctrl+↑/↓", "カレンダーの時刻移動"),
    ("Ctrl+Home/End", "履歴の最初/最新"),
];

fn cost_items(buckets: &[cost_timeline::Bucket], edges: &[f64]) -> Vec<Item> {
    buckets
        .iter()
        .filter_map(|bucket| {
            Some(Item {
                selection: bucket.sample,
                start: *edges.get(bucket.index)?,
                end: *edges.get(bucket.index + 1)?,
                label: format!(
                    "USD {:.6} · {} tokens · {} calls",
                    bucket.usage.cost,
                    tokens(&bucket.usage),
                    bucket.usage.calls
                ),
                kind: Kind::Assistant,
                usage: Some(bucket.usage),
            })
        })
        .collect()
}

#[derive(Debug, PartialEq)]
enum InputRange {
    Days(NaiveDate, NaiveDate),
    Preset(f64),
}

fn parse_range(input: &str) -> Result<InputRange, &'static str> {
    for (name, seconds) in ["1h", "6h", "24h", "7d", "30d", "90d"]
        .into_iter()
        .zip(PRESETS)
    {
        if input == name {
            return Ok(InputRange::Preset(seconds));
        }
    }
    if input == "all" {
        return Ok(InputRange::Preset(0.0));
    }
    let (a, b) = input.split_once("..").unwrap_or((input, input));
    let a = NaiveDate::parse_from_str(a.trim(), "%Y-%m-%d")
        .map_err(|_| "開始日を YYYY-MM-DD で入力してください")?;
    let b = NaiveDate::parse_from_str(b.trim(), "%Y-%m-%d")
        .map_err(|_| "終了日を YYYY-MM-DD で入力してください")?;
    if !(0..=9999).contains(&a.year()) || !(0..=9999).contains(&b.year()) {
        return Err("日付は西暦 0000〜9999 年の範囲で入力してください");
    }
    Ok(InputRange::Days(a.min(b), a.max(b)))
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

fn union_duration(intervals: &[(f64, f64)]) -> f64 {
    let mut current: Option<(f64, f64)> = None;
    let mut total = 0.0;
    for &(start, end) in intervals {
        current = match current {
            Some((a, b)) if start <= b => Some((a, b.max(end))),
            Some((a, b)) => {
                total += (b - a).max(0.0);
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    total + current.map_or(0.0, |(a, b)| (b - a).max(0.0))
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

fn kind_color(kind: Kind) -> Color {
    match kind {
        Kind::User => Color::Yellow,
        Kind::Assistant => Color::Cyan,
        Kind::Thinking => Color::DarkGray,
        Kind::Tool => Color::Green,
        Kind::Agent => Color::Magenta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rireki::model::{Event, Lane, Session, Source, Usage};

    fn fixture() -> Snapshot {
        let date = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        let start = midnight(date) + 9.0 * 3600.0;
        let make = |name: &str, times: &[(f64, f64)]| {
            let mut lane = Lane::new(name);
            lane.events = times
                .iter()
                .enumerate()
                .map(|(i, &(a, b))| Event {
                    start: start + a,
                    end: start + b,
                    kind: if i == 0 { Kind::User } else { Kind::Tool },
                    name: "日本語ツール".into(),
                    summary: "画面の確認".into(),
                    input: String::new(),
                    output: String::new(),
                    call_id: None,
                    text_ref: Default::default(),
                    usage: Usage {
                        input: 100,
                        output: 10,
                        ..Default::default()
                    },
                    model: Some("gpt-5".into()),
                    status: Default::default(),
                })
                .collect();
            lane.finish();
            lane
        };
        let mut session = Session {
            id: "test".into(),
            source: Source::Codex,
            cwd: "/tmp/日本語".into(),
            headless: false,
            title: "実装セッション".into(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![
                make("main", &[(0.0, 600.0), (700.0, 1200.0), (7200.0, 7800.0)]),
                make("child", &[(300.0, 900.0)]),
            ],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        Snapshot {
            sessions: vec![Arc::new(session)],
        }
    }

    fn screen(model: &mut Model, snapshot: &Snapshot, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| model.draw(frame, frame.area(), snapshot, &[0]))
            .unwrap();
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
        text
    }

    #[test]
    fn calendar_project_totals_match_gui_session_sums_and_scope() {
        let mut snapshot = fixture();
        let mut second = (*snapshot.sessions[0]).clone();
        second.id = "parallel-session".into();
        snapshot.sessions.push(Arc::new(second));
        let mut other = (*snapshot.sessions[0]).clone();
        other.id = "other-project".into();
        other.cwd = "/tmp/another".into();
        other.lanes.truncate(1);
        other.lanes[0].events.truncate(1);
        other.finalize();
        snapshot.sessions.push(Arc::new(other));
        let mut alpha = (*snapshot.sessions[0]).clone();
        alpha.id = "alpha".into();
        alpha.cwd = "/tmp/alpha".into();
        snapshot.sessions.push(Arc::new(alpha));

        let mut model = Model::default();
        model.sync(&snapshot, &[0, 1, 2, 3]);
        model.set_mode(Mode::Day);
        model.ensure_rows(&snapshot, &[0, 1, 2, 3]);
        // main と child の重複は1セッション内で統合し、別セッション同士はGUI同様に加算する。
        assert_eq!(
            model.projects,
            [
                ("/tmp/日本語".into(), 3600.0),
                ("/tmp/alpha".into(), 1800.0),
                ("/tmp/another".into(), 600.0),
            ]
        );
        assert_eq!(model.days[0].active, 1800.0);

        model.handle_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE),
            &snapshot,
            &[0, 1, 2, 3],
        );
        assert!(model.show_projects);
        model.handle_key(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            &snapshot,
            &[0, 1, 2, 3],
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 3)).unwrap();
        terminal
            .draw(|frame| model.draw_projects(frame, frame.area()))
            .unwrap();
        assert_eq!(model.project_selected, 2);
        assert_eq!(model.project_offset, 1);
        model.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &snapshot,
            &[0, 1, 2, 3],
        );
        assert!(!model.show_projects);
        assert_eq!(model.rows[model.selected].cwd, "/tmp/another");
        assert!(model.current_item().is_some());

        model.sync(&snapshot, &[2]);
        model.ensure_rows(&snapshot, &[2]);
        assert_eq!(model.projects, [("/tmp/another".into(), 600.0)]);
    }

    #[test]
    fn date_ranges_are_inclusive_and_reversed_input_is_normalized() {
        let a = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let b = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        assert_eq!(
            parse_range("2026-10-22..2026-10-01"),
            Ok(InputRange::Days(a, b))
        );
        assert_eq!(parse_range("2026-10-22"), Ok(InputRange::Days(b, b)));
        assert!(parse_range("2026-02-30").is_err());
        assert_eq!(parse_range("90d"), Ok(InputRange::Preset(7776000.0)));
        let mut model = Model {
            input: Some("2026-10-22..2026-10-22".into()),
            ..Default::default()
        };
        model.edit(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(model.range, (midnight(b), midnight(b + Duration::days(1))));
        assert!(!model.is_editing());
    }

    #[test]
    fn calendar_groups_child_lanes_and_excludes_idle_time() {
        let snapshot = fixture();
        let mut model = Model::default();
        model.sync(&snapshot, &[0]);
        model.set_mode(Mode::Day);
        model.day = local(snapshot.sessions[0].start).date_naive();
        model.ensure_rows(&snapshot, &[0]);
        assert_eq!(model.days.len(), 1);
        assert_eq!(model.days[0].sessions, 1);
        assert_eq!(model.days[0].active, 1800.0);
        assert_eq!(model.rows.iter().filter(|row| !row.header).count(), 2);
        assert_eq!(
            model.rows[1].items[0].selection,
            Selection {
                session: 0,
                lane: 0,
                first: 0,
                last: 1
            }
        );
        assert_eq!(
            union_duration(&[(0.0, 10.0), (2.0, 4.0), (8.0, 15.0), (20.0, 21.0)]),
            16.0
        );
    }

    #[test]
    fn timeline_drilldown_and_folding_keep_real_selections() {
        let snapshot = fixture();
        let mut model = Model::default();
        model.sync(&snapshot, &[0]);
        model.set_range(
            snapshot.sessions[0].start - 1.0,
            snapshot.sessions[0].end + 1.0,
        );
        model.settings.granularity = Granularity::Call;
        model.ensure_rows(&snapshot, &[0]);
        model.selected = 1;
        model.item = 1;
        let expected = Selection {
            session: 0,
            lane: 0,
            first: 1,
            last: 1,
        };
        assert_eq!(
            model.handle_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &snapshot,
                &[0]
            ),
            Action::Open(expected)
        );
        model.handle_key(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &snapshot,
            &[0],
        );
        assert!(model.range.0 < snapshot.sessions[0].lanes[0].events[1].start);
        assert!(model.range.1 > snapshot.sessions[0].lanes[0].events[1].end);
        model.handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &snapshot,
            &[0],
        );
        model.ensure_rows(&snapshot, &[0]);
        assert_eq!(model.rows.len(), 1);
        model.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &snapshot,
            &[0],
        );
        model.ensure_rows(&snapshot, &[0]);
        assert!(model.rows.len() > 1);
    }

    #[test]
    fn directory_rows_use_main_lanes_and_visible_filter_removes_empty_projects() {
        let snapshot = fixture();
        let mut model = Model::default();
        model.sync(&snapshot, &[0]);
        model.settings.directory = true;
        model.settings.granularity = Granularity::Call;
        model.set_range(
            snapshot.sessions[0].start - 1.0,
            snapshot.sessions[0].end + 1.0,
        );
        model.ensure_rows(&snapshot, &[0]);
        assert_eq!(
            model.rows.iter().map(|row| row.items.len()).sum::<usize>(),
            3
        );
        assert!(
            model
                .rows
                .iter()
                .flat_map(|row| &row.items)
                .all(|item| item.selection.lane == 0)
        );
        model.settings.in_view_only = true;
        model.step(1);
        model.ensure_rows(&snapshot, &[0]);
        assert!(model.rows.is_empty());
    }

    #[test]
    fn costs_keep_unknown_model_usage_and_invalidate_after_period_moves() {
        let mut snapshot = fixture();
        Arc::make_mut(&mut snapshot.sessions[0]).lanes[0].events[0].model =
            Some("unpriced-test-model".into());
        let mut model = Model::default();
        model.sync(&snapshot, &[0]);
        model.settings.cost = true;
        model.set_range(
            snapshot.sessions[0].start - 1.0,
            snapshot.sessions[0].end + 1.0,
        );
        let view = model.cost_request().unwrap();
        let costs = cost_timeline::Cache::default()
            .build_while(&snapshot, &[0], view, false, || true)
            .unwrap();
        assert_eq!(costs.usage.calls, 4);
        assert_eq!(costs.usage.unpriced, 1);
        assert_eq!(tokens(&costs.usage), 440);
        model.apply_cost(Arc::new(costs));
        assert!(model.cost_request().is_none());
        model.ensure_rows(&snapshot, &[0]);
        assert!(!model.rows[1].items.is_empty());
        assert!(screen(&mut model, &snapshot, 100, 26).contains("API換算料金"));
        model.step(1);
        assert!(model.cost_request().is_some());
        assert!(model.cost.is_none());
    }

    #[test]
    fn all_modes_render_japanese_at_small_and_large_terminal_sizes() {
        let snapshot = fixture();
        let mut model = Model::default();
        model.sync(&snapshot, &[0]);
        for mode in [Mode::Timeline, Mode::Day, Mode::Week, Mode::Month] {
            model.set_mode(mode);
            for (width, height) in [(120, 32), (44, 18), (12, 6), (1, 1)] {
                let rendered = screen(&mut model, &snapshot, width, height);
                if width >= 44 {
                    assert!(rendered.contains(mode.label()));
                }
            }
        }
        assert!(!clean("危険\x1b[31m\nログ").contains('\x1b'));
    }

    #[test]
    fn range_input_keeps_quit_as_text_and_settings_round_trip() {
        let mut model = Model {
            input: Some(String::new()),
            ..Default::default()
        };
        model.edit(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(model.input.as_deref(), Some("q"));
        model.paste("\x1b\n2026-10-22");
        assert_eq!(model.input.as_deref(), Some("q2026-10-22"));
        model.settings.directory = true;
        model.settings.collapsed.insert("日本語".into());
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&model.settings).unwrap()).unwrap();
        assert_eq!(restored, model.settings);
    }

    #[test]
    fn month_date_cursor_drills_into_the_chosen_day_and_full_range_includes_last_instant() {
        let mut snapshot = fixture();
        let session = Arc::make_mut(&mut snapshot.sessions[0]);
        session.lanes[0].events[2].end = session.lanes[0].events[2].start;
        session.finalize();
        let mut model = Model::default();
        model.sync(&snapshot, &[0]);
        assert_eq!(model.minimap.iter().sum::<u32>(), 4);
        model.set_mode(Mode::Month);
        let day = model.day;
        model.handle_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::ALT),
            &snapshot,
            &[0],
        );
        assert_eq!(model.day, day + Duration::days(1));
        model.handle_key(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &snapshot,
            &[0],
        );
        assert_eq!(
            model.range,
            (
                midnight(day + Duration::days(1)),
                midnight(day + Duration::days(2))
            )
        );
        model.set_mode(Mode::Month);
        model.handle_key(
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            &snapshot,
            &[0],
        );
        model.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &snapshot,
            &[0],
        );
        assert_eq!(model.mode(), Mode::Day);
        assert_eq!(model.day, day);
    }
}
