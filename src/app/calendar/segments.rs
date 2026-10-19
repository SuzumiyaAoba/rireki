//! カレンダーの稼働区間抽出・グループ化。

use super::{IDLE, SessionHash, Track};

/// 各レーンがその日 [ds, de) に動いていた区間 (tracks の位置, 開始, 終了)。
/// IDLE より長く手が空いたところで区切る（1 日中開いているセッションが列を占有しないように）
pub(super) fn day_segments(
    sessions: &[std::sync::Arc<crate::model::Session>],
    tracks: &[Track],
    ds: f64,
    de: f64,
) -> Vec<(usize, f64, f64)> {
    indexed_segments(sessions, tracks, 0..tracks.len(), ds, de)
}

pub(super) fn indexed_segments(
    sessions: &[std::sync::Arc<crate::model::Session>],
    tracks: &[Track],
    indices: impl Iterator<Item = usize>,
    ds: f64,
    de: f64,
) -> Vec<(usize, f64, f64)> {
    let mut out = Vec::new();
    for ti in indices {
        let t = &tracks[ti];
        let lane = &sessions[t.session].lanes[t.lane];
        if lane
            .range()
            .is_none_or(|(start, end)| end < ds || start >= de)
        {
            continue;
        }
        let (i, j) = segment_bounds(lane, ds, de);
        let mut cur: Option<(f64, f64)> = None;
        for e in &lane.events[i.min(j)..j] {
            let (s, en) = (e.start.max(ds), e.end.min(de));
            cur = match cur {
                Some((a, b)) if s <= b + IDLE => Some((a, b.max(en))),
                Some(seg) => {
                    out.push((ti, seg.0, seg.1));
                    Some((s, en))
                }
                None => Some((s, en)),
            };
        }
        if let Some((a, b)) = cur {
            out.push((ti, a, b));
        }
    }
    out
}

pub(super) fn track_days(
    edges: &[f64],
    ranges: impl Iterator<Item = Option<(f64, f64)>>,
) -> Vec<(usize, usize)> {
    let days = edges.len().saturating_sub(1);
    let ordered = edges.windows(2).all(|pair| pair[0] <= pair[1]);
    ranges
        .map(|range| {
            let Some((start, end)) = range else {
                return (0, 0);
            };
            if days == 0 {
                return (0, 0);
            }
            // 存在しないローカルの午前0時などで境界が逆転した場合は、元の区間判定へ渡す。
            if !ordered {
                return (0, days);
            }
            let first = if start.is_nan() {
                0
            } else {
                edges[1..].partition_point(|&de| de <= start)
            };
            let last = if end.is_nan() {
                days
            } else {
                edges[..days].partition_point(|&ds| ds <= end)
            };
            if first < last { (first, last) } else { (0, 0) }
        })
        .collect()
}

pub(super) fn sparse_days(spans: &[(usize, usize)], days: usize) -> bool {
    let occupied = spans.iter().fold(0_usize, |sum, &(start, end)| {
        sum.saturating_add(end - start)
    });
    occupied <= spans.len().saturating_mul(days) / 2
}

pub(super) fn sparse_track_days(
    edges: &[f64],
    ranges: impl Iterator<Item = Option<(f64, f64)>> + Clone,
) -> Option<Vec<(usize, usize)>> {
    let days = edges.len().saturating_sub(1);
    if days == 0 || !edges.windows(2).all(|pair| pair[0] <= pair[1]) {
        return None;
    }
    // 全レーンが全日を覆う場合、日別の選別とその配列を作らない。
    if ranges.clone().all(|range| {
        range.is_some_and(|(start, end)| !(start >= edges[1] || end < edges[days - 1]))
    }) {
        return None;
    }
    let spans = track_days(edges, ranges);
    sparse_days(&spans, days).then_some(spans)
}

/// グループ（セッション）の区間
pub(super) struct GroupSeg {
    pub(super) start: f64,
    pub(super) end: f64,
    /// この区間に動いていたレーン（tracks の位置、tracks の順 = メインが先）
    pub(super) lanes: Vec<usize>,
    /// 代表のセッション（起動元）
    pub(super) session: usize,
}

/// レーンの区間をグループごとにまとめ、重なる・近い区間をつなげる
pub(super) fn group_segments(segs: &[(usize, f64, f64)], group: &[usize]) -> Vec<GroupSeg> {
    if few_groups(segs, group) {
        let ordered = segs
            .windows(2)
            .all(|pair| pair[0].1.total_cmp(&pair[1].1).is_le());
        return group_segments_tree(segs, group, ordered);
    }
    let mut sorted: Vec<_> = segs
        .iter()
        .map(|&(ti, a, b)| (group[ti], ti, a, b))
        .collect();
    sorted.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.2.total_cmp(&y.2)));
    merge_grouped(sorted)
}

