//! 詳細表示と再生一覧の上限、順序、アンカーと使用量の検証。

use super::*;

fn list_event(name: &str, status: Status) -> Event {
    Event {
        start: 1.0,
        end: 2.0,
        kind: Kind::Tool,
        name: name.into(),
        summary: Default::default(),
        input: String::new(),
        output: String::new(),
        call_id: None,
        text_ref: Default::default(),
        usage: Default::default(),
        model: None,
        status,
    }
}

#[test]
fn detail_usage_scale_tracks_filters_display_limit_and_updates() {
    let mut events: Vec<_> = (0..=DETAIL_MAX)
        .map(|_| list_event("Read", Status::Ok))
        .collect();
    events[0].usage = Usage {
        input: 100,
        cache_read: 50,
        cache_write: 25,
        output: 2,
        ..Default::default()
    };
    events[1] = list_event("Edit", Status::Error);
    events[1].usage = Usage {
        input: 10,
        output: 80,
        ..Default::default()
    };
    events[DETAIL_MAX].usage = Usage {
        input: 10_000,
        output: 10_000,
        ..Default::default()
    };
    let state = DetailList::new();
    let key = |generation, filter, reversed| DetailListKey {
        sel: Selection {
            session: 1,
            lane: 0,
            first: 0,
            last: DETAIL_MAX,
        },
        generation,
        filter,
        reversed,
        width: 400,
        live: true,
        replay_visible: None,
    };
    state.prepare(key(1, None, false), &events);
    assert_eq!(
        state.usage_scale(),
        UsageScale {
            input: 175,
            output: 80
        }
    );
    state.prepare(key(1, None, true), &events);
    assert_eq!(
        state.usage_scale(),
        UsageScale {
            input: 175,
            output: 80
        }
    );
    state.prepare(key(1, Some(RowFilter::Name("Read".into())), false), &events);
    assert_eq!(
        state.usage_scale(),
        UsageScale {
            input: 175,
            output: 2
        }
    );
    state.prepare(key(1, Some(RowFilter::Failed), false), &events);
    assert_eq!(
        state.usage_scale(),
        UsageScale {
            input: 10,
            output: 80
        }
    );
    state.prepare(
        key(1, Some(RowFilter::Name("Missing".into())), false),
        &events,
    );
    assert_eq!(state.usage_scale(), UsageScale::default());
    events[0].usage = Usage {
        input: u32::MAX,
        cache_read: u32::MAX,
        cache_write: u32::MAX,
        output: 1,
        ..Default::default()
    };
    state.prepare(key(2, None, false), &events);
    assert_eq!(
        state.usage_scale(),
        UsageScale {
            input: u32::MAX as u64 * 3,
            output: 80
        }
    );
}

#[test]
fn replay_usage_scale_includes_new_rows_and_resets_on_rewind() {
    let mut events = vec![
        list_event("Read", Status::Ok),
        list_event("Edit", Status::Ok),
        list_event("Read", Status::Ok),
    ];
    events[0].usage = Usage {
        input: 100,
        output: 2,
        ..Default::default()
    };
    events[1].usage = Usage {
        input: 50,
        cache_read: 100,
        ..Default::default()
    };
    events[2].usage = Usage {
        output: 40,
        ..Default::default()
    };
    for reversed in [false, true] {
        let state = DetailList::new();
        let key = |visible, filter| DetailListKey {
            sel: Selection {
                session: 1,
                lane: 0,
                first: 0,
                last: 2,
            },
            generation: 1,
            filter,
            reversed,
            width: 400,
            live: false,
            replay_visible: Some(visible),
        };
        state.prepare(key(0, None), &events);
        assert_eq!(state.usage_scale(), UsageScale::default());
        state.prepare(key(1, None), &events);
        assert_eq!(
            state.usage_scale(),
            UsageScale {
                input: 100,
                output: 2
            }
        );
        state.prepare(key(2, None), &events);
        assert_eq!(
            state.usage_scale(),
            UsageScale {
                input: 150,
                output: 2
            }
        );
        state.prepare(key(3, None), &events);
        assert_eq!(
            state.usage_scale(),
            UsageScale {
                input: 150,
                output: 40
            }
        );
        state.prepare(key(1, None), &events);
        assert_eq!(
            state.usage_scale(),
            UsageScale {
                input: 100,
                output: 2
            }
        );
        let filter = Some(RowFilter::Name("Read".into()));
        state.prepare(key(1, filter.clone()), &events);
        state.prepare(key(3, filter), &events);
        assert_eq!(
            state.usage_scale(),
            UsageScale {
                input: 100,
                output: 40
            }
        );
    }
}

