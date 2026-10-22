//! 本文読み込みと集計の順序・キャッシュ・キャンセルを管理する。

use std::collections::HashMap;

use crate::db::{Db, JsCache};
use crate::model::{Session, truncate};

use super::content::{DetailEvent, format_input, shell_commands};
use super::files::{FileHistory, is_edit};
use super::summary::{SelectionSummary, SessionCache};
use super::{DetailCache, EVENT_LIMIT, Selection};

/// 本文の読み取り接続と JS・集計のキャッシュ。GUI のタスク制御には依存しない。
#[derive(Default)]
pub struct DetailLoader {
    reader: Option<Db>,
    js_cache: Option<JsCache>,
    summaries: SessionCache,
}

impl DetailLoader {
    pub fn prepare(&mut self) {
        if self.reader.is_none() {
            self.reader = Db::open_read().ok();
        }
        if self.js_cache.is_none() {
            self.js_cache = JsCache::open().ok();
        }
    }

    fn load(
        &self,
        sel: Selection,
        events: Vec<DetailEvent>,
        count: usize,
        is_current: impl Fn() -> bool,
    ) -> Option<DetailCache> {
        if !is_current() {
            return None;
        }
        let limit = if count == 1 { 6000 } else { 400 };
        let shown = count.min(EVENT_LIMIT);
        let rows: Vec<i64> = events.iter().filter_map(|e| e.row).collect();
        let mut fetched = self
            .reader
            .as_ref()
            .map(|db| db.details(&rows))
            .unwrap_or_default();
        let mut d = DetailCache {
            sel,
            texts: Vec::new(),
            raws: Vec::new(),
            touched: Vec::new(),
            out_lines: Vec::new(),
            commands: Vec::new(),
            files: Vec::new(),
            patches: HashMap::new(),
            selection_summary: SelectionSummary::default(),
            session_summary: std::sync::Arc::default(),
        };
        let mut history = FileHistory::default();
        for e in events {
            if !is_current() {
                return None;
            }
            let i = e.index;
            let edit = e.edit;
            let (input, output) = e.text(&mut fetched);
            let (input, raw) = format_input(input, self.js_cache.as_ref());
            let touched = history.record(i, &input, edit || raw.is_some());
            if i < shown {
                d.out_lines.push(output.lines().count());
                d.commands.push(
                    shell_commands(&input)
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                );
                d.raws.push(raw);
                d.touched.push(touched);
                d.texts
                    .push((truncate(&input, limit), truncate(&output, limit)));
            }
        }
        (d.files, d.patches) = history.finish();
        is_current().then_some(d)
    }

    pub fn load_session(
        &mut self,
        sel: Selection,
        session: &std::sync::Arc<Session>,
        is_current: impl Fn() -> bool,
    ) -> Option<DetailCache> {
        if sel.first > sel.last {
            return None;
        }
        let evs = session
            .lanes
            .get(sel.lane)?
            .events
            .get(sel.first..=sel.last)?;
        let shown = evs.len().min(EVENT_LIMIT);
        let mut events = Vec::new();
        for (ci, chunk) in evs.chunks(256).enumerate() {
            if !is_current() {
                return None;
            }
            for (offset, e) in chunk.iter().enumerate() {
                let index = ci * 256 + offset;
                let edit = is_edit(e);
                if index < shown || edit {
                    events.push(DetailEvent::capture(index, e, edit));
                }
            }
        }
        let selection_summary = SelectionSummary::build(evs, &is_current)?;
        let session_summary = self.summaries.get(session, &is_current)?;
        let mut detail = self.load(sel, events, evs.len(), &is_current)?;
        detail.selection_summary = selection_summary;
        detail.session_summary = session_summary;
        Some(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Event, EventText, Kind, Loaded, Source, Status};

    #[test]
    fn overwritten_texts_keep_details_captured_before_commit() {
        let fixture = crate::db::fixtures::Fixture::new();
        let mut db = fixture.open();
        let mut items = crate::db::fixtures::items(&[("tool", "旧入力", "旧結果")]);
        let first = crate::db::pack_files(&[("file".into(), "1".into(), &items)]);
        first[0].release_texts(&mut items);
        db.put(&first).unwrap();
        drop(first);
        let Loaded::Session(old) = items.remove(0) else {
            unreachable!()
        };
        let old = std::sync::Arc::new(old);
        let e = &old.lanes[0].events[0];
        let EventText::Row(id) = e.text() else {
            unreachable!()
        };
        let captured = DetailEvent {
            index: 0,
            row: Some(id),
            text_ref: e.text_ref.clone(),
            input: String::new(),
            output: String::new(),
            edit: false,
        };
        let new = crate::db::fixtures::items(&[("tool", "new input", "new output")]);
        db.put(&crate::db::pack_files(&[("file".into(), "2".into(), &new)]))
            .unwrap();
        let mut loader = DetailLoader {
            reader: Some(Db::open_index_read(&fixture.texts).unwrap()),
            ..Default::default()
        };
        let sel = Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: 0,
        };
        let captured = loader.load(sel, vec![captured], 1, || true).unwrap();
        let snapshot = loader.load_session(sel, &old, || true).unwrap();
        assert_eq!(captured.texts, vec![("旧入力".into(), "旧結果".into())]);
        assert_eq!(snapshot.texts, captured.texts);
        assert_eq!(snapshot.out_lines, captured.out_lines);
    }

