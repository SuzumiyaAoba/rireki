//! 端末描画から独立した一覧・選択・キー操作。非同期結果は要求世代が一致するときだけ反映する。

mod location;
pub use location::Location;
use location::{EventKey, RowKey};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::catalog::{Catalog, SourceFilter};
use crate::detail::{DetailCache, Selection};
use crate::model::Snapshot;

use super::worker::{Request, Update};

const QUERY_LIMIT: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Sessions,
    Events,
    Detail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Session(usize),
    Event {
        session: usize,
        lane: usize,
        event: usize,
    },
}

impl Row {
    pub fn session(self) -> usize {
        match self {
            Self::Session(session) | Self::Event { session, .. } => session,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventRef {
    pub lane: usize,
    pub event: usize,
}

pub struct State {
    pub snapshot: Snapshot,
    pub revision: u64,
    pub catalog: Catalog,
    pub source: SourceFilter,
    pub live_only: bool,
    pub project: Option<String>,
    pub rows: Vec<Row>,
    pub row_selected: Option<usize>,
    pub events: Vec<EventRef>,
    pub event_selected: Option<usize>,
    pub detail: Option<DetailCache>,
    pub detail_revision: u64,
    pub detail_scroll: usize,
    pub detail_max_scroll: usize,
    pub detail_page: usize,
    pub session_page: usize,
    pub event_page: usize,
    pub focus: Focus,
    pub query: String,
    pub editing_query: bool,
    pub help: bool,
    pub loading: bool,
    pub progress: Option<(usize, usize)>,
    pub searching: bool,
    pub detail_loading: bool,
    pub search_total: usize,
    pub status: String,
    session_selected: Option<usize>,
    ever_selected: bool,
    search_generation: u64,
    detail_generation: u64,
    select_first_result: bool,
    result_selection: Option<RowKey>,
    pending_search: Option<Request>,
    pending_detail: Option<Request>,
    refresh_requested: bool,
    worker_failed: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            revision: 0,
            catalog: Catalog::default(),
            source: SourceFilter::All,
            live_only: false,
            project: None,
            rows: Vec::new(),
            row_selected: None,
            events: Vec::new(),
            event_selected: None,
            detail: None,
            detail_revision: 0,
            detail_scroll: 0,
            detail_max_scroll: 0,
            detail_page: 10,
            session_page: 10,
            event_page: 10,
            focus: Focus::Sessions,
            query: String::new(),
            editing_query: false,
            help: false,
            loading: true,
            progress: None,
            searching: false,
            detail_loading: false,
            search_total: 0,
            status: String::new(),
            session_selected: None,
            ever_selected: false,
            search_generation: 0,
            detail_generation: 0,
            select_first_result: false,
            result_selection: None,
            pending_search: None,
            pending_detail: None,
            refresh_requested: false,
            worker_failed: false,
        }
    }
}

impl State {
    pub fn selection(&self) -> Option<Selection> {
        let event = self.selected_event()?;
        Some(Selection {
            session: self.session_selected?,
            lane: event.lane,
            first: event.event,
            last: event.event,
        })
    }

    /// 別画面から対象を開く。現在の検索・絞り込みで隠れている場合も対象を表示する。
    pub fn reveal(&mut self, selection: Selection) -> bool {
        if self
            .snapshot
            .sessions
            .get(selection.session)
            .and_then(|session| session.lanes.get(selection.lane))
            .and_then(|lane| lane.events.get(selection.first..=selection.last))
            .is_none()
        {
            return false;
        }
        if !self.catalog.order.contains(&selection.session) {
            self.source = SourceFilter::All;
            self.project = None;
            self.live_only = false;
            self.catalog = Catalog::new(&self.snapshot, self.source, None);
        }
        self.query.clear();
        self.editing_query = false;
        self.session_selected = Some(selection.session);
        self.ever_selected = true;
        self.rebuild_events(None, false);
        self.event_selected = self
            .events
            .iter()
            .position(|item| item.lane == selection.lane && item.event == selection.first);
        self.update_search(false);
        self.request_detail(true);
        true
    }

    pub fn project(&mut self, project: Option<String>) {
        self.project = project;
        self.refilter();
    }

