//! 期間・読み込み元・モデル・プロジェクト・ツール・時間帯ごとの統計。GPUI に依存しない。

mod cache;
pub use cache::Cache;

#[cfg(test)]
use crate::model::Event;
use crate::model::{Snapshot, Source, UsageSum};
use crate::time::local;
use chrono::{Datelike, Duration, NaiveDate};
use std::collections::HashMap;

/// 集計する期間。全期間以外は今日までの直近の日数
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Period {
    Week,
    Month,
    Quarter,
    All,
}

impl Period {
    /// 期間の日数。全期間は履歴の最初の日から数えるので決まっていない
    pub fn days(self) -> Option<i64> {
        match self {
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::Quarter => Some(90),
            Self::All => None,
        }
    }
}

/// グラフの 1 列の長さ。長い全期間は週・月ごとにまとめる
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Day,
    Week,
    Month,
}

/// 日ごとの列にする全期間の最長の日数
const DAY_UNIT_MAX: i64 = 92;
/// 週ごとの列にする全期間の最長の日数。超えると月ごと
const WEEK_UNIT_MAX: i64 = 371;
/// 時間帯の表のマスの数（曜日 × 時）
pub const HOURS: usize = 7 * 24;

/// 読み込み元の並び（Claude, Codex）での位置
#[inline]
pub fn source_index(s: Source) -> usize {
    usize::from(s == Source::Codex)
}

#[derive(Clone, Default, Debug)]
pub struct Totals {
    /// イベントのあるセッションの数。複数のレーンは 1 件として数える
    pub sessions: usize,
    pub turns: usize,
    /// ツール呼び出し（サブエージェントの起動を含む）
    pub tools: usize,
    /// 失敗・拒否・中断
    pub failures: usize,
    pub usage: UsageSum,
}

impl Totals {
    pub fn merge(&mut self, other: &Self) {
        self.sessions += other.sessions;
        self.turns += other.turns;
        self.tools += other.tools;
        self.failures += other.failures;
        self.usage.merge(&other.usage);
    }
}

/// グラフの列や時間帯のマスに積む量
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Amount {
    pub tokens: u64,
    /// 推定料金（USD）。料金の分からないモデルの分は含まない
    pub cost: f64,
    pub turns: u64,
    pub tools: u64,
    /// その区間にイベントのあるセッションの数
    pub sessions: u64,
}

impl Amount {
    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.tokens += other.tokens;
        self.cost += other.cost;
        self.turns += other.turns;
        self.tools += other.tools;
        self.sessions += other.sessions;
    }
}

/// グラフの 1 列。from〜to の両日を含む
#[derive(Clone, Debug)]
pub struct Bucket {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// 読み込み元ごと（Claude, Codex）
    pub sources: [Amount; 2],
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub struct Calls {
    pub calls: usize,
    /// 失敗・拒否・中断
    pub failures: usize,
}

impl Calls {
    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.calls += other.calls;
        self.failures += other.failures;
    }
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub name: String,
    /// サブエージェントの起動
    pub agent: bool,
    /// 読み込み元ごと（Claude, Codex）
    pub sources: [Calls; 2],
}

pub struct Summary {
    pub period: Period,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub unit: Unit,
    pub total: Totals,
    /// 直前の同じ長さの期間。全期間にはない
    pub previous: Option<Totals>,
    /// 読み込み元ごと（Claude, Codex）
    pub sources: [Totals; 2],
    /// モデルごとの使用量（読み込み元ごと）。トークンの多い順
    pub models: Vec<(String, [UsageSum; 2])>,
    /// 作業ディレクトリごとの集計（読み込み元ごと）。トークンの多い順
    pub projects: Vec<(String, [Totals; 2])>,
    /// 呼び出しの多い順
    pub tools: Vec<Tool>,
    pub activity: Vec<Bucket>,
    /// 曜日（月曜 = 0）× 時（0〜23）の `HOURS` マス。期間内の日をローカル時間で重ねる
    pub hours: Vec<Amount>,
    /// 曜日ごと・時ごとの合計。セッションは重ねずに数える
    pub by_weekday: [Amount; 7],
    pub by_hour: [Amount; 24],
    /// イベントのある日の数
    pub active_days: usize,
    /// 使用量の記録があるセッションの数
    pub recorded_sessions: usize,
}

impl Summary {
    /// 期間の日数（今日を含む）
    pub fn days(&self) -> i64 {
        (self.to - self.from).num_days() + 1
    }

