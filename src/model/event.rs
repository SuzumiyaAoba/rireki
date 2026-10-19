//! イベントの種類、本文参照、実行結果と件数集計。

use super::{EventText, TextRef, Usage};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    #[inline]
    pub fn label(self) -> &'static str {
        match self {
            Source::Claude => "Claude",
            Source::Codex => "Codex",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    User,
    Assistant,
    Thinking,
    Tool,
    Agent,
}

impl Kind {
    /// サブエージェント起動もツール呼び出しとして数える。
    #[inline]
    pub fn is_tool(self) -> bool {
        matches!(self, Self::Tool | Self::Agent)
    }
}

/// ツール呼び出しの結果。メッセージは常に Ok
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Status {
    #[default]
    Ok,
    /// 失敗（エラーの結果、0 以外の終了コード）
    Error,
    /// ユーザーが実行を拒否した
    Rejected,
    /// ユーザーが中断した
    Interrupted,
    /// 結果がない（実行中、または結果を記録せずに終わった）
    Pending,
}

impl Status {
    /// 失敗の扱い（エラー・拒否・中断）か
    #[inline]
    pub fn failed(self) -> bool {
        matches!(self, Status::Error | Status::Rejected | Status::Interrupted)
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    /// Unix 秒
    pub start: f64,
    pub end: f64,
    pub kind: Kind,
    /// ツール名、または "User" / "Assistant" など。同じ名前はイベント間で共有する
    pub name: Arc<str>,
    /// 要約は公開した版の間で共有する。変更時は別の文字列へ差し替える。
    pub summary: Arc<str>,
    pub input: String,
    pub output: String,
    /// ツール呼び出し ID（サブエージェントとの紐付けに使う）
    pub call_id: Option<Arc<str>>,
    /// 索引DBの行IDまたは保存待ちの共有本文。本文は詳細表示時に取り出す。
    pub text_ref: TextRef,
    /// この時点の API 呼び出しのトークン使用量（なければ空）
    pub usage: Usage,
    /// 使用量を計上したモデル（使用量のあるイベントだけ）
    pub model: Option<Arc<str>>,
    pub status: Status,
}

impl Event {
    /// input / output を除いた複製
    pub fn clone_meta(&self) -> Self {
        Self {
            start: self.start,
            end: self.end,
            kind: self.kind,
            name: self.name.clone(),
            summary: self.summary.clone(),
            input: String::new(),
            output: String::new(),
            call_id: self.call_id.clone(),
            text_ref: self.text_ref.clone(),
            usage: self.usage,
            model: self.model.clone(),
            status: self.status,
        }
    }

    pub fn text(&self) -> EventText<'_> {
        match self.text_ref.text() {
            EventText::Inline(_, _) => EventText::Inline(&self.input, &self.output),
            text => text,
        }
    }

    pub fn row(&self) -> Option<i64> {
        self.text_ref.row_id()
    }
}

/// 圧縮本文はファイルごとに共有されるため、同じ保持元を一回だけ数える。
pub fn pending_text_bytes<'a>(events: impl IntoIterator<Item = &'a Event>) -> usize {
    let mut seen = std::collections::HashSet::new();
    events
        .into_iter()
        .filter_map(|e| e.text_ref.key().map(|key| (key, &e.text_ref)))
        .filter(|(key, _)| seen.insert(*key))
        .map(|(_, text)| text.pending_bytes())
        .sum()
}

/// イベント列の集計（詳細パネルの概要に出す）。トークン数は `UsageSum` で数える
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Stats {
    /// ユーザーの発話（ターン）の数
    pub turns: usize,
    /// ツール呼び出し（サブエージェントの起動を含む）の数
    pub tools: usize,
    pub errors: usize,
    pub rejected: usize,
    pub interrupted: usize,
    /// 1 回の API 呼び出しの入力（キャッシュ含む）の最大。コンテキストの大きさの最大
    pub peak: u64,
}

impl Stats {
    pub fn of<'a>(events: impl IntoIterator<Item = &'a Event>) -> Self {
        let mut s = Stats::default();
        for e in events {
            s.add(e);
        }
        s
    }

    #[inline]
    pub fn add(&mut self, e: &Event) {
        match e.kind {
            Kind::User => self.turns += 1,
            Kind::Tool | Kind::Agent => self.tools += 1,
            _ => {}
        }
        match e.status {
            Status::Error => self.errors += 1,
            Status::Rejected => self.rejected += 1,
            Status::Interrupted => self.interrupted += 1,
            _ => {}
        }
        let u = &e.usage;
        self.peak = self.peak.max(u.input_total());
    }

    /// 失敗（エラー・拒否・中断）の数
    #[inline]
    pub fn failures(&self) -> usize {
        self.errors + self.rejected + self.interrupted
    }
}
