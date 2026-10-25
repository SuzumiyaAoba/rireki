use super::super::view::View;
use super::*;
use crate::model::{Event, Kind, Lane, Snapshot, Source, Status};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

fn fixture() -> Arc<Session> {
    let mut lane = Lane::new("main");
    lane.origin = "test.jsonl".into();
    lane.events = (0..4)
        .map(|n| Event {
            start: 100.0 + f64::from(n),
            end: 100.5 + f64::from(n),
            kind: Kind::User,
            name: "User".into(),
            summary: format!("日本語 {n}").into(),
            input: format!("本文 {n}"),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
            usage: Default::default(),
            model: None,
            status: Status::Ok,
        })
        .collect();
    let mut session = Session {
        id: "one".into(),
        source: Source::Claude,
        cwd: "/project".into(),
        headless: false,
        title: "日本語セッション".into(),
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
    state.apply(super::super::worker::Update::History {
        snapshot: Snapshot { sessions },
        loading: false,
        progress: None,
    });
}
fn setup() -> (State, Workspace) {
    let mut state = State::default();
    let mut workspace = Workspace::new(false, &mut state);
    publish(&mut state, vec![fixture()]);
    workspace.sync(&mut state);
    (state, workspace)
}
fn key(workspace: &mut Workspace, state: &mut State, code: KeyCode) {
    assert!(!workspace.key(KeyEvent::new(code, KeyModifiers::NONE), state));
    workspace.sync(state);
}

#[test]
fn range_selection_survives_live_insertion_and_history_resolves_identifiers() {
    let (mut state, mut workspace) = setup();
    let range = Selection {
        session: 0,
        lane: 0,
        first: 1,
        last: 3,
    };
    workspace.open(range, &mut state, true);
    let mut updated = (*state.snapshot.sessions[0]).clone();
    let mut inserted = updated.lanes[0].events[0].clone();
    inserted.start = 99.0;
    inserted.end = 99.5;
    inserted.summary = "inserted".into();
    updated.lanes[0].events.insert(0, inserted);
    updated.finalize();
    let mut other = (*fixture()).clone();
    other.id = "other".into();
    publish(&mut state, vec![Arc::new(other), Arc::new(updated)]);
    workspace.sync(&mut state);
    assert_eq!(
        workspace.selection,
        Some(Selection {
            session: 1,
            lane: 0,
            first: 2,
            last: 4
        })
    );
    workspace.open(
        Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: 0,
        },
        &mut state,
        true,
    );
    workspace.navigate(&mut state, false);
    assert_eq!(workspace.selection.unwrap().last, 4);
    workspace.navigate(&mut state, true);
    assert_eq!(workspace.selection.unwrap().session, 0);
}

#[test]
fn stale_details_are_rejected_and_hidden_targets_clear_filters() {
    let (mut state, mut workspace) = setup();
    let stale = workspace.generations[Job::Inspect];
    let selected = Selection {
        session: 0,
        lane: 0,
        first: 1,
        last: 1,
    };
    state.toggle_live();
    workspace.open(selected, &mut state, true);
    assert!(!state.live_only);
    assert!(workspace.inspect_loading);
    workspace.apply(
        operations::Update::Inspect {
            generation: stale,
            detail: None,
        },
        &mut state,
    );
    assert!(workspace.inspect_loading);
    workspace.apply(
        operations::Update::Inspect {
            generation: workspace.generations[Job::Inspect],
            detail: None,
        },
        &mut state,
    );
    assert!(!workspace.inspect_loading);
}

