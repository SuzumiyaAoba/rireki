//! 期間ごとにセッションの版を弱参照で識別し、更新されたセッションだけ集計し直す。

use super::*;
use crate::model::{Event, Kind, Session};
use crate::time::midnight;
use chrono::{Local, TimeZone, Timelike};
use std::collections::hash_map::Entry;
use std::sync::{Arc, Weak};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
/// 取り消しを確かめる間隔（イベント数）
const CHUNK: usize = 256;

#[derive(Default)]
pub struct Cache {
    periods: Vec<PeriodCache>,
    /// 直前の集計で作り直したセッションの数
    pub rebuilt: usize,
}

struct PeriodCache {
    period: Period,
    range: Range,
    sessions: HashMap<usize, CachedSession>,
}

/// 期間の日付と、イベントの日・時を求める境界
struct Range {
    span: Span,
    start: f64,
    end: f64,
    previous: Option<(f64, f64)>,
    /// span.from からの各日の午前 0 時（ミリ秒）と、末尾に翌日の午前 0 時。
    /// 午前 0 時のない日があれば None とし、イベントごとに日付を変換する
    days: Option<Vec<i64>>,
    /// 日ごとの列
    day_bucket: Vec<u32>,
    /// span.from の曜日（月曜 = 0）
    weekday: usize,
}

impl Range {
    fn new(span: Span) -> Self {
        let start = midnight(span.from);
        let end = midnight(span.to + Duration::days(1));
        let previous = span.previous.map(|p| (midnight(p), start));
        // 日付から境界を作り、23/25 時間の日も同じ日付に入れる。
        let days: Option<Vec<i64>> = (0..=span.days() as i64)
            .map(|i| {
                Local
                    .from_local_datetime(
                        &(span.from + Duration::days(i))
                            .and_hms_opt(0, 0, 0)
                            .unwrap(),
                    )
                    .earliest()
                    .map(|t| t.timestamp_millis())
            })
            .collect();
        // 曖昧な午前 0 時（夏時間の終わり）では、境界が当日の最初の時刻にならないことがある。
        // 境界の直前が前日・境界が当日になっていなければ、イベントごとに日付を変換する。
        let date = |ms: i64| {
            Local
                .timestamp_millis_opt(ms)
                .single()
                .map(|t| t.date_naive())
        };
        let days = days.filter(|d| {
            d.windows(2).all(|w| w[0] < w[1])
                && d.iter().enumerate().all(|(i, &b)| {
                    let day = span.from + Duration::days(i as i64);
                    date(b) == Some(day) && date(b - 1) == Some(day - Duration::days(1))
                })
        });
        let mut day_bucket = Vec::with_capacity(span.days());
        for (i, &(a, b)) in span.buckets.iter().enumerate() {
            day_bucket.extend(std::iter::repeat_n(
                i as u32,
                (b - a).num_days() as usize + 1,
            ));
        }
        let weekday = span.from.weekday().num_days_from_monday() as usize;
        Self {
            span,
            start,
            end,
            previous,
            days,
            day_bucket,
            weekday,
        }
    }

    /// 同じ期間か。日付と両端の時刻が同じなら、日の境界は作り直さない
    fn matches(&self, span: &Span) -> bool {
        self.span == *span
            && self.start == midnight(span.from)
            && self.end == midnight(span.to + Duration::days(1))
    }

