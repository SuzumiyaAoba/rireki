//! 検索。セッションのタイトル・ディレクトリ・ID と、イベントの名前・要約を部分一致で探す
//!
//! 検索用に ASCII だけを小文字にしたバイト列をセッションごとに作っておき（初回の検索時に作り、
//! 以後は更新されたセッションだけ作り直す）、memchr の memmem で走査する。ASCII の小文字化は
//! バイト長を変えないので、一致した位置をそのまま元の文字列の位置として使える。

use memchr::memmem;

use crate::model::{Session, Store};

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
            let mut buf = Vec::new();
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
    sessions: Vec<Option<Text>>,
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
    /// 更新されたセッションの検索用テキストを捨てる（次の検索で作り直す）
    pub fn invalidate(&mut self, touched: &[usize]) {
        for &i in touched {
            if let Some(t) = self.sessions.get_mut(i) {
                *t = None;
            }
        }
    }

    /// `scope` のセッションから探す。空白で区切った語はすべてを含むものだけを返す（大文字・小文字は区別しない）
    pub fn search(&mut self, store: &Store, scope: &[usize], query: &str, limit: usize) -> Results {
        let terms: Vec<Vec<u8>> = query.split_whitespace().map(|t| t.to_ascii_lowercase().into_bytes()).collect();
        let Some((first, rest)) = terms.split_first() else {
            return Results::default();
        };
        if self.sessions.len() < store.sessions.len() {
            self.sessions.resize_with(store.sessions.len(), || None);
        }
        let finder = memmem::Finder::new(first);
        let others: Vec<memmem::Finder> = rest.iter().map(memmem::Finder::new).collect();
        let mut out = Results::default();
        let mut hits = Vec::new();
        for &si in scope {
            let text = self.sessions[si].get_or_insert_with(|| build(&store.sessions[si]));
            if terms.iter().all(|t| memmem::find(&text.head, t).is_some()) {
                out.sessions.push(si);
            }
            for (li, (buf, starts)) in text.lanes.iter().enumerate() {
                let events = &store.sessions[si].lanes[li].events;
                // 1 イベントにつき 1 件だけ拾い、見つかったら次のイベントの先頭から探し直す
                let mut from = 0;
                while let Some(p) = finder.find(&buf[from..]).map(|p| p + from) {
                    let k = starts.partition_point(|&s| s as usize <= p) - 1;
                    let a = starts[k] as usize;
                    let b = starts.get(k + 1).map_or(buf.len(), |&e| e as usize);
                    if others.iter().all(|f| f.find(&buf[a..b]).is_some()) {
                        let at = ((p - a) as u32, (p - a + first.len()) as u32);
                        hits.push(Hit { session: si, lane: li, event: k, at, start: events[k].start });
                    }
                    from = b;
                }
            }
        }
        out.total = hits.len();
        // 新しい順に上限まで。全体を並べ替えず、上位だけを選んでから並べる
        let newer = |a: &Hit, b: &Hit| b.start.total_cmp(&a.start);
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit, newer);
            hits.truncate(limit);
        }
        hits.sort_by(newer);
        out.events = hits;
        out.sessions.sort_by(|&a, &b| store.sessions[b].end.total_cmp(&store.sessions[a].end));
        out
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
    use crate::model::{Event, Kind, Lane, Loaded, Source};

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
        let r = ix.search(&store, &[0, 1], "CARGO", 10);
        assert_eq!(r.events.iter().map(|h| h.session).collect::<Vec<_>>(), [1, 0]);
        // 「名前 要約」の中の位置
        assert_eq!(r.events[0].at, (5, 10));
        assert_eq!(ix.search(&store, &[0, 1], "bash test", 10).events.len(), 1);
        assert_eq!(ix.search(&store, &[0, 1], "parser", 10).sessions, [0]);
        assert_eq!(ix.search(&store, &[0, 1], "作業", 10).sessions, [1]);
        assert_eq!(ix.search(&store, &[1], "test", 10).total, 0);
        // 更新されたセッションは作り直す
        store.replace([("a".to_string(), vec![session("s1", "Fix Parser", &[("Bash", "make", 1.0)])])]);
        ix.invalidate(&[0]);
        assert_eq!(ix.search(&store, &[0, 1], "cargo", 10).total, 1);
    }

    #[test]
    fn find_range() {
        assert_eq!(find("Run Cargo Test", "cargo"), Some(4..9));
        assert_eq!(find("日本語 abc", "abc"), Some(10..13));
        assert_eq!(find("abc", "x"), None);
    }
}