    pub fn restore_filter(&mut self, source: SourceFilter, project: Option<String>) {
        self.source = source;
        self.project = project;
        self.refilter();
    }

    pub fn toggle_live(&mut self) {
        self.live_only = !self.live_only;
        self.refilter();
    }

    pub fn refresh_live(&mut self) {
        if self.live_only {
            let before = self.catalog.order.clone();
            self.rebuild_catalog();
            if before != self.catalog.order {
                self.refilter();
            }
        }
    }

    fn rebuild_catalog(&mut self) {
        self.catalog = Catalog::new(&self.snapshot, self.source, self.project.as_deref());
        if self.live_only {
            let now = chrono::Local::now().timestamp() as f64;
            self.catalog
                .order
                .retain(|&i| now - self.snapshot.sessions[i].end < rireki::index::LIVE_SECS);
        }
    }

    pub fn selected_session(&self) -> Option<usize> {
        self.session_selected
    }

    pub fn selected_event(&self) -> Option<EventRef> {
        self.event_selected
            .and_then(|i| self.events.get(i).copied())
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        let mut requests = Vec::with_capacity(3);
        requests.extend(self.pending_search.take());
        requests.extend(self.pending_detail.take());
        if std::mem::take(&mut self.refresh_requested) {
            requests.push(Request::Refresh);
        }
        requests
    }

    pub fn apply(&mut self, update: Update) {
        // 別スレッドが停止直前に公開した結果で、失敗後の loading を再開しない。
        if self.worker_failed && !matches!(update, Update::Error(_)) {
            return;
        }
        match update {
            Update::History {
                snapshot,
                loading,
                progress,
            } => {
                self.loading = loading;
                self.progress = progress;
                if self.snapshot.sessions.len() != snapshot.sessions.len()
                    || self
                        .snapshot
                        .sessions
                        .iter()
                        .zip(&snapshot.sessions)
                        .any(|(old, new)| !std::sync::Arc::ptr_eq(old, new))
                {
                    self.replace_snapshot(snapshot);
                }
            }
            Update::Search {
                generation,
                results,
            } if generation == self.search_generation => {
                self.searching = false;
                if self.query.trim().is_empty() {
                    return;
                }
                self.search_total = results.total.saturating_add(results.sessions.len());
                self.rows = results
                    .sessions
                    .into_iter()
                    .map(Row::Session)
                    .chain(results.events.into_iter().map(|h| Row::Event {
                        session: h.session,
                        lane: h.lane,
                        event: h.event,
                    }))
                    .collect();
                self.row_selected = self
                    .result_selection
                    .as_ref()
                    .and_then(|key| key.locate(&self.snapshot))
                    .and_then(|selected| self.rows.iter().position(|&row| row == selected));
                if self.select_first_result && !self.rows.is_empty() {
                    self.select_first_result = false;
                    self.row_selected = Some(0);
                    self.activate_row();
                }
            }
            Update::Detail { generation, detail } if generation == self.detail_generation => {
                self.detail = detail.map(|detail| *detail);
                self.detail_revision = self.detail_revision.wrapping_add(1);
                self.detail_loading = false;
            }
            Update::Error(message) => {
                self.status = message;
                self.worker_failed = true;
                self.loading = false;
                self.searching = false;
                self.detail_loading = false;
                self.pending_search = None;
                self.pending_detail = None;
                self.refresh_requested = false;
            }
            _ => {}
        }
    }

    fn replace_snapshot(&mut self, snapshot: Snapshot) {
        self.revision = self.revision.wrapping_add(1);
        let old_index = self.session_selected;
        let old_session = self
            .session_selected
            .and_then(|i| self.snapshot.sessions.get(i))
            .cloned();
        let event_key = self.event_key();
        self.snapshot = snapshot;
        self.rebuild_catalog();
        self.session_selected = old_session.as_ref().and_then(|old| {
            self.catalog.order.iter().copied().find(|&i| {
                let new = &self.snapshot.sessions[i];
                old.id == new.id && old.source == new.source
            })
        });
        let initial = !self.ever_selected && self.session_selected.is_none();
        if initial {
            self.session_selected = self.catalog.order.first().copied();
            self.ever_selected = self.session_selected.is_some();
        }
        let same_version = old_session
            .as_ref()
            .zip(self.session_selected)
            .is_some_and(|(old, i)| std::sync::Arc::ptr_eq(old, &self.snapshot.sessions[i]));
        if !same_version || old_index != self.session_selected {
            self.rebuild_events(event_key.as_ref(), initial);
            self.request_detail(false);
        }
        self.update_search(self.select_first_result);
    }

