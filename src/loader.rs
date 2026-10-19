//! Claude Code / Codex のセッションログ (JSONL) を読み込む
//!
//! 形式別の解析状態は `claude` / `codex`、差分更新は `live` が管理する。
//! このルートは共通のイベント生成・保留呼び出し・共有文字列と公開 API を保つ。
//! ファイル探索は `discovery`、行読み出しは `log_lines`、共通の本文整形は
//! `format`、Codex のツール・JS の整形は `codex_tools` が担当する。

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde_json::Value;

use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Status, Usage, one_line, truncate};


mod claude;
mod codex;
use claude::{ClaudeParse, ClaudeParser, load_claude, load_claude_legacy_agent};
#[cfg(test)]
use claude::parse_claude_file;
use codex::{CodexParser, load_codex, skip_codex_line};
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