#[test]
fn detail_list_filters_within_original_display_limit() {
    let mut events: Vec<_> = (0..DETAIL_MAX + 3)
        .map(|i| {
            list_event(
                if i % 2 == 0 { "Read" } else { "Edit" },
                match i % 5 {
                    0 => Status::Error,
                    1 => Status::Rejected,
                    2 => Status::Interrupted,
                    3 => Status::Pending,
                    _ => Status::Ok,
                },
            )
        })
        .collect();
    let all = detail_rows(&events, None, false);
    assert_eq!(all.len(), DETAIL_MAX + 2);
    assert_eq!(all[0], DetailRow::Summary);
    assert_eq!(all[DETAIL_MAX], DetailRow::Event(DETAIL_MAX - 1));
    assert_eq!(all[DETAIL_MAX + 1], DetailRow::Limit);
    for filter in [RowFilter::Name("Edit".into()), RowFilter::Failed] {
        let rows = detail_rows(&events, Some(&filter), false);
        let expected: Vec<_> = events
            .iter()
            .take(DETAIL_MAX)
            .enumerate()
            .filter(|(_, e)| match &filter {
                RowFilter::Name(name) => e.name.as_ref() == name,
                RowFilter::Failed => matches!(
                    e.status,
                    Status::Error | Status::Rejected | Status::Interrupted
                ),
            })
            .map(|(i, _)| DetailRow::Event(i))
            .collect();
        assert_eq!(&rows[1..rows.len() - 1], &expected);
    }
    events[DETAIL_MAX].name = "AfterLimit".into();
    assert_eq!(
        detail_rows(&events, Some(&RowFilter::Name("AfterLimit".into())), false),
        [DetailRow::Summary, DetailRow::Empty, DetailRow::Limit]
    );
    assert_eq!(
        detail_rows(&[], None, false),
        [DetailRow::Summary, DetailRow::Empty]
    );
}

#[test]
fn newest_history_orders_events_by_time_across_prompts() {
    let events: Vec<_> = [
        Kind::Tool,
        Kind::User,
        Kind::Tool,
        Kind::Assistant,
        Kind::User,
        Kind::Thinking,
        Kind::Assistant,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| Event {
        start: i as f64,
        end: i as f64 + 1.0,
        kind,
        ..list_event("event", Status::Ok)
    })
    .collect();
    assert_eq!(
        detail_rows(&events, None, true),
        [
            DetailRow::Summary,
            DetailRow::Event(6),
            DetailRow::Event(5),
            DetailRow::Event(4),
            DetailRow::Event(3),
            DetailRow::Event(2),
            DetailRow::Event(1),
            DetailRow::Event(0)
        ]
    );
    assert_eq!(
        replay::rows(&events, 6, None, true),
        [
            DetailRow::Event(5),
            DetailRow::Event(4),
            DetailRow::Event(3),
            DetailRow::Event(2),
            DetailRow::Event(1),
            DetailRow::Event(0)
        ]
    );
}