    /// 開始時刻の (期間内の日, 時間帯のマス)。`cursor` はレーン内の直前の日で、時刻順に呼ぶと探索を省ける
    #[inline]
    fn locate(&self, start: f64, cursor: &mut Option<usize>) -> Option<(usize, usize)> {
        let Some(days) = &self.days else {
            let t = local(start);
            let date = t.date_naive();
            if date < self.span.from || date > self.span.to {
                return None;
            }
            let day = (date - self.span.from).num_days() as usize;
            return Some((
                day,
                date.weekday().num_days_from_monday() as usize * 24 + t.hour() as usize,
            ));
        };
        // local() と同じミリ秒への切り捨て。Unix epoch 前の小数秒も一致させる。
        let millis = (start * 1000.0) as i64;
        if millis < days[0] || millis >= days[days.len() - 1] {
            return None;
        }
        let mut day = cursor.unwrap_or_else(|| days.partition_point(|&b| b <= millis) - 1);
        while days[day + 1] <= millis {
            day += 1;
        }
        *cursor = Some(day);
        let hour = if days[day + 1] - days[day] == DAY_MS {
            ((millis - days[day]) / HOUR_MS) as usize
        } else {
            // 夏時間の切り替わる日は、経過時間と時計の時がずれる。
            local(start).hour() as usize
        };
        Some((day, (self.weekday + day) % 7 * 24 + hour))
    }
}

/// 1 セッションの期間内の集計
struct CachedSession {
    version: Weak<Session>,
    total: Totals,
    previous: Totals,
    models: Vec<(String, UsageSum)>,
    tools: Vec<(String, bool, Calls)>,
    /// イベントのある列とその量（列の順）
    buckets: Vec<(u32, Amount)>,
    /// イベントのある時間帯のマスとその量
    cells: Vec<(u8, Amount)>,
    /// イベントのある日（期間の最初の日から）
    days: Vec<u32>,
    recorded: bool,
}

/// セッションの集計に使う作業領域。使った所だけを次のセッションのために戻す
struct Scratch {
    buckets: Vec<Amount>,
    cells: Vec<Amount>,
    days: Vec<bool>,
    touched_buckets: Vec<u32>,
    touched_cells: Vec<u8>,
    touched_days: Vec<u32>,
}

impl Scratch {
    fn new(range: &Range) -> Self {
        Self {
            buckets: vec![Amount::default(); range.span.buckets.len()],
            cells: vec![Amount::default(); HOURS],
            days: vec![false; range.day_bucket.len()],
            touched_buckets: Vec::new(),
            touched_cells: Vec::new(),
            touched_days: Vec::new(),
        }
    }

    #[inline]
    fn add(&mut self, bucket: usize, cell: usize, day: usize, amount: &Amount) {
        let b = &mut self.buckets[bucket];
        if b.sessions == 0 {
            b.sessions = 1;
            self.touched_buckets.push(bucket as u32);
        }
        b.merge(amount);
        let c = &mut self.cells[cell];
        if c.sessions == 0 {
            c.sessions = 1;
            self.touched_cells.push(cell as u8);
        }
        c.merge(amount);
        if !std::mem::replace(&mut self.days[day], true) {
            self.touched_days.push(day as u32);
        }
    }

    /// 触った所を取り出して空に戻す
    fn take(&mut self, out: &mut CachedSession) {
        self.touched_buckets.sort_unstable();
        out.buckets = self
            .touched_buckets
            .drain(..)
            .map(|b| (b, std::mem::take(&mut self.buckets[b as usize])))
            .collect();
        self.touched_cells.sort_unstable();
        out.cells = self
            .touched_cells
            .drain(..)
            .map(|c| (c, std::mem::take(&mut self.cells[c as usize])))
            .collect();
        self.touched_days.sort_unstable();
        for &d in &self.touched_days {
            self.days[d as usize] = false;
        }
        out.days = std::mem::take(&mut self.touched_days);
    }
}

/// 名前ごとの件数。セッション内の種類は少なく、同じ名前が続きやすいので、直前の位置から探す
struct Tally<K, V> {
    items: Vec<(K, V)>,
    last: usize,
}

impl<K: PartialEq + Copy, V: Default> Tally<K, V> {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            last: 0,
        }
    }

    #[inline]
    fn entry(&mut self, key: K) -> &mut V {
        if self.items.get(self.last).is_none_or(|(k, _)| *k != key) {
            self.last = match self.items.iter().position(|(k, _)| *k == key) {
                Some(i) => i,
                None => {
                    self.items.push((key, V::default()));
                    self.items.len() - 1
                }
            };
        }
        &mut self.items[self.last].1
    }
}