#[test]
fn shortcuts_preserve_text_input_and_open_event_body() {
    let (mut state, mut workspace) = setup();
    key(&mut workspace, &mut state, KeyCode::F(2));
    key(&mut workspace, &mut state, KeyCode::Char('L'));
    assert!(!state.live_only);
    workspace.key(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
        &mut state,
    );
    assert!(state.editing_query);
    key(&mut workspace, &mut state, KeyCode::Char('q'));
    assert_eq!(state.query, "q");
    key(&mut workspace, &mut state, KeyCode::Esc);
    workspace.inspector_actions(
        vec![inspect::Action::Select(Selection {
            session: 0,
            lane: 0,
            first: 1,
            last: 1,
        })],
        &mut state,
    );
    assert_eq!(workspace.inspector.settings().tab, inspect::Tab::Body);
    key(&mut workspace, &mut state, KeyCode::Char('P'));
    key(&mut workspace, &mut state, KeyCode::Char('q'));
    assert!(matches!(workspace.dialog, Some(Dialog::Projects { ref query, .. }) if query == "q"));
}

#[test]
fn all_pages_render_without_overflow_and_saved_preferences_roundtrip() {
    let (mut state, mut workspace) = setup();
    for (width, height) in [(1, 1), (30, 8), (80, 24), (140, 45)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut view = View::default();
        for page in 1..=8 {
            key(&mut workspace, &mut state, KeyCode::F(page));
            terminal
                .draw(|f| workspace.draw(f, &mut state, &mut view))
                .unwrap();
        }
        key(&mut workspace, &mut state, KeyCode::Char('?'));
        terminal
            .draw(|f| workspace.draw(f, &mut state, &mut view))
            .unwrap();
        key(&mut workspace, &mut state, KeyCode::Esc);
    }
    let prefs = Preferences {
        source: SourceFilter::Codex,
        project: Some("/日本語".into()),
        live_only: true,
        sidebar: false,
        page: Page::Statistics,
        ..Preferences::default()
    };
    let restored: Preferences =
        serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
    assert_eq!(restored.source, SourceFilter::Codex);
    assert_eq!(restored.project.as_deref(), Some("/日本語"));
    assert_eq!(restored.page, Page::Statistics);
    assert!(restored.live_only && !restored.sidebar);
    assert!(workspace.save(&state).is_ok()); // Explicit roots never persist settings.
}

#[test]
fn lane_follow_and_history_restore_the_analysis_tab() {
    let (mut state, mut workspace) = setup();
    let range = Selection {
        session: 0,
        lane: 0,
        first: 0,
        last: 3,
    };
    workspace.open(range, &mut state, true);
    workspace.inspector.set_tab(inspect::Tab::Analysis);
    let mut updated = (*state.snapshot.sessions[0]).clone();
    let mut appended = updated.lanes[0].events[3].clone();
    appended.start = 105.0;
    appended.end = 106.0;
    appended.summary = "new".into();
    updated.lanes[0].events.push(appended);
    updated.finalize();
    publish(&mut state, vec![Arc::new(updated)]);
    workspace.sync(&mut state);
    assert_eq!(workspace.selection.unwrap().last, 4);
    workspace.inspector_actions(
        vec![inspect::Action::Select(Selection {
            first: 2,
            last: 2,
            ..range
        })],
        &mut state,
    );
    assert_eq!(workspace.inspector.settings().tab, inspect::Tab::Body);
    workspace.navigate(&mut state, false);
    assert_eq!(workspace.selection.unwrap().last, 4);
    assert_eq!(workspace.inspector.settings().tab, inspect::Tab::Analysis);
    workspace.fullscreen = true;
    key(&mut workspace, &mut state, KeyCode::Tab);
    assert_eq!(state.focus, Focus::Detail);
    key(&mut workspace, &mut state, KeyCode::Esc);
    assert!(!workspace.fullscreen);
}

#[test]
fn live_filter_expires_without_new_snapshot_and_statistics_escape_returns() {
    let (mut state, mut workspace) = setup();
    // A session can age out between polls even when the snapshot is unchanged.
    state.live_only = true;
    assert!(!state.catalog.order.is_empty());
    workspace.last_live_check = Instant::now() - std::time::Duration::from_secs(4);
    workspace.sync(&mut state);
    assert!(state.catalog.order.is_empty());
    assert!(workspace.selection.is_none());
    assert_eq!(workspace.take_cancellations().len(), 2);
    key(&mut workspace, &mut state, KeyCode::F(6));
    key(&mut workspace, &mut state, KeyCode::Esc);
    assert_eq!(workspace.page, Page::History);
}

