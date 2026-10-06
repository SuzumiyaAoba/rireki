use rireki::loader::{self, Job, JobKind, LiveLog};
use rireki::model::{EventText, Lane, Loaded, Status};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rireki-live-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn job(&self, kind: JobKind) -> Job {
        Job {
            kind,
            path: self.0.join("session.jsonl"),
            mtime: 0.0,
            fingerprint: String::new(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn append(path: &Path, bytes: &[u8]) {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn line(value: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn codex(ty: &str, time: u32, payload: Value) -> Value {
    json!({"type": ty, "timestamp": format!("2026-10-05T00:00:{time:02}Z"), "payload": payload})
}

fn claude(ty: &str, time: u32, message: Value) -> Value {
    json!({"type": ty, "timestamp": format!("2026-10-05T00:00:{time:02}Z"), "sessionId": "parent", "cwd": "/project", "message": message})
}

fn body(event: &rireki::model::Event) -> (String, String) {
    match event.text() {
        EventText::Inline(i, o) => (i.to_owned(), o.to_owned()),
        EventText::Pending(i, o) => (i, o),
        EventText::Row(_) => panic!("unexpected database row"),
    }
}

fn lane_value(lane: &Lane) -> Value {
    json!({"name":lane.name, "spawned_by":lane.spawned_by, "model":lane.model, "effort":lane.effort,
        "events":lane.events.iter().map(|e| json!({"start":e.start.to_bits(), "end":e.end.to_bits(),
            "kind":format!("{:?}",e.kind), "status":format!("{:?}",e.status), "name":e.name.as_ref(),
            "summary":e.summary.as_ref(), "body":body(e), "call_id":e.call_id.as_deref(), "model":e.model.as_deref(),
            "usage":format!("{:?}",e.usage)})).collect::<Vec<_>>()})
}

fn canonical(items: &[Loaded]) -> Value {
    Value::Array(items.iter().map(|item| match item {
        Loaded::Session(s) => json!({"id":s.id, "source":format!("{:?}",s.source), "cwd":s.cwd, "headless":s.headless,
            "title":s.title, "branch":s.branch, "version":s.version, "lanes":s.lanes.iter().map(lane_value).collect::<Vec<_>>()}),
        Loaded::Child(parent, lane) => json!({"parent":parent, "lane":lane_value(lane)}),
    }).collect())
}

fn check(cache: &mut LiveLog, job: &Job) -> Vec<Loaded> {
    let cached = cache.parse(job);
    assert_eq!(canonical(&cached), canonical(&loader::parse(job)));
    cached
}

#[test]
fn codex_appends_update_old_calls_usage_models_and_preserve_previous_versions() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Codex);
    let rows = [
        codex(
            "session_meta",
            0,
            json!({"id":"session", "cwd":"/project", "cli_version":"1", "git":{"branch":"main"}}),
        ),
        codex("turn_context", 0, json!({"model":"gpt-a", "effort":"high"})),
        codex(
            "event_msg",
            0,
            json!({"type":"token_count", "info":{"total_token_usage":{"input_tokens":10}}}),
        ),
        codex(
            "response_item",
            5,
            json!({"type":"message", "role":"user", "content":[{"text":"日本語"}]}),
        ),
        codex(
            "response_item",
            3,
            json!({"type":"function_call", "name":"exec_command", "call_id":"call", "arguments":"{\"cmd\":\"ls\"}"}),
        ),
        codex(
            "event_msg",
            6,
            json!({"type":"token_count", "info":{"total_token_usage":{"input_tokens":15,"output_tokens":2}}}),
        ),
        codex("event_msg", 7, json!({"type":"turn_aborted"})),
        codex(
            "response_item",
            8,
            json!({"type":"function_call_output", "call_id":"call", "output":"Process exited with code 0\nFinal output:\n完成"}),
        ),
        codex("turn_context", 9, json!({"model":"gpt-b", "effort":"low"})),
        codex(
            "event_msg",
            10,
            json!({"type":"token_count", "info":{"total_token_usage":{"input_tokens":1},"last_token_usage":{"input_tokens":3,"output_tokens":4}}}),
        ),
    ];
    let mut cache = LiveLog::default();
    let mut previous = Vec::new();
    let mut bytes = 0;
    for row in rows {
        let row = line(row);
        append(&job.path, &row);
        let items = check(&mut cache, &job);
        assert_eq!(cache.last_update().verified_bytes, bytes);
        assert_eq!(cache.last_update().parsed_lines, 1);
        assert_eq!(cache.last_update().resets, 0);
        bytes += row.len() as u64;
        previous.push((canonical(&items), items));
    }
    for (expected, old) in &previous {
        assert_eq!(*expected, canonical(old));
    }
    let Loaded::Session(final_session) = &previous.last().unwrap().1[0] else {
        unreachable!()
    };
    let event = &final_session.lanes[0].events[0];
    assert_eq!(event.call_id.as_deref(), Some("call"));
    assert_eq!(event.status, Status::Ok);
    assert_eq!(
        body(event),
        (
            "$ ls".into(),
            "Process exited with code 0\nFinal output:\n完成".into()
        )
    );
    assert_eq!(event.model.as_deref(), Some("gpt-b"));
}

#[test]
fn every_byte_of_partial_utf8_and_final_rows_matches_full_parsing() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Codex);
    let rows = [
        codex("session_meta", 0, json!({"id":"partial"})),
        codex(
            "response_item",
            1,
            json!({"type":"message", "role":"user", "content":[{"text":"途中の日本語 🚀"}]}),
        ),
        codex(
            "response_item",
            2,
            json!({"type":"function_call", "name":"read_file", "call_id":"x", "arguments":"{\"path\":\"a.rs\"}"}),
        ),
        codex(
            "response_item",
            3,
            json!({"type":"function_call_output", "call_id":"x", "output":"結果"}),
        ),
    ];
    let mut cache = LiveLog::default();
    for row in rows {
        let mut bytes = serde_json::to_vec(&row).unwrap();
        bytes.extend_from_slice(b" \r\n\n");
        for byte in bytes {
            append(&job.path, &[byte]);
            check(&mut cache, &job);
        }
    }
    append(&job.path, b"{broken\n");
    check(&mut cache, &job);
    let mut final_row = line(codex(
        "response_item",
        4,
        json!({"type":"message","role":"assistant","content":[{"text":"末尾"}]}),
    ));
    final_row.pop();
    append(&job.path, &final_row);
    let published = check(&mut cache, &job);
    append(&job.path, b"malformed\n");
    let current = check(&mut cache, &job);
    assert_ne!(canonical(&published), canonical(&current));
}

#[test]
fn edits_in_earlier_checksum_blocks_truncation_and_file_replacement_reset_state() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Codex);
    let mut bytes = line(codex("session_meta", 0, json!({"id":"rewrite"})));
    let first = line(codex(
        "response_item",
        1,
        json!({"type":"message","role":"user","content":[{"text":"before"}]}),
    ));
    bytes.extend(&first);
    bytes.extend(vec![b'\n'; 3 * 64 * 1024]);
    std::fs::write(&job.path, &bytes).unwrap();
    let mut cache = LiveLog::default();
    let previous = check(&mut cache, &job);
    let old = canonical(&previous);
    let at = bytes.windows(6).position(|b| b == b"before").unwrap();
    bytes[at..at + 6].copy_from_slice(b"edited");
    std::fs::write(&job.path, &bytes).unwrap();
    check(&mut cache, &job);
    assert_eq!(cache.last_update().resets, 1);
    assert_eq!(canonical(&previous), old);
    let replacement = fixture.0.join("replacement");
    std::fs::write(
        &replacement,
        line(codex(
            "response_item",
            2,
            json!({"type":"message","role":"user","content":[{"text":"replacement"}]}),
        )),
    )
    .unwrap();
    std::fs::rename(replacement, &job.path).unwrap();
    check(&mut cache, &job);
    assert_eq!(cache.last_update().resets, 1);
    std::fs::write(&job.path, []).unwrap();
    assert!(check(&mut cache, &job).is_empty());
    assert_eq!(cache.last_update().resets, 1);
    std::fs::remove_file(&job.path).unwrap();
    assert!(check(&mut cache, &job).is_empty());
    append(&job.path, &first);
    check(&mut cache, &job);
}

