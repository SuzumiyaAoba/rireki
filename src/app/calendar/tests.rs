//! カレンダーの期間、区間の結合と列配置の参照比較。

use super::*;

fn track(session: usize, lane: usize) -> Track {
    Track {
        session,
        lane,
        depth: 0,
        link: None,
        active: false,
    }
}

fn key() -> Key {
    Key {
        first: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
        days: 7,
        generation: 1,
    }
}

fn seg(start: f64, end: f64, lanes: Vec<usize>) -> GroupSeg {
    GroupSeg {
        start,
        end,
        lanes,
        session: 0,
    }
}

#[test]
fn packs_non_overlapping_into_one_column() {
    let r = pack_columns(&[(0.0, 100.0), (200.0, 300.0), (50.0, 150.0)], 1.0);
    assert_eq!(r, vec![(0, 1, 2), (0, 1, 1), (1, 1, 2)]);
    // 右の列が空いている時間帯のものは広げる
    let r = pack_columns(
        &[(0.0, 300.0), (0.0, 100.0), (50.0, 150.0), (200.0, 260.0)],
        1.0,
    );
    assert_eq!(r, vec![(0, 1, 3), (1, 1, 3), (2, 1, 3), (1, 2, 3)]);
}

#[test]
fn groups_join_lanes_of_the_same_group_and_split_on_idle() {
    let groups = group_segments(
        &[
            (0, 100.0, 200.0),
            (1, 120.0, 300.0),
            (0, 1500.0, 1600.0),
            (0, 3000.0, 3100.0),
            (2, 90.0, 180.0),
        ],
        &[0, 0, 2],
    );
    let spans: Vec<_> = groups
        .iter()
        .map(|g| (g.start, g.end, g.lanes.clone()))
        .collect();
    assert_eq!(
        spans,
        vec![
            (100.0, 300.0, vec![0, 1]),
            (1500.0, 1600.0, vec![0]),
            (3000.0, 3100.0, vec![0]),
            (90.0, 180.0, vec![2])
        ]
    );
}

#[test]
fn day_layout_counts_union_of_active_time_and_root_sessions() {
    let tracks = Rc::new(vec![track(0, 0), track(0, 1), track(1, 0)]);
    let day = DayLayout::new(
        tracks,
        vec![
            seg(100.0, 200.0, vec![0, 1]),
            seg(150.0, 400.0, vec![2]),
            seg(1000.0, 1100.0, vec![0]),
        ],
    );
    assert_eq!(day.active, 400.0);
    assert_eq!(day.sessions, 2);
    assert_eq!(
        day.groups.iter().map(|g| g.session).collect::<Vec<_>>(),
        vec![0, 1, 0]
    );
}

#[test]
fn day_layout_counts_full_representatives_with_aliasing_bits_and_repeated_lanes() {
    for indices in [
        [0, 1, 2, 3, 4, 5, 0],
        [0, 64, usize::MAX, 63, 128, usize::MAX - 64, 0],
    ] {
        let tracks: Rc<Vec<Track>> = Rc::new(
            indices
                .into_iter()
                .enumerate()
                .map(|(i, session)| track(session, usize::from(i == 6)))
                .collect(),
        );
        for order in [
            [0, 6, 1, 2, 3, 4, 5],
            [5, 4, 3, 2, 1, 6, 0],
            [0, 5, 3, 1, 6, 4, 2],
        ] {
            for length in [0, 1, 7, 64, 129, 257] {
                let groups = (0..length)
                    .map(|i| {
                        let lane = order[i % order.len()];
                        seg(
                            i as f64 * 10.0,
                            i as f64 * 10.0 + 5.0,
                            if lane == 0 { vec![0, 2] } else { vec![lane] },
                        )
                    })
                    .collect();
                let day = DayLayout::new(tracks.clone(), groups);
                assert_eq!(
                    day.sessions,
                    if length == 0 {
                        0
                    } else if length == 1 {
                        1
                    } else {
                        6
                    }
                );
                assert_eq!(day.active.to_bits(), (length as f64 * 5.0).to_bits());
                assert!(Rc::ptr_eq(&day.tracks, &tracks));
                for (i, group) in day.groups.iter().enumerate() {
                    assert_eq!(group.session, indices[order[i % order.len()]]);
                    assert_eq!(group.start.to_bits(), (i as f64 * 10.0).to_bits());
                    assert_eq!(group.end.to_bits(), (i as f64 * 10.0 + 5.0).to_bits());
                }
            }
        }
    }
}

