//! 検索。セッションのタイトル・ディレクトリ・ID と、イベントの名前・要約を部分一致で探す
//!
//! 検索用に ASCII だけを小文字にしたバイト列をセッションごとに作っておき（初回の検索時に作り、
//! 以後は更新されたセッションだけ作り直す）、memchr の memmem で走査する。ASCII の小文字化は
//! バイト長を変えないので、一致した位置をそのまま元の文字列の位置として使える。

use memchr::memmem;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Weak};

use crate::model::{Event, Session};

/// 1 セッション分の検索用テキスト
struct Text {
    /// タイトル・ディレクトリ・セッション ID を改行でつないだもの
    head: Vec<u8>,
    /// レーンごとの小文字化した本文と、元のイベントへの位置索引
    lanes: Vec<LaneText>,
}

enum LaneText {
    Flat(Vec<u8>, Vec<u32>),
    /// 名前を共有し、要約をイベント順に連結する。
    Grouped(Box<GroupText>),
}

struct NameText {
    name: (u32, u32),
    events: (u32, u32),
}

struct GroupText {
    summaries: Vec<u8>,
    starts: Vec<u32>,
    names: Vec<u8>,
    groups: Vec<NameText>,
    name_bytes: Vec<u8>,
    events: Vec<u32>,
}

impl GroupText {
    fn name(&self, group: &NameText) -> &[u8] {
        &self.name_bytes[group.name.0 as usize..group.name.1 as usize]
    }

    fn events(&self, group: &NameText) -> &[u32] {
        &self.events[group.events.0 as usize..group.events.1 as usize]
    }
}

#[inline(never)]
fn build(s: &Session, current: &impl Fn() -> bool) -> Option<Text> {
    let mut head = format!("{}\n{}\n{}", s.title, s.cwd, s.id).into_bytes();
    head.make_ascii_lowercase();
    let mut lanes = Vec::with_capacity(s.lanes.len());
    for lane in &s.lanes {
        let Some(first) = lane.events.first() else {
            lanes.push(LaneText::Flat(Vec::new(), Vec::new()));
            continue;
        };
        let mut bytes = 0;
        let mut names = [(&first.name, 0usize, 0usize); 16];
        let mut name_count = 0;
        let mut grouped = lane.events.len() >= 64;
        let mut event_names = Vec::new();
        let mut run = 0;
        let mut last = 0;
        for (chunk_index, chunk) in lane.events.chunks(256).enumerate() {
            if !current() { return None; }
            for (i, e) in chunk.iter().enumerate() {
                bytes += e.name.len() + e.summary.len() + 2;
                if grouped {
                    let name = &e.name;
                    last = if last < name_count && (Arc::ptr_eq(names[last].0, name) || names[last].0 == name) {
                        last
                    } else {
                        let event = chunk_index * 256 + i;
                        // 同じ名前が続く間は番号を書かず、名前が変わった時に区間を埋める。
                        if last != 0 { event_names[run..event].fill(last as u8); }
                        run = event;
                        let next = if let Some(i) = names[..name_count].iter().position(|n| Arc::ptr_eq(n.0, name) || n.0 == name) {
                            i
                        } else if name_count < 16 {
                            names[name_count] = (name, 0, 0);
                            name_count += 1;
                            name_count - 1
                        } else {
                            grouped = false;
                            continue;
                        };
                        if next != 0 && event_names.is_empty() { event_names = vec![0; lane.events.len()]; }
                        next
                    };
                    names[last].1 += 1;
                    names[last].2 += e.summary.len() + 1;
                }
            }
        }
        if grouped && last != 0 { event_names[run..].fill(last as u8); }
        let names = &names[..name_count];
        // 名前を取り除く節約が、追加のイベント番号とグループの管理費を上回る場合だけ使う。
        grouped = grouped && names.iter().map(|n| n.0.len() + n.2 + n.1 * 9 + std::mem::size_of::<NameText>()).sum::<usize>()
            + std::mem::size_of::<GroupText>()
            < bytes + lane.events.len() * 4;
        if grouped {
            let mut name_bytes = Vec::with_capacity(names.iter().map(|n| n.0.len()).sum());
            let mut groups = Vec::with_capacity(names.len());
            let mut cursors = [0; 16];
            let mut offset = 0;
            for (i, &(name, count, _)) in names.iter().enumerate() {
                let a = name_bytes.len();
                name_bytes.extend_from_slice(name.as_bytes());
                cursors[i] = offset;
                groups.push(NameText { name: (a as u32, name_bytes.len() as u32), events: (offset as u32, (offset + count) as u32) });
                offset += count;
            }
            name_bytes.make_ascii_lowercase();
            let mut grouped_events = vec![0; lane.events.len()];
            let mut summaries = Vec::with_capacity(names.iter().map(|n| n.2).sum());
            let mut starts = Vec::with_capacity(lane.events.len());
            let reuse_names = !event_names.is_empty();
            if !reuse_names { event_names.reserve_exact(lane.events.len()); }
            let mut last = 0;
            for (chunk_index, chunk) in lane.events.chunks(256).enumerate() {
                if !current() { return None; }
                let from = summaries.len();
                for (i, e) in chunk.iter().enumerate() {
                    if reuse_names {
                        last = event_names[chunk_index * 256 + i] as usize;
                    } else {
                        if !Arc::ptr_eq(names[last].0, &e.name) && names[last].0 != &e.name {
                            last = names.iter().position(|n| Arc::ptr_eq(n.0, &e.name) || n.0 == &e.name).unwrap();
                        }
                        event_names.push(last as u8);
                    }
                    starts.push(summaries.len() as u32);
                    grouped_events[cursors[last]] = (chunk_index * 256 + i) as u32;
                    cursors[last] += 1;
                    summaries.extend_from_slice(e.summary.as_bytes());
                    summaries.push(b'\n');
                }
                summaries[from..].make_ascii_lowercase();
            }
            lanes.push(LaneText::Grouped(Box::new(GroupText { summaries, starts, names: event_names, groups, name_bytes, events: grouped_events })));
            continue;
        }
        drop(event_names);
        let mut buf = Vec::with_capacity(bytes);
        let mut starts = Vec::with_capacity(lane.events.len());
        for chunk in lane.events.chunks(256) {
            if !current() { return None; }
            let from = buf.len();
            for e in chunk {
                starts.push(buf.len() as u32);
                buf.extend_from_slice(e.name.as_bytes());
                buf.push(b' ');
                buf.extend_from_slice(e.summary.as_bytes());
                buf.push(b'\n');
            }
            buf[from..].make_ascii_lowercase();
        }
        lanes.push(LaneText::Flat(buf, starts));
    }
    Some(Text { head, lanes })
}