#[test]
fn claude_split_messages_replace_usage_and_tool_results_without_double_counting() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Claude);
    let rows = [
        claude("user", 4, json!({"content":"利用者"})),
        claude(
            "assistant",
            2,
            json!({"id":"m", "model":"claude-a", "usage":{"input_tokens":10}, "content":[{"type":"tool_use","id":"x","name":"Bash","input":{"command":"ls"}}]}),
        ),
        claude(
            "assistant",
            3,
            json!({"id":"m", "model":"claude-b", "usage":{"input_tokens":12,"output_tokens":5}, "content":[{"type":"text","text":"説明"}]}),
        ),
        claude(
            "user",
            6,
            json!({"content":[{"type":"tool_result","tool_use_id":"x","content":"完了"}]}),
        ),
        json!({"type":"ai-title","aiTitle":"新しいタイトル","gitBranch":"topic","version":"2","entrypoint":"sdk"}),
        claude(
            "assistant",
            7,
            json!({"id":"m", "usage":{"input_tokens":15}, "content":[]}),
        ),
    ];
    let mut cache = LiveLog::default();
    let mut previous = Vec::new();
    for row in rows {
        append(&job.path, &line(row));
        let items = check(&mut cache, &job);
        assert_eq!(cache.last_update().parsed_lines, 1);
        previous.push((canonical(&items), items));
    }
    for (expected, items) in &previous {
        assert_eq!(*expected, canonical(items));
    }
    let Loaded::Session(session) = &previous.last().unwrap().1[0] else {
        unreachable!()
    };
    assert_eq!(session.title, "新しいタイトル");
    assert_eq!(
        session.lanes[0]
            .events
            .iter()
            .map(|e| e.usage.input)
            .sum::<u32>(),
        15
    );
    assert_eq!(
        body(&session.lanes[0].events[0]),
        ("$ ls".into(), "完了".into())
    );
}

