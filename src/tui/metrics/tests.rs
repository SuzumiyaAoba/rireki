use super::render::{comparison, cost_label, padded};
use crate::model::{Snapshot, UsageSum, format_number as number};
use chrono::NaiveDate;
use ratatui::text::Span;

use super::*;
use crossterm::event::KeyModifiers;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use rireki::model::{Event, Kind, Lane, Session, Source, Status, TextRef, Usage};
use std::sync::Arc;

fn fixture() -> (Snapshot, NaiveDate) {
    let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    let mut lane = Lane::new("main");
    for (offset, model, kind) in [
        (0, "claude-sonnet-4-5", Kind::Assistant),
        (-8, "unknown-model", Kind::Assistant),
        (0, "", Kind::Tool),
        (0, "", Kind::User),
    ] {
        let start = midnight(today + Duration::days(offset)) + 3600.0;
        lane.events.push(Event {
            start,
            end: start + 1.0,
            kind,
            name: "日本語ツール\u{1b}".into(),
            summary: "summary".into(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: TextRef::default(),
            usage: if kind == Kind::Assistant {
                Usage {
                    input: 100,
                    cache_read: 200,
                    cache_write: 40,
                    output: 50,
                    reasoning: 20,
                    ..Usage::default()
                }
            } else {
                Usage::default()
            },
            model: (!model.is_empty()).then(|| model.into()),
            status: if kind == Kind::Tool {
                Status::Error
            } else {
                Status::Ok
            },
        });
    }
    lane.finish();
    let mut session = Session {
        id: "one".into(),
        source: Source::Claude,
        cwd: "/日本語/project".into(),
        headless: false,
        title: "日本語 title".into(),
        branch: String::new(),
        version: String::new(),
        lanes: vec![lane],
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    };
    session.finalize();
    (
        Snapshot {
            sessions: vec![Arc::new(session)],
        },
        today,
    )
}

fn computed(period: Period) -> (Snapshot, Computed) {
    let (snapshot, today) = fixture();
    let params = Params {
        period,
        today,
        cost_view: None,
        in_view_only: true,
    };
    let result = Cache::default()
        .build(&snapshot, &[0], params, || true)
        .unwrap();
    (snapshot, result)
}

#[test]
fn metrics_use_core_scope_previous_period_and_unknown_prices() {
    let (snapshot, result) = computed(Period::Week);
    assert_eq!(result.summary.total.usage.total(), 390);
    assert_eq!(result.summary.total.failures, 1);
    assert_eq!(result.summary.previous.as_ref().unwrap().usage.total(), 390);
    assert_eq!(result.summary.previous.as_ref().unwrap().usage.unpriced, 1);
    assert_eq!(
        result.costs.usage.total(),
        result.summary.total.usage.total()
    );
    let params = Params {
        period: Period::All,
        today: result.summary.to,
        cost_view: None,
        in_view_only: false,
    };
    let empty = Cache::default()
        .build(&snapshot, &[], params, || true)
        .unwrap();
    assert_eq!(empty.summary.total.sessions, 0);
    assert!(empty.costs.directories.is_empty());
    assert!(
        Cache::default()
            .build(&snapshot, &[0], params, || false)
            .is_none()
    );
}

#[test]
fn period_navigation_drills_down_to_exclusive_local_day_end() {
    let (_, result) = computed(Period::Week);
    let today = result.summary.to;
    let mut state = State::default();
    state.apply(result);
    state.set_tab(Tab::Trend);
    state.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    let Action::Timeline { start, end } =
        state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    else {
        panic!("timeline action");
    };
    assert_eq!(start, midnight(today));
    assert_eq!(end, midnight(today + Duration::days(1)));
    state.set_tab(Tab::Projects);
    assert!(
        matches!(state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Action::Project(p) if p == "/日本語/project")
    );
    assert!(matches!(
        state.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)),
        Action::Recompute
    ));
    assert_eq!(state.period, Period::Quarter);
}

#[test]
fn cost_bucket_opens_actual_call_and_zoom_preserves_valid_range() {
    let (_, result) = computed(Period::Week);
    let mut state = State::default();
    state.apply(result);
    state.set_tab(Tab::Costs);
    state.cost_bin = state.costs.as_ref().unwrap().directories[0].buckets[0].index;
    assert!(matches!(
        state.activate(),
        Action::Event(Selection {
            session: 0,
            lane: 0,
            ..
        })
    ));
    let original = state.costs.as_ref().unwrap().view;
    assert!(matches!(
        state.handle_key(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE)),
        Action::Recompute
    ));
    let zoomed = state.params().cost_view.unwrap();
    assert_eq!(
        zoomed.end - zoomed.start,
        (original.end - original.start) / 2.0
    );
    assert!(matches!(
        state.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE)),
        Action::Recompute
    ));
    assert!(state.params().cost_view.is_none());
}

#[test]
fn metrics_render_every_tab_in_tiny_and_wide_terminals_without_control_text() {
    let (snapshot, result) = computed(Period::All);
    let mut state = State::default();
    state.apply(result);
    for (width, height) in [(1, 1), (24, 7), (80, 24), (160, 45)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        for tab in Tab::ALL {
            state.set_tab(tab);
            terminal
                .draw(|frame| draw(frame, frame.area(), &mut state, &snapshot))
                .unwrap();
            // 全角文字の後続セルは空白なので、表示幅ぶん進めて画面を復元する。
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
            assert!(!text.contains('\u{1b}'));
            if width >= 80 && height >= 24 {
                assert!(
                    text.contains(tab.label()),
                    "{width}x{height} {tab:?}: {text}"
                );
            }
        }
    }
}

#[test]
fn saved_metrics_preferences_round_trip_and_unicode_columns_keep_graphemes() {
    let mut state = State {
        period: Period::All,
        metric: Metric::Tokens,
        ..State::default()
    };
    state.set_tab(Tab::Hours);
    state.in_view_only = false;
    let saved = serde_json::to_string(&state.settings()).unwrap();
    let mut restored = State::default();
    restored.apply_settings(serde_json::from_str(&saved).unwrap());
    assert_eq!(restored.period, Period::All);
    assert_eq!(restored.metric, Metric::Tokens);
    assert_eq!(restored.tab, Tab::Hours);
    assert!(!restored.in_view_only);
    let missing: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(missing.period_days, 30);
    // 旧 TUI の保存値を共通指標でも読めることを確認する。
    for (saved, expected) in [
        ("Tokens", Metric::Tokens),
        ("Cost", Metric::Cost),
        ("Turns", Metric::Turns),
        ("Sessions", Metric::Sessions),
        ("Tools", Metric::Tools),
    ] {
        let settings: Settings =
            serde_json::from_value(serde_json::json!({"metric": saved})).unwrap();
        assert_eq!(settings.metric, expected);
        assert_eq!(serde_json::to_value(settings).unwrap()["metric"], saved);
    }
    assert_eq!(Span::raw(padded("日本語", 4)).width(), 4);
    assert_eq!(padded("e\u{301}x", 1), "e\u{301}");
}

#[test]
fn unknown_prices_and_zero_previous_are_not_reported_as_free_or_infinite() {
    assert_eq!(
        cost_label(&UsageSum {
            calls: 1,
            unpriced: 1,
            ..UsageSum::default()
        }),
        "料金不明"
    );
    assert_eq!(comparison(20.0, Some(0.0)), "新規");
    assert_eq!(comparison(0.0, Some(0.0)), "±0.0%");
    assert_eq!(comparison(20.0, None), "比較なし");
    assert_eq!(number(1_234_567), "1,234,567");
}
