//! Claude Code / Codex のセッションログ (JSONL) を読み込む

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rayon::prelude::*;
use serde_json::Value;

use crate::model::{Event, Kind, Lane, Loaded, Session, Source, one_line, truncate};

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

struct Stamp {
    mtime: f64,
    fp: String,
}

impl Stamp {
    fn new() -> Self {
        Self { mtime: 0.0, fp: String::new() }
    }

    fn push(&mut self, p: &Path) {
        let Ok(m) = fs::metadata(p) else { return };
        let t = m.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok());
        let t = t.map(|d| d.as_secs_f64()).unwrap_or(0.0);
        self.mtime = self.mtime.max(t);
        self.fp.push_str(&format!("{t:.3}:{};", m.len()));
    }
}

/// 読み込み対象のファイルを新しい順に列挙する
pub fn discover() -> Vec<Job> {
    let mut jobs: Vec<Job> = Vec::new();
    let home = dirs::home_dir().unwrap_or_default();

    let claude = home.join(".claude/projects");
    for proj in read_dir(&claude) {
        for f in read_dir(&proj) {
            if !f.extension().is_some_and(|e| e == "jsonl") {
                continue;
            }
            let legacy = f.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("agent-"));
            let mut st = Stamp::new();
            st.push(&f);
            let kind = if legacy {
                JobKind::ClaudeLegacyAgent
            } else {
                let mut subs = read_dir(&f.with_extension("").join("subagents"));
                subs.sort();
                for s in &subs {
                    st.push(s);
                }
                JobKind::Claude
            };
            jobs.push(Job { kind, path: f, mtime: st.mtime, fingerprint: st.fp });
        }
    }

    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or(home.join(".codex")).join("sessions");
    let mut stack = vec![codex];
    while let Some(dir) = stack.pop() {
        for p in read_dir(&dir) {
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "jsonl") {
                let mut st = Stamp::new();
                st.push(&p);
                jobs.push(Job { kind: JobKind::Codex, path: p, mtime: st.mtime, fingerprint: st.fp });
            }
        }
    }

    jobs.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
    jobs
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

fn read_dir(p: &Path) -> Vec<PathBuf> {
    fs::read_dir(p).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default()
}

/// ファイル全体を一度に読む。行ごとの String 確保と UTF-8 検証を避け、行はバイト列のまま JSON パーサに渡す
fn read_all(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap_or_default()
}

fn lines(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    memchr::memchr_iter(b'\n', buf)
        .chain(std::iter::once(buf.len()))
        .scan(0, |start, end| {
            let line = &buf[*start..end];
            *start = end + 1;
            Some(line)
        })
        .filter(|l| !l.is_empty())
}

fn ts(v: &Value) -> Option<f64> {
    let s = v.get("timestamp")?.as_str()?;
    let t = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(t.timestamp_millis() as f64 / 1000.0)
}

fn pretty(v: &Value) -> String {
    match v {
        Value::String(s) => truncate(s, MAX_TEXT),
        other => truncate(&yaml(other).join("\n"), MAX_TEXT),
    }
}

/// JSON を YAML 風の読みやすい行に整形する（文字列は引用符を外し、複数行はそのまま字下げする）
fn yaml(v: &Value) -> Vec<String> {
    match v {
        Value::Object(m) if !m.is_empty() => m.iter().flat_map(|(k, v)| entry(k, v)).collect(),
        Value::Array(a) if !a.is_empty() => {
            if let Some(s) = inline_list(a) {
                return vec![s];
            }
            a.iter()
                .flat_map(|x| {
                    let mut ls = yaml(x).into_iter();
                    let first = ls.next().unwrap_or_default();
                    std::iter::once(format!("- {first}")).chain(ls.map(|l| format!("  {l}")))
                })
                .collect()
        }
        other => vec![scalar(other)],
    }
}

fn entry(k: &str, v: &Value) -> Vec<String> {
    let nested = |ls: Vec<String>| std::iter::once(format!("{k}:")).chain(ls.into_iter().map(|l| format!("  {l}"))).collect();
    match v {
        Value::String(s) if s.contains('\n') => nested(s.lines().map(String::from).collect()),
        Value::Object(m) if !m.is_empty() => nested(yaml(v)),
        Value::Array(a) if !a.is_empty() => match inline_list(a) {
            Some(s) => vec![format!("{k}: {s}")],
            None => nested(yaml(v)),
        },
        other => vec![format!("{k}: {}", scalar(other))],
    }
}

/// 短いスカラーだけの配列は 1 行にまとめる
fn inline_list(a: &[Value]) -> Option<String> {
    if a.iter().any(|x| x.is_object() || x.is_array() || x.as_str().is_some_and(|s| s.contains('\n'))) {
        return None;
    }
    let s = format!("[{}]", a.iter().map(scalar).collect::<Vec<_>>().join(", "));
    (s.chars().count() <= 80).then_some(s)
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) if s.is_empty() => "\"\"".into(),
        Value::String(s) => s.clone(),
        Value::Object(_) => "{}".into(),
        Value::Array(_) => "[]".into(),
        other => other.to_string(),
    }
}