    #[test]
    fn pending_and_committed_texts_match_inline_details_including_late_edits() {
        let fixture = crate::db::fixtures::Fixture::new();
        let mut db = fixture.open();
        let values: Vec<_> = (0..EVENT_LIMIT + 2)
            .map(|i| {
                if i == EVENT_LIMIT + 1 {
                    (
                        "apply_patch".to_string(),
                        "*** Update File: src/a.rs\n@@\n-old\n+日本語".to_string(),
                        String::new(),
                    )
                } else {
                    (
                        "exec_command".into(),
                        format!("$ cargo test {i}"),
                        format!("結果 {i}\n"),
                    )
                }
            })
            .collect();
        let refs: Vec<_> = values
            .iter()
            .map(|(n, i, o)| (n.as_str(), i.as_str(), o.as_str()))
            .collect();
        let mut items = crate::db::fixtures::items(&refs);
        let Loaded::Session(inline) = &items[0] else {
            unreachable!()
        };
        let sel = Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: EVENT_LIMIT + 1,
        };
        let reference = DetailLoader::default()
            .load_session(sel, &std::sync::Arc::new(inline.clone()), || true)
            .unwrap();
        let packed = crate::db::pack_files(&[("file".into(), "fp".into(), &items)]);
        packed[0].release_texts(&mut items);
        let Loaded::Session(s) = items.remove(0) else {
            unreachable!()
        };
        let snapshot = std::sync::Arc::new(s);
        let mut loader = DetailLoader {
            reader: Some(Db::open_index_read(&fixture.texts).unwrap()),
            ..Default::default()
        };
        let pending = loader.load_session(sel, &snapshot, || true).unwrap();
        db.put(&packed).unwrap();
        drop(packed);
        let saved = loader.load_session(sel, &snapshot, || true).unwrap();
        for actual in [pending, saved] {
            assert_eq!(actual.texts, reference.texts);
            assert_eq!(actual.out_lines, reference.out_lines);
            assert_eq!(actual.commands, reference.commands);
            assert_eq!(actual.touched, reference.touched);
            assert_eq!(actual.patches, reference.patches);
            assert_eq!(
                (actual.files[0].path.as_str(), &actual.files[0].events),
                ("src/a.rs", &vec![EVENT_LIMIT + 1])
            );
            assert_eq!(
                actual.selection_summary.total.stats,
                reference.selection_summary.total.stats
            );
            assert_eq!(
                actual.selection_summary.total.usage,
                reference.selection_summary.total.usage
            );
        }
        assert_eq!(
            crate::model::pending_text_bytes(&snapshot.lanes[0].events),
            0
        );
    }

    fn list_event(name: &str, status: Status) -> Event {
        Event {
            start: 1.0,
            end: 2.0,
            kind: Kind::Tool,
            name: name.into(),
            summary: Default::default(),
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: Default::default(),
            usage: Default::default(),
            model: None,
            status,
        }
    }

    #[test]
    fn detail_keeps_edits_after_display_limit_and_commands() {
        let sel = Selection {
            session: 0,
            lane: 0,
            first: 10,
            last: EVENT_LIMIT + 10,
        };
        let events = vec![
            DetailEvent {
                index: 0,
                row: None,
                text_ref: Default::default(),
                input: "$ cargo test".into(),
                output: "one\ntwo".into(),
                edit: false,
            },
            DetailEvent {
                index: EVENT_LIMIT,
                row: None,
                text_ref: Default::default(),
                input: "*** Update File: src/a.rs\n@@\n-old\n+new".into(),
                output: String::new(),
                edit: true,
            },
        ];
        let d = DetailLoader::default()
            .load(sel, events, EVENT_LIMIT + 1, || true)
            .unwrap();
        assert_eq!(d.commands, [vec!["cargo test".to_string()]]);
        assert_eq!(d.out_lines, [2]);
        assert_eq!(d.files.len(), 1);
        assert_eq!(
            (&*d.files[0].path, d.files[0].added, d.files[0].removed),
            ("src/a.rs", 1, 1)
        );
        assert_eq!(d.files[0].events, [EVENT_LIMIT]);
        assert!(d.patches.contains_key(&EVENT_LIMIT));
    }

    #[test]
    fn detail_cancels_before_work_and_between_events() {
        let sel = Selection {
            session: 0,
            lane: 0,
            first: 0,
            last: 1,
        };
        assert!(
            DetailLoader::default()
                .load(sel, Vec::new(), 2, || false)
                .is_none()
        );
        let checks = std::cell::Cell::new(0);
        let events = (0..2)
            .map(|index| DetailEvent {
                index,
                row: None,
                text_ref: Default::default(),
                input: "test".into(),
                output: String::new(),
                edit: false,
            })
            .collect();
        let result = DetailLoader::default().load(sel, events, 2, || {
            checks.set(checks.get() + 1);
            checks.get() <= 2
        });
        assert!(result.is_none());
        assert_eq!(checks.get(), 3);
    }

    #[test]
    fn background_details_keep_full_summaries_edits_and_snapshot_versions() {
        use crate::model::Lane;
        use std::sync::Arc;
        let mut lane = Lane::new("main");
        lane.events = (0..400)
            .map(|i| {
                let mut e = list_event("Read", Status::Ok);
                e.start = i as f64;
                e.end = i as f64 + 0.5;
                e.usage.input = 100;
                e
            })
            .collect();
        lane.events[310].name = "Edit".into();
        lane.events[310].input = "*** Update File: src/a.rs\n@@\n-old\n+new".into();
        let mut session = Arc::new(Session {
            id: "background".into(),
            source: Source::Codex,
            cwd: String::new(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 400.0,
            parents: Vec::new(),
            tree: Vec::new(),
        });
        let mut loader = DetailLoader::default();
        let sel = Selection {
            session: 0,
            lane: 0,
            first: 10,
            last: 310,
        };
        let detail = loader.load_session(sel, &session, || true).unwrap();
        assert_eq!(detail.selection_summary.total.stats.tools, 301);
        assert_eq!(detail.selection_summary.total.usage.total(), 30_100);
        assert_eq!(detail.session_summary.total.usage.total(), 40_000);
        assert_eq!(detail.files[0].events, [EVENT_LIMIT]);
        assert_eq!(detail.texts.len(), EVENT_LIMIT);
        let next = loader
            .load_session(
                Selection {
                    first: 0,
                    last: 0,
                    ..sel
                },
                &session,
                || true,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&detail.session_summary, &next.session_summary));
        let old = session.clone();
        Arc::make_mut(&mut session).lanes[0].events[0].status = Status::Error;
        let updated = loader.load_session(sel, &session, || true).unwrap();
        assert_eq!(updated.session_summary.total.stats.errors, 1);
        assert_eq!(detail.session_summary.total.stats.errors, 0);
        assert_eq!(old.lanes[0].events[0].status, Status::Ok);
        assert!(!Arc::ptr_eq(
            &detail.session_summary,
            &updated.session_summary
        ));
        for invalid in [
            Selection {
                lane: usize::MAX,
                ..sel
            },
            Selection {
                first: 311,
                last: 310,
                ..sel
            },
            Selection { last: 400, ..sel },
        ] {
            assert!(loader.load_session(invalid, &session, || true).is_none());
        }
    }
}
