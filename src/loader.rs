//! Claude Code / Codex のセッションログ (JSONL) を読み込む
//!
//! このモジュールは形式ごとの解析状態とイベントの対応付けを管理する。
//! ファイル探索は `discovery`、行読み出しは `log_lines`、共通の本文整形は
//! `format`、Codex のツール・JS の整形は `codex_tools` に分ける。

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde_json::Value;

use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Status, Usage, one_line, truncate};

mod codex_row;
mod codex_tools;
mod discovery;
mod format;
mod live;
mod log_lines;

pub use codex_tools::{JS_MARK, exec_detail};
pub use discovery::{discover, discover_in};
pub use live::{LiveLog, LiveStats};

use codex_tools::{codex_output, codex_output_text, codex_tool};
use discovery::read_dir;
use format::{claude_input, claude_patch, pretty_output, summarize_input, text_of};
use log_lines::{BufferedLines, LogLines, json_row};

const MAX_TEXT: usize = 6000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobKind {
    Claude,
    /// 旧形式: プロジェクト直下の agent-*.jsonl
    ClaudeLegacyAgent,
    Codex,
}

/// 読み込み対象のファイル 1 つ（Claude はサブエージェントのファイルも含む）
#[derive(Clone, Debug)]
pub struct Job {
    pub kind: JobKind,
    pub path: PathBuf,
    /// 関連ファイルを含めた最終更新時刻 (Unix 秒)
    pub mtime: f64,
    /// 変更検知用の指紋（関連ファイルの mtime とサイズ）
    pub fingerprint: String,
}