/// 結果が JSON なら整形する
fn pretty_output(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('{') || t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            return yaml(&v).join("\n");
        }
    }
    // Codex の "[exit 0 · 0.1s]" 見出しの後ろが JSON の場合
    if let Some((head, body)) = s.split_once('\n').filter(|(h, _)| h.starts_with('[') && h.ends_with(']')) {
        let b = pretty_output(body);
        if b != body {
            return format!("{head}\n{b}");
        }
    }
    s.to_string()
}

/// ツール入力から一行の要約を作る
fn summarize_input(name: &str, input: &Value) -> String {
    const KEYS: &[&str] =
        &["description", "file_path", "path", "command", "cmd", "pattern", "url", "query", "prompt", "skill", "plan"];
    if let Value::Object(map) = input {
        for k in KEYS {
            if let Some(v) = map.get(*k) {
                let s = match v {
                    Value::String(s) => s.clone(),
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" "),
                    other => other.to_string(),
                };
                if !s.is_empty() {
                    return one_line(&s, 160);
                }
            }
        }
        return one_line(&input.to_string(), 160);
    }
    if let Value::String(s) = input {
        // apply_patch などはファイル名だけ拾う
        if name == "apply_patch" {
            let files: Vec<_> = s
                .lines()
                .filter_map(|l| l.strip_prefix("*** ").and_then(|l| l.split_once(": ")).map(|x| x.1))
                .collect();
            if !files.is_empty() {
                return one_line(&files.join(", "), 160);
            }
        }
        return one_line(s, 160);
    }
    String::new()
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            items.iter().filter_map(|i| i.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

fn is_agent_tool(name: &str) -> bool {
    matches!(name, "Agent" | "Task" | "spawn_agent")
}

fn message_event(t: f64, kind: Kind, name: &str, text: &str) -> Event {
    Event {
        start: t,
        end: t,
        kind,
        name: name.into(),
        summary: one_line(text, 160),
        input: truncate(text, MAX_TEXT),
        output: String::new(),
        call_id: None,
        row: None,
    }
}

fn tool_event(t: f64, name: &str, input: &Value, call_id: Option<String>) -> Event {
    Event {
        start: t,
        end: t,
        kind: if is_agent_tool(name) { Kind::Agent } else { Kind::Tool },
        name: name.into(),
        summary: summarize_input(name, input),
        input: pretty(input),
        output: String::new(),
        call_id,
        row: None,
    }
}

/// tool_result を対応する呼び出しに反映する
struct Pending(HashMap<String, usize>);

impl Pending {
    fn finish(&mut self, events: &mut [Event], id: &str, t: f64, output: String) {
        if let Some(i) = self.0.remove(id) {
            let e = &mut events[i];
            e.end = t.max(e.start);
            e.output = truncate(&pretty_output(&output), MAX_TEXT);
        }
    }
}

// ---------------------------------------------------------------- Claude

struct ClaudeParse {
    lane: Lane,
    cwd: String,
    title: String,
    session_id: String,
    headless: bool,
}

fn parse_claude_file(p: &Path, lane_name: &str) -> ClaudeParse {
    let mut lane = Lane::new(lane_name);
    let mut pending = Pending(HashMap::new());
    let mut cwd = String::new();
    let mut title = String::new();
    let mut session_id = String::new();
    let mut headless = false;

    let buf = read_all(p);
    for line in lines(&buf) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if cwd.is_empty() {
            if let Some(c) = v.get("cwd").and_then(|c| c.as_str()) {
                cwd = c.into();
            }
        }
        if let Some(e) = v.get("entrypoint").and_then(|e| e.as_str()) {
            headless |= e.starts_with("sdk");
        }
        if session_id.is_empty() {
            if let Some(c) = v.get("sessionId").and_then(|c| c.as_str()) {
                session_id = c.into();
            }
        }
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty == "ai-title" || ty == "summary" {
            if let Some(s) = v.get("aiTitle").or(v.get("summary")).and_then(|s| s.as_str()) {
                title = s.into();
            }
        }
        if ty == "assistant" {
            if lane.model.is_empty()
                && let Some(m) = v.pointer("/message/model").and_then(|m| m.as_str()).filter(|m| !m.starts_with('<'))
            {
                lane.model = m.to_string();
            }
            if lane.effort.is_empty()
                && let Some(e) = v.get("effort").and_then(|e| e.as_str())
            {
                lane.effort = e.to_string();
            }
        }
        let Some(t) = ts(&v) else { continue };
        let Some(content) = v.pointer("/message/content") else {
            continue;
        };
        match ty {
            "user" => {
                if v.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
                    continue;
                }
                if let Value::Array(items) = content {
                    for it in items {
                        if it.get("type").and_then(|x| x.as_str()) == Some("tool_result") {
                            let id = it.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                            let out = text_of(it.get("content").unwrap_or(&Value::Null));
                            pending.finish(&mut lane.events, id, t, out);
                        }
                    }
                }
                let text = text_of(content);
                let text = text.trim();
                if !text.is_empty() && !text.starts_with('<') {
                    if title.is_empty() {
                        title = one_line(text, 80);
                    }
                    lane.events.push(message_event(t, Kind::User, "User", text));
                }
            }
            "assistant" => {
                let Value::Array(items) = content else {
                    continue;
                };
                for it in items {
                    match it.get("type").and_then(|x| x.as_str()) {
                        Some("text") => {
                            let text = it.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                lane.events.push(message_event(t, Kind::Assistant, "Assistant", text));
                            }
                        }
                        Some("thinking") => {
                            let text = it.get("thinking").and_then(|x| x.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                lane.events.push(message_event(t, Kind::Thinking, "Thinking", text));
                            }
                        }
                        Some("tool_use") => {
                            let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                            let id = it.get("id").and_then(|x| x.as_str()).map(String::from);
                            let input = it.get("input").cloned().unwrap_or(Value::Null);
                            if let Some(id) = &id {
                                pending.0.insert(id.clone(), lane.events.len());
                            }
                            lane.events.push(tool_event(t, name, &input, id));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    lane.finish();
    ClaudeParse { lane, cwd, title, session_id, headless }
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
        id: parsed.session_id.is_empty().then(|| id.clone()).unwrap_or(parsed.session_id),
        source: Source::Claude,
        cwd: parsed.cwd,
        headless: parsed.headless,
        title: parsed.title,
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

fn load_codex(p: &Path) -> Option<Loaded> {
    let mut lane = Lane::new("main");
    let mut pending = Pending(HashMap::new());
    let mut id = String::new();
    let mut cwd = String::new();
    let mut title = String::new();
    let mut parent: Option<String> = None;
    let mut headless = false;

    let buf = read_all(p);
    for line in lines(&buf) {
        // 巨大で不要な行は JSON パース前に捨てる
        let head = &line[..line.len().min(160)];
        // turn_context はモデルとエフォートを拾うため最初の 1 行だけ読む
        let ctx = SKIP[1].find(head).is_some() && lane.model.is_empty();
        if !ctx && SKIP.iter().any(|f| f.find(head).is_some()) {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let Some(pl) = v.get("payload") else { continue };

        if ty == "turn_context" {
            let str_at = |k: &str| pl.pointer(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
            lane.model = str_at("/model");
            lane.effort = str_at("/effort");
            if lane.effort.is_empty() {
                lane.effort = str_at("/collaboration_mode/settings/reasoning_effort");
            }
            continue;
        }
        if ty == "session_meta" {
            id = pl.get("id").and_then(|x| x.as_str()).unwrap_or_default().into();
            cwd = pl.get("cwd").and_then(|x| x.as_str()).unwrap_or_default().into();
            headless = pl.get("source").and_then(|x| x.as_str()) == Some("exec");
            if let Some(sub) = pl.pointer("/source/subagent") {
                let Some(spawn) = sub.get("thread_spawn") else {
                    return None; // guardian などの内部サブエージェントは表示しない
                };
                parent = spawn.get("parent_thread_id").and_then(|x| x.as_str()).map(String::from);
                let nick = spawn.get("agent_nickname").and_then(|x| x.as_str()).unwrap_or("agent");
                let path = spawn.get("agent_path").and_then(|x| x.as_str()).unwrap_or("");
                lane.name = format!("{nick}: {}", path.rsplit('/').next().unwrap_or(path));
            }
            continue;
        }
        if ty != "response_item" {
            continue;
        }
        let Some(t) = ts(&v) else { continue };
        let pty = pl.get("type").and_then(|x| x.as_str()).unwrap_or("");
        match pty {
            "message" => {
                let role = pl.get("role").and_then(|x| x.as_str()).unwrap_or("");
                let text = text_of(pl.get("content").unwrap_or(&Value::Null));
                let text = text.trim();
                if text.is_empty() || text.starts_with('<') || text.starts_with("# AGENTS.md") {
                    continue;
                }
                match role {
                    "user" => {
                        if title.is_empty() {
                            title = one_line(text, 80);
                        }
                        lane.events.push(message_event(t, Kind::User, "User", text));
                    }
                    "assistant" => lane.events.push(message_event(t, Kind::Assistant, "Assistant", text)),
                    _ => {}
                }
            }
            "function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call" => {
                let name = pl.get("name").and_then(|x| x.as_str()).unwrap_or(pty);
                let raw = pl.get("arguments").or(pl.get("input")).or(pl.get("action")).cloned().unwrap_or(Value::Null);
                // arguments は JSON 文字列のことが多い
                let input = match &raw {
                    Value::String(s) => serde_json::from_str::<Value>(s).ok().filter(|v| v.is_object()).unwrap_or(raw),
                    _ => raw,
                };
                let call_id = pl.get("call_id").and_then(|x| x.as_str()).map(String::from);
                if let Some(c) = &call_id {
                    pending.0.insert(c.clone(), lane.events.len());
                }
                let mut ev = tool_event(t, name, &input, call_id);
                (ev.name, ev.summary, ev.input) = codex_tool(name, &input);
                lane.events.push(ev);
            }
            "function_call_output" | "custom_tool_call_output" => {
                let c = pl.get("call_id").and_then(|x| x.as_str()).unwrap_or("");
                let out = match pl.get("output") {
                    Some(Value::String(s)) => s.clone(),
                    Some(o) => o.get("content").map(text_of).unwrap_or_else(|| o.to_string()),
                    None => String::new(),
                };
                pending.finish(&mut lane.events, c, t, codex_output(&out));
            }
            _ => {}
        }
    }
    lane.finish();
    if lane.events.is_empty() {
        return None;
    }
    if let Some(parent) = parent {
        return Some(Loaded::Child(parent, lane));
    }
    Some(Loaded::Session(Session {
        id,
        source: Source::Codex,
        cwd,
        headless,
        title,
        lanes: vec![lane],
        start: 0.0,
        end: 0.0,
        parents: Vec::new(),
        tree: Vec::new(),
    }))
}

/// exec の入力として保存する JS の目印。詳細表示時に `exec_detail` で整形する
pub const JS_MARK: &str = "\u{1}exec\n";
/// 保存する JS の上限（切れると構文エラーになり、静的解析にフォールバックする）
const MAX_JS: usize = 50_000;

/// 詳細表示用の exec の整形。モック実行で呼び出しが取れればそれを、取れなければ静的解析の結果を使う
pub fn exec_detail(js: &str) -> String {
    if let Some(calls) = crate::jsmock::run(js) {
        let args: Vec<(&str, Value)> = calls
            .iter()
            .map(|c| {
                let v = if c.args.is_object() { c.args.clone() } else { serde_json::json!({ "input": c.args }) };
                (c.name.as_str(), v)
            })
            .collect();
        let items: Vec<Item> = args
            .iter()
            .zip(&calls)
            .map(|((n, v), c)| Item {
                name: n,
                label: c.label.clone().or_else(|| c.at.and_then(|at| label_near(js, at))),
                args: Box::new(JsonArgs(v)),
                raw: pretty(v),
            })
            .collect();
        let body = format_calls(&items);
        return truncate(&format!("（JS をモック実行して推定: {} 件）\n\n{body}", items.len()), MAX_TEXT);
    }
    let calls = js_tool_calls(js);
    if calls.is_empty() {
        return truncate(js, MAX_TEXT);
    }
    let items: Vec<Item> = calls
        .iter()
        .map(|(n, a)| Item {
            name: n,
            label: call_label(js, n),
            args: Box::new(JsArgs(a)),
            raw: a.trim_matches(|c| c == '(' || c == ')').to_string(),
        })
        .collect();
    if items.len() == 1 && codex_format(items[0].name, &*items[0].args).1.is_empty() {
        return truncate(js, MAX_TEXT);
    }
    truncate(&format_calls(&items), MAX_TEXT)
}

/// 詳細表示用の 1 呼び出し
struct Item<'a> {
    name: &'a str,
    /// 呼び出しに付いた名前（代入先の変数・オブジェクトのキーなど）
    label: Option<String>,
    args: Box<dyn Args + 'a>,
    /// 整形できないときの表示
    raw: String,
}

fn format_calls(items: &[Item]) -> String {
    let body = |it: &Item| {
        let d = codex_format(it.name, &*it.args).1;
        if d.is_empty() { it.raw.clone() } else { d }
    };
    if let [it] = items {
        return match &it.label {
            Some(l) => format!("{l} ← {}\n{}", it.name, body(it)),
            None => body(it),
        };
    }
    let mut detail = String::new();
    for (i, it) in items.iter().enumerate() {
        let head = match &it.label {
            Some(l) => format!("#{i} {l} ← {}", it.name),
            None => format!("#{i} {}", it.name),
        };
        detail.push_str(&format!("{head}\n{}\n\n", body(it)));
    }
    detail.trim_end().to_string()
}

/// 静的解析で見つけた呼び出し（`js_tool_calls` が返したツール名のスライス）の名前
fn call_label(js: &str, name: &str) -> Option<String> {
    let at = (name.as_ptr() as usize).checked_sub(js.as_ptr() as usize)?.checked_sub(6)?;
    label_before(js, at)
}

/// モック実行で得た位置（`tools.xxx(` の周辺）から呼び出しの名前を探す
fn label_near(js: &str, at: usize) -> Option<String> {
    let mut end = (at + 6).min(js.len());
    while !js.is_char_boundary(end) {
        end -= 1;
    }
    let line_start = js[..at.min(end)].rfind('\n').map_or(0, |i| i + 1);
    let i = js[line_start..end].rfind("tools.")? + line_start;
    label_before(js, i)
}

/// `tools.` の直前を見て、呼び出しに付いた名前を返す
///
/// - `const result = await tools.x(...)` → `result`
/// - `{ build: tools.x(...) }` → `build`
/// - `["lint", tools.x(...)]` → `lint`
fn label_before(js: &str, at: usize) -> Option<String> {
    let mut p = js[..at].trim_end();
    if let Some(q) = p.strip_suffix("await").filter(|q| !q.ends_with(is_ident)) {
        p = q.trim_end();
    }
    let ident = |s: &str| {
        let s = s.trim_end();
        let n = s.chars().rev().take_while(|&c| is_ident(c)).count();
        let id = &s[s.len() - s.chars().rev().take(n).map(char::len_utf8).sum::<usize>()..];
        (!id.is_empty() && !id.starts_with(|c: char| c.is_ascii_digit())).then(|| id.to_string())
    };
    if let Some(q) = p.strip_suffix('=') {
        if q.ends_with(['=', '!', '<', '>']) {
            return None;
        }
        return ident(q.trim_end_matches(['+', '-', '*', '/', '|', '&', '?']));
    }
    if let Some(q) = p.strip_suffix(':') {
        let q = q.trim_end();
        if let Some(&qc @ (b'"' | b'\'' | b'`')) = q.as_bytes().last() {
            let open = q[..q.len() - 1].rfind(qc as char)?;
            return Some(q[open + 1..q.len() - 1].to_string()).filter(|s| !s.is_empty());
        }
        return ident(q);
    }
    if let Some(q) = p.strip_suffix(',') {
        let q = q.trim_end();
        let &qc @ (b'"' | b'\'' | b'`') = q.as_bytes().last()? else { return None };
        let open = q[..q.len() - 1].rfind(qc as char)?;
        if q[..open].trim_end().ends_with('[') {
            return Some(q[open + 1..q.len() - 1].to_string()).filter(|s| !s.is_empty() && s.len() <= 60);
        }
    }
    None
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Codex のツール呼び出しを表示用に整える: (名前, 要約, 入力表示)
///
/// コードモードの `exec` は JS で `tools.xxx({...})` を呼ぶので、中身のツールを取り出す。
fn codex_tool(name: &str, input: &Value) -> (String, String, String) {
    if let ("exec", Value::String(js)) = (name, input) {
        // 名前と要約は静的解析で決め、詳細は表示時にモック実行で作る（exec_detail）
        let calls = js_tool_calls(js);
        let stored = format!("{JS_MARK}{}", truncate(js, MAX_JS));
        return match calls.as_slice() {
            [] => (name.into(), summarize_input(name, input), stored),
            [(inner, args)] => (inner.to_string(), codex_format(inner, &JsArgs(args)).0, stored),
            many => {
                let sums: Vec<String> = many
                    .iter()
                    .map(|(inner, args)| {
                        let s = codex_format(inner, &JsArgs(args)).0;
                        let s = if s.is_empty() { inner.to_string() } else { s };
                        match call_label(js, inner) {
                            Some(l) => format!("{l}: {s}"),
                            None => s,
                        }
                    })
                    .collect();
                let first = many[0].0;
                let n = if many.iter().all(|(x, _)| *x == first) { first } else { "exec" };
                let s = format!("[{}] {}", many.len(), sums.join(" ; "));
                (n.into(), one_line(&s, 160), stored)
            }
        };
    }
    if let ("apply_patch", Value::String(p)) = (name, input) {
        return (name.into(), patch_summary(p), truncate(p, MAX_TEXT));
    }
    let (s, d) = match input {
        Value::Object(_) => codex_format(name, &JsonArgs(input)),
        _ => (String::new(), String::new()),
    };
    let s = if s.is_empty() { summarize_input(name, input) } else { s };
    let d = if d.is_empty() { pretty(input) } else { d };
    (name.into(), s, d)
}

/// JSON / JS オブジェクトリテラルの両方から引数を取り出す
trait Args {
    fn str(&self, key: &str) -> Option<String>;
    /// 位置引数の先頭の文字列 (`tools.apply_patch("...")`)
    fn first(&self) -> Option<String> {
        None
    }
    /// update_plan の (step, status) 列
    fn plan(&self) -> Vec<(String, String)>;
}

struct JsonArgs<'a>(&'a Value);

impl Args for JsonArgs<'_> {
    fn str(&self, key: &str) -> Option<String> {
        match self.0.get(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")),
            Value::Null => None,
            v => Some(v.to_string()),
        }
    }
    fn plan(&self) -> Vec<(String, String)> {
        let Some(Value::Array(a)) = self.0.get("plan") else { return Vec::new() };
        a.iter()
            .map(|p| {
                let f = |k| p.get(k).and_then(|x: &Value| x.as_str()).unwrap_or("").to_string();
                (f("step"), f("status"))
            })
            .collect()
    }
}

struct JsArgs<'a>(&'a str);

impl Args for JsArgs<'_> {
    fn str(&self, key: &str) -> Option<String> {
        js_fields(self.0, key).into_iter().next()
    }
    fn first(&self) -> Option<String> {
        let s = self.0.strip_prefix('(')?.trim_start();
        let q = *s.as_bytes().first()?;
        matches!(q, b'"' | b'\'' | b'`').then(|| js_unquote(js_balanced_str(s, q)))
    }
    fn plan(&self) -> Vec<(String, String)> {
        let steps = js_fields(self.0, "step");
        let mut st = js_fields(self.0, "status").into_iter();
        steps.into_iter().map(|s| (s, st.next().unwrap_or_default())).collect()
    }
}

/// JS ソース中の `tools.name(args)` 呼び出しを列挙する
fn js_tool_calls(js: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = js;
    while let Some(i) = rest.find("tools.") {
        rest = &rest[i + 6..];
        let n = rest.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
        if n == 0 || rest.as_bytes().get(n) != Some(&b'(') {
            continue;
        }
        let name = &rest[..n];
        let args = js_balanced(&rest[n..]);
        out.push((name, args));
        rest = &rest[n + args.len()..];
    }
    out
}

/// 先頭の括弧に対応する閉じ括弧までを返す（文字列リテラル内は無視）
fn js_balanced(s: &str) -> &str {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut esc = false;
    for (i, b) in s.bytes().enumerate() {
        if let Some(q) = quote {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' => quote = Some(b),
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return &s[..=i];
                }
            }
            _ => {}
        }
    }
    s
}

/// `key: "..."` / `"key": "..."` / `key: 123` の値をすべて取り出す
fn js_fields(src: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(i) = src[from..].find(key).map(|i| i + from) {
        from = i + key.len();
        let prev = if i > 0 { b[i - 1] } else { b'{' };
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            continue;
        }
        let mut j = from;
        if b.get(j) == Some(&b'"') && prev == b'"' {
            j += 1;
        } else if prev == b'"' {
            continue;
        }
        while b.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
            j += 1;
        }
        if b.get(j) != Some(&b':') {
            continue;
        }
        j += 1;
        while b.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
            j += 1;
        }
        match b.get(j) {
            Some(&q @ (b'"' | b'\'' | b'`')) => {
                let lit = js_balanced_str(&src[j..], q);
                out.push(js_unquote(lit));
                from = j + lit.len();
            }
            Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b't' || *c == b'f' => {
                let n = src[j..].bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'-').count();
                out.push(src[j..j + n].to_string());
            }
            _ => {}
        }
    }
    out
}

