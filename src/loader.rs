//! Claude Code / Codex のセッションログ (JSONL) を読み込む

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use rayon::prelude::*;
use serde_json::Value;

use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Status, Usage, one_line, truncate};

mod codex_row;
mod live;
pub use live::{LiveLog, LiveStats};

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
        write!(&mut self.fp, "{t:.3}:{};", m.len()).unwrap();
    }
}

/// 読み込み対象のファイルを新しい順に列挙する
pub fn discover() -> Vec<Job> {
    let home = dirs::home_dir().unwrap_or_default();
    let claude = home.join(".claude/projects");
    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or(home.join(".codex")).join("sessions");
    discover_in(&claude, &codex)
}

/// 計測とテストでは実際のホームを変更せず、同じ形式のルートを指定する
pub fn discover_in(claude: &Path, codex: &Path) -> Vec<Job> {
    let mut jobs: Vec<Job> = Vec::new();
    for proj in read_dir(claude) {
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

    let mut stack = vec![codex.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue; };
        for entry in entries.flatten() {
            let p = entry.path();
            // 列挙時の種類を使い、ログごとの is_dir の stat を省く。リンクは従来どおり辿る
            let directory = entry.file_type().map(|kind| {
                if kind.is_symlink() { p.is_dir() } else { kind.is_dir() }
            }).unwrap_or_else(|_| p.is_dir());
            if directory {
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

/// ファイル全体をバイト列で読み、行ごとの String 確保を避ける。
fn read_all(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap_or_default()
}

/// 行のUTF-8をまとめて検証し、JSON文字列ごとの再検証を省く。
/// 不正な入力では従来のデシリアライザを使い、エラーの扱いも維持する。
fn json_row(line: &[u8]) -> serde_json::Result<Value> {
    match simdutf8::basic::from_utf8(line) {
        Ok(text) => serde_json::from_str(text),
        Err(_) => serde_json::from_slice(line),
    }
}

#[cfg(test)]
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

/// 大きなログは一定量ずつ読み、不要な巨大行は先頭だけ見て読み捨てる。
/// 必要な行をまたぐ場合だけ、再利用するバッファに連結する
struct BufferedLines<R> {
    reader: R,
    line: Vec<u8>,
    skip: fn(&[u8]) -> bool,
    position: u64,
    last_complete: u64,
}

impl<R: BufRead> BufferedLines<R> {
    fn next_record(&mut self) -> Option<(&[u8], bool)> {
        self.next_line()?;
        Some((&self.line, self.last_complete == self.position))
    }

    fn next_line(&mut self) -> Option<&[u8]> {
        'line: loop {
            self.line.clear();
            let mut chunks = Vec::new();
            let mut decided = false;
            let mut skipping = false;
            loop {
                let buf = self.reader.fill_buf().ok()?;
                if buf.is_empty() {
                    finish_line(&mut self.line, chunks);
                    return (!skipping && !self.line.is_empty() && !(self.skip)(&self.line))
                        .then_some(self.line.as_slice());
                }
                let end = memchr::memchr(b'\n', buf).unwrap_or(buf.len());
                let complete = end < buf.len();
                if !skipping {
                    let mut copied = 0;
                    if !decided {
                        copied = end.min(160 - self.line.len());
                        self.line.extend_from_slice(&buf[..copied]);
                        if self.line.len() == 160 || complete {
                            skipping = (self.skip)(&self.line);
                            decided = true;
                        }
                    }
                    if !skipping {
                        append_line(&mut self.line, &mut chunks, &buf[copied..end]);
                    }
                }
                let consumed = end + usize::from(complete);
                self.reader.consume(consumed);
                self.position += consumed as u64;
                if complete { self.last_complete = self.position; }
                if complete {
                    finish_line(&mut self.line, chunks);
                    if skipping || self.line.is_empty() {
                        continue 'line;
                    }
                    return Some(&self.line);
                }
            }
        }
    }
}

/// 巨大な行の再確保では古い領域と新しい領域が同時に残る。
/// 容量を超えて2 MiBより大きくなる行だけ分割し、長さが分かってから一度確保する。
fn append_line(line: &mut Vec<u8>, chunks: &mut Vec<Vec<u8>>, bytes: &[u8]) {
    if chunks.is_empty() && (line.len() + bytes.len() <= line.capacity() || line.len() + bytes.len() <= 2 * 1024 * 1024) {
        line.extend_from_slice(bytes);
    } else {
        if chunks.is_empty() { chunks.push(std::mem::take(line)); }
        chunks.push(bytes.to_vec());
    }
}

fn finish_line(line: &mut Vec<u8>, chunks: Vec<Vec<u8>>) {
    if chunks.is_empty() { return; }
    let len = chunks.iter().map(Vec::len).sum();
    line.reserve_exact(len);
    for chunk in chunks { line.extend_from_slice(&chunk); }
}

enum LogLines {
    Memory { bytes: Vec<u8>, offset: usize },
    Buffered(BufferedLines<BufReader<fs::File>>),
}

impl LogLines {
    fn open(p: &Path, skip: fn(&[u8]) -> bool) -> Self {
        if fs::metadata(p).is_ok_and(|m| m.len() > 8 * 1024 * 1024)
            && let Ok(file) = fs::File::open(p)
        {
            Self::Buffered(BufferedLines {
                reader: BufReader::with_capacity(256 * 1024, file),
                line: Vec::new(),
                skip, position: 0, last_complete: 0,
            })
        } else {
            Self::Memory {
                bytes: read_all(p),
                offset: 0,
            }
        }
    }

    fn next_line(&mut self) -> Option<&[u8]> {
        match self {
            Self::Memory { bytes, offset } => loop {
                if *offset >= bytes.len() {
                    return None;
                }
                let end =
                    memchr::memchr(b'\n', &bytes[*offset..]).map_or(bytes.len(), |i| i + *offset);
                let start = *offset;
                *offset = end + 1;
                if start < end {
                    return Some(&bytes[start..end]);
                }
            },
            Self::Buffered(reader) => reader.next_line(),
        }
    }
}

fn ts(v: &Value) -> Option<f64> {
    let s = v.get("timestamp")?.as_str()?;
    let t = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(t.timestamp_millis() as f64 / 1000.0)
}

fn pretty(v: &Value) -> String {
    match v {
        Value::String(s) => truncate(s, MAX_TEXT),
        other => bounded_yaml(other, MAX_TEXT),
    }
}

/// JSON を YAML 風の読みやすい行に整形する（文字列は引用符を外し、複数行はそのまま字下げする）
#[cfg(test)]
fn yaml(v: &Value) -> Vec<String> {
    yaml_lines(v, usize::MAX).collect()
}

fn yaml_lines(v: &Value, line_limit: usize) -> Box<dyn Iterator<Item = String> + '_> {
    match v {
        Value::Object(m) if !m.is_empty() => Box::new(m.iter().flat_map(move |(k, v)| entry_lines(k, v, line_limit))),
        Value::Array(a) if !a.is_empty() => {
            if let Some(s) = inline_list(a) {
                return Box::new(std::iter::once(s));
            }
            Box::new(a.iter()
                .flat_map(move |x| {
                    let mut ls = yaml_lines(x, line_limit);
                    let first = ls.next().unwrap_or_default();
                    std::iter::once(format!("- {first}")).chain(ls.map(|l| format!("  {l}")))
                }))
        }
        other => Box::new(std::iter::once(scalar_prefix(other, line_limit))),
    }
}

fn entry(k: &str, v: &Value) -> Vec<String> {
    entry_lines(k, v, usize::MAX).collect()
}

fn entry_lines<'a>(k: &'a str, v: &'a Value, line_limit: usize) -> Box<dyn Iterator<Item = String> + 'a> {
    let k = text_prefix(k, line_limit);
    let nested = |ls: Box<dyn Iterator<Item = String> + 'a>| -> Box<dyn Iterator<Item = String> + 'a> {
        Box::new(std::iter::once(format!("{k}:")).chain(ls.map(|l| format!("  {l}"))))
    };
    match v {
        Value::String(s) if s.contains('\n') => nested(Box::new(s.lines().map(move |line| text_prefix(line, line_limit).to_owned()))),
        Value::Object(m) if !m.is_empty() => nested(yaml_lines(v, line_limit)),
        Value::Array(a) if !a.is_empty() => match inline_list(a) {
            Some(s) => Box::new(std::iter::once(format!("{k}: {s}"))),
            None => nested(yaml_lines(v, line_limit)),
        },
        other => Box::new(std::iter::once(format!("{k}: {}", scalar_prefix(other, line_limit)))),
    }
}

fn text_prefix(text: &str, limit: usize) -> &str {
    if text.len() <= limit { text } else {
        let end = text.char_indices().nth(limit).map_or(text.len(), |(i, _)| i);
        &text[..end]
    }
}

fn scalar_prefix(value: &Value, limit: usize) -> String {
    match value {
        Value::String(text) if !text.is_empty() => text_prefix(text, limit).to_owned(),
        _ => scalar(value),
    }
}

/// 表示上限の一文字先まで生成し、元の切り詰めと同じ位置に省略記号を付ける。
fn bounded_yaml(v: &Value, limit: usize) -> String {
    let mut out = String::new();
    let mut chars = 0;
    for (i, line) in yaml_lines(v, limit.saturating_add(1)).enumerate() {
        if i != 0 {
            out.push('\n');
            chars += 1;
            if chars > limit { return truncate(&out, limit); }
        }
        for c in line.chars() {
            out.push(c);
            chars += 1;
            if chars > limit { return truncate(&out, limit); }
        }
    }
    out
}

/// 短いスカラーだけの配列は 1 行にまとめる
fn inline_list(a: &[Value]) -> Option<String> {
    let mut out = String::from("[");
    let mut chars = 1;
    for (i, value) in a.iter().enumerate() {
        if value.is_object() || value.is_array() { return None; }
        if i != 0 {
            out.push_str(", ");
            chars += 2;
        }
        // 閉じ括弧を含めて80文字。長い配列や文字列を全て複製・連結せずに判定する。
        let remaining = 80usize.checked_sub(chars + 1)?;
        if let Some(text) = value.as_str()
            && (text.chars().take(remaining + 1).count() > remaining || text.contains('\n'))
        { return None; }
        let text = scalar(value);
        let count = text.chars().count();
        if count > remaining { return None; }
        chars += count;
        out.push_str(&text);
    }
    out.push(']');
    Some(out)
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
fn pretty_output(s: &str) -> Cow<'_, str> {
    let t = s.trim();
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return Cow::Owned(bounded_yaml(&v, MAX_TEXT));
    }
    // Codex の "[exit 0 · 0.1s]" 見出しの後ろが JSON の場合
    if let Some((head, body)) = s.split_once('\n').filter(|(h, _)| h.starts_with('[') && h.ends_with(']')) {
        let b = pretty_output(body);
        if let Cow::Owned(b) = b
            && b != body
        {
            return Cow::Owned(format!("{head}\n{b}"));
        }
    }
    Cow::Borrowed(s)
}

/// ツール入力から一行の要約を作る
fn summarize_input(name: &str, input: &Value) -> String {
    const KEYS: &[&str] =
        &["description", "file_path", "path", "command", "cmd", "pattern", "url", "query", "prompt", "skill", "plan"];
    if name == "TodoWrite" {
        let todos = todo_items(input);
        if !todos.is_empty() {
            return plan_summary(&todos);
        }
    }
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

fn text_of(content: &Value) -> Cow<'_, str> {
    match content {
        Value::String(s) => Cow::Borrowed(s),
        Value::Array(items) => {
            let mut parts = items.iter().filter_map(|i| i.get("text").and_then(|t| t.as_str()));
            let Some(first) = parts.next() else { return Cow::Borrowed("") };
            match parts.next() {
                None => Cow::Borrowed(first),
                Some(second) => Cow::Owned([first, second].into_iter().chain(parts).collect::<Vec<_>>().join("\n")),
            }
        }
        _ => Cow::Borrowed(""),
    }
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

/// シェルのコマンドの表示。`$ コマンド` の後に、その他の引数を YAML 風に添える
fn shell_text(cmd: &str, rest: &str) -> String {
    let mut d = format!("$ {cmd}");
    if !rest.is_empty() {
        d.push_str("\n\n");
        d.push_str(rest);
    }
    d
}

/// `keys` 以外の引数を YAML 風に並べる
fn yaml_except(input: &Value, keys: &[&str]) -> String {
    match input {
        Value::Object(m) => m
            .iter()
            .filter(|(k, _)| !keys.contains(&k.as_str()))
            .flat_map(|(k, v)| entry(k, v))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Claude のツール入力を表示用に整える。専用の形がないものは YAML 風にする
///
/// 編集系は差分（`*** Update File: パス` + `@@` 区切りの -/+ 行）にする。結果に行番号付きの差分が
/// あれば、結果を受け取ったときに置き換える（`claude_patch`）
fn claude_input(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str());
    let text = match name {
        "Bash" => s("command").map(|cmd| shell_text(cmd, &yaml_except(input, &["command", "description"]))),
        "Edit" => match (s("file_path"), s("old_string"), s("new_string")) {
            (Some(p), Some(a), Some(b)) => Some(format!("*** Update File: {p}\n{}", line_diff(a, b))),
            _ => None,
        },
        "MultiEdit" => s("file_path").zip(input.get("edits").and_then(|e| e.as_array())).map(|(p, edits)| {
            let hunks: Vec<String> = edits
                .iter()
                .map(|e| {
                    let f = |k| e.get(k).and_then(|v: &Value| v.as_str()).unwrap_or("");
                    line_diff(f("old_string"), f("new_string"))
                })
                .collect();
            format!("*** Update File: {p}\n{}", hunks.join("\n"))
        }),
        "Write" => s("file_path").zip(s("content")).map(|(p, c)| added_file(p, c)),
        "TodoWrite" => {
            let todos = todo_items(input);
            (!todos.is_empty()).then(|| checklist(&todos))
        }
        "ExitPlanMode" => s("plan").map(String::from),
        // サブエージェントへの指示は Markdown で読めるよう、その他の引数を前に置いて本文をそのまま残す
        "Agent" | "Task" => s("prompt").map(|p| {
            let rest = yaml_except(input, &["prompt"]);
            if rest.is_empty() { p.to_string() } else { format!("{rest}\n\n{p}") }
        }),
        _ => None,
    };
    match text {
        Some(t) => truncate(&t, MAX_TEXT),
        None => pretty(input),
    }
}

/// 新しく作るファイルの差分（全行を追加として並べる）
fn added_file(path: &str, content: &str) -> String {
    let mut d = format!("*** Add File: {path}");
    for l in content.lines() {
        d.push_str("\n+");
        d.push_str(l);
    }
    d
}

/// Claude の Edit / Write の結果（toolUseResult）から、行番号付きの差分を作る
fn claude_patch(r: &Value) -> Option<String> {
    let path = r.get("filePath")?.as_str()?;
    let hunks = r.get("structuredPatch")?.as_array().filter(|h| !h.is_empty())?;
    let mut d = format!("*** Update File: {path}");
    for h in hunks {
        let n = |k| h.get(k).and_then(|v: &Value| v.as_u64()).unwrap_or(0);
        d.push_str(&format!("\n@@ -{},{} +{},{} @@", n("oldStart"), n("oldLines"), n("newStart"), n("newLines")));
        for l in h.get("lines")?.as_array()? {
            d.push('\n');
            d.push_str(l.as_str().unwrap_or(""));
        }
    }
    Some(truncate(&d, MAX_TEXT))
}

/// 行単位の差分。`old` と `new` の全行を文脈として含め、`@@` で始める
fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut d = String::from("@@");
    for (mark, l) in diff_lines(&a, &b) {
        d.push('\n');
        d.push(mark);
        d.push_str(l);
    }
    d
}

/// 2 つの行列の差分を (' ' | '-' | '+', 行) の列で返す（最長共通部分列による）
fn diff_lines<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, &'a str)> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (a2, b2) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut out: Vec<(char, &str)> = a[..pre].iter().map(|l| (' ', *l)).collect();
    let (n, m) = (a2.len(), b2.len());
    if n * m > 1_000_000 {
        // 大きすぎるときは比べずに、消した行と足した行を並べる
        out.extend(a2.iter().map(|l| ('-', *l)));
        out.extend(b2.iter().map(|l| ('+', *l)));
    } else {
        // lcs[i * (m + 1) + j] = a2[i..] と b2[j..] の最長共通部分列の長さ
        let w = m + 1;
        let mut lcs = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * w + j] = if a2[i] == b2[j] {
                    lcs[(i + 1) * w + j + 1] + 1
                } else {
                    lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a2[i] == b2[j] {
                out.push((' ', a2[i]));
                i += 1;
                j += 1;
            } else if lcs[(i + 1) * w + j] >= lcs[i * w + j + 1] {
                out.push(('-', a2[i]));
                i += 1;
            } else {
                out.push(('+', b2[j]));
                j += 1;
            }
        }
        out.extend(a2[i..].iter().map(|l| ('-', *l)));
        out.extend(b2[j..].iter().map(|l| ('+', *l)));
    }
    out.extend(a[a.len() - suf..].iter().map(|l| (' ', *l)));
    out
}

