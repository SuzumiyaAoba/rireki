//! 期間ごとにセッションの版を弱参照で識別し、更新されたセッションだけ集計し直す。

use super::*;
use super::durations::{BUCKETS, bucket, pack, unpack_into};
use crate::model::{Event, Kind, Session, Status};
use crate::time::midnight;
use chrono::{Datelike, Duration, Local, TimeZone, Timelike};
use crate::model::Snapshot;
use crate::time::local;
use std::collections::hash_map::{Entry, RandomState};
use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};
use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
/// 取り消しを確かめる間隔（イベント数）
const CHUNK: usize = 256;
/// 前回の種類数による予約の上限。小さな絞り込みでも大きく確保しない。
const MERGED_RESERVE: usize = 256;

#[derive(Default)]
pub struct Cache {
    periods: Vec<PeriodCache>,
    /// 直前の集計で作り直したセッションの数
    pub rebuilt: usize,
}

struct PeriodCache {
    period: Period,
    range: Range,
    sessions: HashMap<usize, CachedSession, BuildHasherDefault<SessionHash>>,
    hashes: RandomState,
    names: Names,
    merged_counts: [u16; 2],
}

use crate::model::SessionIndexHasher as SessionHash;

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
    fn matches(&self, from: NaiveDate, to: NaiveDate) -> bool {
        // 同じ PeriodCache 内では、列・集約単位・前期間も両端の日付から一意に決まる。
        self.span.from == from && self.span.to == to
            && self.start == midnight(from)
            && self.end == midnight(to + Duration::days(1))
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
    models: Vec<(Option<Arc<str>>, u64, UsageSum)>,
    tools: Vec<CachedTool>,
    /// イベントのある列とその量（列の順）
    buckets: Vec<(u32, Amount)>,
    /// イベントのある時間帯のマスとその量
    cells: Vec<(u8, Amount)>,
    /// イベントのある日（期間の最初の日から）
    days: Vec<u32>,
    meta: SessionMeta,
}

/// セッション内のツールごとの件数と所要時間
struct CachedTool {
    name: Arc<str>,
    agent: bool,
    hash: u64,
    calls: Calls,
    /// 所要時間の最長（秒）と、`pack` で詰めた区間ごとの件数（区間の順）
    max: f64,
    durations: Box<[u32]>,
}

/// セッションを走査する間のツールごとの件数。所要時間の区間は `Scratch` に数える
#[derive(Default)]
struct ToolTally {
    calls: Calls,
    max: f64,
}

impl ToolTally {
    /// 呼び出しを数え、結果があれば所要時間の区間を返す
    #[inline]
    fn add(&mut self, e: &Event) -> Option<u8> {
        let calls = &mut self.calls;
        calls.calls += 1;
        match e.status {
            Status::Pending => {
                calls.pending += 1;
                return None;
            }
            Status::Ok => {}
            Status::Error => calls.failures += 1,
            Status::Rejected => {
                calls.failures += 1;
                calls.rejected += 1;
            }
            Status::Interrupted => {
                calls.failures += 1;
                calls.interrupted += 1;
            }
        }
        // 詳細の分析と同じ経過時間。欠損や不正な値は 0 として数える。
        let seconds = crate::detail::analysis::duration(e);
        self.max = self.max.max(seconds);
        Some(bucket(seconds))
    }
}

/// 期間内の結合で、ツールごとの所要時間の区間を連続した領域に数える
#[derive(Default)]
struct ToolMerge {
    sources: [Calls; 2],
    /// `hist` 内の位置。所要時間がなければ None
    slot: Option<usize>,
    /// 件数のある区間の範囲。結合後はこの範囲だけを読む
    first: u8,
    last: u8,
    max: f64,
}

/// 使用量の有無とパスのハッシュを一語に収め、boolの後の余白を使う。
/// ハッシュの下位56ビットを保持し、検索時は上位ビットにも分散させる。
/// 衝突時はパス全体を照合する。
#[derive(Default)]
struct SessionMeta(AtomicU64);

impl SessionMeta {
    const HASH: u64 = 0x00ff_ffff_ffff_ffff;
    const RECORDED: u64 = 1 << 63;
    const HASHED: u64 = 1 << 62;

    fn set_project_hash(&self, hash: u64) -> u64 {
        let value = (self.0.load(Ordering::Relaxed) & Self::RECORDED) | Self::HASHED | (hash & Self::HASH);
        self.0.store(value, Ordering::Relaxed);
        value
    }

