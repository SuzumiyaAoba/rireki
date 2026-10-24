//! MCP の読み取り専用履歴 API。GUI の絞り込みから独立した同一スナップショットを使う。

use super::query::{Args, Order, kind, selected, selected_slice, source, status};
pub(super) use super::schema::definitions;

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use serde_json::{Value, json};

use crate::db::Db;
use crate::detail::analysis::duration;
use crate::model::{Event, EventText, Session, Snapshot, Stats, Status, UsageSum, clip};

pub(super) fn call(
    snapshot: &Snapshot,
    revision: u64,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let definitions = definitions();
    let definition = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .ok_or_else(|| format!("Unknown tool: {name}"))?;
    if let Some(arguments) = arguments.as_object() {
        for key in arguments.keys() {
            if definition["inputSchema"]["properties"].get(key).is_none() {
                return Err(format!("Unsupported argument for {name}: {key}"));
            }
        }
    }
    let args: Args = serde_json::from_value(arguments).map_err(|e| e.to_string())?;
    args.validate()?;
    if args.expected_revision.is_some_and(|v| v != revision) {
        return Err(format!(
            "History changed (current revision {revision}); restart pagination without expected_revision"
        ));
    }
    if matches!(name, "get_session" | "get_events") && args.session_id.is_none() {
        return Err("session_id is required".into());
    }
    if name == "search_events" && args.query.trim().is_empty() {
        return Err("query must not be empty".into());
    }
    let sessions: Vec<_> = snapshot
        .sessions
        .iter()
        .enumerate()
        .filter(|(i, s)| snapshot.visible(*i) && args.session_matches(s))
        .collect();
    if args.session_id.is_some() && sessions.is_empty() {
        return Err("Session not found or excluded by filters".into());
    }
    let mut result = match name {
        "list_sessions" => list_sessions(&sessions, &args),
        "get_session" => {
            let s = sessions[0].1;
            let mut result = session(s);
            result["lanes"] = Value::Array(s.lanes.iter().enumerate().map(|(i, l)| json!({
                "index":i,"name":l.name,"parent":s.parents.get(i).copied().flatten(),
                "spawned_by":l.spawned_by,"model":l.model,"effort":l.effort,"events":l.events.len()
            })).collect());
            result["analysis"] = analyze(&sessions, &args);
            result
        }
        "get_events" | "search_events" => events(snapshot, &sessions, &args)?,
        "analyze_history" => analyze(&sessions, &args),
        _ => return Err(format!("Unknown tool: {name}")),
    };
    result["revision"] = json!(revision);
    Ok(result)
}

fn session(s: &Session) -> Value {
    json!({"id":s.id,"source":source(s.source),"title":s.title,"cwd":s.cwd,"branch":s.branch,
        "version":s.version,"headless":s.headless,"start":s.start,"end":s.end,
        "lane_count":s.lanes.len(),"event_count":s.lanes.iter().map(|l| l.events.len()).sum::<usize>()})
}

fn list_sessions(sessions: &[(usize, &std::sync::Arc<Session>)], args: &Args) -> Value {
    let query = args.query.to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    let mut matches: Vec<_> = sessions
        .iter()
        .filter(|(_, s)| crate::search::matches_words(&words, [&s.title, &s.cwd, &s.id]))
        .collect();
    let total = matches.len();
    let end = (args.offset + args.limit()).min(total);
    // ページ外を全件ソートしない。同時刻・同 ID の順序も従来の安定ソートと揃える。
    let compare = |&&(ai, a): &&(usize, &std::sync::Arc<Session>),
                   &&(bi, b): &&(usize, &std::sync::Arc<Session>)| {
        b.start
            .total_cmp(&a.start)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| ai.cmp(&bi))
    };
    let selected = if args.offset < end {
        if end < total {
            matches.select_nth_unstable_by(end, compare);
        }
        let selected = &mut matches[..end];
        if args.offset > 0 {
            selected.select_nth_unstable_by(args.offset, compare);
        }
        let selected = &mut selected[args.offset..];
        selected.sort_unstable_by(compare);
        selected
    } else {
        &mut []
    };
    page(
        "sessions",
        selected.iter().map(|(_, s)| session(s)).collect(),
        total,
        args,
    )
}