impl Job {
    pub fn key(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

/// ファイルを並列にパースする。結果は jobs と同じ順
pub fn load(jobs: &[Job]) -> Vec<Vec<Loaded>> {
    jobs.par_iter().map(parse).collect()
}

pub fn parse(job: &Job) -> Vec<Loaded> {
    let p = &job.path;
    match job.kind {
        JobKind::Claude => load_claude(p),
        JobKind::ClaudeLegacyAgent => load_claude_legacy_agent(p).into_iter().collect(),
        JobKind::Codex => load_codex(p).into_iter().collect(),
    }
}

fn ts(v: &Value) -> Option<f64> {
    let s = v.get("timestamp")?.as_str()?;
    let t = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(t.timestamp_millis() as f64 / 1000.0)
}

fn is_agent_tool(name: &str) -> bool {
    matches!(name, "Agent" | "Task" | "spawn_agent")
}

fn message_event(t: f64, kind: Kind, name: Arc<str>, text: &str) -> Event {
    Event {
        start: t,
        end: t,
        kind,
        name,
        summary: one_line(text, if kind == Kind::User { usize::MAX } else { 160 }).into(),
        input: truncate(text, MAX_TEXT),
        output: String::new(),
        call_id: None,
        text_ref: Default::default(),
        usage: Usage::default(),
        model: None,
        status: Status::Ok,
    }
}

/// 結果を受け取るまでは Pending
fn tool_event(
    t: f64,
    name: Arc<str>,
    call_id: Option<&str>,
    summary: String,
    input: String,
) -> Event {
    Event {
        start: t,
        end: t,
        kind: if is_agent_tool(&name) { Kind::Agent } else { Kind::Tool },
        name,
        summary: summary.into(),
        input,
        output: String::new(),
        call_id: call_id.map(Arc::from),
        text_ref: Default::default(),
        usage: Usage::default(),
        model: None,
        status: Status::Pending,
    }
}

/// tool_result を対応する呼び出しに反映する
#[derive(Clone)]
struct Pending(HashMap<String, usize>, Option<TextChanges>);

/// ライブ解析だけで、未圧縮の新規イベントと更新された古い本文を追跡する。
#[derive(Clone, Default)]
struct TextChanges {
    first: usize,
    edited: Vec<usize>,
}

impl Pending {
    /// 結果を反映し、呼び出しのイベントの位置を返す
    fn finish(&mut self, events: &mut [Event], id: &str, t: f64, output: &str, status: Status) -> Option<usize> {
        let i = self.0.remove(id)?;
        let e = &mut events[i];
        if let crate::model::EventText::Pending(input, _) = e.text() {
            e.input = input;
            e.text_ref = Default::default();
        }
        e.end = t.max(e.start);
        e.output = truncate(&pretty_output(output), MAX_TEXT);
        e.status = status;
        if let Some(changes) = &mut self.1
            && i < changes.first
        {
            changes.edited.push(i);
        }
        Some(i)
    }
}

/// Claude の tool_result の状態。`result` は同じ行の toolUseResult
fn claude_status(item: &Value, result: Option<&Value>, out: &str) -> Status {
    if result.and_then(|r| r.get("interrupted")).and_then(|x| x.as_bool()) == Some(true) {
        return Status::Interrupted;
    }
    if item.get("is_error").and_then(|x| x.as_bool()) != Some(true) {
        return Status::Ok;
    }
    let rejected = result.and_then(|r| r.as_str()) == Some("User rejected tool use");
    if rejected || out.starts_with("The user doesn't want to") {
        Status::Rejected
    } else if out.starts_with("[Request interrupted by user") {
        Status::Interrupted
    } else {
        Status::Error
    }
}

// ---------------------------------------------------------------- Claude

/// モデル名やイベント名の共有（イベントごとに文字列を複製しない）
#[derive(Clone, Default)]
pub struct Interner(HashMap<String, Arc<str>>);

impl Interner {
    pub fn get(&mut self, s: &str) -> Arc<str> {
        if let Some(a) = self.0.get(s) {
            return a.clone();
        }
        let a: Arc<str> = Arc::from(s);
        self.0.insert(s.to_string(), a.clone());
        a
    }
}

fn num(v: &Value, k: &str) -> u32 {
    v.pointer(k).and_then(|x| x.as_u64()).map_or(0, |n| n.min(u32::MAX as u64) as u32)
}

fn add_usage(a: Usage, b: Usage) -> Usage {
    Usage {
        input: a.input.saturating_add(b.input),
        cache_read: a.cache_read.saturating_add(b.cache_read),
        cache_write: a.cache_write.saturating_add(b.cache_write),
        cache_write_1h: a.cache_write_1h.saturating_add(b.cache_write_1h),
        output: a.output.saturating_add(b.output),
        reasoning: a.reasoning.saturating_add(b.reasoning),
    }
}

/// Claude の message.usage。input_tokens はキャッシュを含まない
fn claude_usage(u: &Value) -> Usage {
    Usage {
        input: num(u, "/input_tokens"),
        cache_read: num(u, "/cache_read_input_tokens"),
        cache_write: num(u, "/cache_creation_input_tokens"),
        cache_write_1h: num(u, "/cache_creation/ephemeral_1h_input_tokens"),
        output: num(u, "/output_tokens"),
        reasoning: num(u, "/output_tokens_details/thinking_tokens"),
    }
}

/// Codex の token_count の累計（total_token_usage）。input_tokens はキャッシュを含む
fn codex_usage(u: &Value) -> Usage {
    let cached = num(u, "/cached_input_tokens");
    Usage {
        input: num(u, "/input_tokens").saturating_sub(cached),
        cache_read: cached,
        cache_write: num(u, "/cache_write_input_tokens"),
        cache_write_1h: 0,
        output: num(u, "/output_tokens"),
        reasoning: num(u, "/reasoning_output_tokens"),
    }
}

fn sub_usage(a: Usage, b: Usage) -> Usage {
    Usage {
        input: a.input.saturating_sub(b.input),
        cache_read: a.cache_read.saturating_sub(b.cache_read),
        cache_write: a.cache_write.saturating_sub(b.cache_write),
        cache_write_1h: a.cache_write_1h.saturating_sub(b.cache_write_1h),
        output: a.output.saturating_sub(b.output),
        reasoning: a.reasoning.saturating_sub(b.reasoning),
    }
}

struct ClaudeParse {
    lane: Lane,
    cwd: String,
    title: String,
    session_id: String,
    headless: bool,
    branch: String,
    version: String,
}

#[derive(Clone)]
struct ClaudeParser {
    lane: Lane,
    pending: Pending,
    names: Interner,
    cwd: String,
    title: String,
    session_id: String,
    headless: bool,
    branch: String,
    version: String,
    usages: Vec<(usize, Usage, Option<Arc<str>>)>,
    models: Interner,
    usage_ix: HashMap<String, usize>,
}

impl ClaudeParser {
    fn new(name: &str) -> Self {
        Self {
            lane: Lane::new(name), pending: Pending(HashMap::new(), None), names: Interner::default(),
            cwd: String::new(), title: String::new(), session_id: String::new(), headless: false,
            branch: String::new(), version: String::new(), usages: Vec::new(),
            models: Interner::default(), usage_ix: HashMap::new(),
        }
    }

    fn push(&mut self, line: &[u8]) {
        let Ok(v) = json_row(line) else {
            return;
        };
        if self.cwd.is_empty()
            && let Some(c) = v.get("cwd").and_then(|c| c.as_str())
        {
            self.cwd = c.into();
        }
        if let Some(e) = v.get("entrypoint").and_then(|e| e.as_str()) {
            self.headless |= e.starts_with("sdk");
        }
        if self.session_id.is_empty()
            && let Some(c) = v.get("sessionId").and_then(|c| c.as_str())
        {
            self.session_id = c.into();
        }
        if let Some(b) = v.get("gitBranch").and_then(|b| b.as_str()).filter(|b| !b.is_empty() && *b != "HEAD")
            && self.branch != b
        {
            self.branch = b.into();
        }
        if let Some(x) = v.get("version").and_then(|x| x.as_str())
            && self.version != x
        {
            self.version = x.into();
        }
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if (ty == "ai-title" || ty == "summary")
            && let Some(s) = v
                .get("aiTitle")
                .or(v.get("summary"))
                .and_then(|s| s.as_str())
        {
            self.title = s.into();
        }
        if ty == "assistant" {
            if self.lane.model.is_empty()
                && let Some(m) = v.pointer("/message/model").and_then(|m| m.as_str()).filter(|m| !m.starts_with('<'))
            {
                self.lane.model = m.to_string();
            }
            if self.lane.effort.is_empty()
                && let Some(e) = v.get("effort").and_then(|e| e.as_str())
            {
                self.lane.effort = e.to_string();
            }
        }
        let Some(t) = ts(&v) else { return };
        let Some(content) = v.pointer("/message/content") else {
            return;
        };
        match ty {
            "user" => {
                if v.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
                    return;
                }
                if let Value::Array(items) = content {
                    let results: Vec<&Value> = items
                        .iter()
                        .filter(|it| it.get("type").and_then(|x| x.as_str()) == Some("tool_result"))
                        .collect();
                    // toolUseResult は行に 1 つなので、結果が 1 つの行でだけ使う
                    let detail = v.get("toolUseResult").filter(|_| results.len() == 1);
                    for it in results {
                        let id = it.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                        let raw = text_of(it.get("content").unwrap_or(&Value::Null));
                        let status = claude_status(it, detail, &raw);
                        let out = raw.strip_prefix("<tool_use_error>")
                            .map(|e| e.strip_suffix("</tool_use_error>").unwrap_or(e)).unwrap_or(&raw);
                        let Some(i) = self.pending.finish(&mut self.lane.events, id, t, out, status) else { continue };
                        if let Some(d) = detail.and_then(claude_patch) {
                            self.lane.events[i].input = d;
                        }
                    }
                }
                let text = text_of(content);
                let text = text.trim();
                if !text.is_empty() && !text.starts_with('<') {
                    if self.title.is_empty() {
                        self.title = one_line(text, usize::MAX);
                    }
                    self.lane.events.push(message_event(t, Kind::User, self.names.get("User"), text));
                }
            }
            "assistant" => {
                let Value::Array(items) = content else {
                    return;
                };
                let first = self.lane.events.len();
                for it in items {
                    match it.get("type").and_then(|x| x.as_str()) {
                        Some("text") => {
                            let text = it.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                self.lane.events.push(message_event(t, Kind::Assistant, self.names.get("Assistant"), text));
                            }
                        }
                        Some("thinking") => {
                            let text = it.get("thinking").and_then(|x| x.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                self.lane.events.push(message_event(t, Kind::Thinking, self.names.get("Thinking"), text));
                            }
                        }
                        Some("tool_use") => {
                            let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                            let id = it.get("id").and_then(|x| x.as_str());
                            let input = it.get("input").unwrap_or(&Value::Null);
                            if let Some(id) = id {
                                self.pending.0.insert(id.to_owned(), self.lane.events.len());
                            }
                            self.lane.events.push(tool_event(
                                t,
                                self.names.get(name),
                                id,
                                summarize_input(name, input),
                                claude_input(name, input),
                            ));
                        }
                        _ => {}
                    }
                }
                if let Some(u) = v.pointer("/message/usage").map(claude_usage)
                    && !u.is_empty()
                {
                    let id = v.pointer("/message/id").and_then(|x| x.as_str()).unwrap_or_default();
                    // そのメッセージの最初のイベント。イベントを生まない行なら直前のイベント
                    let at = first.min(self.lane.events.len().saturating_sub(1));
                    let model = v
                        .pointer("/message/model")
                        .and_then(|m| m.as_str())
                        .filter(|m| !m.starts_with('<'))
                        .map(|m| self.models.get(m));
                    match self.usage_ix.get(id) {
                        Some(&k) if !id.is_empty() => (self.usages[k].1, self.usages[k].2) = (u, model),
                        _ => {
                            self.usage_ix.insert(id.to_string(), self.usages.len());
                            self.usages.push((at, u, model));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> ClaudeParse {
        // 同じ message.id の使用量は最後の値を一度だけ計上する。
        // 生のイベント列には加算せず、公開する版にだけ反映する。
        for (i, u, m) in self.usages {
            if let Some(e) = self.lane.events.get_mut(i) {
                e.usage = add_usage(e.usage, u);
                e.model = m.or(e.model.take());
            }
        }
        self.lane.finish();
        ClaudeParse { lane: self.lane, cwd: self.cwd, title: self.title,
            session_id: self.session_id, headless: self.headless, branch: self.branch, version: self.version }
    }
}

fn parse_claude_file(p: &Path, lane_name: &str) -> ClaudeParse {
    let mut parser = ClaudeParser::new(lane_name);
    let mut lines = LogLines::open(p, |_| false);
    while let Some(line) = lines.next_line() { parser.push(line); }
    parser.finish()
}

fn load_claude(p: &Path) -> Vec<Loaded> {
    let parsed = parse_claude_file(p, "main");
    let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
    let mut lanes = vec![parsed.lane];

    // 新形式: <session>/subagents/agent-*.jsonl (+ .meta.json)
    let sub_dir = p.with_extension("").join("subagents");
    let mut subs: Vec<PathBuf> =
        read_dir(&sub_dir).into_iter().filter(|f| f.extension().is_some_and(|e| e == "jsonl")).collect();
    subs.sort();
    for f in subs {
        let meta: Value = fs::read_to_string(f.with_extension("meta.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        let ty = meta.get("agentType").and_then(|x| x.as_str()).unwrap_or("agent");
        let desc = meta.get("description").and_then(|x| x.as_str()).unwrap_or("");
        let mut sub = parse_claude_file(&f, &format!("{ty}: {desc}")).lane;
        sub.spawned_by = meta.get("toolUseId").and_then(|x| x.as_str()).map(String::from);
        if !sub.events.is_empty() {
            lanes.push(sub);
        }
    }

    vec![Loaded::Session(Session {
        id: if parsed.session_id.is_empty() {
            id.clone()
        } else {
            parsed.session_id
        },
        source: Source::Claude,
        cwd: parsed.cwd,
        headless: parsed.headless,
        title: parsed.title,
        branch: parsed.branch,
        version: parsed.version,
        lanes,
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    })]
}

fn load_claude_legacy_agent(p: &Path) -> Option<Loaded> {
    let name = p.file_stem()?.to_str()?.to_string();
    let parsed = parse_claude_file(p, &name);
    if parsed.session_id.is_empty() {
        return None;
    }
    Some(Loaded::Child(parsed.session_id, parsed.lane))
}

// ---------------------------------------------------------------- Codex

/// JSON パース前に読み飛ばす行の種類
static SKIP: std::sync::LazyLock<[memchr::memmem::Finder<'static>; 4]> = std::sync::LazyLock::new(|| {
    ["\"type\":\"event_msg\"", "\"type\":\"turn_context\"", "\"type\":\"reasoning\"", "\"type\":\"world_state\""]
        .map(memchr::memmem::Finder::new)
});

/// event_msg のうち読むもの（小さい行だけ）: ゴールの設定、ターンの中断
static KEEP_EVENTS: std::sync::LazyLock<[memchr::memmem::Finder<'static>; 2]> = std::sync::LazyLock::new(|| {
    ["\"type\":\"thread_goal_updated\"", "\"type\":\"turn_aborted\""].map(memchr::memmem::Finder::new)
});

fn skip_codex_line(line: &[u8]) -> bool {
    let head = &line[..line.len().min(160)];
    let ctx = SKIP[1].find(head).is_some();
    let tc = memchr::memmem::find(head, b"\"type\":\"token_count\"").is_some();
    let keep = SKIP[0].find(head).is_some() && KEEP_EVENTS.iter().any(|f| f.find(head).is_some());
    !ctx && !tc && !keep && SKIP.iter().any(|f| f.find(head).is_some())
}

#[derive(Clone)]
struct CodexParser {
    lane: Lane,
    pending: Pending,
    names: Interner,
    id: String,
    cwd: String,
    title: String,
    parent: Option<String>,
    headless: bool,
    branch: String,
    version: String,
    goal: String,
    total: Usage,
    unassigned: Usage,
    model: Option<Arc<str>>,
    hidden: bool,
}

impl CodexParser {
    fn new() -> Self {
        Self {
            lane: Lane::new("main"), pending: Pending(HashMap::new(), None), names: Interner::default(),
            id: String::new(), cwd: String::new(), title: String::new(), parent: None, headless: false,
            branch: String::new(), version: String::new(), goal: String::new(), total: Usage::default(),
            unassigned: Usage::default(), model: None, hidden: false,
        }
    }

    fn push(&mut self, line: &[u8]) {
        if self.hidden { return; }
        // 巨大で不要な行は JSON パース前に捨てる
        let head = &line[..line.len().min(160)];
        let tc = memchr::memmem::find(head, b"\"type\":\"token_count\"").is_some();
        if skip_codex_line(line) {
            return;
        }
        let Ok(v) = codex_row::parse(line) else {
            return;
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let Some(pl) = v.get("payload") else { return };

        if tc {
            if let Some(info) = pl.get("info").filter(|i| !i.is_null()) {
                let now = info.get("total_token_usage").map(codex_usage).unwrap_or_default();
                // 累計が減ったら（圧縮などで数え直し）直近の分だけを使う
                let d = if now.total() >= self.total.total() {
                    sub_usage(now, self.total)
                } else {
                    info.get("last_token_usage").map(codex_usage).unwrap_or_default()
                };
                self.total = now;
                self.unassigned = add_usage(self.unassigned, d);
                if let Some(e) = self.lane.events.last_mut() {
                    e.usage = add_usage(e.usage, std::mem::take(&mut self.unassigned));
                    e.model = self.model.clone();
                }
            }
            return;
        }
        if ty == "turn_context" {
            let str_at = |k: &str| pl.pointer(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
            let m = pl.pointer("/model").and_then(|x| x.as_str()).unwrap_or_default();
            if !m.is_empty() && self.model.as_deref() != Some(m) {
                self.model = Some(Arc::from(m));
            }
            if !self.lane.model.is_empty() {
                return;
            }
            self.lane.model = m.to_owned();
            self.lane.effort = str_at("/effort");
            if self.lane.effort.is_empty() {
                self.lane.effort = str_at("/collaboration_mode/settings/reasoning_effort");
            }
            return;
        }
        if ty == "event_msg" {
            match pl.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                "thread_goal_updated" => {
                    // ゴール（/goal）で始めたセッションは、ゴールをユーザーの発話として扱う
                    let obj = pl.pointer("/goal/objective").and_then(|x| x.as_str()).unwrap_or("").trim();
                    if let Some(t) = ts(&v).filter(|_| !obj.is_empty() && obj != self.goal) {
                        self.goal = obj.to_string();
                        if self.title.is_empty() {
                            self.title = one_line(obj, usize::MAX);
                        }
                        self.lane.events.push(message_event(t, Kind::User, self.names.get("User"), obj));
                    }
                }
                "turn_aborted" => {
                    for &i in self.pending.0.values() {
                        self.lane.events[i].status = Status::Interrupted;
                    }
                }
                _ => {}
            }
            return;
        }
        if ty == "session_meta" {
            self.id = pl.get("id").and_then(|x| x.as_str()).unwrap_or_default().into();
            self.cwd = pl.get("cwd").and_then(|x| x.as_str()).unwrap_or_default().into();
            self.headless = pl.get("source").and_then(|x| x.as_str()) == Some("exec");
            self.version = pl.get("cli_version").and_then(|x| x.as_str()).unwrap_or_default().into();
            self.branch = pl.pointer("/git/branch").and_then(|x| x.as_str()).unwrap_or_default().into();
            if let Some(sub) = pl.pointer("/source/subagent") {
                let Some(spawn) = sub.get("thread_spawn") else {
                    self.hidden = true; return; // guardian などの内部サブエージェントは表示しない
                };
                self.parent = spawn.get("parent_thread_id").and_then(|x| x.as_str()).map(String::from);
                let nick = spawn.get("agent_nickname").and_then(|x| x.as_str()).unwrap_or("agent");
                let path = spawn.get("agent_path").and_then(|x| x.as_str()).unwrap_or("");
                self.lane.name = format!("{nick}: {}", crate::paths::basename(path));
            }
            return;
        }
        if ty != "response_item" {
            return;
        }
        let Some(t) = ts(&v) else { return };
        let pty = pl.get("type").and_then(|x| x.as_str()).unwrap_or("");
        match pty {
            "message" => {
                let role = pl.get("role").and_then(|x| x.as_str()).unwrap_or("");
                let text = text_of(pl.get("content").unwrap_or(&Value::Null));
                let text = text.trim();
                if text.is_empty() || text.starts_with('<') || text.starts_with("# AGENTS.md") {
                    return;
                }
                match role {
                    "user" => {
                        if self.title.is_empty() {
                            self.title = one_line(text, usize::MAX);
                        }
                        self.lane.events.push(message_event(t, Kind::User, self.names.get("User"), text));
                    }
                    "assistant" => self.lane.events.push(message_event(t, Kind::Assistant, self.names.get("Assistant"), text)),
                    _ => {}
                }
            }
            "function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call" => {
                let name = pl.get("name").and_then(|x| x.as_str()).unwrap_or(pty);
                let raw = pl
                    .get("arguments")
                    .or(pl.get("input"))
                    .or(pl.get("action"))
                    .unwrap_or(&Value::Null);
                // arguments は JSON 文字列のことが多い
                let decoded = raw
                    .as_str()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .filter(Value::is_object);
                let input = decoded.as_ref().unwrap_or(raw);
                let call_id = pl.get("call_id").and_then(|x| x.as_str());
                if let Some(c) = call_id {
                    self.pending.0.insert(c.to_owned(), self.lane.events.len());
                }
                let (display_name, summary, formatted) = codex_tool(name, input);
                let mut ev = tool_event(t, self.names.get(&display_name), call_id, summary, formatted);
                // exec の中の名前が Agent でも、元の呼び出しが exec ならツールのままにする。
                ev.kind = if is_agent_tool(name) { Kind::Agent } else { Kind::Tool };
                self.lane.events.push(ev);
            }
            "function_call_output" | "custom_tool_call_output" => {
                let c = pl.get("call_id").and_then(|x| x.as_str()).unwrap_or("");
                let (out, status) = {
                    let raw = codex_output_text(pl.get("output"));
                    let (out, status) = codex_output(&raw);
                    // 整形結果が独立していれば、次のJSON整形より前に巨大な結合本文を解放する。
                    let out = match out {
                        Cow::Owned(text) => Cow::Owned(text),
                        Cow::Borrowed(_) => raw,
                    };
                    (out, status)
                };
                self.pending.finish(&mut self.lane.events, c, t, &out, status);
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Option<Loaded> {
        if self.hidden { return None; }
    self.lane.finish();
    if self.lane.events.is_empty() {
        return None;
    }
    if !self.unassigned.is_empty() {
        self.lane.events[0].usage = add_usage(self.lane.events[0].usage, self.unassigned);
        self.lane.events[0].model = self.model;
    }
    if let Some(parent) = self.parent {
        return Some(Loaded::Child(parent, self.lane));
    }
    Some(Loaded::Session(Session {
        id: self.id,
        source: Source::Codex,
        cwd: self.cwd,
        headless: self.headless,
        title: self.title,
        branch: self.branch,
        version: self.version,
        lanes: vec![self.lane],
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    }))

    }
}

fn load_codex(p: &Path) -> Option<Loaded> {
    let mut parser = CodexParser::new();
    let mut lines = LogLines::open(p, skip_codex_line);
    while let Some(line) = lines.next_line() {
        parser.push(line);
        if parser.hidden { break; }
    }
    parser.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    struct DiscoveryFixture(PathBuf);

    impl DiscoveryFixture {
        fn new() -> Self {
            let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
            let root = std::env::temp_dir().join(format!("rireki-discovery-{}-{nonce}", std::process::id()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn file(&self, path: &str, size: usize, seconds: u64) -> PathBuf {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, vec![b'x'; size]).unwrap();
            fs::OpenOptions::new().write(true).open(&path).unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)).unwrap();
            path
        }

        fn scan(&self) -> Vec<Job> {
            discover_in(&self.0.join("claude"), &self.0.join("codex"))
        }
    }

    impl Drop for DiscoveryFixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn parsed_event_names_share_storage_and_preserve_display_names() {
        let fixture = DiscoveryFixture::new();
        let claude = fixture.file("claude.jsonl", 0, 1000);
        let content = serde_json::json!([
            {"type":"text", "text":"first"}, {"type":"text", "text":"second"},
            {"type":"thinking", "thinking":"first"}, {"type":"thinking", "thinking":"second"},
            {"type":"tool_use", "name":"独自ツール", "input":{}},
            {"type":"tool_use", "name":"独自ツール", "input":{}},
            {"type":"tool_use", "name":"Agent", "input":{}}
        ]);
        fs::write(&claude, serde_json::json!({"timestamp":"2026-10-05T00:00:00Z",
            "type":"assistant", "message":{"content":content}}).to_string()).unwrap();
        let lane = parse_claude_file(&claude, "main").lane;
        assert_eq!(lane.events.iter().map(|e| e.name.as_ref()).collect::<Vec<_>>(),
            ["Assistant", "Assistant", "Thinking", "Thinking", "独自ツール", "独自ツール", "Agent"]);
        for (a, b) in [(0, 1), (2, 3), (4, 5)] {
            assert!(Arc::ptr_eq(&lane.events[a].name, &lane.events[b].name));
        }
        assert_eq!(lane.events[6].kind, Kind::Agent);

        let codex = fixture.file("codex.jsonl", 0, 1000);
        let payloads = [
            serde_json::json!({"type":"message", "role":"user", "content":[{"text":"first"}]}),
            serde_json::json!({"type":"message", "role":"user", "content":[{"text":"second"}]}),
            serde_json::json!({"type":"function_call", "name":"exec", "arguments":"await tools.exec_command({cmd: 'cargo test'});"}),
            serde_json::json!({"type":"function_call", "name":"exec", "arguments":"await tools.exec_command({cmd: 'cargo check'});"}),
            serde_json::json!({"type":"function_call", "name":"spawn_agent", "arguments":"{}"}),
            serde_json::json!({"type":"function_call", "name":"exec", "arguments":"await tools.spawn_agent({task_name: 'test'});"})
        ];
        let lines = payloads.into_iter().map(|payload| serde_json::json!({
            "timestamp":"2026-10-05T00:00:00Z", "type":"response_item", "payload":payload
        }).to_string()).collect::<Vec<_>>().join("\n");
        fs::write(&codex, lines).unwrap();
        let Loaded::Session(session) = load_codex(&codex).unwrap() else { panic!("session expected") };
        let events = &session.lanes[0].events;
        assert_eq!(events.iter().map(|e| e.name.as_ref()).collect::<Vec<_>>(),
            ["User", "User", "exec_command", "exec_command", "spawn_agent", "spawn_agent"]);
        assert!(Arc::ptr_eq(&events[0].name, &events[1].name));
        assert!(Arc::ptr_eq(&events[2].name, &events[3].name));
        assert_eq!(events[4].kind, Kind::Agent);
        assert_eq!(events[5].kind, Kind::Tool);
        assert!(Arc::ptr_eq(&events[4].name, &events[5].name));
        let mut cloned = events[2].clone_meta();
        assert!(Arc::ptr_eq(&cloned.name, &events[2].name));
        cloned.name = "replacement".into();
        assert_eq!(events[2].name.as_ref(), "exec_command");
    }

    #[test]
    fn discovery_keeps_fingerprints_subagent_order_and_detects_changes() {
        let fixture = DiscoveryFixture::new();
        fixture.file("claude/project/one.jsonl", 1, 1000);
        fixture.file("claude/project/one/subagents/z.jsonl", 3, 1020);
        fixture.file("claude/project/one/subagents/a.jsonl", 2, 1010);
        fixture.file("claude/project/one/subagents/note.txt", 4, 1030);
        fixture.file("claude/project/agent-old.jsonl", 5, 1040);
        fixture.file("claude/project/agent-old/subagents/ignored.jsonl", 6, 9999);
        fixture.file("codex/2026/10/05/a.jsonl", 7, 1100);
        fixture.file("codex/2026/10/05/b.jsonl", 8, 1200);
        fixture.file("codex/ignored.txt", 9, 9999);
        let jobs = fixture.scan();
        let records: Vec<_> = jobs.iter().map(|j| (j.path.strip_prefix(&fixture.0).unwrap().components()
            .map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"), j.kind, j.mtime, j.fingerprint.as_str())).collect();
        assert_eq!(records, vec![
            ("codex/2026/10/05/b.jsonl".into(), JobKind::Codex, 1200.0, "1200.000:8;"),
            ("codex/2026/10/05/a.jsonl".into(), JobKind::Codex, 1100.0, "1100.000:7;"),
            ("claude/project/agent-old.jsonl".into(), JobKind::ClaudeLegacyAgent, 1040.0, "1040.000:5;"),
            ("claude/project/one.jsonl".into(), JobKind::Claude, 1030.0, "1000.000:1;1010.000:2;1030.000:4;1020.000:3;"),
        ]);
        fixture.file("claude/project/one/subagents/b.jsonl", 10, 1050);
        fixture.file("codex/2026/10/05/a.jsonl", 11, 1150);
        fs::rename(fixture.0.join("codex/2026/10/05/b.jsonl"), fixture.0.join("codex/2026/10/05/b.jsonl.bak")).unwrap();
        let jobs = fixture.scan();
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[0].fingerprint, "1150.000:11;");
        let parent = jobs.iter().find(|j| j.kind == JobKind::Claude).unwrap();
        assert_eq!(parent.mtime, 1050.0);
        assert_eq!(parent.fingerprint, "1000.000:1;1010.000:2;1050.000:10;1030.000:4;1020.000:3;");
        fs::remove_file(fixture.0.join("claude/project/one/subagents/a.jsonl")).unwrap();
        assert_eq!(fixture.scan().iter().find(|j| j.kind == JobKind::Claude).unwrap().fingerprint,
            "1000.000:1;1050.000:10;1030.000:4;1020.000:3;");
        assert!(discover_in(&fixture.0.join("missing-a"), &fixture.0.join("missing-b")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_follows_file_and_directory_links_and_keeps_missing_log_behavior() {
        use std::os::unix::fs::symlink;
        let fixture = DiscoveryFixture::new();
        let log = fixture.file("codex/year/day/log.jsonl", 5, 1000);
        symlink(&log, fixture.0.join("codex/linked.jsonl")).unwrap();
        symlink(fixture.0.join("codex/year"), fixture.0.join("codex/alias")).unwrap();
        symlink(fixture.0.join("absent"), fixture.0.join("codex/missing.jsonl")).unwrap();
        let mut jobs = fixture.scan();
        jobs.sort_by(|a, b| a.path.cmp(&b.path));
        let records: Vec<_> = jobs.iter().map(|j| (j.path.strip_prefix(fixture.0.join("codex")).unwrap().to_string_lossy().into_owned(), j.mtime, j.fingerprint.as_str())).collect();
        assert_eq!(records, vec![
            ("alias/day/log.jsonl".into(), 1000.0, "1000.000:5;"),
            ("linked.jsonl".into(), 1000.0, "1000.000:5;"),
            ("missing.jsonl".into(), 0.0, ""),
            ("year/day/log.jsonl".into(), 1000.0, "1000.000:5;"),
        ]);
        assert!(jobs.iter().all(|j| j.kind == JobKind::Codex));
    }

    #[test]
    fn claude_result_status() {
        let item = |err: bool| serde_json::json!({"type": "tool_result", "is_error": err});
        assert_eq!(claude_status(&item(false), None, "ok"), Status::Ok);
        assert_eq!(claude_status(&item(true), None, "Exit code 1"), Status::Error);
        let rejected = serde_json::json!("User rejected tool use");
        assert_eq!(claude_status(&item(true), Some(&rejected), "The user doesn't want to proceed"), Status::Rejected);
        assert_eq!(claude_status(&item(true), None, "[Request interrupted by user for tool use]"), Status::Interrupted);
        let interrupted = serde_json::json!({"interrupted": true, "stdout": ""});
        assert_eq!(claude_status(&item(false), Some(&interrupted), ""), Status::Interrupted);
        // 失敗でなければ、出力に中断の文言があっても成功
        assert_eq!(claude_status(&item(false), None, "[Request interrupted by user]"), Status::Ok);
    }


}