    fn record(&mut self) {
        *self.0.get_mut() |= Self::RECORDED;
    }

    fn recorded(&self) -> bool {
        self.0.load(Ordering::Relaxed) & Self::RECORDED != 0
    }

    #[inline]
    fn project_hash(&self, path: &str, hashes: &RandomState) -> u64 {
        // 名前の参照を借りたまま、数値の目印だけを書き戻す。
        // 同じセッションの版と期間では値が一定で、他のデータの公開には使わない。
        let saved = self.0.load(Ordering::Relaxed);
        let value = if saved & Self::HASHED == 0 {
            self.set_project_hash(hashes.hash_one(path))
        } else {
            saved
        };
        (value & Self::HASH).wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }
}

/// 同じ期間のランダムな種で計算済みのハッシュ。衝突時は名前の内容も照合する。
struct HashedKey<K> {
    hash: u64,
    key: K,
}

impl<K: TallyKey> PartialEq for HashedKey<K> {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.key.same(other.key)
    }
}

impl<K: TallyKey> Eq for HashedKey<K> {}

impl<K> Hash for HashedKey<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

#[derive(Default)]
struct StoredHash(u64);

impl Hasher for StoredHash {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write_u64(&mut self, hash: u64) {
        self.0 = hash;
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("HashedKey writes a single u64");
    }
}

/// 検索表には配列の位置だけを置き、集計値は連続した配列に持つ。
/// 同じハッシュの別名だけ別表に持ち、名前の内容も照合する。
struct Merged<K, V> {
    heads: HashMap<u64, usize, BuildHasherDefault<StoredHash>>,
    collisions: HashMap<HashedKey<K>, usize, BuildHasherDefault<StoredHash>>,
    rows: Vec<(HashedKey<K>, V)>,
    last: usize,
}

impl<K, V> Default for Merged<K, V> {
    fn default() -> Self {
        Self { heads: HashMap::default(), collisions: HashMap::default(), rows: Vec::new(), last: 0 }
    }
}

impl<K: TallyKey, V: Default> Merged<K, V> {
    fn with_capacity(count: usize) -> Self {
        Self {
            heads: HashMap::with_capacity_and_hasher(count, BuildHasherDefault::default()),
            rows: Vec::with_capacity(count),
            ..Self::default()
        }
    }

    #[inline]
    fn entry(&mut self, key: HashedKey<K>) -> &mut V {
        if self.rows.get(self.last).is_some_and(|(previous, _)| *previous == key) {
            return &mut self.rows[self.last].1;
        }
        let index = match self.heads.entry(key.hash) {
            Entry::Occupied(entry) => {
                let index = *entry.get();
                if self.rows[index].0 == key {
                    index
                } else {
                    match self.collisions.entry(key) {
                        Entry::Occupied(entry) => *entry.get(),
                        Entry::Vacant(entry) => {
                            let index = self.rows.len();
                            let key = HashedKey { hash: entry.key().hash, key: entry.key().key };
                            self.rows.push((key, V::default()));
                            entry.insert(index);
                            index
                        }
                    }
                }
            }
            Entry::Vacant(entry) => {
                let index = self.rows.len();
                self.rows.push((key, V::default()));
                entry.insert(index);
                index
            }
        };
        self.last = index;
        &mut self.rows[index].1
    }

    fn into_iter(self) -> impl Iterator<Item = (HashedKey<K>, V)> {
        // 結果配列への入力領域の再利用で、拡張時の余剰容量を引き継がない。
        self.rows.into_boxed_slice().into_vec().into_iter()
    }
}

/// 同じ名前は期間内で一つの参照にそろえる。名前だけを弱参照し、履歴は保持しない。
#[derive(Default)]
struct Names(HashMap<u64, Weak<str>, BuildHasherDefault<StoredHash>>);

impl Names {
    fn get(&mut self, name: &Arc<str>, hash: u64) -> Arc<str> {
        match self.0.entry(hash) {
            Entry::Occupied(mut entry) => match entry.get().upgrade() {
                Some(shared) if shared.as_ref().same(name.as_ref()) => shared,
                // 同じハッシュの別名は合流せず、元の参照を使う。
                Some(_) => name.clone(),
                None => {
                    entry.insert(Arc::downgrade(name));
                    name.clone()
                }
            },
            Entry::Vacant(entry) => {
                entry.insert(Arc::downgrade(name));
                name.clone()
            }
        }
    }
}

