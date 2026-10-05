//! 検索。セッションのタイトル・ディレクトリ・ID と、イベントの名前・要約を部分一致で探す
//!
//! 検索用に ASCII だけを小文字にしたバイト列をセッションごとに作っておき（初回の検索時に作り、
//! 以後は更新されたセッションだけ作り直す）、memchr の memmem で走査する。ASCII の小文字化は
//! バイト長を変えないので、一致した位置をそのまま元の文字列の位置として使える。

use memchr::memmem;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::{Arc, Weak};

use crate::model::Session;

/// 1 セッション分の検索用テキスト
struct Text {
    /// タイトル・ディレクトリ・セッション ID を改行でつないだもの
    head: Vec<u8>,
    /// レーンごとに、イベントの「名前 要約」を改行区切りで連結したものと、各イベントの開始位置
    lanes: Vec<(Vec<u8>, Vec<u32>)>,
}

fn build(s: &Session) -> Text {
    let mut head = format!("{}\n{}\n{}", s.title, s.cwd, s.id).into_bytes();
    head.make_ascii_lowercase();
    let lanes = s
        .lanes
        .iter()
        .map(|l| {
            let bytes = l
                .events
                .iter()
                .map(|e| e.name.len() + e.summary.len() + 2)
                .sum();
            let mut buf = Vec::with_capacity(bytes);
            let mut starts = Vec::with_capacity(l.events.len());
            for e in &l.events {
                starts.push(buf.len() as u32);
                buf.extend_from_slice(e.name.as_bytes());
                buf.push(b' ');
                buf.extend_from_slice(e.summary.as_bytes());
                buf.push(b'\n');
            }
            buf.make_ascii_lowercase();
            (buf, starts)
        })
        .collect();
    Text { head, lanes }
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
        let mut hits: BinaryHeap<Oldest> = BinaryHeap::new();
        let mut candidates = VecDeque::new();
        for &si in scope {
            if !current() { return None; }
            // 弱参照は旧版の本文を保持しない。同じセッションの版が変われば自動で作り直す
            let version = Arc::downgrade(&sessions[si]);
            if self.sessions[si].as_ref().is_none_or(|(old, _)| !old.ptr_eq(&version)) {
                self.sessions[si] = Some((version, build(&sessions[si])));
            }
            let text = &self.sessions[si].as_ref().unwrap().1;
            if finder.find(&text.head).is_some()
                && others.iter().all(|f| f.find(&text.head).is_some())
            {
                out.sessions.push(si);
            }
            for (li, (buf, starts)) in text.lanes.iter().enumerate() {
                if !current() { return None; }
                let events = &sessions[si].lanes[li].events;
                // 1 イベントにつき 1 件だけ拾い、見つかったら次のイベントの先頭から探し直す
                let mut from = 0;
                let mut k = 0;
                let mut scanned = 0usize;
                candidates.clear();
                while let Some(p) = finder.find(&buf[from..]).map(|p| p + from) {
                    if scanned.is_multiple_of(1024) && !current() { return None; }
                    scanned += 1;
                    // 密な一致では次のイベントへ進むだけ。疎な一致では二分探索して飛ばす
                    if starts.get(k + 1).is_some_and(|&s| s as usize <= p) {
                        k += 1;
                        if starts.get(k + 1).is_some_and(|&s| s as usize <= p) {
                            k += starts[k..].partition_point(|&s| s as usize <= p) - 1;
                        }
                    }
                    let a = starts[k] as usize;
                    let b = starts.get(k + 1).map_or(buf.len(), |&e| e as usize);
                    if others.iter().all(|f| f.find(&buf[a..b]).is_some()) {
                        out.total += 1;
                        let at = ((p - a) as u32, (p - a + first.len()) as u32);
                        if limit > 0 {
                            if candidates.len() == limit {
                                candidates.pop_front();
                            }
                            candidates.push_back(Hit {
                                session: si,
                                lane: li,
                                event: k,
                                at,
                                start: events[k].start,
                            });
                        }
                    }
                    from = b;
                }
                // 時刻順のレーンでは末尾の limit 件だけが、全体の上位に入りうる
                for hit in candidates.drain(..) {
                    if hits.len() < limit {
                        hits.push(Oldest(hit));
                    } else if let Some(mut oldest) = hits.peek_mut()
                        && newest(&hit, &oldest.0).is_gt()
                    {
                        *oldest = Oldest(hit);
                    }
                }
            }
        }
        out.events = hits.into_iter().map(|h| h.0).collect();
        out.events.sort_by(|a, b| newest(b, a));
        out.sessions.sort_by(|&a, &b| sessions[b].end.total_cmp(&sessions[a].end));
        current().then_some(out)
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
                row: None,
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
            store.replace([(
                format!("file-{si}"),
                vec![session(&format!("s{si}"), "test", &events)],
            )]);
        }
        let mut ix = Index::default();
        let scope = [4, 1, 6, 0, 3, 5, 2];
        for query in ["cargo", "cargo test", "build", "bash test", "missing"] {
            let all = ix.search(&store.sessions, &scope, query, usize::MAX);
            for limit in [0, 1, 3, 500, 2000] {
                let limited = ix.search(&store.sessions, &scope, query, limit);
                assert_eq!(limited.total, all.total);
                assert_eq!(limited.events, all.events[..all.events.len().min(limit)]);
                assert_eq!(limited.sessions, all.sessions);
            }
        }
    }
}
