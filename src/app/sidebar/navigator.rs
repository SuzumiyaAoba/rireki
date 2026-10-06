//! ナビゲーターの行とスクロール位置。本文や旧セッションの版は保持しない。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{ListAlignment, ListOffset, ListState, px};

use super::NavRow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Key {
    pub(super) generation: u64,
    pub(super) limit: usize,
    pub(super) folded: [bool; 3],
    pub(super) width: u32,
}

struct Data {
    key: Key,
    rows: Rc<Vec<NavRow>>,
    projects: Vec<String>,
    expires: f64,
    built_at: f64,
}

pub(in crate::app) struct NavigatorList {
    pub(in crate::app) state: ListState,
    data: RefCell<Option<Data>>,
}

fn same_row(a: NavRow, a_projects: &[String], b: NavRow, b_projects: &[String]) -> bool {
    match (a, b) {
        (NavRow::Section(a, _), NavRow::Section(b, _)) => a == b,
        (NavRow::Project(a), NavRow::Project(b)) => {
            a_projects.get(a).zip(b_projects.get(b)).is_some_and(|(a, b)| a == b)
        }
        (NavRow::More(_), NavRow::More(_)) => true,
        _ => a == b,
    }
}

fn anchor(old: &[NavRow], old_projects: &[String], new: &[NavRow], new_projects: &[String], offset: ListOffset) -> ListOffset {
    if offset.item_ix == 0 && offset.offset_in_item == px(0.0) { return ListOffset::default(); }
    let Some(&row) = old.get(offset.item_ix) else { return ListOffset::default() };
    // 「さらに表示」の場所には追加された先頭セッションを置く。
    let more = matches!(row, NavRow::More(_)).then(|| {
        old.iter().rev().find(|r| matches!(r, NavRow::Session(_)))
            .and_then(|last| new.iter().position(|r| r == last))
            .and_then(|i| (i + 1 < new.len()).then_some(i + 1))
    }).flatten();
    let item_ix = more.or_else(|| new.iter().position(|&next| same_row(row, old_projects, next, new_projects)));
    item_ix.map(|item_ix| ListOffset { item_ix, ..offset }).unwrap_or_default()
}