#[test]
fn single_prompt_history_changes_order_in_normal_and_replay_lists() {
    let events: Vec<_> = [Kind::User, Kind::Thinking, Kind::Tool, Kind::Assistant]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| Event {
            start: i as f64,
            end: i as f64 + 1.0,
            kind,
            ..list_event("event", Status::Ok)
        })
        .collect();
    for replay_visible in [None, Some(events.len())] {
        let state = DetailList::new();
        let key = |reversed| DetailListKey {
            sel: Selection {
                session: 1,
                lane: 0,
                first: 10,
                last: 13,
            },
            generation: 1,
            filter: None,
            reversed,
            width: 400,
            live: false,
            replay_visible,
        };
        let shown_events = || {
            state
                .data
                .borrow()
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .filter_map(|row| {
                    if let DetailRow::Event(i) = row {
                        Some(*i)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        state.prepare(key(false), &events);
        assert_eq!(shown_events(), [0, 1, 2, 3]);
        state.prepare(key(true), &events);
        assert_eq!(shown_events(), [3, 2, 1, 0]);
        state.prepare(key(false), &events);
        assert_eq!(shown_events(), [0, 1, 2, 3]);
    }
}

#[test]
fn detail_list_keeps_event_anchor_on_filter_width_and_live_updates() {
    let events = vec![
        list_event("Read", Status::Ok),
        list_event("Edit", Status::Error),
        list_event("Edit", Status::Ok),
    ];
    let state = DetailList::new();
    let key = |generation, filter, width, last| DetailListKey {
        sel: Selection {
            session: 1,
            lane: 2,
            first: 10,
            last,
        },
        generation,
        filter,
        reversed: false,
        width,
        live: false,
        replay_visible: None,
    };
    state.prepare(key(1, None, 400, 12), &events);
    state.state.scroll_to(ListOffset {
        item_ix: 2,
        offset_in_item: px(17.0),
    });
    state.prepare(
        key(1, Some(RowFilter::Name("Edit".into())), 400, 12),
        &events,
    );
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
    state.prepare(
        key(2, Some(RowFilter::Name("Edit".into())), 320, 13),
        &events,
    );
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
    state.prepare(
        key(2, Some(RowFilter::Name("Read".into())), 320, 13),
        &events,
    );
    assert_eq!(state.state.logical_scroll_top().item_ix, 0);
    state.state.scroll_to(ListOffset {
        item_ix: 1,
        offset_in_item: px(20.0),
    });
    state.reset();
    state.prepare(key(3, None, 400, 12), &events);
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (0, px(0.0)));
}

#[test]
fn reversed_detail_list_keeps_display_limit_and_filters() {
    let mut events: Vec<_> = (0..DETAIL_MAX + 1)
        .map(|_| list_event("Read", Status::Ok))
        .collect();
    events[0] = list_event("Edit", Status::Error);
    events[DETAIL_MAX - 1] = list_event("Edit", Status::Ok);
    events[DETAIL_MAX] = list_event("Edit", Status::Error);
    let rows = detail_rows(&events, None, true);
    assert_eq!(rows.len(), DETAIL_MAX + 2);
    assert_eq!(rows[0], DetailRow::Summary);
    assert_eq!(rows[1], DetailRow::Event(DETAIL_MAX - 1));
    assert_eq!(rows[DETAIL_MAX], DetailRow::Event(0));
    assert_eq!(rows[DETAIL_MAX + 1], DetailRow::Limit);
    assert_eq!(
        detail_rows(&events, Some(&RowFilter::Name("Edit".into())), true),
        [
            DetailRow::Summary,
            DetailRow::Event(DETAIL_MAX - 1),
            DetailRow::Event(0),
            DetailRow::Limit
        ]
    );
    assert_eq!(
        detail_rows(&events, Some(&RowFilter::Failed), true),
        [DetailRow::Summary, DetailRow::Event(0), DetailRow::Limit]
    );
    assert_eq!(
        detail_rows(&events, Some(&RowFilter::Name("Missing".into())), true),
        [DetailRow::Summary, DetailRow::Empty, DetailRow::Limit]
    );
    assert_eq!(
        detail_rows(&[], None, true),
        [DetailRow::Summary, DetailRow::Empty]
    );
}

#[test]
fn detail_list_keeps_event_anchor_when_reversed_and_extended() {
    let mut events = vec![
        list_event("Read", Status::Ok),
        list_event("Edit", Status::Error),
        list_event("Edit", Status::Ok),
    ];
    events[0].kind = Kind::User;
    let state = DetailList::new();
    let key = |generation, last, reversed| DetailListKey {
        sel: Selection {
            session: 1,
            lane: 2,
            first: 10,
            last,
        },
        generation,
        filter: None,
        reversed,
        width: 400,
        live: false,
        replay_visible: None,
    };
    state.prepare(key(1, 12, false), &events);
    state.state.scroll_to(ListOffset {
        item_ix: 1,
        offset_in_item: px(17.0),
    });
    state.prepare(key(1, 12, true), &events);
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (3, px(17.0)));
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows[offset.item_ix],
        DetailRow::Event(0)
    );
    events.push(list_event("Read", Status::Ok));
    state.prepare(key(2, 13, true), &events);
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (4, px(17.0)));
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows[offset.item_ix],
        DetailRow::Event(0)
    );
    state.prepare(key(2, 13, false), &events);
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
}

