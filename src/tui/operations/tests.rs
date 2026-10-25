use super::*;
use rireki::model::{Lane, Source};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn session(title: &str) -> Arc<Session> {
    Arc::new(Session {
        id: "operations-fixture".into(),
        source: Source::Codex,
        cwd: "/tmp/operations".into(),
        title: title.into(),
        branch: String::new(),
        version: String::new(),
        headless: false,
        lanes: vec![Lane::new("main")],
        start: 100.0,
        end: 101.0,
        parents: vec![None],
        tree: vec![(0, 0)],
    })
}

fn idle() -> Operations {
    Operations {
        shared: Arc::default(),
        threads: Vec::new(),
    }
}

fn inspect(generation: u64) -> Request {
    Request::Inspect {
        generation,
        selection: Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: 0,
        },
        session: session("fixture"),
    }
}

#[test]
fn coalescing_cancellation_and_receive_discard_obsolete_results() {
    let operations = idle();
    operations.request(inspect(1));
    operations.request(inspect(2));
    assert_eq!(lock(&operations.shared.pending).len(), 1);
    assert_eq!(
        lock(&operations.shared.pending)
            .get(&RequestKey::Job(Job::Inspect))
            .unwrap()
            .generation(),
        Some((Job::Inspect, 2))
    );
    operations.shared.publish(Update::Inspect {
        generation: 1,
        detail: None,
    });
    assert!(operations.try_recv().is_none());
    operations.shared.publish(Update::Inspect {
        generation: 2,
        detail: None,
    });
    operations.cancel(Job::Inspect, 3);
    assert!(operations.try_recv().is_none());
    assert!(lock(&operations.shared.pending).is_empty());
    operations.shared.publish(Update::Inspect {
        generation: 2,
        detail: None,
    });
    assert!(operations.try_recv().is_none());
    operations.request(inspect(4));
    // 世代照合後に届いた結果も、受信時の照合で落とす。
    lock(&operations.shared.completed).insert(
        UpdateKey::Job(Job::Inspect),
        Update::Inspect {
            generation: 3,
            detail: None,
        },
    );
    assert!(operations.try_recv().is_none());
    operations.shared.publish(Update::Inspect {
        generation: 4,
        detail: None,
    });
    assert!(matches!(
        operations.try_recv(),
        Some(Update::Inspect { generation: 4, .. })
    ));
}

#[test]
fn cancelling_one_job_preserves_other_results_and_control_messages() {
    let operations = idle();
    operations.request(inspect(7));
    operations.request(Request::Analysis {
        generation: 7,
        selection: Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: 0,
        },
        session: session("analysis"),
        filter: analysis::Filter::default(),
        order: analysis::Order::default(),
    });
    operations.shared.publish(Update::Inspect {
        generation: 7,
        detail: None,
    });
    operations.shared.publish(Update::Analysis {
        generation: 7,
        analysis: None,
    });
    operations.shared.publish(Update::Mcp {
        url: Some("current".into()),
        config: String::new(),
        error: None,
    });
    operations
        .shared
        .publish(Update::Error("worker failed".into()));
    // Snapshot は応答を持たず、配送待ちのエラーも消さない。
    operations.request(Request::Snapshot(Snapshot::default()));
    operations.cancel(Job::Inspect, 8);

    assert_eq!(lock(&operations.shared.pending).len(), 2);
    assert!(!operations.shared.current(Job::Inspect, 7));
    assert!(operations.shared.current(Job::Analysis, 7));
    assert!(matches!(
        operations.try_recv(),
        Some(Update::Analysis { generation: 7, .. })
    ));
    assert!(
        matches!(operations.try_recv(), Some(Update::Mcp { url: Some(url), .. }) if url == "current")
    );
    assert!(
        matches!(operations.try_recv(), Some(Update::Error(error)) if error == "worker failed")
    );
    assert!(operations.try_recv().is_none());
}

#[test]
fn control_queue_keeps_latest_snapshot_and_never_accepts_old_mcp_status() {
    let operations = idle();
    operations.request(inspect(1));
    operations.request(Request::Snapshot(Snapshot {
        sessions: vec![session("old")],
    }));
    operations.request(Request::Snapshot(Snapshot {
        sessions: vec![session("latest")],
    }));
    operations.request(Request::Mcp { port: Some(0) });
    operations.request(Request::Mcp { port: None });
    let (requests, generation) = take_requests(&operations.shared, true).unwrap();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests.get(&RequestKey::Mcp),
        Some(Request::Mcp { port: None })
    ));
    assert!(
        matches!(requests.get(&RequestKey::Snapshot), Some(Request::Snapshot(snapshot)) if snapshot.sessions[0].title == "latest")
    );
    assert_eq!(lock(&operations.shared.pending).len(), 1);
    operations.shared.publish_mcp(
        generation - 1,
        Update::Mcp {
            url: Some("old".into()),
            config: String::new(),
            error: None,
        },
    );
    assert!(operations.try_recv().is_none());
    operations.shared.publish_mcp(
        generation,
        Update::Mcp {
            url: None,
            config: String::new(),
            error: None,
        },
    );
    assert!(matches!(
        operations.try_recv(),
        Some(Update::Mcp { url: None, .. })
    ));
    operations.stop();
    assert!(take_requests(&operations.shared, false).is_none());
}

fn mcp_status(operations: &Operations) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(update) = operations.try_recv() {
            match update {
                Update::Mcp { url, config, error } => {
                    assert!(error.is_none(), "{error:?}");
                    if let Some(url) = &url {
                        let config: serde_json::Value = serde_json::from_str(&config).unwrap();
                        assert_eq!(config["mcpServers"]["rireki"]["url"], *url);
                    }
                    return url;
                }
                Update::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "MCP 制御が完了しませんでした");
        thread::sleep(Duration::from_millis(10));
    }
}

fn sessions(url: &str) -> String {
    let address = url
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/mcp")
        .unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#;
    write!(stream, "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    response
}

#[test]
fn mcp_publishes_live_snapshots_and_stops_without_waiting_for_computation_queue() {
    let mut operations = idle();
    // 計算キューを意図的に処理しない。制御ワーカーだけでも公開と停止が完了する。
    operations.spawn("rireki-tui-mcp-test", control).unwrap();
    operations.request(inspect(1));
    operations.request(Request::Snapshot(Snapshot {
        sessions: vec![session("before-refresh")],
    }));
    operations.request(Request::Mcp { port: Some(0) });
    let url = mcp_status(&operations).unwrap();
    assert!(sessions(&url).contains("before-refresh"));
    operations.request(Request::Snapshot(Snapshot {
        sessions: vec![session("after-refresh")],
    }));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !sessions(&url).contains("after-refresh") {
        assert!(
            Instant::now() < deadline,
            "MCP に更新が反映されませんでした"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(lock(&operations.shared.pending).len(), 1);
    operations.request(Request::Mcp { port: None });
    assert!(mcp_status(&operations).is_none());
    let address = url
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/mcp")
        .unwrap();
    assert!(TcpStream::connect(address).is_err());
    operations.finish();
}
