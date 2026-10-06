//! 選択範囲の本文・編集履歴・集計。背景処理の実行方法と画面の状態は呼び出し元が管理する。

pub mod summary;
pub mod analysis;
pub mod replay;

use crate::db::{Db, JsCache};
use crate::loader;
use crate::model::truncate;
use crate::model::{Event, EventText, Session, TextRef};
use crate::patch::{FilePatch, parse_patch};
use std::collections::HashMap;
use summary::{SelectionSummary, SessionCache, SessionSummary};

/// 詳細データに保持するイベント本文の上限。編集履歴と集計は選択全体を対象にする。
pub const EVENT_LIMIT: usize = 300;
const DETAIL_MAX: usize = EVENT_LIMIT;

/// ファイルを編集するツール
const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"];

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Selection {
    pub session: usize,
    pub lane: usize,
    pub first: usize,
    pub last: usize,
}

/// 選択中ブロックの詳細（選択時に一度だけ作る）
pub struct DetailCache {
    pub sel: Selection,
    /// 先頭 DETAIL_MAX 件の (input, output)。表示用に切り詰め済み
    pub texts: Vec<(String, String)>,
    /// exec の JS 本文（生データ表示用）。texts と同じ並び
    pub raws: Vec<Option<String>>,
    /// 各イベントで編集したファイル (パス, 追加行数, 削除行数)。texts と同じ並び
    pub touched: Vec<Vec<(String, usize, usize)>>,
    /// 結果の行数（切り詰める前）。texts と同じ並び
    pub out_lines: Vec<usize>,
    /// 実行したシェルのコマンド（切り詰めない）。一覧で全文を出す。texts と同じ並び
    pub commands: Vec<Vec<String>>,
    /// 選択全体で編集したファイル（編集の多い順）
    pub files: Vec<FileStat>,
    /// ファイルを編集したイベントの差分（選択の先頭からの位置 → そのファイルごとの差分）
    pub patches: HashMap<usize, Vec<FilePatch>>,
    pub selection_summary: SelectionSummary,
    pub session_summary: std::sync::Arc<SessionSummary>,
}

impl DetailCache {
    pub fn end(&self) -> f64 {
        self.selection_summary.end
    }
}

/// 選択の中で編集されたファイル
pub struct FileStat {
    pub path: String,
    pub edits: usize,
    pub added: usize,
    pub removed: usize,
    /// このファイルを編集したイベント（選択の先頭からの位置）
    pub events: Vec<usize>,
}

fn is_edit(e: &Event) -> bool {
    EDIT_TOOLS.contains(&e.name.as_ref())
}

/// 整形した入力から、実行したシェルコマンドを取り出す。
fn shell_commands(input: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for para in input.split("\n\n") {
        let at = if para.starts_with("$ ") {
            Some(0)
        } else {
            para.find("\n$ ").map(|i| i + 1)
        };
        if let Some(at) = at {
            let cmd = para[at + 2..].trim_end();
            if !cmd.is_empty() {
                out.push(cmd);
            }
        }
    }
    out
}

/// 本文の読み取り接続と JS・集計のキャッシュ。GUI のタスク制御には依存しない。
#[derive(Default)]
pub struct DetailLoader {
    reader: Option<Db>,
    js_cache: Option<JsCache>,
    summaries: SessionCache,
}

