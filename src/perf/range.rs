use crate::histogram::count_bins;
use crate::model::Snapshot;
use crate::time::{local, midnight};
use chrono::Duration;

/// ミニマップと期間カレンダーが使う集計を、本文・パスを出さずに計測する。
pub(crate) fn benchmark(store: &Snapshot, order: &[usize]) {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let (start, end) = order
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &si| {
            (a.min(store.sessions[si].start), b.max(store.sessions[si].end))
        });
    if !start.is_finite() || !end.is_finite() || end <= start {
        println!("{}", serde_json::json!({"empty_range":true}));
        return;
    }
    let padding = (end - start) * 0.02;
    let (a, b) = (start - padding, end + padding);
    for bins in [42, 256, 1024] {
        let edges: Vec<_> = (0..=bins).map(|i| a + (b - a) * i as f64 / bins as f64).collect();
        let counts = count_bins(store, order, &edges);
        let mut hash = DefaultHasher::new();
        counts.hash(&mut hash);
        println!(
            "{}",
            serde_json::json!({"bins":bins, "events":counts.iter().map(|&n| u64::from(n)).sum::<u64>(),
            "counts_hash":format!("{:016x}", hash.finish())})
        );
        crate::perf::measure(&format!("range_bins_{bins}"), 31, || {
            count_bins(store, order, &edges)
        });
    }
    let day = local(end).date_naive();
    let edges: Vec<_> = (0..=42)
        .map(|i| midnight(day - Duration::days(35) + Duration::days(i)))
        .collect();
    let counts = count_bins(store, order, &edges);
    let mut hash = DefaultHasher::new();
    counts.hash(&mut hash);
    println!(
        "{}",
        serde_json::json!({"range":"calendar", "events":counts.iter().map(|&n| u64::from(n)).sum::<u64>(),
        "counts_hash":format!("{:016x}", hash.finish())})
    );
    crate::perf::measure("range_calendar", 31, || count_bins(store, order, &edges));
}
