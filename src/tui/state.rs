//! 端末描画から独立した一覧・選択・キー操作。非同期結果は要求世代が一致するときだけ反映する。

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::catalog::{Catalog, SourceFilter};
use crate::detail::{DetailCache, Selection};
use crate::model::{Event, Kind, Session, Snapshot, Source};

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
                KeyCode::Backspace => {
                    if self.query.pop().is_some() {
                        self.update_search(true);
                    }
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
            KeyCode::Char('r') => {
                if !self.worker_failed {
                    self.refresh_requested = true;
                    self.loading = true;
                }
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

/// 添字ではなくログの識別子で保持する閲覧履歴。削除された項目は解決しない。
#[derive(Clone)]
pub struct Location {
    session: String,
    source: Source,
    first: EventKey,
    last: EventKey,
}

impl Location {
    pub fn capture(snapshot: &Snapshot, selection: Selection) -> Option<Self> {
        let session = snapshot.sessions.get(selection.session)?;
        Some(Self {
            session: session.id.clone(),
            source: session.source,
            first: EventKey::capture(
                session,
                EventRef {
                    lane: selection.lane,
                    event: selection.first,
                },
            )?,
            last: EventKey::capture(
                session,
                EventRef {
                    lane: selection.lane,
                    event: selection.last,
                },
            )?,
        })
    }
    pub fn resolve(&self, snapshot: &Snapshot) -> Option<Selection> {
        let index = snapshot
            .sessions
            .iter()
            .position(|s| s.id == self.session && s.source == self.source)?;
        let first = self.first.locate(&snapshot.sessions[index])?;
        let last = self.last.locate(&snapshot.sessions[index])?;
        (first.lane == last.lane).then_some(Selection {
            session: index,
            lane: first.lane,
            first: first.event.min(last.event),
            last: first.event.max(last.event),
        })
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

/// 検索結果のセッション行とイベント行を区別して、更新後も同じ行を選択する。
struct RowKey {
    session: String,
    source: Source,
    event: Option<EventKey>,
}

impl RowKey {
    fn capture(snapshot: &Snapshot, row: Row) -> Option<Self> {
        let session = snapshot.sessions.get(row.session())?;
        Some(Self {
            session: session.id.clone(),
            source: session.source,
            event: match row {
                Row::Session(_) => None,
                Row::Event { lane, event, .. } => {
                    Some(EventKey::capture(session, EventRef { lane, event })?)
                }
            },
        })
    }

    fn locate(&self, snapshot: &Snapshot) -> Option<Row> {
        let session = snapshot
            .sessions
            .iter()
            .position(|session| session.id == self.session && session.source == self.source)?;
        match &self.event {
            None => Some(Row::Session(session)),
            Some(key) => {
                let event = key.locate(&snapshot.sessions[session])?;
                Some(Row::Event {
                    session,
                    lane: event.lane,
                    event: event.event,
                })
            }
        }
    }
}

/// 添字は更新で移動するため、レーンとイベントの不変属性で選択を引き継ぐ。
#[derive(Clone)]
struct EventKey {
    origin: String,
    lane_name: String,
    spawned_by: Option<String>,
    start: u64,
    kind: Kind,
    name: String,
    call_id: Option<String>,
    occurrence: usize,
}

impl EventKey {
    fn capture(session: &Session, reference: EventRef) -> Option<Self> {
        let lane = session.lanes.get(reference.lane)?;
        let event = lane.events.get(reference.event)?;
        let mut key = Self {
            origin: lane.origin.clone(),
            lane_name: lane.name.clone(),
            spawned_by: lane.spawned_by.clone(),
            start: event.start.to_bits(),
            kind: event.kind,
            name: event.name.to_string(),
            call_id: event.call_id.as_deref().map(str::to_string),
            occurrence: 0,
        };
        key.occurrence = lane.events[..reference.event]
            .iter()
            .filter(|event| key.matches(event))
            .count();
        Some(key)
    }

    fn matches(&self, event: &Event) -> bool {
        if let Some(id) = &self.call_id {
            event.call_id.as_deref() == Some(id.as_str())
        } else {
            event.call_id.is_none()
                && event.start.to_bits() == self.start
                && event.kind == self.kind
                && event.name.as_ref() == self.name
        }
    }

    fn locate(&self, session: &Session) -> Option<EventRef> {
        let lane = session.lanes.iter().position(|lane| {
            lane.origin == self.origin
                && lane.name == self.lane_name
                && lane.spawned_by == self.spawned_by
        })?;
        let event = session.lanes[lane]
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| self.matches(event))
            .nth(self.occurrence)?
            .0;
        Some(EventRef { lane, event })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Lane, Status};
    use std::sync::Arc;

    fn event(start: f64, summary: &str) -> Event {
        Event {
            start,
            end: start,
            kind: Kind::User,
            name: "User".into(),
            summary: summary.into(),
            input: summary.into(),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
            usage: Default::default(),
            model: None,
            status: Status::Ok,
        }
    }

    fn session(id: &str, source: Source, cwd: &str, events: Vec<Event>) -> Arc<Session> {
        let mut lane = Lane::new("main");
        lane.origin = format!("{id}.jsonl");
        lane.events = events;
        let mut session = Session {
            id: id.into(),
            source,
            cwd: cwd.into(),
            headless: false,
            title: id.into(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: vec![],
            tree: vec![],
        };
        session.finalize();
        Arc::new(session)
    }

    fn publish(state: &mut State, sessions: Vec<Arc<Session>>) {
        state.apply(Update::History {
            snapshot: Snapshot { sessions },
            loading: false,
            progress: None,
        });
    }

    fn key(state: &mut State, code: KeyCode) -> bool {
        state.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn all_lanes_are_chronological_and_selection_survives_insertions_and_lane_reordering() {
        let mut main = session("one", Source::Claude, "/one", vec![event(3.0, "main")]);
        let mut child = Lane::new("child");
        child.origin = "child.jsonl".into();
        child.events = vec![event(1.0, "first"), event(4.0, "selected")];
        Arc::make_mut(&mut main).lanes.push(child);
        Arc::make_mut(&mut main).finalize();
        let mut state = State::default();
        publish(&mut state, vec![main.clone()]);
        assert_eq!(
            state.events,
            [
                EventRef { lane: 1, event: 0 },
                EventRef { lane: 0, event: 0 },
                EventRef { lane: 1, event: 1 }
            ]
        );
        state.focus = Focus::Events;
        key(&mut state, KeyCode::End);
        let mut updated = (*main).clone();
        updated.lanes[1].events.insert(1, event(2.0, "inserted"));
        updated.lanes.swap(0, 1);
        updated.finalize();
        publish(
            &mut state,
            vec![
                session("newest", Source::Codex, "/two", vec![event(10.0, "new")]),
                Arc::new(updated),
            ],
        );
        assert_eq!(state.selected_session(), Some(1));
        assert_eq!(state.selected_event(), Some(EventRef { lane: 0, event: 2 }));
        assert_eq!(state.row_selected, Some(1));
        // 選択したイベントが消えた場合、隣のイベントへ勝手に移動しない。
        let mut removed = (*state.snapshot.sessions[1]).clone();
        removed.lanes[0].events.pop();
        removed.finalize();
        let newest = state.snapshot.sessions[0].clone();
        publish(&mut state, vec![newest, Arc::new(removed)]);
        assert_eq!(state.selected_event(), None);
        assert!(state.detail.is_none());
    }

    #[test]
    fn removed_session_does_not_select_a_different_session() {
        let mut state = State::default();
        publish(
            &mut state,
            vec![
                session(
                    "selected",
                    Source::Claude,
                    "/one",
                    vec![event(2.0, "chosen")],
                ),
                session("other", Source::Codex, "/two", vec![event(1.0, "other")]),
            ],
        );
        let other = state.snapshot.sessions[1].clone();
        publish(&mut state, vec![other]);
        assert_eq!(state.selected_session(), None);
        assert_eq!(state.row_selected, None);
        key(&mut state, KeyCode::Down);
        assert_eq!(state.selected_session(), Some(0));
    }

    #[test]
    fn search_editing_handles_unicode_paste_escape_and_stale_results() {
        let mut state = State::default();
        publish(
            &mut state,
            vec![session(
                "検索",
                Source::Claude,
                "/one",
                vec![event(1.0, "日本語")],
            )],
        );
        key(&mut state, KeyCode::Char('/'));
        state.paste("日本\n語\u{1b}qsp");
        assert_eq!(state.query, "日本語qsp");
        assert_eq!(state.source, SourceFilter::All);
        let stale = state.search_generation;
        key(&mut state, KeyCode::Backspace);
        assert_eq!(state.query, "日本語qs");
        state.apply(Update::Search {
            generation: stale,
            results: crate::search::Results {
                sessions: vec![0],
                ..Default::default()
            },
        });
        assert!(state.rows.is_empty());
        assert!(state.searching);
        state.paste(&"あ".repeat(QUERY_LIMIT * 2));
        assert_eq!(state.query.chars().count(), QUERY_LIMIT);
        assert!(state.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        key(&mut state, KeyCode::Esc);
        assert!(!state.editing_query);
        assert!(state.query.is_empty());
        assert_eq!(state.rows, [Row::Session(0)]);
        state.apply(Update::Search {
            generation: stale,
            results: crate::search::Results::default(),
        });
        assert_eq!(state.rows, [Row::Session(0)]);
    }

    #[test]
    fn source_and_project_filters_scope_search_requests() {
        let mut state = State::default();
        publish(
            &mut state,
            vec![
                session("claude", Source::Claude, "/one", vec![event(1.0, "a")]),
                session("codex", Source::Codex, "/two", vec![event(2.0, "b")]),
            ],
        );
        key(&mut state, KeyCode::Char('p'));
        assert_eq!(state.project.as_deref(), Some("/two"));
        assert_eq!(state.rows, [Row::Session(1)]);
        key(&mut state, KeyCode::Char('s'));
        assert_eq!(state.source, SourceFilter::Claude);
        assert_eq!(state.project, None);
        assert_eq!(state.rows, [Row::Session(0)]);
        key(&mut state, KeyCode::Char('/'));
        state.paste("a");
        let request = state
            .take_requests()
            .into_iter()
            .find(|r| matches!(r, Request::Search { .. }))
            .unwrap();
        let Request::Search { scope, .. } = request else {
            unreachable!()
        };
        assert_eq!(scope, [0]);
    }

    #[test]
    fn outdated_detail_cannot_replace_new_selection() {
        let mut state = State::default();
        publish(
            &mut state,
            vec![session(
                "one",
                Source::Claude,
                "/one",
                vec![event(1.0, "first"), event(2.0, "second")],
            )],
        );
        let stale = state.detail_generation;
        let detail = crate::detail::DetailLoader::default().load_session(
            Selection {
                session: 0,
                lane: 0,
                first: 0,
                last: 0,
            },
            &state.snapshot.sessions[0],
            || true,
        );
        state.focus = Focus::Events;
        key(&mut state, KeyCode::Down);
        state.apply(Update::Detail {
            generation: stale,
            detail: detail.map(Box::new),
        });
        assert!(state.detail.is_none());
        assert!(state.detail_loading);
        assert_eq!(state.selected_event(), Some(EventRef { lane: 0, event: 1 }));
        let detail = crate::detail::DetailLoader::default().load_session(
            Selection {
                session: 0,
                lane: 0,
                first: 1,
                last: 1,
            },
            &state.snapshot.sessions[0],
            || true,
        );
        state.apply(Update::Detail {
            generation: state.detail_generation,
            detail: detail.map(Box::new),
        });
        assert_eq!(state.detail.as_ref().unwrap().texts[0].0, "second");
    }

    #[test]
    fn search_lists_session_and_event_hits_and_opens_the_matching_event() {
        let mut state = State::default();
        publish(
            &mut state,
            vec![
                session(
                    "needle title",
                    Source::Claude,
                    "/one",
                    vec![event(1.0, "metadata")],
                ),
                session(
                    "other",
                    Source::Codex,
                    "/two",
                    vec![event(2.0, "first"), event(3.0, "needle event")],
                ),
            ],
        );
        key(&mut state, KeyCode::Char('/'));
        state.paste("needle");
        let mut index = crate::search::Index::default();
        let results = index.search(
            &state.snapshot.sessions,
            &state.catalog.order,
            &state.query,
            500,
        );
        state.apply(Update::Search {
            generation: state.search_generation,
            results,
        });
        assert_eq!(state.search_total, 2);
        assert_eq!(
            state.rows,
            [
                Row::Session(0),
                Row::Event {
                    session: 1,
                    lane: 0,
                    event: 1
                }
            ]
        );
        key(&mut state, KeyCode::Enter);
        key(&mut state, KeyCode::Down);
        assert_eq!(state.selected_session(), Some(1));
        assert_eq!(state.selected_event(), Some(EventRef { lane: 0, event: 1 }));
        key(&mut state, KeyCode::Enter);
        assert_eq!(state.focus, Focus::Events);
        let requests = state.take_requests();
        assert!(requests.iter().any(|request| matches!(
            request,
            Request::Detail {
                selection: Some((
                    Selection {
                        session: 1,
                        lane: 0,
                        first: 1,
                        last: 1
                    },
                    _
                )),
                ..
            }
        )));
    }

    #[test]
    fn worker_failure_stops_loading_and_does_not_queue_more_work() {
        let mut state = State::default();
        state.apply(Update::Error("worker stopped".into()));
        // 停止と競合して既に公開されていた別 worker の結果も反映しない。
        state.apply(Update::History {
            snapshot: Snapshot::default(),
            loading: true,
            progress: Some((0, 10)),
        });
        key(&mut state, KeyCode::Char('/'));
        state.paste("query");
        key(&mut state, KeyCode::Enter);
        key(&mut state, KeyCode::Char('r'));
        assert!(!state.loading);
        assert!(!state.searching);
        assert!(!state.detail_loading);
        assert!(state.take_requests().is_empty());
        assert_eq!(state.status, "worker stopped");
    }

    #[test]
    fn live_update_preserves_an_event_hit_when_the_session_title_also_matches() {
        let mut state = State::default();
        publish(
            &mut state,
            vec![session(
                "needle title",
                Source::Claude,
                "/one",
                vec![event(2.0, "first"), event(3.0, "needle selected")],
            )],
        );
        key(&mut state, KeyCode::Char('/'));
        state.paste("needle");
        let mut index = crate::search::Index::default();
        let results = index.search(
            &state.snapshot.sessions,
            &state.catalog.order,
            &state.query,
            500,
        );
        state.apply(Update::Search {
            generation: state.search_generation,
            results,
        });
        key(&mut state, KeyCode::Enter);
        key(&mut state, KeyCode::Down);
        assert_eq!(
            state.rows[state.row_selected.unwrap()],
            Row::Event {
                session: 0,
                lane: 0,
                event: 1
            }
        );

        let mut updated = (*state.snapshot.sessions[0]).clone();
        updated.lanes[0].events.insert(0, event(1.0, "inserted"));
        updated.finalize();
        publish(
            &mut state,
            vec![
                session("newest", Source::Codex, "/two", vec![event(10.0, "new")]),
                Arc::new(updated),
            ],
        );
        let results = index.search(
            &state.snapshot.sessions,
            &state.catalog.order,
            &state.query,
            500,
        );
        state.apply(Update::Search {
            generation: state.search_generation,
            results,
        });
        assert_eq!(
            state.rows[state.row_selected.unwrap()],
            Row::Event {
                session: 1,
                lane: 0,
                event: 2
            }
        );
        assert_eq!(state.selected_session(), Some(1));
        assert_eq!(state.selected_event(), Some(EventRef { lane: 0, event: 2 }));
    }

    #[test]
    fn live_update_keeps_the_pending_first_search_selection() {
        let mut state = State::default();
        let target = session(
            "needle title",
            Source::Claude,
            "/one",
            vec![event(1.0, "target")],
        );
        publish(
            &mut state,
            vec![
                target.clone(),
                session(
                    "unrelated",
                    Source::Codex,
                    "/two",
                    vec![event(10.0, "selected initially")],
                ),
            ],
        );
        assert_eq!(state.selected_session(), Some(1));
        key(&mut state, KeyCode::Char('/'));
        state.paste("needle");
        let stale_generation = state.search_generation;
        let mut index = crate::search::Index::default();
        let stale_results = index.search(
            &state.snapshot.sessions,
            &state.catalog.order,
            &state.query,
            500,
        );
        publish(
            &mut state,
            vec![
                target,
                session(
                    "unrelated",
                    Source::Codex,
                    "/two",
                    vec![event(10.0, "selected initially"), event(11.0, "appended")],
                ),
            ],
        );
        state.apply(Update::Search {
            generation: stale_generation,
            results: stale_results,
        });
        assert!(state.rows.is_empty());
        assert!(state.searching);
        let results = index.search(
            &state.snapshot.sessions,
            &state.catalog.order,
            &state.query,
            500,
        );
        state.apply(Update::Search {
            generation: state.search_generation,
            results,
        });
        assert_eq!(state.rows, [Row::Session(0)]);
        assert_eq!(state.row_selected, Some(0));
        assert_eq!(state.selected_session(), Some(0));
    }

    #[test]
    fn detail_refresh_keeps_scroll_bounds_until_the_new_content_arrives() {
        let mut state = State::default();
        publish(
            &mut state,
            vec![session(
                "one",
                Source::Claude,
                "/one",
                vec![event(1.0, "selected")],
            )],
        );
        state.focus = Focus::Detail;
        state.detail_scroll = 30;
        state.detail_max_scroll = 100;
        publish(
            &mut state,
            vec![session(
                "one",
                Source::Claude,
                "/one",
                vec![event(1.0, "selected"), event(2.0, "appended")],
            )],
        );
        assert!(state.detail_loading);
        assert_eq!(state.detail_max_scroll, 100);
        key(&mut state, KeyCode::Down);
        assert_eq!(state.detail_scroll, 31);
        state.focus = Focus::Events;
        key(&mut state, KeyCode::Down);
        assert_eq!(state.detail_scroll, 0);
        assert_eq!(state.detail_max_scroll, 0);
    }
}
