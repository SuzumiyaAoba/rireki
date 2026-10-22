use super::live_cache::LIVE_CACHE_BYTES;
use super::*;
use crate::db::fixtures::Fixture;
use crate::model::EventText;
use std::sync::mpsc;

fn task(fixture: &Fixture) -> Task {
    let path = fixture.index.with_file_name("log.jsonl");
    std::fs::write(&path, concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"session\"}}\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-10-05T00:00:00Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"日本語の入力\"}]}}"
    )).unwrap();
    Task {
        live: false,
        job: Job {
            kind: loader::JobKind::Codex,
            path,
            mtime: 0.0,
            fingerprint: "first".into(),
        },
    }
}

fn completion(indexer: &mut Indexer) {
    let done = indexer
        .writer
        .as_ref()
        .unwrap()
        .completed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    indexer.finish_write(done);
}

#[test]
fn mixed_live_and_stable_tasks_keep_order_and_retry_only_failed_saves() {
    let fixture = Fixture::new();
    let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
    let tasks: Vec<_> = (0..3).map(|i| {
        let path = fixture.index.with_file_name(format!("{i}.jsonl"));
        std::fs::write(&path, format!("{}\n{}\n",
            serde_json::json!({"type":"session_meta", "payload":{"id":format!("session-{i}")}}),
            serde_json::json!({"type":"response_item", "timestamp":"2026-10-05T00:00:00Z",
                "payload":{"type":"message", "role":"user", "content":[{"text":format!("入力 {i}")}]}}))).unwrap();
        Task { live: i == 1, job: Job { kind: loader::JobKind::Codex, path,
            mtime: if i == 1 { now() } else { 0.0 }, fingerprint: format!("fp-{i}") } }
    }).collect();
    fixture.reject_writes(true);
    let updates = indexer.process(&tasks);
    completion(&mut indexer);
    assert!(indexer.stable.is_empty() && indexer.pending.is_empty());
    for (i, (t, u)) in tasks.iter().zip(&updates).enumerate() {
        assert_eq!(u.origin, t.job.key());
        let Loaded::Session(s) = &u.items[0] else {
            unreachable!()
        };
        assert_eq!(s.id, format!("session-{i}"));
        let e = &s.lanes[0].events[0];
        let (input, output) = match e.text() {
            EventText::Inline(i, o) if t.live => (i.to_owned(), o.to_owned()),
            EventText::Pending(i, o) => (i, o),
            _ => panic!("unexpected body state"),
        };
        assert_eq!((input, output), (format!("入力 {i}"), String::new()));
    }
    assert_eq!(
        indexer.live.get(&tasks[1].job.key()),
        Some(&tasks[1].job.fingerprint)
    );
    let retry = indexer.plan_jobs(tasks.iter().map(|t| t.job.clone()).collect());
    assert_eq!(
        retry.iter().map(|t| t.job.key()).collect::<Vec<_>>(),
        vec![tasks[0].job.key(), tasks[2].job.key()]
    );
    fixture.reject_writes(false);
    let saved = indexer.process(&retry);
    completion(&mut indexer);
    assert_eq!(indexer.stable.len(), 2);
    assert!(
        indexer
            .plan_jobs(tasks.iter().map(|t| t.job.clone()).collect())
            .is_empty()
    );
    let reader = Db::open_index_read(&fixture.texts).unwrap();
    for (u, i) in saved.iter().zip([0, 2]) {
        let Loaded::Session(s) = &u.items[0] else {
            unreachable!()
        };
        let EventText::Row(id) = s.lanes[0].events[0].text() else {
            panic!("saved row expected")
        };
        assert_eq!(
            reader.details(&[id])[&id],
            (format!("入力 {i}"), String::new())
        );
    }
}