impl NavigatorList {
    pub(in crate::app) fn new() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(100.0)),
            data: RefCell::new(None),
        }
    }

    pub(super) fn prepare(&self, key: Key, at: f64, build: impl FnOnce() -> (Vec<NavRow>, Vec<String>, f64)) {
        let mut data = self.data.borrow_mut();
        if data.as_ref().is_some_and(|d| d.key == key && d.built_at <= at && at < d.expires) { return; }
        let (rows, projects, expires) = build();
        let offset = data.as_ref().map(|old| {
            anchor(&old.rows, &old.projects, &rows, &projects, self.state.logical_scroll_top())
        }).unwrap_or_default();
        self.state.splice(0..self.state.item_count(), rows.len());
        self.state.scroll_to(offset);
        *data = Some(Data { key, rows: Rc::new(rows), projects, expires, built_at: at });
    }

    pub(super) fn row(&self, index: usize) -> Option<NavRow> {
        self.data.borrow().as_ref()?.rows.get(index).copied()
    }

    pub(in crate::app) fn rows(&self) -> Option<Rc<Vec<NavRow>>> {
        self.data.borrow().as_ref().map(|data| data.rows.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset(item_ix: usize) -> ListOffset { ListOffset { item_ix, offset_in_item: px(7.0) } }

    #[test]
    fn project_reordering_keeps_the_same_directory_and_pixel_offset() {
        let old = vec![NavRow::Section("projects", 2), NavRow::Project(0), NavRow::Project(1)];
        let new = vec![NavRow::Section("live", 1), NavRow::Live(4), NavRow::Section("projects", 2), NavRow::Project(0), NavRow::Project(1)];
        let next = anchor(&old, &["a".into(), "b".into()], &new, &["b".into(), "a".into()], offset(1));
        assert_eq!(next.item_ix, 4);
        assert_eq!(next.offset_in_item, px(7.0));
    }

    #[test]
    fn loading_more_keeps_the_first_added_session_at_the_button_position() {
        let old = [NavRow::Section("sessions", 5), NavRow::Session(0), NavRow::Session(1), NavRow::More(3)];
        let new = [NavRow::Section("sessions", 5), NavRow::Session(0), NavRow::Session(1), NavRow::Session(2), NavRow::Session(3), NavRow::More(1)];
        assert_eq!(anchor(&old, &[], &new, &[], offset(3)).item_ix, 3);
        let next = anchor(&old, &[], &new, &[], offset(0));
        assert_eq!(next.item_ix, 0);
        assert_eq!(next.offset_in_item, px(7.0));
    }

    #[test]
    fn live_and_recent_copies_of_the_same_session_are_distinct_anchors() {
        let old = [NavRow::Live(1), NavRow::Session(1)];
        let new = [NavRow::Live(1), NavRow::Session(2), NavRow::Session(1)];
        assert_eq!(anchor(&old, &[], &new, &[], offset(1)).item_ix, 2);
        assert_eq!(anchor(&old, &[], &new, &[], offset(0)).item_ix, 0);
        assert_eq!(anchor(&old, &[], &[], &[], offset(1)).offset_in_item, px(0.0));
    }

    #[test]
    fn unchanged_rows_are_shared_until_live_membership_expires() {
        let list = NavigatorList::new();
        let key = Key { generation: 1, limit: 50, folded: [false; 3], width: 320.0f32.to_bits() };
        list.prepare(key, 20.0, || (vec![NavRow::Live(1)], vec![], 30.0));
        let first = list.rows().unwrap();
        list.prepare(key, 29.9, || panic!("valid layout rebuilt"));
        assert!(Rc::ptr_eq(&first, &list.rows().unwrap()));
        list.prepare(key, 30.0, || (vec![NavRow::Section("sessions", 1)], vec![], f64::INFINITY));
        assert_eq!(&**list.rows().unwrap(), &[NavRow::Section("sessions", 1)]);
        assert!(!Rc::ptr_eq(&first, &list.rows().unwrap()));
    }

    #[test]
    fn scope_folding_limit_and_width_changes_rebuild_and_release_old_rows() {
        let list = NavigatorList::new();
        let key = Key { generation: 1, limit: 50, folded: [false; 3], width: 320.0f32.to_bits() };
        for changed in [
            Key { generation: 2, ..key }, Key { limit: 150, ..key },
            Key { folded: [true, false, false], ..key },
            Key { folded: [false, true, false], ..key },
            Key { folded: [false, false, true], ..key },
            Key { width: 400.0f32.to_bits(), ..key },
        ] {
            list.prepare(key, 20.0, || (vec![NavRow::Session(1)], vec![], f64::INFINITY));
            let first = Rc::downgrade(&list.rows().unwrap());
            list.prepare(changed, 20.0, || (vec![], vec![], f64::INFINITY));
            assert!(first.upgrade().is_none());
            assert_eq!(list.state.item_count(), 0);
        }
    }

    #[test]
    fn moving_the_clock_back_rechecks_live_membership() {
        let list = NavigatorList::new();
        let key = Key { generation: 1, limit: 50, folded: [false; 3], width: 320.0f32.to_bits() };
        list.prepare(key, 40.0, || (vec![NavRow::Session(1)], vec![], f64::INFINITY));
        list.prepare(key, 20.0, || (vec![NavRow::Live(1), NavRow::Session(1)], vec![], 30.0));
        assert_eq!(&**list.rows().unwrap(), &[NavRow::Live(1), NavRow::Session(1)]);
    }

    #[test]
    fn the_top_stays_at_the_top_when_a_live_section_is_added() {
        let old = [NavRow::Section("projects", 1), NavRow::Project(0)];
        let new = [NavRow::Section("live", 1), NavRow::Live(1), NavRow::Section("projects", 1), NavRow::Project(0)];
        let next = anchor(&old, &["a".into()], &new, &["a".into()], ListOffset::default());
        assert_eq!(next.item_ix, 0);
        assert_eq!(next.offset_in_item, px(0.0));
    }
}
