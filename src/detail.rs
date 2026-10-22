//! 選択範囲の本文・編集履歴・集計。背景処理の実行方法と画面の状態は呼び出し元が管理する。
//!
//! `loading` は読み込みとキャンセル、`content` は本文の復元と整形、
//! `files` は差分とファイル履歴を担当する。公開 API はこのモジュールから提供する。

pub mod analysis;
mod content;
mod files;
mod loading;
pub mod replay;
pub mod summary;

pub use files::FileStat;
pub use loading::DetailLoader;

use crate::patch::FilePatch;
use std::collections::HashMap;
use summary::{SelectionSummary, SessionSummary};

/// 詳細データに保持するイベント本文の上限。編集履歴と集計は選択全体を対象にする。
pub const EVENT_LIMIT: usize = 300;

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
    /// 先頭 EVENT_LIMIT 件の (input, output)。表示用に切り詰め済み
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
