//! キャッシュを使わない照合用の全件集計。テストでのみコンパイルする。

use super::*;
use crate::model::{Event, Snapshot};
use crate::time::local;
use chrono::{Datelike, Duration};
use std::collections::HashMap;

impl Totals {
    fn add(&mut self, e: &Event) {
        use crate::model::Kind;
        self.sessions = 1;
        self.turns += usize::from(e.kind == Kind::User);
        self.tools += usize::from(e.kind.is_tool());
        self.failures += usize::from(e.status.failed());
        self.usage.add(&e.usage, e.model.as_deref());
    }
}

impl Summary {
    /// 二分探索・日付境界の表・キャッシュを使わず、全イベントの日時を変換して数える
    pub(super) fn reference(
        store: &Snapshot,
        order: &[usize],
        period: Period,
        today: NaiveDate,
    ) -> Self {
        use crate::model::Kind;
        use crate::time::midnight;
        use chrono::Timelike;
        use std::collections::HashSet;
        let span = Span::new(Span::from_date(store, order, period, today), period, today);
        let (start, end) = (midnight(span.from), midnight(span.to + Duration::days(1)));
        let previous = span.previous.map(|p| (midnight(p), start));
        let mut out = Self::empty(&span, period);
        let mut models: HashMap<&str, [UsageSum; 2]> = HashMap::new();
        let mut projects: HashMap<&str, [Totals; 2]> = HashMap::new();
        let mut tools: HashMap<(&str, bool), [Calls; 2]> = HashMap::new();
        let mut days = HashSet::new();
        for &si in order {
            let session = &store.sessions[si];
            let src = source_index(session.source);
            let mut current = Totals::default();
            let mut before = Totals::default();
            let mut recorded = false;
            let mut buckets = HashSet::new();
            let mut cells = HashSet::new();
            for e in session.lanes.iter().flat_map(|l| &l.events) {
                if !e.start.is_finite() {
                    continue;
                }
                if previous.is_some_and(|(a, b)| a <= e.start && e.start < b) {
                    before.add(e);
                }
                if !(start <= e.start && e.start < end) {
                    continue;
                }
                current.add(e);
                let tool = e.kind.is_tool();
                let mut usage = UsageSum::default();
                usage.add(&e.usage, e.model.as_deref());
                if !e.usage.is_empty() {
                    recorded = true;
                    models
                        .entry(e.model.as_deref().unwrap_or("モデル不明"))
                        .or_default()[src]
                        .merge(&usage);
                }
                if tool {
                    let calls = &mut tools
                        .entry((e.name.as_ref(), e.kind == Kind::Agent))
                        .or_default()[src];
                    calls.calls += 1;
                    calls.failures += usize::from(e.status.failed());
                }
                let t = local(e.start);
                let date = t.date_naive();
                if date < span.from || date > span.to {
                    continue;
                }
                let amount = Amount {
                    tokens: e.usage.total(),
                    cost: usage.cost,
                    turns: u64::from(e.kind == Kind::User),
                    tools: u64::from(tool),
                    sessions: 0,
                };
                let bucket = span
                    .buckets
                    .iter()
                    .position(|&(a, b)| a <= date && date <= b)
                    .unwrap();
                out.activity[bucket].sources[src].merge(&amount);
                buckets.insert(bucket);
                let cell = date.weekday().num_days_from_monday() as usize * 24 + t.hour() as usize;
                out.hours[cell].merge(&amount);
                cells.insert(cell);
                days.insert(date);
            }
            for b in buckets {
                out.activity[b].sources[src].sessions += 1;
            }
            for &c in &cells {
                out.hours[c].sessions += 1;
            }
            out.count_cells(cells);
            if let Some(p) = &mut out.previous {
                p.merge(&before);
            }
            if current.sessions > 0 {
                out.total.merge(&current);
                out.sources[src].merge(&current);
                projects.entry(&session.cwd).or_default()[src].merge(&current);
                out.recorded_sessions += usize::from(recorded);
            }
        }
        out.active_days = days.len();
        out.sum_cells();
        out.finish(models, projects, tools);
        out
    }
}