fn js_balanced_str(s: &str, q: u8) -> &str {
    let mut esc = false;
    for (i, b) in s.bytes().enumerate().skip(1) {
        if esc {
            esc = false;
        } else if b == b'\\' {
            esc = true;
        } else if b == q {
            return &s[..=i];
        }
    }
    s
}

/// JS の文字列リテラルを復号する（JSON 互換のエスケープを想定）
fn js_unquote(lit: &str) -> String {
    let inner = lit.get(1..lit.len().saturating_sub(1)).unwrap_or("");
    let quoted = if lit.starts_with('"') {
        lit.to_string()
    } else {
        format!("\"{}\"", inner.replace("\\'", "'").replace("\\`", "`").replace('"', "\\\"").replace('\n', "\\n"))
    };
    serde_json::from_str::<String>(&quoted).unwrap_or_else(|_| inner.to_string())
}

/// ツールごとの (要約, 入力表示)。空文字は既定の表示に任せる
fn codex_format(name: &str, a: &dyn Args) -> (String, String) {
    match name {
        "exec_command" | "shell" | "local_shell" => {
            let Some(cmd) = a.str("cmd").or_else(|| a.str("command")) else { return Default::default() };
            let mut d = format!("$ {cmd}");
            if let Some(w) = a.str("workdir") {
                d.push_str(&format!("\n\ncwd: {w}"));
            }
            (one_line(&cmd, 160), truncate(&d, MAX_TEXT))
        }
        "write_stdin" => {
            let id = a.str("session_id").unwrap_or_default();
            let chars = a.str("chars").unwrap_or_default();
            let s = if chars.is_empty() { format!("session {id} の出力を待機") } else { format!("session {id} へ入力") };
            (s.clone(), if chars.is_empty() { s } else { format!("{s}\n\n{chars}") })
        }
        "apply_patch" => {
            let Some(p) = a.first().or_else(|| a.str("input")) else { return Default::default() };
            (patch_summary(&p), truncate(&p, MAX_TEXT))
        }
        "wait" => {
            let s = format!("cell {} の完了を待機", a.str("cell_id").unwrap_or_default());
            (s.clone(), s)
        }
        "update_plan" => {
            let plan = a.plan();
            let mut d = a.str("explanation").map(|e| format!("{e}\n\n")).unwrap_or_default();
            for (step, st) in &plan {
                let mark = match st.as_str() {
                    "completed" => "✓",
                    "in_progress" => "▶",
                    _ => "○",
                };
                d.push_str(&format!("{mark} {step}\n"));
            }
            let done = plan.iter().filter(|p| p.1 == "completed").count();
            let cur = plan.iter().find(|p| p.1 == "in_progress").map(|p| p.0.as_str()).unwrap_or("");
            (one_line(&format!("{done}/{} {cur}", plan.len()), 160), d.trim_end().to_string())
        }
        _ => Default::default(),
    }
}

