//! 共通ヘッダー、セッション一覧、全レーンのイベント一覧。画面配置は Workspace が管理する。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_segmentation::UnicodeSegmentation;

use rireki::catalog::SourceFilter;
use rireki::model::{Kind, Status};

use super::state::{Focus, Row, State};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const SELECTED: Color = Color::Blue;

#[derive(Default)]
pub(super) struct View {}

impl View {
    pub(super) fn header(&self, frame: &mut Frame<'_>, area: Rect, state: &State) {
        let source = match state.source {
            SourceFilter::All => "すべて",
            SourceFilter::Claude => "Claude",
            SourceFilter::Codex => "Codex",
        };
        let project = state.project.as_deref().map_or("全プロジェクト", |cwd| {
            state
                .catalog
                .projects
                .iter()
                .find(|project| project.cwd == cwd)
                .map_or(cwd, |project| project.name.as_str())
        });
        let activity = if state.searching {
            "検索中…".to_owned()
        } else if state.loading {
            state.progress.map_or_else(
                || "読み込み中…".to_owned(),
                |(done, total)| format!("読み込み {done}/{total}"),
            )
        } else {
            format!("{} セッション", state.catalog.order.len())
        };
        let title = Line::from(vec![
            Span::styled(" rireki TUI ", emphasis()),
            Span::raw(format!("│ {source} │ ")),
            Span::raw(safe_inline(project, usize::from(area.width))),
            Span::styled(format!(" │ {activity}"), muted()),
        ]);
        frame.render_widget(title, Rect::new(area.x, area.y, area.width, 1));
        if area.height <= 1 {
            return;
        }
        let query_area = Rect::new(area.x, area.y + 1, area.width, 1);
        let prefix = if state.editing_query {
            " 検索 /"
        } else {
            " / 検索: "
        };
        let prefix_width = Span::raw(prefix).width();
        let query_width = usize::from(area.width).saturating_sub(prefix_width + 1);
        let query = tail(&safe_inline(&state.query, 4096), query_width);
        let cursor_width = Span::raw(query.as_str()).width();
        let query_style = if state.editing_query {
            Style::default().fg(ACCENT)
        } else {
            muted()
        };
        frame.render_widget(
            Line::from(vec![
                Span::styled(prefix, query_style),
                Span::raw(query),
                Span::styled(
                    if state.editing_query {
                        "  Enter:確定 Esc:取消"
                    } else if state.query.trim().is_empty() {
                        "タイトル・要約を検索"
                    } else {
                        "  Esc:検索解除"
                    },
                    muted(),
                ),
            ]),
            query_area,
        );
        if state.editing_query && !state.help && usize::from(area.width) > prefix_width {
            let x = area.x + (prefix_width + cursor_width).min(usize::from(area.width - 1)) as u16;
            frame.set_cursor_position((x, query_area.y));
        }
    }

