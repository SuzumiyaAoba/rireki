//! 詳細パネルの表示・操作。履歴の取得、分析、クリップボードやファイル操作は呼び出し元へ返す。
//!
//! `Context` の selection/cache は同じ世代を渡す。対象変更時は `selection_changed`、
//! 分析結果到着時は `set_analysis` を呼ぶ。`Load` は詳細パネルの範囲変更、`Select` は
//! 履歴に残す通常のイベント選択で、どちらも呼び出し元が DetailLoader を背景で動かす。
//! `Analyze` の結果は呼び出し元で要求世代を照合してから反映する。

mod render;
mod rich;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde::{Deserialize, Serialize};

use rireki::detail::analysis::{Analysis, Filter, KindFilter, Order, StatusFilter};
use rireki::detail::replay::{Playback, SPEEDS};
use rireki::detail::{DetailCache, Selection};
use rireki::model::{Event, Kind, Session, Source};

#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub session: &'a Arc<Session>,
    pub session_index: usize,
    pub selection: Selection,
    pub cache: Option<&'a DetailCache>,
    pub revision: u64,
}

impl<'a> Context<'a> {
    fn events(self) -> &'a [Event] {
        self.session
            .lanes
            .get(self.selection.lane)
            .and_then(|lane| lane.events.get(self.selection.first..=self.selection.last))
            .unwrap_or(&[])
    }

    fn event(self, lane: usize, event: usize) -> Selection {
        Selection {
            session: self.session_index,
            lane,
            first: event,
            last: event,
        }
    }

    fn lane(self, lane: usize) -> Option<Selection> {
        let last = self.session.lanes.get(lane)?.events.len().checked_sub(1)?;
        Some(Selection {
            session: self.session_index,
            lane,
            first: 0,
            last,
        })
    }

    fn cache(self) -> Option<&'a DetailCache> {
        self.cache.filter(|cache| cache.sel == self.selection)
    }
}

pub enum Action {
    /// パネルの範囲を変更し、背景で本文と分析を再取得する。
    Load(Selection),
    /// 通常の選択として移動する。戻る操作で元の範囲へ戻れる。
    Select(Selection),
    Analyze {
        selection: Selection,
        filter: Filter,
        order: Order,
    },
    Copy(String),
    /// JSON の書き出し。保存先の指定はフロントエンド共通の操作で処理する。
    Export(String),
    /// 記録元のログをファイルマネージャーで示す。
    Reveal(String),
    Status(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    #[default]
    Overview,
    History,
    Files,
    Analysis,
    Replay,
    Body,
}

impl Tab {
    const ALL: [Self; 6] = [
        Self::Overview,
        Self::History,
        Self::Files,
        Self::Analysis,
        Self::Replay,
        Self::Body,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Overview => "概要",
            Self::History => "履歴",
            Self::Files => "変更",
            Self::Analysis => "分析",
            Self::Replay => "再生",
            Self::Body => "本文",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub tab: Tab,
    pub line_numbers: bool,
    pub raw: bool,
    /// 0: 古い順、1: 新しい順、2: 所要時間、3: トークン数。
    pub order: u8,
    pub speed_index: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab: Tab::Overview,
            line_numbers: true,
            raw: false,
            order: 0,
            speed_index: 2,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Target {
    #[default]
    None,
    Event {
        lane: usize,
        event: usize,
    },
    Lane(usize),
    File(usize),
}

struct Entry {
    line: Line<'static>,
    target: Target,
}

impl Entry {
    fn text(text: impl Into<String>) -> Self {
        Self {
            line: rich::plain(text),
            target: Target::None,
        }
    }
    fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            line: rich::plain(text).style(style),
            target: Target::None,
        }
    }
    fn target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }
}

#[derive(PartialEq)]
struct DocumentKey {
    session: usize,
    selection: Selection,
    revision: u64,
    width: u16,
    replay_visible: Option<usize>,
}

pub struct Inspector {
    pub tab: Tab,
    filter: Filter,
    order: Order,
    analysis: Option<Arc<Analysis>>,
    editing: bool,
    original_query: String,
    line_numbers: bool,
    raw: bool,
    expanded_body: bool,
    open_files: HashSet<String>,
    all_diffs: bool,
    playback: Option<Playback>,
    playback_context: Option<(usize, Selection, u64)>,
    playback_start: f64,
    playback_end: f64,
    playback_offset: f64,
    speed_index: usize,
    follow_replay: bool,
    cursor: usize,
    event_page: usize,
    event_pages: usize,
    scroll: usize,
    page: usize,
    width: u16,
    rows: Vec<Entry>,
    key: Option<DocumentKey>,
    dirty: bool,
}