pub(super) fn merge_grouped(
    sorted: impl IntoIterator<Item = (usize, usize, f64, f64)>,
) -> Vec<GroupSeg> {
    let mut out = Vec::new();
    let mut current_group = None;
    let mut cur: Option<GroupSeg> = None;
    for (key, ti, a, b) in sorted {
        if current_group != Some(key) {
            out.extend(cur.take());
            current_group = Some(key);
        }
        match &mut cur {
            Some(g) if a <= g.end + IDLE => {
                g.end = g.end.max(b);
                if !g.lanes.contains(&ti) {
                    g.lanes.push(ti);
                }
            }
            _ => {
                out.extend(cur.take());
                cur = Some(GroupSeg {
                    start: a,
                    end: b,
                    lanes: vec![ti],
                    session: 0,
                });
            }
        }
    }
    out.extend(cur);
    for g in &mut out {
        g.lanes.sort_unstable();
    }
    out
}

pub(super) fn few_groups(segs: &[(usize, f64, f64)], group: &[usize]) -> bool {
    if segs.len() < 256 {
        return false;
    }
    let mut keys = [0usize; 16];
    let mut used = 0;
    for &(ti, _, _) in segs.iter().take(128) {
        let key = group[ti];
        if !keys[..used].contains(&key) {
            if used == keys.len() {
                return false;
            }
            keys[used] = key;
            used += 1;
        }
    }
    true
}

pub(super) fn merge_lane_intervals<const LARGE: bool>(
    out: &mut Vec<GroupSeg>,
    intervals: impl IntoIterator<Item = (usize, f64, f64)>,
) {
    let mut membership = LaneMembership::default();
    let mut cur: Option<GroupSeg> = None;
    for (ti, a, b) in intervals {
        match &mut cur {
            Some(g) if a <= g.end + IDLE => {
                g.end = g.end.max(b);
                if LARGE {
                    membership.add(&mut g.lanes, ti);
                } else if !g.lanes.contains(&ti) {
                    g.lanes.push(ti);
                }
            }
            _ => {
                out.extend(cur.take());
                if LARGE {
                    membership.reset(ti);
                }
                cur = Some(GroupSeg {
                    start: a,
                    end: b,
                    lanes: vec![ti],
                    session: 0,
                });
            }
        }
    }
    out.extend(cur);
}

pub(super) fn group_segments_tree(
    segs: &[(usize, f64, f64)],
    group: &[usize],
    ordered: bool,
) -> Vec<GroupSeg> {
    let mut by: std::collections::BTreeMap<usize, Vec<(usize, f64, f64)>> = Default::default();
    for &(ti, a, b) in segs {
        by.entry(group[ti]).or_default().push((ti, a, b));
    }
    let mut out = Vec::new();
    for (_, mut intervals) in by {
        if !ordered {
            intervals.sort_by(|x, y| x.1.total_cmp(&y.1));
        }
        if intervals.len() < 64 {
            merge_lane_intervals::<false>(&mut out, intervals);
        } else {
            merge_lane_intervals::<true>(&mut out, intervals);
        }
    }
    for g in &mut out {
        g.lanes.sort_unstable();
    }
    out
}

#[derive(Default)]
pub(super) struct LaneMembership {
    pub(super) mask: u64,
    pub(super) index:
        Option<std::collections::HashSet<usize, std::hash::BuildHasherDefault<SessionHash>>>,
}

impl LaneMembership {
    fn reset(&mut self, ti: usize) {
        self.mask = 1 << (ti % 64);
        self.index = None;
    }

    fn add(&mut self, lanes: &mut Vec<usize>, ti: usize) {
        if lanes.len() == 1 && lanes[0] == ti {
            return;
        }
        let bit = 1 << (ti % 64);
        if self.mask & bit == 0 {
            self.mask |= bit;
            lanes.push(ti);
        } else if lanes.len() >= 64 {
            let index = self.index.get_or_insert_with(|| {
                let mut index = std::collections::HashSet::with_capacity_and_hasher(
                    lanes.len() + 1,
                    Default::default(),
                );
                index.extend(lanes.iter().copied());
                index
            });
            if index.insert(ti) {
                lanes.push(ti);
            }
            self.mask = u64::MAX;
        } else if !lanes.contains(&ti) {
            lanes.push(ti);
        }
    }
}

/// 端点で全体を使えるときだけ、日別区間の二分探索を省く。
pub(super) fn segment_bounds(lane: &crate::model::Lane, ds: f64, de: f64) -> (usize, usize) {
    let i = if lane.cum_end.first().is_some_and(|&end| end >= ds) {
        0
    } else {
        lane.cum_end.partition_point(|&end| end < ds)
    };
    // total_cmpで先頭へ並ぶ負のNaNは、従来の探索へ渡す。
    let j = if lane.events.first().is_some_and(|e| !e.start.is_nan())
        && lane.events.last().is_some_and(|e| e.start < de)
    {
        lane.events.len()
    } else {
        lane.events.partition_point(|e| e.start < de)
    };
    (i, j)
}
