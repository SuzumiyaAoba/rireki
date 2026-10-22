use rireki::catalog::{Catalog, SourceFilter};
use rireki::detail::{DetailLoader, Selection};
use rireki::history::History;
use rireki::index::{CHUNK, Indexer};
use rireki::model::{Snapshot, Status};
use rireki::search;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    root: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rireki-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let claude = root.join("claude/projects");
        let codex = root.join("codex/sessions");
        std::fs::create_dir_all(claude.join("project")).unwrap();
        std::fs::create_dir_all(codex.join("2026/10/05")).unwrap();
        Self {
            root,
            claude,
            codex,
        }
    }

    fn codex_session(&self, id: &str) -> PathBuf {
        let path = self.codex.join("2026/10/05").join(format!("{id}.jsonl"));
        append(
            &path,
            [
                codex(
                    "session_meta",
                    9,
                    json!({"id":id,"cwd":"/workspace/codex","source":"cli"}),
                ),
                codex(
                    "response_item",
                    10,
                    json!({"type":"message","role":"user","content":[{"text":"履歴を検索"}]}),
                ),
            ],
        );
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn append(path: &Path, rows: impl IntoIterator<Item = Value>) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for row in rows {
        serde_json::to_writer(&mut file, &row).unwrap();
        file.write_all(b"\n").unwrap();
    }
}

fn codex(ty: &str, time: u32, payload: Value) -> Value {
    json!({"type":ty,"timestamp":format!("2026-10-05T00:00:{time:02}Z"),"payload":payload})
}

fn position(snapshot: &Snapshot, id: &str) -> usize {
    snapshot.sessions.iter().position(|s| s.id == id).unwrap()
}

#[test]
fn headless_history_supports_catalog_search_details_and_incremental_refresh() {
    let fixture = Fixture::new();
    append(
        &fixture.claude.join("project/claude-session.jsonl"),
        [json!({
            "type":"user", "timestamp":"2026-10-05T00:00:05Z",
            "sessionId":"claude-session", "cwd":"/workspace/claude",
            "message":{"role":"user","content":"Claude の確認"}
        })],
    );
    let codex_path = fixture.codex_session("codex-session");
    append(
        &codex_path,
        [codex(
            "response_item",
            11,
            json!({
                "type":"function_call", "name":"exec_command", "call_id":"test",
                "arguments":json!({"cmd":"cargo test"}).to_string()
            }),
        )],
    );

    // 実際のホームや索引 DB を開かず、UI が利用する一連の API を使う。
    let mut history = History::with_indexer(Indexer::in_memory(true));
    let cached = history.load_cached();
    assert_eq!(cached.files, 0);
    assert!(cached.delta.touched.is_empty());
    assert!(cached.delta.snapshot.sessions.is_empty());
    let tasks = history.plan_in(&fixture.claude, &fixture.codex);
    assert_eq!(tasks.len(), 2);
    let mut batches = history.ingest(tasks);
    let first = batches.next().unwrap();
    assert_eq!((first.done, first.total), (2, 2));
    assert_eq!(first.delta.touched.len(), 2);
    assert!(batches.next().is_none());
    drop(batches);
    let old = first.delta.snapshot;
    let ci = position(&old, "codex-session");
    let ai = position(&old, "claude-session");

    let catalog = Catalog::new(&old, SourceFilter::All, None);
    assert_eq!(catalog.scope.len(), 2);
    assert_eq!(catalog.order, [ci, ai]);
    assert_eq!(catalog.projects.len(), 2);
    assert_eq!(catalog.projects[0].cwd, "/workspace/codex");
    assert_eq!(catalog.projects[0].sessions, 1);
    let claude = Catalog::new(&old, SourceFilter::Claude, None);
    assert_eq!(claude.scope, [ai]);
    assert_eq!(claude.order, [ai]);
    let focused = Catalog::new(&old, SourceFilter::All, Some("/workspace/claude"));
    assert_eq!(focused.scope.len(), 2);
    assert_eq!(focused.order, [ai]);

    let mut search = search::Index::default();
    let found = search.search(&old.sessions, &catalog.scope, "CARGO TEST", 10);
    assert_eq!(found.total, 1);
    let hit = found.events[0];
    assert_eq!(hit.session, ci);
    let selection = Selection {
        session: hit.session,
        lane: hit.lane,
        first: hit.event,
        last: hit.event,
    };
    let mut details = DetailLoader::default();
    let pending = details
        .load_session(selection, &old.sessions[ci], || true)
        .unwrap();
    assert_eq!(pending.texts, [("$ cargo test".into(), String::new())]);
    assert_eq!(pending.commands, [vec!["cargo test".to_string()]]);
    assert_eq!(
        old.sessions[ci].lanes[0].events[hit.event].status,
        Status::Pending
    );

    append(
        &codex_path,
        [
            codex(
                "response_item",
                12,
                json!({"type":"function_call_output","call_id":"test","output":"テスト成功"}),
            ),
            codex(
                "response_item",
                13,
                json!({"type":"message","role":"assistant","content":[{"text":"差分更新の確認"}]}),
            ),
        ],
    );
    let tasks = history.plan_in(&fixture.claude, &fixture.codex);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].job.path, codex_path);
    let changed = history.ingest(tasks).next().unwrap();
    assert_eq!((changed.done, changed.total), (1, 1));
    assert_eq!(changed.delta.touched, [ci]);
    let current = changed.delta.snapshot;
    assert!(!Arc::ptr_eq(&old.sessions[ci], &current.sessions[ci]));
    assert!(Arc::ptr_eq(&old.sessions[ai], &current.sessions[ai]));
    assert_eq!(current.sessions[ci].lanes[0].events.len(), 3);
    assert_eq!(old.sessions[ci].lanes[0].events.len(), 2);
    assert_eq!(
        current.sessions[ci].lanes[0].events[hit.event].status,
        Status::Ok
    );

    let refreshed = details
        .load_session(selection, &current.sessions[ci], || true)
        .unwrap();
    assert_eq!(
        refreshed.texts,
        [("$ cargo test".into(), "テスト成功".into())]
    );
    assert_eq!(
        details
            .load_session(selection, &old.sessions[ci], || true)
            .unwrap()
            .texts,
        pending.texts
    );
    assert_eq!(
        search
            .search(&current.sessions, &catalog.scope, "差分更新", 10)
            .total,
        1
    );
    assert!(
        search
            .search(&old.sessions, &catalog.scope, "差分更新", 10)
            .is_empty()
    );

    let unchanged = history.plan_in(&fixture.claude, &fixture.codex);
    assert!(unchanged.is_empty());
    assert!(history.ingest(unchanged).next().is_none());
    let snapshot = history.snapshot();
    assert!(Arc::ptr_eq(&snapshot.sessions[ci], &current.sessions[ci]));
    assert!(Arc::ptr_eq(&snapshot.sessions[ai], &current.sessions[ai]));
}

