//! セッション全体の更新と、表示中の選択範囲の更新を区別する。

use crate::detail::{DetailCache, Selection};
use crate::model::{Event, Kind, Session};

/// 長い選択範囲もあるため、メタデータの照合は読み込みタスクの背景スレッドで行う。
pub(super) fn same_selection(before: &Session, after: &Session, sel: Selection) -> bool {
    let Some(a) = before.lanes.get(sel.lane) else {
        return false;
    };
    let Some(b) = after.lanes.get(sel.lane) else {
        return false;
    };
    let (Some(a_events), Some(b_events)) = (
        a.events.get(sel.first..=sel.last),
        b.events.get(sel.first..=sel.last),
    ) else {
        return false;
    };
    if a.name != b.name
        || a.model != b.model
        || a.effort != b.effort
        || a.spawned_by != b.spawned_by
        || !a_events.iter().zip(b_events).all(|(a, b)| same_event(a, b))
    {
        return false;
    }
    // エージェント起動の本文には、子の件数・時間・状態を示すボタンも含まれる。
    if a_events.iter().any(|event| event.kind == Kind::Agent) {
        return before.lanes.len() == after.lanes.len()
            && before.lanes.iter().zip(&after.lanes).all(|(a, b)| {
                a.name == b.name
                    && a.spawned_by == b.spawned_by
                    && a.range() == b.range()
                    && a.events.len() == b.events.len()
                    && crate::model::Stats::of(&a.events) == crate::model::Stats::of(&b.events)
            });
    }
    true
}

fn same_event(a: &Event, b: &Event) -> bool {
    a.start == b.start
        && a.end == b.end
        && a.kind == b.kind
        && a.status == b.status
        && a.name == b.name
        && a.summary == b.summary
        && a.usage == b.usage
        && a.model == b.model
        && a.call_id == b.call_id
        && a.input == b.input
        && a.output == b.output
}

/// DB の本文差し替えや、表示上限を超えた編集の変更も見落とさない。
pub(super) fn same_content(a: &DetailCache, b: &DetailCache) -> bool {
    a.sel == b.sel
        && a.texts == b.texts
        && a.raws == b.raws
        && a.touched == b.touched
        && a.out_lines == b.out_lines
        && a.commands == b.commands
        && a.patches == b.patches
        && a.files.len() == b.files.len()
        && a.files.iter().zip(&b.files).all(|(a, b)| {
            a.path == b.path
                && a.edits == b.edits
                && a.added == b.added
                && a.removed == b.removed
                && a.events == b.events
        })
        && a.selection_summary.total.stats == b.selection_summary.total.stats
        && a.selection_summary.total.usage == b.selection_summary.total.usage
        && a.selection_summary.names == b.selection_summary.names
        && a.selection_summary.end == b.selection_summary.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::DetailLoader;
    use crate::model::{Lane, Source, Status};
    use std::sync::Arc;

    fn session() -> Session {
        let mut lane = Lane::new("main");
        lane.events = (0..4)
            .map(|i| Event {
                start: i as f64,
                end: i as f64 + 0.5,
                kind: Kind::Tool,
                name: "Read".into(),
                summary: "summary".into(),
                input: "input".into(),
                output: "output".into(),
                call_id: None,
                text_ref: Default::default(),
                usage: Default::default(),
                model: None,
                status: Status::Ok,
            })
            .collect();
        let mut session = Session {
            id: "detail-update".into(),
            source: Source::Codex,
            cwd: "project".into(),
            headless: false,
            title: "title".into(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        session
    }

    fn selection() -> Selection {
        Selection {
            session: 0,
            lane: 0,
            first: 1,
            last: 2,
        }
    }

    #[test]
    fn selection_ignores_appends_and_updates_outside_the_selected_range() {
        let before = session();
        let mut after = before.clone();
        after.lanes[0].events[3].status = Status::Error;
        let appended = after.lanes[0].events[0].clone();
        after.lanes[0].events.push(appended);
        after.title = "new title".into();
        after.finalize();
        assert!(same_selection(&before, &after, selection()));
        let mut loader = DetailLoader::default();
        let old = loader
            .load_session(selection(), &Arc::new(before), || true)
            .unwrap();
        let new = loader
            .load_session(selection(), &Arc::new(after), || true)
            .unwrap();
        assert!(same_content(&old, &new));
        assert_ne!(old.session_summary.total.stats, new.session_summary.total.stats);
    }

    #[test]
    fn selection_detects_status_text_timing_usage_and_model_updates() {
        let before = session();
        let edits: [fn(&mut Event); 7] = [
            |e| e.status = Status::Error,
            |e| e.output = "changed".into(),
            |e| e.end += 1.0,
            |e| e.usage.output = 1,
            |e| e.model = Some("model".into()),
            |e| e.summary = "changed".into(),
            |e| e.call_id = Some("call".into()),
        ];
        for edit in edits {
            let mut after = before.clone();
            edit(&mut after.lanes[0].events[1]);
            assert!(!same_selection(&before, &after, selection()));
        }
        let mut after = before.clone();
        after.lanes[0].events.truncate(2);
        assert!(!same_selection(&before, &after, selection()));
    }

    #[test]
    fn content_detects_reloaded_bodies_and_edits_beyond_the_display_limit() {
        let sel = selection();
        let source = Arc::new(session());
        let mut loader = DetailLoader::default();
        let old = loader.load_session(sel, &source, || true).unwrap();
        let mut new = loader.load_session(sel, &source, || true).unwrap();
        assert!(same_content(&old, &new));
        new.texts[0].1 = "reloaded body".into();
        assert!(!same_content(&old, &new));
        new.texts = old.texts.clone();
        new.files.push(crate::detail::FileStat {
            path: "late.rs".into(),
            edits: 1,
            added: 1,
            removed: 0,
            events: vec![crate::detail::EVENT_LIMIT + 1],
        });
        assert!(!same_content(&old, &new));
    }
}
