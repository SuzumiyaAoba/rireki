use crate::model::{Snapshot, UsageSum};
use crate::statistics::{Amount, Cache, Period, Summary, Totals};
use crate::time::midnight;
use chrono::{Duration, NaiveDate};
use std::sync::Arc;

/// 本文やパスを出さず、全集計値の一致と集計の所要時間を測る。
pub(crate) fn benchmark(store: &Snapshot, order: &[usize], today: NaiveDate) {
    use std::hash::{DefaultHasher, Hash, Hasher};
    fn usage_hash(u: &UsageSum, h: &mut DefaultHasher) {
        (
            u.input,
            u.cache_read,
            u.cache_write,
            u.output,
            u.reasoning,
            u.calls,
            u.unpriced,
        )
            .hash(h);
        // セッション単位の和でも比較できるよう、料金だけ 100 万分の 1 USD で丸める。
        ((u.cost * 1e6).round() as u64).hash(h);
    }
    fn totals_hash(t: &Totals, h: &mut DefaultHasher) {
        (t.sessions, t.turns, t.tools, t.failures).hash(h);
        usage_hash(&t.usage, h);
    }
    fn amount_hash(a: &Amount, h: &mut DefaultHasher) {
        (a.tokens, a.turns, a.tools, a.sessions).hash(h);
        ((a.cost * 1e6).round() as u64).hash(h);
    }
    for (period, name) in [
        (Period::Week, "week"),
        (Period::Month, "month"),
        (Period::Quarter, "quarter"),
        (Period::All, "all"),
    ] {
        let summary = Summary::build(store, order, period, today);
        let mut h = DefaultHasher::new();
        (
            summary.from,
            summary.to,
            summary.recorded_sessions,
            summary.active_days,
        )
            .hash(&mut h);
        format!("{:?}", summary.unit).hash(&mut h);
        totals_hash(&summary.total, &mut h);
        if let Some(previous) = &summary.previous {
            totals_hash(previous, &mut h);
        }
        for t in &summary.sources {
            totals_hash(t, &mut h);
        }
        for (m, u) in &summary.models {
            m.hash(&mut h);
            u.iter().for_each(|u| usage_hash(u, &mut h));
        }
        for (p, t) in &summary.projects {
            p.hash(&mut h);
            t.iter().for_each(|t| totals_hash(t, &mut h));
        }
        for t in &summary.tools {
            // 内訳と所要時間を追加する前と比べられるよう、件数と失敗だけを使う。
            (&t.name, t.agent, t.sources.map(|c| (c.calls, c.failures))).hash(&mut h);
        }
        for a in &summary.activity {
            (a.from, a.to).hash(&mut h);
            a.sources.iter().for_each(|a| amount_hash(a, &mut h));
        }
        summary.hours.iter().for_each(|a| amount_hash(a, &mut h));
        summary
            .by_weekday
            .iter()
            .chain(&summary.by_hour)
            .for_each(|a| amount_hash(a, &mut h));
        // 失敗の推移・ツールの内訳と所要時間・セッションごとの集計は別のハッシュにする。
        let mut detail = DefaultHasher::new();
        for a in summary.activity.iter().flat_map(|b| &b.sources).chain(&summary.hours)
            .chain(&summary.by_weekday).chain(&summary.by_hour) {
            a.failures.hash(&mut detail);
        }
        for t in &summary.tools {
            (&t.name, t.agent, t.sources, t.durations.count()).hash(&mut detail);
            for p in [50, 95, 100] {
                t.durations.percentile(p).map(f64::to_bits).hash(&mut detail);
            }
        }
        for row in &summary.sessions {
            row.index.hash(&mut detail);
            totals_hash(&row.total, &mut detail);
        }
        println!(
            "{}",
            serde_json::json!({"period":name,"sessions":summary.total.sessions,
            "turns":summary.total.turns,"tokens":summary.total.usage.total(),
            "summary_hash":format!("{:016x}",h.finish()),
            "detail_hash":format!("{:016x}",detail.finish())})
        );
        crate::perf::measure(&format!("statistics_{name}"), 15, || {
            Summary::build(store, order, period, today)
        });
        let mut cache = Cache::default();
        cache.build(store, order, period, today);
        crate::perf::measure(&format!("statistics_cached_{name}"), 31, || {
            let summary = cache.build(store, order, period, today);
            assert_eq!(cache.rebuilt, 0);
            summary
        });
        let start = midnight(summary.from);
        let end = midnight(today + Duration::days(1));
        if let Some(si) = order.iter().copied().max_by_key(|&si| {
            store.sessions[si]
                .lanes
                .iter()
                .map(|l| {
                    l.events
                        .iter()
                        .filter(|e| e.start >= start && e.start < end)
                        .count()
                })
                .sum::<usize>()
        }) {
            let mut changed = store.clone();
            let session = Arc::make_mut(&mut changed.sessions[si]);
            if let Some(event) = session
                .lanes
                .iter_mut()
                .flat_map(|l| &mut l.events)
                .find(|e| e.start >= start && e.start < end)
            {
                event.usage.input = event.usage.input.saturating_add(1);
            }
            let mut use_changed = false;
            crate::perf::measure(&format!("statistics_updated_{name}"), 15, || {
                use_changed = !use_changed;
                let summary = cache.build(
                    if use_changed { &changed } else { store },
                    order,
                    period,
                    today,
                );
                assert_eq!(cache.rebuilt, 1);
                summary
            });
        }
    }
}
