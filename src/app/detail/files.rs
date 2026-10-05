use super::{DetailCache, Selection};
use gpui::{ListAlignment, ListOffset, ListState, px};
use std::cell::{Cell, RefCell};

pub(super) const FILE_MAX: usize = 100;
pub(super) const PER_EDIT: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileRow {
    Summary,
    File(usize),
    History(usize),
    Edit { file: usize, event: usize },
    Empty(usize),
    Expand(usize),
    More,
}

impl FileRow {
    fn file(self) -> Option<usize> {
        match self {
            Self::Summary | Self::More => None,
            Self::File(i) | Self::History(i) | Self::Empty(i) | Self::Expand(i) | Self::Edit { file: i, .. } => Some(i),
        }
    }

    fn with_file(self, file: usize) -> Self {
        match self {
            Self::File(_) => Self::File(file),
            Self::History(_) => Self::History(file),
            Self::Edit { event, .. } => Self::Edit { file, event },
            Self::Empty(_) => Self::Empty(file),
            Self::Expand(_) => Self::Expand(file),
            row => row,
        }
    }
}

#[derive(PartialEq)]
pub(super) struct FileListKey {
    pub sel: Selection,
    pub generation: u64,
    pub width: u32,
    pub open: Option<String>,
    pub all_files: bool,
    pub all_diffs: bool,
}

struct FileListData {
    key: FileListKey,
    paths: Vec<String>,
    rows: Vec<FileRow>,
}

fn rows(cache: &DetailCache, open: Option<&str>, all_files: bool, all_diffs: bool) -> Vec<FileRow> {
    let mut rows = vec![FileRow::Summary];
    for (file, f) in cache
        .files
        .iter()
        .enumerate()
        .take(if all_files { usize::MAX } else { FILE_MAX })
    {
        rows.push(FileRow::File(file));
        if open != Some(f.path.as_str()) {
            continue;
        }
        rows.push(FileRow::History(file));
        let mut edits = 0;
        let mut hidden = 0;
        for &event in &f.events {
            let count: usize = cache
                .patches
                .get(&event)
                .into_iter()
                .flatten()
                .filter(|p| p.path == f.path)
                .map(|p| p.rows.len())
                .sum();
            // 空の差分でも見出しのある編集は表示する
            let present = cache
                .patches
                .get(&event)
                .is_some_and(|ps| ps.iter().any(|p| p.path == f.path));
            if !present {
                continue;
            }
            rows.push(FileRow::Edit { file, event });
            edits += 1;
            hidden += count.saturating_sub(PER_EDIT);
        }
        if edits == 0 {
            rows.push(FileRow::Empty(file));
        }
        if !all_diffs && hidden > 0 {
            rows.push(FileRow::Expand(file));
        }
    }
    if !all_files && cache.files.len() > FILE_MAX {
        rows.push(FileRow::More);
    }
    rows
}

fn anchor(old: &FileListData, paths: &[String], rows: &[FileRow], offset: ListOffset) -> ListOffset {
    let Some(mut row) = old.rows.get(offset.item_ix).copied() else {
        return ListOffset::default();
    };
    if let Some(file) = row.file() {
        let Some(path) = old.paths.get(file) else {
            return ListOffset::default();
        };
        let Some(file) = paths.iter().position(|p| p == path) else {
            return ListOffset::default();
        };
        row = row.with_file(file);
    }
    rows.iter()
        .position(|&next| next == row)
        .or_else(|| {
            (row == FileRow::More)
                .then(|| rows.iter().position(|&next| next == FileRow::File(FILE_MAX)))
                .flatten()
        })
        .map(|item_ix| ListOffset { item_ix, ..offset })
        .unwrap_or_default()
}

pub(in crate::app) struct FileList {
    pub(in crate::app) state: ListState,
    pub(in crate::app) rendered: Cell<usize>,
    data: RefCell<Option<FileListData>>,
}