#[test]
fn claude_related_files_metadata_legacy_children_and_deletion_follow_current_files() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Claude);
    append(
        &job.path,
        &line(claude("user", 0, json!({"content":"メイン"}))),
    );
    let mut cache = LiveLog::default();
    check(&mut cache, &job);
    let subs = job.path.with_extension("").join("subagents");
    std::fs::create_dir_all(&subs).unwrap();
    let child = subs.join("agent-a.jsonl");
    append(&child, &line(claude("user", 1, json!({"content":"子"}))));
    let meta = child.with_extension("meta.json");
    std::fs::write(
        &meta,
        br#"{"agentType":"explore","description":"first","toolUseId":"call"}"#,
    )
    .unwrap();
    check(&mut cache, &job);
    assert_eq!(cache.last_update().parsed_lines, 1);
    std::fs::write(
        &meta,
        br#"{"agentType":"review","description":"changed","toolUseId":"other"}"#,
    )
    .unwrap();
    check(&mut cache, &job);
    assert_eq!(cache.last_update().parsed_lines, 0);
    append(
        &child,
        &line(claude(
            "assistant",
            2,
            json!({"content":[{"type":"text","text":"追加"}]}),
        )),
    );
    check(&mut cache, &job);
    assert_eq!(cache.last_update().parsed_lines, 1);
    std::fs::remove_file(&child).unwrap();
    let items = check(&mut cache, &job);
    let Loaded::Session(s) = &items[0] else {
        unreachable!()
    };
    assert_eq!(s.lanes.len(), 1);
    let legacy = Job {
        kind: JobKind::ClaudeLegacyAgent,
        path: fixture.0.join("agent-legacy.jsonl"),
        ..job.clone()
    };
    let mut legacy_cache = LiveLog::default();
    append(
        &legacy.path,
        b"{\"type\":\"summary\",\"summary\":\"no session\"}\n",
    );
    assert!(check(&mut legacy_cache, &legacy).is_empty());
    append(
        &legacy.path,
        &line(claude("user", 3, json!({"content":"旧形式"}))),
    );
    check(&mut legacy_cache, &legacy);
}

#[test]
fn oversized_ignored_rows_are_streamed_and_required_rows_keep_their_text() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Codex);
    let mut cache = LiveLog::default();
    append(
        &job.path,
        &line(codex(
            "response_item",
            0,
            json!({"type":"message","role":"user","content":[{"text":"small"}]}),
        )),
    );
    check(&mut cache, &job);
    append(
        &job.path,
        &line(codex(
            "response_item",
            1,
            json!({"type":"reasoning","opaque":"x".repeat(4 * 1024 * 1024)}),
        )),
    );
    append(
        &job.path,
        &line(codex(
            "response_item",
            2,
            json!({"type":"message","role":"assistant","content":[{"text":"y".repeat(3 * 1024 * 1024)}]}),
        )),
    );
    check(&mut cache, &job);
    assert!(cache.retained_bytes() < 200 * 1024);
    append(
        &job.path,
        &line(codex(
            "response_item",
            3,
            json!({"type":"message","role":"assistant","content":[{"text":"追加"}]}),
        )),
    );
    check(&mut cache, &job);
    assert_eq!(cache.last_update().parsed_lines, 1);
}