    fn empty(span: &Span, period: Period) -> Self {
        Self {
            period,
            from: span.from,
            to: span.to,
            unit: span.unit,
            total: Totals::default(),
            previous: span.previous.map(|_| Totals::default()),
            sources: Default::default(),
            models: Vec::new(),
            projects: Vec::new(),
            tools: Vec::new(),
            activity: span
                .buckets
                .iter()
                .map(|&(from, to)| Bucket {
                    from,
                    to,
                    sources: Default::default(),
                })
                .collect(),
            hours: vec![Amount::default(); HOURS],
            by_weekday: Default::default(),
            by_hour: Default::default(),
            active_days: 0,
            recorded_sessions: 0,
        }
    }

    /// セッションが触れた時間帯のマスを、曜日ごと・時ごとのセッション数に数える
    fn count_cells(&mut self, cells: impl IntoIterator<Item = usize>) {
        let (mut weekdays, mut hours) = (0u8, 0u32);
        for cell in cells {
            weekdays |= 1 << (cell / 24);
            hours |= 1 << (cell % 24);
        }
        for (d, a) in self.by_weekday.iter_mut().enumerate() {
            a.sessions += u64::from(weekdays >> d & 1);
        }
        for (h, a) in self.by_hour.iter_mut().enumerate() {
            a.sessions += u64::from(hours >> h & 1);
        }
    }

    /// マスの量を曜日ごと・時ごとに足す（セッション数は `count_cells` で数える）
    fn sum_cells(&mut self) {
        for (i, cell) in self.hours.iter().enumerate() {
            for a in [&mut self.by_weekday[i / 24], &mut self.by_hour[i % 24]] {
                a.tokens += cell.tokens;
                a.cost += cell.cost;
                a.turns += cell.turns;
                a.tools += cell.tools;
            }
        }
    }

    /// 集計した表を、件数の多い順・名前順の決まった並びにする
    fn finish(
        &mut self,
        models: HashMap<&str, [UsageSum; 2]>,
        projects: HashMap<&str, [Totals; 2]>,
        tools: HashMap<(&str, bool), [Calls; 2]>,
    ) {
        let tokens = |u: &[UsageSum; 2]| u[0].total() + u[1].total();
        self.models = models.into_iter().map(|(m, u)| (m.to_owned(), u)).collect();
        self.models
            .sort_by(|a, b| tokens(&b.1).cmp(&tokens(&a.1)).then_with(|| a.0.cmp(&b.0)));
        let key = |t: &[Totals; 2]| {
            (
                t[0].usage.total() + t[1].usage.total(),
                t[0].turns + t[1].turns,
            )
        };
        self.projects = projects
            .into_iter()
            .map(|(p, t)| (p.to_owned(), t))
            .collect();
        self.projects
            .sort_by(|a, b| key(&b.1).cmp(&key(&a.1)).then_with(|| a.0.cmp(&b.0)));
        self.tools = tools
            .into_iter()
            .map(|((name, agent), sources)| Tool {
                name: name.to_owned(),
                agent,
                sources,
            })
            .collect();
        self.tools.sort_by(|a, b| {
            let calls = |t: &Tool| t.sources[0].calls + t.sources[1].calls;
            calls(b)
                .cmp(&calls(a))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.agent.cmp(&b.agent))
        });
    }
}

/// 期間の日付と列の区切り。集計の方法によらず同じ規則で決める
#[derive(Clone, PartialEq, Debug)]
struct Span {
    from: NaiveDate,
    to: NaiveDate,
    unit: Unit,
    /// 列ごとの (最初の日, 最後の日)
    buckets: Vec<(NaiveDate, NaiveDate)>,
    /// 直前の同じ長さの期間の最初の日
    previous: Option<NaiveDate>,
}

impl Span {
    fn new(store: &Snapshot, order: &[usize], period: Period, today: NaiveDate) -> Self {
        let from = match period.days() {
            Some(n) => today - Duration::days(n - 1),
            None => {
                // 最も早い開始時刻だけを日付に変換する。
                let first = order
                    .iter()
                    .map(|&si| store.sessions[si].start)
                    .filter(|t| t.is_finite())
                    .fold(f64::INFINITY, f64::min);
                if first.is_finite() {
                    local(first).date_naive().min(today)
                } else {
                    today
                }
            }
        };
        let days = (today - from).num_days() + 1;
        let unit = match period {
            Period::All if days > WEEK_UNIT_MAX => Unit::Month,
            Period::All if days > DAY_UNIT_MAX => Unit::Week,
            _ => Unit::Day,
        };
        let mut buckets = Vec::new();
        let mut day = from;
        while day <= today {
            // 週は月曜から、月は 1 日から。期間の端の列は途中で切れる。
            let last = match unit {
                Unit::Day => day,
                Unit::Week => {
                    day + Duration::days(6 - i64::from(day.weekday().num_days_from_monday()))
                }
                Unit::Month => month_end(day),
            }
            .min(today);
            buckets.push((day, last));
            day = last + Duration::days(1);
        }
        Self {
            from,
            to: today,
            unit,
            buckets,
            previous: period.days().map(|n| from - Duration::days(n)),
        }
    }