impl FileList {
    pub(in crate::app) fn new() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(200.0)),
            rendered: Cell::new(0),
            data: RefCell::new(None),
        }
    }

    pub(in crate::app) fn reset(&self) {
        *self.data.borrow_mut() = None;
        self.rendered.set(0);
        self.state.reset(0);
    }

    pub(super) fn prepare(&self, key: FileListKey, cache: &DetailCache) {
        let mut data = self.data.borrow_mut();
        if data.as_ref().is_some_and(|d| d.key == key) {
            return;
        }
        let rows = rows(cache, key.open.as_deref(), key.all_files, key.all_diffs);
        let paths: Vec<_> = cache.files.iter().map(|f| f.path.clone()).collect();
        let offset = data
            .as_ref()
            .filter(|d| {
                d.key.sel.session == key.sel.session
                    && d.key.sel.lane == key.sel.lane
                    && d.key.sel.first == key.sel.first
            })
            .map(|d| anchor(d, &paths, &rows, self.state.logical_scroll_top()))
            .unwrap_or_default();
        self.state.splice(0..self.state.item_count(), rows.len());
        self.state.scroll_to(offset);
        *data = Some(FileListData { key, paths, rows });
    }

    pub(super) fn row(&self, index: usize) -> Option<FileRow> {
        self.data.borrow().as_ref()?.rows.get(index).copied()
    }

    pub(super) fn top_event(&self) -> Option<usize> {
        match self.row(self.state.logical_scroll_top().item_ix)? {
            FileRow::Edit { event, .. } => Some(event),
            _ => None,
        }
    }

    pub(super) fn at_bottom(&self) -> bool {
        let bounds = self.state.viewport_bounds();
        bounds.size.height > px(0.0)
            && self
                .state
                .item_count()
                .checked_sub(1)
                .and_then(|i| self.state.bounds_for_item(i))
                .is_some_and(|last| last.bottom() <= bounds.bottom() + px(4.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::detail::FileStat;
    use crate::patch::{DiffRow, FilePatch};

    fn cache() -> DetailCache {
        DetailCache {
            selection_summary: Default::default(),
            session_summary: Default::default(),
            sel: Selection {
                session: 0,
                lane: 0,
                first: 0,
                last: 400,
            },
            texts: Vec::new(),
            raws: Vec::new(),
            touched: Vec::new(),
            out_lines: Vec::new(),
            commands: Vec::new(),
            files: vec![
                FileStat {
                    path: "a.rs".into(),
                    edits: 3,
                    added: 130,
                    removed: 0,
                    events: vec![1, 300, 400],
                },
                FileStat {
                    path: "b.rs".into(),
                    edits: 1,
                    added: 1,
                    removed: 0,
                    events: vec![1],
                },
            ],
            patches: [
                (1, vec![patch("a.rs", 100), patch("a.rs", 30), patch("b.rs", 1)]),
                (300, vec![patch("a.rs", 0)]),
                (400, vec![patch("a.rs", 5)]),
            ]
            .into(),
        }
    }

    fn patch(path: &str, count: usize) -> FilePatch {
        FilePatch {
            op: "変更",
            path: path.into(),
            added: count,
            removed: 0,
            rows: (0..count)
                .map(|i| DiffRow {
                    kind: '+',
                    old: None,
                    new: Some(i as u32 + 1),
                    text: "line".into(),
                })
                .collect(),
        }
    }

    fn key(cache: &DetailCache) -> FileListKey {
        FileListKey {
            sel: cache.sel,
            generation: 1,
            width: 400.0f32.to_bits(),
            open: Some("a.rs".into()),
            all_files: false,
            all_diffs: false,
        }
    }

    #[test]
    fn files_keep_each_edit_and_fold_budget_per_event() {
        let cache = cache();
        let folded = rows(&cache, Some("a.rs"), false, false);
        assert_eq!(
            folded,
            [
                FileRow::Summary,
                FileRow::File(0),
                FileRow::History(0),
                FileRow::Edit { file: 0, event: 1 },
                FileRow::Edit { file: 0, event: 300 },
                FileRow::Edit { file: 0, event: 400 },
                FileRow::Expand(0),
                FileRow::File(1)
            ]
        );
        let full = rows(&cache, Some("a.rs"), false, true);
        assert_eq!(
            full,
            folded
                .into_iter()
                .filter(|r| *r != FileRow::Expand(0))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            rows(&cache, Some("b.rs"), false, false),
            [
                FileRow::Summary,
                FileRow::File(0),
                FileRow::File(1),
                FileRow::History(1),
                FileRow::Edit { file: 1, event: 1 }
            ]
        );
    }

    #[test]
    fn files_keep_limit_and_anchor_when_revealing_more() {
        let mut cache = cache();
        cache.files.extend((2..105).map(|i| FileStat {
            path: format!("{i}.rs"),
            edits: 0,
            added: 0,
            removed: 0,
            events: Vec::new(),
        }));
        let limited = rows(&cache, None, false, false);
        assert_eq!(limited.len(), FILE_MAX + 2);
        assert_eq!(limited.last(), Some(&FileRow::More));
        let old = FileListData {
            key: key(&cache),
            paths: cache.files.iter().map(|f| f.path.clone()).collect(),
            rows: limited,
        };
        let all = rows(&cache, None, true, false);
        let next = anchor(
            &old,
            &old.paths,
            &all,
            ListOffset {
                item_ix: FILE_MAX + 1,
                offset_in_item: px(3.0),
            },
        );
        assert_eq!(all[next.item_ix], FileRow::File(FILE_MAX));
        assert_eq!(next.offset_in_item, px(3.0));
        assert_eq!(all.len(), cache.files.len() + 1);
    }

    #[test]
    fn files_keep_edit_anchor_after_reordering_and_expansion() {
        let cache = cache();
        let old = FileListData {
            key: key(&cache),
            paths: vec!["a.rs".into(), "b.rs".into()],
            rows: rows(&cache, Some("a.rs"), false, false),
        };
        let offset = ListOffset {
            item_ix: 4,
            offset_in_item: px(17.0),
        };
        let new_paths = vec!["b.rs".into(), "a.rs".into()];
        let new_rows = vec![
            FileRow::Summary,
            FileRow::File(0),
            FileRow::File(1),
            FileRow::History(1),
            FileRow::Edit { file: 1, event: 1 },
            FileRow::Edit { file: 1, event: 300 },
            FileRow::Edit { file: 1, event: 400 },
        ];
        let next = anchor(&old, &new_paths, &new_rows, offset);
        assert_eq!(new_rows[next.item_ix], FileRow::Edit { file: 1, event: 300 });
        assert_eq!(next.offset_in_item, offset.offset_in_item);
        let reset = anchor(&old, &new_paths, &[FileRow::Summary], offset);
        assert_eq!((reset.item_ix, reset.offset_in_item), (0, px(0.0)));
    }
}
