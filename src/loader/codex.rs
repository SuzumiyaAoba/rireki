//! Codex のログ解析とセッションの構築。
use super::codex_row;

use super::{
    Arc, Cow, HashMap, Interner, Kind, Lane, Loaded, LogLines, Path, Pending, Session, Source,
    Status, Usage, Value, codex_output, codex_output_text, codex_tool, codex_usage, is_agent_tool,
    message_event, one_line, text_of, tool_event, ts,
};

// ---------------------------------------------------------------- Codex

/// JSON パース前に読み飛ばす行の種類
static SKIP: std::sync::LazyLock<[memchr::memmem::Finder<'static>; 4]> =
    std::sync::LazyLock::new(|| {
        [
            "\"type\":\"event_msg\"",
            "\"type\":\"turn_context\"",
            "\"type\":\"reasoning\"",
            "\"type\":\"world_state\"",
        ]
        .map(memchr::memmem::Finder::new)
    });

/// event_msg のうち読むもの（小さい行だけ）: ゴールの設定、ターンの中断
static KEEP_EVENTS: std::sync::LazyLock<[memchr::memmem::Finder<'static>; 2]> =
    std::sync::LazyLock::new(|| {
        [
            "\"type\":\"thread_goal_updated\"",
            "\"type\":\"turn_aborted\"",
        ]
        .map(memchr::memmem::Finder::new)
    });

pub(super) fn skip_codex_line(line: &[u8]) -> bool {
    let head = &line[..line.len().min(160)];
    let ctx = SKIP[1].find(head).is_some();
    let tc = memchr::memmem::find(head, b"\"type\":\"token_count\"").is_some();
    let keep = SKIP[0].find(head).is_some() && KEEP_EVENTS.iter().any(|f| f.find(head).is_some());
    !ctx && !tc && !keep && SKIP.iter().any(|f| f.find(head).is_some())
}

#[derive(Clone)]
pub(super) struct CodexParser {
    pub(super) lane: Lane,
    pub(super) pending: Pending,
    pub(super) names: Interner,
    pub(super) id: String,
    pub(super) cwd: String,
    pub(super) title: String,
    pub(super) parent: Option<String>,
    pub(super) headless: bool,
    pub(super) branch: String,
    pub(super) version: String,
    pub(super) goal: String,
    pub(super) total: Usage,
    pub(super) unassigned: Usage,
    pub(super) model: Option<Arc<str>>,
    pub(super) hidden: bool,
}

impl CodexParser {
    pub(super) fn new() -> Self {
        Self {
            lane: Lane::new("main"),
            pending: Pending(HashMap::new(), None),
            names: Interner::default(),
            id: String::new(),
            cwd: String::new(),
            title: String::new(),
            parent: None,
            headless: false,
            branch: String::new(),
            version: String::new(),
            goal: String::new(),
            total: Usage::default(),
            unassigned: Usage::default(),
            model: None,
            hidden: false,
        }
    }

    pub(super) fn push(&mut self, line: &[u8]) {
        if self.hidden {
            return;
        }
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
                let now = info
                    .get("total_token_usage")
                    .map(codex_usage)
                    .unwrap_or_default();
                // 累計が減ったら（圧縮などで数え直し）直近の分だけを使う
                let d = if now.total() >= self.total.total() {
                    now.saturating_sub(self.total)
                } else {
                    info.get("last_token_usage")
                        .map(codex_usage)
                        .unwrap_or_default()
                };
                self.total = now;
                self.unassigned = self.unassigned.saturating_add(d);
                if let Some(e) = self.lane.events.last_mut() {
                    e.usage = e.usage.saturating_add(std::mem::take(&mut self.unassigned));
                    e.model = self.model.clone();
                }
            }
            return;
        }
        if ty == "turn_context" {
            let str_at = |k: &str| {
                pl.pointer(k)
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            let m = pl
                .pointer("/model")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
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
                    let obj = pl
                        .pointer("/goal/objective")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim();
                    if let Some(t) = ts(&v).filter(|_| !obj.is_empty() && obj != self.goal) {
                        self.goal = obj.to_string();
                        if self.title.is_empty() {
                            self.title = one_line(obj, usize::MAX);
                        }
                        self.lane.events.push(message_event(
                            t,
                            Kind::User,
                            self.names.get("User"),
                            obj,
                        ));
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
            self.id = pl
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .into();
            self.cwd = pl
                .get("cwd")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .into();
            self.headless = pl.get("source").and_then(|x| x.as_str()) == Some("exec");
            self.version = pl
                .get("cli_version")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .into();
            self.branch = pl
                .pointer("/git/branch")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .into();
            if let Some(sub) = pl.pointer("/source/subagent") {
                let Some(spawn) = sub.get("thread_spawn") else {
                    self.hidden = true;
                    return; // guardian などの内部サブエージェントは表示しない
                };
                self.parent = spawn
                    .get("parent_thread_id")
                    .and_then(|x| x.as_str())
                    .map(String::from);
                let nick = spawn
                    .get("agent_nickname")
                    .and_then(|x| x.as_str())
                    .unwrap_or("agent");
                let path = spawn
                    .get("agent_path")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
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
                        self.lane.events.push(message_event(
                            t,
                            Kind::User,
                            self.names.get("User"),
                            text,
                        ));
                    }
                    "assistant" => self.lane.events.push(message_event(
                        t,
                        Kind::Assistant,
                        self.names.get("Assistant"),
                        text,
                    )),
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
                let mut ev = tool_event(
                    t,
                    self.names.get(&display_name),
                    call_id,
                    summary,
                    formatted,
                );
                // exec の中の名前が Agent でも、元の呼び出しが exec ならツールのままにする。
                ev.kind = if is_agent_tool(name) {
                    Kind::Agent
                } else {
                    Kind::Tool
                };
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
                self.pending
                    .finish(&mut self.lane.events, c, t, &out, status);
            }
            _ => {}
        }
    }

    pub(super) fn finish(mut self) -> Option<Loaded> {
        if self.hidden {
            return None;
        }
        self.lane.finish();
        if self.lane.events.is_empty() {
            return None;
        }
        if !self.unassigned.is_empty() {
            self.lane.events[0].usage = self.lane.events[0].usage.saturating_add(self.unassigned);
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

pub(super) fn load_codex(p: &Path) -> Option<Loaded> {
    let mut parser = CodexParser::new();
    let mut lines = LogLines::open(p, skip_codex_line);
    while let Some(line) = lines.next_line() {
        parser.push(line);
        if parser.hidden {
            break;
        }
    }
    parser.finish()
}