#[derive(Default)]
pub struct Index {
    sessions: Vec<Option<(Weak<Session>, Text)>>,
}

/// イベントの一致
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hit {
    pub session: usize,
    pub lane: usize,
    pub event: usize,
    /// 最初の語が一致した、「名前 要約」の中のバイト範囲
    pub at: (u32, u32),
    /// イベントの開始時刻（並べ替え用）
    start: f64,
}

fn newest(a: &Hit, b: &Hit) -> Ordering {
    a.start
        .total_cmp(&b.start)
        .then((a.session, a.lane, a.event).cmp(&(b.session, b.lane, b.event)))
}

/// 上限を超えたら最も古い一致を取り除くため、古い順の最大ヒープにする
#[derive(Clone, Copy)]
struct Oldest(Hit);

impl PartialEq for Oldest {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Oldest {}
impl PartialOrd for Oldest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Oldest {
    fn cmp(&self, other: &Self) -> Ordering {
        newest(&other.0, &self.0)
    }
}

/// 上限内は配列へ集め、上限を超えて候補を選別するときだけヒープにする。
enum Hits {
    Collecting(Vec<Oldest>),
    Bounded(BinaryHeap<Oldest>),
}

/// 時刻順の列の末尾だけを残し、イベント本体は全体の候補へ併合するときに読む。
struct Candidates {
    entries: Vec<(usize, (u32, u32))>,
    next: usize,
    limit: usize,
}

impl Candidates {
    fn clear(&mut self) { self.entries.clear(); self.next = 0; }

    fn push(&mut self, event: usize, at: (u32, u32)) {
        if self.limit == 0 { return; }
        if self.entries.len() == self.limit {
            self.entries[self.next] = (event, at);
            self.next += 1;
            if self.next == self.limit { self.next = 0; }
        } else { self.entries.push((event, at)); }
    }