#[test]
fn hidden_internal_codex_sessions_and_changed_parent_metadata_match_full_loading() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Codex);
    let row = line(codex(
        "response_item",
        1,
        json!({"type":"message","role":"user","content":[{"text":"child"}]}),
    ));
    let mut cache = LiveLog::default();
    append(
        &job.path,
        &line(codex(
            "session_meta",
            0,
            json!({"source":{"subagent":{"guardian":{}}}}),
        )),
    );
    append(&job.path, &row);
    assert!(check(&mut cache, &job).is_empty());
    let mut replacement = line(codex(
        "session_meta",
        0,
        json!({"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent","agent_nickname":"agent","agent_path":"/a/b"}}}}),
    ));
    replacement.extend(row);
    std::fs::write(&job.path, replacement).unwrap();
    let items = check(&mut cache, &job);
    assert!(matches!(&items[0], Loaded::Child(parent, _) if parent == "parent"));
}

#[test]
fn claude_partial_usage_and_patch_results_keep_sorted_indices_and_shared_texts() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Claude);
    let mut cache = LiveLog::default();
    append(
        &job.path,
        &line(claude(
            "assistant",
            5,
            json!({"id":"patch-message", "model":"claude", "usage":{"input_tokens":10},
        "content":[{"type":"tool_use","id":"patch","name":"Edit","input":{"file_path":"a.rs","old_string":"old","new_string":"new"}}]}),
        )),
    );
    let previous = check(&mut cache, &job);
    let old = canonical(&previous);
    let result = json!({"type":"user","sessionId":"parent","timestamp":"2026-10-05T00:00:06Z",
        "message":{"content":[{"type":"tool_result","tool_use_id":"patch","content":"updated"}]},
        "toolUseResult":{"filePath":"a.rs","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-old","+new"]}]}});
    for byte in serde_json::to_vec(&result).unwrap() {
        append(&job.path, &[byte]);
        check(&mut cache, &job);
    }
    append(&job.path, b"\n");
    let items = check(&mut cache, &job);
    let Loaded::Session(s) = &items[0] else {
        unreachable!()
    };
    assert!(
        body(&s.lanes[0].events[0])
            .0
            .starts_with("*** Update File: a.rs")
    );
    assert_eq!(canonical(&previous), old);
    let mut snapshot = s.clone();
    snapshot.finalize();
    let Loaded::Session(mut reference) = loader::parse(&job).remove(0) else {
        unreachable!()
    };
    reference.finalize();
    let selection = rireki::detail::Selection {
        session: 0,
        lane: 0,
        first: 0,
        last: 0,
    };
    let actual = rireki::detail::DetailLoader::default()
        .load_session(selection, &std::sync::Arc::new(snapshot), || true)
        .unwrap();
    let expected = rireki::detail::DetailLoader::default()
        .load_session(selection, &std::sync::Arc::new(reference), || true)
        .unwrap();
    assert_eq!(actual.texts, expected.texts);
    assert_eq!(actual.touched, expected.touched);
    assert_eq!(actual.out_lines, expected.out_lines);
    assert_eq!(actual.files[0].path, "a.rs");
    // 過去に戻る時刻と既出 message.id の最終値を、途中のJSONを含めて一バイトずつ反映する。
    let usage = claude(
        "assistant",
        2,
        json!({"id":"patch-message", "usage":{"input_tokens":20},"content":[{"type":"text","text":"後から来た古い時刻"}]}),
    );
    for byte in serde_json::to_vec(&usage).unwrap() {
        append(&job.path, &[byte]);
        check(&mut cache, &job);
    }
    append(&job.path, b"\n");
    let items = check(&mut cache, &job);
    let Loaded::Session(s) = &items[0] else {
        unreachable!()
    };
    assert_eq!(
        s.lanes[0].events.iter().map(|e| e.usage.input).sum::<u32>(),
        20
    );
}

