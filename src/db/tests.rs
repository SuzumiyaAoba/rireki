//! 保存状態、旧版の本文、並列読み取りの互換性テスト。

use super::connection::remove_db;
use super::events::{EventGroup, EventRows, SummaryCache, read_events};
use super::packing::pack;
use super::*;
use crate::model::{Kind, Lane, Loaded, Usage};
use rusqlite::params;
use std::sync::Arc;

#[test]
fn saved_text_references_preserve_order_and_release_compressed_snapshots() {
    let fixture = fixtures::Fixture::new();
    let mut db = fixture.open();
    let output = "日本語\n".repeat(4096);
    let mut items = fixtures::items(&[("first", "", ""), ("second", "入力\n", &output)]);
    let Loaded::Session(extra) =
        fixtures::items(&[("other", "other input", "other output")]).remove(0)
    else {
        unreachable!()
    };
    let mut child = extra.lanes[0].clone();
    child.name = "child".into();
    let Loaded::Session(session) = &mut items[0] else {
        unreachable!()
    };
    session.lanes.push(extra.lanes[0].clone());
    items.push(Loaded::Child("session".into(), child));
    let packed = pack_files(&[("file".into(), "fp".into(), &items)]);
    let weak = Arc::downgrade(&packed[0].texts);
    packed[0].release_texts(&mut items);
    let mut store = crate::model::Store::default();
    store.replace([("file".into(), items)]);
    let snapshot = store.snapshot();
    let events: Vec<_> = snapshot.sessions[0]
        .lanes
        .iter()
        .flat_map(|l| &l.events)
        .collect();
    assert_eq!(events.len(), 4);
    let expected = [
        ("", ""),
        ("入力\n", output.as_str()),
        ("other input", "other output"),
        ("other input", "other output"),
    ];
    for (e, (i, o)) in events.iter().zip(expected) {
        assert!(e.input.is_empty() && e.output.is_empty());
        let crate::model::EventText::Pending(input, output) = e.text() else {
            panic!("pending expected")
        };
        assert_eq!((input.as_str(), output.as_str()), (i, o));
    }
    let bytes = crate::model::pending_text_bytes(events.iter().copied());
    assert_eq!(
        bytes,
        packed[0]
            .texts
            .iter()
            .map(|(i, o)| i.len() + o.len())
            .sum::<usize>()
    );
    db.put(&packed).unwrap();
    drop(packed);
    assert!(
        weak.upgrade().is_none(),
        "旧版のスナップショットも圧縮本文を保持しない"
    );
    assert_eq!(crate::model::pending_text_bytes(events.iter().copied()), 0);
    let reader = Db::open_index_read(&fixture.texts).unwrap();
    let ids: Vec<_> = events
        .iter()
        .map(|e| match e.text() {
            crate::model::EventText::Row(id) => id,
            _ => panic!("row expected"),
        })
        .collect();
    let mut texts = reader.details(&ids);
    for (id, (i, o)) in ids.iter().zip(expected) {
        let (input, output) = texts.remove(id).unwrap();
        assert_eq!((input.as_str(), output.as_str()), (i, o));
    }
}

