use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rireki::loader::{Job, JobKind};
use rireki::model::{Loaded, Session, Status};
use serde_json::{Value, json};

struct Log(PathBuf);

impl Log {
    fn parse(outputs: &[Value]) -> Session {
        let mut rows = vec![json!({"type":"session_meta","payload":{"id":"output-check"}})];
        for (i, output) in outputs.iter().enumerate() {
            rows.push(json!({"timestamp":"2026-10-06T00:00:00Z","type":"response_item",
                "payload":{"type":"function_call","name":"exec_command","call_id":format!("call-{i}"),
                    "arguments":"{\"cmd\":\"cargo test\"}"}}));
            rows.push(json!({"timestamp":"2026-10-06T00:00:01Z","type":"response_item",
                "payload":{"type":"function_call_output","call_id":format!("call-{i}"),"output":output}}));
        }
        Self::parse_rows(JobKind::Codex, &rows)
    }

    fn parse_rows(kind: JobKind, rows: &[Value]) -> Session {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let log = Self(std::env::temp_dir().join(format!("rireki-output-{}-{nonce}-{unique}.jsonl", std::process::id())));
        let text = rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&log.0).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        drop(file);
        assert_eq!(fs::read_to_string(&log.0).unwrap(), text);
        let mut parsed = rireki::loader::parse(&Job {
            kind, path: log.0.clone(), mtime: 0.0, fingerprint: String::new(),
        });
        let Loaded::Session(session) = parsed.remove(0) else { panic!("session expected") };
        assert_eq!(fs::read_to_string(&log.0).unwrap(), text);
        session
    }
}

#[test]
fn claude_text_blocks_keep_newlines_error_wrappers_and_empty_results() {
    let content = [
        json!("<tool_use_error>日本語の失敗\n</tool_use_error>"),
        json!([{"text":"{\"ok\":true}"}]),
        json!([{"text":"a"}, {"text":""}, {"text":"b"}, {"text":123}]),
        json!([{"type":"image"}, {"text":null}]),
    ];
    let calls: Vec<_> = (0..content.len()).map(|i| json!({"type":"tool_use","id":format!("call-{i}"),
        "name":"Bash","input":{"command":"cargo test"}})).collect();
    let results: Vec<_> = content.into_iter().enumerate().map(|(i, content)| json!({
        "type":"tool_result","tool_use_id":format!("call-{i}"),"content":content,"is_error":i==0,
    })).collect();
    let session = Log::parse_rows(JobKind::Claude, &[
        json!({"type":"assistant","timestamp":"2026-10-06T00:00:00Z","message":{"content":calls}}),
        json!({"type":"user","timestamp":"2026-10-06T00:00:01Z","message":{"content":results}}),
    ]);
    assert_eq!(session.lanes[0].events.len(), 4);
    for (event, (output, status)) in session.lanes[0].events.iter().zip([
        ("日本語の失敗\n", Status::Error), ("ok: true", Status::Ok),
        ("a\n\nb", Status::Ok), ("", Status::Ok),
    ]) {
        assert_eq!(event.output, output);
        assert_eq!(event.status, status);
    }
}

impl Drop for Log {
    fn drop(&mut self) { let _ = fs::remove_file(&self.0); }
}

#[test]
fn output_encodings_keep_text_headers_json_formatting_and_failures() {
    let outputs = [
        json!("日本語の結果\n"),
        json!([{"type":"input_text","text":"Script completed\nWall time 0.2 seconds\nOutput:\n"},
            {"type":"input_text","text":"hello"}, {"type":"input_image","image_url":"unused"}]),
        json!({"content":[{"text":"apply_patch verification failed: missing"}]}),
        json!("{\"output\":\"日本語\\n\",\"metadata\":{\"exit_code\":2,\"duration_seconds\":0.5}}"),
        json!("[exit 0]\n{\"ok\":true,\"lines\":[1,2]}"),
        json!("unexpected header\nOutput:\n{\"ok\":true}"),
        Value::Null,
    ];
    let session = Log::parse(&outputs);
    let expected = [
        ("日本語の結果\n", Status::Ok),
        ("[0.2s]\nhello", Status::Ok),
        ("apply_patch verification failed: missing", Status::Error),
        ("[exit 2 · 0.5s]\n日本語\n", Status::Error),
        ("[exit 0]\nok: true\nlines: [1, 2]", Status::Ok),
        ("unexpected header\nOutput:\n{\"ok\":true}", Status::Ok),
        ("null", Status::Ok),
    ];
    assert_eq!(session.lanes[0].events.len(), expected.len());
    for (event, (text, status)) in session.lanes[0].events.iter().zip(expected) {
        assert_eq!(event.output, text);
        assert_eq!(event.status, status);
        assert_eq!(event.end - event.start, 1.0);
    }
}

#[test]
fn image_outputs_keep_text_order_and_nested_tool_failures() {
    let output = json!([
        {"type":"input_image","image_url":"A".repeat(2 * 1024 * 1024)},
        {"type":"input_text","text":"Script completed\nWall time 0.1 seconds\nOutput:\n"},
        {"type":"input_image","image_url":"unused","text":"{\"i\":0,\"status\":\"fulfilled\","},
        {"type":"input_text","text":"\"value\":{\"exit_code\":1,\"output\":\"日本語\\n\"}}"},
        {"text":null}, "ignored", ["ignored"],
    ]);
    let session = Log::parse(&[output]);
    let event = &session.lanes[0].events[0];
    assert_eq!(event.output, "[0.1s]\n── #0 [exit 1]\n日本語");
    assert_eq!(event.status, Status::Error);
}

#[test]
fn failures_after_the_display_limit_are_still_detected() {
    let body = "日本語の長い結果\r\n".repeat(10_000);
    let output = format!("Script completed\nWall time 0.2 seconds\nOutput:\n{body}{{\"exit_code\":1,\"output\":\"late failure\"}}");
    let session = Log::parse(&[json!(output)]);
    let event = &session.lanes[0].events[0];
    let normalized = format!("[0.2s]\n{}", "日本語の長い結果\n".repeat(10_000));
    let expected = normalized.chars().take(6000).collect::<String>() + "…";
    assert_eq!(event.output, expected);
    assert_eq!(event.status, Status::Error);
    assert_eq!(event.output.chars().count(), 6001);
}
