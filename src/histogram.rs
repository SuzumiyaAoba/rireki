//! 時刻の半開区間ごとのイベント数。疎なレーンと密なレーンを別の探索で集計する。

use crate::model::Snapshot;

// edges は昇順。開始時刻を半開区間 [edges[i], edges[i + 1]) に数える。
pub fn count_bins(store: &Snapshot, order: &[usize], edges: &[f64]) -> Vec<u32> {
    let mut out = vec![0u32; edges.len().saturating_sub(1)];
    let (Some(&a), Some(&b)) = (edges.first(), edges.last()) else {
        return out;
    };
    for &si in order {
        let s = &store.sessions[si];
        if s.end < a || s.start >= b {
            continue;
        }
        for lane in &s.lanes {
            let evs = &lane.events;
            // レーン全体が期間内なら、開始と終了の二分探索は不要。
            let first = if evs.first().is_some_and(|e| a <= e.start) {
                0
            } else {
                evs.partition_point(|e| e.start < a)
            };
            // 先頭がNaNだと比較述語が単調でないため、従来の探索を使う。
            let last = if evs.last().is_some_and(|e| e.start < b)
                && evs.first().is_some_and(|e| !e.start.is_nan())
            {
                evs.len()
            } else {
                evs.partition_point(|e| e.start < b)
            };
            if first == last {
                continue;
            }
            let events = &evs[first..last];
            // 履歴のない区間は探索しない。短いレーンは全期間でも数列だけに収まる。
            let from = edges.partition_point(|&t| t <= events[0].start) - 1;
            // 全イベントが同じ列に収まる場合は、列の終わりも探索しない。
            if events[events.len() - 1].start < edges[from + 1] {
                out[from] += events.len() as u32;
                continue;
            }
            let to = edges.partition_point(|&t| t <= events[events.len() - 1].start);
            if events.len() < to - from {
                // 点在する少数のイベントでは、空の区間を走査せずイベントから列を求める。
                for e in events {
                    let bin = edges.partition_point(|&t| t <= e.start) - 1;
                    out[bin] += 1;
                }
            } else {
                if events[0].start.is_finite() && events[events.len() - 1].start.is_finite() {
                    let columns = to - from;
                    if columns >= 64 && events.len() <= columns.saturating_mul(4) {
                        add_linear(events, &edges[from + 1..to], &mut out[from..to]);
                    } else {
                        let mut prev = 0;
                        for i in from..to - 1 {
                            let next = events.partition_point(|e| e.start < edges[i + 1]);
                            out[i] += (next - prev) as u32;
                            prev = next;
                        }
                        out[to - 1] += (events.len() - prev) as u32;
                    }
                } else {
                    let mut prev = 0;
                    for i in from..to {
                        let next = events.partition_point(|e| e.start < edges[i + 1]);
                        out[i] += (next - prev) as u32;
                        prev = next;
                    }
                }
            }
        }
    }
    out
}

