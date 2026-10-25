//! ログの不変属性を使って更新後の選択・閲覧履歴を解決する。

use super::{EventRef, Row};
use crate::detail::Selection;
use crate::model::{Event, Kind, Session, Snapshot, Source};

/// 添字ではなくログの識別子で保持する閲覧履歴。削除された項目は解決しない。
#[derive(Clone)]
pub struct Location {
    session: String,
    source: Source,
    first: EventKey,
    last: EventKey,
}

impl Location {
    pub fn capture(snapshot: &Snapshot, selection: Selection) -> Option<Self> {
        let session = snapshot.sessions.get(selection.session)?;
        Some(Self {
            session: session.id.clone(),
            source: session.source,
            first: EventKey::capture(
                session,
                EventRef {
                    lane: selection.lane,
                    event: selection.first,
                },
            )?,
            last: EventKey::capture(
                session,
                EventRef {
                    lane: selection.lane,
                    event: selection.last,
                },
            )?,
        })
    }
    pub fn resolve(&self, snapshot: &Snapshot) -> Option<Selection> {
        let index = snapshot
            .sessions
            .iter()
            .position(|s| s.id == self.session && s.source == self.source)?;
        let first = self.first.locate(&snapshot.sessions[index])?;
        let last = self.last.locate(&snapshot.sessions[index])?;
        (first.lane == last.lane).then_some(Selection {
            session: index,
            lane: first.lane,
            first: first.event.min(last.event),
            last: first.event.max(last.event),
        })
    }
}

/// 検索結果のセッション行とイベント行を区別して、更新後も同じ行を選択する。
pub(super) struct RowKey {
    session: String,
    source: Source,
    event: Option<EventKey>,
}

impl RowKey {
    pub(super) fn capture(snapshot: &Snapshot, row: Row) -> Option<Self> {
        let session = snapshot.sessions.get(row.session())?;
        Some(Self {
            session: session.id.clone(),
            source: session.source,
            event: match row {
                Row::Session(_) => None,
                Row::Event { lane, event, .. } => {
                    Some(EventKey::capture(session, EventRef { lane, event })?)
                }
            },
        })
    }

    pub(super) fn locate(&self, snapshot: &Snapshot) -> Option<Row> {
        let session = snapshot
            .sessions
            .iter()
            .position(|session| session.id == self.session && session.source == self.source)?;
        match &self.event {
            None => Some(Row::Session(session)),
            Some(key) => {
                let event = key.locate(&snapshot.sessions[session])?;
                Some(Row::Event {
                    session,
                    lane: event.lane,
                    event: event.event,
                })
            }
        }
    }
}

/// 添字は更新で移動するため、レーンとイベントの不変属性で選択を引き継ぐ。
#[derive(Clone)]
pub(super) struct EventKey {
    origin: String,
    lane_name: String,
    spawned_by: Option<String>,
    start: u64,
    kind: Kind,
    name: String,
    call_id: Option<String>,
    occurrence: usize,
}

impl EventKey {
    pub(super) fn capture(session: &Session, reference: EventRef) -> Option<Self> {
        let lane = session.lanes.get(reference.lane)?;
        let event = lane.events.get(reference.event)?;
        let mut key = Self {
            origin: lane.origin.clone(),
            lane_name: lane.name.clone(),
            spawned_by: lane.spawned_by.clone(),
            start: event.start.to_bits(),
            kind: event.kind,
            name: event.name.to_string(),
            call_id: event.call_id.as_deref().map(str::to_string),
            occurrence: 0,
        };
        key.occurrence = lane.events[..reference.event]
            .iter()
            .filter(|event| key.matches(event))
            .count();
        Some(key)
    }

    fn matches(&self, event: &Event) -> bool {
        if let Some(id) = &self.call_id {
            event.call_id.as_deref() == Some(id.as_str())
        } else {
            event.call_id.is_none()
                && event.start.to_bits() == self.start
                && event.kind == self.kind
                && event.name.as_ref() == self.name
        }
    }

    pub(super) fn locate(&self, session: &Session) -> Option<EventRef> {
        let lane = session.lanes.iter().position(|lane| {
            lane.origin == self.origin
                && lane.name == self.lane_name
                && lane.spawned_by == self.spawned_by
        })?;
        let event = session.lanes[lane]
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| self.matches(event))
            .nth(self.occurrence)?
            .0;
        Some(EventRef { lane, event })
    }
}