    fn event_key(&self) -> Option<EventKey> {
        let session = self.snapshot.sessions.get(self.session_selected?)?;
        EventKey::capture(session, self.selected_event()?)
    }

    fn rebuild_events(&mut self, preserve: Option<&EventKey>, choose_first: bool) {
        self.events.clear();
        self.event_selected = None;
        let Some(session) = self
            .session_selected
            .and_then(|i| self.snapshot.sessions.get(i))
        else {
            return;
        };
        self.events = session
            .lanes
            .iter()
            .enumerate()
            .flat_map(|(lane, data)| {
                (0..data.events.len()).map(move |event| EventRef { lane, event })
            })
            .collect();
        self.events.sort_by(|a, b| {
            session.lanes[a.lane].events[a.event]
                .start
                .total_cmp(&session.lanes[b.lane].events[b.event].start)
                .then((a.lane, a.event).cmp(&(b.lane, b.event)))
        });
        self.event_selected = preserve
            .and_then(|key| key.locate(session))
            .and_then(|target| self.events.iter().position(|&item| item == target));
        if self.event_selected.is_none() && choose_first && !self.events.is_empty() {
            self.event_selected = Some(0);
        }
    }

    fn update_search(&mut self, select_first: bool) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.select_first_result = select_first;
        if select_first {
            self.result_selection = None;
        }
        self.search_total = 0;
        let has_query = !self.query.trim().is_empty();
        self.searching = has_query && !self.worker_failed;
        self.rows = if has_query {
            Vec::new()
        } else {
            self.catalog
                .order
                .iter()
                .copied()
                .map(Row::Session)
                .collect()
        };
        self.row_selected = self
            .rows
            .iter()
            .position(|row| Some(row.session()) == self.session_selected);
        self.pending_search = (!self.worker_failed).then(|| Request::Search {
            generation: self.search_generation,
            snapshot: self.snapshot.clone(),
            scope: self.catalog.order.clone(),
            query: self.query.clone(),
        });
    }

    fn request_detail(&mut self, reset_scroll: bool) {
        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.detail_revision = self.detail_revision.wrapping_add(1);
        self.detail = None;
        if reset_scroll {
            self.detail_scroll = 0;
            self.detail_max_scroll = 0;
        }
        let selection = self
            .session_selected
            .zip(self.selected_event())
            .map(|(session, event)| {
                (
                    Selection {
                        session,
                        lane: event.lane,
                        first: event.event,
                        last: event.event,
                    },
                    self.snapshot.sessions[session].clone(),
                )
            });
        self.detail_loading = selection.is_some() && !self.worker_failed;
        self.pending_detail = (!self.worker_failed).then_some(Request::Detail {
            generation: self.detail_generation,
            selection,
        });
    }

    fn activate_row(&mut self) {
        let Some(row) = self.row_selected.and_then(|i| self.rows.get(i)).copied() else {
            return;
        };
        self.result_selection = RowKey::capture(&self.snapshot, row);
        let changed_session = self.session_selected != Some(row.session());
        if changed_session {
            self.session_selected = Some(row.session());
            self.ever_selected = true;
            self.rebuild_events(None, true);
        }
        let old_event = self.selected_event();
        if let Row::Event { lane, event, .. } = row {
            self.event_selected = self
                .events
                .iter()
                .position(|&item| item == EventRef { lane, event });
        }
        if changed_session || old_event != self.selected_event() {
            self.request_detail(true);
        }
    }

    fn refilter(&mut self) {
        self.rebuild_catalog();
        if !self
            .session_selected
            .is_some_and(|i| self.catalog.order.contains(&i))
        {
            self.session_selected = self.catalog.order.first().copied();
            self.ever_selected |= self.session_selected.is_some();
            self.rebuild_events(None, true);
            self.request_detail(true);
        }
        self.update_search(true);
    }

