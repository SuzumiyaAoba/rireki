use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use crate::model::{Session, Store};
use crate::search::{Index, Results};

type Match = (usize, usize, usize, (u32, u32));

/// 索引の連結・探索と独立して、イベントを一件ずつ検索する。
fn reference(sessions: &[Arc<Session>], scope: &[usize], query: &str, limit: usize) -> (Vec<usize>, Vec<Match>, usize) {
    let terms: Vec<_> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
    let mut heads = Vec::new();
    let mut hits = Vec::new();
    for &si in scope {
        let s = &sessions[si];
        let head = format!("{}\n{}\n{}", s.title, s.cwd, s.id).to_ascii_lowercase();
        if terms.iter().all(|t| head.contains(t)) { heads.push(si); }
        for (li, lane) in s.lanes.iter().enumerate() {
            for (ei, e) in lane.events.iter().enumerate() {
                let text = format!("{} {}\n", e.name, e.summary).to_ascii_lowercase();
                if terms.iter().all(|t| text.contains(t)) {
                    let start = text.find(&terms[0]).unwrap() as u32;
                    hits.push((si, li, ei, (start, start + terms[0].len() as u32)));
                }
            }
        }
    }
    let total = hits.len();
    hits.sort_by(|a, b| {
        let start = |&(si, li, ei, _): &Match| sessions[si].lanes[li].events[ei].start;
        start(b).total_cmp(&start(a)).then((b.0, b.1, b.2).cmp(&(a.0, a.1, a.2)))
    });
    hits.truncate(limit);
    heads.sort_by(|&a, &b| sessions[b].end.total_cmp(&sessions[a].end));
    (heads, hits, total)
}

fn contents(result: &Results) -> (Vec<usize>, Vec<Match>, usize) {
    (result.sessions.clone(), result.events.iter().map(|h| (h.session, h.lane, h.event, h.at)).collect(), result.total)
}

fn dataset(label: &str, sessions: &[Arc<Session>], scope: &[usize], queries: &[&str]) {
    println!("{}", serde_json::json!({"dataset":label}));
    if scope.is_empty() { return; }
    for (case, &query) in queries.iter().enumerate() {
        let mut index = Index::default();
        let result = index.search(sessions, scope, query, 500);
        let actual = contents(&result);
        assert_eq!(actual, reference(sessions, scope, query, 500));
        let mut hash = DefaultHasher::new();
        actual.hash(&mut hash);
        println!("{}", serde_json::json!({"case":case,"total":result.total,"events":result.events.len(),
            "sessions":result.sessions.len(),"content_hash":format!("{:016x}",hash.finish()),"reference_equal":true}));
        super::measure(&format!("search_cold_{case}"), 15, || Index::default().search(sessions, scope, query, 500));
        super::measure(&format!("search_warm_{case}"), 31, || index.search(sessions, scope, query, 500));
    }
    let largest = *scope.iter().max_by_key(|&&si| sessions[si].lanes.iter().map(|l| l.events.len()).sum::<usize>()).unwrap();
    super::measure("search_cancel_cold", 31, || {
        let mut index = Index::default();
        let checks = Cell::new(0);
        let result = index.search_while(sessions, &[largest], queries[0], 500, || {
            checks.set(checks.get() + 1);
            checks.get() < 3
        });
        assert!(result.is_none());
        assert_eq!(checks.get(), 3);
    });
}

pub(crate) fn benchmark(path: &Path) {
    let db = crate::db::Db::open_index_read(path).unwrap();
    let mut store = Store::default();
    store.replace(db.load_all().unwrap());
    let order = super::view_order(&store);
    dataset("real", &store.sessions, &order, &["exec", "exec cargo", "日本語"]);
    let dense = super::synthetic_store(200_000);
    dataset("dense", &dense.sessions, &[0], &["cargo", "exec cargo", "日本語"]);
}