#[test]
fn search_reveals_hidden_panes_and_paste_waits_for_analysis_confirmation() {
    let (mut state, mut workspace) = setup();
    workspace.fullscreen = true;
    workspace.sidebar = false;
    state.focus = Focus::Detail;
    workspace.key(
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
        &mut state,
    );
    assert!(!workspace.fullscreen && workspace.sidebar && state.editing_query);
    key(&mut workspace, &mut state, KeyCode::Esc);
    workspace.open(
        Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: 3,
        },
        &mut state,
        false,
    );
    key(&mut workspace, &mut state, KeyCode::Char('/'));
    workspace.take_requests();
    workspace.paste("日本語q", &mut state);
    assert!(workspace.take_requests().is_empty());
    key(&mut workspace, &mut state, KeyCode::Enter);
    assert!(
        workspace
            .take_requests()
            .iter()
            .any(|r| matches!(r, Request::Analysis { filter, .. } if filter.query == "日本語q"))
    );
}

#[test]
fn live_log_changes_invalidate_analysis_exports_until_the_new_result_arrives() {
    use rireki::detail::analysis::{Analysis, Filter, Order};
    let (mut state, mut workspace) = setup();
    workspace.open(
        Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: 3,
        },
        &mut state,
        false,
    );
    let filter = Filter {
        query: "日本語 3".into(),
        ..Filter::default()
    };
    let analysis = Analysis::build(
        &state.snapshot.sessions[0].lanes[0].events,
        &filter,
        Order::Oldest,
        || true,
    )
    .unwrap();
    workspace.apply(
        operations::Update::Analysis {
            generation: workspace.generations[Job::Analysis],
            analysis: Some(Arc::new(analysis)),
        },
        &mut state,
    );
    workspace.inspect_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &mut state,
    );
    assert!(matches!(workspace.dialog, Some(Dialog::Export { .. })));
    workspace.dialog = None;
    let mut changed = (*state.snapshot.sessions[0]).clone();
    changed.lanes[0].events.remove(1);
    changed.finalize();
    publish(&mut state, vec![Arc::new(changed)]);
    workspace.sync(&mut state);
    assert_eq!(workspace.selection.unwrap().last, 2);
    workspace.inspect_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &mut state,
    );
    assert!(workspace.dialog.is_none());
    assert!(state.status.contains("読み込"));
    let analysis = Analysis::build(
        &state.snapshot.sessions[0].lanes[0].events,
        &filter,
        Order::Oldest,
        || true,
    )
    .unwrap();
    workspace.apply(
        operations::Update::Analysis {
            generation: workspace.generations[Job::Analysis],
            analysis: Some(Arc::new(analysis)),
        },
        &mut state,
    );
    workspace.inspect_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        &mut state,
    );
    let Some(Dialog::Export { content, .. }) = &workspace.dialog else {
        panic!("expected export");
    };
    let json: serde_json::Value = serde_json::from_str(content).unwrap();
    assert_eq!(json["events"][0]["event"], 3);
    assert_eq!(json["events"][0]["summary"], "日本語 3");
}

#[test]
fn removing_the_selected_session_during_analysis_input_does_not_trap_global_keys() {
    let (mut state, mut workspace) = setup();
    state.focus = Focus::Detail;
    key(&mut workspace, &mut state, KeyCode::Char('/'));
    workspace.paste("未確定の検索", &mut state);
    assert!(workspace.inspector.is_editing());
    publish(&mut state, vec![]);
    workspace.sync(&mut state);
    assert!(workspace.selection.is_none());
    assert!(!workspace.inspector.is_editing());
    key(&mut workspace, &mut state, KeyCode::F(7));
    assert_eq!(workspace.page, Page::Mcp);
    assert!(workspace.key(
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        &mut state
    ));
}