/// セッションの集計に使う作業領域。使った所だけを次のセッションのために戻す
struct Scratch {
    buckets: Vec<Amount>,
    cells: Vec<Amount>,
    days: Vec<bool>,
    touched_buckets: Vec<u32>,
    touched_cells: Vec<u8>,
    touched_days: Vec<u32>,
    /// セッション内のツールの位置 × 区間の件数と、件数のある位置
    durations: Vec<u32>,
    touched_durations: Vec<u32>,
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
            durations: Vec::new(),
            touched_durations: Vec::new(),
        }
    }

    /// セッション内の `slot` 番目のツールの所要時間を数える
    #[inline]
    fn add_duration(&mut self, slot: usize, bucket: u8) {
        let i = slot * BUCKETS + usize::from(bucket);
        if i >= self.durations.len() {
            self.durations.resize((slot + 1) * BUCKETS, 0);
        }
        let n = &mut self.durations[i];
        if *n == 0 {
            self.touched_durations.push(i as u32);
        }
        *n = n.saturating_add(1);
    }

    /// ツールの位置の順に、詰めた区間の件数を取り出して空に戻す
    fn take_durations(&mut self, slots: usize) -> Vec<Box<[u32]>> {
        self.touched_durations.sort_unstable();
        let mut out = Vec::with_capacity(slots);
        let mut rest = &self.touched_durations[..];
        for slot in 0..slots {
            let n = rest.partition_point(|&i| (i as usize) / BUCKETS == slot);
            let mut packed = Vec::with_capacity(n);
            for &i in &rest[..n] {
                let count = std::mem::take(&mut self.durations[i as usize]);
                pack(&mut packed, (i as usize % BUCKETS) as u8, count);
            }
            out.push(packed.into_boxed_slice());
            rest = &rest[n..];
        }
        self.touched_durations.clear();
        out
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

/// 共有する名前は本文を比較せず、別の割り当ての同名も内容で照合する。
trait TallyKey: Copy {
    fn same(self, other: Self) -> bool;
}

impl TallyKey for &str {
    #[inline]
    fn same(self, other: Self) -> bool { std::ptr::eq(self, other) || self == other }
}

impl TallyKey for (&str, bool) {
    #[inline]
    fn same(self, other: Self) -> bool { self.1 == other.1 && self.0.same(other.0) }
}

fn model_name(model: Option<&Arc<str>>) -> &str {
    model.map(|m| m.as_ref()).unwrap_or("モデル不明")
}

/// イベント走査中は文字列を直接照合し、集計の保存時だけ共有参照を使う。
#[derive(Clone, Copy)]
struct NameRef<'a, A> {
    text: &'a str,
    shared: A,
}

impl<A: Copy> TallyKey for NameRef<'_, A> {
    #[inline]
    fn same(self, other: Self) -> bool { self.text.same(other.text) }
}

impl<A: Copy> TallyKey for (NameRef<'_, A>, bool) {
    #[inline]
    fn same(self, other: Self) -> bool { self.1 == other.1 && self.0.same(other.0) }
}