/// パッチの対象ファイルを「Update a.rs, Add b.rs」の形で並べる
fn patch_summary(p: &str) -> String {
    let files: Vec<String> = p
        .lines()
        .filter_map(|l| {
            let r = l.strip_prefix("*** ")?;
            let (op, path) = r.split_once(" File: ")?;
            let base = path.trim().rsplit('/').next().unwrap_or(path);
            Some(format!("{op} {base}"))
        })
        .collect();
    one_line(&files.join(", "), 160)
}

/// コードモードの `text()` が出力した JSON 1 行を読みやすくする
///
/// `{"i":0,"status":"fulfilled","value":{exit_code, output, ...}}`（Promise.allSettled の結果）や
/// `{"exit_code":0,"output":"..."}`（ツールの戻り値そのまま）を見出し + 本文にする。
fn codex_js_line(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line.trim()).ok().filter(|v: &Value| v.is_object())?;
    let idx = v.get("i").map(|i| format!("#{i} "));
    let (val, rejected) = match v.get("status").and_then(|s| s.as_str()) {
        Some("rejected") => (v.get("reason")?, true),
        Some(_) => (v.get("value")?, false),
        None => (&v, false),
    };
    let idx = idx.unwrap_or_default();
    if rejected {
        let r = val.as_str().map(String::from).unwrap_or_else(|| val.to_string());
        return Some(format!("── {idx}rejected\n{r}"));
    }
    let out = val.get("output")?.as_str()?;
    let mut meta = Vec::new();
    if let Some(c) = val.get("exit_code").filter(|c| !c.is_null()) {
        meta.push(format!("exit {c}"));
    } else if let Some(id) = val.get("session_id").filter(|c| !c.is_null()) {
        meta.push(format!("running · session {id}"));
    }
    if let Some(w) = val.get("wall_time_seconds").and_then(|w| w.as_f64()) {
        meta.push(format!("{w:.1}s"));
    }
    Some(format!("── {idx}[{}]\n{}", meta.join(" · "), out.trim_end()))
}

