//! 画面間の選択・履歴・設定・背景要求をまとめる。各画面は共通コアの同じ版を参照する。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::{Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use serde::{Deserialize, Serialize};

use rireki::catalog::SourceFilter;
use rireki::cost_timeline;
use rireki::detail::{DetailCache, Selection};
use rireki::model::Session;

use super::inspect::{self, Inspector};
use super::metrics;
use super::operations::{self, Request};
use super::state::{Focus, Location, State};
use super::temporal;
use super::view::View;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
enum Page {
    #[default]
    History,
    Temporal,
    Statistics,
    Mcp,
    Settings,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    page: Page,
    source: SourceFilter,
    project: Option<String>,
    live_only: bool,
    sidebar: bool,
    sidebar_width: u16,
    events_height: u16,
    fullscreen: bool,
    temporal: temporal::Settings,
    metrics: metrics::Settings,
    inspector: inspect::Settings,
    mcp_port: u16,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            page: Page::History,
            source: SourceFilter::All,
            project: None,
            live_only: false,
            sidebar: true,
            sidebar_width: 32,
            events_height: 38,
            fullscreen: false,
            temporal: temporal::Settings::default(),
            metrics: metrics::Settings::default(),
            inspector: inspect::Settings::default(),
            mcp_port: rireki::mcp::DEFAULT_PORT,
        }
    }
}

fn preferences_path() -> PathBuf {
    std::env::var_os("RIREKI_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rireki")
        })
        .join("tui.json")
}

enum Dialog {
    Projects { query: String, selected: usize },
    Export { path: String, content: String },
    Port(String),
}

pub enum Effect {
    Copy(String),
    Reveal(String),
}

#[derive(Clone)]
struct Visit {
    location: Location,
    tab: inspect::Tab,
}

pub struct Workspace {
    page: Page,
    temporal: temporal::Model,
    metrics: metrics::State,
    inspector: Inspector,
    selection: Option<Selection>,
    follow_lane: bool,
    location: Option<Location>,
    selected_session: Option<Arc<Session>>,
    cache: Option<Box<DetailCache>>,
    cache_revision: u64,
    inspect_loading: bool,
    observed_selection: Option<Selection>,
    observed_location: Option<Location>,
    history: VecDeque<Visit>,
    future: Vec<Visit>,
    mark: Option<Location>,
    last_revision: u64,
    last_scope: Vec<usize>,
    last_day: NaiveDate,
    last_live_check: Instant,
    metrics_dirty: bool,
    last_cost: Option<cost_timeline::View>,
    generations: [u64; 4],
    pending: Vec<Request>,
    effects: Vec<Effect>,
    cancellations: Vec<(usize, u64)>,
    failed: bool,
    sidebar: bool,
    sidebar_width: u16,
    events_height: u16,
    fullscreen: bool,
    settings_row: usize,
    persistent: bool,
    help: bool,
    help_scroll: u16,
    dialog: Option<Dialog>,
    mcp_port: u16,
    mcp_url: Option<String>,
    mcp_config: String,
    mcp_busy: bool,
    mcp_error: Option<String>,
    mcp_scroll: u16,
}

impl Workspace {
    pub fn new(persistent: bool, state: &mut State) -> Self {
        let prefs: Preferences = if persistent {
            std::fs::read(preferences_path())
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        } else {
            Preferences::default()
        };
        state.live_only = prefs.live_only;
        state.restore_filter(prefs.source, prefs.project);
        let mut temporal = temporal::Model::default();
        temporal.apply_settings(prefs.temporal);
        let mut metrics = metrics::State::default();
        metrics.apply_settings(prefs.metrics);
        let mut inspector = Inspector::default();
        inspector.apply_settings(prefs.inspector);
        if prefs.fullscreen {
            state.focus = Focus::Detail;
        } else if !prefs.sidebar {
            state.focus = Focus::Events;
        }
        Self {
            page: prefs.page,
            temporal,
            metrics,
            inspector,
            selection: None,
            follow_lane: false,
            location: None,
            selected_session: None,
            cache: None,
            cache_revision: 0,
            inspect_loading: false,
            observed_selection: None,
            observed_location: None,
            history: VecDeque::new(),
            future: Vec::new(),
            mark: None,
            last_revision: u64::MAX,
            last_scope: vec![],
            last_day: Local::now().date_naive(),
            last_live_check: Instant::now(),
            metrics_dirty: true,
            last_cost: None,
            generations: [0; 4],
            pending: vec![],
            effects: vec![],
            cancellations: vec![],
            failed: false,
            sidebar: prefs.sidebar,
            sidebar_width: prefs.sidebar_width.clamp(20, 50),
            events_height: prefs.events_height.clamp(20, 75),
            fullscreen: prefs.fullscreen,
            settings_row: 0,
            persistent,
            help: false,
            help_scroll: 0,
            dialog: None,
            mcp_port: prefs.mcp_port.max(1),
            mcp_url: None,
            mcp_config: String::new(),
            mcp_busy: false,
            mcp_error: None,
            mcp_scroll: 0,
        }
    }

