//! Claude Code のログ解析とセッションの構築。

use super::{
    Arc, HashMap, Interner, Kind, Lane, Loaded, LogLines, Path, PathBuf, Pending, Session, Source,
    Usage, Value, claude_input, claude_patch, claude_status, claude_usage, fs, json_row,
    message_event, one_line, read_dir, summarize_input, text_of, tool_event, ts,
};

pub(super) struct ClaudeParse {
    pub(super) lane: Lane,
    pub(super) cwd: String,
    pub(super) title: String,
    pub(super) session_id: String,
    pub(super) headless: bool,
    pub(super) branch: String,
    pub(super) version: String,
}

#[derive(Clone)]
pub(super) struct ClaudeParser {
    pub(super) lane: Lane,
    pub(super) pending: Pending,
    pub(super) names: Interner,
    pub(super) cwd: String,
    pub(super) title: String,
    pub(super) session_id: String,
    pub(super) headless: bool,
    pub(super) branch: String,
    pub(super) version: String,
    pub(super) usages: Vec<(usize, Usage, Option<Arc<str>>)>,
    pub(super) models: Interner,
    pub(super) usage_ix: HashMap<String, usize>,
}

impl ClaudeParser {
    pub(super) fn new(name: &str) -> Self {
        Self {
            lane: Lane::new(name),
            pending: Pending(HashMap::new(), None),
            names: Interner::default(),
            cwd: String::new(),
            title: String::new(),
            session_id: String::new(),
            headless: false,
            branch: String::new(),
            version: String::new(),
            usages: Vec::new(),
            models: Interner::default(),
            usage_ix: HashMap::new(),
        }
    }

    pub(super) fn push(&mut self, line: &[u8]) {
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
        if let Some(b) = v
            .get("gitBranch")
            .and_then(|b| b.as_str())
            .filter(|b| !b.is_empty() && *b != "HEAD")
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
                && let Some(m) = v
                    .pointer("/message/model")
                    .and_then(|m| m.as_str())
                    .filter(|m| !m.starts_with('<'))
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
                        let out = raw
                            .strip_prefix("<tool_use_error>")
                            .map(|e| e.strip_suffix("</tool_use_error>").unwrap_or(e))
                            .unwrap_or(&raw);
                        let Some(i) =
                            self.pending
                                .finish(&mut self.lane.events, id, t, out, status)
                        else {
                            continue;
                        };
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
                    self.lane.events.push(message_event(
                        t,
                        Kind::User,
                        self.names.get("User"),
                        text,
                    ));
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
                                self.lane.events.push(message_event(
                                    t,
                                    Kind::Assistant,
                                    self.names.get("Assistant"),
                                    text,
                                ));
                            }
                        }
                        Some("thinking") => {
                            let text = it.get("thinking").and_then(|x| x.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                self.lane.events.push(message_event(
                                    t,
                                    Kind::Thinking,
                                    self.names.get("Thinking"),
                                    text,
                                ));
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
                    let id = v
                        .pointer("/message/id")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default();
                    // そのメッセージの最初のイベント。イベントを生まない行なら直前のイベント
                    let at = first.min(self.lane.events.len().saturating_sub(1));
                    let model = v
                        .pointer("/message/model")
                        .and_then(|m| m.as_str())
                        .filter(|m| !m.starts_with('<'))
                        .map(|m| self.models.get(m));
                    match self.usage_ix.get(id) {
                        Some(&k) if !id.is_empty() => {
                            (self.usages[k].1, self.usages[k].2) = (u, model)
                        }
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

    pub(super) fn finish(mut self) -> ClaudeParse {
        // 同じ message.id の使用量は最後の値を一度だけ計上する。
        // 生のイベント列には加算せず、公開する版にだけ反映する。
        for (i, u, m) in self.usages {
            if let Some(e) = self.lane.events.get_mut(i) {
                e.usage = e.usage.saturating_add(u);
                e.model = m.or(e.model.take());
            }
        }
        self.lane.finish();
        ClaudeParse {
            lane: self.lane,
            cwd: self.cwd,
            title: self.title,
            session_id: self.session_id,
            headless: self.headless,
            branch: self.branch,
            version: self.version,
        }
    }
}

pub(super) fn parse_claude_file(p: &Path, lane_name: &str) -> ClaudeParse {
    let mut parser = ClaudeParser::new(lane_name);
    let mut lines = LogLines::open(p, |_| false);
    while let Some(line) = lines.next_line() {
        parser.push(line);
    }
    parser.finish()
}

pub(super) fn load_claude(p: &Path) -> Vec<Loaded> {
    let parsed = parse_claude_file(p, "main");
    let id = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let mut lanes = vec![parsed.lane];

    // 新形式: <session>/subagents/agent-*.jsonl (+ .meta.json)
    let sub_dir = p.with_extension("").join("subagents");
    let mut subs: Vec<PathBuf> = read_dir(&sub_dir)
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    subs.sort();
    for f in subs {
        let meta: Value = fs::read_to_string(f.with_extension("meta.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        let ty = meta
            .get("agentType")
            .and_then(|x| x.as_str())
            .unwrap_or("agent");
        let desc = meta
            .get("description")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let mut sub = parse_claude_file(&f, &format!("{ty}: {desc}")).lane;
        sub.spawned_by = meta
            .get("toolUseId")
            .and_then(|x| x.as_str())
            .map(String::from);
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

pub(super) fn load_claude_legacy_agent(p: &Path) -> Option<Loaded> {
    let name = p.file_stem()?.to_str()?.to_string();
    let parsed = parse_claude_file(p, &name);
    if parsed.session_id.is_empty() {
        return None;
    }
    Some(Loaded::Child(parsed.session_id, parsed.lane))
}
