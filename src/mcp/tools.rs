//! MCP の読み取り専用履歴 API。GUI の絞り込みから独立した同一スナップショットを使う。

pub(super) use super::schema::definitions;

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::Db;
use crate::detail::analysis::duration;
use crate::model::{clip, Event, EventText, Kind, Session, Snapshot, Source, Stats, Status, UsageSum};

pub(super) const MAX_LIMIT: usize = 100;
pub(super) const MAX_OFFSET: usize = 10_000;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Order {
    #[default]
    Oldest,
    Newest,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Args {
    query: String,
    source: Option<String>,
    cwd: Option<String>,
    session_id: Option<String>,
    after: Option<f64>,
    before: Option<f64>,
    lane: Option<usize>,
    event_index: Option<usize>,
    kind: Option<String>,
    status: Option<String>,
    offset: usize,
    limit: Option<usize>,
    order: Order,
    include_text: bool,
    text_limit: Option<usize>,
    expected_revision: Option<u64>,
}

impl Args {
    fn validate(&self) -> Result<(), String> {
        if self.event_index.is_some() && self.lane.is_none() {
            return Err("event_index requires lane".into());
        }
        if self.limit.is_some_and(|v| !(1..=MAX_LIMIT).contains(&v)) || self.offset > MAX_OFFSET {
            return Err("limit must be 1..100 and offset must be 0..10000".into());
        }
        if self.text_limit.is_some_and(|v| !(1..=20_000).contains(&v)) {
            return Err("text_limit must be 1..20000 characters per input/output".into());
        }
        if self.query.chars().count() > 4096 {
            return Err("query must be at most 4096 characters".into());
        }
        if self.after.zip(self.before).is_some_and(|(a, b)| a > b) {
            return Err("after must be <= before (Unix seconds)".into());
        }
        for (value, allowed, field) in [
            (self.source.as_deref(), &["claude", "codex"][..], "source"),
            (
                self.kind.as_deref(),
                &["user", "assistant", "thinking", "tool", "agent"][..],
                "kind",
            ),
            (
                self.status.as_deref(),
                &[
                    "ok",
                    "error",
                    "rejected",
                    "interrupted",
                    "pending",
                    "failed",
                ][..],
                "status",
            ),
        ] {
            if value.is_some_and(|v| !allowed.contains(&v)) {
                return Err(format!(
                    "invalid {field}; expected one of {}",
                    allowed.join(", ")
                ));
            }
        }
        Ok(())
    }

    fn limit(&self) -> usize {
        self.limit.unwrap_or(50)
    }

    fn session_matches(&self, s: &Session) -> bool {
        self.session_id.as_deref().is_none_or(|id| s.id == id)
            && self.source.as_deref().is_none_or(|v| source(s.source) == v)
            && self.cwd.as_deref().is_none_or(|v| s.cwd == v)
            && self.after.is_none_or(|v| s.end >= v)
            && self.before.is_none_or(|v| s.start <= v)
    }

    fn event_matches(&self, e: &Event, words: &[&str]) -> bool {
        self.after.is_none_or(|v| e.start >= v)
            && self.before.is_none_or(|v| e.start <= v)
            && self.kind.as_deref().is_none_or(|v| kind(e.kind) == v)
            && self.status.as_deref().is_none_or(|v| {
                if v == "failed" {
                    e.status.failed()
                } else {
                    status(e.status) == v
                }
            })
            && crate::search::matches_words(words, [
                &e.name, &e.summary, e.model.as_deref().unwrap_or(""),
            ])
    }
}

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

fn source(s: Source) -> &'static str {
    match s {
        Source::Claude => "claude",
        Source::Codex => "codex",
    }
}

fn kind(k: Kind) -> &'static str {
    match k {
        Kind::User => "user",
        Kind::Assistant => "assistant",
        Kind::Thinking => "thinking",
        Kind::Tool => "tool",
        Kind::Agent => "agent",
    }
}

fn status(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Error => "error",
        Status::Rejected => "rejected",
        Status::Interrupted => "interrupted",
        Status::Pending => "pending",
    }
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
    matches.sort_by(|(_, a), (_, b)| b.start.total_cmp(&a.start).then_with(|| a.id.cmp(&b.id)));
    let total = matches.len();
    page(
        "sessions",
        matches
            .into_iter()
            .skip(args.offset)
            .take(args.limit())
            .map(|(_, s)| session(s))
            .collect(),
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
    let mut total = 0;
    for &(si, s) in sessions {
        for (li, lane) in s.lanes.iter().enumerate() {
            if args.lane.is_some_and(|v| v != li) {
                continue;
            }
            for (ei, e) in lane.events.iter().enumerate() {
                if args.event_index.is_some_and(|v| v != ei) || !args.event_matches(e, &words) {
                    continue;
                }
                total += 1;
                let rank = Rank {
                    start: e.start,
                    si,
                    li,
                    ei,
                    newest: matches!(args.order, Order::Newest),
                };
                if heap.len() < keep {
                    heap.push(rank);
                } else if heap.peek().is_some_and(|worst| rank < *worst) {
                    *heap.peek_mut().unwrap() = rank;
                }
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

fn event_text(e: &Event, reader: Option<&Db>) -> Option<(String, String)> {
    match e.text() {
        EventText::Inline(i, o) => Some((i.to_owned(), o.to_owned())),
        EventText::Pending(i, o) => Some((i, o)),
        EventText::Row(row) => {
            let fetched = reader.and_then(|db| db.details(&[row]).remove(&row));
            // DB 更新で旧版の本文が圧縮保持へ移った場合は、行 ID の再利用より旧版を優先する。
            match e.text() {
                EventText::Pending(i, o) => Some((i, o)),
                _ => fetched,
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
    let mut tools = HashMap::<&str, Tool>::new();
    let mut count = 0;
    let mut session_count = 0;
    for &(_, s) in sessions {
        let before = count;
        for (li, lane) in s.lanes.iter().enumerate() {
            if args.lane.is_some_and(|v| v != li) {
                continue;
            }
            for (ei, e) in lane.events.iter().enumerate() {
                if args.event_index.is_some_and(|v| v != ei) || !args.event_matches(e, &words) {
                    continue;
                }
                count += 1;
                stats.add(e);
                tokens.add(&e.usage, e.model.as_deref());
                if !e.usage.is_empty() {
                    models
                        .entry(e.model.as_deref().unwrap_or("unknown"))
                        .or_default()
                        .add(&e.usage, e.model.as_deref());
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
        let expected = Some(("日本語の入力".to_string(), "original output".to_string()));
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
