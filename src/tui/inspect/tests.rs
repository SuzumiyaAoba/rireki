use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use rireki::detail::DetailLoader;
use rireki::model::{Lane, Status, TextRef, Usage};
use std::time::Duration;

fn fixture(count: usize) -> Arc<Session> {
    let mut lane = Lane::new("メイン");
    lane.origin = "/tmp/history.jsonl".into();
    lane.events = (0..count)
        .map(|i| Event {
            start: 100.0 + i as f64,
            end: 101.0 + i as f64,
            kind: if i == 0 { Kind::User } else { Kind::Tool },
            name: if i == 0 {
                "User"
            } else if i + 1 == count {
                "apply_patch"
            } else {
                "exec_command"
            }
            .into(),
            summary: format!("日本語イベント{i}").into(),
            input: if i + 1 == count {
                "*** Update File: src/日本語.rs\n@@ -1,2 +1,2 @@\n-old\n+new".into()
            } else {
                format!("**本文** {i}\n$ cargo test")
            },
            output: "\x1b[31m結果\x1b[0m\n\x1b]52;c;dGVzdA==\x07".into(),
            call_id: (i == 1).then(|| "child-call".into()),
            text_ref: TextRef::default(),
            status: if i % 3 == 0 {
                Status::Error
            } else {
                Status::Ok
            },
            usage: Usage {
                input: 100,
                output: 10,
                cache_read: 30,
                reasoning: 2,
                ..Usage::default()
            },
            model: Some("claude-sonnet-4-5".into()),
        })
        .collect();
    let mut child = Lane::new("子エージェント");
    child.spawned_by = Some("child-call".into());
    if count > 1 {
        child.events.push(lane.events[1].clone());
        child.events[0].call_id = None;
    }
    let mut session = Session {
        id: "session'quoted".into(),
        source: Source::Claude,
        cwd: "/tmp/project's".into(),
        headless: false,
        title: "日本語セッション\x1b[2J".into(),
        branch: "master".into(),
        version: "test".into(),
        lanes: vec![lane, child],
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    };
    session.finalize();
    Arc::new(session)
}

fn context<'a>(session: &'a Arc<Session>, cache: &'a DetailCache) -> Context<'a> {
    Context {
        session,
        session_index: cache.sel.session,
        selection: cache.sel,
        cache: Some(cache),
        revision: 1,
    }
}

fn cache(session: &Arc<Session>) -> DetailCache {
    DetailLoader::default()
        .load_session(
            Selection {
                session: 7,
                lane: 0,
                first: 0,
                last: session.lanes[0].events.len() - 1,
            },
            session,
            || true,
        )
        .unwrap()
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn metadata_and_analysis_cover_events_beyond_body_limit_and_page_them() {
    let session = fixture(602);
    let cache = cache(&session);
    let context = context(&session, &cache);
    let mut inspector = Inspector::default();
    inspector.set_analysis(Arc::new(
        Analysis::build(context.events(), &Filter::default(), Order::Oldest, || true).unwrap(),
    ));
    inspector.key(key('4'), context);
    inspector.ensure(context, 160);
    assert!(
        inspector
            .rows
            .iter()
            .any(|row| row.line.to_string().contains("84280 tokens"))
    );
    assert_eq!(inspector.event_pages, 2);
    inspector.key(key('.'), context);
    inspector.ensure(context, 160);
    assert!(inspector.rows.iter().any(|row| row.target
        == Target::Event {
            lane: 0,
            event: 601
        }));
    let json: serde_json::Value =
        serde_json::from_str(&inspector.export(context).unwrap()).unwrap();
    assert_eq!(json["event_count"], 602);
    assert_eq!(json["events"][601]["event"], 602);
    assert_eq!(json["events"][601]["usage"]["reasoning"], 2);
}

#[test]
fn filters_request_background_analysis_and_search_accepts_unicode_without_global_shortcuts() {
    let session = fixture(10);
    let cache = cache(&session);
    let context = context(&session, &cache);
    let mut inspector = Inspector::default();
    assert!(matches!(
        inspector.key(key('f'), context).as_slice(),
        [Action::Analyze {
            filter: Filter {
                status: StatusFilter::Failed,
                ..
            },
            ..
        }]
    ));
    inspector.key(key('/'), context);
    inspector.paste("日本語\x1b\n");
    inspector.key(key('q'), context);
    assert!(inspector.is_editing());
    assert!(matches!(
        inspector.analysis_request(context.selection),
        Action::Analyze { filter, .. } if filter.query.is_empty()
    ));
    let actions = inspector.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), context);
    assert!(matches!(&actions[0], Action::Analyze { filter, .. } if filter.query == "日本語q"));
    assert!(!inspector.is_editing());
}

#[test]
fn late_file_changes_expand_diff_and_navigate_to_original_event() {
    let session = fixture(602);
    let cache = cache(&session);
    let context = context(&session, &cache);
    let mut inspector = Inspector::default();
    inspector.key(key('3'), context);
    inspector.ensure(context, 120);
    inspector.cursor = inspector
        .rows
        .iter()
        .position(|row| row.target == Target::File(0))
        .unwrap();
    inspector.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), context);
    inspector.ensure(context, 120);
    assert!(
        inspector
            .rows
            .iter()
            .any(|row| row.line.to_string().contains("- old"))
    );
    inspector.cursor = inspector
        .rows
        .iter()
        .position(|row| {
            row.target
                == Target::Event {
                    lane: 0,
                    event: 601,
                }
        })
        .unwrap();
    assert!(matches!(
        inspector
            .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), context)
            .as_slice(),
        [Action::Select(Selection {
            first: 601,
            last: 601,
            ..
        })]
    ));
}

