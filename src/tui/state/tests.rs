use super::*;
use crate::model::{Event, Kind, Lane, Session, Source, Status};
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