/// Codex のツール出力から定型ヘッダ (Wall time など) を取り除き、終了コードと所要時間を 1 行にまとめる
fn codex_output(s: &str) -> String {
    let Some(i) = s.find("Output:\n").filter(|&i| i < 400) else { return s.to_string() };
    let (mut exit, mut wall) = (None, None);
    for l in s[..i].lines() {
        let l = l.trim();
        if let Some(c) = l.strip_prefix("Process exited with code ") {
            exit = Some(c.to_string());
        } else if let Some(w) = l.strip_prefix("Wall time") {
            wall = Some(w.trim_start_matches(':').trim().replace(" seconds", "s").replace(" second", "s"));
        } else if !(l.is_empty()
            || l.starts_with("Chunk ID")
            || l.starts_with("Original token count")
            || l.starts_with("Script ")
            || l.starts_with("Process running"))
        {
            return s.to_string();
        }
    }
    let body = &s[i + 8..];
    let lines: Vec<String> = body.lines().map(|l| codex_js_line(l).unwrap_or_else(|| l.to_string())).collect();
    let body = &lines.join("\n");
    let meta: Vec<String> = exit.map(|c| format!("exit {c}")).into_iter().chain(wall).collect();
    if meta.is_empty() { body.to_string() } else { format!("[{}]\n{body}", meta.join(" · ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 保存された JS を詳細表示用に整形し、モック実行の見出しを除く
    fn detail(d: &str) -> String {
        let d = exec_detail(d.strip_prefix(JS_MARK).expect("exec の入力は JS のまま保存する"));
        d.split_once("\n\n").filter(|(h, _)| h.starts_with("（JS をモック")).map_or(d.clone(), |(_, b)| b.to_string())
    }

    #[test]
    fn codex_labels() {
        let js = Value::String(
            "const build = await tools.exec_command({cmd:\"make\"});\nconst r = await Promise.allSettled([\n [\"lint\", tools.exec_command({cmd:\"ls\"})],\n tools.exec_command({cmd:\"pwd\"})]);\nconst o = { test: tools.exec_command({cmd:\"t\"}) };".into(),
        );
        let (_, s, d) = codex_tool("exec", &js);
        assert_eq!(s, "[4] build: make ; lint: ls ; pwd ; test: t");
        assert_eq!(
            detail(&d),
            "#0 build ← exec_command\n$ make\n\n#1 lint ← exec_command\n$ ls\n\n#2 exec_command\n$ pwd\n\n#3 test ← exec_command\n$ t"
        );
        let js = Value::String("for (const f of [\"a\"]) { const out = await tools.exec_command({cmd: `cat ${f}`}); }".into());
        let (_, _, d) = codex_tool("exec", &js);
        assert_eq!(detail(&d), "out ← exec_command\n$ cat a");
        // 配列の要素の 0 番目が名前
        let js = Value::String(
            "const commands=[[\"cpu\",\"sysctl\"],[\"os\",\"sw_vers\"]];\nconst rs=await Promise.allSettled(commands.map(async ([name,cmd])=>({name,...await tools.exec_command({cmd})})));\nfor (const [n, c] of [[\"x\", \"ls\"]]) await tools.exec_command({cmd: c});\nawait tools.exec_command({cmd: \"pwd\"});".into(),
        );
        let (_, _, d) = codex_tool("exec", &js);
        assert_eq!(
            detail(&d),
            "#0 cpu ← exec_command\n$ sysctl\n\n#1 os ← exec_command\n$ sw_vers\n\n#2 x ← exec_command\n$ ls\n\n#3 exec_command\n$ pwd"
        );
    }

    #[test]
    fn codex_mock_loop() {
        let js = Value::String("for (const f of [\"a\", \"b\"]) await tools.exec_command({cmd: `cat ${f}`});".into());
        let (_, _, d) = codex_tool("exec", &js);
        assert_eq!(detail(&d), "#0 exec_command\n$ cat a\n\n#1 exec_command\n$ cat b");
    }

    #[test]
    fn codex_exec_js() {
        let js = Value::String(
            "const r=await tools.exec_command({cmd:\"sed -n '1,2p' a.py\",workdir:\"/w\",yield_time_ms:10000}); text(r.output);".into(),
        );
        let (n, s, d) = codex_tool("exec", &js);
        assert_eq!(n, "exec_command");
        assert_eq!(s, "sed -n '1,2p' a.py");
        assert_eq!(detail(&d), "r ← exec_command\n$ sed -n '1,2p' a.py\n\ncwd: /w");
    }

    #[test]
    fn codex_plan_js() {
        let js = Value::String(
            "await tools.update_plan({plan:[\n {step:\"A\",status:\"completed\"},\n {step:\"B\",status:\"in_progress\"}]})".into(),
        );
        let (n, s, d) = codex_tool("exec", &js);
        assert_eq!(n, "update_plan");
        assert_eq!(s, "1/2 B");
        assert_eq!(detail(&d), "✓ A\n▶ B");
    }

    #[test]
    fn codex_output_header() {
        assert_eq!(
            pretty(&serde_json::json!({"a": 1, "b": "x\ny", "c": [1, 2], "d": {"e": null}, "f": [{"g": true, "h": "i"}]})),
            "a: 1\nb:\n  x\n  y\nc: [1, 2]\nd:\n  e: null\nf:\n  - g: true\n    h: i"
        );
        assert_eq!(pretty_output("[exit 0]\n{\"ok\":true}"), "[exit 0]\nok: true");
        assert_eq!(codex_output("Script completed\nWall time 0.2 seconds\nOutput:\nhello"), "[0.2s]\nhello");
        assert_eq!(
            codex_output("Chunk ID: x\nWall time: 0.1 seconds\nProcess exited with code 1\nOriginal token count: 3\nOutput:\nerr"),
            "[exit 1 · 0.1s]\nerr"
        );
    }

    #[test]
    fn codex_multi_js() {
        let js = Value::String(
            "const r=await Promise.allSettled([tools.exec_command({cmd:\"ls\"}),tools.exec_command({cmd:\"pwd\"})]);".into(),
        );
        let (n, s, d) = codex_tool("exec", &js);
        assert_eq!(n, "exec_command");
        assert_eq!(s, "[2] ls ; pwd");
        assert_eq!(detail(&d), "#0 exec_command\n$ ls\n\n#1 exec_command\n$ pwd");
        let js = Value::String("text(await tools.apply_patch(\"*** Begin Patch\\n*** Update File: /a/b.rs\\n@@\\n\"))".into());
        let (n, s, _) = codex_tool("exec", &js);
        assert_eq!((n.as_str(), s.as_str()), ("apply_patch", "Update b.rs"));
        assert_eq!(
            codex_output("Script completed\nWall time 0.2 seconds\nOutput:\n{\"i\":0,\"status\":\"fulfilled\",\"value\":{\"exit_code\":0,\"wall_time_seconds\":0.01,\"output\":\"hi\\n\"}}"),
            "[0.2s]\n── #0 [exit 0 · 0.0s]\nhi"
        );
    }
}
