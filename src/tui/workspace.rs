//! 画面間の選択・履歴・設定・背景要求をまとめる。各画面は共通コアの同じ版を参照する。

mod input;
mod persistence;
mod render;
use persistence::Preferences;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};

use rireki::catalog::SourceFilter;
use rireki::cost_timeline;
use rireki::detail::{DetailCache, Selection};
use rireki::model::Session;

use super::inspect::{self, Inspector};
use super::metrics;
use super::operations::{self, Generations, Job, Request};
use super::state::{Focus, Location, State};
use super::temporal;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
enum Page {
    #[default]
    History,
    Temporal,
    Statistics,
    Mcp,
    Settings,
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
    generations: Generations<u64>,
    pending: Vec<Request>,
    effects: Vec<Effect>,
    cancellations: Vec<(Job, u64)>,
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
        let prefs = Preferences::load(persistent);
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
            generations: Generations::default(),
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

    fn generation(&mut self, kind: Job) -> u64 {
        self.generations[kind] = self.generations[kind].wrapping_add(1);
        self.generations[kind]
    }
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.pending)
    }
    pub fn take_cancellations(&mut self) -> Vec<(Job, u64)> {
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
                if generation == self.generations[Job::Inspect] =>
            {
                self.cache = detail;
                self.inspect_loading = false;
                self.cache_revision = self.cache_revision.wrapping_add(1);
            }
            operations::Update::Analysis {
                generation,
                analysis: Some(analysis),
            } if generation == self.generations[Job::Analysis] => {
                self.inspector.set_analysis(analysis)
            }
            operations::Update::Metrics {
                generation,
                computed,
            } if generation == self.generations[Job::Metrics] => self.metrics.apply(*computed),
            operations::Update::Cost {
                generation,
                summary,
            } if generation == self.generations[Job::Cost] => self.temporal.apply_cost(summary),
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
            let generation = self.generation(Job::Cost);
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
        for kind in [Job::Inspect, Job::Analysis] {
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
        let generation = self.generation(Job::Inspect);
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
            let generation = self.generation(Job::Analysis);
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
        let generation = self.generation(Job::Metrics);
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
                        let generation = self.generation(Job::Analysis);
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

#[cfg(test)]
mod tests;