    fn cycle_source(&mut self) {
        self.source = match self.source {
            SourceFilter::All => SourceFilter::Claude,
            SourceFilter::Claude => SourceFilter::Codex,
            SourceFilter::Codex => SourceFilter::All,
        };
        self.project = None;
        self.refilter();
    }

    fn cycle_project(&mut self, backward: bool) {
        let current = self
            .project
            .as_ref()
            .and_then(|cwd| self.catalog.projects.iter().position(|p| &p.cwd == cwd))
            .map_or(0, |i| i + 1);
        let len = self.catalog.projects.len() + 1;
        let next = if backward {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        self.project = next
            .checked_sub(1)
            .map(|i| self.catalog.projects[i].cwd.clone());
        self.refilter();
    }

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Sessions => {
                self.row_selected = moved(self.row_selected, self.rows.len(), delta);
                self.activate_row();
            }
            Focus::Events => {
                let next = moved(self.event_selected, self.events.len(), delta);
                if self.event_selected != next {
                    self.event_selected = next;
                    self.request_detail(true);
                }
            }
            Focus::Detail => {
                self.detail_scroll = self
                    .detail_scroll
                    .saturating_add_signed(delta)
                    .min(self.detail_max_scroll)
            }
        }
    }

    fn change_focus(&mut self, backward: bool) {
        self.focus = match (self.focus, backward) {
            (Focus::Sessions, false) | (Focus::Detail, true) => Focus::Events,
            (Focus::Events, false) | (Focus::Sessions, true) => Focus::Detail,
            _ => Focus::Sessions,
        };
    }

    /// 貼り付けは検索入力にだけ使う。端末制御文字と際限のない入力を受け付けない。
    pub fn paste(&mut self, text: &str) {
        if !self.editing_query {
            return;
        }
        let remaining = QUERY_LIMIT.saturating_sub(self.query.chars().count());
        let input: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(remaining)
            .collect();
        if !input.is_empty() {
            self.query.push_str(&input);
            self.update_search(true);
        }
    }

    /// true は終了要求。キーの release は処理せず、リピートは通常の移動として扱う。
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            return true;
        }
        if self.editing_query {
            match key.code {
                KeyCode::Esc => {
                    self.editing_query = false;
                    self.query.clear();
                    self.update_search(false);
                }
                KeyCode::Enter => {
                    self.editing_query = false;
                    self.focus = Focus::Sessions;
                }
                KeyCode::Backspace if self.query.pop().is_some() => {
                    self.update_search(true);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.query.clear();
                    self.update_search(true);
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.paste(&c.to_string())
                }
                _ => {}
            }
            return false;
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.help = false,
                KeyCode::Char('q') => return true,
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => self.editing_query = true,
            KeyCode::Char('s') => self.cycle_source(),
            KeyCode::Char('p') => self.cycle_project(false),
            KeyCode::Char('P') => self.cycle_project(true),
            KeyCode::Char('r') if !self.worker_failed => {
                self.refresh_requested = true;
                self.loading = true;
            }
            KeyCode::Tab => self.change_focus(false),
            KeyCode::BackTab => self.change_focus(true),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.change_focus(false),
            KeyCode::Left | KeyCode::Char('h') => self.change_focus(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp | KeyCode::PageDown => {
                let page = match self.focus {
                    Focus::Sessions => self.session_page,
                    Focus::Events => self.event_page,
                    Focus::Detail => self.detail_page,
                }
                .max(1);
                self.move_selection(if key.code == KeyCode::PageUp {
                    -(page as isize)
                } else {
                    page as isize
                });
            }
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(isize::MAX),
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.update_search(false);
            }
            KeyCode::Esc => {
                self.focus = match self.focus {
                    Focus::Detail => Focus::Events,
                    _ => Focus::Sessions,
                }
            }
            _ => {}
        }
        false
    }
}

fn moved(current: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if delta == isize::MIN {
        return Some(0);
    }
    if delta == isize::MAX {
        return Some(len - 1);
    }
    Some(match current {
        Some(current) => current.saturating_add_signed(delta).min(len - 1),
        None if delta < 0 => len - 1,
        None => 0,
    })
}

#[cfg(test)]
mod tests;