#[test]
fn failed_writes_keep_texts_retry_and_mark_only_successful_fingerprints() {
    let fixture = Fixture::new();
    let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
    let task = task(&fixture);
    fixture.reject_writes(true);
    let updates = indexer.process(std::slice::from_ref(&task));
    completion(&mut indexer);
    assert!(indexer.stable.is_empty() && indexer.pending.is_empty());
    assert_eq!(indexer.plan_jobs(vec![task.job.clone()]).len(), 1);
    let Loaded::Session(s) = &updates[0].items[0] else {
        unreachable!()
    };
    let EventText::Pending(i, o) = s.lanes[0].events[0].text() else {
        panic!("failed save must keep body")
    };
    assert_eq!((i.as_str(), o.as_str()), ("日本語の入力", ""));
    fixture.reject_writes(false);
    let updates = indexer.process(std::slice::from_ref(&task));
    completion(&mut indexer);
    assert!(indexer.plan_jobs(vec![task.job.clone()]).is_empty());
    assert_eq!(
        indexer.stable.get(&task.job.key()).map(String::as_str),
        Some("first")
    );
    let Loaded::Session(s) = &updates[0].items[0] else {
        unreachable!()
    };
    assert!(matches!(s.lanes[0].events[0].text(), EventText::Row(_)));
}

#[test]
fn late_save_completion_does_not_replace_new_live_or_pending_version() {
    let fixture = Fixture::new();
    let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
    let mut task = task(&fixture);
    let _ = indexer.process(std::slice::from_ref(&task));
    let old = indexer
        .writer
        .as_ref()
        .unwrap()
        .completed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    task.live = true;
    task.job.fingerprint = "live".into();
    let updates = indexer.process(std::slice::from_ref(&task));
    indexer.finish_write(old);
    assert!(indexer.stable.is_empty());
    assert_eq!(
        indexer.live.get(&task.job.key()).map(String::as_str),
        Some("live")
    );
    let Loaded::Session(s) = &updates[0].items[0] else {
        unreachable!()
    };
    let EventText::Pending(input, output) = s.lanes[0].events[0].text() else {
        panic!("live text expected")
    };
    assert_eq!((input, output), ("日本語の入力".into(), String::new()));
    indexer.pending.insert(task.job.key(), "newer".into());
    indexer.finish_write(Completion {
        files: vec![(task.job.key(), "older".into())],
        saved: true,
    });
    assert_eq!(
        indexer.pending.get(&task.job.key()).map(String::as_str),
        Some("newer")
    );
}

#[test]
fn no_writer_keeps_original_texts_and_does_not_claim_saved() {
    let fixture = Fixture::new();
    let mut indexer = Indexer::with_db(None, None);
    let task = task(&fixture);
    let updates = indexer.process(std::slice::from_ref(&task));
    let Loaded::Session(s) = &updates[0].items[0] else {
        unreachable!()
    };
    assert!(matches!(
        s.lanes[0].events[0].text(),
        EventText::Inline(_, _)
    ));
    assert_eq!(s.lanes[0].events[0].input, "日本語の入力");
    assert!(indexer.stable.is_empty());
    assert_eq!(indexer.plan_jobs(vec![task.job.clone()]).len(), 1);
}

