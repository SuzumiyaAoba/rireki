//! 解析・保存結果を本文を表示せず比較するためのハッシュ。

use crate::model::{Event, EventText, Lane, Loaded};
use std::hash::{DefaultHasher, Hash, Hasher};

pub(super) fn items_hash(items: &[Loaded]) -> u64 {
    let mut hash = DefaultHasher::new();
    items.len().hash(&mut hash);
    for item in items {
        let lanes: &[Lane] = match item {
            Loaded::Session(s) => {
                (
                    0u8,
                    &s.id,
                    s.source as u8,
                    &s.cwd,
                    s.headless,
                    &s.title,
                    &s.branch,
                    &s.version,
                )
                    .hash(&mut hash);
                &s.lanes
            }
            Loaded::Child(parent, lane) => {
                (1u8, parent).hash(&mut hash);
                std::slice::from_ref(lane)
            }
        };
        lanes.len().hash(&mut hash);
        for lane in lanes {
            (&lane.name, &lane.spawned_by, &lane.model, &lane.effort).hash(&mut hash);
            lane.events.len().hash(&mut hash);
            for e in &lane.events {
                (
                    e.start.to_bits(),
                    e.end.to_bits(),
                    e.kind as u8,
                    e.status as u8,
                )
                    .hash(&mut hash);
                let (input, output) = match e.text() {
                    EventText::Inline(i, o) => {
                        (std::borrow::Cow::Borrowed(i), std::borrow::Cow::Borrowed(o))
                    }
                    EventText::Pending(i, o) => {
                        (std::borrow::Cow::Owned(i), std::borrow::Cow::Owned(o))
                    }
                    EventText::Row(_) => (
                        std::borrow::Cow::Borrowed(e.input.as_str()),
                        std::borrow::Cow::Borrowed(e.output.as_str()),
                    ),
                };
                (
                    &e.name,
                    &e.summary,
                    input.as_ref(),
                    output.as_ref(),
                    &e.call_id,
                    &e.model,
                )
                    .hash(&mut hash);
                (
                    e.usage.input,
                    e.usage.cache_read,
                    e.usage.cache_write,
                    e.usage.cache_write_1h,
                    e.usage.output,
                    e.usage.reasoning,
                )
                    .hash(&mut hash);
            }
        }
    }
    hash.finish()
}

pub(super) fn event_hash(e: &Event, input: &str, output: &str) -> u64 {
    let mut hash = DefaultHasher::new();
    (
        e.start.to_bits(),
        e.end.to_bits(),
        e.kind as u8,
        e.status as u8,
    )
        .hash(&mut hash);
    (&e.name, &e.summary, input, output, &e.call_id, &e.model).hash(&mut hash);
    (
        e.usage.input,
        e.usage.cache_read,
        e.usage.cache_write,
        e.usage.cache_write_1h,
        e.usage.output,
        e.usage.reasoning,
    )
        .hash(&mut hash);
    hash.finish()
}
