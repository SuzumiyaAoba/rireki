//! ペイン間のキー操作、設定変更、ダイアログの入力。

use super::{
    Dialog, Effect, Focus, Location, Page, Request, Selection, State, Workspace, inspect, metrics,
    project_choices, temporal,
};
use crate::tui::effects;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

impl Workspace {
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
                metrics::Action::Session { index, failures } => {
                    if let Some(selection) = metrics::session_selection(&state.snapshot, index, failures) {
                        self.open(selection, state, true);
                    }
                }
            },
            Page::Mcp => self.mcp_key(key, state),
            Page::Settings => self.settings_key(key, state),
        }
        false
    }

    pub(super) fn inspect_key(&mut self, key: KeyEvent, state: &mut State) {
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
                    match effects::save(std::path::Path::new(path), content) {
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