#[test]
fn replay_list_appends_filtered_events_keeps_scroll_and_rewinds() {
    let events = vec![
        list_event("Read", Status::Ok),
        list_event("Edit", Status::Error),
        list_event("Edit", Status::Ok),
    ];
    let state = DetailList::new();
    let key = |visible, filter| DetailListKey {
        sel: Selection {
            session: 1,
            lane: 2,
            first: 10,
            last: 12,
        },
        generation: 1,
        filter,
        reversed: false,
        width: 400,
        live: false,
        replay_visible: Some(visible),
    };
    let edit = Some(RowFilter::Name("Edit".into()));
    state.prepare(key(1, edit.clone()), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [DetailRow::Empty]
    );
    state.prepare(key(2, edit.clone()), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [DetailRow::Event(1)]
    );
    state.state.scroll_to(ListOffset {
        item_ix: 0,
        offset_in_item: px(17.0),
    });
    state.prepare(key(3, edit.clone()), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [DetailRow::Event(1), DetailRow::Event(2)]
    );
    assert_eq!(state.state.logical_scroll_top().offset_in_item, px(17.0));
    state.prepare(key(1, edit), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [DetailRow::Empty]
    );
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (0, px(0.0)));
    state.prepare(key(1, None), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [DetailRow::Event(0)]
    );
    state.prepare(key(3, None), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [
            DetailRow::Event(0),
            DetailRow::Event(1),
            DetailRow::Event(2)
        ]
    );
}

#[test]
fn reversed_replay_prepends_new_events_and_preserves_the_event_being_read() {
    let events: Vec<_> = [
        Kind::User,
        Kind::Tool,
        Kind::Assistant,
        Kind::User,
        Kind::Tool,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| Event {
        start: i as f64,
        end: i as f64 + 1.0,
        kind,
        ..list_event("event", Status::Ok)
    })
    .collect();
    let state = DetailList::new();
    let key = |visible, reversed| DetailListKey {
        sel: Selection {
            session: 1,
            lane: 2,
            first: 10,
            last: 14,
        },
        generation: 1,
        filter: None,
        reversed,
        width: 400,
        live: false,
        replay_visible: Some(visible),
    };
    state.prepare(key(3, true), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [
            DetailRow::Event(2),
            DetailRow::Event(1),
            DetailRow::Event(0)
        ]
    );
    state.state.scroll_to(ListOffset {
        item_ix: 1,
        offset_in_item: px(17.0),
    });
    state.prepare(key(5, true), &events);
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (3, px(17.0)));
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows[offset.item_ix],
        DetailRow::Event(1)
    );
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows[0],
        DetailRow::Event(4)
    );
    state.prepare(key(5, false), &events);
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (1, px(17.0)));
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows[offset.item_ix],
        DetailRow::Event(1)
    );
    state.scroll_to_latest(false);
    assert_eq!(state.state.logical_scroll_top().item_ix, 5);
    state.prepare(key(5, true), &events);
    state.scroll_to_latest(true);
    let offset = state.state.logical_scroll_top();
    assert_eq!((offset.item_ix, offset.offset_in_item), (0, px(0.0)));
    state.prepare(key(1, true), &events);
    assert_eq!(
        state.data.borrow().as_ref().unwrap().rows,
        [DetailRow::Event(0)]
    );
}

#[test]
fn replay_top_edge_excludes_scrolling_inside_the_first_event() {
    assert!(replay_at_top(ListOffset::default()));
    assert!(replay_at_top(ListOffset {
        item_ix: 0,
        offset_in_item: px(4.0)
    }));
    assert!(!replay_at_top(ListOffset {
        item_ix: 0,
        offset_in_item: px(17.0)
    }));
    assert!(!replay_at_top(ListOffset {
        item_ix: 1,
        offset_in_item: px(0.0)
    }));
}

#[test]
fn resume_quotes_cwd() {
    let s = Session {
        id: "abc".into(),
        source: Source::Claude,
        cwd: "/w/it's here".into(),
        headless: false,
        title: String::new(),
        branch: String::new(),
        version: String::new(),
        lanes: Vec::new(),
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    };
    assert_eq!(
        resume_command(&s),
        r#"cd '/w/it'\''s here' && claude --resume abc"#
    );
    let s = Session {
        source: Source::Codex,
        cwd: "/w".into(),
        ..s
    };
    assert_eq!(resume_command(&s), "cd '/w' && codex resume abc");
}

#[test]
fn short_numbers() {
    assert_eq!(fmt_tokens(9_999), "9,999");
    assert_eq!(fmt_tokens(85_234), "85.2k");
    assert_eq!(fmt_tokens(12_345_678), "12.35M");
    assert_eq!(fmt_tokens(13_230_930_000), "13.23B");
    assert_eq!(fmt_usd(7435.094), "$7,435.09");
    assert_eq!(fmt_usd(0.995), "$1.00");
    assert_eq!(fmt_usd(0.0012), "$0.0012");
    assert_eq!(fmt_usd(0.0), "$0.00");
    assert_eq!(exit_code("Exit code 127\nnot found"), Some("127"));
    assert_eq!(exit_code("[exit 1 · 0.2s]\nerr"), Some("1"));
    assert_eq!(exit_code("[0.2s]\nok"), None);
}