    fn merge(&self, hits: &mut Hits, session: usize, lane: usize, events: &[Event]) {
        if self.entries.is_empty() { return; }
        if let Hits::Collecting(entries) = hits {
            if self.entries.len() <= self.limit - entries.len() {
                let needed = entries.len() + self.entries.len();
                if needed > entries.capacity() {
                    let capacity = needed.max(entries.capacity().saturating_mul(2)).max(4).min(self.limit);
                    entries.reserve_exact(capacity - entries.len());
                }
                entries.extend(self.entries[self.next..].iter().chain(&self.entries[..self.next]).map(|&(event, at)| {
                    Oldest(Hit { session, lane, event, at, start: events[event].start })
                }));
                return;
            }
            *hits = Hits::Bounded(BinaryHeap::from(std::mem::take(entries)));
        }
        let Hits::Bounded(hits) = hits else { unreachable!() };
        let needed = hits.len() + self.entries.len().min(self.limit - hits.len());
        if needed > hits.capacity() {
            let capacity = needed.max(hits.capacity().saturating_mul(2)).max(4).min(self.limit);
            hits.reserve_exact(capacity - hits.len());
        }
        for &(event, at) in self.entries[self.next..].iter().chain(&self.entries[..self.next]) {
            let hit = Hit { session, lane, event, at, start: events[event].start };
            if hits.len() < self.limit {
                hits.push(Oldest(hit));
            } else if let Some(mut oldest) = hits.peek_mut()
                && newest(&hit, &oldest.0).is_gt()
            { *oldest = Oldest(hit); }
        }
    }
}

/// 名前を含む従来の本文、または名前が一種類のレーンの要約を走査する。
fn scan<'a, 'n: 'a>(
    buf: &[u8], starts: &[u32], prefix: usize, finder: &memmem::Finder<'n>,
    others: impl Iterator<Item = &'a memmem::Finder<'n>> + Clone,
    candidates: &mut Candidates, current: &impl Fn() -> bool,
) -> Option<usize> {
    candidates.clear();
    let mut from = 0;
    let mut k = 0;
    let mut scanned = 0usize;
    let mut total = 0;
    while let Some(p) = finder.find(&buf[from..]).map(|p| p + from) {
        if scanned.is_multiple_of(1024) && !current() { return None; }
        scanned += 1;
        if starts.get(k + 1).is_some_and(|&s| s as usize <= p) {
            k += 1;
            if starts.get(k + 1).is_some_and(|&s| s as usize <= p) {
                k += starts[k..].partition_point(|&s| s as usize <= p) - 1;
            }
        }
        let a = starts[k] as usize;
        let b = starts.get(k + 1).map_or(buf.len(), |&e| e as usize);
        if others.clone().all(|f| f.find(&buf[a..b]).is_some()) {
            total += 1;
            let at = p - a + prefix;
            candidates.push(k, (at as u32, (at + finder.needle().len()) as u32));
        }
        from = b;
        k += 1;
    }
    Some(total)
}

#[derive(Default)]
pub struct Results {
    /// タイトルなどが一致したセッション（新しい順）
    pub sessions: Vec<usize>,
    /// 名前や要約が一致したイベント（新しい順、上限まで）
    pub events: Vec<Hit>,
    /// 上限で切る前のイベントの一致数
    pub total: usize,
}

impl Results {
    /// 結果の項目数（セッション + イベント）
    pub fn len(&self) -> usize {
        self.sessions.len() + self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty() && self.events.is_empty()
    }
}

impl Index {
    /// `scope` のセッションから探す。空白で区切った語はすべてを含むものだけを返す（大文字・小文字は区別しない）
    pub fn search(&mut self, sessions: &[Arc<Session>], scope: &[usize], query: &str, limit: usize) -> Results {
        self.search_while(sessions, scope, query, limit, || true).unwrap()
    }

