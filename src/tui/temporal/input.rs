//! 日付・期間入力、ズーム、時間軸とカレンダーのキー操作。

use super::{Action, Granularity, Mode, Model, PRESETS, Snapshot, local, midnight};
use chrono::{Datelike, Duration, Local, Months, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Model {
    pub fn paste(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.extend(
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(64usize.saturating_sub(input.chars().count())),
            );
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

    pub(super) fn edit(&mut self, key: KeyEvent) -> Action {
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

    pub(super) fn step(&mut self, direction: i64) {
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
}

#[derive(Debug, PartialEq)]
pub(super) enum InputRange {
    Days(NaiveDate, NaiveDate),
    Preset(f64),
}

pub(super) fn parse_range(input: &str) -> Result<InputRange, &'static str> {
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