#[test]
fn overwritten_snapshots_keep_bodies_and_empty_files_do_not_reuse_ids() {
    use crate::model::EventText;
    let fixture = fixtures::Fixture::new();
    let mut db = fixture.open();
    let reader = Db::open_index_read(&fixture.texts).unwrap();
    let mut versions = Vec::new();
    let mut ids = Vec::new();
    // 空の版を挟んでも、詳細読み込みが取得した旧版の行IDは再利用しない。
    for (fp, values) in [
        ("1", vec![("old", "旧入力", "旧結果")]),
        ("empty", vec![]),
        ("2", vec![("new", "new input", "new output")]),
    ] {
        let mut items = fixtures::items(&values);
        let packed = pack_files(&[("file".into(), fp.into(), &items)]);
        let weak = Arc::downgrade(&packed[0].source);
        packed[0].release_texts(&mut items);
        db.put(&packed).unwrap();
        drop(packed);
        let Loaded::Session(session) = items.remove(0) else {
            unreachable!()
        };
        if let Some(e) = session.lanes[0].events.first() {
            let EventText::Row(id) = e.text() else {
                panic!("saved row expected")
            };
            ids.push(id);
        }
        versions.push((session, weak));
    }
    assert!(ids[1] > ids[0]);
    assert!(reader.details(&[ids[0]]).is_empty());
    let EventText::Pending(input, output) = versions[0].0.lanes[0].events[0].text() else {
        panic!("old body expected")
    };
    assert_eq!((input.as_str(), output.as_str()), ("旧入力", "旧結果"));
    assert_eq!(
        reader.details(&[ids[1]])[&ids[1]],
        ("new input".into(), "new output".into())
    );
    assert_eq!(
        crate::model::pending_text_bytes(&versions[2].0.lanes[0].events),
        0
    );
    let weak = versions[0].1.clone();
    drop(versions);
    assert!(
        weak.upgrade().is_none(),
        "writer must not retain superseded snapshots"
    );
}

#[test]
fn failed_overwrite_preserves_both_versions_for_retry() {
    use crate::model::EventText;
    let fixture = fixtures::Fixture::new();
    let mut db = fixture.open();
    let mut old = fixtures::items(&[("old", "old input", "old output")]);
    let first = pack_files(&[("file".into(), "1".into(), &old)]);
    first[0].release_texts(&mut old);
    db.put(&first).unwrap();
    drop(first);
    let mut new = fixtures::items(&[("new", "new input", "new output")]);
    let second = pack_files(&[("file".into(), "2".into(), &new)]);
    second[0].release_texts(&mut new);
    fixture.reject_writes(true);
    assert!(db.put(&second).is_err());
    assert_eq!(db.fingerprints().unwrap()["file"], "1");
    for (items, expected) in [
        (&old, ("old input", "old output")),
        (&new, ("new input", "new output")),
    ] {
        let Loaded::Session(s) = &items[0] else {
            unreachable!()
        };
        let EventText::Pending(i, o) = s.lanes[0].events[0].text() else {
            panic!("body expected")
        };
        assert_eq!((i.as_str(), o.as_str()), expected);
    }
    fixture.reject_writes(false);
    db.put(&second).unwrap();
    assert_eq!(db.fingerprints().unwrap()["file"], "2");
    let Loaded::Session(old) = &old[0] else {
        unreachable!()
    };
    assert!(matches!(
        old.lanes[0].events[0].text(),
        EventText::Pending(_, _)
    ));
    let Loaded::Session(new) = &new[0] else {
        unreachable!()
    };
    assert!(matches!(new.lanes[0].events[0].text(), EventText::Row(_)));
}