#[test]
fn month_sessions_keep_first_interval_order_and_selection_when_clipped() {
    let day = DayLayout::new(
        Rc::new(vec![
            track(9, 0),
            track(7, 0),
            track(9, 1),
            track(usize::MAX, 0),
        ]),
        vec![
            seg(100.0, 200.0, vec![0]),
            seg(50.0, 100.0, vec![1]),
            seg(300.0, 350.0, vec![2]),
            seg(500.0, 600.0, vec![3]),
            seg(100.0, 180.0, vec![1]),
        ],
    );
    assert_eq!(day.sessions, 3);
    assert!(day.month_sessions(0).next().is_none());
    let expected = [
        (7, 50.0_f64, 100.0_f64, vec![1]),
        (9, 100.0, 200.0, vec![0]),
        (usize::MAX, 500.0, 600.0, vec![3]),
    ];
    for limit in [1, 2, 3, usize::MAX] {
        let actual: Vec<_> = day
            .month_sessions(limit)
            .map(|g| {
                (
                    g.session,
                    g.start.to_bits(),
                    g.end.to_bits(),
                    g.lanes.clone(),
                )
            })
            .collect();
        let expected: Vec<_> = expected
            .iter()
            .take(limit)
            .map(|(si, start, end, lanes)| (*si, start.to_bits(), end.to_bits(), lanes.clone()))
            .collect();
        assert_eq!(actual, expected);
    }
    let first = day.month_sessions(2).collect::<Vec<_>>();
    assert!(std::ptr::eq(first[0], &day.groups[0]));
    assert!(std::ptr::eq(first[1], &day.groups[2]));
}

#[test]
fn layout_reuses_only_the_same_dates_and_data() {
    let mut cache = Cache::default();
    let key = key();
    let layout = cache.set(
        key,
        vec![DayLayout::new(Rc::new(vec![track(0, 0)]), vec![])],
        |_| "",
    );
    assert!(Rc::ptr_eq(&layout, &cache.get(key).unwrap()));
    assert!(
        std::mem::take(&mut cache.seek),
        "新しい期間では作業のある時刻を探す"
    );
    for changed in [
        Key {
            first: key.first + Duration::days(7),
            ..key
        },
        Key { days: 1, ..key },
        Key {
            generation: key.generation + 1,
            ..key
        },
    ] {
        assert!(cache.get(changed).is_none());
    }
    cache.set(
        Key {
            generation: 2,
            ..key
        },
        Vec::new(),
        |_| "",
    );
    assert!(
        !cache.seek,
        "同じ期間のデータ更新ではスクロールを動かさない"
    );
    let old = Rc::downgrade(&layout);
    drop(layout);
    cache.set(Key { days: 1, ..key }, Vec::new(), |_| "");
    assert!(old.upgrade().is_none(), "以前の期間を保持し続けない");
}

#[test]
fn counts_and_legend_cover_only_sessions_with_activity() {
    let mut cache = Cache::default();
    let tracks = Rc::new(vec![track(0, 0), track(1, 0), track(2, 0)]);
    let layout = cache.set(
        key(),
        vec![
            DayLayout::new(
                tracks.clone(),
                vec![seg(0.0, 3600.0, vec![0]), seg(1800.0, 5400.0, vec![1])],
            ),
            DayLayout::new(tracks, vec![seg(0.0, 600.0, vec![0])]),
        ],
        |si| if si == 1 { "/b" } else { "/a" },
    );
    assert_eq!(layout.counts, "2 セッション · 稼働 1h40m");
    assert_eq!(
        layout.projects,
        vec![("/a".to_string(), 4200.0), ("/b".to_string(), 3600.0)]
    );
    assert!(!layout.empty);
    assert!(cache.set(key(), Vec::new(), |_| "").empty);
}

#[test]
fn counts_keep_distinct_high_indices_and_merge_repeated_lanes_and_days() {
    let tracks = Rc::new(vec![
        track(0, 0),
        track(1, 0),
        track(1, 1),
        track(usize::MAX / 2 + 1, 0),
        track(usize::MAX - 1, 0),
        track(usize::MAX, 0),
    ]);
    let days = (0..2)
        .map(|_| {
            let groups = (0..tracks.len())
                .map(|i| seg(i as f64 * 120.0, i as f64 * 120.0 + 60.0, vec![i]))
                .collect();
            DayLayout::new(tracks.clone(), groups)
        })
        .collect();
    let layout = Cache::default().set(key(), days, |_| "/all");
    assert_eq!(layout.counts, "5 セッション · 稼働 12m");
    assert_eq!(layout.projects, vec![("/all".to_string(), 720.0)]);
    assert!(layout.days.iter().all(|day| day.sessions == 5));
    assert!(!layout.empty);
}

