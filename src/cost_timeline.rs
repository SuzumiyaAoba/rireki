//! 表示期間内の API 換算料金。すべてのレーンを呼び出しの開始時刻で集計する。
//! 本文は読まず、直近の期間のセッション集計だけを版ごとに再利用する。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Weak};

use crate::detail::Selection;
use crate::model::{Session, Snapshot, UsageSum};

const MAX_BINS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub start: f64,
    pub end: f64,
    pub bins: usize,
}

impl View {
    pub fn new(start: f64, end: f64, bins: usize) -> Option<Self> {
        (start.is_finite() && end.is_finite() && end > start && (end - start).is_finite()).then_some(Self {
            start,
            end,
            bins: bins.clamp(1, MAX_BINS),
        })
    }

    fn edges(self) -> Vec<f64> {
        (0..=self.bins)
            .map(|i| {
                if i == self.bins {
                    self.end
                } else {
                    self.start + (self.end - self.start) * (i as f64 / self.bins as f64)
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct Bucket {
    pub index: usize,
    pub usage: UsageSum,
    /// この区間で換算料金が最大の API 呼び出し。詳細を開くための位置。
    pub sample: Selection,
    sample_cost: f64,
}

impl Bucket {
    fn add_event(&mut self, usage: &UsageSum, sample: Selection) {
        self.usage.merge(usage);
        if usage.cost > self.sample_cost {
            self.sample = sample;
            self.sample_cost = usage.cost;
        }
    }

    fn merge(&mut self, other: &Self) {
        self.usage.merge(&other.usage);
        if other.sample_cost > self.sample_cost {
            self.sample = other.sample;
            self.sample_cost = other.sample_cost;
        }
    }
}

#[derive(Debug)]
pub struct SessionCosts {
    pub session: usize,
    pub active: bool,
    pub usage: UsageSum,
    /// 空の区間は保持しない。
    pub buckets: Vec<Bucket>,
}

#[derive(Debug)]
pub struct Directory {
    pub cwd: String,
    pub sessions: Vec<Arc<SessionCosts>>,
    pub usage: UsageSum,
    pub buckets: Vec<Bucket>,
}

#[derive(Debug)]
pub struct Summary {
    pub view: View,
    pub edges: Vec<f64>,
    pub directories: Vec<Directory>,
    pub usage: UsageSum,
    pub sessions: usize,
}

struct Entry {
    version: Weak<Session>,
    costs: Arc<SessionCosts>,
}

#[derive(Default)]
pub struct Cache {
    view: Option<View>,
    sessions: HashMap<usize, Entry>,
    pub rebuilt: usize,
}

enum Seen {
    Dense(Vec<u64>),
    Sparse(HashSet<usize>),
}

impl Seen {
    fn new(sessions: usize, order: &[usize]) -> Self {
        if sessions <= order.len().saturating_mul(16) && order.iter().all(|&i| i < sessions) {
            Self::Dense(vec![0; sessions.div_ceil(64)])
        } else {
            Self::Sparse(HashSet::new())
        }
    }

    fn insert(&mut self, i: usize) -> bool {
        match self {
            Self::Dense(words) => {
                let word = &mut words[i / 64];
                let mask = 1 << (i % 64);
                let fresh = *word & mask == 0;
                *word |= mask;
                fresh
            }
            Self::Sparse(set) => set.insert(i),
        }
    }

    fn contains(&self, i: usize) -> bool {
        match self {
            Self::Dense(words) => words.get(i / 64).is_some_and(|word| word & (1 << (i % 64)) != 0),
            Self::Sparse(set) => set.contains(&i),
        }
    }
}

impl Cache {
    pub fn build_while(
        &mut self,
        store: &Snapshot,
        order: &[usize],
        view: View,
        in_view_only: bool,
        keep_going: impl Fn() -> bool,
    ) -> Option<Summary> {
        if !keep_going() {
            return None;
        }
        self.rebuilt = 0;
        if self.view != Some(view) {
            self.sessions.clear();
            self.view = Some(view);
        }
        let edges = view.edges();
        let mut out =
            Summary { view, edges, directories: Vec::new(), usage: UsageSum::default(), sessions: 0 };
        let mut seen = Seen::new(store.sessions.len(), order);
        let mut directories: HashMap<&str, usize> = HashMap::new();
        let mut prices = crate::pricing::Cache::default();
        for &si in order {
            if !keep_going() {
                return None;
            }
            if !seen.insert(si) {
                continue;
            }
            let Some(session) = store.sessions.get(si) else {
                continue;
            };
            let costs = if let Some(entry) = self.sessions.get(&si).filter(|e| e.version.as_ptr() == Arc::as_ptr(session)) {
                entry.costs.clone()
            } else {
                let costs = Arc::new(session_costs(si, session, &out.edges, &mut prices, &keep_going)?);
                self.sessions.insert(si, Entry { version: Arc::downgrade(session), costs: costs.clone() });
                self.rebuilt += 1;
                costs
            };
            if in_view_only && !costs.active {
                continue;
            }
            out.usage.merge(&costs.usage);
            out.sessions += 1;
            let next = out.directories.len();
            let di = *directories.entry(&session.cwd).or_insert_with(|| {
                out.directories.push(Directory {
                    cwd: session.cwd.clone(),
                    sessions: Vec::new(),
                    usage: UsageSum::default(),
                    buckets: Vec::new(),
                });
                next
            });
            let directory = &mut out.directories[di];
            directory.usage.merge(&costs.usage);
            directory.sessions.push(costs);
        }
        self.sessions.retain(|si, _| seen.contains(*si));
        let mut positions = [u16::MAX; MAX_BINS + 1];
        for directory in &mut out.directories {
            if !keep_going() {
                return None;
            }
            if directory.sessions.len() == 1 {
                directory.buckets = directory.sessions[0].buckets.clone();
                continue;
            }
            if view.bins <= MAX_BINS {
                let (first, rest) = directory.sessions.split_first().unwrap();
                directory.buckets = first.buckets.clone();
                for (position, bucket) in directory.buckets.iter().enumerate() {
                    positions[bucket.index] = position as u16;
                }
                for session in rest {
                    for bucket in &session.buckets {
                        let position = &mut positions[bucket.index];
                        if *position == u16::MAX {
                            *position = directory.buckets.len() as u16;
                            directory.buckets.push(bucket.clone());
                        } else {
                            directory.buckets[*position as usize].merge(bucket);
                        }
                    }
                }
                for bucket in &directory.buckets {
                    positions[bucket.index] = u16::MAX;
                }
                directory.buckets.sort_unstable_by_key(|b| b.index);
                directory.buckets.shrink_to_fit();
                continue;
            }
            // Viewの直接構築で上限を超えた場合も、従来の集計を使う。
            let mut buckets: BTreeMap<usize, Bucket> = BTreeMap::new();
            for session in &directory.sessions {
                for bucket in &session.buckets {
                    buckets
                        .entry(bucket.index)
                        .and_modify(|b| b.merge(bucket))
                        .or_insert_with(|| bucket.clone());
                }
            }
            directory.buckets = buckets.into_values().collect();
        }
        keep_going().then_some(out)
    }
}

#[inline]
fn event_bucket<'a>(
    buckets: &'a mut BTreeMap<usize, Bucket>,
    bin: usize,
    usage: &UsageSum,
    sample: Selection,
) -> &'a mut Bucket {
    match buckets.entry(bin) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(Bucket { index: bin, usage: *usage, sample, sample_cost: usage.cost })
        }
        std::collections::btree_map::Entry::Occupied(entry) => {
            let bucket = entry.into_mut();
            bucket.add_event(usage, sample);
            bucket
        }
    }
}

fn session_costs<'a>(
    si: usize,
    session: &'a Session,
    edges: &[f64],
    prices: &mut crate::pricing::Cache<'a>,
    keep_going: &impl Fn() -> bool,
) -> Option<SessionCosts> {
    let (start, end) = (edges[0], *edges.last()?);
    let mut out =
        SessionCosts { session: si, active: false, usage: UsageSum::default(), buckets: Vec::new() };
    let mut buckets: BTreeMap<usize, Bucket> = BTreeMap::new();
    for (li, lane) in session.lanes.iter().enumerate() {
        if !keep_going() {
            return None;
        }
        let first = lane.events.partition_point(|e| e.start < start);
        let last = lane.events.partition_point(|e| e.start < end);
        out.active |= first < last || (first > 0 && lane.cum_end[first - 1] >= start);
        let mut bin = 0;
        let events = &lane.events[first..last];
        if events.len() < 64 {
            for (offset, e) in events.iter().enumerate() {
                if offset % 256 == 0 && !keep_going() {
                    return None;
                }
                if e.usage.is_empty() {
                    continue;
                }
                while bin + 1 < edges.len() && e.start >= edges[bin + 1] {
                    bin += 1;
                }
                let mut usage = UsageSum::default();
                usage.add_priced(&e.usage, prices.get(e.model.as_deref()));
                out.usage.merge(&usage);
                event_bucket(
                    &mut buckets,
                    bin,
                    &usage,
                    Selection { session: si, lane: li, first: first + offset, last: first + offset },
                );
            }
            continue;
        }
        // 元と同じ256イベントの先頭で確認し、区間内では集計先を再利用する。
        for (chunk_index, chunk) in events.chunks(256).enumerate() {
            if !keep_going() {
                return None;
            }
            let base = first + chunk_index * 256;
            let mut events = chunk.iter().enumerate();
            let mut pending = None;
            while let Some((offset, e)) =
                pending.take().or_else(|| events.find(|(_, e)| !e.usage.is_empty()))
            {
                if e.usage.is_empty() {
                    continue;
                }
                while bin + 1 < edges.len() && e.start >= edges[bin + 1] {
                    bin += 1;
                }
                let mut usage = UsageSum::default();
                usage.add_priced(&e.usage, prices.get(e.model.as_deref()));
                out.usage.merge(&usage);
                let bucket = event_bucket(
                    &mut buckets,
                    bin,
                    &usage,
                    Selection { session: si, lane: li, first: base + offset, last: base + offset },
                );
                for (offset, e) in events.by_ref() {
                    if bin + 1 < edges.len() && e.start >= edges[bin + 1] {
                        pending = Some((offset, e));
                        break;
                    }
                    if !e.usage.is_empty() {
                        let mut usage = UsageSum::default();
                        usage.add_priced(&e.usage, prices.get(e.model.as_deref()));
                        out.usage.merge(&usage);
                        bucket.add_event(
                            &usage,
                            Selection { session: si, lane: li, first: base + offset, last: base + offset },
                        );
                    }
                }
            }
        }
    }
    out.buckets = buckets.into_values().collect();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Kind, Lane, Source, Usage};

    fn event(start: f64, model: Option<&str>, input: u32) -> Event {
        Event {
            start,
            end: start + 300.0,
            kind: Kind::Assistant,
            name: "Assistant".into(),
            summary: Default::default(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
            status: Default::default(),
            model: model.map(Into::into),
            usage: Usage { input, ..Usage::default() },
        }
    }

    fn session(cwd: &str, lanes: Vec<Vec<Event>>, headless: bool) -> Arc<Session> {
        let mut session = Session {
            id: cwd.into(),
            source: Source::Codex,
            cwd: cwd.into(),
            headless,
            title: "test".into(),
            branch: String::new(),
            version: String::new(),
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
            lanes: lanes
                .into_iter()
                .map(|events| {
                    let mut lane = Lane::new("main");
                    lane.events = events;
                    lane
                })
                .collect(),
        };
        session.finalize();
        Arc::new(session)
    }

    fn build(cache: &mut Cache, store: &Snapshot, order: &[usize], view: View, in_view: bool) -> Summary {
        cache.build_while(store, order, view, in_view, || true).unwrap()
    }

    fn assert_usage(actual: UsageSum, expected: UsageSum) {
        assert_eq!(
            (
                actual.input,
                actual.cache_read,
                actual.cache_write,
                actual.output,
                actual.reasoning,
                actual.calls,
                actual.unpriced
            ),
            (
                expected.input,
                expected.cache_read,
                expected.cache_write,
                expected.output,
                expected.reasoning,
                expected.calls,
                expected.unpriced
            )
        );
        assert!((actual.cost - expected.cost).abs() <= 1e-9 * expected.cost.max(1.0));
    }

    #[test]
    fn seen_matches_hash_set_across_word_boundaries_subsets_and_invalid_indices() {
        for sessions in [0, 1, 63, 64, 65, 127, 128, 129, 513, 2049] {
            for order in [
                (0..sessions).chain((0..sessions).rev()).collect::<Vec<_>>(),
                vec![],
                vec![0],
                vec![sessions, usize::MAX, 0, sessions, usize::MAX],
            ] {
                let mut actual = Seen::new(sessions, &order);
                let mut expected = HashSet::new();
                for &i in &order {
                    assert_eq!(actual.contains(i), expected.contains(&i));
                    assert_eq!(actual.insert(i), expected.insert(i));
                    assert!(actual.contains(i));
                }
                for i in (0..=sessions).chain([usize::MAX]) {
                    assert_eq!(actual.contains(i), expected.contains(&i));
                }
            }
        }
    }

    #[test]
    fn directory_bucket_positions_preserve_order_ties_and_reset_between_directories() {
        let store = Snapshot {
            sessions: vec![
                session("/a", vec![vec![event(8.0, Some("gpt-5.4"), 100), event(15.999, None, 7)]], false),
                session("/b", vec![vec![event(0.0, Some("gpt-5.4"), 13), event(15.999, Some("gpt-5.4"), 40)]], false),
                session("/a", vec![vec![event(1.0, Some("unpriced-test"), 11), event(8.0, Some("gpt-5.4"), 100)]], true),
                session("/b", vec![vec![event(20.0, Some("gpt-5.4"), 19)]], false),
                session("/empty", vec![vec![event(2.0, None, 0)]], false),
            ],
        };
        for bins in [0, 1, 7, MAX_BINS, MAX_BINS + 1] {
            let view = View { start: 0.0, end: 16.0, bins };
            for order in [vec![], vec![0, 1, 2, 3, 4], vec![4, 3, 2, 1, 0, 0, 99]] {
                let mut cache = Cache::default();
                for in_view in [false, true, false] {
                    let summary = build(&mut cache, &store, &order, view, in_view);
                    for directory in &summary.directories {
                        let mut expected: BTreeMap<usize, Bucket> = BTreeMap::new();
                        for session in &directory.sessions {
                            for bucket in &session.buckets {
                                expected
                                    .entry(bucket.index)
                                    .and_modify(|b| b.merge(bucket))
                                    .or_insert_with(|| bucket.clone());
                            }
                        }
                        assert_eq!(directory.buckets.len(), expected.len());
                        for (actual, expected) in directory.buckets.iter().zip(expected.into_values()) {
                            assert_eq!(actual.index, expected.index);
                            assert_eq!(actual.usage, expected.usage);
                            assert_eq!(actual.usage.cost.to_bits(), expected.usage.cost.to_bits());
                            assert_eq!(actual.sample, expected.sample);
                            assert_eq!(actual.sample_cost.to_bits(), expected.sample_cost.to_bits());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bucket_runs_keep_exact_sums_samples_and_cancellation_points() {
        let store = Snapshot {
            sessions: vec![session(
                "/runs",
                (0..3)
                    .map(|li| {
                        (0..4096)
                            .map(|i| {
                                event(
                                    i as f64,
                                    match (i + li) % 5 {
                                        0 => None,
                                        1 => Some("unpriced-test"),
                                        _ => Some("gpt-5.4"),
                                    },
                                    if i % 256 < 192 { 0 } else { (i % 13 + 1) * 17 },
                                )
                            })
                            .collect()
                    })
                    .collect(),
                false,
            )],
        };
        for bins in [1, 7, 512] {
            let view = View::new(0.0, 4096.0, bins).unwrap();
            let mut cache = Cache::default();
            let checks = std::cell::Cell::new(0);
            let summary = cache
                .build_while(&store, &[0], view, false, || {
                    checks.set(checks.get() + 1);
                    true
                })
                .unwrap();
            // 開始・セッション・3レーン・各レーンの256件ごと・ディレクトリ・完了。
            assert_eq!(checks.get(), 1 + 1 + 3 + 3 * 16 + 1 + 1);
            let actual = &summary.directories[0].sessions[0];
            let edges = &summary.edges;
            let mut expected_bins = 0;
            for index in 0..bins {
                let events = store.sessions[0].lanes.iter().enumerate().flat_map(|(li, lane)| {
                    lane.events.iter().enumerate().filter_map(move |(i, e)| {
                        (edges[index] <= e.start
                            && e.start < edges[index + 1]
                            && !e.usage.is_empty())
                        .then_some((li, i, e))
                    })
                });
                let mut expected = UsageSum::default();
                let mut sample = None;
                for (li, i, e) in events {
                    expected.add(&e.usage, e.model.as_deref());
                    let price = e.model.as_deref().and_then(crate::pricing::price);
                    let cost = price.map_or(0.0, |p| p.cost(&e.usage));
                    if sample.is_none_or(|(_, previous)| cost > previous) {
                        sample = Some((Selection { session: 0, lane: li, first: i, last: i }, cost));
                    }
                }
                let bucket = actual.buckets.iter().find(|b| b.index == index);
                if let Some((sample, cost)) = sample {
                    expected_bins += 1;
                    let bucket = bucket.unwrap();
                    assert_eq!(bucket.usage, expected);
                    assert_eq!(bucket.usage.cost.to_bits(), expected.cost.to_bits());
                    assert_eq!(bucket.sample, sample);
                    assert_eq!(bucket.sample_cost.to_bits(), cost.to_bits());
                } else {
                    assert!(bucket.is_none());
                }
            }
            assert_eq!(actual.buckets.len(), expected_bins);
            for stop_at in 1..=checks.get() {
                let mut cache = Cache::default();
                let calls = std::cell::Cell::new(0);
                assert!(
                    cache
                        .build_while(&store, &[0], view, false, || {
                            calls.set(calls.get() + 1);
                            calls.get() < stop_at
                        })
                        .is_none()
                );
                assert_eq!(calls.get(), stop_at);
                let resumed = build(&mut cache, &store, &[0], view, false);
                assert_eq!(format!("{resumed:?}"), format!("{summary:?}"));
            }
        }
    }

    #[test]
    fn boundaries_children_unknown_models_and_empty_usage() {
        let known = Some("gpt-5.4");
        let mut cached = event(10.0, known, 1_000_000);
        cached.usage = Usage {
            input: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 2_000_000,
            cache_write_1h: 1_000_000,
            output: 1_000_000,
            reasoning: 900_000,
        };
        let store = Snapshot {
            sessions: vec![
                session(
                    "/a",
                    vec![
                        vec![
                            event(-1.0, known, 100),
                            event(0.0, known, 1_000_000),
                            cached.clone(),
                            event(20.0, known, 200),
                        ],
                        vec![event(10.0, None, 50)],
                    ],
                    false,
                ),
                session("/a", vec![vec![event(19.0, Some("unpriced-test"), 25)]], true),
                session("/b", vec![vec![event(5.0, known, 0)]], false),
            ],
        };
        let summary =
            build(&mut Cache::default(), &store, &[0, 1, 2, 0], View::new(0.0, 20.0, 2).unwrap(), true);
        assert_eq!(summary.sessions, 3);
        assert_eq!(summary.directories.len(), 2);
        assert_eq!(summary.usage.calls, 4);
        assert_eq!(summary.usage.unpriced, 2);
        let a = &summary.directories[0];
        assert_eq!(a.sessions.len(), 2);
        assert_eq!(a.buckets[0].usage.calls, 1);
        assert_eq!(a.buckets[1].usage.calls, 3);
        assert_eq!(a.buckets[1].sample, Selection { session: 0, lane: 0, first: 2, last: 2 });
        assert_usage(
            a.usage,
            UsageSum::of(
                store.sessions[..2]
                    .iter()
                    .flat_map(|s| &s.lanes)
                    .flat_map(|l| &l.events)
                    .filter(|e| e.start >= 0.0 && e.start < 20.0),
            ),
        );
        assert!(summary.directories[1].buckets.is_empty());
        assert_eq!(summary.directories[1].usage.calls, 0);
        // 推論は出力に含まれ、キャッシュも既存の換算と同じ料金になる。
        assert!(
            (a.buckets[1].usage.cost - crate::pricing::price("gpt-5.4").unwrap().cost(&cached.usage)).abs()
                < 1e-9
        );
    }

    #[test]
    fn cached_updates_scope_and_range_match_full_scan() {
        let store = Snapshot {
            sessions: vec![
                session(
                    "/a",
                    vec![
                        (0..2000)
                            .map(|i| {
                                event(
                                    i as f64 * 0.25 - 100.0,
                                    if i % 5 == 0 { None } else { Some("claude-sonnet-4-5") },
                                    i + 1,
                                )
                            })
                            .collect(),
                    ],
                    false,
                ),
                session("/a", vec![vec![event(-500.0, Some("gpt-5.4"), 100), event(200.0, None, 100)]], true),
                session("/b", vec![vec![event(1000.0, Some("gpt-5.4"), 20)]], false),
            ],
        };
        let mut cache = Cache::default();
        for (start, end) in [(-1000.0, 2000.0), (0.0, 300.0), (100.0, 101.0), (4000.0, 5000.0)] {
            for bins in [1, 2, 7, 100] {
                let view = View::new(start, end, bins).unwrap();
                for order in [vec![], vec![0], vec![2, 1, 0]] {
                    let summary = build(&mut cache, &store, &order, view, false);
                    let expected = UsageSum::of(
                        order
                            .iter()
                            .flat_map(|&si| &store.sessions[si].lanes)
                            .flat_map(|l| &l.events)
                            .filter(|e| start <= e.start && e.start < end),
                    );
                    assert_usage(summary.usage, expected);
                    for directory in &summary.directories {
                        for i in 0..bins {
                            let expected = UsageSum::of(
                                directory
                                    .sessions
                                    .iter()
                                    .flat_map(|s| &store.sessions[s.session].lanes)
                                    .flat_map(|l| &l.events)
                                    .filter(|e| {
                                        summary.edges[i] <= e.start && e.start < summary.edges[i + 1]
                                    }),
                            );
                            let actual = directory
                                .buckets
                                .iter()
                                .find(|b| b.index == i)
                                .map_or(UsageSum::default(), |b| b.usage);
                            assert_usage(actual, expected);
                        }
                    }
                }
            }
        }
        let view = View::new(0.0, 300.0, 7).unwrap();
        let old = build(&mut cache, &store, &[0, 1], view, false);
        assert_eq!(cache.rebuilt, 2);
        let reused = build(&mut cache, &store, &[0, 1], view, true);
        assert_eq!(cache.rebuilt, 0);
        assert!(Arc::ptr_eq(&old.directories[0].sessions[0], &reused.directories[0].sessions[0]));
        let mut changed = store.clone();
        let mut replacement = (*changed.sessions[0]).clone();
        replacement.lanes[0].events.push(event(220.0, Some("gpt-5.4"), 10_000));
        replacement.finalize();
        changed.sessions[0] = Arc::new(replacement);
        let updated = build(&mut cache, &changed, &[0, 1], view, true);
        assert_eq!(cache.rebuilt, 1);
        assert!(Arc::ptr_eq(&reused.directories[0].sessions[1], &updated.directories[0].sessions[1]));
        let fresh = build(&mut Cache::default(), &changed, &[0, 1], view, true);
        assert_usage(updated.usage, fresh.usage);
        assert_eq!(Arc::strong_count(&store.sessions[0]), 1);
    }

    #[test]
    fn cancellation_does_not_publish_partial_session_and_outside_calls_are_not_charged() {
        let store = Snapshot {
            sessions: vec![
                session("/a", vec![(0..2000).map(|i| event(i as f64, Some("gpt-5.4"), 1)).collect()], false),
                session("/b", vec![vec![event(-10_000.0, Some("gpt-5.4"), 500)]], false),
            ],
        };
        let mut cache = Cache::default();
        let checks = std::cell::Cell::new(0);
        let view = View::new(0.0, 2500.0, 10).unwrap();
        let stopped = cache.build_while(&store, &[0, 1], view, false, || {
            checks.set(checks.get() + 1);
            checks.get() < 6
        });
        assert!(stopped.is_none());
        assert!(cache.sessions.is_empty());
        let summary = build(&mut cache, &store, &[0, 1], view, false);
        assert_eq!(summary.usage.calls, 2000);
        assert_eq!(summary.sessions, 2);
        let filtered = build(&mut cache, &store, &[0, 1], view, true);
        assert_eq!(filtered.sessions, 1);
        assert_usage(filtered.usage, summary.usage);
        assert!(View::new(f64::NAN, 1.0, 1).is_none());
        assert!(View::new(1.0, 1.0, 1).is_none());
        assert!(View::new(1.0, f64::INFINITY, 1).is_none());
        assert_eq!(View::new(0.0, 1.0, 0).unwrap().bins, 1);
        assert_eq!(View::new(0.0, 1.0, usize::MAX).unwrap().bins, 512);
    }
}