struct DetailEvent {
    index: usize,
    row: Option<i64>,
    text_ref: TextRef,
    input: String,
    output: String,
    edit: bool,
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
        let shown = count.min(DETAIL_MAX);
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
        let mut at: HashMap<String, usize> = HashMap::new();
        for e in events {
            if !is_current() {
                return None;
            }
            let i = e.index;
            let (input, output) = match e.row.and_then(|r| fetched.remove(&r)) {
                Some(v) => v,
                None => match e.text_ref.text() {
                    EventText::Pending(i, o) => (i, o),
                    _ => (e.input, e.output),
                },
            };
            let (input, raw) = match input.strip_prefix(loader::JS_MARK) {
                Some(js) => (
                    exec_detail_cached(self.js_cache.as_ref(), js),
                    Some(js.to_string()),
                ),
                None => (input, None),
            };
            let patches = if e.edit || raw.is_some() {
                parse_patch(&input)
            } else {
                Vec::new()
            };
            let patches: Vec<FilePatch> = patches.into_iter().filter(|p| !p.path.is_empty()).collect();
            let touched: Vec<(String, usize, usize)> = patches
                .iter()
                .map(|p| (p.path.clone(), p.added, p.removed))
                .collect();
            if !patches.is_empty() {
                d.patches.insert(i, patches);
            }
            for (path, added, removed) in &touched {
                let k = *at.entry(path.clone()).or_insert_with(|| {
                    d.files.push(FileStat {
                        path: path.clone(),
                        edits: 0,
                        added: 0,
                        removed: 0,
                        events: Vec::new(),
                    });
                    d.files.len() - 1
                });
                let f = &mut d.files[k];
                f.edits += 1;
                if f.events.last() != Some(&i) {
                    f.events.push(i);
                }
                f.added += added;
                f.removed += removed;
            }
            if i < shown {
                d.out_lines.push(output.lines().count());
                d.commands
                    .push(shell_commands(&input).into_iter().map(str::to_string).collect());
                d.raws.push(raw);
                d.touched.push(touched);
                d.texts.push((truncate(&input, limit), truncate(&output, limit)));
            }
        }
        d.files.sort_by(|a, b| {
            b.edits
                .cmp(&a.edits)
                .then((b.added + b.removed).cmp(&(a.added + a.removed)))
        });
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
        let evs = session.lanes.get(sel.lane)?.events.get(sel.first..=sel.last)?;
        let shown = evs.len().min(DETAIL_MAX);
        let mut events = Vec::new();
        for (ci, chunk) in evs.chunks(256).enumerate() {
            if !is_current() {
                return None;
            }
            for (offset, e) in chunk.iter().enumerate() {
                let index = ci * 256 + offset;
                let edit = is_edit(e);
                if index < shown || edit {
                    let (row, input, output) = match e.text() {
                        EventText::Row(row) => (Some(row), e.input.clone(), e.output.clone()),
                        EventText::Inline(input, output) => (None, input.to_string(), output.to_string()),
                        EventText::Pending(input, output) => (None, input, output),
                    };
                    events.push(DetailEvent {
                        index,
                        row,
                        text_ref: e.text_ref.clone(),
                        input,
                        output,
                        edit,
                    });
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

/// exec の JS を整形する。初回はモック実行してキャッシュに保存する。
fn exec_detail_cached(cache: Option<&JsCache>, js: &str) -> String {
    let h = crate::jsmock::hash(js);
    if let Some(d) = cache.and_then(|c| c.get(h)) {
        return d;
    }
    let d = loader::exec_detail(js);
    if let Some(c) = cache {
        c.put(h, &d);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Loaded, Source, Status};

    #[test]
    fn overwritten_texts_keep_details_captured_before_commit() {
        let fixture = crate::db::fixtures::Fixture::new();
        let mut db = fixture.open();
        let mut items = crate::db::fixtures::items(&[("tool", "旧入力", "旧結果")]);
        let first = crate::db::pack_files(&[("file".into(), "1".into(), &items)]);
        first[0].release_texts(&mut items);
        db.put(&first).unwrap();
        drop(first);
        let Loaded::Session(old) = items.remove(0) else { unreachable!() };
        let old = std::sync::Arc::new(old);
        let e = &old.lanes[0].events[0];
        let EventText::Row(id) = e.text() else { unreachable!() };
        let captured = DetailEvent { index: 0, row: Some(id), text_ref: e.text_ref.clone(),
            input: String::new(), output: String::new(), edit: false };
        let new = crate::db::fixtures::items(&[("tool", "new input", "new output")]);
        db.put(&crate::db::pack_files(&[("file".into(), "2".into(), &new)])).unwrap();
        let mut loader = DetailLoader { reader: Some(Db::open_index_read(&fixture.texts).unwrap()), ..Default::default() };
        let sel = Selection { session: 0, lane: 0, first: 0, last: 0 };
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
        let values: Vec<_> = (0..DETAIL_MAX + 2).map(|i| if i == DETAIL_MAX + 1 {
            ("apply_patch".to_string(), "*** Update File: src/a.rs\n@@\n-old\n+日本語".to_string(), String::new())
        } else { ("exec_command".into(), format!("$ cargo test {i}"), format!("結果 {i}\n")) }).collect();
        let refs: Vec<_> = values.iter().map(|(n, i, o)| (n.as_str(), i.as_str(), o.as_str())).collect();
        let mut items = crate::db::fixtures::items(&refs);
        let Loaded::Session(inline) = &items[0] else { unreachable!() };
        let sel = Selection { session: 0, lane: 0, first: 0, last: DETAIL_MAX + 1 };
        let reference = DetailLoader::default().load_session(sel, &std::sync::Arc::new(inline.clone()), || true).unwrap();
        let packed = crate::db::pack_files(&[("file".into(), "fp".into(), &items)]);
        packed[0].release_texts(&mut items);
        let Loaded::Session(s) = items.remove(0) else { unreachable!() };
        let snapshot = std::sync::Arc::new(s);
        let mut loader = DetailLoader { reader: Some(Db::open_index_read(&fixture.texts).unwrap()), ..Default::default() };
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
            assert_eq!((actual.files[0].path.as_str(), &actual.files[0].events), ("src/a.rs", &vec![DETAIL_MAX + 1]));
            assert_eq!(actual.selection_summary.total.stats, reference.selection_summary.total.stats);
            assert_eq!(actual.selection_summary.total.usage, reference.selection_summary.total.usage);
        }
        assert_eq!(crate::model::pending_text_bytes(&snapshot.lanes[0].events), 0);
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
            last: DETAIL_MAX + 10,
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
                index: DETAIL_MAX,
                row: None,
                text_ref: Default::default(),
                input: "*** Update File: src/a.rs\n@@\n-old\n+new".into(),
                output: String::new(),
                edit: true,
            },
        ];
        let d = DetailLoader::default()
            .load(sel, events, DETAIL_MAX + 1, || true)
            .unwrap();
        assert_eq!(d.commands, [vec!["cargo test".to_string()]]);
        assert_eq!(d.out_lines, [2]);
        assert_eq!(d.files.len(), 1);
        assert_eq!(
            (&*d.files[0].path, d.files[0].added, d.files[0].removed),
            ("src/a.rs", 1, 1)
        );
        assert_eq!(d.files[0].events, [DETAIL_MAX]);
        assert!(d.patches.contains_key(&DETAIL_MAX));
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
        assert_eq!(detail.files[0].events, [DETAIL_MAX]);
        assert_eq!(detail.texts.len(), DETAIL_MAX);
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
        assert!(!Arc::ptr_eq(&detail.session_summary, &updated.session_summary));
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

    #[test]
    fn commands_in_input() {
        assert_eq!(shell_commands("$ ls -la\n\ncwd: /w"), vec!["ls -la"]);
        let exec = "（JS をモック実行して推定: 2 件）\n\n#0 exec_command\n$ cat a\n  | wc\n\ncwd: /w\n\n#1 a ← exec_command\n$ rg x";
        assert_eq!(shell_commands(exec), vec!["cat a\n  | wc", "rg x"]);
        assert!(shell_commands("{\"a\": 1}").is_empty());
    }
}