impl Default for Inspector {
    fn default() -> Self {
        Self {
            tab: Tab::Overview,
            filter: Filter::default(),
            order: Order::Oldest,
            analysis: None,
            editing: false,
            original_query: String::new(),
            line_numbers: true,
            raw: false,
            expanded_body: false,
            open_files: HashSet::new(),
            all_diffs: false,
            playback: None,
            playback_context: None,
            playback_start: 0.0,
            playback_end: 0.0,
            playback_offset: 0.0,
            speed_index: 2,
            follow_replay: true,
            cursor: 0,
            event_page: 0,
            event_pages: 1,
            scroll: 0,
            page: 10,
            width: 80,
            rows: Vec::new(),
            key: None,
            dirty: true,
        }
    }
}

impl Inspector {
    pub fn set_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.cursor = 0;
            self.event_page = 0;
            self.scroll = 0;
            self.dirty = true;
        }
        if tab != Tab::Replay {
            self.pause();
        }
    }

    pub fn pause(&mut self) {
        if let Some(playback) = self.playback.as_mut() {
            playback.pause(Instant::now());
        }
    }

    pub fn settings(&self) -> Settings {
        Settings {
            tab: self.tab,
            line_numbers: self.line_numbers,
            raw: self.raw,
            order: match self.order {
                Order::Oldest => 0,
                Order::Newest => 1,
                Order::Duration => 2,
                Order::Tokens => 3,
            },
            speed_index: self.speed_index,
        }
    }

    pub fn apply_settings(&mut self, settings: Settings) {
        self.tab = settings.tab;
        self.line_numbers = settings.line_numbers;
        self.raw = settings.raw;
        self.order = match settings.order {
            1 => Order::Newest,
            2 => Order::Duration,
            3 => Order::Tokens,
            _ => Order::Oldest,
        };
        self.speed_index = settings.speed_index.min(SPEEDS.len() - 1);
        self.dirty = true;
    }

    pub fn selection_changed(&mut self) {
        self.invalidate_analysis();
        if self.editing {
            self.filter.query = std::mem::take(&mut self.original_query);
            self.editing = false;
        }
        self.playback = None;
        self.playback_context = None;
        self.playback_offset = 0.0;
        self.cursor = 0;
        self.event_page = 0;
        self.scroll = 0;
        self.open_files.clear();
        self.expanded_body = false;
        self.follow_replay = true;
        self.key = None;
        self.dirty = true;
    }

    /// ログの版だけが変わった時はスクロール・検索入力・再生位置を保ち、集計だけ失効する。
    pub fn invalidate_analysis(&mut self) {
        self.analysis = None;
        self.dirty = true;
    }

    pub fn analysis_request(&self, selection: Selection) -> Action {
        let mut filter = self.filter.clone();
        if self.editing {
            filter.query = self.original_query.clone();
        }
        Action::Analyze {
            selection,
            filter,
            order: self.order,
        }
    }

    pub fn set_analysis(&mut self, analysis: Arc<Analysis>) {
        self.analysis = Some(analysis);
        self.dirty = true;
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn paste(&mut self, text: &str) {
        if self.editing {
            let remaining = 1024_usize.saturating_sub(self.filter.query.chars().count());
            self.filter
                .query
                .extend(text.chars().filter(|c| !c.is_control()).take(remaining));
            self.dirty = true;
        }
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(playback) = self
            .playback
            .as_mut()
            .filter(|playback| playback.is_playing())
        else {
            return false;
        };
        playback.tick(now);
        true
    }

    fn ensure_playback(&mut self, context: Context<'_>) {
        let key = (
            Arc::as_ptr(context.session) as usize,
            context.selection,
            context.revision,
        );
        if self.playback.is_some() && self.playback_context == Some(key) {
            return;
        }
        let events = context.events();
        let start = events
            .first()
            .map_or(0.0, |e| if e.start.is_finite() { e.start } else { 0.0 });
        let end = context.cache().map_or_else(
            || {
                events
                    .iter()
                    .map(|e| e.end)
                    .filter(|v| v.is_finite())
                    .fold(start, f64::max)
            },
            DetailCache::end,
        );
        let end = if end.is_finite() {
            end.max(start)
        } else {
            start
        };
        self.playback_context = Some(key);
        if let Some(playback) = self.playback.as_mut() {
            if self.playback_start == start && self.playback_end == end {
                return;
            }
            // 更新で選択範囲の末尾が伸び縮みしても、再生中の絶対時刻を保つ。
            let now = Instant::now();
            playback.tick(now);
            let at = playback.timestamp().clamp(start, end);
            let playing = playback.is_playing();
            let mut next = Playback::new(at, end, SPEEDS[self.speed_index]);
            if playing {
                next.play(now);
            }
            self.playback_offset = at - start;
            self.playback = Some(next);
        } else {
            self.playback_offset = 0.0;
            self.playback = Some(Playback::new(start, end, SPEEDS[self.speed_index]));
        }
        self.playback_start = start;
        self.playback_end = end;
    }

    fn rewind(&mut self, context: Context<'_>) {
        let playing = self.playback.as_ref().is_some_and(Playback::is_playing);
        self.playback = None;
        self.playback_context = None;
        self.ensure_playback(context);
        if playing {
            self.playback.as_mut().unwrap().play(Instant::now());
        }
        self.cursor = 0;
        self.scroll = 0;
        self.dirty = true;
    }

    pub fn key(&mut self, key: KeyEvent, context: Context<'_>) -> Vec<Action> {
        if self.editing {
            match key.code {
                KeyCode::Esc => {
                    self.filter.query = std::mem::take(&mut self.original_query);
                    self.editing = false;
                }
                KeyCode::Enter => {
                    self.editing = false;
                    self.analysis = None;
                    self.cursor = 0;
                    self.dirty = true;
                    return vec![self.analysis_request(context.selection)];
                }
                KeyCode::Backspace => {
                    self.filter.query.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.filter.query.clear()
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && self.filter.query.chars().count() < 1024
                        && !c.is_control() =>
                {
                    self.filter.query.push(c);
                }
                _ => {}
            }
            self.dirty = true;
            return Vec::new();
        }
        self.ensure(context, self.width);
        let mut actions = Vec::new();
        match key.code {
            KeyCode::Char(c @ '1'..='6') => {
                self.set_tab(Tab::ALL[(c as u8 - b'1') as usize]);
                if self.tab == Tab::Replay {
                    self.ensure_playback(context);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-(self.page as isize)),
            KeyCode::PageDown => self.move_by(self.page as isize),
            KeyCode::Home => {
                self.cursor = 0;
                self.follow_replay = false;
            }
            KeyCode::End => {
                self.cursor = self.rows.len().saturating_sub(1);
                self.follow_replay = true;
            }
            KeyCode::Enter => match self
                .rows
                .get(self.cursor)
                .map(|entry| entry.target)
                .unwrap_or_default()
            {
                Target::Event { lane, event } => {
                    actions.push(Action::Select(context.event(lane, event)))
                }
                Target::Lane(lane) => {
                    if let Some(selection) = context.lane(lane) {
                        actions.push(Action::Select(selection));
                    }
                }
                Target::File(file) => {
                    if let Some(path) = context
                        .cache()
                        .and_then(|cache| cache.files.get(file))
                        .map(|file| file.path.clone())
                    {
                        if !self.open_files.remove(&path) {
                            self.open_files.insert(path);
                        }
                        self.dirty = true;
                    }
                }
                Target::None => {}
            },
            KeyCode::Char('/') => {
                self.original_query = self.filter.query.clone();
                self.editing = true;
                self.tab = Tab::Analysis;
                self.dirty = true;
            }
            KeyCode::Char('t') => {
                self.filter.kind = match self.filter.kind {
                    KindFilter::All => KindFilter::Tools,
                    KindFilter::Tools => KindFilter::Messages,
                    KindFilter::Messages => KindFilter::All,
                };
                self.changed_filter(context, &mut actions);
            }
            KeyCode::Char('f') => {
                self.filter.status = match self.filter.status {
                    StatusFilter::All => StatusFilter::Failed,
                    StatusFilter::Failed => StatusFilter::Pending,
                    StatusFilter::Pending => StatusFilter::All,
                };
                self.changed_filter(context, &mut actions);
            }
            KeyCode::Char('n') => {
                let names = context.cache().map(|cache| &cache.selection_summary.names);
                if let Some(names) = names {
                    let next = self
                        .filter
                        .name
                        .as_ref()
                        .and_then(|name| names.iter().position(|(value, _, _)| value == name))
                        .map_or(0, |at| at + 1);
                    self.filter.name = names.get(next).map(|(name, _, _)| name.clone());
                }
                self.changed_filter(context, &mut actions);
            }
            KeyCode::Char('o') => {
                self.order = match self.order {
                    Order::Oldest => Order::Newest,
                    Order::Newest => Order::Duration,
                    Order::Duration => Order::Tokens,
                    Order::Tokens => Order::Oldest,
                };
                self.changed_filter(context, &mut actions);
            }
            KeyCode::Char('0') => {
                self.filter = Filter::default();
                self.changed_filter(context, &mut actions);
            }
            KeyCode::Char(',' | '.') => {
                self.event_page = if key.code == KeyCode::Char('.') {
                    (self.event_page + 1).min(self.event_pages.saturating_sub(1))
                } else {
                    self.event_page.saturating_sub(1)
                };
                self.cursor = 0;
                self.scroll = 0;
                self.follow_replay = false;
                self.dirty = true;
            }
            KeyCode::Char('x') => {
                self.expanded_body = !self.expanded_body;
                self.all_diffs = !self.all_diffs;
                self.dirty = true;
            }
            KeyCode::Char('z') => {
                self.raw = !self.raw;
                self.dirty = true;
            }
            KeyCode::Char('l') => {
                self.line_numbers = !self.line_numbers;
                self.dirty = true;
            }
            KeyCode::Char(' ') if self.tab == Tab::Replay => {
                self.ensure_playback(context);
                if self.playback.as_ref().is_some_and(Playback::is_finished) {
                    self.rewind(context);
                }
                let playback = self.playback.as_mut().unwrap();
                if playback.is_playing() {
                    playback.pause(Instant::now());
                } else {
                    playback.play(Instant::now());
                }
                self.dirty = true;
            }
            KeyCode::Char('+' | '=') if self.tab == Tab::Replay => self.change_speed(true),
            KeyCode::Char('-') if self.tab == Tab::Replay => self.change_speed(false),
            KeyCode::Char('R') if self.tab == Tab::Replay => {
                self.rewind(context);
            }
            KeyCode::Char('[' | ']') => {
                let len = context.session.lanes.len();
                if len > 0 {
                    let direction = if key.code == KeyCode::Char(']') {
                        1
                    } else {
                        len - 1
                    };
                    let mut lane = context.selection.lane;
                    for _ in 0..len {
                        lane = (lane + direction) % len;
                        if let Some(selection) = context.lane(lane) {
                            actions.push(Action::Load(selection));
                            break;
                        }
                    }
                }
            }
            KeyCode::Char('a') => {
                if let Some(selection) = context.lane(context.selection.lane) {
                    actions.push(Action::Load(selection));
                }
            }
            KeyCode::Char('u') => {
                if let Some(lane) = context.session.lanes.get(context.selection.lane)
                    && let Some((event, _)) = lane
                        .events
                        .iter()
                        .enumerate()
                        .take(context.selection.first)
                        .rev()
                        .find(|(_, e)| e.kind == Kind::User)
                {
                    actions.push(Action::Select(context.event(context.selection.lane, event)));
                }
            }
            KeyCode::Char('b') => {
                if let Some(call) = context
                    .session
                    .lanes
                    .get(context.selection.lane)
                    .and_then(|lane| lane.spawned_by.as_deref())
                {
                    'outer: for (lane, events) in context.session.lanes.iter().enumerate() {
                        if let Some(event) = events
                            .events
                            .iter()
                            .position(|e| e.call_id.as_deref() == Some(call))
                        {
                            actions.push(Action::Select(context.event(lane, event)));
                            break 'outer;
                        }
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(event) = context.events().first()
                    && let Some(lane) = event
                        .call_id
                        .as_deref()
                        .and_then(|call| context.session.lane_spawned_by(call))
                    && let Some(selection) = context.lane(lane)
                {
                    actions.push(Action::Select(selection));
                }
            }
            KeyCode::Char('c') => actions.push(Action::Copy(resume_command(context.session))),
            KeyCode::Char('C') => actions.push(Action::Copy(context.session.id.clone())),
            KeyCode::Char('g') => {
                if let Some(lane) = context
                    .session
                    .lanes
                    .get(context.selection.lane)
                    .filter(|lane| !lane.origin.is_empty())
                {
                    actions.push(Action::Reveal(lane.origin.clone()));
                }
            }
            KeyCode::Char('e') => match self.export(context) {
                Some(json) => actions.push(Action::Export(json)),
                None => actions.push(Action::Status("分析結果を読み込んでいます".into())),
            },
            KeyCode::Char('y') => {
                let text = if self.tab == Tab::Analysis {
                    self.export(context)
                } else if self.tab == Tab::Body {
                    context.cache().and_then(|cache| {
                        if self.raw {
                            cache
                                .raws
                                .first()
                                .and_then(|raw| raw.clone())
                                .or_else(|| cache.texts.first().map(|text| text.0.clone()))
                        } else {
                            cache.texts.first().map(|text| text.0.clone())
                        }
                    })
                } else if self.tab == Tab::Files {
                    self.rows.get(self.cursor).and_then(|entry| {
                        if let Target::File(file) = entry.target {
                            context
                                .cache()
                                .and_then(|cache| cache.files.get(file))
                                .map(|file| file.path.clone())
                        } else {
                            Some(entry.line.to_string())
                        }
                    })
                } else {
                    self.rows
                        .get(self.cursor)
                        .map(|entry| entry.line.to_string())
                };
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    actions.push(Action::Copy(text));
                }
            }
            KeyCode::Char('Y') => {
                if let Some(text) = context
                    .cache()
                    .and_then(|cache| cache.texts.first())
                    .map(|text| text.1.clone())
                    .filter(|text| !text.is_empty())
                {
                    actions.push(Action::Copy(text));
                }
            }
            _ => {}
        }
        actions
    }

    fn changed_filter(&mut self, context: Context<'_>, actions: &mut Vec<Action>) {
        self.analysis = None;
        self.cursor = 0;
        self.event_page = 0;
        self.scroll = 0;
        self.dirty = true;
        actions.push(self.analysis_request(context.selection));
    }

    fn change_speed(&mut self, faster: bool) {
        self.speed_index = if faster {
            (self.speed_index + 1).min(SPEEDS.len() - 1)
        } else {
            self.speed_index.saturating_sub(1)
        };
        if let Some(playback) = self.playback.as_mut() {
            playback.set_speed(SPEEDS[self.speed_index], Instant::now());
        }
        self.dirty = true;
    }

    fn move_by(&mut self, delta: isize) {
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(self.rows.len().saturating_sub(1));
        self.follow_replay = false;
    }

    fn export(&self, context: Context<'_>) -> Option<String> {
        let lane = context.session.lanes.get(context.selection.lane)?;
        self.analysis.as_ref().map(|analysis| {
            analysis.export_json(
                context.events(),
                context.selection.first,
                &context.session.id,
                &lane.name,
            )
        })
    }

    fn ensure(&mut self, context: Context<'_>, width: u16) {
        if self.tab == Tab::Replay {
            self.ensure_playback(context);
        }
        let key = DocumentKey {
            session: Arc::as_ptr(context.session) as usize,
            selection: context.selection,
            revision: context.revision,
            width,
            replay_visible: if self.tab == Tab::Replay {
                self.playback.as_ref().map(|playback| {
                    context
                        .events()
                        .partition_point(|event| event.start <= playback.timestamp())
                })
            } else {
                None
            },
        };
        if self.key.as_ref() == Some(&key) && !self.dirty {
            return;
        }
        let entries = render::document(self, context);
        self.rows = entries
            .into_iter()
            .flat_map(|entry| {
                rich::wrap(entry.line, usize::from(width.max(1)))
                    .into_iter()
                    .map(move |line| Entry {
                        line,
                        target: entry.target,
                    })
            })
            .collect();
        if self.tab == Tab::Replay && self.follow_replay {
            self.cursor = self.rows.len().saturating_sub(1);
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        self.key = Some(key);
        self.dirty = false;
        self.width = width;
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>, area: Rect, context: Context<'_>) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(format!(
                " 詳細 · {} #{}–{} ",
                rich::clean(
                    context
                        .session
                        .lanes
                        .get(context.selection.lane)
                        .map_or("", |lane| lane.name.as_str())
                )
                .replace('\n', " "),
                context.selection.first + 1,
                context.selection.last + 1
            ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.is_empty() {
            return;
        }
        let control_height = if inner.height >= 6 { 2 } else { 1 };
        let [tabs, controls, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(control_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);
        frame.render_widget(
            Line::from(
                Tab::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| {
                        let style = if *tab == self.tab {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        Span::styled(format!(" {}:{} ", i + 1, tab.title()), style)
                    })
                    .collect::<Vec<_>>(),
            ),
            tabs,
        );
        let controls_text = if self.editing {
            format!(
                "検索 /{}\nEnter:検索  Esc:取消  Ctrl-U:消去",
                self.filter.query
            )
        } else if self.tab == Tab::Replay {
            self.ensure_playback(context);
            let playback = self.playback.as_ref().unwrap();
            format!(
                "Space:{}  +/-:{}倍  R:先頭  End:追従\n{:.1}/{:.1}秒  {}",
                if playback.is_playing() {
                    "停止"
                } else {
                    "再生"
                },
                SPEEDS[self.speed_index],
                self.playback_offset + playback.position(),
                self.playback_offset + playback.duration(),
                if self.follow_replay {
                    "追従中"
                } else {
                    "追従停止"
                }
            )
        } else if self.tab == Tab::Body {
            format!(
                "x:展開  z:{}  l:行番号  y:入力コピー Y:結果コピー\nu:直前の指示 d:子エージェント b:起動元",
                if self.raw {
                    "整形表示"
                } else {
                    "生データ"
                }
            )
        } else if self.tab == Tab::Files {
            "Enter:変更履歴の開閉 / イベントへ  x:差分全行\ny:パス・差分をコピー".to_owned()
        } else {
            format!(
                "t:{}  f:{}  o:{}  n:{}  0:解除\n/ {}{}",
                kind_label(self.filter.kind),
                status_label(self.filter.status),
                order_label(self.order),
                self.filter.name.as_deref().unwrap_or("すべての名前"),
                if self.editing {
                    "検索中: "
                } else {
                    "検索: "
                },
                rich::clean(&self.filter.query)
            )
        };
        frame.render_widget(
            Paragraph::new(rich::clean(&controls_text)).style(Style::default().fg(Color::Gray)),
            controls,
        );
        self.page = usize::from(body.height).max(1);
        self.ensure(context, body.width);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        if self.cursor >= self.scroll.saturating_add(self.page) {
            self.scroll = self.cursor.saturating_sub(self.page - 1);
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(self.page));
        for (offset, entry) in self
            .rows
            .iter()
            .skip(self.scroll)
            .take(self.page)
            .enumerate()
        {
            let selected = self.scroll + offset == self.cursor;
            let line = if selected {
                entry
                    .line
                    .clone()
                    .style(Style::default().bg(Color::DarkGray))
            } else {
                entry.line.clone()
            };
            frame.render_widget(
                line,
                Rect::new(body.x, body.y + offset as u16, body.width, 1),
            );
        }
        let footer_text = if self.editing {
            "Enter:検索  Esc:取消".to_owned()
        } else {
            format!(
                "{}/{}  ↑↓:移動 Enter:開く  ,.:頁{}/{}  [ ]:レーン  c:再開 e:JSON",
                if self.rows.is_empty() {
                    0
                } else {
                    self.cursor + 1
                },
                self.rows.len(),
                self.event_page + 1,
                self.event_pages
            )
        };
        frame.render_widget(
            Line::styled(footer_text, Style::default().fg(Color::DarkGray)),
            footer,
        );
    }
}

fn kind_label(filter: KindFilter) -> &'static str {
    match filter {
        KindFilter::All => "全種別",
        KindFilter::Tools => "ツール",
        KindFilter::Messages => "発言",
    }
}
fn status_label(filter: StatusFilter) -> &'static str {
    match filter {
        StatusFilter::All => "全状態",
        StatusFilter::Failed => "失敗",
        StatusFilter::Pending => "結果なし",
    }
}
fn order_label(order: Order) -> &'static str {
    match order {
        Order::Oldest => "古い順",
        Order::Newest => "新しい順",
        Order::Duration => "所要時間順",
        Order::Tokens => "トークン順",
    }
}

pub fn resume_command(session: &Session) -> String {
    fn quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
    let command = match session.source {
        Source::Claude => "claude --resume",
        Source::Codex => "codex resume",
    };
    format!(
        "cd {} && {command} {}",
        quote(&session.cwd),
        quote(&session.id)
    )
}