    pub fn save(&self, state: &State) -> std::io::Result<()> {
        if !self.persistent {
            return Ok(());
        }
        let prefs = Preferences {
            page: self.page,
            source: state.source,
            project: state.project.clone(),
            live_only: state.live_only,
            sidebar: self.sidebar,
            sidebar_width: self.sidebar_width,
            events_height: self.events_height,
            fullscreen: self.fullscreen,
            temporal: self.temporal.settings(),
            metrics: self.metrics.settings(),
            inspector: self.inspector.settings(),
            mcp_port: self.mcp_port,
        };
        let path = preferences_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&prefs)?)
    }

    fn generation(&mut self, kind: usize) -> u64 {
        self.generations[kind] = self.generations[kind].wrapping_add(1);
        self.generations[kind]
    }
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.pending)
    }
    pub fn take_cancellations(&mut self) -> Vec<(usize, u64)> {
        std::mem::take(&mut self.cancellations)
    }
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn apply(&mut self, update: operations::Update, state: &mut State) {
        if self.failed {
            return;
        }
        match update {
            operations::Update::Inspect { generation, detail }
                if generation == self.generations[0] =>
            {
                self.cache = detail;
                self.inspect_loading = false;
                self.cache_revision = self.cache_revision.wrapping_add(1);
            }
            operations::Update::Analysis {
                generation,
                analysis: Some(analysis),
            } if generation == self.generations[1] => self.inspector.set_analysis(analysis),
            operations::Update::Metrics {
                generation,
                computed,
            } if generation == self.generations[2] => self.metrics.apply(*computed),
            operations::Update::Cost {
                generation,
                summary,
            } if generation == self.generations[3] => self.temporal.apply_cost(summary),
            operations::Update::Mcp { url, config, error } => {
                self.mcp_busy = false;
                self.mcp_url = url;
                self.mcp_config = config;
                self.mcp_error = error;
                state.status = self.mcp_error.clone().unwrap_or_else(|| {
                    if self.mcp_url.is_some() {
                        "MCP サーバーを起動しました".into()
                    } else {
                        "MCP サーバーを停止しました".into()
                    }
                });
            }
            operations::Update::Error(error) => {
                state.status = error;
                self.failed = true;
                self.metrics.loading = false;
                self.inspect_loading = false;
                self.mcp_busy = false;
                self.pending.clear();
            }
            _ => {}
        }
    }

    pub fn sync(&mut self, state: &mut State) -> bool {
        if self.last_live_check.elapsed().as_secs() >= 3 {
            state.refresh_live();
            self.last_live_check = Instant::now();
        }
        let changed = self.last_revision != state.revision;
        let scope_changed = self.last_scope != state.catalog.order;
        if changed || scope_changed {
            self.temporal.sync(&state.snapshot, &state.catalog.order);
            self.metrics_dirty = true;
            self.last_cost = None;
            self.last_scope = state.catalog.order.clone();
            self.last_revision = state.revision;
        }
        if changed && !self.failed {
            self.pending.push(Request::Snapshot(state.snapshot.clone()));
        }
        if self.last_day != Local::now().date_naive() {
            self.last_day = Local::now().date_naive();
            self.metrics_dirty = true;
        }
        let current = state.selection();
        let same_event = changed
            && current.is_some()
            && self
                .observed_location
                .as_ref()
                .and_then(|l| l.resolve(&state.snapshot))
                == current;
        if same_event {
            if let Some(mut selection) = self
                .location
                .as_ref()
                .and_then(|l| l.resolve(&state.snapshot))
            {
                let version = &state.snapshot.sessions[selection.session];
                if self.follow_lane {
                    selection.last = version.lanes[selection.lane].events.len() - 1;
                }
                if self.selection != Some(selection)
                    || self
                        .selected_session
                        .as_ref()
                        .is_none_or(|old| !Arc::ptr_eq(old, version))
                {
                    self.load(selection, state, false);
                }
            } else if let Some(selection) = current {
                self.load(selection, state, true);
            }
        } else if current != self.observed_selection || changed {
            if !changed && let Some(location) = self.location.clone() {
                self.history.push_back(Visit {
                    location,
                    tab: self.inspector.settings().tab,
                });
                while self.history.len() > 128 {
                    self.history.pop_front();
                }
                self.future.clear();
            }
            if let Some(selection) = current {
                self.load(selection, state, true);
            } else {
                self.clear_selection();
            }
        }
        self.observed_selection = current;
        self.observed_location = current.and_then(|s| Location::capture(&state.snapshot, s));
        if self.page == Page::Statistics && self.metrics_dirty && !self.failed {
            self.request_metrics(state);
        }
        if self.page == Page::Temporal
            && !self.failed
            && let Some(view) = self.temporal.cost_request()
            && self.last_cost != Some(view)
        {
            self.last_cost = Some(view);
            let generation = self.generation(3);
            self.pending.push(Request::Cost {
                generation,
                snapshot: state.snapshot.clone(),
                scope: state.catalog.order.clone(),
                view,
                in_view_only: self.temporal.in_view_only(),
            });
        }
        changed || scope_changed
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.selected_session = None;
        self.cache = None;
        self.location = None;
        self.inspect_loading = false;
        for kind in 0..2 {
            let generation = self.generation(kind);
            self.cancellations.push((kind, generation));
        }
        self.inspector.selection_changed();
    }

    fn load(&mut self, selection: Selection, state: &State, reset: bool) {
        let Some(session) = state.snapshot.sessions.get(selection.session) else {
            return;
        };
        if session
            .lanes
            .get(selection.lane)
            .and_then(|lane| lane.events.get(selection.first..=selection.last))
            .is_none()
        {
            return;
        }
        if reset {
            self.follow_lane = selection.first == 0
                && selection.last + 1 == session.lanes[selection.lane].events.len();
        }
        self.selection = Some(selection);
        self.location = Location::capture(&state.snapshot, selection);
        self.selected_session = Some(session.clone());
        self.cache = None;
        self.cache_revision = self.cache_revision.wrapping_add(1);
        if reset {
            self.inspector.selection_changed();
        } else {
            self.inspector.invalidate_analysis();
        }
        if self.failed {
            return;
        }
        self.inspect_loading = true;
        let generation = self.generation(0);
        self.pending.push(Request::Inspect {
            generation,
            selection,
            session: session.clone(),
        });
        if let inspect::Action::Analyze {
            selection,
            filter,
            order,
        } = self.inspector.analysis_request(selection)
        {
            let generation = self.generation(1);
            self.pending.push(Request::Analysis {
                generation,
                selection,
                session: session.clone(),
                filter,
                order,
            });
        }
    }

    fn open(&mut self, selection: Selection, state: &mut State, remember: bool) {
        if state.reveal(selection) {
            if remember && self.selection != Some(selection) {
                if let Some(location) = self.location.clone() {
                    self.history.push_back(Visit {
                        location,
                        tab: self.inspector.settings().tab,
                    });
                }
                while self.history.len() > 128 {
                    self.history.pop_front();
                }
                self.future.clear();
            }
            self.page = Page::History;
            state.focus = Focus::Detail;
            self.observed_selection = state.selection();
            self.observed_location = self
                .observed_selection
                .and_then(|s| Location::capture(&state.snapshot, s));
            self.load(selection, state, true);
        }
    }

    fn navigate(&mut self, state: &mut State, forward: bool) {
        loop {
            let next = if forward {
                self.future.pop()
            } else {
                self.history.pop_back()
            };
            let Some(next) = next else {
                state.status = "移動できる閲覧履歴がありません".into();
                return;
            };
            if let Some(selection) = next.location.resolve(&state.snapshot) {
                if let Some(current) = self.location.clone() {
                    if forward {
                        self.history.push_back(Visit {
                            location: current,
                            tab: self.inspector.settings().tab,
                        });
                    } else {
                        self.future.push(Visit {
                            location: current,
                            tab: self.inspector.settings().tab,
                        });
                    }
                }
                self.open(selection, state, false);
                self.inspector.set_tab(next.tab);
                return;
            }
        }
    }

    fn request_metrics(&mut self, state: &State) {
        self.metrics_dirty = false;
        self.metrics.loading = true;
        let generation = self.generation(2);
        self.pending.push(Request::Metrics {
            generation,
            snapshot: state.snapshot.clone(),
            scope: state.catalog.order.clone(),
            params: self.metrics.params(),
        });
    }

    fn inspector_actions(&mut self, actions: Vec<inspect::Action>, state: &mut State) {
        for action in actions {
            match action {
                inspect::Action::Load(selection) => self.open(selection, state, true),
                inspect::Action::Select(selection) => {
                    self.open(selection, state, true);
                    self.inspector
                        .set_tab(if selection.first == selection.last {
                            inspect::Tab::Body
                        } else {
                            inspect::Tab::History
                        });
                }
                inspect::Action::Analyze {
                    selection,
                    filter,
                    order,
                } => {
                    if !self.failed
                        && let Some(session) = state.snapshot.sessions.get(selection.session)
                    {
                        let generation = self.generation(1);
                        self.pending.push(Request::Analysis {
                            generation,
                            selection,
                            session: session.clone(),
                            filter,
                            order,
                        });
                    }
                }
                inspect::Action::Copy(text) => self.effects.push(Effect::Copy(text)),
                inspect::Action::Export(content) => {
                    self.dialog = Some(Dialog::Export {
                        path: format!(
                            "rireki-analysis-{}.json",
                            Local::now().format("%Y%m%d-%H%M%S")
                        ),
                        content,
                    })
                }
                inspect::Action::Reveal(path) => self.effects.push(Effect::Reveal(path)),
                inspect::Action::Status(status) => state.status = status,
            }
        }
    }

    pub fn tick(&mut self) -> bool {
        if self.page != Page::History {
            self.inspector.pause();
            return false;
        }
        self.inspector.tick(Instant::now())
    }

    pub fn paste(&mut self, text: &str, state: &mut State) {
        if let Some(dialog) = &mut self.dialog {
            let target = match dialog {
                Dialog::Projects { query, .. } => query,
                Dialog::Export { path, .. } => path,
                Dialog::Port(port) => port,
            };
            append_input(target, text);
            return;
        }
        if self.inspector.is_editing() {
            self.inspector.paste(text);
        } else if self.temporal.is_editing() {
            self.temporal.paste(text);
        } else {
            state.paste(text);
        }
    }

    /// true は終了要求。文字入力中は画面ショートカットより入力を優先する。
    pub fn key(&mut self, key: KeyEvent, state: &mut State) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            return true;
        }
        if self.dialog.is_some() {
            self.dialog_key(key, state);
            return false;
        }
        if self.inspector.is_editing() {
            self.inspect_key(key, state);
            return false;
        }
        if self.page == Page::Temporal && self.temporal.is_editing() {
            self.temporal_key(key, state);
            return false;
        }
        if state.editing_query {
            return state.handle_key(key);
        }
        if key.code == KeyCode::Char('q') {
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('l' | 'L') => {
                    state.toggle_live();
                    return false;
                }
                KeyCode::Char('f' | 'F') => {
                    self.page = Page::History;
                    self.fullscreen = false;
                    self.sidebar = true;
                    state.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
                    return false;
                }
                KeyCode::Char('r' | 'R') => {
                    state.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
                    return false;
                }
                _ => {}
            }
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.help = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(10),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::F(1) => {
                self.page = Page::History;
                return false;
            }
            KeyCode::F(n @ 2..=5) => {
                self.page = Page::Temporal;
                self.temporal.set_mode(match n {
                    3 => temporal::Mode::Day,
                    4 => temporal::Mode::Week,
                    5 => temporal::Mode::Month,
                    _ => temporal::Mode::Timeline,
                });
                self.last_cost = None;
                return false;
            }
            KeyCode::F(6) => {
                self.page = Page::Statistics;
                return false;
            }
            KeyCode::F(7) => {
                self.page = Page::Mcp;
                return false;
            }
            KeyCode::F(8) => {
                self.page = Page::Settings;
                return false;
            }
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
                return false;
            }
            KeyCode::Char('/') if self.page == Page::History && state.focus != Focus::Detail => {
                self.sidebar = true;
                self.fullscreen = false;
                state.handle_key(key);
                return false;
            }
            KeyCode::Char('s') => {
                state.handle_key(key);
                return false;
            }
            KeyCode::Char('p') => {
                state.handle_key(key);
                return false;
            }
            KeyCode::Char('P') => {
                self.dialog = Some(Dialog::Projects {
                    query: String::new(),
                    selected: 0,
                });
                return false;
            }
            KeyCode::Char('B') => {
                self.sidebar = !self.sidebar;
                if !self.sidebar && state.focus == Focus::Sessions {
                    state.focus = Focus::Events;
                }
                return false;
            }
            KeyCode::Char('[') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.navigate(state, false);
                return false;
            }
            KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.navigate(state, true);
                return false;
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.fullscreen = !self.fullscreen;
                state.focus = Focus::Detail;
                self.page = Page::History;
                return false;
            }
            _ => {}
        }
        match self.page {
            Page::History => {
                if state.focus == Focus::Detail {
                    match key.code {
                        KeyCode::Tab | KeyCode::BackTab => {
                            if !self.fullscreen {
                                state.handle_key(key);
                                if !self.sidebar && state.focus == Focus::Sessions {
                                    state.focus = Focus::Events;
                                }
                            }
                        }
                        KeyCode::Esc => {
                            self.fullscreen = false;
                            state.focus = Focus::Events;
                        }
                        _ => self.inspect_key(key, state),
                    }
                } else if key.code == KeyCode::Enter && state.focus == Focus::Events {
                    if let Some(selection) = state.selection() {
                        self.open(selection, state, true);
                        self.inspector.set_tab(inspect::Tab::Body);
                    }
                } else if key.code == KeyCode::Char('v') && state.focus == Focus::Events {
                    if let Some(mark) = self.mark.take() {
                        if let Some(mark) = mark.resolve(&state.snapshot)
                            && let Some(current) = state.selection()
                            && current.session == mark.session
                            && current.lane == mark.lane
                        {
                            self.open(
                                Selection {
                                    first: mark.first.min(current.first),
                                    last: mark.first.max(current.first),
                                    ..mark
                                },
                                state,
                                true,
                            );
                        } else {
                            state.status = "範囲の開始と終了は同じレーンで選んでください".into();
                        }
                    } else {
                        self.mark = state
                            .selection()
                            .and_then(|selection| Location::capture(&state.snapshot, selection));
                        state.status =
                            "範囲の開始を記録しました。終了イベントへ移動して v で確定".into();
                    }
                } else if matches!(key.code, KeyCode::Char('a' | 'I')) {
                    if let Some(mut selection) = state.selection() {
                        selection.first = 0;
                        selection.last = state.snapshot.sessions[selection.session].lanes
                            [selection.lane]
                            .events
                            .len()
                            - 1;
                        self.open(selection, state, true);
                    }
                } else {
                    if state.handle_key(key) {
                        return true;
                    }
                    if !self.sidebar && state.focus == Focus::Sessions {
                        state.focus = Focus::Events;
                    }
                }
            }
            Page::Temporal => self.temporal_key(key, state),
            Page::Statistics if key.code == KeyCode::Esc => self.page = Page::History,
            Page::Statistics => match self.metrics.handle_key(key) {
                metrics::Action::None => {}
                metrics::Action::Recompute => self.metrics_dirty = true,
                metrics::Action::Timeline { start, end } => {
                    self.page = Page::Temporal;
                    self.temporal.set_mode(temporal::Mode::Timeline);
                    self.temporal.set_range(start, end);
                    self.last_cost = None;
                }
                metrics::Action::Project(project) => state.project(Some(project)),
                metrics::Action::Event(selection) => self.open(selection, state, true),
            },
            Page::Mcp => self.mcp_key(key, state),
            Page::Settings => self.settings_key(key, state),
        }
        false
    }

    fn inspect_key(&mut self, key: KeyEvent, state: &mut State) {
        if let (Some(selection), Some(session)) = (self.selection, &self.selected_session) {
            let actions = self.inspector.key(
                key,
                inspect::Context {
                    session,
                    session_index: selection.session,
                    selection,
                    cache: self.cache.as_deref(),
                    revision: self.cache_revision,
                },
            );
            self.inspector_actions(actions, state);
        }
    }
    fn temporal_key(&mut self, key: KeyEvent, state: &mut State) {
        match self
            .temporal
            .handle_key(key, &state.snapshot, &state.catalog.order)
        {
            temporal::Action::Open(selection) => self.open(selection, state, true),
            temporal::Action::Changed => self.last_cost = None,
            temporal::Action::None => {}
        }
    }

    fn mcp_key(&mut self, key: KeyEvent, state: &mut State) {
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') if !self.mcp_busy && !self.failed => {
                self.mcp_busy = true;
                self.mcp_error = None;
                self.pending.push(Request::Snapshot(state.snapshot.clone()));
                self.pending.push(Request::Mcp {
                    port: self.mcp_url.is_none().then_some(self.mcp_port),
                });
            }
            KeyCode::Char('e') if self.mcp_url.is_none() => {
                self.dialog = Some(Dialog::Port(self.mcp_port.to_string()))
            }
            KeyCode::Char('y') if !self.mcp_config.is_empty() => {
                self.effects.push(Effect::Copy(self.mcp_config.clone()))
            }
            KeyCode::Char('u') => {
                if let Some(url) = &self.mcp_url {
                    self.effects.push(Effect::Copy(url.clone()));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.mcp_scroll = self.mcp_scroll.saturating_add(1).min(30)
            }
            KeyCode::Up | KeyCode::Char('k') => self.mcp_scroll = self.mcp_scroll.saturating_sub(1),
            KeyCode::PageDown => self.mcp_scroll = self.mcp_scroll.saturating_add(8).min(30),
            KeyCode::PageUp => self.mcp_scroll = self.mcp_scroll.saturating_sub(8),
            KeyCode::Home => self.mcp_scroll = 0,
            KeyCode::Esc => self.page = Page::History,
            _ => {}
        }
    }

    fn settings_key(&mut self, key: KeyEvent, state: &mut State) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings_row = (self.settings_row + 1).min(7)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings_row = self.settings_row.saturating_sub(1)
            }
            KeyCode::Enter
            | KeyCode::Char(' ')
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h' | 'l') => {
                let down = matches!(key.code, KeyCode::Left | KeyCode::Char('h'));
                match self.settings_row {
                    0 => self.sidebar = !self.sidebar,
                    1 => {
                        self.fullscreen = !self.fullscreen;
                        if self.fullscreen {
                            state.focus = Focus::Detail;
                        }
                    }
                    2 => {
                        self.sidebar_width = if down {
                            self.sidebar_width.saturating_sub(5).max(20)
                        } else {
                            (self.sidebar_width + 5).min(50)
                        }
                    }
                    3 => {
                        self.events_height = if down {
                            self.events_height.saturating_sub(5).max(20)
                        } else {
                            (self.events_height + 5).min(75)
                        }
                    }
                    4 => state.toggle_live(),
                    5 => {
                        state.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
                    }
                    6 => {
                        self.dialog = Some(Dialog::Projects {
                            query: String::new(),
                            selected: 0,
                        })
                    }
                    _ => {
                        self.sidebar = true;
                        self.fullscreen = false;
                        self.sidebar_width = 32;
                        self.events_height = 38;
                        self.temporal.apply_settings(temporal::Settings::default());
                        self.metrics.apply_settings(metrics::Settings::default());
                        self.inspector.apply_settings(inspect::Settings::default());
                        self.metrics_dirty = true;
                        self.last_cost = None;
                    }
                }
            }
            KeyCode::Esc => self.page = Page::History,
            _ => {}
        }
    }

    fn dialog_key(&mut self, key: KeyEvent, state: &mut State) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        if key.code == KeyCode::Esc {
            return;
        }
        let mut close = false;
        match &mut dialog {
            Dialog::Projects { query, selected } => {
                let choices = project_choices(state, query);
                match key.code {
                    KeyCode::Down => {
                        *selected = selected
                            .saturating_add(1)
                            .min(choices.len().saturating_sub(1))
                    }
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Enter => {
                        if let Some(project) = choices.get(*selected) {
                            state.project(project.clone());
                        }
                        close = true;
                    }
                    _ => {
                        edit_input(query, key);
                        *selected = 0;
                    }
                }
            }
            Dialog::Port(port) => {
                if key.code == KeyCode::Enter {
                    match port.trim().parse::<u16>() {
                        Ok(value) if value > 0 => {
                            self.mcp_port = value;
                            close = true;
                        }
                        _ => state.status = "ポート番号は 1〜65535 で指定してください".into(),
                    }
                } else {
                    edit_input(port, key);
                }
            }
            Dialog::Export { path, content } => {
                if key.code == KeyCode::Enter {
                    match super::effects::save(std::path::Path::new(path), content) {
                        Ok(()) => {
                            state.status = format!("保存しました: {path}");
                            close = true;
                        }
                        Err(error) => {
                            state.status = format!(
                                "保存できません: {error}（別のファイル名を指定してください）"
                            )
                        }
                    }
                } else {
                    edit_input(path, key);
                }
            }
        }
        if !close {
            self.dialog = Some(dialog);
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>, state: &mut State, view: &mut View) {
        let area = frame.area();
        if area.is_empty() {
            return;
        }
        let [nav, header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(if area.height >= 10 { 2 } else { 1 }),
            Constraint::Min(0),
            Constraint::Length(if area.height >= 10 { 2 } else { 1 }),
        ])
        .areas(area);
        let active = match self.page {
            Page::History => 1,
            Page::Temporal => match self.temporal.mode() {
                temporal::Mode::Timeline => 2,
                temporal::Mode::Day => 3,
                temporal::Mode::Week => 4,
                temporal::Mode::Month => 5,
            },
            Page::Statistics => 6,
            Page::Mcp => 7,
            Page::Settings => 8,
        };
        let labels = ["履歴", "時間軸", "日", "週", "月", "統計", "MCP", "設定"];
        let tabs: Vec<_> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                ratatui::text::Span::styled(
                    format!(" F{}{} ", i + 1, label),
                    if active == i + 1 {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::Cyan)
                    },
                )
            })
            .collect();
        frame.render_widget(Line::from(tabs), nav);
        view.header(frame, header, state);
        match self.page {
            Page::History => self.draw_history(frame, body, state, view),
            Page::Temporal => {
                self.temporal
                    .draw(frame, body, &state.snapshot, &state.catalog.order)
            }
            Page::Statistics => metrics::draw(frame, body, &mut self.metrics, &state.snapshot),
            Page::Mcp => self.draw_mcp(frame, body),
            Page::Settings => self.draw_settings(frame, body, state),
        }
        let loading = if self.inspect_loading && self.page == Page::History {
            "詳細読み込み中…  "
        } else {
            ""
        };
        let status = format!(
            "{loading}{}{}",
            if state.live_only {
                "進行中のみ  "
            } else {
                ""
            },
            state.status
        );
        let hints = if self.page == Page::History {
            " Tab:ペイン  1〜6:詳細タブ  /:検索  s/p/P:絞込  B:サイドバー  Shift-Enter:全画面  ?:操作  q:終了"
        } else {
            " F1〜F8:画面  s/p/P:絞込  ?:全操作  q:終了"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(clean(&status)),
                Line::styled(hints, Style::default().fg(Color::Cyan)),
            ])
            .scroll((u16::from(footer.height == 1), 0)),
            footer,
        );
        if self.help {
            self.draw_help(frame, area);
        }
        if let Some(dialog) = &self.dialog {
            draw_dialog(frame, area, dialog, state);
        }
    }

    fn draw_history(&mut self, frame: &mut Frame<'_>, body: Rect, state: &mut State, view: &View) {
        if self.fullscreen {
            self.draw_inspect(frame, body);
            return;
        }
        if body.width >= 100 && body.height >= 12 {
            let right = if self.sidebar {
                let [sessions, right] = Layout::horizontal([
                    Constraint::Percentage(self.sidebar_width),
                    Constraint::Min(0),
                ])
                .areas(body);
                view.sessions(frame, sessions, state);
                right
            } else {
                body
            };
            let [events, detail] = Layout::vertical([
                Constraint::Percentage(self.events_height),
                Constraint::Min(0),
            ])
            .areas(right);
            view.events(frame, events, state);
            self.draw_inspect(frame, detail);
        } else {
            match state.focus {
                Focus::Sessions if self.sidebar => view.sessions(frame, body, state),
                Focus::Sessions | Focus::Events => view.events(frame, body, state),
                Focus::Detail => self.draw_inspect(frame, body),
            }
        }
    }

    fn draw_inspect(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if let (Some(selection), Some(session)) = (self.selection, &self.selected_session) {
            self.inspector.draw(
                frame,
                area,
                inspect::Context {
                    session,
                    session_index: selection.session,
                    selection,
                    cache: self.cache.as_deref(),
                    revision: self.cache_revision,
                },
            );
        } else {
            frame.render_widget(
                Paragraph::new("セッションとイベントを選択してください")
                    .block(Block::default().borders(Borders::ALL).title("詳細")),
                area,
            );
        }
    }

    fn draw_mcp(&self, frame: &mut Frame<'_>, area: Rect) {
        let status = if self.mcp_busy {
            "切り替え中…"
        } else if self.mcp_url.is_some() {
            "起動中"
        } else {
            "停止中"
        };
        let mut lines = vec![
            format!("状態: {status}  ポート: {}", self.mcp_port),
            "Enter: 起動/停止  e: ポート編集  y: 接続設定コピー  u: URLコピー".into(),
            "ローカルホスト限定・読み取り専用。現在読み込んだ履歴を共有します。".into(),
            String::new(),
        ];
        if let Some(error) = &self.mcp_error {
            lines.push(error.clone());
        }
        if let Some(url) = &self.mcp_url {
            lines.push(url.clone());
            lines.extend(self.mcp_config.lines().map(str::to_owned));
        }
        lines.extend([
            String::new(),
            "公開ツール".into(),
            "list_sessions — セッションの検索・一覧".into(),
            "get_session — メタデータ・サブエージェント・使用量".into(),
            "get_events — 本文・ツールの入力と結果".into(),
            "search_events — イベントの検索".into(),
            "analyze_history — 使用量・実行結果の集計".into(),
        ]);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .map(|line| Line::raw(clean(&line)))
                    .collect::<Vec<_>>(),
            )
            .scroll((self.mcp_scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" MCP サーバー "),
            ),
            area,
        );
    }

    fn draw_settings(&self, frame: &mut Frame<'_>, area: Rect, state: &State) {
        let rows = [
            format!("サイドバー: {}", on(self.sidebar)),
            format!("詳細の全画面: {}", on(self.fullscreen)),
            format!("サイドバーの幅: {}%", self.sidebar_width),
            format!("イベント一覧の高さ: {}%", self.events_height),
            format!("進行中のセッションのみ: {}", on(state.live_only)),
            format!("読み込み元: {:?}", state.source),
            format!(
                "プロジェクト: {}",
                state.project.as_deref().unwrap_or("すべて")
            ),
            "表示設定を既定に戻す".into(),
        ];
        let mut lines = vec![
            Line::raw("↑↓:選択  Enter/左右:変更  Esc:履歴へ戻る"),
            Line::default(),
        ];
        for (i, row) in rows.iter().enumerate() {
            lines.push(Line::styled(
                format!(
                    "{} {}",
                    if i == self.settings_row { "›" } else { " " },
                    clean(row)
                ),
                if i == self.settings_row {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ));
        }
        lines.push(Line::default());
        lines.push(Line::raw(if self.persistent {
            "終了時に表示・絞り込み・詳細タブを tui.json に保存します。"
        } else {
            "明示ログルートのため、通常の設定ファイルは読み書きしません。"
        }));
        lines.push(Line::raw(
            "料金は共通の prices.json のモデル別単価で計算します。",
        ));
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((
                    (self.settings_row + 4)
                        .saturating_sub(usize::from(area.height.saturating_sub(2)))
                        .min(u16::MAX as usize) as u16,
                    0,
                ))
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" 表示設定 ")),
            area,
        );
    }

    fn draw_help(&self, frame: &mut Frame<'_>, area: Rect) {
        let popup = popup(area);
        frame.render_widget(Clear, popup);
        let mut help = HELP.to_owned();
        help.push_str("\n\n時間軸・カレンダーの追加操作\n");
        for (key, action) in temporal::HELP {
            help.push_str(&format!("{key}: {action}\n"));
        }
        frame.render_widget(
            Paragraph::new(help)
                .wrap(Wrap { trim: false })
                .scroll((self.help_scroll, 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" 操作一覧 · ↑↓/PgUp/PgDn:スクロール Esc:閉じる "),
                ),
            popup,
        );
    }
}

