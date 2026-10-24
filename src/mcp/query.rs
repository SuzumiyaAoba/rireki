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
        selected(&session.lanes, self.lane).flat_map(move |(li, lane)| {
            selected(&lane.events, self.event_index)
                .filter(move |(_, e)| self.event_matches(e, words))
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

/// 指定した項目だけを直接参照し、元の添字と範囲外の空集合を維持する。
pub(super) fn selected<T>(items: &[T], index: Option<usize>) -> impl Iterator<Item = (usize, &T)> {
    let (start, items) = selected_slice(items, index);
    items
        .iter()
        .enumerate()
        .map(move |(i, item)| (start + i, item))
}

pub(super) fn selected_slice<T>(items: &[T], index: Option<usize>) -> (usize, &[T]) {
    let range = selected_range(items.len(), index);
    (range.start, &items[range])
}

fn selected_range(len: usize, index: Option<usize>) -> std::ops::Range<usize> {
    let start = index.unwrap_or(0).min(len);
    let end = if index.is_some() {
        start.saturating_add(1).min(len)
    } else {
        len
    };
    start..end
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fixtures;
    use crate::model::{Lane, Loaded};

    #[test]
    fn direct_selection_preserves_indices_filters_and_out_of_range_results() {
        let mut items = fixtures::items(&[("Read", "", ""), ("Write", "", ""), ("Read", "", "")]);
        let Loaded::Session(session) = &mut items[0] else {
            panic!()
        };
        session.lanes[0].events[1].status = Status::Rejected;
        session.lanes[0].events[2].model = Some("MODEL-İ".into());
        session.lanes.push(Lane::new("empty"));
        session.lanes.push(session.lanes[0].clone());
        for lane in [None, Some(0), Some(1), Some(2), Some(3), Some(usize::MAX)] {
            for event_index in [None, Some(0), Some(1), Some(2), Some(3), Some(usize::MAX)] {
                for (query, after, status) in [
                    ("", None, None),
                    ("read", Some(1.0), None),
                    ("", None, Some("failed")),
                    ("model-i\u{307}", None, None),
                ] {
                    let args = Args {
                        lane,
                        event_index,
                        after,
                        status: status.map(str::to_owned),
                        ..Args::default()
                    };
                    let words: Vec<_> = query.split_whitespace().collect();
                    let expected: Vec<_> = session
                        .lanes
                        .iter()
                        .enumerate()
                        .filter(|(li, _)| lane.is_none_or(|v| v == *li))
                        .flat_map(|(li, l)| {
                            l.events.iter().enumerate().map(move |(ei, e)| (li, ei, e))
                        })
                        .filter(|(_, ei, e)| {
                            event_index.is_none_or(|v| v == *ei) && args.event_matches(e, &words)
                        })
                        .map(|(li, ei, _)| (li, ei))
                        .collect();
                    let actual: Vec<_> = args
                        .matching_events(session, &words)
                        .map(|(li, ei, _)| (li, ei))
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "lane={lane:?}, event={event_index:?}, query={query}"
                    );
                }
            }
        }
    }
}