    pub(super) fn sessions(&self, frame: &mut Frame<'_>, area: Rect, state: &mut State) {
        let count = state.rows.len();
        let position = state.row_selected.map_or(0, |selected| selected + 1);
        let title = if state.query.trim().is_empty() {
            format!(" セッション {position}/{count} ")
        } else {
            format!(" 検索結果 {position}/{count} (全{}件) ", state.search_total)
        };
        let block = pane(title, state.focus == Focus::Sessions);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        state.session_page = usize::from(inner.height).max(1);
        if count == 0 {
            let message = if state.loading {
                "履歴を読み込んでいます…"
            } else if state.searching {
                "検索しています…"
            } else if !state.query.trim().is_empty() {
                "一致する履歴がありません"
            } else {
                "表示できる履歴がありません"
            };
            frame.render_widget(Paragraph::new(message).style(muted()), inner);
            return;
        }
        let start = viewport_start(count, state.row_selected, usize::from(inner.height));
        for (row, item) in state
            .rows
            .iter()
            .enumerate()
            .skip(start)
            .take(usize::from(inner.height))
        {
            let (session_index, hit) = match *item {
                Row::Session(session) => (session, None),
                Row::Event {
                    session,
                    lane,
                    event,
                } => (session, Some((lane, event))),
            };
            let Some(session) = state.snapshot.sessions.get(session_index) else {
                continue;
            };
            let selected = state.row_selected == Some(row);
            let mut spans = vec![Span::raw(if selected { "› " } else { "  " })];
            if let Some((lane, event)) = hit {
                let Some(event) = session
                    .lanes
                    .get(lane)
                    .and_then(|lane| lane.events.get(event))
                else {
                    continue;
                };
                spans.push(Span::styled("• ", Style::default().fg(ACCENT)));
                spans.push(Span::styled(
                    format!("{} ", source_mark(session.source)),
                    muted(),
                ));
                let title = if session.title.is_empty() {
                    &session.id
                } else {
                    &session.title
                };
                spans.push(Span::styled(
                    short_inline(title, usize::from(inner.width / 4).clamp(4, 18)),
                    muted(),
                ));
                spans.push(Span::styled(" │ ", muted()));
                spans.push(Span::raw(short_inline(
                    &event.name,
                    usize::from(inner.width / 4).clamp(4, 16),
                )));
                spans.push(Span::raw(" "));
                spans.push(Span::raw(safe_inline(
                    &event.summary,
                    usize::from(inner.width),
                )));
            } else {
                spans.push(Span::styled(
                    format!("{} ", source_mark(session.source)),
                    Style::default().fg(ACCENT),
                ));
                let title = if session.title.is_empty() {
                    &session.id
                } else {
                    &session.title
                };
                spans.push(Span::raw(safe_inline(title, usize::from(inner.width))));
                spans.push(Span::styled(
                    format!(
                        "  {}",
                        rireki::time::local(session.start).format("%m/%d %H:%M")
                    ),
                    muted(),
                ));
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(selection_style(selected)),
                Rect::new(inner.x, inner.y + (row - start) as u16, inner.width, 1),
            );
        }
    }

    pub(super) fn events(&self, frame: &mut Frame<'_>, area: Rect, state: &mut State) {
        let count = state.events.len();
        let position = state.event_selected.map_or(0, |selected| selected + 1);
        let block = pane(
            format!(" イベント {position}/{count} · 全レーン "),
            state.focus == Focus::Events,
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(session) = state
            .selected_session()
            .and_then(|index| state.snapshot.sessions.get(index))
        else {
            state.event_page = usize::from(inner.height).max(1);
            frame.render_widget(
                Paragraph::new("セッションを選択してください").style(muted()),
                inner,
            );
            return;
        };
        let [session_header, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        let title = if session.title.is_empty() {
            &session.id
        } else {
            &session.title
        };
        frame.render_widget(
            Line::styled(
                format!(
                    "{} · {}",
                    session.source.label(),
                    safe_inline(title, usize::from(inner.width))
                ),
                emphasis(),
            ),
            session_header,
        );
        state.event_page = usize::from(list.height).max(1);
        if count == 0 {
            frame.render_widget(Paragraph::new("イベントがありません").style(muted()), list);
        }
        let start = viewport_start(count, state.event_selected, usize::from(list.height));
        for (row, event_ref) in state
            .events
            .iter()
            .enumerate()
            .skip(start)
            .take(usize::from(list.height))
        {
            let Some(lane) = session.lanes.get(event_ref.lane) else {
                continue;
            };
            let Some(event) = lane.events.get(event_ref.event) else {
                continue;
            };
            let selected = state.event_selected == Some(row);
            let line = Line::from(vec![
                Span::raw(if selected { "› " } else { "  " }),
                Span::styled(
                    format!("{} ", rireki::time::local(event.start).format("%H:%M:%S")),
                    muted(),
                ),
                Span::styled(
                    format!("{} ", kind_label(event.kind)),
                    kind_style(event.kind),
                ),
                Span::styled(format!("[{}] ", safe_inline(&lane.name, 16)), muted()),
                Span::styled(
                    format!("{} ", safe_inline(&event.name, 24)),
                    status_style(event.status),
                ),
                Span::raw(safe_inline(&event.summary, usize::from(inner.width))),
            ]);
            frame.render_widget(
                Paragraph::new(line).style(selection_style(selected)),
                Rect::new(list.x, list.y + (row - start) as u16, list.width, 1),
            );
        }
    }
}

fn printable(c: char) -> char {
    match c {
        '\0'..='\u{1f}' => char::from_u32(c as u32 + 0x2400).unwrap_or('�'),
        '\u{7f}' => '␡',
        '\u{80}'..='\u{9f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => '�',
        _ => c,
    }
}

fn safe_inline(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            _ => printable(c),
        })
        .collect()
}