#[test]
fn failed_batch_keeps_all_pending_texts_until_retry_commits() {
    let fixture = fixtures::Fixture::new();
    let mut db = fixture.open();
    let mut first = fixtures::items(&[("good", "入力", "結果")]);
    let mut second = fixtures::items(&[("reject", "second", "body")]);
    let packed = pack_files(&[
        ("first".into(), "1".into(), &first),
        ("second".into(), "2".into(), &second),
    ]);
    packed[0].release_texts(&mut first);
    packed[1].release_texts(&mut second);
    db.conn
        .execute_batch(
            "CREATE TRIGGER reject_event BEFORE INSERT ON events WHEN new.name = 'reject'
            BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .unwrap();
    assert!(db.put(&packed).is_err());
    assert!(db.fingerprints().unwrap().is_empty());
    for (items, expected) in [(&first, ("入力", "結果")), (&second, ("second", "body"))] {
        let Loaded::Session(s) = &items[0] else {
            unreachable!()
        };
        let crate::model::EventText::Pending(i, o) = s.lanes[0].events[0].text() else {
            panic!("rollback must retain pending body")
        };
        assert_eq!((i.as_str(), o.as_str()), expected);
    }
    db.conn.execute_batch("DROP TRIGGER reject_event;").unwrap();
    db.put(&packed).unwrap();
    assert_eq!(db.fingerprints().unwrap().len(), 2);
    for items in [&first, &second] {
        let Loaded::Session(s) = &items[0] else {
            unreachable!()
        };
        assert!(matches!(
            s.lanes[0].events[0].text(),
            crate::model::EventText::Row(_)
        ));
    }
}

#[test]
fn summary_cache_preserves_collisions_versions_and_utf8_boundaries() {
    fn add(cache: &mut SummaryCache, events: &mut Vec<EventGroup>, text: &str) -> Arc<str> {
        if events.last().is_none_or(|g| g.events.len() == 7) {
            events.push(EventGroup {
                lane_id: events.len() as i64,
                events: Vec::new(),
            });
        }
        let summary = cache.get(
            text,
            (events.len() - 1, events.last().unwrap().events.len()),
            |g, i| events.get(g).and_then(|g| g.events.get(i)),
        );
        let Loaded::Session(mut session) = fixtures::items(&[("User", "", "")]).remove(0) else {
            unreachable!()
        };
        let mut event = session.lanes[0].events.remove(0);
        event.summary = summary.clone();
        events.last_mut().unwrap().events.push(event);
        summary
    }
    let mut cache = SummaryCache::new();
    let mut events = Vec::new();
    let key = "日本語の要約";
    let slot = |s: &str| SummaryCache::key(s) as usize & 1023;
    let collision = (0..30_000)
        .map(|n| format!("別の要約-{n}"))
        .find(|s| slot(s) == slot(key))
        .expect("衝突する要約が必要");
    let original = add(&mut cache, &mut events, key);
    assert!(Arc::ptr_eq(&original, &add(&mut cache, &mut events, key)));
    for lane_id in 10..18 {
        events.push(EventGroup {
            lane_id,
            events: Vec::new(),
        });
    }
    assert!(Arc::ptr_eq(&original, &add(&mut cache, &mut events, key)));
    let replaced = add(&mut cache, &mut events, &collision);
    assert_eq!(original.as_ref(), key);
    assert_eq!(replaced.as_ref(), collision);
    assert!(Arc::ptr_eq(
        &replaced,
        &add(&mut cache, &mut events, &collision)
    ));
    assert_eq!(add(&mut cache, &mut events, key).as_ref(), key);
    assert_eq!(replaced.as_ref(), collision);

    let sampled_a = "0123456789abcdef中間甲abcdefgh";
    let sampled_b = "0123456789abcdef中間乙abcdefgh";
    assert_eq!(SummaryCache::key(sampled_a), SummaryCache::key(sampled_b));
    let a = add(&mut cache, &mut events, sampled_a);
    let b = add(&mut cache, &mut events, sampled_b);
    assert_eq!(a.as_ref(), sampled_a);
    assert_eq!(b.as_ref(), sampled_b);
    assert_eq!(add(&mut cache, &mut events, sampled_a).as_ref(), sampled_a);

    let boundary = "あ".repeat(170) + "ab";
    assert_eq!(boundary.len(), 512);
    let shared = add(&mut cache, &mut events, &boundary);
    assert!(Arc::ptr_eq(
        &shared,
        &add(&mut cache, &mut events, &boundary)
    ));
    let longer = boundary + "é";
    let first = add(&mut cache, &mut events, &longer);
    let second = add(&mut cache, &mut events, &longer);
    assert_eq!(first.as_ref(), longer);
    assert_eq!(second.as_ref(), longer);
    assert!(!Arc::ptr_eq(&first, &second));
    for short in [
        "",
        "a",
        "1234567",
        "12345678",
        "123456789012345",
        "1234567890123456",
        "ééé",
        "日本",
    ] {
        let value = add(&mut cache, &mut events, short);
        assert!(Arc::ptr_eq(&value, &add(&mut cache, &mut events, short)));
    }
    events.clear();
    assert_eq!(add(&mut cache, &mut events, key).as_ref(), key);
    assert_eq!(std::mem::size_of::<SummaryCache>(), 16 * 1024);
    if usize::BITS > 32 {
        let large = (u32::MAX as u64 + 1) as usize;
        assert_eq!(
            cache
                .get("群の位置が大きい", (large, 0), |_, _| None)
                .as_ref(),
            "群の位置が大きい"
        );
        assert_eq!(
            cache
                .get("行の位置が大きい", (0, large), |_, _| None)
                .as_ref(),
            "行の位置が大きい"
        );
    }
}

#[test]
fn parallel_event_reads_handle_empty_and_sparse_ids() {
    let path = std::env::temp_dir().join(format!(
        "rireki-events-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE events (
            id INTEGER PRIMARY KEY, lane_id INTEGER, start REAL, end REAL, kind INTEGER,
            name TEXT, summary TEXT, call_id TEXT, u_in INTEGER, u_cr INTEGER, u_cw INTEGER,
            u_out INTEGER, u_rs INTEGER, u_cw1h INTEGER, u_model TEXT, status INTEGER
        )",
    )
    .unwrap();
    assert!(read_events(&conn).unwrap().is_empty());
    let ids = [-7, 0, 11, 12, 13, 4000, 900000];
    for &id in &ids {
        let name = if id == 13 { "独自ツール" } else { "User" };
        let summary = if id == 12 { "" } else { "日本語" };
        let call_id = match id {
            0 => Some(""),
            13 => Some("呼び出し-α"),
            _ => None,
        };
        conn.execute("INSERT INTO events VALUES (?1, 7, ?2, ?3, 0, ?4, ?5, ?6, 1, 2, 3, 4, 5, 6, 'gpt-6-sol', 0)",
            params![id, id as f64, id as f64 + 0.5, name, summary, call_id]).unwrap();
    }
    let parts = read_events(&conn).unwrap();
    assert!(parts.len() <= ids.len());
    let events: Vec<_> = parts
        .into_iter()
        .flat_map(|part| match part {
            EventRows::Grouped(groups) => groups
                .into_iter()
                .flat_map(|group| {
                    group
                        .events
                        .into_iter()
                        .map(move |event| (group.lane_id, event))
                })
                .collect::<Vec<_>>(),
            EventRows::Flat(rows) => rows,
        })
        .collect();
    assert_eq!(events.len(), ids.len());
    for ((lane, event), id) in events.iter().zip(ids) {
        assert_eq!(*lane, 7);
        assert_eq!(event.row(), Some(id));
        assert_eq!((event.start, event.end), (id as f64, id as f64 + 0.5));
        assert_eq!(event.kind, Kind::User);
        assert_eq!(
            event.name.as_ref(),
            if id == 13 { "独自ツール" } else { "User" }
        );
        assert_eq!(event.summary.as_ref(), if id == 12 { "" } else { "日本語" });
        assert_eq!(
            event.call_id.as_deref(),
            match id {
                0 => Some(""),
                13 => Some("呼び出し-α"),
                _ => None,
            }
        );
        assert_eq!(
            event.usage,
            Usage {
                input: 1,
                cache_read: 2,
                cache_write: 3,
                output: 4,
                reasoning: 5,
                cache_write_1h: 6
            }
        );
        assert_eq!(event.model.as_deref(), Some("gpt-6-sol"));
    }
    assert!(std::sync::Arc::ptr_eq(&events[0].1.name, &events[1].1.name));
    assert!(Arc::ptr_eq(&events[0].1.summary, &events[1].1.summary));
    drop(conn);
    remove_db(&path);
}

#[test]
fn interleaved_lane_rows_switch_to_flat_reads_and_keep_order() {
    let fixture = fixtures::Fixture::new();
    let mut db = fixture.open();
    let mut items = fixtures::items(&[]);
    let Loaded::Session(session) = &mut items[0] else {
        unreachable!()
    };
    let Loaded::Session(sample) = fixtures::items(&[("User", "入力", "結果")]).remove(0) else {
        unreachable!()
    };
    for lane_index in 0..2 {
        let mut lane = Lane::new(format!("lane-{lane_index}"));
        for i in 0..2048 {
            let mut event = sample.lanes[0].events[0].clone();
            event.start = i as f64;
            event.end = i as f64 + 0.5;
            event.summary = format!("{lane_index}:{i}").into();
            lane.events.push(event);
        }
        session.lanes.push(lane);
    }
    db.put(&pack_files(&[("file".into(), "fp".into(), &items)]))
        .unwrap();
    db.conn.execute("UPDATE events SET id=-id", []).unwrap();
    db.conn
        .execute(
            "UPDATE events SET id=CASE WHEN -id <= 2048 THEN (-id-1)*2+1 ELSE (-id-2049)*2+2 END",
            [],
        )
        .unwrap();
    assert!(
        read_events(&db.conn)
            .unwrap()
            .iter()
            .all(|part| matches!(part, EventRows::Flat(_)))
    );
    let files = db.load_all().unwrap();
    let Loaded::Session(session) = &files[0].1[0] else {
        unreachable!()
    };
    for (lane_index, lane) in session.lanes[1..].iter().enumerate() {
        assert_eq!(lane.events.len(), 2048);
        for (i, event) in lane.events.iter().enumerate() {
            assert_eq!(event.summary.as_ref(), format!("{lane_index}:{i}"));
            assert_eq!(event.row(), Some((i * 2 + lane_index + 1) as i64));
        }
    }
}

#[test]
fn load_all_keeps_lanes_and_event_order_across_sparse_ranges() {
    let fixture = fixtures::Fixture::new();
    let mut db = fixture.open();
    let mut items = fixtures::items(&[]);
    let Loaded::Session(s) = &mut items[0] else {
        unreachable!()
    };
    for (lane_index, count) in [37, 19, 0].into_iter().enumerate() {
        let mut lane = Lane::new(format!("lane-{lane_index}"));
        for i in 0..count {
            let mut event = fixtures::items(&[("User", "入力", "")]);
            let Loaded::Session(event) = event.remove(0) else {
                unreachable!()
            };
            let mut event = event.lanes[0].events[0].clone();
            event.summary = format!("{lane_index}:{i}").into();
            lane.events.push(event);
        }
        s.lanes.push(lane);
    }
    db.put(&pack_files(&[("file".into(), "fp".into(), &items)]))
        .unwrap();
    // レーンの行を混在させ、0・負値・離れたIDも含む範囲に配置する。
    db.conn
        .execute("UPDATE events SET id = id + 1000", [])
        .unwrap();
    let mut expected = vec![Vec::new(); 3];
    let mut old = 1001;
    for (lane_index, count) in [37, 19, 0].into_iter().enumerate() {
        for i in 0..count {
            let id = if i == count - 1 {
                900000 + lane_index as i64
            } else {
                i as i64 * 2 + lane_index as i64 - 7
            };
            db.conn
                .execute("UPDATE events SET id = ?1 WHERE id = ?2", params![id, old])
                .unwrap();
            expected[lane_index].push((id, format!("{lane_index}:{i}")));
            old += 1;
        }
    }
    // 孤立したレーンへの参照は、従来と同じく表示の対象にしない。
    db.conn
        .execute("UPDATE events SET lane_id = -1 WHERE id = -7", [])
        .unwrap();
    expected[0].remove(0);
    let files = db.load_all().unwrap();
    assert_eq!(files.len(), 1);
    let Loaded::Session(s) = &files[0].1[0] else {
        unreachable!()
    };
    assert_eq!(s.lanes.len(), 4); // 元の空のmainと3レーン
    assert!(s.lanes[0].events.is_empty());
    for (lane, expected) in s.lanes[1..].iter().zip(expected) {
        let actual: Vec<_> = lane
            .events
            .iter()
            .map(|e| (e.row().unwrap(), e.summary.to_string()))
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn detail_reads_preserve_contents_and_release_read_transaction() {
    let db = Db {
        conn: Connection::open_in_memory().unwrap(),
        saved: HashMap::new(),
        last_event_id: 0,
    };
    db.conn
        .execute_batch("CREATE TABLE texts (id INTEGER PRIMARY KEY, input BLOB, output BLOB)")
        .unwrap();
    let contents = [
        (-1, "日本語\n入力".to_string(), "結果\n".repeat(1000)),
        (5, String::new(), String::new()),
        (90123, "コマンド".into(), "出力".into()),
    ];
    for (id, input, output) in &contents {
        db.conn
            .execute(
                "INSERT INTO texts VALUES (?1, ?2, ?3)",
                params![id, pack(input), pack(output)],
            )
            .unwrap();
    }
    db.conn.pragma_update(None, "query_only", true).unwrap();
    let expected: HashMap<_, _> = contents
        .iter()
        .map(|(id, i, o)| (*id, (i.clone(), o.clone())))
        .collect();
    assert!(db.details(&[]).is_empty());
    assert_eq!(db.details(&[90123, 0, -1, 5, -1]), expected);
    assert!(db.conn.is_autocommit());
    // 呼び出し元が読み取りトランザクションを持つ場合も、終了させない。
    let tx = db.conn.unchecked_transaction().unwrap();
    assert_eq!(db.details(&[5, -1, 90123]), expected);
    assert!(!db.conn.is_autocommit());
    drop(tx);
    assert!(db.conn.is_autocommit());
    assert_eq!(db.details(&[90123, -1, 5]), expected);
}

#[test]
fn detail_batches_preserve_sparse_duplicate_and_missing_rows() {
    let db = Db {
        conn: Connection::open_in_memory().unwrap(),
        saved: HashMap::new(),
        last_event_id: 0,
    };
    db.conn
        .execute_batch("CREATE TABLE texts (id INTEGER PRIMARY KEY, input BLOB, output BLOB)")
        .unwrap();
    let mut contents = HashMap::new();
    let mut ids = Vec::new();
    for i in 0..145 {
        let id = i * 97 - 7;
        let input = format!("入力 {i}\n{}", "あ".repeat(i as usize));
        let output = format!("出力 {i}");
        db.conn
            .execute(
                "INSERT INTO texts VALUES (?1, ?2, ?3)",
                params![id, pack(&input), pack(&output)],
            )
            .unwrap();
        contents.insert(id, (input, output));
        ids.push(id);
    }
    db.conn
        .execute(
            "INSERT INTO texts VALUES (?1, ?2, ?3)",
            params![9999999, "invalid blob", pack("")],
        )
        .unwrap();
    db.conn.pragma_update(None, "query_only", true).unwrap();
    for count in [1, 12, 13, 59, 60, 61, 124, 125, 145] {
        let mut selected: Vec<_> = ids[..count].iter().copied().rev().collect();
        selected.extend([9999999, 99999, ids[0], ids[count - 1]]);
        let expected: HashMap<_, _> = selected
            .iter()
            .filter_map(|id| contents.get(id).map(|texts| (*id, texts.clone())))
            .collect();
        assert_eq!(db.details(&selected), expected);
        assert!(db.conn.is_autocommit());
    }
}

#[test]
fn pack_roundtrip() {
    for s in ["", "abc", &"日本語テキスト".repeat(500)] {
        assert_eq!(unpack(&pack(s)), s);
    }
}