#[test]
fn zoom_reuses_segments_and_recalculates_columns_with_minimum_height() {
    let mut cache = Cache::default();
    let tracks = Rc::new(vec![track(0, 0), track(1, 0)]);
    let layout = cache.set(
        key(),
        vec![DayLayout::new(
            tracks,
            vec![seg(100.0, 200.0, vec![0]), seg(300.0, 400.0, vec![1])],
        )],
        |_| "",
    );
    assert_eq!(cache.columns(1.0)[0], vec![(0, 1, 1), (0, 1, 1)]);
    // 縮小すると最小の高さで重なるので横に並べる
    assert_eq!(cache.columns(10.0)[0], vec![(0, 1, 2), (1, 1, 2)]);
    assert!(Rc::ptr_eq(&layout, &cache.get(key()).unwrap()));
}

#[test]
fn legend_owns_names_and_preserves_equal_names_ties_and_addition_order() {
    let mut names = vec!["/b".to_string(), "/b".to_string(), "/a".to_string()];
    assert_ne!(names[0].as_ptr(), names[1].as_ptr());
    let tracks = Rc::new(vec![track(0, 0), track(1, 0), track(2, 0)]);
    let expected = (0.1_f64 + 0.2) + 0.4;
    let layout = Cache::default().set(
        key(),
        vec![
            DayLayout::new(
                tracks.clone(),
                vec![seg(0.0, 0.1, vec![0]), seg(0.0, expected, vec![2])],
            ),
            DayLayout::new(tracks.clone(), vec![seg(0.0, 0.2, vec![1])]),
            DayLayout::new(tracks, vec![seg(0.0, 0.4, vec![0])]),
        ],
        |si| names[si].as_str(),
    );
    names.clear();
    assert_eq!(
        layout
            .projects
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["/a", "/b"]
    );
    assert!(
        layout
            .projects
            .iter()
            .all(|(_, active)| active.to_bits() == expected.to_bits())
    );
    assert_ne!(expected.to_bits(), (0.1_f64 + 0.4 + 0.2).to_bits());
}

#[test]
fn month_grid_starts_on_the_monday_before_the_first() {
    let d = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
    assert_eq!(month_grid_start(d(10, 17)), d(9, 28));
    assert_eq!(month_grid_start(d(6, 30)), d(6, 1));
    assert_eq!(fmt_hm(45.0 * 60.0), "45m");
    assert_eq!(fmt_hm(3.0 * 3600.0 + 12.0 * 60.0), "3h12m");
}

