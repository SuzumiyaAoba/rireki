//! ディレクトリごとのメインイベントを、重なりのない最小限の行に詰める。

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use super::{Block, Cache, Granularity};
use crate::model::Session;

#[derive(Clone, Debug)]
pub struct SessionBlock {
    pub session: usize,
    pub block: Block,
    /// 描画する矩形の位置と幅。最小幅・表示範囲での切り取りも衝突判定に含める。
    pub x: f32,
    pub width: f32,
}

pub struct Group {
    pub cwd: String,
    pub sessions: Vec<usize>,
    pub rows: Vec<Vec<SessionBlock>>,
}

#[derive(Clone, Copy)]
pub struct View {
    pub range: (f64, f64),
    pub spp: f64,
    pub gran: Granularity,
    pub min_width: f32,
    pub in_view_only: bool,
}

/// 読み込み元・プロジェクトで絞り込んだ order を使い、子セッションと子レーンは並べない。
pub fn groups(
    cache: &mut Cache,
    sessions: &[Arc<Session>],
    order: &[usize],
    exec_parent: &HashMap<usize, (usize, usize, f64)>,
    view: View,
) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut by_cwd = HashMap::new();
    let (v0, v1) = view.range;
    let plot_width = ((v1 - v0) / view.spp) as f32;
    for &si in order {
        if exec_parent.contains_key(&si) {
            continue;
        }
        let session = &sessions[si];
        let Some(lane) = session.lanes.first() else {
            continue;
        };
        let blocks = if session.end < v0 || session.start > v1 {
            Vec::new()
        } else {
            cache.aggregate_main(si, lane, view.range, view.spp, view.gran)
        };
        if view.in_view_only && blocks.is_empty() {
            continue;
        }
        let index = *by_cwd.entry(session.cwd.as_str()).or_insert_with(|| {
            groups.push(Group {
                cwd: session.cwd.clone(),
                sessions: Vec::new(),
                rows: vec![Vec::new()],
            });
            groups.len() - 1
        });
        let group = &mut groups[index];
        group.sessions.push(si);
        group.rows[0].extend(blocks.into_iter().map(|block| {
            let x = (((block.start - v0) / view.spp) as f32).max(-2.0);
            let right = (((block.end - v0) / view.spp) as f32).min(plot_width + 2.0);
            SessionBlock {
                session: si,
                block,
                x,
                width: (right - x).max(view.min_width),
            }
        }));
    }
    for group in &mut groups {
        group.rows = pack(group.rows.pop().unwrap());
    }
    groups
}

#[derive(PartialEq)]
struct Occupied {
    end: f32,
    row: usize,
}

impl Eq for Occupied {}

impl PartialOrd for Occupied {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Occupied {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .end
            .total_cmp(&self.end)
            .then_with(|| other.row.cmp(&self.row))
    }
}