    /// 別の検索・表示範囲・データ版になった仕事を途中で打ち切る
    pub fn search_while(&mut self, sessions: &[Arc<Session>], scope: &[usize], query: &str, limit: usize, current: impl Fn() -> bool) -> Option<Results> {
        if !current() { return None; }
        let terms: Vec<Vec<u8>> = query.split_whitespace().map(|t| t.to_ascii_lowercase().into_bytes()).collect();
        let Some((first, rest)) = terms.split_first() else {
            return Some(Results::default());
        };
        if self.sessions.len() < sessions.len() {
            self.sessions.resize_with(sessions.len(), || None);
        }
        let finder = memmem::Finder::new(first);
        let others: Vec<memmem::Finder> = rest.iter().map(memmem::Finder::new).collect();
        let mut out = Results::default();
        let mut hits = Hits::Collecting(Vec::new());
        let mut candidates = Candidates { entries: Vec::new(), next: 0, limit };
        let mut summary_terms: [Vec<_>; 16] = std::array::from_fn(|_| Vec::new());
        for &si in scope {
            if !current() { return None; }
            // 弱参照は旧版の本文を保持しない。同じセッションの版が変われば自動で作り直す
            if self.sessions[si].as_ref().is_none_or(|(old, _)| old.as_ptr() != Arc::as_ptr(&sessions[si])) {
                // 中断した索引をキャッシュへ公開せず、次の要求では完全な版を作り直す。
                self.sessions[si] = Some((Arc::downgrade(&sessions[si]), build(&sessions[si], &current)?));
            }
            let text = &self.sessions[si].as_ref().unwrap().1;
            if finder.find(&text.head).is_some()
                && others.iter().all(|f| f.find(&text.head).is_some())
            {
                out.sessions.push(si);
            }
            for (li, lane) in text.lanes.iter().enumerate() {
                if !current() { return None; }
                let events = &sessions[si].lanes[li].events;
                if let LaneText::Grouped(grouped) = lane {
                    let buf = &grouped.summaries;
                    let starts = &grouped.starts;
                    if grouped.groups.len() == 1 && finder.find(grouped.name(&grouped.groups[0])).is_none() {
                        let group = &grouped.groups[0];
                        let name = grouped.name(group);
                        summary_terms[0].clear();
                        summary_terms[0].extend(others.iter().filter(|f| f.find(name).is_none()));
                        out.total += scan(buf, starts, name.len() + 1, &finder, summary_terms[0].iter().copied(), &mut candidates, &current)?;
                        candidates.merge(&mut hits, si, li, events);
                        continue;
                    }
                    let mut scanned = 0usize;
                    let mut named = [None; 16];
                    for (ni, group) in grouped.groups.iter().enumerate() {
                        candidates.clear();
                        let name = grouped.name(group);
                        let group_events = grouped.events(group);
                        let missing = &mut summary_terms[ni];
                        missing.clear();
                        missing.extend(others.iter().filter(|f| f.find(name).is_none()));
                        named[ni] = finder.find(name);
                        let Some(p) = named[ni] else { continue; };
                        let at = (p as u32, (p + first.len()) as u32);
                        if missing.is_empty() {
                            // 名前だけですべての語が一致する場合は、件数と末尾だけを拾う。
                            let count = group_events.len();
                            let end = scanned + count;
                            while scanned < end {
                                if scanned.is_multiple_of(1024) && !current() { return None; }
                                scanned += (1024 - scanned % 1024).min(end - scanned);
                            }
                            out.total += count;
                            candidates.entries.extend(group_events[count.saturating_sub(limit)..].iter().map(|&event| (event as usize, at)));
                        } else {
                            for &event in group_events {
                                if scanned.is_multiple_of(1024) && !current() { return None; }
                                scanned += 1;
                                let event = event as usize;
                                let a = starts[event] as usize;
                                let b = starts.get(event + 1).map_or(buf.len(), |&e| e as usize);
                                if missing.iter().all(|f| f.find(&buf[a..b]).is_some()) {
                                    out.total += 1;
                                    candidates.push(event, at);
                                }
                            }
                        }
                        candidates.merge(&mut hits, si, li, events);
                    }
                    // 要約は元のイベント順の一本の列を探す。名前で拾ったイベントは二重に数えない。
                    candidates.clear();
                    let mut from = 0;
                    let mut k = 0;
                    if named[..grouped.groups.len()].iter().any(Option::is_none) {
                        while let Some(p) = finder.find(&buf[from..]).map(|p| p + from) {
                            if starts.get(k + 1).is_some_and(|&s| s as usize <= p) {
                                k += 1;
                                if starts.get(k + 1).is_some_and(|&s| s as usize <= p) {
                                    k += starts[k..].partition_point(|&s| s as usize <= p) - 1;
                                }
                            }
                            let a = starts[k] as usize;
                            let b = starts.get(k + 1).map_or(buf.len(), |&e| e as usize);
                            let ni = grouped.names[k] as usize;
                            if named[ni].is_none() {
                                if scanned.is_multiple_of(1024) && !current() { return None; }
                                scanned += 1;
                                if summary_terms[ni].iter().all(|f| f.find(&buf[a..b]).is_some()) {
                                    out.total += 1;
                                    let p = p - a + grouped.name(&grouped.groups[ni]).len() + 1;
                                    let at = (p as u32, (p + first.len()) as u32);
                                    candidates.push(k, at);
                                }
                            }
                            from = b;
                            k += 1;
                        }
                    }
                    candidates.merge(&mut hits, si, li, events);
                    continue;
                }
                let LaneText::Flat(buf, starts) = lane else { unreachable!() };
                out.total += scan(buf, starts, 0, &finder, others.iter(), &mut candidates, &current)?;
                candidates.merge(&mut hits, si, li, events);
            }
        }
        out.events = match hits {
            Hits::Collecting(hits) => hits.into_iter().map(|h| h.0).collect(),
            Hits::Bounded(hits) => hits.into_iter().map(|h| h.0).collect(),
        };
        out.events.sort_by(|a, b| newest(b, a));
        out.sessions.sort_by(|&a, &b| sessions[b].end.total_cmp(&sessions[a].end));
        current().then_some(out)
    }
}

impl Index {
    /// 指定された旧版の索引だけを解放する。遅れて届いた更新通知で新しい索引を消さない。
    pub fn invalidate_versions<'a>(&mut self, versions: impl IntoIterator<Item = (usize, &'a Arc<Session>)>) {
        for (si, version) in versions {
            if let Some(entry) = self.sessions.get_mut(si)
                && entry.as_ref().is_some_and(|(old, _)| old.as_ptr() == Arc::as_ptr(version))
            {
                *entry = None;
            }
        }
    }
}

