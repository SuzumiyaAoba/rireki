//! 履歴 API の引数検証と共通の絞り込み。HTTP や応答 JSON の生成には依存しない。

use crate::model::{Event, Kind, Session, Source, Status};
use serde::Deserialize;

pub(super) const MAX_LIMIT: usize = 100;
pub(super) const MAX_OFFSET: usize = 10_000;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Order {
    #[default]
    Oldest,
    Newest,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Args {
    pub(super) query: String,
    pub(super) source: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) after: Option<f64>,
    pub(super) before: Option<f64>,
    pub(super) lane: Option<usize>,
    pub(super) event_index: Option<usize>,
    pub(super) kind: Option<String>,
    pub(super) status: Option<String>,
    pub(super) offset: usize,
    pub(super) limit: Option<usize>,
    pub(super) order: Order,
    pub(super) include_text: bool,
    pub(super) text_limit: Option<usize>,
    pub(super) expected_revision: Option<u64>,
}

impl Args {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.event_index.is_some() && self.lane.is_none() {
            return Err("event_index requires lane".into());
        }
        if self.limit.is_some_and(|v| !(1..=MAX_LIMIT).contains(&v)) || self.offset > MAX_OFFSET {
            return Err("limit must be 1..100 and offset must be 0..10000".into());
        }
        if self.text_limit.is_some_and(|v| !(1..=20_000).contains(&v)) {
            return Err("text_limit must be 1..20000 characters per input/output".into());
        }
        if self.query.chars().count() > 4096 {
            return Err("query must be at most 4096 characters".into());
        }
        if self.after.zip(self.before).is_some_and(|(a, b)| a > b) {
            return Err("after must be <= before (Unix seconds)".into());
        }
        for (value, allowed, field) in [
            (self.source.as_deref(), &["claude", "codex"][..], "source"),
            (
                self.kind.as_deref(),
                &["user", "assistant", "thinking", "tool", "agent"][..],
                "kind",
            ),
            (
                self.status.as_deref(),
                &[
                    "ok",
                    "error",
                    "rejected",
                    "interrupted",
                    "pending",
                    "failed",
                ][..],
                "status",
            ),
        ] {
            if value.is_some_and(|v| !allowed.contains(&v)) {
                return Err(format!(
                    "invalid {field}; expected one of {}",
                    allowed.join(", ")
                ));
            }
        }
        Ok(())
    }

    pub(super) fn limit(&self) -> usize {
        self.limit.unwrap_or(50)
    }

    pub(super) fn session_matches(&self, s: &Session) -> bool {
        self.session_id.as_deref().is_none_or(|id| s.id == id)
            && self.source.as_deref().is_none_or(|v| source(s.source) == v)
            && self.cwd.as_deref().is_none_or(|v| s.cwd == v)
            && self.after.is_none_or(|v| s.end >= v)
            && self.before.is_none_or(|v| s.start <= v)
    }

    /// 一覧取得と分析に同じ絞り込みを適用し、レーン・イベントの保存順を保つ。
    pub(super) fn matching_events<'a>(
        &'a self,
        session: &'a Session,
        words: &'a [&'a str],
    ) -> impl Iterator<Item = (usize, usize, &'a Event)> + 'a {
        session
            .lanes
            .iter()
            .enumerate()
            .filter(|(li, _)| self.lane.is_none_or(|lane| lane == *li))
            .flat_map(move |(li, lane)| {
                lane.events
                    .iter()
                    .enumerate()
                    .filter(move |(ei, e)| {
                        self.event_index.is_none_or(|index| index == *ei)
                            && self.event_matches(e, words)
                    })
                    .map(move |(ei, e)| (li, ei, e))
            })
    }

    fn event_matches(&self, e: &Event, words: &[&str]) -> bool {
        self.after.is_none_or(|v| e.start >= v)
            && self.before.is_none_or(|v| e.start <= v)
            && self.kind.as_deref().is_none_or(|v| kind(e.kind) == v)
            && self.status.as_deref().is_none_or(|v| {
                if v == "failed" {
                    e.status.failed()
                } else {
                    status(e.status) == v
                }
            })
            && crate::search::matches_words(
                words,
                [&e.name, &e.summary, e.model.as_deref().unwrap_or("")],
            )
    }
}

pub(super) fn source(s: Source) -> &'static str {
    match s {
        Source::Claude => "claude",
        Source::Codex => "codex",
    }
}

pub(super) fn kind(k: Kind) -> &'static str {
    match k {
        Kind::User => "user",
        Kind::Assistant => "assistant",
        Kind::Thinking => "thinking",
        Kind::Tool => "tool",
        Kind::Agent => "agent",
    }
}

pub(super) fn status(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Error => "error",
        Status::Rejected => "rejected",
        Status::Interrupted => "interrupted",
        Status::Pending => "pending",
    }
}