impl<K: TallyKey, V: Default> Tally<K, V> {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            last: 0,
        }
    }

    #[inline]
    fn entry(&mut self, key: K) -> &mut V {
        if self.items.get(self.last).is_none_or(|(k, _)| !k.same(key)) {
            self.last = match self.items.iter().position(|(k, _)| k.same(key)) {
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
        hashes: &RandomState,
        names: &mut Names,
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
            meta: SessionMeta::default(),
        };
        let mut models: Tally<NameRef<'_, Option<&Arc<str>>>, UsageSum> = Tally::new();
        let mut tools: Tally<(NameRef<'_, &Arc<str>>, bool), ToolTally> = Tally::new();
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
                        p.tools += usize::from(e.kind.is_tool());
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
                    let tool = e.kind.is_tool();
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
                            .entry(NameRef { text: e.model.as_deref().unwrap_or("モデル不明"), shared: e.model.as_ref() })
                            .merge(&usage);
                        out.meta.record();
                    }
                    if tool
                        && let Some(bucket) = tools.entry((NameRef { text: &e.name, shared: &e.name }, e.kind == Kind::Agent)).add(e)
                    {
                        scratch.add_duration(tools.last, bucket);
                    }
                    if let Some((day, cell)) = range.locate(e.start, &mut cursor) {
                        let amount = Amount {
                            tokens: e.usage.total(),
                            cost,
                            turns: u64::from(user),
                            tools: u64::from(tool),
                            failures: u64::from(failed),
                            sessions: 0,
                        };
                        scratch.add(range.day_bucket[day] as usize, cell, day, &amount);
                    }
                }
            }
        }
        scratch.take(&mut out);
        // collect() の入力配列の再利用で余剰容量も残さず、保存する件数だけ確保する。
        out.models.reserve_exact(models.items.len());
        out.models.extend(models
            .items
            .into_iter()
            .map(|(m, u)| {
                let hash = hashes.hash_one(m.text);
                (m.shared.map(|m| names.get(m, hash)), hash, u)
            })
        );
        let durations = scratch.take_durations(tools.items.len());
        out.tools.reserve_exact(tools.items.len());
        out.tools.extend(tools
            .items
            .into_iter()
            .zip(durations)
            .map(|(((name, agent), tally), durations)| {
                let hash = hashes.hash_one((name.text, agent));
                CachedTool {
                    name: names.get(name.shared, hash),
                    agent,
                    hash,
                    calls: tally.calls,
                    max: tally.max,
                    durations,
                }
            })
        );
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
        let from = Span::from_date(store, order, period, today);
        let cache = match self.periods.iter().position(|p| p.period == period) {
            Some(index) => {
                let cache = &mut self.periods[index];
                if !cache.range.matches(from, today) {
                    cache.sessions.clear();
                    cache.names.0.clear();
                    cache.merged_counts = [0; 2];
                    cache.range = Range::new(Span::new(from, period, today));
                }
                cache
            }
            None => {
                self.periods.push(PeriodCache {
                    period,
                    range: Range::new(Span::new(from, period, today)),
                    sessions: HashMap::default(),
                    hashes: RandomState::new(),
                    names: Names::default(),
                    merged_counts: [0; 2],
                });
                self.periods.last_mut().unwrap()
            }
        };
        cache.sessions.retain(|&si, cached| {
            store
                .sessions
                .get(si)
                .is_some_and(|s| cached.version.as_ptr() == Arc::as_ptr(s))
        });
        cache.names.0.retain(|_, name| name.strong_count() > 0);
        let range = &cache.range;
        // clear() 後も同じ種を使うので、保持したセッションのハッシュも再利用できる。
        let hashes = &cache.hashes;
        // 先に不足分を作り、キャッシュを借用しながら結果を結合する。
        let mut prices = crate::pricing::Cache::default();
        let mut scratch = None;
        let clamp = order.len() < usize::from(cache.merged_counts[0].max(cache.merged_counts[1]));
        let mut entries = if clamp { [0_usize; 2] } else { [usize::MAX; 2] };
        // retain() 後は有効な添字と版だけが残る。件数が全体と同じなら不足分はない。
        // 小さな絞り込みの予約数を数える場合は、この走査を使う。
        if clamp || cache.sessions.len() != store.sessions.len() {
            for &si in order {
                if !keep_running() {
                    return None;
                }
                let contribution = match cache.sessions.entry(si) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        let scratch = scratch.get_or_insert_with(|| Scratch::new(range));
                        let contribution = CachedSession::build(
                            &store.sessions[si], range, &mut prices, scratch,
                            hashes, &mut cache.names, &keep_running,
                        )?;
                        self.rebuilt += 1;
                        entry.insert(contribution)
                    }
                };
                if clamp {
                    entries[0] = entries[0].saturating_add(contribution.models.len());
                    entries[1] = entries[1].saturating_add(contribution.tools.len());
                }
            }
        } else if !order.is_empty() && !keep_running() {
            return None;
        }
        let mut out = Summary::empty(&range.span, period);
        let mut models: Merged<&str, [UsageSum; 2]> = Merged::with_capacity(usize::from(cache.merged_counts[0]).min(entries[0]));
        let mut projects: Merged<&str, [Totals; 2]> = Merged::default();
        let mut tools: Merged<(&str, bool), ToolMerge> = Merged::with_capacity(usize::from(cache.merged_counts[1]).min(entries[1]));
        // ツールごとの所要時間の区間。結合したツールの順に BUCKETS ずつ並べる
        let mut hist: Vec<u32> = Vec::with_capacity(usize::from(cache.merged_counts[1]) * BUCKETS);
        let mut days = vec![false; range.day_bucket.len()];
        // 表示順の件数で確保し、期間外のセッションの分は結合の後に返す。
        out.sessions.reserve_exact(order.len());
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
            out.sessions.push(SessionRow { index: si, start: session.start, total: c.total.clone() });
            projects
                .entry(HashedKey { hash: c.meta.project_hash(&session.cwd, hashes), key: session.cwd.as_str() })
                [src]
                .merge(&c.total);
            out.recorded_sessions += usize::from(c.meta.recorded());
            for (model, hash, usage) in &c.models {
                models
                    .entry(HashedKey { hash: *hash, key: model_name(model.as_ref()) })
                    [src]
                    .merge(usage);
            }
            for tool in &c.tools {
                let sum = tools.entry(HashedKey { hash: tool.hash, key: (tool.name.as_ref(), tool.agent) });
                sum.sources[src].merge(&tool.calls);
                if tool.durations.is_empty() {
                    continue;
                }
                // 詰めた値は区間の順なので、両端から件数のある範囲が分かる。
                let (first, last) = ((tool.durations[0] >> 24) as u8, (tool.durations[tool.durations.len() - 1] >> 24) as u8);
                let slot = match sum.slot {
                    Some(slot) => {
                        sum.first = sum.first.min(first);
                        sum.last = sum.last.max(last);
                        slot
                    }
                    None => {
                        hist.resize(hist.len() + BUCKETS, 0);
                        (sum.first, sum.last) = (first, last);
                        *sum.slot.insert(hist.len() / BUCKETS - 1)
                    }
                };
                let counts: &mut [u32; BUCKETS] = (&mut hist[slot * BUCKETS..][..BUCKETS]).try_into().unwrap();
                unpack_into(counts, &tool.durations);
                sum.max = sum.max.max(tool.max);
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
        out.sessions.shrink_to_fit();
        out.sum_cells();
        cache.merged_counts = [models.rows.len().min(MERGED_RESERVE) as u16,
                               tools.rows.len().min(MERGED_RESERVE) as u16];
        out.finish(
            models.into_iter().map(|(k, v)| (k.key, v)),
            projects.into_iter().map(|(k, v)| (k.key, v)),
            tools.into_iter().map(|(k, v)| {
                let durations = v.slot.map_or_else(Durations::default, |slot| {
                    let base = slot * BUCKETS;
                    Durations::from_counts(&hist[base + usize::from(v.first)..=base + usize::from(v.last)], v.first, v.max)
                });
                (k.key, ToolSum { sources: v.sources, durations })
            }),
        );
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_cache_reuse_checks_coverage_versions_and_reused_indices() {
        use crate::model::{Lane, Source, Usage};
        use std::cell::Cell;

        let today = NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        let start = midnight(today) + 3600.0;
        let mut store = Snapshot { sessions: (0..3).map(|i| {
            let mut lane = Lane::new("main");
            lane.events.push(Event {
                start, end: start + 1.0, kind: Kind::Tool, name: format!("tool-{i}").into(),
                summary: "coverage".into(), input: String::new(), output: String::new(),
                call_id: None, text_ref: Default::default(),
                usage: Usage { input: 1, ..Default::default() },
                model: Some(format!("test-cache-model-{i}").into()), status: Default::default(),
            });
            let mut session = Session {
                id: format!("session-{i}"), source: if i == 1 { Source::Codex } else { Source::Claude },
                cwd: format!("/project/{i}"), headless: false, title: "coverage".into(),
                branch: String::new(), version: String::new(), lanes: vec![lane],
                start, end: start + 1.0, parents: Vec::new(), tree: Vec::new(),
            };
            session.finalize();
            Arc::new(session)
        }).collect() };
        fn check(cache: &mut Cache, store: &Snapshot, order: &[usize], today: NaiveDate) {
            let actual = cache.build(store, order, Period::Month, today);
            let expected = Summary::reference(store, order, Period::Month, today);
            assert_eq!(actual.total.sessions, order.len());
            assert_eq!(actual.recorded_sessions, order.len());
            assert_eq!(actual.sources.each_ref().map(|s| s.sessions), expected.sources.each_ref().map(|s| s.sessions));
            let projects = |s: &Summary| s.projects.iter().map(|(name, sources)|
                (name.clone(), sources.each_ref().map(|s| s.sessions))).collect::<Vec<_>>();
            assert_eq!(projects(&actual), projects(&expected));
            let models = |s: &Summary| s.models.iter().map(|(name, sources)|
                (name.clone(), sources.each_ref().map(|s| (s.total(), s.calls, s.unpriced, s.cost.to_bits())))).collect::<Vec<_>>();
            assert_eq!(models(&actual), models(&expected));
            let tools = |s: &Summary| s.tools.iter().map(|tool|
                (tool.name.clone(), tool.agent, tool.sources, tool.durations.clone())).collect::<Vec<_>>();
            assert_eq!(tools(&actual), tools(&expected));
        }
        let mut cache = Cache::default();
        check(&mut cache, &store, &[0, 1], today);
        assert_eq!(cache.rebuilt, 2);
        // キャッシュと選択範囲が同じ件数でも、未集計のセッションがある。
        check(&mut cache, &store, &[1, 2], today);
        assert_eq!(cache.rebuilt, 1);
        check(&mut cache, &store, &[0, 1, 2], today);
        assert_eq!(cache.rebuilt, 0);
        // 全件がそろった後の絞り込みでは、各セッションの予約数も数える。
        assert_eq!(cache.periods[0].merged_counts, [3, 3]);
        check(&mut cache, &store, &[1], today);
        assert_eq!(cache.rebuilt, 0);
        let polls = Cell::new(0);
        assert!(cache.build_while(&store, &[0, 1, 2], Period::Month, today, || {
            polls.set(polls.get() + 1);
            polls.get() < 2
        }).is_none());
        check(&mut cache, &store, &[0, 1, 2], today);
        assert_eq!(cache.rebuilt, 0);
        store.sessions.remove(0);
        check(&mut cache, &store, &[0, 1], today);
        assert_eq!(cache.rebuilt, 2);
        Arc::make_mut(&mut store.sessions[0]).cwd = "/renamed".into();
        check(&mut cache, &store, &[0, 1], today);
        assert_eq!(cache.rebuilt, 1);
        check(&mut cache, &store, &[1, 1, 0], today);
        assert_eq!(cache.rebuilt, 0);
    }

    #[test]
    fn shared_names_check_content_on_hash_collisions() {
        let a: Arc<str> = "同名".into();
        let same: Arc<str> = "同名".into();
        let different: Arc<str> = "別名".into();
        let mut names = Names::default();
        let first = names.get(&a, 7);
        assert!(Arc::ptr_eq(&first, &names.get(&same, 7)));
        let second = names.get(&different, 7);
        assert_eq!(second.as_ref(), "別名");
        assert!(!Arc::ptr_eq(&first, &second));
        drop(a);
        drop(first);
        assert!(Arc::ptr_eq(&second, &names.get(&different, 7)));
    }

    #[test]
    fn hash_collisions_keep_names_and_call_kinds_separate() {
        let first = String::from("同じ名前");
        let second = first.clone();
        let mut calls: Merged<(&str, bool), usize> = Merged::default();
        for key in [(first.as_str(), false), (second.as_str(), false),
                    (second.as_str(), true), ("別の名前", false),
                    (first.as_str(), false), (second.as_str(), true),
                    (second.as_str(), false)] {
            *calls.entry(HashedKey { hash: 7, key }) += 1;
        }
        let counts: HashMap<_, _> = calls.into_iter().map(|(key, count)| (key.key, count)).collect();
        assert_eq!(counts.len(), 3);
        assert_eq!(counts[&("同じ名前", false)], 4);
        assert_eq!(counts[&("同じ名前", true)], 2);
        assert_eq!(counts[&("別の名前", false)], 1);
    }

    #[test]
    fn compressed_project_hash_collisions_keep_full_paths_separate() {
        let mut first = SessionMeta::default();
        let second = SessionMeta::default();
        first.set_project_hash(0x0123_4567_89ab_cdef);
        second.set_project_hash(0xff23_4567_89ab_cdef);
        first.record();
        assert!(first.recorded());
        assert!(!second.recorded());
        let hashes = RandomState::new();
        assert_eq!(first.project_hash("/work/project", &hashes), second.project_hash("/work/project/sub", &hashes));
        let path = String::from("/work/project");
        let same = path.clone();
        let mut projects: Merged<&str, usize> = Merged::default();
        for key in [path.as_str(), "/work/project/sub", same.as_str(),
                    "/work/project/sub", path.as_str()] {
            *projects.entry(HashedKey { hash: first.project_hash(key, &hashes), key }) += 1;
        }
        let counts: HashMap<_, _> = projects.into_iter().map(|(key, count)| (key.key, count)).collect();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts["/work/project"], 3);
        assert_eq!(counts["/work/project/sub"], 2);
        first.set_project_hash(0);
        assert_eq!(first.project_hash("/work/zero", &hashes), 0);
        assert!(first.recorded());
    }
}
