use super::input::{InputRange, parse_range};
use super::rows::union_duration;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Span;

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
