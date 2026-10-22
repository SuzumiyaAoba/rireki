//! 選択範囲全体の差分とファイル履歴。本文の表示上限には依存しない。

use std::collections::HashMap;

use crate::model::Event;
use crate::patch::{FilePatch, parse_patch};

/// ファイルを編集するツール。
const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"];

/// 選択の中で編集されたファイル
pub struct FileStat {
    pub path: String,
    pub edits: usize,
    pub added: usize,
    pub removed: usize,
    /// このファイルを編集したイベント（選択の先頭からの位置）
    pub events: Vec<usize>,
}

pub(super) fn is_edit(event: &Event) -> bool {
    EDIT_TOOLS.contains(&event.name.as_ref())
}

#[derive(Default)]
pub(super) struct FileHistory {
    files: Vec<FileStat>,
    patches: HashMap<usize, Vec<FilePatch>>,
    positions: HashMap<String, usize>,
}

impl FileHistory {
    /// 編集ツールと JS の整形結果から差分を拾い、イベントごとの変更量を返す。
    pub(super) fn record(
        &mut self,
        index: usize,
        input: &str,
        inspect: bool,
    ) -> Vec<(String, usize, usize)> {
        let patches = if inspect {
            parse_patch(input)
        } else {
            Vec::new()
        };
        let patches: Vec<FilePatch> = patches.into_iter().filter(|p| !p.path.is_empty()).collect();
        let touched: Vec<(String, usize, usize)> = patches
            .iter()
            .map(|p| (p.path.clone(), p.added, p.removed))
            .collect();
        if !patches.is_empty() {
            self.patches.insert(index, patches);
        }
        for (path, added, removed) in &touched {
            let k = *self.positions.entry(path.clone()).or_insert_with(|| {
                self.files.push(FileStat {
                    path: path.clone(),
                    edits: 0,
                    added: 0,
                    removed: 0,
                    events: Vec::new(),
                });
                self.files.len() - 1
            });
            let f = &mut self.files[k];
            f.edits += 1;
            if f.events.last() != Some(&index) {
                f.events.push(index);
            }
            f.added += added;
            f.removed += removed;
        }
        touched
    }

    pub(super) fn finish(mut self) -> (Vec<FileStat>, HashMap<usize, Vec<FilePatch>>) {
        self.files.sort_by(|a, b| {
            b.edits
                .cmp(&a.edits)
                .then((b.added + b.removed).cmp(&(a.added + a.removed)))
        });
        (self.files, self.patches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_patch_counts_event_positions_and_stable_ties() {
        let mut history = FileHistory::default();
        let touched = history.record(
            4,
            "*** Update File: repeated.rs\n+a\n*** Update File: repeated.rs\n-b\n\
             *** Update File: first.rs\n+x\n*** Update File: second.rs\n+y\n\
             *** Update File: \n+ignored",
            true,
        );
        assert_eq!(touched.len(), 4);
        assert!(
            history
                .record(5, "*** Update File: ignored.rs\n+x", false)
                .is_empty()
        );
        history.record(6, "*** Update File: repeated.rs\n+c", true);

        let (files, patches) = history.finish();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["repeated.rs", "first.rs", "second.rs"]
        );
        assert_eq!(
            (files[0].edits, files[0].added, files[0].removed),
            (3, 2, 1)
        );
        assert_eq!(files[0].events, [4, 6]);
        assert_eq!(patches[&4].len(), 4);
        assert_eq!(patches[&6].len(), 1);
        assert!(!patches.contains_key(&5));
    }
}