fn page(key: &str, items: Vec<Value>, total: usize, args: &Args) -> Value {
    let end = args.offset + items.len();
    json!({key:items,"total":total,"offset":args.offset,"next_offset":(end < total).then_some(end)})
}

// ヒープには要求ページの末尾までの位置だけを保持し、全履歴のイベント列は複製しない。
struct Rank {
    start: f64,
    si: usize,
    li: usize,
    ei: usize,
    newest: bool,
}

impl PartialEq for Rank {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for Rank {}
impl PartialOrd for Rank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Rank {
    fn cmp(&self, other: &Self) -> Ordering {
        let time = self.start.total_cmp(&other.start);
        (if self.newest { time.reverse() } else { time })
            .then_with(|| (self.si, self.li, self.ei).cmp(&(other.si, other.li, other.ei)))
    }
}

/// 絞り込みのない新着一覧は、整列を確認したレーンの上位だけ候補へ渡す。
/// 同時刻はイベント番号順とし、未整列のレーンは全件を通常の順位判定へ渡す。
fn newest_candidates(
    session: &Session,
    args: &Args,
    keep: usize,
    mut offer: impl FnMut(usize, usize, &Event),
) -> usize {
    let mut total = 0;
    for (li, lane) in selected(&session.lanes, args.lane) {
        let (base, events) = selected_slice(&lane.events, args.event_index);
        total += events.len();
        if events.len() <= keep || !events.is_sorted_by(|a, b| a.start.total_cmp(&b.start).is_le())
        {
            for (ei, event) in events.iter().enumerate() {
                offer(li, base + ei, event);
            }
            continue;
        }
        let mut end = events.len();
        let mut remaining = keep;
        while remaining > 0 {
            let time = events[end - 1].start;
            let first = if end > 1 && events[end - 2].start.total_cmp(&time).is_eq() {
                events[..end].partition_point(|e| e.start.total_cmp(&time).is_lt())
            } else {
                end - 1
            };
            let take = remaining.min(end - first);
            for (ei, event) in events[first..first + take].iter().enumerate() {
                offer(li, base + first + ei, event);
            }
            remaining -= take;
            end = first;
        }
    }
    total
}

fn events(
    snapshot: &Snapshot,
    sessions: &[(usize, &std::sync::Arc<Session>)],
    args: &Args,
) -> Result<Value, String> {
    if args.session_id.is_some() {
        if args.lane.is_some_and(|li| li >= sessions[0].1.lanes.len()) {
            return Err("Lane index out of range".into());
        }
        if let (Some(li), Some(ei)) = (args.lane, args.event_index)
            && ei >= sessions[0].1.lanes[li].events.len()
        {
            return Err("Event index out of range".into());
        }
    }
    let query = args.query.to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    let mut heap = BinaryHeap::new();
    let keep = args.offset + args.limit();
    let newest = matches!(args.order, Order::Newest);
    let unfiltered_newest = newest
        && words.is_empty()
        && args.after.is_none()
        && args.before.is_none()
        && args.kind.is_none()
        && args.status.is_none();
    let mut total = 0;
    for &(si, s) in sessions {
        let mut offer = |li, ei, e: &Event| {
            let rank = Rank {
                start: e.start,
                si,
                li,
                ei,
                newest,
            };
            if heap.len() < keep {
                heap.push(rank);
            } else if heap.peek().is_some_and(|worst| rank < *worst) {
                *heap.peek_mut().unwrap() = rank;
            }
        };
        if unfiltered_newest {
            total += newest_candidates(s, args, keep, offer);
        } else {
            for (li, ei, e) in args.matching_events(s, &words) {
                total += 1;
                offer(li, ei, e);
            }
        }
    }
    let reader = args.include_text.then(Db::open_read).and_then(Result::ok);
    let items = heap.into_sorted_vec().into_iter().skip(args.offset).map(|r| {
        let s = &snapshot.sessions[r.si];
        let e = &s.lanes[r.li].events[r.ei];
        let mut value = json!({"session_id":s.id,"source":source(s.source),"lane":r.li,"event_index":r.ei,
            "start":e.start,"end":e.end,"kind":kind(e.kind),"name":e.name.as_ref(),
            "summary":e.summary.as_ref(),"status":status(e.status),"call_id":e.call_id.as_deref(),
            "model":e.model.as_deref(),"usage":{"input":e.usage.input,"cache_read":e.usage.cache_read,
            "cache_write":e.usage.cache_write,"output":e.usage.output,"reasoning":e.usage.reasoning}});
        if args.include_text {
            let text = event_text(e, reader.as_ref());
            value["text_available"] = json!(text.is_some());
            if let Some((input, output)) = text {
                let limit = args.text_limit.unwrap_or(4000);
                let (input, input_truncated) = clip(&input, limit);
                let (output, output_truncated) = clip(&output, limit);
                value["input"] = json!(input);
                value["output"] = json!(output);
                value["input_truncated"] = json!(input_truncated);
                value["output_truncated"] = json!(output_truncated);
            }
        }
        value
    }).collect();
    Ok(page("events", items, total, args))
}

fn event_text<'a>(e: &'a Event, reader: Option<&Db>) -> Option<(Cow<'a, str>, Cow<'a, str>)> {
    match e.text() {
        EventText::Inline(i, o) => Some((Cow::Borrowed(i), Cow::Borrowed(o))),
        EventText::Pending(i, o) => Some((Cow::Owned(i), Cow::Owned(o))),
        EventText::Row(row) => {
            let fetched = reader.and_then(|db| db.details(&[row]).remove(&row));
            // DB 更新で旧版の本文が圧縮保持へ移った場合は、行 ID の再利用より旧版を優先する。
            match e.text() {
                EventText::Pending(i, o) => Some((Cow::Owned(i), Cow::Owned(o))),
                _ => fetched.map(|(i, o)| (Cow::Owned(i), Cow::Owned(o))),
            }
        }
    }
}