fn short_inline(text: &str, width: usize) -> String {
    let safe = safe_inline(text, width.saturating_mul(4));
    if Span::raw(safe.as_str()).width() <= width {
        return safe;
    }
    if width == 0 {
        return String::new();
    }
    let mut short = String::new();
    let mut used = 0;
    for grapheme in safe.graphemes(true) {
        let cells = Span::raw(grapheme).width();
        if used + cells >= width {
            break;
        }
        short.push_str(grapheme);
        used += cells;
    }
    short.push('…');
    short
}

fn source_mark(source: rireki::model::Source) -> &'static str {
    match source {
        rireki::model::Source::Claude => "C",
        rireki::model::Source::Codex => "X",
    }
}

fn tail(text: &str, width: usize) -> String {
    let mut used = 0;
    let start = text
        .grapheme_indices(true)
        .rev()
        .find_map(|(index, grapheme)| {
            used += Span::raw(grapheme).width();
            (used > width).then_some(index + grapheme.len())
        })
        .unwrap_or(0);
    text[start..].to_owned()
}

fn viewport_start(count: usize, selected: Option<usize>, height: usize) -> usize {
    selected
        .unwrap_or(0)
        .saturating_sub(height / 2)
        .min(count.saturating_sub(height))
}

fn pane(title: String, focused: bool) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(ACCENT)
        } else {
            muted()
        });
    if title.is_empty() {
        block
    } else {
        block.title(title)
    }
}

fn emphasis() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}
fn muted() -> Style {
    Style::default().fg(MUTED)
}
fn selection_style(selected: bool) -> Style {
    if selected {
        Style::default().bg(SELECTED).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}
fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::User => "ユーザー",
        Kind::Assistant => "応答",
        Kind::Thinking => "思考",
        Kind::Tool => "ツール",
        Kind::Agent => "エージェント",
    }
}
fn kind_style(kind: Kind) -> Style {
    Style::default().fg(match kind {
        Kind::User => Color::Green,
        Kind::Assistant => ACCENT,
        Kind::Thinking => Color::Magenta,
        Kind::Tool => Color::Yellow,
        Kind::Agent => Color::LightBlue,
    })
}
fn status_style(status: Status) -> Style {
    Style::default().fg(match status {
        Status::Ok => Color::Green,
        Status::Error => Color::Red,
        Status::Rejected | Status::Interrupted => Color::LightRed,
        Status::Pending => Color::Yellow,
    })
}

#[cfg(test)]
mod tests {
    use super::super::worker::Update;
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use rireki::model::{Event, Lane, Session, Snapshot, Source};
    use std::sync::Arc;

