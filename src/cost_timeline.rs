//! 表示期間内の API 換算料金。すべてのレーンを呼び出しの開始時刻で集計する。
//! 本文は読まず、直近の期間のセッション集計だけを版ごとに再利用する。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Weak};

use crate::detail::Selection;
use crate::model::{Session, Snapshot, UsageSum};

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
            bins: bins.clamp(1, 512),
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
        let mut seen = HashSet::new();
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
            let version = Arc::downgrade(session);
            let costs = if let Some(entry) = self.sessions.get(&si).filter(|e| e.version.ptr_eq(&version)) {
                entry.costs.clone()
            } else {
                let costs = Arc::new(session_costs(si, session, &out.edges, &mut prices, &keep_going)?);
                self.sessions.insert(si, Entry { version, costs: costs.clone() });
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
        self.sessions.retain(|si, _| seen.contains(si));
        for directory in &mut out.directories {
            if !keep_going() {
                return None;
            }
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
        for (offset, e) in lane.events[first..last].iter().enumerate() {
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
            let sample = Selection { session: si, lane: li, first: first + offset, last: first + offset };
            match buckets.entry(bin) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Bucket { index: bin, usage, sample, sample_cost: usage.cost });
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let bucket = entry.get_mut();
                    bucket.usage.merge(&usage);
                    if usage.cost > bucket.sample_cost {
                        bucket.sample = sample;
                        bucket.sample_cost = usage.cost;
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