#[test]
fn replay_advances_on_monotonic_time_and_parent_child_prompt_navigation_is_explicit() {
    let session = fixture(10);
    let cache = cache(&session);
    let context = context(&session, &cache);
    let mut inspector = Inspector::default();
    inspector.key(key('5'), context);
    let now = Instant::now();
    inspector.playback.as_mut().unwrap().play(now);
    assert!(inspector.tick(now + Duration::from_secs(2)));
    assert_eq!(inspector.playback.as_ref().unwrap().position(), 2.0);
    inspector.ensure(context, 120);
    assert!(
        inspector
            .rows
            .iter()
            .any(|row| row.target == Target::Event { lane: 0, event: 2 })
    );
    assert!(
        !inspector
            .rows
            .iter()
            .any(|row| row.target == Target::Event { lane: 0, event: 3 })
    );
    let child_context = Context {
        selection: Selection {
            session: 7,
            lane: 1,
            first: 0,
            last: 0,
        },
        cache: None,
        ..context
    };
    assert!(matches!(
        inspector.key(key('b'), child_context).as_slice(),
        [Action::Select(Selection {
            lane: 0,
            first: 1,
            ..
        })]
    ));
    let tool_context = Context {
        selection: Selection {
            session: 7,
            lane: 0,
            first: 1,
            last: 1,
        },
        cache: None,
        ..context
    };
    assert!(matches!(
        inspector.key(key('d'), tool_context).as_slice(),
        [Action::Select(Selection { lane: 1, .. })]
    ));
    assert!(matches!(
        inspector.key(key('u'), tool_context).as_slice(),
        [Action::Select(Selection {
            lane: 0,
            first: 0,
            ..
        })]
    ));
}

#[test]
fn all_tabs_render_in_small_and_wide_terminals_without_log_escape_sequences() {
    let session = fixture(10);
    let cache = cache(&session);
    let context = context(&session, &cache);
    for (width, height) in [(140, 35), (40, 12), (8, 4), (1, 1)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut inspector = Inspector::default();
        inspector.set_analysis(Arc::new(
            Analysis::build(context.events(), &Filter::default(), Order::Oldest, || true).unwrap(),
        ));
        for tab in Tab::ALL {
            inspector.tab = tab;
            inspector.dirty = true;
            terminal
                .draw(|frame| inspector.draw(frame, frame.area(), context))
                .unwrap();
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .all(|cell| !cell.symbol().contains('\x1b'))
            );
        }
    }
}

#[test]
fn preferences_are_validated_and_resume_command_quotes_all_log_values() {
    let mut inspector = Inspector::default();
    inspector.apply_settings(Settings {
        order: u8::MAX,
        speed_index: usize::MAX,
        ..Settings::default()
    });
    assert_eq!(inspector.settings().order, 0);
    assert_eq!(inspector.settings().speed_index, SPEEDS.len() - 1);
    let saved = serde_json::to_string(&inspector.settings()).unwrap();
    let restored: Settings = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.speed_index, SPEEDS.len() - 1);
    assert_eq!(
        resume_command(&fixture(2)),
        "cd '/tmp/project'\\''s' && claude --resume 'session'\\''quoted'"
    );
}

#[test]
fn replay_retains_absolute_position_when_live_range_grows_and_rewinds_to_original_start() {
    let original = fixture(3);
    let original_cache = cache(&original);
    let original_context = context(&original, &original_cache);
    let mut inspector = Inspector::default();
    inspector.key(key('5'), original_context);
    let now = Instant::now();
    inspector.playback.as_mut().unwrap().play(now);
    inspector
        .playback
        .as_mut()
        .unwrap()
        .pause(now + Duration::from_secs(2));
    assert_eq!(inspector.playback.as_ref().unwrap().timestamp(), 102.0);
    let extended = fixture(6);
    let extended_cache = cache(&extended);
    let extended_context = context(&extended, &extended_cache);
    inspector.invalidate_analysis();
    inspector.ensure_playback(extended_context);
    let playback = inspector.playback.as_ref().unwrap();
    assert_eq!(playback.timestamp(), 102.0);
    assert_eq!(inspector.playback_offset + playback.position(), 2.0);
    assert_eq!(inspector.playback_offset + playback.duration(), 6.0);
    assert!(!playback.is_playing());
    inspector.key(key('R'), extended_context);
    assert_eq!(inspector.playback.as_ref().unwrap().timestamp(), 100.0);
    assert_eq!(inspector.playback_offset, 0.0);
    inspector.playback.as_mut().unwrap().play(now);
    inspector
        .playback
        .as_mut()
        .unwrap()
        .pause(now + Duration::from_secs(6));
    inspector.key(key(' '), extended_context);
    assert!(inspector.playback.as_ref().unwrap().is_playing());
    assert!(inspector.playback.as_ref().unwrap().timestamp() < 101.0);
}

#[test]
fn invalidating_a_log_revision_disables_export_without_losing_scroll() {
    let session = fixture(10);
    let cache = cache(&session);
    let context = context(&session, &cache);
    let mut inspector = Inspector::default();
    inspector.set_analysis(Arc::new(
        Analysis::build(context.events(), &Filter::default(), Order::Oldest, || true).unwrap(),
    ));
    inspector.cursor = 20;
    inspector.scroll = 10;
    assert!(inspector.export(context).is_some());
    inspector.invalidate_analysis();
    assert!(inspector.export(context).is_none());
    assert_eq!((inspector.cursor, inspector.scroll), (20, 10));
}