#[test]
fn interrupted_batches_replan_only_unprocessed_files_and_keep_published_snapshots() {
    let fixture = Fixture::new();
    let total = CHUNK * 2 + 3;
    for i in 0..total {
        fixture.codex_session(&format!("session-{i}"));
    }
    let mut history = History::with_indexer(Indexer::in_memory(true));
    let tasks = history.plan_in(&fixture.claude, &fixture.codex);
    assert_eq!(tasks.len(), total);

    // イテレータを作っただけでは、未処理ファイルを既読にしない。
    drop(history.ingest(tasks));
    assert!(history.snapshot().sessions.is_empty());
    let tasks = history.plan_in(&fixture.claude, &fixture.codex);
    assert_eq!(tasks.len(), total);
    let processed: HashSet<_> = tasks.iter().take(CHUNK).map(|t| t.job.key()).collect();
    let first = history.ingest(tasks).next().unwrap();
    assert_eq!((first.done, first.total), (CHUNK, total));
    assert_eq!(first.delta.touched.len(), CHUNK);
    let old = first.delta.snapshot;
    assert_eq!(old.sessions.len(), CHUNK);

    let remaining = history.plan_in(&fixture.claude, &fixture.codex);
    assert_eq!(remaining.len(), total - CHUNK);
    assert!(remaining.iter().all(|t| !processed.contains(&t.job.key())));
    let resumed: Vec<_> = history.ingest(remaining).collect();
    assert_eq!(resumed.len(), 2);
    assert_eq!((resumed[0].done, resumed[0].total), (CHUNK, CHUNK + 3));
    assert_eq!(resumed[0].delta.touched.len(), CHUNK);
    assert_eq!(resumed[0].delta.snapshot.sessions.len(), CHUNK * 2);
    assert_eq!((resumed[1].done, resumed[1].total), (CHUNK + 3, CHUNK + 3));
    assert_eq!(resumed[1].delta.touched.len(), 3);
    assert_eq!(resumed[1].delta.snapshot.sessions.len(), total);

    let snapshot = history.snapshot();
    let actual: HashSet<_> = snapshot.sessions.iter().map(|s| s.id.clone()).collect();
    let expected: HashSet<_> = (0..total).map(|i| format!("session-{i}")).collect();
    assert_eq!(actual, expected);
    assert_eq!(
        Catalog::new(&snapshot, SourceFilter::Codex, None)
            .order
            .len(),
        total
    );
    assert_eq!(old.sessions.len(), CHUNK);
    for (i, session) in old.sessions.iter().enumerate() {
        assert!(Arc::ptr_eq(session, &snapshot.sessions[i]));
    }
    assert!(history.plan_in(&fixture.claude, &fixture.codex).is_empty());
}