#[test]
fn daily_segments_preserve_midnight_boundaries_overlaps_and_idle_gaps() {
    use crate::model::{Event, Lane, Session, Source};
    let mut lane = Lane::new("main");
    for (start, end) in [
        (-100.0, 0.0),
        (100.0, 200.0),
        (2000.0, 90_000.0),
        (2500.0, 2600.0),
        (86_400.0, 87_000.0),
        (200_000.0, 200_100.0),
    ] {
        lane.events.push(Event {
            start,
            end,
            kind: Kind::Tool,
            name: "exec_command".into(),
            summary: Default::default(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
            usage: Default::default(),
            model: None,
            status: Default::default(),
        });
    }
    let mut session = Session {
        id: String::new(),
        source: Source::Codex,
        cwd: String::new(),
        headless: false,
        title: String::new(),
        branch: String::new(),
        version: String::new(),
        lanes: vec![lane, Lane::new("empty")],
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    };
    session.finalize();
    let sessions = [std::sync::Arc::new(session)];
    let tracks = [track(0, 0), track(0, 1)];
    assert_eq!(
        day_segments(&sessions, &tracks, 0.0, 86_400.0),
        [(0, 0.0, 200.0), (0, 2000.0, 86_400.0)]
    );
    assert_eq!(
        day_segments(&sessions, &tracks, 86_400.0, 172_800.0),
        [(0, 86_400.0, 90_000.0)]
    );
    assert!(day_segments(&sessions, &tracks, 200_100.1, 300_000.0).is_empty());
    assert!(day_segments(&sessions, &tracks, -1000.0, -100.0).is_empty());
    assert_eq!(
        day_segments(&sessions, &tracks, 200_100.0, 300_000.0),
        [(0, 200_100.0, 200_100.0)]
    );
}

#[test]
fn track_day_ranges_preserve_direct_boundary_predicates() {
    let ranges = [
        None,
        Some((-10.0, 0.0)),
        Some((0.0, 0.0)),
        Some((0.0, 10.0)),
        Some((10.0, 10.0)),
        Some((10.0, 20.0)),
        Some((20.0, 30.0)),
        Some((30.0, 40.0)),
        Some((-10.0, 40.0)),
        Some((20.0, 10.0)),
        Some((f64::NEG_INFINITY, f64::INFINITY)),
        Some((f64::INFINITY, f64::INFINITY)),
        Some((f64::NEG_INFINITY, f64::NEG_INFINITY)),
        Some((f64::NAN, 10.0)),
        Some((10.0, f64::NAN)),
        Some((f64::NAN, f64::NAN)),
    ];
    for edges in [
        vec![],
        vec![0.0],
        vec![0.0, 10.0, 20.0, 30.0],
        vec![0.0, 10.0, 10.0, 20.0],
        vec![20.0, 10.0, 0.0],
        vec![10.0, 0.0, 30.0],
        vec![0.0, f64::NAN, 20.0],
        vec![f64::NEG_INFINITY, 0.0, f64::INFINITY],
    ] {
        let spans = track_days(&edges, ranges.iter().copied());
        let sparse = sparse_track_days(&edges, ranges.iter().copied());
        let ordered = edges.windows(2).all(|pair| pair[0] <= pair[1]);
        for (ti, &range) in ranges.iter().enumerate() {
            let (start, end) = spans[ti];
            assert!(start <= end && end <= edges.len().saturating_sub(1));
            for (day, pair) in edges.windows(2).enumerate() {
                let expected = range.is_some_and(|(a, b)| !(b < pair[0] || a >= pair[1]));
                let selected = start <= day && day < end;
                if ordered {
                    assert_eq!(selected, expected, "{ti} {day} {edges:?}");
                } else {
                    assert!(!expected || selected);
                }
                let filtered = sparse.as_ref().is_none_or(|spans| {
                    let (start, end) = spans[ti];
                    start <= day && day < end
                });
                assert_eq!(filtered && expected, expected);
            }
        }
    }
    assert!(sparse_track_days(&[0.0, 10.0, 20.0], [Some((-1.0, 30.0)); 8].into_iter()).is_none());
    assert!(
        sparse_track_days(
            &[0.0, 10.0, 20.0, 30.0],
            [Some((1.0, 2.0)), Some((21.0, 22.0))].into_iter()
        )
        .is_some()
    );
}

#[test]
fn selected_day_tracks_preserve_indices_midnight_points_and_sparse_events() {
    use crate::model::{Event, Lane, Session, Source};
    let lane = |name, ranges: &[(f64, f64)]| {
        let mut lane = Lane::new(name);
        for &(start, end) in ranges {
            lane.events.push(Event {
                start,
                end,
                kind: Kind::Tool,
                name: "exec_command".into(),
                summary: Default::default(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: Default::default(),
                usage: Default::default(),
                model: None,
                status: Default::default(),
            });
        }
        lane
    };
    let mut session = Session {
        id: String::new(),
        source: Source::Codex,
        cwd: String::new(),
        headless: false,
        title: String::new(),
        branch: String::new(),
        version: String::new(),
        lanes: vec![
            lane(
                "main",
                &[
                    (-100.0, 0.0),
                    (100.0, 200.0),
                    (2000.0, 90_000.0),
                    (2500.0, 2600.0),
                    (86_400.0, 87_000.0),
                    (200_000.0, 200_100.0),
                ],
            ),
            lane("point", &[(86_400.0, 86_400.0), (300_000.0, 350_000.0)]),
            lane("empty", &[]),
        ],
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    };
    session.finalize();
    let sessions = [std::sync::Arc::new(session)];
    let tracks = [track(0, 2), track(0, 1), track(0, 0)];
    let edges = [0.0, 86_400.0, 172_800.0, 259_200.0, 345_600.0, 432_000.0];
    let spans = track_days(
        &edges,
        tracks
            .iter()
            .map(|t| sessions[t.session].lanes[t.lane].range()),
    );
    let bits = |segments: Vec<(usize, f64, f64)>| {
        segments
            .into_iter()
            .map(|(ti, a, b)| (ti, a.to_bits(), b.to_bits()))
            .collect::<Vec<_>>()
    };
    for (day, pair) in edges.windows(2).enumerate() {
        let selected = spans
            .iter()
            .enumerate()
            .filter_map(|(ti, &(start, end))| (start <= day && day < end).then_some(ti));
        assert_eq!(
            bits(indexed_segments(
                &sessions, &tracks, selected, pair[0], pair[1]
            )),
            bits(day_segments(&sessions, &tracks, pair[0], pair[1]))
        );
    }
    assert_eq!(
        day_segments(&sessions, &tracks, 86_400.0, 172_800.0),
        [(1, 86_400.0, 86_400.0), (2, 86_400.0, 90_000.0)]
    );
}

#[cfg(test)]
mod bounds_tests {
    use super::segment_bounds;
    use crate::model::{Event, Kind, Lane, Session, Source};

    fn lane(ranges: &[(f64, f64)]) -> Lane {
        let mut lane = Lane::new("main");
        lane.events = ranges
            .iter()
            .map(|&(start, end)| Event {
                start,
                end,
                kind: Kind::Tool,
                name: "exec_command".into(),
                summary: Default::default(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: Default::default(),
                usage: Default::default(),
                model: None,
                status: Default::default(),
            })
            .collect();
        lane.finish();
        let mut session = Session {
            id: String::new(),
            source: Source::Codex,
            cwd: String::new(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        session.lanes.pop().unwrap()
    }

    #[test]
    fn bounds_keep_overlaps_inclusive_ends_and_exclusive_starts() {
        let lane = lane(&[(-5.0, 0.0), (0.0, 20.0), (10.0, 10.0), (30.0, 31.0)]);
        for (ds, de, expected) in [
            (0.0, 40.0, (0, 4)),
            (0.0, 30.0, (0, 3)),
            (1.0, 11.0, (1, 3)),
            (20.0, 30.0, (1, 3)),
            (21.0, 30.0, (3, 3)),
            (31.0, 40.0, (3, 4)),
            (32.0, 40.0, (4, 4)),
            (f64::NEG_INFINITY, f64::INFINITY, (0, 4)),
            (f64::NAN, 100.0, (0, 4)),
            (20.0, f64::NAN, (1, 0)),
            (40.0, 0.0, (4, 1)),
        ] {
            assert_eq!(segment_bounds(&lane, ds, de), expected, "{ds:?} {de:?}");
        }
        assert_eq!(segment_bounds(&Lane::new("empty"), 0.0, 1.0), (0, 0));
    }

    #[test]
    fn bounds_keep_original_search_for_nan_starts_in_sorted_events() {
        for ranges in [
            vec![
                (-f64::NAN, f64::NAN),
                (-f64::NAN, f64::NAN),
                (-f64::NAN, f64::NAN),
                (1.0, 2.0),
            ],
            vec![(0.0, 1.0), (f64::NAN, f64::NAN)],
            vec![(-f64::NAN, 2.0), (0.0, 3.0), (f64::NAN, 4.0)],
        ] {
            let lane = lane(&ranges);
            for (ds, de) in [
                (0.0, 10.0),
                (-1.0, 1.0),
                (f64::NEG_INFINITY, f64::INFINITY),
                (f64::NAN, f64::NAN),
            ] {
                let expected = (
                    lane.cum_end.partition_point(|&end| end < ds),
                    lane.events.partition_point(|e| e.start < de),
                );
                assert_eq!(segment_bounds(&lane, ds, de), expected);
            }
        }
    }
}

#[cfg(test)]
mod group_sort_tests {
    use super::group_segments;

    #[test]
    fn groups_keep_key_order_signed_zero_idle_boundary_and_unique_sorted_lanes() {
        let segments = [
            (2, 100.0, 200.0),
            (0, 0.0, 5.0),
            (1, 5.0, 10.0),
            (0, 900.0, 910.0),
            (0, 5000.0, 5010.0),
            (1, -0.0, 20.0),
        ];
        let grouped = group_segments(&segments, &[17, 17, 0]);
        let actual: Vec<_> = grouped
            .iter()
            .map(|g| (g.start.to_bits(), g.end.to_bits(), g.lanes.clone()))
            .collect();
        assert_eq!(
            actual,
            vec![
                (100.0_f64.to_bits(), 200.0_f64.to_bits(), vec![2]),
                ((-0.0_f64).to_bits(), 910.0_f64.to_bits(), vec![0, 1]),
                (5000.0_f64.to_bits(), 5010.0_f64.to_bits(), vec![0])
            ]
        );
        assert!(grouped.iter().all(|g| g.session == 0));
        assert!(group_segments(&[], &[]).is_empty());
    }

    #[test]
    fn large_few_group_inputs_keep_ordered_and_reversed_segments_with_high_keys() {
        let group = [usize::MAX; 256];
        let mut segments: Vec<_> = (0..256)
            .map(|ti| (ti, ti as f64 * 1000.0, ti as f64 * 1000.0 + 10.0))
            .collect();
        for reverse in [false, true] {
            if reverse {
                segments.reverse();
            }
            let result = group_segments(&segments, &group);
            assert_eq!(result.len(), 256);
            for (ti, g) in result.iter().enumerate() {
                assert_eq!(
                    (g.start, g.end, g.session),
                    (ti as f64 * 1000.0, ti as f64 * 1000.0 + 10.0, 0)
                );
                assert_eq!(g.lanes, [ti]);
            }
        }
    }
}

#[cfg(test)]
mod lane_membership_tests {
    use super::group_segments;

    #[test]
    fn grouped_lanes_keep_colliding_indices_and_remove_repeats() {
        for count in [1usize, 31, 63, 64, 65, 257] {
            let lanes: Vec<_> = (0..count).map(|i| i * 64).collect();
            let group = vec![usize::MAX; lanes.last().copied().unwrap() + 1];
            let segments: Vec<_> = lanes
                .iter()
                .rev()
                .flat_map(|&ti| [(ti, 0.0, 10.0), (ti, 0.0, 10.0)])
                .collect();
            let result = group_segments(&segments, &group);
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].lanes, lanes);
            assert_eq!(
                (result[0].start, result[0].end, result[0].session),
                (0.0, 10.0, 0)
            );
        }
    }

    #[test]
    fn grouped_lanes_reset_membership_at_idle_gaps_and_group_boundaries() {
        for keys in [1usize, 8, 17] {
            for count in [63usize, 64, 65, 257] {
                let group: Vec<_> = (0..count * keys * 64).map(|i| (i / 64) % keys).collect();
                let mut segments = Vec::new();
                for start in [0.0, 1010.0] {
                    for i in (0..count * keys).rev() {
                        segments.extend([(i * 64, start, start + 10.0); 2]);
                    }
                }
                let result = group_segments(&segments, &group);
                assert_eq!(result.len(), keys * 2);
                for key in 0..keys {
                    let lanes: Vec<_> = (0..count).map(|i| (i * keys + key) * 64).collect();
                    for (phase, start) in [0.0, 1010.0].into_iter().enumerate() {
                        let g = &result[key * 2 + phase];
                        assert_eq!(g.lanes, lanes, "{keys} {count} {key} {phase}");
                        assert_eq!((g.start, g.end, g.session), (start, start + 10.0, 0));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod column_index_tests {
    use super::pack_columns;
    fn pack_columns_reference(spans: &[(f64, f64)], spp: f64) -> Vec<(usize, usize, usize)> {
        let pad = 8.0 * spp;
        let mut order: Vec<usize> = (0..spans.len()).collect();
        order.sort_by(|&a, &b| spans[a].0.total_cmp(&spans[b].0).then(a.cmp(&b)));
        let mut out = vec![(0, 1, 1); spans.len()];
        let mut i = 0;
        while i < order.len() {
            // 重なりの連鎖（クラスタ）ごとに列を割り当てる
            let mut cols: Vec<f64> = Vec::new();
            let mut members = Vec::new();
            let mut cluster_end = f64::MIN;
            while i < order.len() {
                let k = order[i];
                let (s, e) = spans[k];
                if !members.is_empty() && s >= cluster_end + pad {
                    break;
                }
                let e = e.max(s + pad);
                let col = match cols.iter().position(|&ce| ce + pad <= s) {
                    Some(c) => {
                        cols[c] = e;
                        c
                    }
                    None => {
                        cols.push(e);
                        cols.len() - 1
                    }
                };
                out[k].0 = col;
                members.push(k);
                cluster_end = cluster_end.max(e);
                i += 1;
            }
            let n = cols.len();
            for &k in &members {
                out[k].2 = n;
            }
            for &k in &members {
                let (s, e) = (spans[k].0, spans[k].1.max(spans[k].0 + pad));
                let blocked = |c: usize| {
                    members.iter().any(|&o| {
                        o != k
                            && out[o].0 == c
                            && spans[o].0 < e + pad
                            && s < spans[o].1.max(spans[o].0 + pad) + pad
                    })
                };
                out[k].1 = (out[k].0 + 1..n).take_while(|&c| !blocked(c)).count() + 1;
            }
        }
        out
    }

    #[test]
    fn indexed_columns_choose_the_leftmost_free_column_at_the_exact_end() {
        let mut spans: Vec<_> = (0..64)
            .map(|i| (0.0, if i == 0 { 10000.0 } else { 1000.0 - i as f64 }))
            .collect();
        spans.extend([(1100.0, 1101.0), (1110.0, 1111.0), (1116.0, 1117.0)]);
        let actual = pack_columns(&spans, 1.0);
        assert_eq!(actual, pack_columns_reference(&spans, 1.0));
        assert_eq!(&actual[64..], &[(1, 1, 64), (2, 62, 64), (1, 1, 64)]);
    }

    #[test]
    fn indexed_columns_keep_nan_infinity_signed_zero_and_unusual_padding() {
        for count in [0usize, 1, 31, 32, 33, 63, 64, 65, 128] {
            for spp in [
                -1.0,
                -0.0,
                0.0,
                0.125,
                1.0,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NAN,
            ] {
                for shape in 0..4 {
                    let spans: Vec<_> = (0..count)
                        .map(|i| {
                            let s = match shape {
                                0 => {
                                    if i % 2 == 0 {
                                        -0.0
                                    } else {
                                        0.0
                                    }
                                }
                                1 => {
                                    if i % 5 == 0 {
                                        -f64::NAN
                                    } else {
                                        i as f64
                                    }
                                }
                                2 => {
                                    if i % 5 == 0 {
                                        f64::NAN
                                    } else {
                                        i as f64
                                    }
                                }
                                _ => {
                                    if i % 2 == 0 {
                                        f64::NEG_INFINITY
                                    } else {
                                        f64::INFINITY
                                    }
                                }
                            };
                            let e = if i % 3 == 0 { f64::NAN } else { s + 50.0 };
                            (s, e)
                        })
                        .collect();
                    assert_eq!(
                        pack_columns(&spans, spp),
                        pack_columns_reference(&spans, spp),
                        "{count} {spp} {shape}"
                    );
                }
            }
        }
    }

    #[test]
    fn indexed_columns_reset_all_working_ranges_between_clusters() {
        let mut spans = vec![(0.0, 10.0); 64];
        spans.extend((1..=64).map(|i| (i as f64 * 1000.0, i as f64 * 1000.0 + 10.0)));
        spans.extend([(1000000.0, 1000010.0); 128]);
        let actual = pack_columns(&spans, 0.125);
        assert_eq!(actual, pack_columns_reference(&spans, 0.125));
        for (i, &v) in actual[..64].iter().enumerate() {
            assert_eq!(v, (i, 1, 64));
        }
        assert!(actual[64..128].iter().all(|&v| v == (0, 1, 1)));
        for (i, &v) in actual[128..].iter().enumerate() {
            assert_eq!(v, (i, 1, 128));
        }
    }
    #[test]
    fn padded_disjoint_columns_keep_exact_boundaries_and_clamped_ends() {
        for count in [2usize, 63, 64, 65] {
            let spans: Vec<_> = (0..count)
                .map(|i| {
                    let start = i as f64 * 16.0;
                    let end = if i + 1 == count {
                        f64::INFINITY
                    } else if i % 3 == 0 {
                        f64::NAN
                    } else {
                        start - 10.0
                    };
                    (start, end)
                })
                .collect();
            let actual = pack_columns(&spans, 1.0);
            assert_eq!(actual, vec![(0, 1, 1); count]);
            assert_eq!(actual, pack_columns_reference(&spans, 1.0));
        }
        let spans = [(0.0, 8.0), (16.0, 16.0)];
        assert_eq!(pack_columns(&spans, 1.0), vec![(0, 1, 1); 2]);
        let spans = [(0.0, 8.5), (16.0, 16.0)];
        assert_eq!(pack_columns(&spans, 1.0), vec![(0, 1, 2), (1, 1, 2)]);
        assert_eq!(
            pack_columns(&spans, 1.0),
            pack_columns_reference(&spans, 1.0)
        );
    }

    #[test]
    fn wide_columns_keep_strict_overlap_boundaries_after_switching_queries() {
        let mut spans = vec![(0.0, 5.0); 128];
        spans[0].1 = 1e9;
        spans.extend((0..256).map(|i| {
            let start = 100.0 + i as f64 * 10.0;
            (start, start + 1.0)
        }));
        let start = 2660.0;
        spans.extend([
            (start, start),
            (start, start + 1.0),
            (start + 2.0, start + 3.0),
            (start + 4.0, 1e8),
            (start + 4.0, start + 5.0),
        ]);
        spans.extend((0..512).map(|i| {
            let start = 500000.0 + i as f64 * 10.0;
            (start, start + 1.0)
        }));
        // The empty columns exhaust the scan budget before record 386.
        let actual = pack_columns(&spans, 0.125);
        assert_eq!(actual, pack_columns_reference(&spans, 0.125));
        // Both the old padded end and the future start touch this record.
        assert_eq!(actual[386], (1, 127, 128));
        spans.reverse();
        assert_eq!(
            pack_columns(&spans, 0.125),
            pack_columns_reference(&spans, 0.125)
        );
    }

    #[test]
    fn wide_column_queries_reset_the_tree_between_different_cluster_sizes() {
        let mut spans = Vec::new();
        for (start, columns) in [(0.0, 64), (1000000.0, 129)] {
            let first = spans.len();
            spans.extend(std::iter::repeat_n((start, start + 5.0), columns));
            spans[first].1 = start + 100000.0;
            spans.extend((0..256).map(|i| {
                let s = start + 100.0 + i as f64 * 10.0;
                (
                    s,
                    s + if columns == 129 && i % 7 == 0 {
                        1000.0
                    } else {
                        1.0
                    },
                )
            }));
        }
        spans.extend([(2000000.0, 2000001.0); 4]);
        for reversed in [false, true] {
            if reversed {
                spans.reverse();
            }
            assert_eq!(
                pack_columns(&spans, 0.125),
                pack_columns_reference(&spans, 0.125)
            );
        }
    }

    #[test]
    fn equal_starts_only_skip_expansion_when_the_minimum_height_overlaps() {
        let spans: Vec<_> = (0..192)
            .map(|i| {
                (
                    if i % 2 == 0 { -0.0 } else { 0.0 },
                    if i % 3 == 0 { 100.0 } else { 0.0 },
                )
            })
            .collect();
        for spp in [-0.0, 0.0, 0.125, 1.0] {
            let actual = pack_columns(&spans, spp);
            assert_eq!(actual, pack_columns_reference(&spans, spp));
            if spp > 0.0 {
                assert!(
                    actual
                        .iter()
                        .all(|&(_, width, columns)| width == 1 && columns == spans.len())
                );
            } else {
                assert!(actual.iter().any(|&(_, width, _)| width > 1));
            }
        }
        // A positive pad can round away at a large finite start.
        let spans: Vec<_> = (0..192)
            .map(|i| (1e300, if i % 3 == 0 { 2e300 } else { 1e300 }))
            .collect();
        for spp in [0.125, 1.0] {
            let actual = pack_columns(&spans, spp);
            assert_eq!(actual, pack_columns_reference(&spans, spp));
            assert!(actual.iter().any(|&(_, width, _)| width > 1));
        }
    }

    #[test]
    fn narrow_columns_keep_widths_when_ends_are_ordered_and_clusters_reset() {
        for count in [127usize, 128, 512] {
            let mut spans = Vec::new();
            for (base, columns) in [(0.0, 7), (1000000.0, 2), (2000000.0, 31)] {
                spans.extend((0..count).map(|i| {
                    let start = base + i as f64 * 4.0;
                    let duration = 5.0f64.max((columns - 1) as f64 * 4.0 + 1.0 - i as f64 * 0.25);
                    (start, start + duration)
                }));
            }
            for spp in [0.0, 0.125, 1.0] {
                let actual = pack_columns(&spans, spp);
                assert_eq!(actual, pack_columns_reference(&spans, spp), "{count} {spp}");
                if count == 512 && spp == 0.125 {
                    assert!(actual[..count].iter().any(|&(_, width, _)| width > 1));
                }
            }
            spans.reverse();
            assert_eq!(
                pack_columns(&spans, 0.125),
                pack_columns_reference(&spans, 0.125)
            );
        }
        let mut ordered: Vec<_> = (0..4096)
            .map(|i| {
                let start = i as f64 * 4.0;
                (start, start + 5.0f64.max(25.0 - i as f64 * 0.25))
            })
            .collect();
        for reversed in [false, true] {
            if reversed {
                ordered.reverse();
            }
            let actual = pack_columns(&ordered, 0.125);
            assert_eq!(actual, pack_columns_reference(&ordered, 0.125));
            assert!(actual.iter().any(|&(_, width, _)| width > 1));
        }
        // Unordered effective ends must retain the indexed lookup.
        let mut spans: Vec<_> = (0..512)
            .map(|i| {
                let start = i as f64 * 10.0;
                (start, start + if i % 7 == 0 { 100.0 } else { 15.0 })
            })
            .collect();
        for reversed in [false, true] {
            if reversed {
                spans.reverse();
            }
            assert_eq!(
                pack_columns(&spans, 0.125),
                pack_columns_reference(&spans, 0.125)
            );
        }
    }
}