// 二分探索のループと前向き走査のループを同じ関数に展開しない。
#[inline(never)]
fn add_linear(events: &[crate::model::Event], cuts: &[f64], counts: &mut [u32]) {
    let mut previous = 0;
    for (&cut, count) in cuts.iter().zip(counts.iter_mut()) {
        let mut next = previous;
        while next < events.len() && events[next].start < cut {
            next += 1;
        }
        *count += (next - previous) as u32;
        previous = next;
    }
    *counts.last_mut().unwrap() += (events.len() - previous) as u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Kind, Lane, Session, Source};
    use std::sync::Arc;

    fn snapshot(lanes: Vec<Vec<f64>>) -> Snapshot {
        let sessions = lanes
            .chunks(2)
            .enumerate()
            .map(|(si, lanes)| {
                let mut session = Session {
                    id: si.to_string(),
                    source: Source::Codex,
                    cwd: String::new(),
                    headless: false,
                    title: String::new(),
                    branch: String::new(),
                    version: String::new(),
                    start: 0.0,
                    end: 0.0,
                    parents: Vec::new(),
                    tree: Vec::new(),
                    lanes: lanes
                        .iter()
                        .map(|starts| {
                            let mut lane = Lane::new("test");
                            lane.events = starts
                                .iter()
                                .map(|&start| Event {
                                    start,
                                    end: start + 500.0,
                                    kind: Kind::Tool,
                                    name: "".into(),
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
                            lane
                        })
                        .collect(),
                };
                session.finalize();
                Arc::new(session)
            })
            .collect();
        Snapshot { sessions }
    }

    // 二分探索やセッションの範囲判定を使わず、各半開区間を全イベントと比較する。
    fn full_scan(store: &Snapshot, order: &[usize], edges: &[f64]) -> Vec<u32> {
        edges
            .windows(2)
            .map(|bounds| {
                order
                    .iter()
                    .flat_map(|&si| &store.sessions[si].lanes)
                    .flat_map(|l| &l.events)
                    .filter(|e| bounds[0] <= e.start && e.start < bounds[1])
                    .count() as u32
            })
            .collect()
    }

    #[test]
    fn bins_respect_scope_and_half_open_start_boundaries() {
        let store = snapshot(vec![
            vec![-100.0, -1.0, 0.0, 0.0, 1.0, 2.0],
            vec![],
            vec![0.5, 1.0, 2.0],
        ]);
        let edges = [-1.0, 0.0, 1.0, 2.0];
        assert_eq!(count_bins(&store, &[0, 1], &edges), vec![1, 3, 2]);
        for scope in [vec![], vec![0], vec![1], vec![1, 0]] {
            assert_eq!(
                count_bins(&store, &scope, &edges),
                full_scan(&store, &scope, &edges)
            );
        }
    }

    #[test]
    fn sparse_and_dense_lanes_match_full_scan_for_varied_bins() {
        let mut seed = 15u64;
        let mut lanes = vec![
            vec![],
            vec![-1000.0, 1000.0],
            (0..2000).map(|i| i as f64 / 4.0 - 250.0).collect(),
        ];
        for n in [1, 2, 4, 10, 100, 300] {
            let events = (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed % 4001) as f64 / 4.0 - 500.0
                })
                .collect();
            lanes.push(events);
        }
        let store = snapshot(lanes);
        let order: Vec<_> = (0..store.sessions.len()).collect();
        for (a, b) in [
            (-500.0, 500.0),
            (-1.25, 1.25),
            (0.0, 300.0),
            (1000.0, 2000.0),
            (-10000.0, -1000.0),
        ] {
            for bins in [1, 2, 7, 42, 256, 1024] {
                let edges: Vec<_> = (0..=bins).map(|i| a + (b - a) * i as f64 / bins as f64).collect();
                assert_eq!(
                    count_bins(&store, &order, &edges),
                    full_scan(&store, &order, &edges),
                    "範囲 {a}..{b}、{bins} 列"
                );
            }
        }
    }

    #[test]
    fn empty_and_repeated_edges_are_valid() {
        let store = snapshot(vec![vec![-1.0, 0.0, 0.5, 1.0, 2.0], vec![]]);
        for edges in [
            vec![],
            vec![0.0],
            vec![0.0, 0.0],
            vec![-1.0, -1.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0],
        ] {
            assert_eq!(count_bins(&store, &[0], &edges), full_scan(&store, &[0], &edges));
            assert_eq!(
                count_bins(&Snapshot::default(), &[], &edges),
                vec![0; edges.len().saturating_sub(1)]
            );
        }
    }

    #[test]
    fn whole_lanes_and_single_bins_match_full_scan_at_repeated_boundaries() {
        let store = snapshot(vec![
            vec![-0.0; 4096],
            vec![0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0],
            vec![],
            vec![-2.0, -1.0, 2.0, 3.0],
        ]);
        for edges in [
            vec![-10.0, 10.0],
            vec![-10.0, 0.0, 0.0, 1.0, 1.0, 10.0],
            vec![0.0, 1.0],
            vec![0.25, 0.5, 0.75],
            vec![0.0, 0.0],
            vec![-10.0, -5.0],
        ] {
            for order in [vec![0, 1], vec![1], vec![0], vec![]] {
                assert_eq!(count_bins(&store, &order, &edges), full_scan(&store, &order, &edges));
            }
        }
    }


    #[test]
    fn nan_prefix_in_one_lane_keeps_other_lane_counts_with_overflow_checks() {
        let store = snapshot(vec![vec![-f64::NAN, -f64::NAN, 0.0], vec![-1.0]]);
        assert_eq!(count_bins(&store, &[0], &[-2.0, 2.0]), vec![1]);
        assert_eq!(count_bins(&store, &[0], &[-2.0, -1.0, 0.0, 2.0]), vec![0, 1, 0]);
    }

    #[test]
    fn merge_and_search_density_boundary_match_full_scan() {
        for bins in [2, 7, 8, 42, 63, 64, 65, 256, 1024] {
            for n in [bins - 1, bins, bins + 1, bins * 4 - 1, bins * 4, bins * 4 + 1, bins * 8] {
                for repeated in [false, true] {
                    let starts: Vec<_> = (0..n).map(|i| {
                        if repeated { (i * bins / n) as f64 } else { i as f64 * bins as f64 / n as f64 }
                    }).collect();
                    let store = snapshot(vec![starts.clone(), starts]);
                    for edges in [
                        (0..=bins).map(|i| i as f64).collect::<Vec<_>>(),
                        (0..=bins).flat_map(|i| [i as f64, i as f64]).collect(),
                        (0..=bins).map(|i| i as f64 + 0.5).collect(),
                    ] {
                        assert_eq!(count_bins(&store, &[0], &edges), full_scan(&store, &[0], &edges),
                            "bins={bins}, events={n}, repeated={repeated}");
                    }
                }
            }
        }
    }

    #[test]
    fn finite_merge_with_infinite_edges_preserves_half_open_counts() {
        let store = snapshot(vec![vec![-f64::INFINITY, -1.0, -0.0, 0.0, 0.5, 1.0, f64::INFINITY], (0..=128).map(|i| (i as f64 - 64.0) / 64.0).collect()]);
        for edges in [
            std::iter::once(-f64::INFINITY).chain((0..=64).map(|i| i as f64 / 64.0)).chain(std::iter::once(f64::INFINITY)).collect(),
            vec![-f64::INFINITY, 0.0, 0.0, 0.5, 1.0, f64::INFINITY],
            vec![-2.0, -1.0, 0.0, 0.5, 1.0, 2.0],
            vec![0.0, 0.0, 1.0, 1.0],
        ] {
            assert_eq!(count_bins(&store, &[0], &edges), full_scan(&store, &[0], &edges));
        }
    }

}