fn usage(u: &UsageSum) -> Value {
    json!({"input":u.input,"cache_read":u.cache_read,"cache_write":u.cache_write,
        "output":u.output,"reasoning":u.reasoning,"total":u.total(),"calls":u.calls,
        "estimated_cost_usd":u.cost,"unpriced_calls":u.unpriced})
}

#[derive(Default)]
struct Tool {
    calls: usize,
    failed: usize,
    pending: usize,
    seconds: f64,
    max_seconds: f64,
}

fn analyze(sessions: &[(usize, &std::sync::Arc<Session>)], args: &Args) -> Value {
    let query = args.query.to_lowercase();
    let words: Vec<_> = query.split_whitespace().collect();
    let mut stats = Stats::default();
    let mut tokens = UsageSum::default();
    let mut models = HashMap::<&str, UsageSum>::new();
    let mut prices = crate::pricing::Cache::default();
    let mut tools = HashMap::<&str, Tool>::new();
    let mut count = 0;
    let mut session_count = 0;
    for &(_, s) in sessions {
        let before = count;
        for (_, _, e) in args.matching_events(s, &words) {
            count += 1;
            stats.add(e);
            if !e.usage.is_empty() {
                let price = prices.get(e.model.as_deref());
                tokens.add_priced(&e.usage, price);
                models
                    .entry(e.model.as_deref().unwrap_or("unknown"))
                    .or_default()
                    .add_priced(&e.usage, price);
            }
            if e.kind.is_tool() {
                let tool = tools.entry(&e.name).or_default();
                tool.calls += 1;
                tool.failed += usize::from(e.status.failed());
                tool.pending += usize::from(e.status == Status::Pending);
                if e.status != Status::Pending {
                    let seconds = duration(e);
                    tool.seconds += seconds;
                    tool.max_seconds = tool.max_seconds.max(seconds);
                }
            }
        }
        session_count += usize::from(count > before);
    }
    let mut tools: Vec<_> = tools.into_iter().collect();
    tools.sort_by(|(an, a), (bn, b)| b.calls.cmp(&a.calls).then_with(|| an.cmp(bn)));
    let tool_count = tools.len();
    let mut models: Vec<_> = models.into_iter().collect();
    models.sort_by(|(an, a), (bn, b)| b.total().cmp(&a.total()).then_with(|| an.cmp(bn)));
    let model_count = models.len();
    json!({"sessions":session_count,"events":count,"turns":stats.turns,"tool_calls":stats.tools,
        "errors":stats.errors,"rejected":stats.rejected,"interrupted":stats.interrupted,"peak_context_tokens":stats.peak,
        "usage":usage(&tokens),"tool_count":tool_count,"model_count":model_count,
        "tools":tools.into_iter().take(100).map(|(name,t)| json!({"name":name,"calls":t.calls,
            "failed":t.failed,"pending":t.pending,"completed_seconds":t.seconds,"max_seconds":t.max_seconds})).collect::<Vec<_>>(),
        "models":models.into_iter().take(100).map(|(model,u)| json!({"model":model,"usage":usage(&u)})).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{fixtures, pack_files};
    use crate::model::Loaded;

    fn fixture_session() -> Session {
        let mut items = fixtures::items(&[("Read", "", "")]);
        let Loaded::Session(session) = items.remove(0) else {
            panic!()
        };
        session
    }

    #[test]
    fn session_pages_match_full_stable_sort_including_equal_keys() {
        let sessions: Vec<_> = (0..1023)
            .map(|i| {
                let mut session = fixture_session();
                session.id = format!("session-{}", i % 13);
                session.title = format!("title-{i}");
                session.start = ((i * 19) % 31) as f64;
                std::sync::Arc::new(session)
            })
            .collect();
        let selected: Vec<_> = sessions.iter().enumerate().collect();
        for query in ["", "title-1", "missing"] {
            let mut expected: Vec<_> = sessions
                .iter()
                .filter(|s| s.title.contains(query))
                .collect();
            expected.sort_by(|a, b| b.start.total_cmp(&a.start).then_with(|| a.id.cmp(&b.id)));
            for offset in [0, 1, 49, 100, 999, 1023, 10_000] {
                for limit in [1, 50, 100] {
                    let args = Args {
                        query: query.into(),
                        offset,
                        limit: Some(limit),
                        ..Args::default()
                    };
                    let result = list_sessions(&selected, &args);
                    assert_eq!(result["total"], json!(expected.len()));
                    assert_eq!(
                        result["sessions"],
                        json!(
                            expected
                                .iter()
                                .skip(offset)
                                .take(limit)
                                .map(|s| session(s))
                                .collect::<Vec<_>>()
                        )
                    );
                    let end = offset + expected.iter().skip(offset).take(limit).count();
                    assert_eq!(
                        result["next_offset"],
                        json!((end < expected.len()).then_some(end))
                    );
                }
            }
        }
    }

    #[test]
    fn event_pages_match_full_sort_with_ties_unsorted_times_and_filters() {
        for distribution in 0..7 {
            assert_event_pages(distribution);
        }
    }

    #[test]
    fn newest_candidates_bound_sorted_lanes_and_fall_back_for_unsorted_lanes() {
        let mut session = fixture_session();
        let base = session.lanes[0].events[0].clone();
        session.lanes[0].events = (0..1000)
            .map(|i| {
                let mut event = base.clone();
                event.start = (i / 100) as f64;
                event
            })
            .collect();
        session.lanes.push(session.lanes[0].clone());
        let args = Args::default();
        let mut actual = Vec::new();
        let total = newest_candidates(&session, &args, 5, |li, ei, _| actual.push((li, ei)));
        assert_eq!(total, 2000);
        assert_eq!(
            actual,
            [
                (0, 900),
                (0, 901),
                (0, 902),
                (0, 903),
                (0, 904),
                (1, 900),
                (1, 901),
                (1, 902),
                (1, 903),
                (1, 904)
            ]
        );

        session.lanes[0].events.swap(0, 999);
        actual.clear();
        let total = newest_candidates(&session, &args, 5, |li, ei, _| actual.push((li, ei)));
        assert_eq!(total, 2000);
        assert_eq!(actual.len(), 1005);
        assert_eq!(
            actual[..1000],
            (0..1000).map(|ei| (0, ei)).collect::<Vec<_>>()
        );
        assert_eq!(
            actual[1000..],
            [(1, 900), (1, 901), (1, 902), (1, 903), (1, 904)]
        );
    }

    fn assert_event_pages(distribution: usize) {
        let mut snapshot = Snapshot::default();
        for si in 0..4 {
            let mut session = fixture_session();
            session.id = format!("session-{si}");
            let base = session.lanes[0].events[0].clone();
            session.lanes[0].events = (0..127)
                .map(|ei| {
                    let mut event = base.clone();
                    event.start = match distribution {
                        0 => match ei {
                            0 => -0.0,
                            1 => f64::NEG_INFINITY,
                            2 => f64::INFINITY,
                            3 => f64::NAN,
                            _ => ((ei * 7 + si) % 19) as f64,
                        },
                        1 => 42.0,
                        2 => (ei / 32) as f64,
                        4 if ei == 0 || ei == 126 => 0.0,
                        4 => ((ei * 7 + si) % 19) as f64,
                        5 => match ei {
                            0 => f64::NEG_INFINITY,
                            1..=20 => -0.0,
                            21..=40 => 0.0,
                            41..=80 => 1.0,
                            81..=100 => f64::INFINITY,
                            _ => f64::NAN,
                        },
                        _ => ei as f64,
                    };
                    event.status = if ei % 3 == 0 {
                        Status::Error
                    } else {
                        Status::Ok
                    };
                    event
                })
                .collect();
            if distribution == 6 {
                session.lanes[0].events.truncate(si);
            }
            for _ in 1..if distribution == 3 { 32 } else { 2 } {
                session.lanes.push(session.lanes[0].clone());
            }
            snapshot.sessions.push(std::sync::Arc::new(session));
        }
        let sessions: Vec<_> = snapshot.sessions.iter().enumerate().collect();
        for newest in [false, true] {
            for filter in 0..9 {
                let mut args = Args {
                    limit: Some(50),
                    order: if newest { Order::Newest } else { Order::Oldest },
                    ..Args::default()
                };
                match filter {
                    1 => args.status = Some("failed".into()),
                    2 => args.query = "READ".into(),
                    3 => args.query = "missing".into(),
                    4 => args.before = Some(40.0),
                    5 => args.after = Some(1.0),
                    6 => args.kind = Some("assistant".into()),
                    7 => args.lane = Some(1),
                    8 => {
                        args.lane = Some(0);
                        args.event_index = Some(73);
                    }
                    _ => {}
                }
                let query = args.query.to_lowercase();
                let mut expected = Vec::new();
                for &(si, s) in &sessions {
                    for (li, l) in s.lanes.iter().enumerate() {
                        for (ei, e) in l.events.iter().enumerate() {
                            if args.lane.is_none_or(|index| index == li)
                                && args.event_index.is_none_or(|index| index == ei)
                                && args.status.as_deref().is_none_or(|_| e.status.failed())
                                && args.kind.as_deref().is_none_or(|v| kind(e.kind) == v)
                                && args.after.is_none_or(|time| e.start >= time)
                                && args.before.is_none_or(|time| e.start <= time)
                                && format!(
                                    "{}\n{}\n{}",
                                    e.name,
                                    e.summary,
                                    e.model.as_deref().unwrap_or("")
                                )
                                .to_lowercase()
                                .contains(&query)
                            {
                                expected.push((e.start, si, li, ei));
                            }
                        }
                    }
                }
                expected.sort_by(|a, b| {
                    let time = if newest {
                        b.0.total_cmp(&a.0)
                    } else {
                        a.0.total_cmp(&b.0)
                    };
                    time.then_with(|| (a.1, a.2, a.3).cmp(&(b.1, b.2, b.3)))
                });
                for offset in [0, 1, 73, 999, 10_000] {
                    args.offset = offset;
                    let result = events(&snapshot, &sessions, &args).unwrap();
                    assert_eq!(result["total"], json!(expected.len()));
                    let actual: Vec<_> = result["events"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|e| {
                            (
                                e["session_id"].as_str().unwrap().to_owned(),
                                e["lane"].as_u64().unwrap() as usize,
                                e["event_index"].as_u64().unwrap() as usize,
                            )
                        })
                        .collect();
                    let wanted: Vec<_> = expected
                        .iter()
                        .skip(offset)
                        .take(50)
                        .map(|&(_, si, li, ei)| (format!("session-{si}"), li, ei))
                        .collect();
                    assert_eq!(
                        actual, wanted,
                        "distribution={distribution}, newest={newest}, filter={filter}, offset={offset}"
                    );
                    let end = offset + expected.iter().skip(offset).take(50).count();
                    assert_eq!(
                        result["next_offset"],
                        json!((end < expected.len()).then_some(end))
                    );
                }
            }
        }
    }

    #[test]
    fn inline_bodies_are_borrowed_and_unicode_clipping_is_preserved() {
        let mut session = fixture_session();
        let event = &mut session.lanes[0].events[0];
        event.input = "日本語🙂".repeat(100_000);
        event.output = "短い出力".into();
        assert!(matches!(
            event_text(event, None),
            Some((Cow::Borrowed(_), Cow::Borrowed(_)))
        ));
        let snapshot = Snapshot {
            sessions: vec![std::sync::Arc::new(session)],
        };
        let result = call(&snapshot, 7, "get_events", json!({"session_id":"session", "lane":0, "event_index":0, "include_text":true, "text_limit":5})).unwrap();
        assert_eq!(result["events"][0]["input"], "日本語🙂日");
        assert_eq!(result["events"][0]["input_truncated"], true);
        assert_eq!(result["events"][0]["output"], "短い出力");
        assert_eq!(result["events"][0]["output_truncated"], false);
        assert_eq!(result["revision"], 7);
    }

    #[test]
    fn analysis_reuses_prices_without_changing_usage_or_addition_order() {
        let mut session = fixture_session();
        let base = session.lanes[0].events[0].clone();
        session.lanes[0].events = (0..400)
            .map(|i| {
                let mut event = base.clone();
                event.model = match i % 7 {
                    0 | 1 => Some("claude-sonnet-4-6".into()),
                    2 => Some("gpt-6-sol".into()),
                    3 => None,
                    4 => Some("unknown".into()),
                    _ => Some(format!("unknown-{i}").into()),
                };
                if i % 11 != 0 {
                    event.usage.input = i * 111;
                    event.usage.output = 2 * i;
                    event.usage.cache_write = i;
                    event.usage.cache_write_1h = i / 2;
                    event.usage.cache_read = i * 7;
                    event.usage.reasoning = i;
                }
                event
            })
            .collect();
        let events = &session.lanes[0].events;
        let expected = UsageSum::of(events);
        let mut expected_models = HashMap::<&str, UsageSum>::new();
        for event in events {
            if !event.usage.is_empty() {
                expected_models
                    .entry(event.model.as_deref().unwrap_or("unknown"))
                    .or_default()
                    .add(&event.usage, event.model.as_deref());
            }
        }
        let mut expected_models: Vec<_> = expected_models.into_iter().collect();
        expected_models
            .sort_by(|(an, a), (bn, b)| b.total().cmp(&a.total()).then_with(|| an.cmp(bn)));
        let expected_models: Vec<_> = expected_models
            .into_iter()
            .take(100)
            .map(|(model, u)| json!({"model":model,"usage":usage(&u)}))
            .collect();
        let session = std::sync::Arc::new(session);
        let actual = analyze(&[(0, &session)], &Args::default());
        assert_eq!(actual["usage"], usage(&expected));
        assert_eq!(actual["models"], json!(expected_models));
    }

    #[test]
    fn event_bodies_survive_pending_compression_commits_and_replacement() {
        let fixture = fixtures::Fixture::new();
        let mut db = fixture.open();
        let mut items = fixtures::items(&[("Read", "日本語の入力", "original output")]);
        let packed = pack_files(&[("log".into(), "fp".into(), &items)]);
        packed[0].release_texts(&mut items);
        let Loaded::Session(s) = &items[0] else {
            panic!()
        };
        let event = &s.lanes[0].events[0];
        let expected = Some((
            Cow::Borrowed("日本語の入力"),
            Cow::Borrowed("original output"),
        ));
        assert_eq!(event_text(event, None), expected);
        db.put(&packed).unwrap();
        let reader = Db::open_index_read(&fixture.texts).unwrap();
        assert_eq!(event_text(event, Some(&reader)), expected);
        assert_eq!(event_text(event, None), None);
        let replaced = fixtures::items(&[("Read", "replacement", "different output")]);
        db.put(&pack_files(&[("log".into(), "new".into(), &replaced)]))
            .unwrap();
        assert_eq!(event_text(event, Some(&reader)), expected);
    }
}