/// TodoWrite の (内容, 状態) 列
fn todo_items(input: &Value) -> Vec<(String, String)> {
    let Some(Value::Array(a)) = input.get("todos") else { return Vec::new() };
    a.iter()
        .map(|t| {
            let f = |k| t.get(k).and_then(|x: &Value| x.as_str()).unwrap_or("").to_string();
            (f("content"), f("status"))
        })
        .collect()
}

/// 計画を「✓ 済み / ▶ 進行中 / ○ 未着手」の行で並べる
fn checklist(items: &[(String, String)]) -> String {
    items
        .iter()
        .map(|(step, st)| {
            let mark = match st.as_str() {
                "completed" => "✓",
                "in_progress" => "▶",
                _ => "○",
            };
            format!("{mark} {step}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 計画の要約（「済み数/全体 進行中の項目」）
fn plan_summary(items: &[(String, String)]) -> String {
    let done = items.iter().filter(|p| p.1 == "completed").count();
    let cur = items.iter().find(|p| p.1 == "in_progress").map(|p| p.0.as_str()).unwrap_or("");
    one_line(&format!("{done}/{} {cur}", items.len()), 160)
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

/// ツール出力の本文。新しい形式では `[{type: input_text, text}, ...]` に分かれている
fn codex_output_text(o: Option<&Value>) -> Cow<'_, str> {
    match o {
        Some(Value::String(s)) => Cow::Borrowed(s),
        Some(Value::Array(items)) => {
            let mut parts = items.iter().filter_map(|i| i.get("text").and_then(|t| t.as_str()));
            let Some(first) = parts.next() else { return Cow::Borrowed("") };
            match parts.next() {
                None => Cow::Borrowed(first),
                Some(second) => Cow::Owned([first, second].into_iter().chain(parts).collect()),
            }
        }
        Some(o) => o.get("content").map(text_of).unwrap_or_else(|| Cow::Owned(o.to_string())),
        None => Cow::Borrowed(""),
    }
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
        let &qc @ (b'"' | b'\'' | b'`') = q.as_bytes().last()? else {
            return None;
        };
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
        let Some(Value::Array(a)) = self.0.get("plan") else {
            return Vec::new();
        };
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
            let Some(cmd) = a.str("cmd").or_else(|| a.str("command")) else {
                return Default::default();
            };
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
            let Some(p) = a.first().or_else(|| a.str("input")) else {
                return Default::default();
            };
            (patch_summary(&p), truncate(&p, MAX_TEXT))
        }
        "wait" => {
            let s = format!("cell {} の完了を待機", a.str("cell_id").unwrap_or_default());
            (s.clone(), s)
        }
        "update_plan" => {
            let plan = a.plan();
            let mut d = a.str("explanation").map(|e| format!("{e}\n\n")).unwrap_or_default();
            d.push_str(&checklist(&plan));
            (plan_summary(&plan), d.trim_end().to_string())
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
            let base = crate::paths::basename(path.trim());
            Some(format!("{op} {base}"))
        })
        .collect();
    one_line(&files.join(", "), 160)
}

/// コードモードの `text()` が出力した JSON 1 行を読みやすくする。2 つ目は失敗（reject・0 以外の終了コード）か
///
/// `{"i":0,"status":"fulfilled","value":{exit_code, output, ...}}`（Promise.allSettled の結果）や
/// `{"exit_code":0,"output":"..."}`（ツールの戻り値そのまま）を見出し + 本文にする。
fn codex_js_line(line: &str) -> Option<(String, bool)> {
    // text() を続けて呼ぶと、改行なしで JSON が連なる（`{...}{"i":1,...}`）
    let line = line.trim();
    let mut it = serde_json::Deserializer::from_str(line).into_iter::<Value>();
    let (mut parts, mut failed, mut end) = (Vec::new(), false, 0);
    while let Some(Ok(v)) = it.next() {
        let (text, f) = codex_js_value(&v)?;
        parts.push(text);
        failed |= f;
        end = it.byte_offset();
    }
    if parts.is_empty() {
        return None;
    }
    if !line[end..].trim().is_empty() {
        parts.push(line[end..].trim().to_string());
    }
    Some((parts.join("\n"), failed))
}

fn codex_js_value(v: &Value) -> Option<(String, bool)> {
    if !v.is_object() {
        return None;
    }
    let idx = v.get("i").map(|i| format!("#{i} "));
    let (val, rejected) = match v.get("status").and_then(|s| s.as_str()) {
        Some("rejected") => (v.get("reason")?, true),
        Some(_) => (v.get("value")?, false),
        None => (v, false),
    };
    let idx = idx.unwrap_or_default();
    if rejected {
        let r = val.as_str().map(String::from).unwrap_or_else(|| val.to_string());
        return Some((format!("── {idx}rejected\n{r}"), true));
    }
    let out = val.get("output")?.as_str()?;
    let mut meta = Vec::new();
    let mut failed = false;
    if let Some(c) = val.get("exit_code").filter(|c| !c.is_null()) {
        failed = c.as_i64() != Some(0);
        meta.push(format!("exit {c}"));
    } else if let Some(id) = val.get("session_id").filter(|c| !c.is_null()) {
        meta.push(format!("running · session {id}"));
    }
    if let Some(w) = val.get("wall_time_seconds").and_then(|w| w.as_f64()) {
        meta.push(format!("{w:.1}s"));
    }
    Some((format!("── {idx}[{}]\n{}", meta.join(" · "), out.trim_end()), failed))
}

/// 出力の先頭に出る、失敗・中断を表す定型文
fn codex_failure(s: &str) -> Option<Status> {
    const ERRORS: &[&str] = &[
        "Script failed",
        "apply_patch verification failed",
        "failed to spawn",
        "execution error",
        "unsupported custom tool call",
    ];
    let mut end = s.len().min(200);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let head = &s[..end];
    if head.starts_with("aborted by user") || head.contains("\naborted by user") {
        Some(Status::Interrupted)
    } else if ERRORS.iter().any(|e| head.starts_with(e)) {
        Some(Status::Error)
    } else {
        None
    }
}

/// Codex のツール出力から定型ヘッダ (Wall time など) を取り除き、終了コードと所要時間を 1 行にまとめる。
/// あわせて、終了コードや定型文から呼び出しの成否を判定する
fn codex_output(s: &str) -> (Cow<'_, str>, Status) {
    let mut status = codex_failure(s).unwrap_or(Status::Ok);
    let fail = |status: &mut Status| {
        if *status == Status::Ok {
            *status = Status::Error;
        }
    };
    // 旧形式の `{"output": "...", "metadata": {"exit_code": 0, "duration_seconds": 0.1}}`
    if s.starts_with("{\"output\"")
        && let Ok(v) = serde_json::from_str::<Value>(s)
        && let Some(out) = v.get("output").and_then(|o| o.as_str())
    {
        let mut meta = Vec::new();
        if let Some(c) = v.pointer("/metadata/exit_code").and_then(|c| c.as_i64()) {
            if c != 0 {
                fail(&mut status);
            }
            meta.push(format!("exit {c}"));
        }
        if let Some(d) = v.pointer("/metadata/duration_seconds").and_then(|d| d.as_f64()) {
            meta.push(format!("{d:.1}s"));
        }
        let text = if meta.is_empty() { out.to_string() } else { format!("[{}]\n{out}", meta.join(" · ")) };
        return (Cow::Owned(text), status);
    }
    let Some(i) = s.find("Output:\n").filter(|&i| i < 400) else {
        return (Cow::Borrowed(s), status);
    };
    let (mut exit, mut wall) = (None, None);
    for l in s[..i].lines() {
        let l = l.trim();
        if let Some(c) = l.strip_prefix("Process exited with code ").or_else(|| l.strip_prefix("Exit code: ")) {
            exit = Some(c.to_string());
        } else if let Some(w) = l.strip_prefix("Wall time") {
            wall = Some(w.trim_start_matches(':').trim().replace(" seconds", "s").replace(" second", "s"));
        } else if !(l.is_empty()
            || l.starts_with("Chunk ID")
            || l.starts_with("Original token count")
            || l.starts_with("Script ")
            || l.starts_with("Process running"))
        {
            return (Cow::Borrowed(s), status);
        }
    }
    if exit.as_deref().is_some_and(|c| c.trim() != "0") {
        fail(&mut status);
    }
    let body = &s[i + 8..];
    let lines: Vec<String> = body
        .lines()
        .map(|l| match codex_js_line(l) {
            Some((text, failed)) => {
                if failed {
                    fail(&mut status);
                }
                text
            }
            None => l.to_string(),
        })
        .collect();
    let body = &lines.join("\n");
    let meta: Vec<String> = exit.map(|c| format!("exit {c}")).into_iter().chain(wall).collect();
    let text = if meta.is_empty() { body.to_string() } else { format!("[{}]\n{body}", meta.join(" · ")) };
    (Cow::Owned(text), status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rows_keep_values_key_order_and_errors_at_utf8_boundaries() {
        fn same(input: &[u8]) {
            match (serde_json::from_slice::<Value>(input), json_row(input)) {
                (Ok(old), Ok(new)) => {
                    assert_eq!(old, new);
                    assert_eq!(serde_json::to_vec(&old).unwrap(), serde_json::to_vec(&new).unwrap());
                }
                (Err(old), Err(new)) => {
                    assert_eq!(old.classify(), new.classify());
                    assert_eq!((old.line(), old.column()), (new.line(), new.column()));
                    assert_eq!(old.to_string(), new.to_string());
                }
                _ => panic!("JSON row result differs"),
            }
        }
        for prefix in [0, 1, 59, 60, 61, 62, 63, 64, 65, 66, 123, 124, 125, 126, 127, 128, 129, 130] {
            let row = format!(r#"{{"prefix":"{}","text":"日本語😀é\n\uD83D\uDE00","duplicate":1,"duplicate":2,"number":-0.0}}"#, " ".repeat(prefix));
            for end in 0..=row.len() { same(&row.as_bytes()[..end]); }
            let at = row.find("日本語").unwrap();
            for invalid in [&b"\xff"[..], &b"\xc0\xaf"[..], &b"\xed\xa0\x80"[..], &b"\xf4\x90\x80\x80"[..]] {
                let mut bytes = row.as_bytes().to_vec();
                bytes.splice(at..at + 3, invalid.iter().copied());
                same(&bytes);
            }
        }
        for row in [r#""\uD800""#, r#"{"\uDC00":0}"#, "1e400", "[true,]", "{} trailing", "\"control\x01\""] {
            same(row.as_bytes());
        }
        same(format!("{}0{}", "[".repeat(130), "]".repeat(130)).as_bytes());
    }

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
            fs::File::open(&path).unwrap().set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)).unwrap();
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
    fn buffered_lines_preserve_bytes_and_skip_across_boundaries() {
        let mut input = b"\nabc\r\n".to_vec();
        input.extend_from_slice(
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"text\":\"",
        );
        input.extend(std::iter::repeat_n(b'x', 2_000_000));
        input.extend_from_slice(
            b"\"}}\n{\"type\":\"turn_context\",\"payload\":{\"model\":\"m\"}}\n",
        );
        input.extend_from_slice(b"\xff\n\n");
        input.extend_from_slice("日本語の最後の行".as_bytes());
        let expected: Vec<_> = lines(&input)
            .filter(|l| !skip_codex_line(l))
            .map(<[u8]>::to_vec)
            .collect();
        for cap in [1, 7, 159, 160, 161, 256 * 1024] {
            let mut reader = BufferedLines {
                reader: BufReader::with_capacity(cap, input.as_slice()),
                line: Vec::new(),
                skip: skip_codex_line,
                position: 0, last_complete: 0,
            };
            let mut actual = Vec::new();
            while let Some(line) = reader.next_line() {
                actual.push(line.to_vec());
            }
            assert_eq!(actual, expected, "buffer capacity={cap}");
            assert!(
                reader.line.capacity() < 1024,
                "不要な巨大行を保持していないこと"
            );
        }
    }

    #[test]
    fn buffered_lines_keep_large_rows_without_extra_capacity() {
        let first = "日本語".repeat(300_000);
        let last = "最後の行".repeat(400_000);
        let input = format!("{first}\n\n{{\"type\":\"world_state\",\"ignored\":\"{}\"}}\n{last}", "x".repeat(3 * 1024 * 1024));
        for cap in [64 * 1024, 256 * 1024, 4 * 1024 * 1024] {
            let mut reader = BufferedLines {
                reader: BufReader::with_capacity(cap, input.as_bytes()),
                line: Vec::new(), skip: skip_codex_line,
                position: 0, last_complete: 0,
            };
            assert_eq!(reader.next_line(), Some(first.as_bytes()));
            assert_eq!(reader.next_line(), Some(last.as_bytes()));
            assert_eq!(reader.next_line(), None);
            assert_eq!(reader.line.capacity(), last.len());
        }
    }

    #[test]
    fn buffered_lines_keep_long_required_rows_and_final_skipped_row() {
        let input = format!(
            "{}\n\n{}",
            "日本語".repeat(1000),
            "{\"type\":\"world_state\",\"payload\":{}}"
        );
        let mut reader = BufferedLines {
            reader: BufReader::with_capacity(17, input.as_bytes()),
            line: Vec::new(),
            skip: skip_codex_line,
                position: 0, last_complete: 0,
        };
        assert_eq!(reader.next_line(), Some("日本語".repeat(1000).as_bytes()));
        assert_eq!(reader.next_line(), None);
    }

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
            "const r=await tools.exec_command({cmd:\"sed -n '1,2p' a.py\",workdir:\"/w\",yield_time_ms:10000}); text(r.output);"
                .into(),
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
    fn bounded_yaml_preserves_unicode_indentation_and_truncation() {
        let value = serde_json::json!({"前":"あ\r\nい\n", "rows":[
            {"kind":"日本語", "values":[true, 1, null]},
            {"empty":{}, "text":"x\n\ny\n"}], "empty":[]});
        let expected = "前:\n  あ\n  い\nrows:\n  - kind: 日本語\n    values: [true, 1, null]\n  - empty: {}\n    text:\n      x\n      \n      y\nempty: []";
        assert_eq!(yaml(&value).join("\n"), expected);
        for limit in 0..=expected.chars().count() + 2 {
            assert_eq!(bounded_yaml(&value, limit), truncate(expected, limit), "limit={limit}");
        }
        assert_eq!(bounded_yaml(&serde_json::json!(["a\nb", [2, 3]]), 100), "- a\nb\n- [2, 3]");
        for value in [serde_json::json!({"rows":(0..10_000).collect::<Vec<_>>(), "tail":"省略される"}),
            serde_json::json!({"日本語":"あ".repeat(6001)}),
            serde_json::json!({"lines":"日本語\n".repeat(6001)}),
            Value::Object(std::iter::once(("あ".repeat(6001), Value::Bool(true))).collect())] {
            let full = yaml(&value).join("\n");
            assert_eq!(bounded_yaml(&value, MAX_TEXT), truncate(&full, MAX_TEXT));
        }
    }

    #[test]
    fn inline_arrays_preserve_character_limit_and_scalar_formatting() {
        let short = serde_json::json!(["", null, true, -42, 1.25, "日本語"]);
        assert_eq!(inline_list(short.as_array().unwrap()).as_deref(), Some("[\"\", null, true, -42, 1.25, 日本語]"));
        assert_eq!(inline_list(&[]).as_deref(), Some("[]"));
        let boundary = "あ".repeat(78);
        assert_eq!(inline_list(&[Value::String(boundary.clone())]), Some(format!("[{boundary}]")));
        assert!(inline_list(&[Value::String("あ".repeat(79))]).is_none());
        assert_eq!(inline_list(&[Value::String("x".repeat(74)), Value::String("".into())]),
            Some(format!("[{}, \"\"]", "x".repeat(74))));
        for value in [serde_json::json!(["x\ny"]), serde_json::json!([{}]), serde_json::json!([[]]),
            serde_json::json!(["x".repeat(100_000)]), serde_json::json!(vec![0; 100_000])] {
            assert!(inline_list(value.as_array().unwrap()).is_none());
        }
    }

    #[test]
    fn codex_output_header() {
        assert_eq!(
            pretty(&serde_json::json!({"a": 1, "b": "x\ny", "c": [1, 2], "d": {"e": null}, "f": [{"g": true, "h": "i"}]})),
            "a: 1\nb:\n  x\n  y\nc: [1, 2]\nd:\n  e: null\nf:\n  - g: true\n    h: i"
        );
        assert_eq!(pretty_output("[exit 0]\n{\"ok\":true}"), "[exit 0]\nok: true");
        let ok = |s: &str| (Cow::<str>::Owned(s.to_string()), Status::Ok);
        let err = |s: &str| (Cow::<str>::Owned(s.to_string()), Status::Error);
        assert_eq!(codex_output("Script completed\nWall time 0.2 seconds\nOutput:\nhello"), ok("[0.2s]\nhello"));
        assert_eq!(
            codex_output("Chunk ID: x\nWall time: 0.1 seconds\nProcess exited with code 1\nOriginal token count: 3\nOutput:\nerr"),
            err("[exit 1 · 0.1s]\nerr")
        );
        assert_eq!(codex_output("Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess."), ok("[exit 0 · 0s]\nSuccess."));
        assert_eq!(
            codex_output("{\"output\":\"x\\n\",\"metadata\":{\"exit_code\":2,\"duration_seconds\":0.5}}"),
            err("[exit 2 · 0.5s]\nx\n")
        );
        assert_eq!(codex_output("Script failed\nWall time 0.1 seconds\nOutput:\nboom").1, Status::Error);
        assert_eq!(codex_output("aborted by user after 4.9s").1, Status::Interrupted);
        assert_eq!(codex_output("Wall time: 573.7 seconds\naborted by user").1, Status::Interrupted);
        assert_eq!(codex_output("apply_patch verification failed: Failed to find expected lines").1, Status::Error);
        // ネストした呼び出しの 1 つが失敗していれば失敗
        assert_eq!(
            codex_output("Script completed\nWall time 0.2 seconds\nOutput:\n{\"i\":0,\"status\":\"fulfilled\",\"value\":{\"exit_code\":1,\"output\":\"\"}}\n{\"i\":1,\"status\":\"fulfilled\",\"value\":{\"exit_code\":0,\"output\":\"\"}}").1,
            Status::Error
        );
        assert_eq!(
            codex_output_text(Some(&serde_json::json!([{"type": "input_text", "text": "Script completed\nOutput:\n"}, {"type": "input_text", "text": "hi"}]))),
            "Script completed\nOutput:\nhi"
        );
        // 改行なしで連なった JSON
        assert_eq!(
            codex_output("Script completed\nWall time 0.1 seconds\nOutput:\n{\"i\":0,\"status\":\"fulfilled\",\"value\":{\"exit_code\":0,\"output\":\"a\\n\"}}{\"i\":1,\"status\":\"fulfilled\",\"value\":{\"exit_code\":2,\"output\":\"b\"}}"),
            err("[0.1s]\n── #0 [exit 0]\na\n── #1 [exit 2]\nb")
        );
    }

    #[test]
    fn claude_edits_as_diff() {
        let edit = serde_json::json!({"file_path": "/w/a.rs", "old_string": "a\nb\nc", "new_string": "a\nB\nc\nd"});
        assert_eq!(claude_input("Edit", &edit), "*** Update File: /w/a.rs\n@@\n a\n-b\n+B\n c\n+d");
        let write = serde_json::json!({"file_path": "/w/n.rs", "content": "x\ny\n"});
        assert_eq!(claude_input("Write", &write), "*** Add File: /w/n.rs\n+x\n+y");
        let bash = serde_json::json!({"command": "cargo test", "description": "テスト", "timeout": 60000});
        assert_eq!(claude_input("Bash", &bash), "$ cargo test\n\ntimeout: 60000");
        let todo = serde_json::json!({"todos": [{"content": "A", "status": "completed"}, {"content": "B", "status": "in_progress"}]});
        assert_eq!(claude_input("TodoWrite", &todo), "✓ A\n▶ B");
        assert_eq!(summarize_input("TodoWrite", &todo), "1/2 B");
        // 結果に行番号付きの差分があれば、それで置き換える
        let r = serde_json::json!({"filePath": "/w/a.rs", "structuredPatch": [{"oldStart": 3, "oldLines": 2, "newStart": 3, "newLines": 2, "lines": [" a", "-b", "+B"]}]});
        assert_eq!(claude_patch(&r).unwrap(), "*** Update File: /w/a.rs\n@@ -3,2 +3,2 @@\n a\n-b\n+B");
        assert_eq!(claude_patch(&serde_json::json!({"filePath": "/w", "structuredPatch": []})), None);
    }

    #[test]
    fn diff_keeps_common_lines() {
        let a = ["x", "a", "b", "y"];
        let b = ["x", "b", "c", "y"];
        assert_eq!(diff_lines(&a, &b), [(' ', "x"), ('-', "a"), (' ', "b"), ('+', "c"), (' ', "y")]);
        assert_eq!(diff_lines(&["a"], &["a"]), [(' ', "a")]);
        assert_eq!(diff_lines(&[], &["n"]), [('+', "n")]);
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
            (Cow::Borrowed("[0.2s]\n── #0 [exit 0 · 0.0s]\nhi"), Status::Ok)
        );
    }
}