/// `hay` の中で `term`（小文字化済み）が最初に一致する範囲。ASCII の大文字・小文字は区別しない
pub fn find(hay: &str, term: &str) -> Option<std::ops::Range<usize>> {
    let lower = hay.to_ascii_lowercase();
    let i = memmem::find(lower.as_bytes(), term.as_bytes())?;
    // 小文字化してもバイト位置は変わらないが、文字の途中で切れていないかは確かめる
    (hay.is_char_boundary(i) && hay.is_char_boundary(i + term.len())).then(|| i..i + term.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Kind, Lane, Loaded, Source, Store};

    fn session(id: &str, title: &str, events: &[(&str, &str, f64)]) -> Loaded {
        let mut lane = Lane::new("main");
        lane.events = events
            .iter()
            .map(|&(name, summary, t)| Event {
                start: t,
                end: t,
                kind: Kind::Tool,
                name: name.into(),
                summary: summary.into(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: Default::default(),
                usage: Default::default(),
                model: None,
                status: Default::default(),
            })
            .collect();
        Loaded::Session(Session {
            id: id.into(),
            source: Source::Claude,
            cwd: "/w/rireki".into(),
            headless: false,
            title: title.into(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        })
    }

    #[test]
    fn terms_and_order() {
        let mut store = Store::default();
        store.replace([
            (
                "a".to_string(),
                vec![session("s1", "Fix Parser", &[("Bash", "cargo test", 1.0), ("Read", "src/a.rs", 2.0)])],
            ),
            ("b".to_string(), vec![session("s2", "日本語の作業", &[("Bash", "Cargo build --release", 3.0)])]),
        ]);
        let mut ix = Index::default();
        let r = ix.search(&store.sessions, &[0, 1], "CARGO", 10);
        assert_eq!(r.events.iter().map(|h| h.session).collect::<Vec<_>>(), [1, 0]);
        // 「名前 要約」の中の位置
        assert_eq!(r.events[0].at, (5, 10));
        assert_eq!(ix.search(&store.sessions, &[0, 1], "bash test", 10).events.len(), 1);
        assert_eq!(ix.search(&store.sessions, &[0, 1], "parser", 10).sessions, [0]);
        assert_eq!(ix.search(&store.sessions, &[0, 1], "作業", 10).sessions, [1]);
        assert_eq!(ix.search(&store.sessions, &[1], "test", 10).total, 0);
        // 更新されたセッションは作り直す
        store.replace([("a".to_string(), vec![session("s1", "Fix Parser", &[("Bash", "make", 1.0)])])]);
        assert_eq!(ix.search(&store.sessions, &[0, 1], "cargo", 10).total, 1);
    }

    #[test]
    fn find_range() {
        assert_eq!(find("Run Cargo Test", "cargo"), Some(4..9));
        assert_eq!(find("日本語 abc", "abc"), Some(10..13));
        assert_eq!(find("abc", "x"), None);
    }

    #[test]
    fn snapshots_and_cached_text_keep_their_own_data_version() {
        let mut store = Store::default();
        store.replace([("a".into(), vec![session("s", "old", &[("Bash", "cargo test", 1.0)])])]);
        let old = store.snapshot();
        let mut ix = Index::default();
        let strong = Arc::strong_count(&old.sessions[0]);
        assert_eq!(ix.search(&old.sessions, &[0], "cargo", 10).total, 1);
        assert_eq!(Arc::strong_count(&old.sessions[0]), strong, "検索のキャッシュは本文を保持しない");
        store.replace([("a".into(), vec![session("s", "new", &[("Bash", "make", 2.0)])])]);
        assert!(!Arc::ptr_eq(&old.sessions[0], &store.sessions[0]));
        assert_eq!(ix.search(&store.sessions, &[0], "cargo", 10).total, 0);
        assert_eq!(ix.search(&store.sessions, &[0], "make", 10).total, 1);
        assert_eq!(ix.search(&old.sessions, &[0], "cargo", 10).total, 1);
        assert_eq!(old.sessions[0].title, "old");
        assert_eq!(store.sessions[0].title, "new");
    }

    #[test]
    fn canceled_search_stops_inside_a_dense_lane_and_can_be_reused() {
        let mut store = Store::default();
        let events: Vec<_> = (0..10_000).map(|i| ("Bash", "cargo test", i as f64)).collect();
        store.replace([("a".into(), vec![session("s", "test", &events)])]);
        let mut ix = Index::default();
        assert!(ix.search_while(&store.sessions, &[0], "cargo", 10, || false).is_none());
        assert!(ix.sessions.is_empty());
        assert_eq!(ix.search(&store.sessions, &[0], "cargo", 10).total, 10_000);
        let checks = std::cell::Cell::new(0);
        assert!(ix.search_while(&store.sessions, &[0], "cargo", 10, || {
            checks.set(checks.get() + 1);
            checks.get() < 6
        }).is_none());
        assert_eq!(checks.get(), 6);
        let r = ix.search(&store.sessions, &[0], "cargo", 10);
        assert_eq!(r.total, 10_000);
        assert_eq!(r.events.iter().map(|h| h.event).collect::<Vec<_>>(), (9990..10000).rev().collect::<Vec<_>>());
    }

    #[test]
    fn late_name_overflow_and_short_names_keep_complete_text() {
        let events: Vec<_> = (0..513).map(|i| ("exec_command", "cargo test", i as f64)).collect();
        let Loaded::Session(mut item) = session("s", "", &events) else { unreachable!() };
        for (i, event) in item.lanes[0].events[256..272].iter_mut().enumerate() {
            event.name = format!("other-name-{i}").into();
        }
        let mut store = Store::default();
        store.replace([("file".into(), vec![Loaded::Session(item)])]);
        let mut ix = Index::default();
        let cargo = ix.search(&store.sessions, &[0], "cargo", 1000);
        assert_eq!(cargo.total, 513);
        assert_eq!(cargo.events.iter().map(|h| h.event).collect::<Vec<_>>(), (0..513).rev().collect::<Vec<_>>());
        assert!(cargo.events.iter().all(|h| h.at.0 == store.sessions[0].lanes[0].events[h.event].name.len() as u32 + 1));
        let named = ix.search(&store.sessions, &[0], "other-name-15", 1000);
        assert_eq!(named.total, 1);
        assert_eq!(named.events[0].event, 271);
        assert!(matches!(ix.sessions[0].as_ref().unwrap().1.lanes[0], LaneText::Flat(_, _)));
        // 最初の256件は同じ名前で、その後に別の名前が現れても位置を保つ。
        let Loaded::Session(mut two) = session("s", "", &events) else { unreachable!() };
        two.lanes[0].events[256].name = "Bash".into();
        store.replace([("file".into(), vec![Loaded::Session(two)])]);
        let cargo = ix.search(&store.sessions, &[0], "cargo", 1000);
        assert_eq!(cargo.total, 513);
        assert_eq!(cargo.events.iter().map(|h| h.event).collect::<Vec<_>>(), (0..513).rev().collect::<Vec<_>>());
        assert!(cargo.events.iter().all(|h| h.at == if h.event == 256 { (5, 10) } else { (13, 18) }));
        let named = ix.search(&store.sessions, &[0], "bash cargo", 1000);
        assert_eq!(named.total, 1);
        assert_eq!(named.events[0].event, 256);
        assert_eq!(named.events[0].at, (0, 4));
        assert!(matches!(ix.sessions[0].as_ref().unwrap().1.lanes[0], LaneText::Grouped(_)));
        let short: Vec<_> = (0..513).map(|i| ("X", "cargo test", i as f64)).collect();
        store.replace([("file".into(), vec![session("s", "", &short)])]);
        let cargo = ix.search(&store.sessions, &[0], "cargo", 1000);
        assert_eq!(cargo.total, 513);
        assert!(cargo.events.iter().all(|h| h.at == (2, 7)));
        assert!(matches!(ix.sessions[0].as_ref().unwrap().1.lanes[0], LaneText::Flat(_, _)));
        let empty: Vec<_> = (0..64).map(|i| ("", "cargo test", i as f64)).collect();
        store.replace([("file".into(), vec![session("s", "", &empty)])]);
        let cargo = ix.search(&store.sessions, &[0], "cargo", 1000);
        assert_eq!(cargo.total, 64);
        assert!(cargo.events.iter().all(|h| h.at == (1, 6)));
        assert!(matches!(ix.sessions[0].as_ref().unwrap().1.lanes[0], LaneText::Flat(_, _)));
    }
    #[test]
    fn canceled_text_build_does_not_publish_partial_or_wrong_version() {
        let mut store = Store::default();
        store.replace([("a".into(), vec![session("s", "old", &[("Bash", "cargo", 0.0)])])]);
        let mut ix = Index::default();
        assert_eq!(ix.search(&store.sessions, &[0], "cargo", 10).total, 1);
        let old = ix.sessions[0].as_ref().unwrap().0.clone();
        let events: Vec<_> = (0..1025).map(|i| ("ツール", "NEW Cargo 日本語", i as f64)).collect();
        store.replace([("a".into(), vec![session("s", "new", &events)])]);
        // 長さの計算中と、連結・小文字化が一部まで進んだ段階の両方で中断する。
        for stop in [3, 9] {
            let checks = std::cell::Cell::new(0);
            assert!(ix.search_while(&store.sessions, &[0], "new 日本語", 10, || {
                checks.set(checks.get() + 1);
                checks.get() < stop
            }).is_none());
            assert_eq!(checks.get(), stop);
            assert!(ix.sessions[0].as_ref().unwrap().0.ptr_eq(&old));
        }
        assert_eq!(ix.search(&store.sessions, &[0], "cargo", 10).total, 1025);
        assert_eq!(ix.search(&store.sessions, &[0], "new 日本語", 10).events[0].at, (10, 13));
        assert!(!ix.sessions[0].as_ref().unwrap().0.ptr_eq(&old));
    }

    #[test]
    fn retired_versions_release_hidden_text_without_evicting_newer_or_untouched_text() {
        let mut store = Store::default();
        let events: Vec<_> = (0..1025).map(|i| ("exec_command", "cargo test 日本語", i as f64)).collect();
        store.replace([
            ("large".into(), vec![session("large", "", &events)]),
            ("small".into(), vec![session("small", "", &[("Bash", "small", 1.0)])]),
        ]);
        let mut ix = Index::default();
        assert_eq!(ix.search(&store.sessions, &[0, 1], "cargo", 10).total, 1025);
        let old = store.snapshot();
        let unchanged = Arc::downgrade(&store.sessions[1]);
        let touched = store.replace([("large".into(), Vec::new())]);
        assert_eq!(touched, [0]);
        assert!(!store.visible(0));
        assert_eq!(ix.search(&store.sessions, &[1], "small", 10).total, 1);
        // 旧版が別の読み取りで使われていても、検索対象外の古い索引は解放する。
        assert!(ix.sessions[0].is_some());
        ix.invalidate_versions([0, 0, 2, usize::MAX].into_iter().filter_map(|si| old.sessions.get(si).map(|s| (si, s))));
        assert!(ix.sessions[0].is_none());
        assert!(ix.sessions[1].as_ref().unwrap().0.ptr_eq(&unchanged));
        assert_eq!(old.sessions[0].lanes[0].events.len(), 1025);
        assert_eq!(ix.search(&store.sessions, &[1], "small", 10).total, 1);

        store.replace([("large".into(), vec![session("large", "", &events)])]);
        let intermediate = store.snapshot();
        assert_eq!(ix.search(&store.sessions, &[0], "cargo", 10).total, 1025);
        store.replace([("large".into(), vec![session("large", "", &[("Bash", "new cargo 日本語", 2000.0)])])]);
        assert_eq!(ix.search(&store.sessions, &[0], "new cargo", 10).total, 1);
        let newest = Arc::downgrade(&store.sessions[0]);
        // 更新通知より先に検索が終わった場合と、通知が逆順に届いた場合を再現する。
        ix.invalidate_versions([(0, &intermediate.sessions[0])]);
        ix.invalidate_versions([(0, &old.sessions[0])]);
        assert!(ix.sessions[0].as_ref().unwrap().0.ptr_eq(&newest));
        assert_eq!(ix.search(&store.sessions, &[0], "new cargo", 10).events[0].at, (5, 8));
        let current = store.snapshot();
        ix.invalidate_versions([(0, &current.sessions[0])]);
        assert!(ix.sessions[0].is_none());
        assert_eq!(ix.search(&store.sessions, &[0], "new cargo", 10).total, 1);
    }

    #[test]
    fn limited_results_match_full_search_across_lanes_and_ties() {
        let mut store = Store::default();
        for si in 0..7 {
            let events: Vec<_> = (0..250)
                .map(|i| {
                    (
                        "Bash",
                        if i % 9 == 0 {
                            "cargo test"
                        } else {
                            "cargo build"
                        },
                        (i / 3 + si * 5) as f64,
                    )
                })
                .collect();
            let Loaded::Session(mut item) = session(&format!("s{si}"), "test", &events) else {
                unreachable!();
            };
            for li in 1..3 {
                let mut lane = Lane::new(format!("lane-{li}"));
                lane.events = item.lanes[0].events.iter().enumerate().map(|(i, e)| {
                    let mut event = e.clone();
                    event.start = (i / 3 + si * 5 + li * 2) as f64;
                    event.end = event.start;
                    event.summary = if i % 7 == 0 { "日本語 Cargo test" } else { "日本語 Cargo build" }.into();
                    event
                }).collect();
                item.lanes.push(lane);
            }
            store.replace([(
                format!("file-{si}"),
                vec![Loaded::Session(item)],
            )]);
        }
        let mut ix = Index::default();
        let scope = [4, 1, 6, 0, 3, 5, 2, 1];
        for query in ["cargo", "cargo test", "build", "bash test", "日本語 cargo", "missing"] {
            // 索引・候補の保持と独立した、全イベントの文字列から作る参照結果。
            let terms: Vec<_> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
            let mut reference = Vec::new();
            for &si in &scope {
                for (li, lane) in store.sessions[si].lanes.iter().enumerate() {
                    for (ei, event) in lane.events.iter().enumerate() {
                        let text = format!("{} {}\n", event.name, event.summary).to_ascii_lowercase();
                        if terms.iter().all(|t| text.contains(t)) {
                            let at = text.find(&terms[0]).unwrap() as u32;
                            reference.push(Hit { session: si, lane: li, event: ei, at: (at, at + terms[0].len() as u32), start: event.start });
                        }
                    }
                }
            }
            reference.sort_by(|a, b| newest(b, a));
            let all = ix.search(&store.sessions, &scope, query, usize::MAX);
            assert_eq!(all.total, reference.len());
            assert_eq!(all.events, reference);
            for limit in [0, 1, 3, 31, 500, 2000, all.events.len().saturating_sub(1), all.events.len(), all.events.len() + 1, usize::MAX] {
                let limited = ix.search(&store.sessions, &scope, query, limit);
                assert_eq!(limited.total, all.total);
                assert_eq!(limited.events, all.events[..all.events.len().min(limit)]);
                assert_eq!(limited.sessions, all.sessions);
            }
        }
    }

    #[test]
    fn grouped_names_keep_original_positions_and_full_search_results() {
        let mut store = Store::default();
        let names = ["CARGO test 日本語", "exec_command", "éXÉC", "", "test CARGO", "Bash"];
        let events: Vec<_> = (0..3000).map(|i| (
            names[i % names.len()],
            if i % 7 == 0 { "cargo test 日本語 exec command" } else { "BUILD 日本語" },
            (i / 4) as f64,
        )).collect();
        store.replace([("file".into(), vec![session("grouped", "cargo 日本語", &events)])]);
        let mut ix = Index::default();
        for query in ["cargo", "test cargo", "exec command", "日本語", "éxÉC", "BUILD", "command build", "missing"] {
            let terms: Vec<_> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
            let mut reference = Vec::new();
            for (ei, event) in store.sessions[0].lanes[0].events.iter().enumerate() {
                let text = format!("{} {}\n", event.name, event.summary).to_ascii_lowercase();
                if terms.iter().all(|t| text.contains(t)) {
                    let at = text.find(&terms[0]).unwrap() as u32;
                    reference.push(Hit { session: 0, lane: 0, event: ei, at: (at, at + terms[0].len() as u32), start: event.start });
                }
            }
            reference.sort_by(|a, b| newest(b, a));
            for limit in [0, 1, 31, 500, usize::MAX] {
                let result = ix.search(&store.sessions, &[0], query, limit);
                assert_eq!(result.total, reference.len(), "{query}");
                assert_eq!(result.events, reference[..reference.len().min(limit)], "{query}, limit {limit}");
            }
        }
        assert!(matches!(ix.sessions[0].as_ref().unwrap().1.lanes[0], LaneText::Grouped(_)));
        // 名前の種類が多いレーンでは、従来の連結した索引を使う。
        let Loaded::Session(mut unique) = session("grouped", "", &events) else { unreachable!() };
        for (i, event) in unique.lanes[0].events.iter_mut().enumerate() { event.name = format!("name-{i}").into(); }
        store.replace([("file".into(), vec![Loaded::Session(unique)])]);
        let result = ix.search(&store.sessions, &[0], "name-2999", 500);
        assert_eq!(result.total, 1);
        assert_eq!(result.events[0].event, 2999);
        assert_eq!(result.events[0].at, (0, 9));
        assert!(matches!(ix.sessions[0].as_ref().unwrap().1.lanes[0], LaneText::Flat(_, _)));
    }

    #[test]
    fn grouped_search_checks_cancellation_for_bulk_names_and_rejected_terms() {
        let mut store = Store::default();
        let events: Vec<_> = (0..10_000).map(|i| ("exec_command", "cargo", i as f64)).collect();
        store.replace([("file".into(), vec![session("s", "", &events)])]);
        let mut ix = Index::default();
        for query in ["exec", "exec missing", "cargo"] {
            let reference = ix.search(&store.sessions, &[0], query, 500);
            assert!(matches!(ix.sessions[0].as_ref().unwrap().1.lanes[0], LaneText::Grouped(_)));
            for limit in [0, 500] {
                let checks = std::cell::Cell::new(0);
                let result = ix.search_while(&store.sessions, &[0], query, limit, || {
                    checks.set(checks.get() + 1);
                    true
                }).unwrap();
                assert_eq!(result.total, reference.total);
                assert_eq!(checks.get(), 14, "{query}, limit {limit}");
                for stop in [4, 9, 13, 14] {
                    checks.set(0);
                    assert!(ix.search_while(&store.sessions, &[0], query, limit, || {
                        checks.set(checks.get() + 1);
                        checks.get() < stop
                    }).is_none());
                    assert_eq!(checks.get(), stop);
                }
            }
            assert_eq!(ix.search(&store.sessions, &[0], query, 500).events, reference.events);
        }
    }
}