#[test]
fn disconnected_writer_retries_without_losing_pending_body() {
    let fixture = Fixture::new();
    let task = task(&fixture);
    let mut indexer = Indexer::with_db(None, None);
    let (sender, receiver) = mpsc::sync_channel(1);
    let (done, completed) = mpsc::channel::<Completion>();
    let handle = std::thread::spawn(move || {
        let _ = receiver.recv().unwrap();
        drop(done);
    });
    indexer.writer = Some(Writer {
        sender,
        completed,
        handle,
    });
    let updates = indexer.process(std::slice::from_ref(&task));
    assert!(matches!(
        indexer
            .writer
            .as_ref()
            .unwrap()
            .completed
            .recv_timeout(std::time::Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    indexer.complete_writes();
    assert!(indexer.writer.is_none() && indexer.pending.is_empty() && indexer.stable.is_empty());
    let Loaded::Session(s) = &updates[0].items[0] else {
        unreachable!()
    };
    let EventText::Pending(i, o) = s.lanes[0].events[0].text() else {
        panic!("terminated writer must keep body")
    };
    assert_eq!((i.as_str(), o.as_str()), ("日本語の入力", ""));
    assert_eq!(indexer.plan_jobs(vec![task.job.clone()]).len(), 1);
}

#[test]
fn live_to_stable_keeps_compressed_bodies_on_failed_and_successful_writes() {
    let fixture = Fixture::new();
    let mut indexer = Indexer::with_db(Some(fixture.open()), Some(fixture.open()));
    let mut task = task(&fixture);
    task.live = true;
    let live = indexer.process(std::slice::from_ref(&task));
    let Loaded::Session(old) = &live[0].items[0] else {
        unreachable!()
    };
    assert!(indexer.live_cache.bytes() > 0);
    assert!(indexer.stable.is_empty());
    let expected = ("日本語の入力".to_owned(), String::new());
    let EventText::Pending(i, o) = old.lanes[0].events[0].text() else {
        panic!("shared live text expected")
    };
    assert_eq!((i, o), expected);
    fixture.reject_writes(true);
    task.live = false;
    let failed = indexer.process(std::slice::from_ref(&task));
    completion(&mut indexer);
    assert!(indexer.live_cache.is_empty() && indexer.live_cache.bytes() == 0);
    let Loaded::Session(s) = &failed[0].items[0] else {
        unreachable!()
    };
    let EventText::Pending(i, o) = s.lanes[0].events[0].text() else {
        panic!("failed save must keep shared text")
    };
    assert_eq!((i, o), expected);
    fixture.reject_writes(false);
    let saved = indexer.process(std::slice::from_ref(&task));
    completion(&mut indexer);
    let Loaded::Session(s) = &saved[0].items[0] else {
        unreachable!()
    };
    let EventText::Row(id) = s.lanes[0].events[0].text() else {
        panic!("committed row expected")
    };
    assert_eq!(
        Db::open_index_read(&fixture.texts).unwrap().details(&[id])[&id],
        expected
    );
    let EventText::Pending(i, o) = old.lanes[0].events[0].text() else {
        panic!("old snapshot must remain readable")
    };
    assert_eq!((i, o), expected);
}

#[test]
fn live_cache_budget_eviction_and_removed_files_fall_back_to_full_parsing() {
    let fixture = Fixture::new();
    let mut indexer = Indexer::with_db(None, None);
    let mut first = task(&fixture);
    first.live = true;
    let _ = indexer.process(std::slice::from_ref(&first));
    let old = indexer.live_cache.take(&first.job.key()).unwrap();
    // 小さいログの容量見積もりだけを大きくして、実メモリを大量に確保せず上限到達を再現する。
    indexer
        .live_cache
        .insert(first.job.key(), old, LIVE_CACHE_BYTES / 2 + 1);
    let second_path = fixture.index.with_file_name("second.jsonl");
    std::fs::copy(&first.job.path, &second_path).unwrap();
    let second = Task {
        live: true,
        job: Job {
            path: second_path,
            ..first.job.clone()
        },
    };
    let mut second_cache = loader::LiveLog::default();
    second_cache.parse(&second.job);
    indexer
        .live_cache
        .insert(second.job.key(), second_cache, LIVE_CACHE_BYTES / 2 + 1);
    assert!(!indexer.live_cache.contains(&first.job.key()));
    assert!(indexer.live_cache.bytes() <= LIVE_CACHE_BYTES);
    first.job.fingerprint = "changed".into();
    let updates = indexer.process(std::slice::from_ref(&first));
    let Loaded::Session(s) = &updates[0].items[0] else {
        unreachable!()
    };
    let EventText::Pending(i, o) = s.lanes[0].events[0].text() else {
        panic!("reloaded body expected")
    };
    assert_eq!((i, o), ("日本語の入力".into(), String::new()));
    indexer.plan_jobs(vec![first.job.clone()]);
    assert!(!indexer.live_cache.contains(&second.job.key()));
    indexer.plan_jobs(Vec::new());
    assert!(indexer.live_cache.is_empty() && indexer.live_cache.bytes() == 0);
    indexer.live_cache.insert(
        first.job.key(),
        loader::LiveLog::default(),
        LIVE_CACHE_BYTES + 1,
    );
    assert!(indexer.live_cache.is_empty());
}