fn pack(mut blocks: Vec<SessionBlock>) -> Vec<Vec<SessionBlock>> {
    blocks.sort_unstable_by(|a, b| {
        a.x.total_cmp(&b.x)
            .then_with(|| a.block.start.total_cmp(&b.block.start))
            .then_with(|| a.session.cmp(&b.session))
            .then_with(|| a.block.first.cmp(&b.block.first))
    });
    let mut rows: Vec<Vec<SessionBlock>> = Vec::new();
    let mut occupied: BinaryHeap<Occupied> = BinaryHeap::new();
    let mut free = BinaryHeap::new();
    for block in blocks {
        while occupied.peek().is_some_and(|r| r.end <= block.x) {
            free.push(Reverse(occupied.pop().unwrap().row));
        }
        // 空いた最上段を再利用する。必要な行数は矩形の最大同時重なり数に等しい。
        let row = free.pop().map_or_else(
            || {
                rows.push(Vec::new());
                rows.len() - 1
            },
            |Reverse(row)| row,
        );
        occupied.push(Occupied {
            end: block.x + block.width,
            row,
        });
        rows[row].push(block);
    }
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Kind, Lane, Source};

    fn event(start: f64, end: f64, kind: Kind) -> Event {
        Event {
            start,
            end,
            kind,
            name: format!("{kind:?}").into(),
            summary: Default::default(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
            usage: Default::default(),
            model: None,
            status: Default::default(),
        }
    }

    fn session(cwd: &str, events: Vec<Event>, children: Vec<Event>) -> Arc<Session> {
        let mut main = Lane::new("main");
        main.events = events;
        main.finish();
        let mut child = Lane::new("child");
        child.events = children;
        child.finish();
        let mut session = Session {
            id: String::new(),
            source: Source::Codex,
            cwd: cwd.into(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![main, child],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        Arc::new(session)
    }

    fn view() -> View {
        View {
            range: (0.0, 1000.0),
            spp: 1.0,
            gran: Granularity::Call,
            min_width: 3.0,
            in_view_only: false,
        }
    }

    fn rect(session: usize, x: f32, width: f32) -> SessionBlock {
        SessionBlock {
            session,
            block: Block {
                start: x as f64,
                end: (x + width) as f64,
                first: 0,
                last: 0,
            },
            x,
            width,
        }
    }

    #[test]
    fn sequential_sessions_share_one_row_and_gaps_are_reused() {
        let sessions = vec![
            session(
                "a",
                vec![event(0.0, 10.0, Kind::User), event(40.0, 50.0, Kind::Tool)],
                vec![],
            ),
            session("a", vec![event(20.0, 30.0, Kind::Tool)], vec![]),
            session("b", vec![event(5.0, 25.0, Kind::User)], vec![]),
        ];
        let result = groups(
            &mut Cache::default(),
            &sessions,
            &[0, 2, 1],
            &HashMap::new(),
            view(),
        );
        assert_eq!(
            result.iter().map(|g| g.cwd.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(result[0].sessions, [0, 1]);
        assert_eq!(result[0].rows.len(), 1);
        assert_eq!(
            result[0].rows[0].iter().map(|b| b.session).collect::<Vec<_>>(),
            [0, 1, 0]
        );
        assert_eq!(result[0].rows[0][2].block.first, 1);
    }

    #[test]
    fn parallel_blocks_use_the_lowest_available_row() {
        let result = pack(vec![
            rect(3, 6.0, 1.0),
            rect(2, 5.0, 1.0),
            rect(1, 1.0, 9.0),
            rect(0, 0.0, 4.0),
        ]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].iter().map(|b| b.session).collect::<Vec<_>>(), [0, 2, 3]);
        assert_eq!(result[1].iter().map(|b| b.session).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn packing_matches_maximum_overlap_without_colliding_rectangles() {
        let mut seed = 19u64;
        for count in [1, 10, 100, 1000] {
            let blocks: Vec<_> = (0..count)
                .map(|i| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    rect(i, (seed % 100) as f32, ((seed >> 32) % 50 + 1) as f32)
                })
                .collect();
            let max_overlap = blocks
                .iter()
                .map(|b| {
                    blocks
                        .iter()
                        .filter(|other| other.x <= b.x && b.x < other.x + other.width)
                        .count()
                })
                .max()
                .unwrap();
            let result = pack(blocks);
            assert_eq!(result.len(), max_overlap);
            assert_eq!(result.iter().map(Vec::len).sum::<usize>(), count);
            for row in result {
                assert!(row.windows(2).all(|pair| pair[0].x + pair[0].width <= pair[1].x));
            }
        }
    }

    #[test]
    fn agent_calls_children_and_child_sessions_are_excluded_before_grouping() {
        let sessions = vec![
            session(
                "a",
                vec![
                    event(0.0, 1.0, Kind::User),
                    event(2.0, 900.0, Kind::Agent),
                    event(3.0, 4.0, Kind::Tool),
                    event(400.0, 401.0, Kind::Tool),
                ],
                vec![event(0.0, 800.0, Kind::Tool)],
            ),
            session("a", vec![event(0.0, 1000.0, Kind::Tool)], vec![]),
        ];
        let parents = HashMap::from([(1, (0, 0, 2.0))]);
        let mut cache = Cache::default();
        for gran in Granularity::ALL {
            let result = groups(&mut cache, &sessions, &[0, 1], &parents, View { gran, ..view() });
            assert_eq!(result[0].sessions, [0]);
            let blocks: Vec<_> = result[0].rows.iter().flatten().collect();
            assert!(blocks.iter().all(|b| b.session == 0 && b.block.end <= 401.0));
            assert!(blocks.iter().all(|b| {
                sessions[0].lanes[0].events[b.block.first..=b.block.last]
                    .iter()
                    .all(|e| e.kind != Kind::Agent)
            }));
            if matches!(
                gran,
                Granularity::Auto | Granularity::SameTool | Granularity::Call
            ) {
                assert_eq!(blocks.len(), 3);
            }
        }
        let result = groups(
            &mut cache,
            &sessions,
            &[0],
            &parents,
            View {
                range: (500.0, 600.0),
                in_view_only: true,
                ..view()
            },
        );
        assert!(result.is_empty());
    }

    #[test]
    fn code_mode_agent_calls_are_excluded_without_hiding_other_tools_or_messages() {
        let events = [
            (Kind::User, "Agent"),
            (Kind::Tool, "spawn_agent"),
            (Kind::Tool, "Agent"),
            (Kind::Tool, "Task"),
            (Kind::Tool, "TaskList"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (kind, name))| {
            let mut e = event(i as f64, i as f64 + 10.0, kind);
            e.name = name.into();
            e
        })
        .collect();
        let session = session("a", events, vec![]);
        let mut cache = Cache::default();
        for gran in Granularity::ALL {
            let result = cache.aggregate_main(0, &session.lanes[0], (0.0, 100.0), 1.0, gran);
            assert_eq!(
                result.iter().map(|b| (b.first, b.last)).collect::<Vec<_>>(),
                [(0, 0), (4, 4)]
            );
        }
    }

    #[test]
    fn minimum_width_and_clipping_are_used_for_overlap_and_empty_groups() {
        let sessions = vec![
            session("a", vec![event(-100.0, 0.0, Kind::Tool)], vec![]),
            session("a", vec![event(0.0, 0.0, Kind::Tool)], vec![]),
            session("a", vec![event(1.0, 1.0, Kind::Tool)], vec![]),
            session("empty", vec![event(2000.0, 2001.0, Kind::Tool)], vec![]),
        ];
        let mut cache = Cache::default();
        let result = groups(&mut cache, &sessions, &[0, 1, 2, 3], &HashMap::new(), view());
        assert_eq!(result[0].rows.len(), 2);
        assert_eq!(result[0].rows[0][0].x, -2.0);
        assert_eq!(result[0].rows[0][0].width, 3.0);
        assert_eq!(result[1].rows.len(), 1);
        assert!(result[1].rows[0].is_empty());
        let result = groups(
            &mut cache,
            &sessions,
            &[0, 1, 2, 3],
            &HashMap::new(),
            View {
                in_view_only: true,
                ..view()
            },
        );
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn main_cache_clips_original_event_indices_and_invalidates_replaced_sessions() {
        let mut session = session(
            "a",
            vec![
                event(0.0, 1.0, Kind::Agent),
                event(2.0, 10.0, Kind::User),
                event(4.0, 5.0, Kind::Tool),
                event(6.0, 100.0, Kind::Agent),
                event(8.0, 20.0, Kind::Tool),
            ],
            vec![],
        );
        let mut cache = Cache::default();
        for gran in Granularity::ALL {
            let result = cache.aggregate_main(0, &session.lanes[0], (3.0, 5.0), 1.0, gran);
            assert!(!result.is_empty());
            assert!(
                result
                    .iter()
                    .all(|b| b.first >= 1 && b.last <= 2 && b.end <= 10.0)
            );
            assert!(
                cache
                    .aggregate_main(0, &session.lanes[0], (-10.0, -5.0), 1.0, gran)
                    .is_empty()
            );
        }
        let changed = Arc::make_mut(&mut session);
        changed.lanes[0].events[1].kind = Kind::Agent;
        changed.finalize();
        cache.invalidate(&[0]);
        let result = cache.aggregate_main(0, &session.lanes[0], (0.0, 100.0), 1.0, Granularity::Call);
        assert_eq!(result.iter().map(|b| b.first).collect::<Vec<_>>(), [2, 4]);
    }
}
