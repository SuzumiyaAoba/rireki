//! 表示順と、セッションの起動元探索の計測。

use super::measure;
use crate::model::{Event, Kind, Lane, Session, Source, Store};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

pub(super) fn view_order(store: &Store) -> Vec<usize> {
    let mut order: Vec<_> = (0..store.sessions.len())
        .filter(|&i| store.visible(i))
        .collect();
    order.sort_by(|&a, &b| store.sessions[b].start.total_cmp(&store.sessions[a].start));
    order
}

/// 描画せずに表示用の順序とセッション間の起動元を測る
pub fn view(index: Option<&Path>) {
    if let Some(index) = index {
        let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
        let mut store = Store::default();
        store.replace(db.load_all().unwrap());
        let order = view_order(&store);
        let links = crate::view_index::exec_parents(&store.sessions, &order);
        let mut sorted: Vec<_> = links
            .iter()
            .map(|(&child, &(parent, lane, start))| (child, parent, lane, start.to_bits()))
            .collect();
        sorted.sort_unstable();
        let mut hash = DefaultHasher::new();
        sorted.hash(&mut hash);
        println!(
            "{}",
            serde_json::json!({"sessions":order.len(),"headless":order.iter().filter(|&&i| store.sessions[i].headless).count(),
            "links":links.len(),"links_hash":format!("{:016x}", hash.finish())})
        );
        measure("view_order", 31, || view_order(&store));
        measure("exec_parents_real", 31, || {
            crate::view_index::exec_parents(&store.sessions, &order)
        });
    }
    let mut sessions = Vec::new();
    for si in 0..128 {
        let mut lane = Lane::new("main");
        let events = if si < 64 { 2000 } else { 1 };
        for ei in 0..events {
            let start = if si < 64 {
                ei as f64
            } else {
                5000.0 + si as f64
            };
            lane.events.push(Event {
                start,
                end: if si < 64 && ei == 0 {
                    100_000.0
                } else {
                    start + 0.5
                },
                kind: if si >= 64 {
                    Kind::Assistant
                } else if ei == 0 {
                    Kind::Agent
                } else {
                    Kind::Tool
                },
                name: "exec_command".into(),
                summary: Default::default(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: Default::default(),
                usage: Default::default(),
                model: None,
                status: Default::default(),
            });
        }
        let mut session = Session {
            id: format!("session-{si}"),
            source: Source::Codex,
            cwd: "/synthetic".into(),
            headless: si >= 64,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        sessions.push(std::sync::Arc::new(session));
    }
    let order: Vec<_> = (0..sessions.len()).collect();
    assert!(crate::view_index::exec_parents(&sessions, &order).is_empty());
    measure("exec_parents_overlapping_agent", 7, || {
        crate::view_index::exec_parents(&sessions, &order)
    });
}
