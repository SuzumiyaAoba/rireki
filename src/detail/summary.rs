//! 詳細の集計を背景で作り、同じセッションの版では選択を変えても再利用する。

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use crate::model::{Event, Kind, Session, Stats, UsageSum};

#[derive(Default)]
pub struct Totals {
    pub stats: Stats,
    pub usage: UsageSum,
}

impl Totals {
    // build は呼び出し元で単相化されるので、イベントごとの処理も境界を越えて最適化可能にする。
    #[inline]
    fn add(&mut self, event: &Event, price: Option<crate::pricing::Price>) {
        self.stats.add(event);
        self.usage.add_priced(&event.usage, price);
    }
}

#[derive(Default)]
pub struct SelectionSummary {
    pub total: Totals,
    /// (名前, 件数, 選択範囲内の先頭イベント位置)。同数なら先に登場した順。
    /// GUI は先頭イベントから配色とアイコンを求め、集計は表示に依存しない。
    pub names: Vec<(String, usize, usize)>,
    pub end: f64,
}

impl SelectionSummary {
    pub fn build(events: &[Event], is_current: &impl Fn() -> bool) -> Option<Self> {
        let mut out = Self {
            end: events.first().map_or(0.0, |e| e.start),
            ..Self::default()
        };
        let mut names = HashMap::new();
        let mut prices = crate::pricing::Cache::default();
        let mut last_name: Option<usize> = None;
        let mut event_index = 0;
        for chunk in events.chunks(256) {
            if !is_current() {
                return None;
            }
            for event in chunk {
                let price = if event.usage.is_empty() { None } else { prices.get(event.model.as_deref()) };
                out.total.add(event, price);
                out.end = out.end.max(event.end);
                let index = match last_name.filter(|&i| out.names[i].0 == event.name.as_ref()) {
                    Some(i) => i,
                    None => *names.entry(event.name.as_ref()).or_insert_with(|| {
                        out.names.push((event.name.to_string(), 0, event_index));
                        out.names.len() - 1
                    }),
                };
                last_name = Some(index);
                out.names[index].1 += 1;
                event_index += 1;
            }
        }
        // 同数の名前は最初に登場した順を保つ。
        out.names.sort_by_key(|(_, n, _)| std::cmp::Reverse(*n));
        is_current().then_some(out)
    }
}

#[derive(Default)]
pub struct SessionSummary {
    pub total: Totals,
    pub lanes: Vec<Totals>,
    pub models: Vec<(String, UsageSum)>,
    pub turns: Vec<Vec<TurnSummary>>,
}

pub struct TurnSummary {
    pub index: usize,
    /// 次の指示までに記録された最後の応答。概要から本文へ移動するための位置。
    pub response: Option<usize>,
    pub events: usize,
    pub end: f64,
    pub failures: usize,
}

impl SessionSummary {
    pub fn build(session: &Session, is_current: &impl Fn() -> bool) -> Option<Self> {
        let mut out = Self::default();
        let mut models = HashMap::new();
        let mut prices = crate::pricing::Cache::default();
        let mut last_model: Option<usize> = None;
        for lane in &session.lanes {
            let mut total = Totals::default();
            let mut turns: Vec<TurnSummary> = Vec::new();
            let mut index = 0;
            for chunk in lane.events.chunks(256) {
                if !is_current() {
                    return None;
                }
                for event in chunk {
                    if event.kind == Kind::User {
                        turns.push(TurnSummary {
                            index,
                            response: None,
                            events: 0,
                            end: event.start,
                            failures: 0,
                        });
                    } else if let Some(turn) = turns.last_mut() {
                        if event.kind == Kind::Assistant { turn.response = Some(index); }
                        turn.events += 1;
                        turn.end = turn.end.max(event.end);
                        turn.failures += usize::from(event.status.failed());
                    }
                    index += 1;
                    let price = if event.usage.is_empty() { None } else { prices.get(event.model.as_deref()) };
                    total.add(event, price);
                    out.total.add(event, price);
                    if !event.usage.is_empty() {
                        let model = event.model.as_deref().unwrap_or("");
                        let index = match last_model.filter(|&i| out.models[i].0 == model) {
                            Some(i) => i,
                            None => *models.entry(model).or_insert_with(|| {
                                out.models.push((model.to_owned(), UsageSum::default()));
                                out.models.len() - 1
                            }),
                        };
                        last_model = Some(index);
                        out.models[index].1.add_priced(&event.usage, price);
                    }
                }
            }
            out.lanes.push(total);
            out.turns.push(turns);
        }
        out.models
            .sort_by_key(|(_, usage)| std::cmp::Reverse(usage.total()));
        is_current().then_some(out)
    }
}

