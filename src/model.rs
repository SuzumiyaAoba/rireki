//! GUI と保存形式に依存しない履歴モデル。公開型はこのモジュールから再公開する。

mod event;
mod hashing;
mod session;
mod store;
mod usage;

pub use event::{Event, Kind, Source, Stats, Status, pending_text_bytes};
pub use hashing::SessionIndexHasher;
pub(crate) use hashing::mix_index;
pub use session::{Lane, Session};
pub use store::{Loaded, Snapshot, Store};
pub use usage::{Usage, UsageSum};
pub use crate::text::{EventText, TextRef};

#[cfg(test)]
use std::sync::Arc;

pub fn truncate(s: &str, max: usize) -> String {
    let Some((end, _)) = s.char_indices().nth(max) else {
        return s.to_string();
    };
    let mut out = String::with_capacity(end + '…'.len_utf8());
    out.push_str(&s[..end]);
    out.push('…');
    out
}

pub fn one_line(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max.saturating_mul(4).saturating_add(3)));
    let mut count = 0;
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space |= !out.is_empty();
            continue;
        }
        if space {
            if count == max {
                out.push('…');
                return out;
            }
            out.push(' ');
            count += 1;
            space = false;
        }
        if count == max {
            out.push('…');
            return out;
        }
        out.push(c);
        count += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_rate_distinguishes_missing_input_and_cache_misses() {
        for (usage, expected) in [
            (Usage::default(), None),
            (Usage { output: 100, reasoning: 50, ..Default::default() }, None),
            (Usage { input: 100, ..Default::default() }, Some(0.0)),
            (Usage { cache_write: 100, cache_write_1h: 100, ..Default::default() }, Some(0.0)),
            (Usage { cache_read: 100, ..Default::default() }, Some(1.0)),
            (Usage { input: 20, cache_read: 60, cache_write: 20, cache_write_1h: 10,
                output: 100, reasoning: 50 }, Some(0.6)),
            (Usage { input: u32::MAX, cache_read: u32::MAX, cache_write: u32::MAX,
                ..Default::default() }, Some(1.0 / 3.0)),
        ] {
            assert_eq!(usage.cache_hit_rate(), expected);
            let mut sum = UsageSum::default();
            sum.add_priced(&usage, None);
            assert_eq!(sum.cache_hit_rate(), expected);
        }
    }

    #[test]
    fn cache_hit_rate_weights_calls_by_input_tokens_when_merging() {
        let mut main = UsageSum::default();
        main.add_priced(&Usage { cache_read: 100, output: 10, ..Default::default() }, None);
        let mut child = UsageSum::default();
        child.add_priced(&Usage { input: 700, cache_write: 200, output: 1000,
            ..Default::default() }, None);
        main.merge(&child);
        assert_eq!(main.cache_hit_rate(), Some(0.1));
    }

    #[test]
    fn metadata_versions_share_text_and_preserve_replaced_values() {
        let mut original = test_lane("exec_command", 1.0).events.remove(0);
        original.summary = "日本語の要約".into();
        original.call_id = Some("call-α".into());
        let metadata = original.clone_meta();
        let full = original.clone();
        assert!(Arc::ptr_eq(&original.summary, &metadata.summary));
        assert!(Arc::ptr_eq(original.call_id.as_ref().unwrap(), metadata.call_id.as_ref().unwrap()));
        assert!(Arc::ptr_eq(&original.summary, &full.summary));
        assert!(metadata.input.is_empty() && metadata.output.is_empty());
        assert_eq!(full.input, original.input);
        original.summary = "更新後".into();
        original.call_id = None;
        drop(original);
        assert_eq!(metadata.summary.as_ref(), "日本語の要約");
        assert_eq!(metadata.call_id.as_deref(), Some("call-α"));
        assert_eq!(full.summary, metadata.summary);
        assert_eq!(full.call_id, metadata.call_id);
    }

    #[test]
    fn tool_end_prefix_ignores_other_kinds_and_rebuilds_after_changes() {
        let mut session = test_session();
        let template = session.lanes[0].events[0].clone();
        session.lanes[0].events = vec![
            Event { start: 0.0, end: 1000.0, kind: Kind::Agent, ..template.clone() },
            Event { start: 1.0, end: 5.0, ..template.clone() },
            Event { start: 2.0, end: 2000.0, kind: Kind::Assistant, ..template.clone() },
            Event { start: 3.0, end: 4.0, ..template.clone() },
        ];
        session.finalize();
        assert_eq!(session.lanes[0].cum_end, [1000.0, 1000.0, 2000.0, 2000.0]);
        assert_eq!(session.lanes[0].cum_tool_end, [f64::NEG_INFINITY, 5.0, 5.0, 5.0]);
        session.lanes[0].events[1].kind = Kind::Thinking;
        session.finalize();
        assert_eq!(session.lanes[0].cum_tool_end, [f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY, 4.0]);
        session.lanes[0].events.clear();
        session.finalize();
        assert!(session.lanes[0].cum_end.is_empty() && session.lanes[0].cum_tool_end.is_empty());
        session.lanes[0].events.push(template);
        session.finalize();
        assert_eq!(session.lanes[0].cum_tool_end, [2.0]);
    }

    fn test_lane(name: &str, start: f64) -> Lane {
        let mut lane = Lane::new(name);
        lane.events.push(Event {
            start, end: start + 1.0, kind: Kind::Tool, name: name.into(), summary: name.into(),
            input: format!("input-{name}"), output: format!("output-{name}"), call_id: None,
            text_ref: Default::default(), usage: Default::default(), model: None, status: Default::default(),
        });
        lane
    }

    fn test_session() -> Session {
        Session {
            id: "s".into(), source: Source::Claude, cwd: "project".into(), headless: false,
            title: "initial".into(), branch: "main".into(), version: "1".into(),
            lanes: vec![test_lane("main", 1.0)], start: 0.0, end: 0.0, parents: Vec::new(), tree: Vec::new(),
        }
    }

    #[test]
    fn store_updates_leave_shared_snapshots_and_other_origins_intact() {
        let mut store = Store::default();
        assert_eq!(store.replace([("parent".into(), vec![Loaded::Session(test_session())])]), [0]);
        let initial = store.snapshot();
        assert_eq!(store.replace([("child".into(), vec![Loaded::Child("s".into(), test_lane("child", 3.0))])]), [0]);
        let with_child = store.snapshot();
        assert_eq!(initial.sessions[0].lanes.len(), 1);
        assert_eq!(with_child.sessions[0].lanes.len(), 2);
        let mut updated = test_session();
        updated.headless = true;
        updated.title = "updated".into();
        updated.lanes = vec![test_lane("new-main", 5.0)];
        store.replace([("parent".into(), vec![Loaded::Session(updated)])]);
        assert_eq!(with_child.sessions[0].title, "initial");
        assert!(!with_child.sessions[0].headless);
        assert_eq!(with_child.sessions[0].lanes[0].events[0].input, "input-main");
        let current = &store.sessions[0];
        assert_eq!(current.title, "updated");
        assert!(current.headless);
        assert_eq!(current.lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["new-main", "child"]);
        assert_eq!((current.start, current.end), (3.0, 6.0));
        store.replace([("child".into(), Vec::new())]);
        assert_eq!(store.sessions[0].lanes.len(), 1);
        assert_eq!(with_child.sessions[0].lanes.len(), 2);
        assert_eq!((store.sessions[0].start, store.sessions[0].end), (5.0, 6.0));
        store.replace([("parent".into(), Vec::new())]);
        assert!(!store.visible(0));
        assert!(initial.visible(0));
    }

    #[test]
    fn orphan_replacement_is_attached_once_and_keeps_its_origin() {
        let mut store = Store::default();
        assert!(store.replace([("child".into(), vec![Loaded::Child("s".into(), test_lane("old", 0.0))])]).is_empty());
        store.replace([("child".into(), vec![Loaded::Child("s".into(), test_lane("child", 3.0))])]);
        store.replace([("parent".into(), vec![Loaded::Session(test_session())])]);
        let snapshot = store.snapshot();
        assert_eq!(store.sessions[0].lanes.len(), 2);
        assert_eq!(store.sessions[0].lanes[1].origin, "child");
        store.replace([("child".into(), Vec::new())]);
        assert_eq!(store.sessions[0].lanes.len(), 1);
        assert_eq!(snapshot.sessions[0].lanes[1].events[0].input, "input-child");
    }

    #[test]
    fn finalize_preserves_nested_parents_spawn_times_and_duplicate_ids() {
        let event = |start, id: &str| Event {
            start, end: start + 1.0, kind: Kind::Agent, name: "Agent".into(), summary: Default::default(),
            input: String::new(), output: String::new(), call_id: Some(id.into()), text_ref: Default::default(),
            usage: Default::default(), model: None, status: Default::default(),
        };
        let mut main = Lane::new("main");
        main.events = vec![event(1.0, "child"), event(2.0, "child")];
        let mut child = Lane::new("child");
        child.spawned_by = Some("child".into());
        child.events = vec![event(3.0, "grandchild")];
        let mut grandchild = Lane::new("grandchild");
        grandchild.spawned_by = Some("grandchild".into());
        grandchild.events = vec![event(4.0, "self")];
        let mut detached = Lane::new("detached");
        detached.spawned_by = Some("missing".into());
        detached.events = vec![event(5.0, "detached")];
        let mut session = Session {
            id: "s".into(), source: Source::Claude, cwd: String::new(), headless: false,
            title: String::new(), branch: String::new(), version: String::new(),
            lanes: vec![main, child, grandchild, detached], start: 0.0, end: 0.0, parents: Vec::new(), tree: Vec::new(),
        };
        session.finalize();
        assert_eq!(session.parents, [None, Some(0), Some(1), Some(0)]);
        assert_eq!(session.lanes.iter().map(|l| l.spawn_t).collect::<Vec<_>>(), [None, Some(1.0), Some(3.0), None]);
        assert_eq!(session.tree, [(0, 0), (1, 1), (2, 2), (3, 1)]);
        assert_eq!((session.start, session.end), (1.0, 6.0));
        session.lanes[0].events[0].start = 0.5;
        session.finalize();
        assert_eq!(session.lanes[1].spawn_t, Some(0.5));
        assert_eq!(session.start, 0.5);
    }

    #[test]
    fn bounded_text_matches_full_normalization() {
        for text in [
            "",
            " \t\n",
            "a",
            " a  b \n",
            "日本語\u{3000}abc\u{a0}終わり",
            "🦀é e\u{301}\t\r\n次",
            "abcd     ",
        ] {
            for max in 0..20 {
                let reference = |s: &str| {
                    let mut out: String = s.chars().take(max).collect();
                    if s.chars().count() > max {
                        out.push('…');
                    }
                    out
                };
                assert_eq!(truncate(text, max), reference(text));
                let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
                assert_eq!(
                    one_line(text, max),
                    reference(&joined),
                    "text={text:?}, max={max}"
                );
            }
        }
    }
}