    fn fixture() -> State {
        let event = Event {
            start: 1_790_000_000.0,
            end: 1_790_000_001.5,
            kind: Kind::Tool,
            name: "exec_command".into(),
            summary: "日本語の検索結果".into(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
            usage: Default::default(),
            model: Some("test-model".into()),
            status: Status::Ok,
        };
        let mut lane = Lane::new("main");
        lane.events.push(event);
        let mut session = Session {
            id: "session".into(),
            source: Source::Codex,
            cwd: "/workspace/日本語".into(),
            headless: false,
            title: "日本語の履歴".into(),
            branch: "main".into(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: vec![],
            tree: vec![],
        };
        session.finalize();
        let mut state = State::default();
        state.apply(Update::History {
            snapshot: Snapshot {
                sessions: vec![Arc::new(session)],
            },
            loading: false,
            progress: None,
        });
        state
    }

    /// ペイン単体の描画を検証する。画面の切替・配置は Workspace のテストが扱う。
    fn render(state: &mut State, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let [header, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)])
                    .areas(frame.area());
                let view = View::default();
                view.header(frame, header, state);
                match state.focus {
                    Focus::Sessions => view.sessions(frame, body, state),
                    Focus::Events | Focus::Detail => view.events(frame, body, state),
                }
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &Buffer) -> String {
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
    fn session_and_event_panes_show_japanese_and_selection() {
        let mut state = fixture();
        let buffer = render(&mut state, 80, 20);
        let rendered = text(&buffer);
        assert!(rendered.contains("rireki TUI") && rendered.contains("日本語の履歴"));
        assert!(buffer.content.iter().any(|cell| cell.bg == SELECTED));
        assert!(state.session_page > 1);
        state.focus = Focus::Events;
        let rendered = text(&render(&mut state, 100, 20));
        for expected in ["全レーン", "exec_command", "日本語の検索結果", "[main]"] {
            assert!(
                rendered.contains(expected),
                "missing {expected}:\n{rendered}"
            );
        }
        assert!(state.event_page > 1);
    }

    #[test]
    fn empty_search_and_tiny_panes_are_renderable() {
        let mut state = State::default();
        state.apply(Update::History {
            snapshot: Snapshot::default(),
            loading: false,
            progress: None,
        });
        assert!(text(&render(&mut state, 80, 24)).contains("表示できる履歴がありません"));
        state.query = " 　".into();
        let whitespace = text(&render(&mut state, 80, 24));
        assert!(whitespace.contains("表示できる履歴がありません"));
        assert!(!whitespace.contains("検索結果"));
        state.query = "該当なし".into();
        state.editing_query = true;
        let search = text(&render(&mut state, 80, 24));
        assert!(search.contains("該当なし") && search.contains("一致する履歴がありません"));
        for focus in [Focus::Sessions, Focus::Events] {
            state.focus = focus;
            for (width, height) in [(1, 1), (2, 2), (10, 4), (20, 8), (0, 0)] {
                render(&mut state, width, height);
            }
        }
    }

    #[test]
    fn search_results_distinguish_sessions_from_event_hits() {
        let mut state = fixture();
        state.query = "日本語".into();
        state.search_total = 2;
        state.rows.push(Row::Event {
            session: 0,
            lane: 0,
            event: 0,
        });
        state.row_selected = Some(1);
        let rendered = text(&render(&mut state, 80, 24));
        assert!(rendered.contains("検索結果 2/2"));
        assert!(rendered.contains("• X 日本語の履歴 │ exec_command 日本語の検索結果"));
        assert!(!rendered.contains('↳'));
    }

    #[test]
    fn inline_labels_preserve_graphemes_and_never_pass_terminal_controls() {
        let safe = safe_inline("日本語 e\u{301} 👩‍💻\n\u{1b}[2J\u{7}\u{9b}\u{0}\t終", 100);
        assert!(!safe.chars().any(char::is_control));
        assert!(safe.contains("e\u{301}") && safe.contains("👩‍💻"));
        assert!(safe.contains("␛[2J␇�␀"));
        for width in 0..16 {
            let shortened = short_inline("日本語 e\u{301} 👩‍💻長いタイトル", width);
            assert!(Span::raw(&shortened).width() <= width);
            assert!(!shortened.ends_with('\u{301}') || shortened.contains("e\u{301}"));
            assert!(!shortened.ends_with('\u{200d}'));
        }
        assert_eq!(tail("前半日本語👩‍💻", 2), "👩‍💻");
    }
}