/// 最後に選んだ一つのセッションだけを保持する。本文への強参照と古い版は保持しない。
#[derive(Default)]
pub struct SessionCache {
    version: Weak<Session>,
    summary: Option<Arc<SessionSummary>>,
}

impl SessionCache {
    pub fn get(
        &mut self,
        session: &Arc<Session>,
        is_current: &impl Fn() -> bool,
    ) -> Option<Arc<SessionSummary>> {
        if !is_current() {
            return None;
        }
        if self.version.as_ptr() != Arc::as_ptr(session) {
            self.summary = None;
            self.version = Arc::downgrade(session);
        }
        if self.summary.is_none() {
            self.summary = Some(Arc::new(SessionSummary::build(session, is_current)?));
        }
        self.summary.clone().filter(|_| is_current())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Lane, Source, Status, Usage};

    fn session() -> Arc<Session> {
        let mut lanes = Vec::new();
        for li in 0..3 {
            let mut lane = Lane::new("summary");
            for i in 0..800 {
                lane.events.push(Event {
                    start: i as f64,
                    end: i as f64 + 1.0,
                    kind: [
                        Kind::User,
                        Kind::Tool,
                        Kind::Agent,
                        Kind::Assistant,
                        Kind::Thinking,
                    ][i % 5],
                    name: ["Read", "Edit", "Agent", "User"][i % 4].into(),
                    summary: Default::default(),
                    input: String::new(),
                    output: String::new(),
                    call_id: None,
                    text_ref: Default::default(),
                    status: [
                        Status::Ok,
                        Status::Error,
                        Status::Rejected,
                        Status::Interrupted,
                        Status::Pending,
                    ][i % 5],
                    usage: if i % 7 == 0 {
                        Usage::default()
                    } else {
                        Usage {
                            input: 100 + i as u32,
                            cache_read: 30,
                            cache_write: 20,
                            cache_write_1h: 10,
                            output: 5 + li,
                            reasoning: 2,
                        }
                    },
                    model: match i % 4 {
                        0 => None,
                        1 => Some("unknown".into()),
                        2 => Some("gpt-6-sol".into()),
                        _ => Some("claude-opus-5-5".into()),
                    },
                });
            }
            lanes.push(lane);
        }
        Arc::new(Session {
            id: "summary".into(),
            source: Source::Codex,
            cwd: String::new(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes,
            start: 0.0,
            end: 800.0,
            parents: Vec::new(),
            tree: Vec::new(),
        })
    }

    fn assert_totals(actual: &Totals, events: impl Iterator<Item = Event>) {
        let events: Vec<_> = events.collect();
        assert_eq!(actual.stats, Stats::of(&events));
        assert_eq!(actual.usage, UsageSum::of(&events));
    }

    #[test]
    fn summaries_match_full_scans_and_preserve_tied_order() {
        let session = session();
        let summary = SessionSummary::build(&session, &|| true).unwrap();
        assert_totals(
            &summary.total,
            session.lanes.iter().flat_map(|l| l.events.iter().cloned()),
        );
        for (total, lane) in summary.lanes.iter().zip(&session.lanes) {
            assert_totals(total, lane.events.iter().cloned());
        }
        for (turns, lane) in summary.turns.iter().zip(&session.lanes) {
            for turn in turns {
                let end = lane.events[turn.index + 1..]
                    .iter()
                    .position(|e| e.kind == Kind::User)
                    .map_or(lane.events.len(), |i| turn.index + 1 + i);
                let events = &lane.events[turn.index + 1..end];
                assert_eq!(turn.response, (turn.index + 1..end).rev()
                    .find(|&i| lane.events[i].kind == Kind::Assistant));
                assert_eq!(turn.events, events.len());
                assert_eq!(turn.failures, Stats::of(events).failures());
                assert_eq!(
                    turn.end,
                    events
                        .iter()
                        .map(|e| e.end)
                        .fold(lane.events[turn.index].start, f64::max)
                );
            }
        }
        assert_eq!(
            summary.models,
            UsageSum::by_model(session.lanes.iter().flat_map(|l| &l.events))
        );
        for events in [
            &session.lanes[0].events[..],
            &session.lanes[0].events[7..411],
            &[][..],
        ] {
            let summary = SelectionSummary::build(events, &|| true).unwrap();
            assert_totals(&summary.total, events.iter().cloned());
            assert_eq!(
                summary.end,
                events
                    .iter()
                    .map(|e| e.end)
                    .fold(events.first().map_or(0.0, |e| e.start), f64::max)
            );
            let mut reference: Vec<(String, usize, usize)> = Vec::new();
            for (index, e) in events.iter().enumerate() {
                if let Some((_, n, _)) = reference.iter_mut().find(|(name, _, _)| *name == e.name.as_ref()) {
                    *n += 1;
                } else {
                    reference.push((e.name.to_string(), 1, index));
                }
            }
            reference.sort_by_key(|(_, n, _)| std::cmp::Reverse(*n));
            assert!(summary.names == reference);
        }
    }

    #[test]
    fn cache_reuses_only_matching_versions_and_releases_sessions() {
        let mut session = session();
        let mut cache = SessionCache::default();
        let first = cache.get(&session, &|| true).unwrap();
        assert!(Arc::ptr_eq(&first, &cache.get(&session, &|| true).unwrap()));
        let old = Arc::downgrade(&session);
        Arc::make_mut(&mut session).lanes[0].events.remove(0);
        assert!(old.upgrade().is_none());
        let updated = cache.get(&session, &|| true).unwrap();
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_totals(
            &updated.total,
            session.lanes.iter().flat_map(|l| l.events.iter().cloned()),
        );
        let weak = Arc::downgrade(&session);
        drop(session);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn repeated_names_and_models_match_full_scans_across_chunks_and_lanes() {
        let mut session = session();
        for (li, lane) in Arc::make_mut(&mut session).lanes.iter_mut().enumerate() {
            for (i, event) in lane.events.iter_mut().enumerate() {
                event.name = ["exec_command", "User", "exec_command"][i / 300].into();
                event.model = match (i / 100 + li) % 5 {
                    0 => None,
                    1 => Some("".into()),
                    2 => Some("unknown".into()),
                    3 => Some("gpt-6-sol".into()),
                    _ => Some("claude-opus-5-5".into()),
                };
            }
        }
        let all = || session.lanes.iter().flat_map(|l| &l.events);
        let summary = SessionSummary::build(&session, &|| true).unwrap();
        assert_eq!(summary.total.stats, Stats::of(all()));
        assert_eq!(summary.total.usage, UsageSum::of(all()));
        assert_eq!(summary.models, UsageSum::by_model(all()));
        for (total, lane) in summary.lanes.iter().zip(&session.lanes) {
            assert_eq!(total.usage, UsageSum::of(&lane.events));
            let selected = SelectionSummary::build(&lane.events, &|| true).unwrap();
            assert_eq!(selected.total.stats, Stats::of(&lane.events));
            assert_eq!(selected.total.usage, UsageSum::of(&lane.events));
            assert_eq!(selected.names, vec![("exec_command".into(), 500, 0), ("User".into(), 300, 300)]);
        }
    }

    #[test]
    fn canceled_summaries_never_enter_cache() {
        use std::cell::Cell;
        let session = session();
        let mut cache = SessionCache::default();
        let checks = Cell::new(0);
        assert!(
            cache
                .get(&session, &|| {
                    checks.set(checks.get() + 1);
                    checks.get() < 4
                })
                .is_none()
        );
        assert!(cache.summary.is_none());
        assert!(cache.get(&session, &|| true).is_some());
        assert!(cache.get(&session, &|| false).is_none());
        let checks = Cell::new(0);
        assert!(
            SelectionSummary::build(&session.lanes[0].events, &|| {
                checks.set(checks.get() + 1);
                checks.get() < 2
            })
            .is_none()
        );
    }
}
