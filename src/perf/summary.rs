use crate::detail::summary::{SelectionSummary, SessionCache, SessionSummary};
use crate::model::{Session, Stats, UsageSum};
use std::sync::Arc;

/// 変更前の再描画時の集計と、背景集計・再利用を同じイベントで比較する。
pub(crate) fn benchmark(session: &Arc<Session>, lane: usize) {
    use std::hint::black_box;
    let events = &session.lanes[lane].events;
    let summary = SessionSummary::build(session, &|| true).unwrap();
    assert_eq!(
        summary.total.stats,
        Stats::of(session.lanes.iter().flat_map(|l| &l.events))
    );
    assert_eq!(
        summary.total.usage,
        UsageSum::of(session.lanes.iter().flat_map(|l| &l.events))
    );
    assert_eq!(
        summary.models,
        UsageSum::by_model(session.lanes.iter().flat_map(|l| &l.events))
    );
    for (total, lane) in summary.lanes.iter().zip(&session.lanes) {
        assert_eq!(total.stats, Stats::of(&lane.events));
        assert_eq!(total.usage, UsageSum::of(&lane.events));
    }
    let selected = SelectionSummary::build(events, &|| true).unwrap();
    assert_eq!(selected.total.stats, Stats::of(events));
    assert_eq!(selected.total.usage, UsageSum::of(events));
    println!(
        "{}",
        serde_json::json!({"reference_equal":true, "session_turns":summary.total.stats.turns, "session_tokens":summary.total.usage.total(), "selected_turns":selected.total.stats.turns, "selected_tokens":selected.total.usage.total(), "models":summary.models.len()})
    );
    println!(
        "{}",
        serde_json::json!({"events":session.lanes.iter().map(|l| l.events.len()).sum::<usize>(), "selected_events":events.len(), "lanes":session.lanes.len()})
    );
    crate::perf::measure("detail_session_reference", 31, || {
        let stats = Stats::of(session.lanes.iter().flat_map(|l| &l.events));
        let usage = UsageSum::of(session.lanes.iter().flat_map(|l| &l.events));
        let models = UsageSum::by_model(session.lanes.iter().flat_map(|l| &l.events));
        let lane_usage = (session.lanes.len() > 1).then(|| UsageSum::of(&session.lanes[lane].events));
        let children: Vec<_> = session
            .lanes
            .iter()
            .skip(1)
            .map(|l| Stats::of(&l.events))
            .collect();
        (stats, usage, models, lane_usage, children)
    });
    crate::perf::measure("detail_session_build", 31, || {
        SessionSummary::build(session, &|| true).unwrap()
    });
    let mut cache = SessionCache::default();
    cache.get(session, &|| true).unwrap();
    crate::perf::measure("detail_session_cached", 31, || {
        cache.get(session, &|| true).unwrap()
    });
    crate::perf::measure("detail_selection_reference", 31, || {
        let mut counts = Vec::<(&str, usize)>::new();
        for e in events {
            if let Some((_, count)) = counts.iter_mut().find(|(name, _)| *name == e.name) {
                *count += 1;
            } else {
                counts.push((&e.name, 1));
            }
        }
        counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        (Stats::of(events), UsageSum::of(events), counts)
    });
    crate::perf::measure("detail_selection_build", 31, || {
        SelectionSummary::build(events, &|| true).unwrap()
    });
    let selection = SelectionSummary::build(events, &|| true).unwrap();
    crate::perf::measure("detail_selection_cached", 31, || {
        (
            black_box(selection.total.stats),
            black_box(selection.total.usage),
            black_box(&selection.names),
        )
    });
}