    fn days(&self) -> usize {
        ((self.to - self.from).num_days() + 1) as usize
    }
}

fn month_end(d: NaiveDate) -> NaiveDate {
    let (y, m) = if d.month() == 12 {
        (d.year() + 1, 1)
    } else {
        (d.year(), d.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1).map_or(d, |next| next - Duration::days(1))
}

#[cfg(test)]
impl Totals {
    fn add(&mut self, e: &Event) {
        use crate::model::Kind;
        self.sessions = 1;
        self.turns += usize::from(e.kind == Kind::User);
        self.tools += usize::from(matches!(e.kind, Kind::Tool | Kind::Agent));
        self.failures += usize::from(e.status.failed());
        self.usage.add(&e.usage, e.model.as_deref());
    }
}

#[cfg(test)]
impl Summary {
    /// 二分探索・日付境界の表・キャッシュを使わず、全イベントの日時を変換して数える
    fn reference(store: &Snapshot, order: &[usize], period: Period, today: NaiveDate) -> Self {
        use crate::model::Kind;
        use crate::time::midnight;
        use chrono::Timelike;
        use std::collections::HashSet;
        let span = Span::new(store, order, period, today);
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
                let tool = matches!(e.kind, Kind::Tool | Kind::Agent);
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
                        .entry((e.name.as_str(), e.kind == Kind::Agent))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::model::{Kind, Lane, Session, Status, Usage};
    use crate::time::midnight;

    fn event(start: f64, kind: Kind, usage: Usage) -> Event {
        Event {
            start,
            end: start,
            kind,
            usage,
            name: String::new(),
            summary: String::new(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            row: None,
            model: Some("statistics-test-unknown-model".into()),
            status: Status::Ok,
        }
    }

    fn tool(start: f64, name: &str, status: Status) -> Event {
        Event {
            name: name.into(),
            status,
            kind: if matches!(name, "Agent" | "Task" | "spawn_agent") {
                Kind::Agent
            } else {
                Kind::Tool
            },
            ..event(start, Kind::Tool, Usage::default())
        }
    }

    fn session(source: Source, cwd: &str, lanes: Vec<Vec<Event>>) -> Arc<Session> {
        let mut s = Session {
            id: cwd.into(),
            source,
            cwd: cwd.into(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
            lanes: lanes
                .into_iter()
                .enumerate()
                .map(|(i, events)| {
                    let mut lane = Lane::new(if i == 0 { "main" } else { "agent" });
                    lane.events = events;
                    lane.finish();
                    lane
                })
                .collect(),
        };
        s.finalize();
        Arc::new(s)
    }

    fn assert_usage(actual: &UsageSum, expected: &UsageSum) {
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
        assert_cost(actual.cost, expected.cost);
    }

    fn assert_cost(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1e-9 + expected.abs() * 1e-12,
            "料金が一致しません: {actual} / {expected}"
        );
    }

    fn assert_totals(actual: &Totals, expected: &Totals) {
        assert_eq!(
            (actual.sessions, actual.turns, actual.tools, actual.failures),
            (
                expected.sessions,
                expected.turns,
                expected.tools,
                expected.failures
            )
        );
        assert_usage(&actual.usage, &expected.usage);
    }

    fn assert_amount(actual: &Amount, expected: &Amount, at: impl std::fmt::Debug) {
        assert_eq!(
            (actual.tokens, actual.turns, actual.tools, actual.sessions),
            (
                expected.tokens,
                expected.turns,
                expected.tools,
                expected.sessions
            ),
            "{at:?}"
        );
        assert_cost(actual.cost, expected.cost);
    }

    fn assert_summary(actual: &Summary, expected: &Summary) {
        assert_eq!(
            (
                actual.period,
                actual.from,
                actual.to,
                actual.unit,
                actual.recorded_sessions,
                actual.active_days
            ),
            (
                expected.period,
                expected.from,
                expected.to,
                expected.unit,
                expected.recorded_sessions,
                expected.active_days
            )
        );
        assert_totals(&actual.total, &expected.total);
        assert_eq!(actual.previous.is_some(), expected.previous.is_some());
        if let (Some(a), Some(e)) = (&actual.previous, &expected.previous) {
            assert_totals(a, e);
        }
        for (a, e) in actual.sources.iter().zip(&expected.sources) {
            assert_totals(a, e);
        }
        assert_eq!(actual.models.len(), expected.models.len());
        for ((am, au), (em, eu)) in actual.models.iter().zip(&expected.models) {
            assert_eq!(am, em);
            for (a, e) in au.iter().zip(eu) {
                assert_usage(a, e);
            }
        }
        assert_eq!(actual.projects.len(), expected.projects.len());
        for ((ap, at), (ep, et)) in actual.projects.iter().zip(&expected.projects) {
            assert_eq!(ap, ep);
            for (a, e) in at.iter().zip(et) {
                assert_totals(a, e);
            }
        }
        assert_eq!(actual.tools.len(), expected.tools.len());
        for (a, e) in actual.tools.iter().zip(&expected.tools) {
            assert_eq!((&a.name, a.agent, a.sources), (&e.name, e.agent, e.sources));
        }
        assert_eq!(actual.activity.len(), expected.activity.len());
        for (a, e) in actual.activity.iter().zip(&expected.activity) {
            assert_eq!((a.from, a.to), (e.from, e.to));
            for (i, (s, e)) in a.sources.iter().zip(&e.sources).enumerate() {
                assert_amount(s, e, (actual.period, a.from, i));
            }
        }
        assert_eq!(actual.hours.len(), HOURS);
        for (i, (a, e)) in actual.hours.iter().zip(&expected.hours).enumerate() {
            assert_amount(a, e, (actual.period, "hour", i / 24, i % 24));
        }
        for (i, (a, e)) in actual
            .by_weekday
            .iter()
            .zip(&expected.by_weekday)
            .enumerate()
        {
            assert_amount(a, e, (actual.period, "weekday", i));
        }
        for (i, (a, e)) in actual.by_hour.iter().zip(&expected.by_hour).enumerate() {
            assert_amount(a, e, (actual.period, "by_hour", i));
        }
    }

    const PERIODS: [Period; 4] = [Period::Week, Period::Month, Period::Quarter, Period::All];

    fn mixed_history(today: NaiveDate) -> Snapshot {
        let start = midnight(today - Duration::days(200));
        let mut events: Vec<_> = (0..16000)
            .map(|i| {
                let mut event = event(
                    start + i as f64 * 1080.0,
                    if i % 11 == 0 { Kind::User } else { Kind::Tool },
                    Usage {
                        input: i * 100,
                        output: i * 7,
                        cache_read: i * 30,
                        cache_write: i * 5,
                        cache_write_1h: i * 2,
                        reasoning: i,
                    },
                );
                event.model = match i % 5 {
                    0 => None,
                    1 => Some("モデル不明".into()),
                    2 => Some("gpt-6-sol".into()),
                    3 => Some("claude-haiku-4-5-20251001".into()),
                    _ => Some("test-unknown".into()),
                };
                event.name =
                    ["exec_command", "Read", "apply_patch", "spawn_agent"][i as usize % 4].into();
                if event.name == "spawn_agent" && event.kind == Kind::Tool {
                    event.kind = Kind::Agent;
                }
                if i % 13 == 0 {
                    event.status = Status::Rejected;
                }
                event
            })
            .collect();
        events.push(event(f64::NAN, Kind::User, Usage::default()));
        Snapshot {
            sessions: vec![
                session(Source::Claude, "/a", vec![events.clone(), events.clone()]),
                session(Source::Codex, "/b", vec![events.clone()]),
                session(Source::Codex, "/a", vec![events]),
            ],
        }
    }

    #[test]
    fn cached_summaries_match_full_scan_after_updates_and_scope_changes() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let mut store = mixed_history(today);
        let mut cache = Cache::default();
        for period in PERIODS {
            let expected = Summary::reference(&store, &[0, 1, 2], period, today);
            assert_summary(&cache.build(&store, &[0, 1, 2], period, today), &expected);
            assert_eq!(cache.rebuilt, 3);
            assert_summary(&cache.build(&store, &[0, 1, 2], period, today), &expected);
            assert_eq!(cache.rebuilt, 0);
            let scope = &[2, 1];
            assert_summary(
                &cache.build(&store, scope, period, today),
                &Summary::reference(&store, scope, period, today),
            );
            assert_eq!(cache.rebuilt, 0);
        }
        let old = Arc::downgrade(&store.sessions[1]);
        let updated = Arc::make_mut(&mut store.sessions[1]);
        updated.lanes[0].events[15900].usage.input += 1000;
        updated.lanes[0].events[15900].status = Status::Interrupted;
        assert!(
            old.upgrade().is_none(),
            "キャッシュが古い本文を保持しています"
        );
        for period in [Period::Month, Period::Week, Period::Quarter, Period::All] {
            assert_summary(
                &cache.build(&store, &[0, 1, 2], period, today),
                &Summary::reference(&store, &[0, 1, 2], period, today),
            );
            assert_eq!(cache.rebuilt, 1);
        }
        for scope in [vec![1], vec![], vec![2, 0], vec![0, 1, 2]] {
            for period in [Period::All, Period::Week, Period::Quarter, Period::Month] {
                assert_summary(
                    &cache.build(&store, &scope, period, today),
                    &Summary::reference(&store, &scope, period, today),
                );
            }
        }
        store.sessions.remove(0); // 添字が再利用されても別セッションを混同しない。
        for period in PERIODS {
            assert_summary(
                &cache.build(&store, &[0, 1], period, today),
                &Summary::reference(&store, &[0, 1], period, today),
            );
            assert_summary(
                &cache.build(&store, &[0, 1], period, today + Duration::days(1)),
                &Summary::reference(&store, &[0, 1], period, today + Duration::days(1)),
            );
        }
    }

    #[test]
    fn cancellation_does_not_publish_or_cache_a_partial_session() {
        use std::cell::Cell;
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let store = mixed_history(today);
        let mut cache = Cache::default();
        let checks = Cell::new(0);
        assert!(
            cache
                .build_while(&store, &[0, 1, 2], Period::All, today, || {
                    checks.set(checks.get() + 1);
                    checks.get() < 8
                })
                .is_none()
        );
        assert_eq!(cache.rebuilt, 0);
        assert_summary(
            &cache.build(&store, &[0, 1, 2], Period::All, today),
            &Summary::reference(&store, &[0, 1, 2], Period::All, today),
        );
        assert_eq!(cache.rebuilt, 3);
        assert!(
            cache
                .build_while(&store, &[0, 1, 2], Period::All, today, || false)
                .is_none()
        );
        assert_eq!(cache.rebuilt, 0);
        assert_summary(
            &cache.build(&store, &[0, 1, 2], Period::All, today),
            &Summary::reference(&store, &[0, 1, 2], Period::All, today),
        );
        assert_eq!(cache.rebuilt, 0);
    }

    #[test]
    fn local_date_bins_match_reference_across_clock_changes_and_epoch() {
        for today in [
            (2026, 3, 11),
            (2026, 11, 3),
            (2026, 4, 7),
            (2026, 10, 5),
            (2018, 11, 4),
            (1970, 1, 4),
        ] {
            let today = NaiveDate::from_ymd_opt(today.0, today.1, today.2).unwrap();
            let start = midnight(today - Duration::days(130));
            let mut events: Vec<_> = (0..6300)
                .map(|i| event(start + i as f64 * 1800.0, Kind::User, Usage::default()))
                .collect();
            for offset in -129..2 {
                let boundary = midnight(today + Duration::days(offset));
                for delta in [-0.0011, -0.0001, 0.0, 0.0001, 0.0011] {
                    events.push(event(boundary + delta, Kind::User, Usage::default()));
                }
                // 夏時間の切り替わる日は時の計算が変わるので、各時刻の境界も確かめる。
                for hour in [1.0, 2.0, 3.0] {
                    events.push(event(
                        boundary + hour * 3600.0 - 0.0001,
                        Kind::User,
                        Usage::default(),
                    ));
                    events.push(event(
                        boundary + hour * 3600.0,
                        Kind::User,
                        Usage::default(),
                    ));
                }
            }
            let store = Snapshot {
                sessions: vec![session(Source::Codex, "/clock", vec![events])],
            };
            for period in PERIODS {
                assert_summary(
                    &Summary::build(&store, &[0], period, today),
                    &Summary::reference(&store, &[0], period, today),
                );
            }
        }
    }

    #[test]
    fn aggregates_all_lanes_without_double_counting_sessions_or_reasoning() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let t = midnight(today) + 3600.0;
        let mut tool = tool(t + 1.0, "Bash", Status::Error);
        tool.usage = Usage {
            input: 100,
            cache_read: 40,
            cache_write: 20,
            output: 30,
            reasoning: 10,
            ..Usage::default()
        };
        let store = Snapshot {
            sessions: vec![
                session(
                    Source::Claude,
                    "/a",
                    vec![
                        vec![event(t, Kind::User, Usage::default()), tool],
                        vec![event(
                            t + 2.0,
                            Kind::User,
                            Usage {
                                input: 5,
                                output: 10,
                                ..Usage::default()
                            },
                        )],
                    ],
                ),
                session(
                    Source::Codex,
                    "/b",
                    vec![vec![event(t, Kind::User, Usage::default())]],
                ),
            ],
        };
        let s = Summary::build(&store, &[0, 1], Period::Week, today);
        assert_eq!(
            (
                s.total.sessions,
                s.total.turns,
                s.total.tools,
                s.total.failures
            ),
            (2, 3, 1, 1)
        );
        assert_eq!(
            (
                s.total.usage.total(),
                s.total.usage.reasoning,
                s.total.usage.calls
            ),
            (205, 10, 2)
        );
        assert_eq!((s.recorded_sessions, s.active_days), (1, 1));
        assert_eq!(
            (
                s.sources[0].sessions,
                s.sources[0].turns,
                s.sources[1].sessions
            ),
            (1, 2, 1)
        );
        assert_eq!(s.models[0].1[0].total(), 205);
        assert_eq!(s.projects.len(), 2);
        assert_eq!(s.projects[0].1[0].usage.total(), 205);
        let day = s.activity.last().unwrap();
        assert_eq!(
            (
                day.sources[0].tokens,
                day.sources[0].turns,
                day.sources[0].tools,
                day.sources[0].sessions
            ),
            (205, 2, 1, 1)
        );
        assert_eq!((day.sources[1].turns, day.sources[1].sessions), (1, 1));
        let cell = &s.hours[today.weekday().num_days_from_monday() as usize * 24 + 1];
        assert_eq!((cell.turns, cell.tools, cell.sessions), (3, 1, 2));
        assert_eq!(s.hours.iter().map(|h| h.sessions).sum::<u64>(), 2);
        assert_eq!(s.tools.len(), 1);
        assert_eq!(
            (
                s.tools[0].name.as_str(),
                s.tools[0].agent,
                s.tools[0].sources[0]
            ),
            (
                "Bash",
                false,
                Calls {
                    calls: 1,
                    failures: 1
                }
            )
        );
        assert_eq!(s.total.usage.unpriced, 2);
    }

    #[test]
    fn period_uses_event_start_and_half_open_local_day_boundaries() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let start = midnight(today - Duration::days(6));
        let end = midnight(today + Duration::days(1));
        let store = Snapshot {
            sessions: vec![session(
                Source::Codex,
                "/a",
                vec![vec![
                    event(
                        start - 0.001,
                        Kind::User,
                        Usage {
                            input: 100,
                            ..Usage::default()
                        },
                    ),
                    event(start, Kind::User, Usage::default()),
                    event(end - 0.001, Kind::User, Usage::default()),
                    event(
                        end,
                        Kind::User,
                        Usage {
                            input: 200,
                            ..Usage::default()
                        },
                    ),
                ]],
            )],
        };
        let s = Summary::build(&store, &[0], Period::Week, today);
        assert_eq!(
            (s.total.sessions, s.total.turns, s.total.usage.total()),
            (1, 2, 0)
        );
        // 直前の 7 日には期間の始まる直前のイベントだけが入る。
        let previous = s.previous.as_ref().unwrap();
        assert_eq!(
            (previous.sessions, previous.turns, previous.usage.total()),
            (1, 1, 100)
        );
        assert_eq!((s.activity.len(), s.active_days), (7, 2));
        assert_eq!(s.activity[0].sources[1].turns, 1);
        assert_eq!(s.activity[6].sources[1].turns, 1);
        assert!(s.activity[1..6].iter().all(|a| {
            a.sources
                .iter()
                .all(|a| a.tokens == 0 && a.turns == 0 && a.sessions == 0)
        }));
    }

    #[test]
    fn previous_period_has_the_same_length_and_all_time_has_none() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let store = Snapshot {
            sessions: vec![
                session(
                    Source::Claude,
                    "/a",
                    vec![
                        [6, 7, 29, 30, 89, 90, 179, 180]
                            .into_iter()
                            .map(|d| {
                                event(
                                    midnight(today - Duration::days(d)) + 60.0,
                                    Kind::User,
                                    Usage::default(),
                                )
                            })
                            .collect(),
                    ],
                ),
                session(
                    Source::Codex,
                    "/b",
                    vec![vec![event(
                        midnight(today - Duration::days(100)),
                        Kind::User,
                        Usage::default(),
                    )]],
                ),
            ],
        };
        let turns = |period| {
            let s = Summary::build(&store, &[0, 1], period, today);
            (s.total.turns, s.previous.map(|p| (p.turns, p.sessions)))
        };
        assert_eq!(turns(Period::Week), (1, Some((1, 1))));
        assert_eq!(turns(Period::Month), (3, Some((1, 1))));
        assert_eq!(turns(Period::Quarter), (5, Some((3, 2))));
        assert_eq!(turns(Period::All), (9, None));
    }

    #[test]
    fn long_all_time_charts_use_calendar_weeks_and_months() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(); // 月曜
        let all = |days: i64| {
            let store = Snapshot {
                sessions: vec![session(
                    Source::Codex,
                    "/a",
                    vec![vec![
                        event(
                            midnight(today - Duration::days(days - 1)),
                            Kind::User,
                            Usage::default(),
                        ),
                        event(midnight(today), Kind::User, Usage::default()),
                    ]],
                )],
            };
            let s = Summary::build(&store, &[0], Period::All, today);
            assert_summary(&s, &Summary::reference(&store, &[0], Period::All, today));
            assert_eq!(s.days(), days);
            assert_eq!(
                (
                    s.activity.first().unwrap().from,
                    s.activity.last().unwrap().to
                ),
                (s.from, today)
            );
            assert!(
                s.activity
                    .windows(2)
                    .all(|w| w[0].to + Duration::days(1) == w[1].from)
            );
            assert_eq!(
                s.activity.iter().map(|a| a.sources[1].turns).sum::<u64>(),
                2
            );
            s
        };
        let s = all(92);
        assert_eq!((s.unit, s.activity.len()), (Unit::Day, 92));
        let s = all(93);
        assert_eq!(s.unit, Unit::Week);
        assert!(
            s.activity[1..]
                .iter()
                .all(|a| a.from.weekday() == chrono::Weekday::Mon)
        );
        assert_eq!(s.activity.last().unwrap().from, today);
        let months = |s: &Summary| {
            (s.to.year() * 12 + s.to.month() as i32 - s.from.year() * 12 - s.from.month() as i32
                + 1) as usize
        };
        let s = all(372);
        assert_eq!(s.unit, Unit::Month);
        assert!(s.activity[1..].iter().all(|a| a.from.day() == 1));
        assert_eq!(s.activity.len(), months(&s));
        assert_eq!(s.activity.len(), 14);
        let s = all(3001);
        assert_eq!(s.activity.len(), months(&s));
    }

    #[test]
    fn hours_use_local_weekday_and_hour() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(); // 月曜
        // 夏時間の地域でも同じ時計の時刻になるよう、ローカル日時から作る。
        let at = |days: i64, h: u32, m: u32| {
            use chrono::{Local, TimeZone};
            let t = (today - Duration::days(days)).and_hms_opt(h, m, 0).unwrap();
            Local
                .from_local_datetime(&t)
                .earliest()
                .unwrap()
                .timestamp() as f64
        };
        let store = Snapshot {
            sessions: vec![
                session(
                    Source::Claude,
                    "/a",
                    vec![vec![
                        event(at(0, 9, 30), Kind::User, Usage::default()),
                        tool(at(0, 9, 36), "Read", Status::Ok),
                        event(at(7, 9, 6), Kind::User, Usage::default()),
                        event(at(1, 23, 59), Kind::User, Usage::default()),
                    ]],
                ),
                session(
                    Source::Codex,
                    "/b",
                    vec![vec![event(at(14, 9, 0), Kind::User, Usage::default())]],
                ),
            ],
        };
        let s = Summary::build(&store, &[0, 1], Period::Month, today);
        assert_summary(
            &s,
            &Summary::reference(&store, &[0, 1], Period::Month, today),
        );
        let monday_nine = &s.hours[9];
        assert_eq!(
            (monday_nine.turns, monday_nine.tools, monday_nine.sessions),
            (3, 1, 2)
        );
        let sunday_late = &s.hours[6 * 24 + 23];
        assert_eq!((sunday_late.turns, sunday_late.sessions), (1, 1));
        // 同じセッションが別の日の同じ曜日・時に現れても 1 件と数える。
        assert_eq!((s.by_weekday[0].turns, s.by_weekday[0].sessions), (3, 2));
        assert_eq!((s.by_weekday[6].turns, s.by_weekday[6].sessions), (1, 1));
        assert_eq!(
            (
                s.by_hour[9].turns,
                s.by_hour[9].tools,
                s.by_hour[9].sessions
            ),
            (3, 1, 2)
        );
        assert_eq!((s.by_hour[23].turns, s.by_hour[23].sessions), (1, 1));
        assert_eq!(s.by_weekday.iter().map(|a| a.turns).sum::<u64>(), 4);
        assert_eq!(s.hours.iter().map(|h| h.turns).sum::<u64>(), 4);
        assert_eq!(s.active_days, 4);
    }

    #[test]
    fn tools_are_counted_by_name_kind_and_source() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let t = midnight(today) + 60.0;
        let store = Snapshot {
            sessions: vec![
                session(
                    Source::Claude,
                    "/a",
                    vec![
                        vec![
                            tool(t, "Bash", Status::Ok),
                            tool(t + 1.0, "Bash", Status::Error),
                            tool(t + 2.0, "Task", Status::Ok),
                            tool(t + 3.0, "Read", Status::Pending),
                        ],
                        vec![tool(t + 4.0, "Bash", Status::Rejected)],
                    ],
                ),
                session(
                    Source::Codex,
                    "/b",
                    vec![vec![
                        tool(t, "Read", Status::Interrupted),
                        tool(t + 1.0, "spawn_agent", Status::Ok),
                    ]],
                ),
            ],
        };
        let s = Summary::build(&store, &[0, 1], Period::Week, today);
        assert_summary(
            &s,
            &Summary::reference(&store, &[0, 1], Period::Week, today),
        );
        let found: Vec<_> = s
            .tools
            .iter()
            .map(|t| (t.name.as_str(), t.agent, t.sources[0], t.sources[1]))
            .collect();
        let calls = |calls, failures| Calls { calls, failures };
        assert_eq!(
            found,
            [
                ("Bash", false, calls(3, 2), calls(0, 0)),
                ("Read", false, calls(1, 0), calls(1, 1)),
                ("Task", true, calls(1, 0), calls(0, 0)),
                ("spawn_agent", true, calls(0, 0), calls(1, 0)),
            ]
        );
        assert_eq!((s.total.tools, s.total.failures), (7, 3));
    }

    #[test]
    fn respects_scope() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let past = today - Duration::days(3000);
        let store = Snapshot {
            sessions: vec![
                session(
                    Source::Claude,
                    "/excluded",
                    vec![vec![event(midnight(past), Kind::User, Usage::default())]],
                ),
                session(
                    Source::Codex,
                    "/included",
                    vec![vec![
                        event(
                            midnight(past + Duration::days(1)),
                            Kind::User,
                            Usage::default(),
                        ),
                        event(midnight(today), Kind::User, Usage::default()),
                    ]],
                ),
            ],
        };
        let s = Summary::build(&store, &[1], Period::All, today);
        assert_eq!(s.from, past + Duration::days(1));
        assert_eq!(
            (s.total.sessions, s.total.turns, s.projects.len()),
            (1, 2, 1)
        );
        assert_eq!(s.sources[0].sessions, 0);
        assert_eq!(
            s.activity
                .iter()
                .map(|a| a.sources[1].sessions)
                .sum::<u64>(),
            2
        );
    }

    #[test]
    fn empty_scope_and_usage_are_valid() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        for period in PERIODS {
            let s = Summary::build(&Snapshot::default(), &[], period, today);
            assert_eq!(
                (
                    s.total.sessions,
                    s.total.turns,
                    s.total.usage.total(),
                    s.active_days
                ),
                (0, 0, 0, 0)
            );
            assert!(s.models.is_empty() && s.projects.is_empty() && s.tools.is_empty());
            assert_eq!(s.activity.len() as i64, s.days());
            assert!(
                s.activity
                    .iter()
                    .all(|a| a.sources.iter().all(|a| *a == Amount::default()))
            );
            assert!(s.hours.iter().all(|h| *h == Amount::default()));
        }
    }
}