fn on(value: bool) -> &'static str {
    if value { "表示" } else { "非表示" }
}
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn append_input(target: &mut String, input: &str) {
    let remaining = 4096_usize.saturating_sub(target.chars().count());
    target.extend(input.chars().filter(|c| !c.is_control()).take(remaining));
}
fn edit_input(target: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            target.pop();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => target.clear(),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            append_input(target, &c.to_string())
        }
        _ => {}
    }
}
fn project_choices(state: &State, query: &str) -> Vec<Option<String>> {
    let query = query.to_lowercase();
    std::iter::once(None)
        .chain(
            state
                .catalog
                .projects
                .iter()
                .filter(|p| {
                    p.cwd.to_lowercase().contains(&query) || p.name.to_lowercase().contains(&query)
                })
                .map(|p| Some(p.cwd.clone())),
        )
        .collect()
}
fn popup(area: Rect) -> Rect {
    let width = area.width.min(110);
    let height = area.height.saturating_sub(2).max(1).min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
fn draw_dialog(frame: &mut Frame<'_>, area: Rect, dialog: &Dialog, state: &State) {
    let popup = popup(area);
    frame.render_widget(Clear, popup);
    let (title, lines) = match dialog {
        Dialog::Projects { query, selected } => {
            let mut lines = vec![
                Line::raw(format!("絞り込み: {}", clean(query))),
                Line::raw("文字入力・↑↓で選択、Enterで適用、Escで閉じる"),
            ];
            let start = selected.saturating_sub(usize::from(popup.height).saturating_sub(5));
            for (i, cwd) in project_choices(state, query)
                .iter()
                .enumerate()
                .skip(start)
                .take(usize::from(popup.height).saturating_sub(4))
            {
                lines.push(Line::styled(
                    format!(
                        "{} {}",
                        if i == *selected { "›" } else { " " },
                        clean(cwd.as_deref().unwrap_or("全プロジェクト"))
                    ),
                    if i == *selected {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ));
            }
            (" プロジェクト選択 ", lines)
        }
        Dialog::Export { path, .. } => (
            " JSON を新規ファイルに保存 ",
            vec![
                Line::raw(clean(path)),
                Line::raw("Enter: 保存  Ctrl-U: 消去  Esc: 取消（既存ファイルは上書きしません）"),
            ],
        ),
        Dialog::Port(port) => (
            " MCP のポート番号 ",
            vec![
                Line::raw(clean(port)),
                Line::raw("Enter: 確定  Ctrl-U: 消去  Esc: 取消"),
            ],
        ),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        popup,
    );
}

const HELP: &str = "共通\nF1:履歴 F2:タイムライン F3:日 F4:週 F5:月 F6:統計 F7:MCP F8:設定\ns:読み込み元 p:次のプロジェクト P:プロジェクトを検索して選択 Ctrl-L:進行中のみ\nB:サイドバー表示 Shift-Enter:詳細全画面 Alt-[/Alt-]:閲覧履歴の戻る/進む\nCtrl-F:履歴検索 Ctrl-R:再読込 q/Ctrl-C:終了（検索入力中のqは文字入力） ?:この一覧\n\n履歴\nTab/Shift-Tab:ペイン移動 ↑↓/j/k:選択 Enter:次のペイン\n/:履歴検索 Esc:検索解除/戻る r:更新 Home/End/PgUp/PgDn:移動\nv:同レーンの範囲の開始/確定 a/I:選択レーンの全イベント\n\n詳細\n1:概要 2:履歴 3:ファイル 4:分析 5:再生 6:本文\n↑↓/j/k:選択・スクロール Enter:開く/展開 [ ]:レーン b:起動元 u:直前の指示\nt:種別 f:結果 o:並べ替え n:名前 /:分析検索\nSpace:再生/停止 +/-:再生速度 R:巻き戻し y:コピー Y:出力コピー e:JSON保存 c:再開コマンドコピー\nx:本文・差分展開 z:raw表示 l:行番号 d:子エージェント C:セッションID g:ログの場所\n, / .:前/次の500件 0:分析フィルタ解除 a:現在レーン全体\n\nタイムライン・カレンダー\nd/w/m:日/週/月 c:イベント/API料金 o:レーン/ディレクトリ v:範囲内のみ\na:粒度 Space:プロジェクト折畳 左右:期間移動 +/-:拡大縮小\nu:カレンダーのプロジェクト別稼働時間\nt:今日 L:最新 g:日付・範囲入力 0:全期間 Alt-1〜6:直近期間\n</>:ブロック選択 z:選択範囲に拡大 Enter:詳細 Home/End/PgUp/PgDn:縦移動\n\n統計\n[/]:7/30/90日/全期間 m/M:指標 Tab:概要/推移/時間帯/比較/モデル/プロジェクト/ツール/料金\n↑↓/j/k:項目選択 Enter:期間・プロジェクト・イベントへ移動 c:API料金\n左右/h/l:横スクロール（時間帯・料金はセル移動） Alt-左右:横スクロール\n\nMCP\nEnter:起動/停止 e:ポート番号 y:クライアント設定コピー u:URLコピー\n設定\n↑↓:設定選択 Enter/左右:変更。終了時に保存します。";

#[cfg(test)]
mod tests;