/// モデルの料金を引いた使用量。料金表は同じモデルで使い回す
#[inline]
fn priced<'a>(e: &'a Event, prices: &mut crate::pricing::Cache<'a>) -> UsageSum {
    let price = prices.get(e.model.as_deref());
    let mut usage = UsageSum::default();
    usage.add_priced(&e.usage, price);
    usage
}

impl CachedSession {
    fn build<'a>(
        session: &'a Arc<Session>,
        range: &Range,
        prices: &mut crate::pricing::Cache<'a>,
        scratch: &mut Scratch,
        keep_running: &impl Fn() -> bool,
    ) -> Option<Self> {
        let mut out = Self {
            version: Arc::downgrade(session),
            total: Totals::default(),
            previous: Totals::default(),
            models: Vec::new(),
            tools: Vec::new(),
            buckets: Vec::new(),
            cells: Vec::new(),
            days: Vec::new(),
            recorded: false,
        };
        let mut models: Tally<&str, UsageSum> = Tally::new();
        let mut tools: Tally<(&str, bool), Calls> = Tally::new();
        for lane in &session.lanes {
            if let Some((a, b)) = range.previous {
                let first = lane.events.partition_point(|e| e.start < a);
                let last = lane.events.partition_point(|e| e.start < b);
                for chunk in lane.events[first..last].chunks(CHUNK) {
                    if !keep_running() {
                        return None;
                    }
                    for e in chunk {
                        let p = &mut out.previous;
                        p.sessions = 1;
                        p.turns += usize::from(e.kind == Kind::User);
                        p.tools += usize::from(matches!(e.kind, Kind::Tool | Kind::Agent));
                        p.failures += usize::from(e.status.failed());
                        if !e.usage.is_empty() {
                            p.usage.merge(&priced(e, prices));
                        }
                    }
                }
            }
            let first = lane.events.partition_point(|e| e.start < range.start);
            let last = lane.events.partition_point(|e| e.start < range.end);
            let mut cursor = None;
            for chunk in lane.events[first..last].chunks(CHUNK) {
                if !keep_running() {
                    return None;
                }
                for e in chunk {
                    if !e.start.is_finite() {
                        continue;
                    }
                    let user = e.kind == Kind::User;
                    let tool = matches!(e.kind, Kind::Tool | Kind::Agent);
                    let failed = e.status.failed();
                    out.total.sessions = 1;
                    out.total.turns += usize::from(user);
                    out.total.tools += usize::from(tool);
                    out.total.failures += usize::from(failed);
                    let mut cost = 0.0;
                    if !e.usage.is_empty() {
                        let usage = priced(e, prices);
                        cost = usage.cost;
                        out.total.usage.merge(&usage);
                        models
                            .entry(e.model.as_deref().unwrap_or("モデル不明"))
                            .merge(&usage);
                        out.recorded = true;
                    }
                    if tool {
                        let calls = tools.entry((e.name.as_ref(), e.kind == Kind::Agent));
                        calls.calls += 1;
                        calls.failures += usize::from(failed);
                    }
                    if let Some((day, cell)) = range.locate(e.start, &mut cursor) {
                        let amount = Amount {
                            tokens: e.usage.total(),
                            cost,
                            turns: u64::from(user),
                            tools: u64::from(tool),
                            sessions: 0,
                        };
                        scratch.add(range.day_bucket[day] as usize, cell, day, &amount);
                    }
                }
            }
        }
        scratch.take(&mut out);
        out.models = models
            .items
            .into_iter()
            .map(|(m, u)| (m.to_owned(), u))
            .collect();
        out.tools = tools
            .items
            .into_iter()
            .map(|((name, agent), calls)| (name.to_owned(), agent, calls))
            .collect();
        Some(out)
    }
}

