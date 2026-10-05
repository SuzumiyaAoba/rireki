//! 描画から独立したセッションの表示用索引

use crate::model::Session;
use std::collections::HashMap;

/// 非対話で起動されたセッションを、同じディレクトリの他セッションで
/// その時刻に実行中だったツール呼び出しの子とみなす
pub(crate) fn exec_parents(
    sessions: &[std::sync::Arc<Session>],
    order: &[usize],
) -> HashMap<usize, (usize, usize, f64)> {
    // 起動コマンドはすぐ制御を返すことがあるので、呼び出し終了後もしばらくは候補にする
    const SLACK: f64 = 30.0;
    let is_launch = |e: &crate::model::Event| e.kind == crate::model::Kind::Tool;
    let mut by_cwd: HashMap<&str, Vec<usize>> = Default::default();
    for &si in order {
        by_cwd.entry(sessions[si].cwd.as_str()).or_default().push(si);
    }
    let mut out = HashMap::new();
    for group in by_cwd.values() {
        for &ci in group.iter().filter(|&&ci| sessions[ci].headless) {
            let t = sessions[ci].start;
            let mut best: Option<(usize, usize, f64)> = None;
            for &pi in group {
                let p = &sessions[pi];
                if pi == ci || p.start > t || p.end < t - SLACK {
                    continue;
                }
                for (li, l) in p.lanes.iter().enumerate() {
                    let hi = l.events.partition_point(|e| e.start <= t);
                    let lo = l.cum_tool_end[..hi].partition_point(|&m| m < t - SLACK);
                    for e in l.events[lo..hi].iter().filter(|e| e.end >= t - SLACK && is_launch(e)) {
                        // 直前に起動したものを優先する
                        if best.is_none_or(|b| e.start > b.2) {
                            best = Some((pi, li, e.start));
                        }
                    }
                }
            }
            if let Some(b) = best {
                out.insert(ci, b);
            }
        }
    }
    // 循環を断つ
    let keys: Vec<usize> = out.keys().copied().collect();
    for k in keys {
        let mut cur = k;
        for _ in 0..=out.len() {
            match out.get(&cur) {
                Some(&(p, _, _)) if p == k => {
                    out.remove(&k);
                    break;
                }
                Some(&(p, _, _)) => cur = p,
                None => break,
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, Kind, Lane, Source};
    use std::sync::Arc;

    fn event(start: f64, end: f64, kind: Kind) -> Event {
        Event {
            start,
            end,
            kind,
            name: "tool".into(),
            summary: String::new(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            row: None,
            usage: Default::default(),
            model: None,
            status: Default::default(),
        }
    }

    fn session(cwd: &str, headless: bool, events: Vec<Vec<Event>>) -> Arc<Session> {
        let lanes = events
            .into_iter()
            .map(|events| {
                let mut lane = Lane::new("main");
                lane.events = events;
                lane.finish();
                lane
            })
            .collect();
        let mut session = Session {
            id: String::new(),
            source: Source::Codex,
            cwd: cwd.into(),
            headless,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes,
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        Arc::new(session)
    }

    /// 全イベントを直接調べる。子同士は起動ツールを持たないデータで循環を作らない
    fn reference(sessions: &[Arc<Session>], order: &[usize]) -> HashMap<usize, (usize, usize, f64)> {
        let mut result = HashMap::new();
        for &child in order.iter().filter(|&&i| sessions[i].headless) {
            let child_session = &sessions[child];
            let t = child_session.start;
            let mut best: Option<(usize, usize, f64)> = None;
            for &parent in order {
                let session = &sessions[parent];
                if parent == child || session.cwd != child_session.cwd || session.start > t || session.end < t - 30.0 {
                    continue;
                }
                for (lane, events) in session.lanes.iter().enumerate() {
                    for event in &events.events {
                        if event.kind == Kind::Tool
                            && event.start <= t
                            && event.end >= t - 30.0
                            && best.is_none_or(|(_, _, start)| event.start > start)
                        {
                            best = Some((parent, lane, event.start));
                        }
                    }
                }
            }
            if let Some(best) = best {
                result.insert(child, best);
            }
        }
        result
    }

    #[test]
    fn exec_parents_keep_scope_ties_slack_and_skip_long_agents() {
        let sessions = vec![
            session(
                "a",
                false,
                vec![
                    vec![event(0.0, 10_000.0, Kind::Agent), event(20.0, 20.0, Kind::Tool)],
                    vec![event(49.0, 49.25, Kind::Tool)],
                ],
            ),
            session("a", false, vec![vec![event(49.0, 49.25, Kind::Tool)]]),
            session("other", false, vec![vec![event(49.9, 1000.0, Kind::Tool)]]),
            session("a", true, vec![vec![event(50.0, 50.5, Kind::Assistant)]]),
            session("a", true, vec![vec![event(81.0, 81.5, Kind::Assistant)]]),
        ];
        let order = [1, 0, 2, 3, 4];
        let result = exec_parents(&sessions, &order);
        assert_eq!(result, reference(&sessions, &order));
        assert_eq!(result[&3], (1, 0, 49.0));
        assert!(!result.contains_key(&4));
        assert_eq!(exec_parents(&sessions, &[0, 2, 3])[&3], (0, 1, 49.0));
        let boundary = vec![
            session(
                "a",
                false,
                vec![vec![event(20.0, 20.0, Kind::Tool), event(21.0, 9999.0, Kind::Agent)]],
            ),
            session("a", true, vec![vec![event(50.0, 1000.0, Kind::Tool)]]),
            session("a", true, vec![vec![event(50.001, 50.002, Kind::Assistant)]]),
        ];
        assert_eq!(exec_parents(&boundary, &[0, 1])[&1], (0, 0, 20.0));
        assert!(exec_parents(&boundary, &[0, 2]).is_empty());
    }

    #[test]
    fn exec_parents_match_direct_scan_with_overlapping_events() {
        let mut seed = 7u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            seed >> 32
        };
        for _ in 0..20 {
            let mut sessions = Vec::new();
            for si in 0..48 {
                let headless = si >= 24;
                let lanes = (0..if headless { 1 } else { 3 })
                    .map(|_| {
                        let count = if headless { 1 } else { 24 };
                        (0..count)
                            .map(|_| {
                                let start = (next() % 500) as f64;
                                let end = start + (next() % 1000) as f64;
                                let kind = if headless {
                                    Kind::Assistant
                                } else {
                                    match next() % 3 {
                                        0 => Kind::Tool,
                                        1 => Kind::Agent,
                                        _ => Kind::User,
                                    }
                                };
                                event(start, end, kind)
                            })
                            .collect()
                    })
                    .collect();
                sessions.push(session(if si % 2 == 0 { "a" } else { "b" }, headless, lanes));
            }
            let mut order: Vec<_> = (0..sessions.len()).filter(|_| next() % 4 != 0).collect();
            order.sort_by(|&a, &b| sessions[b].start.total_cmp(&sessions[a].start));
            assert_eq!(exec_parents(&sessions, &order), reference(&sessions, &order));
        }
    }
}