#[test]
fn reused_call_ids_resolve_the_latest_unsorted_call() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Codex);
    let mut cache = LiveLog::default();
    for t in [5, 2] {
        append(
            &job.path,
            &line(codex(
                "response_item",
                t,
                json!({"type":"function_call","name":"exec_command","call_id":"same","arguments":format!("{{\"cmd\":\"run-{t}\"}}")}),
            )),
        );
        check(&mut cache, &job);
    }
    append(
        &job.path,
        &line(codex(
            "response_item",
            7,
            json!({"type":"function_call_output","call_id":"same","output":"done"}),
        )),
    );
    let items = check(&mut cache, &job);
    let Loaded::Session(s) = &items[0] else {
        unreachable!()
    };
    assert_eq!(s.lanes[0].events[0].status, Status::Ok);
    assert_eq!(s.lanes[0].events[1].status, Status::Pending);
}

#[test]
fn hidden_codex_logs_stop_before_a_large_unread_tail() {
    let fixture = Fixture::new();
    let job = fixture.job(JobKind::Codex);
    let header = line(codex(
        "session_meta",
        0,
        json!({"source":{"subagent":{"guardian":{}}}}),
    ));
    append(&job.path, &header);
    append(&job.path, &vec![b'x'; 10 * 1024 * 1024]);
    let mut cache = LiveLog::default();
    assert!(check(&mut cache, &job).is_empty());
    assert_eq!(cache.last_update().parsed_lines, 1);
    append(&job.path, b"more unread data\n");
    assert!(check(&mut cache, &job).is_empty());
    assert_eq!(cache.last_update().verified_bytes, header.len() as u64);
    assert_eq!(cache.last_update().parsed_lines, 0);
    assert!(cache.retained_bytes() < 16 * 1024);
}

#[test]
fn large_batches_of_delayed_results_keep_inputs_and_partial_snapshots() {
    for kind in [JobKind::Codex, JobKind::Claude] {
        let fixture = Fixture::new();
        let job = fixture.job(kind);
        let mut initial = Vec::new();
        for i in 0..600 {
            let call = format!("call-{i}");
            let row = match kind {
                JobKind::Codex => codex(
                    "response_item",
                    i % 59,
                    json!({
                        "type":"function_call", "name":"exec_command", "call_id":call,
                        "arguments":json!({"cmd":format!("echo 入力-{i}")}).to_string()
                    }),
                ),
                _ => claude(
                    "assistant",
                    i % 59,
                    json!({"content":[{
                        "type":"tool_use", "name":"Edit", "id":call,
                        "input":{"file_path":format!("file-{i}.rs"),"old_string":"before","new_string":"after"}
                    }]}),
                ),
            };
            initial.extend(line(row));
        }
        initial.pop(); // 最後の呼び出しは改行前の公開版にだけ含まれる。
        append(&job.path, &initial);
        let mut cache = LiveLog::default();
        let pending = check(&mut cache, &job);
        let old = canonical(&pending);
        let mut results = vec![b'\n'];
        for i in (0..600).rev() {
            let output = format!("結果-{i}。日本語の本文を保持する。\n").repeat(80);
            let call = format!("call-{i}");
            let row = match kind {
                JobKind::Codex => codex(
                    "response_item",
                    i % 59,
                    json!({
                        "type":"function_call_output", "call_id":call, "output":output
                    }),
                ),
                _ => claude(
                    "user",
                    i % 59,
                    json!({"content":[{
                        "type":"tool_result", "tool_use_id":call, "content":output, "is_error":i % 7 == 0
                    }]}),
                ),
            };
            results.extend(line(row));
        }
        results.pop(); // 古い呼び出しへの最終結果も部分行として公開する。
        append(&job.path, &results);
        let completed = check(&mut cache, &job);
        assert_eq!(canonical(&pending), old);
        let expected = canonical(&completed);
        append(&job.path, b"\n");
        assert_eq!(canonical(&check(&mut cache, &job)), expected);
        assert_eq!(canonical(&completed), expected);
        assert_eq!(canonical(&pending), old);
        let Loaded::Session(session) = &completed[0] else {
            unreachable!()
        };
        for event in &session.lanes[0].events {
            let (input, output) = body(event);
            assert!(!input.is_empty() && output.starts_with("結果-"));
        }
    }
}