impl Summary {
    pub fn build(store: &Snapshot, order: &[usize], period: Period, today: NaiveDate) -> Self {
        Cache::default().build(store, order, period, today)
    }
}

impl Cache {
    pub fn build(
        &mut self,
        store: &Snapshot,
        order: &[usize],
        period: Period,
        today: NaiveDate,
    ) -> Summary {
        self.build_while(store, order, period, today, || true)
            .unwrap()
    }

    pub fn build_while(
        &mut self,
        store: &Snapshot,
        order: &[usize],
        period: Period,
        today: NaiveDate,
        keep_running: impl Fn() -> bool,
    ) -> Option<Summary> {
        self.rebuilt = 0;
        if !keep_running() {
            return None;
        }
        let span = Span::new(store, order, period, today);
        let cache = match self.periods.iter().position(|p| p.period == period) {
            Some(index) => {
                let cache = &mut self.periods[index];
                if !cache.range.matches(&span) {
                    cache.sessions.clear();
                    cache.range = Range::new(span);
                }
                cache
            }
            None => {
                self.periods.push(PeriodCache {
                    period,
                    range: Range::new(span),
                    sessions: HashMap::new(),
                });
                self.periods.last_mut().unwrap()
            }
        };
        cache.sessions.retain(|&si, cached| {
            store
                .sessions
                .get(si)
                .is_some_and(|s| cached.version.ptr_eq(&Arc::downgrade(s)))
        });
        let range = &cache.range;
        // 先に不足分を作り、キャッシュを借用しながら結果を結合する。
        let mut prices = crate::pricing::Cache::default();
        let mut scratch = None;
        for &si in order {
            if !keep_running() {
                return None;
            }
            if let Entry::Vacant(entry) = cache.sessions.entry(si) {
                let scratch = scratch.get_or_insert_with(|| Scratch::new(range));
                let contribution = CachedSession::build(
                    &store.sessions[si],
                    range,
                    &mut prices,
                    scratch,
                    &keep_running,
                )?;
                entry.insert(contribution);
                self.rebuilt += 1;
            }
        }
        let mut out = Summary::empty(&range.span, period);
        let mut models: HashMap<&str, [UsageSum; 2]> = HashMap::new();
        let mut projects: HashMap<&str, [Totals; 2]> = HashMap::new();
        let mut tools: HashMap<(&str, bool), [Calls; 2]> = HashMap::new();
        let mut days = vec![false; range.day_bucket.len()];
        for &si in order {
            if !keep_running() {
                return None;
            }
            let session = &store.sessions[si];
            let src = source_index(session.source);
            let c = &cache.sessions[&si];
            if let Some(previous) = &mut out.previous {
                previous.merge(&c.previous);
            }
            if c.total.sessions == 0 {
                continue;
            }
            out.total.merge(&c.total);
            out.sources[src].merge(&c.total);
            projects.entry(&session.cwd).or_default()[src].merge(&c.total);
            out.recorded_sessions += usize::from(c.recorded);
            for (model, usage) in &c.models {
                models.entry(model).or_default()[src].merge(usage);
            }
            for (name, agent, calls) in &c.tools {
                tools.entry((name, *agent)).or_default()[src].merge(calls);
            }
            for (bucket, amount) in &c.buckets {
                out.activity[*bucket as usize].sources[src].merge(amount);
            }
            for (cell, amount) in &c.cells {
                out.hours[*cell as usize].merge(amount);
            }
            out.count_cells(c.cells.iter().map(|(cell, _)| *cell as usize));
            for &day in &c.days {
                days[day as usize] = true;
            }
        }
        out.active_days = days.iter().filter(|&&d| d).count();
        out.sum_cells();
        out.finish(models, projects, tools);
        Some(out)
    }
}
