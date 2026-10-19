//! 履歴一覧の仮想行、スクロールのアンカーと使用量の表示スケール。

use super::{
    Cell, DETAIL_MAX, Event, ListAlignment, ListOffset, ListState, RefCell, RowFilter, Selection,
    Usage, px, replay, usage_input,
};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum DetailRow {
    Summary,
    Event(usize),
    Empty,
    Limit,
}

#[derive(Clone, PartialEq)]
pub(super) struct DetailListKey {
    pub(super) sel: Selection,
    pub(super) generation: u64,
    pub(super) filter: Option<RowFilter>,
    pub(super) reversed: bool,
    pub(super) width: u32,
    pub(super) live: bool,
    pub(super) replay_visible: Option<usize>,
}

pub(super) struct DetailListData {
    pub(super) key: DetailListKey,
    pub(super) rows: Vec<DetailRow>,
    pub(super) usage_scale: UsageScale,
}

/// 一覧内の入力・出力それぞれの最大値。可視行の描画では走査し直さない。
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) struct UsageScale {
    pub(super) input: u64,
    pub(super) output: u64,
}

impl UsageScale {
    pub(super) fn include(&mut self, usage: &Usage) {
        self.input = self.input.max(usage_input(usage));
        self.output = self.output.max(usage.output as u64);
    }

    pub(super) fn from_rows(rows: &[DetailRow], events: &[Event]) -> Self {
        let mut scale = Self::default();
        for row in rows {
            if let DetailRow::Event(i) = row {
                scale.include(&events[*i].usage);
            }
        }
        scale
    }
}

/// 高さの違う行を可視範囲だけ描く。更新時は同じイベントを上端に保つ
pub(in crate::app) struct DetailList {
    pub(in crate::app) state: ListState,
    pub(super) data: RefCell<Option<DetailListData>>,
    pub(super) rendered: Cell<usize>,
}

pub(super) fn detail_rows(
    events: &[Event],
    filter: Option<&RowFilter>,
    reversed: bool,
) -> Vec<DetailRow> {
    let mut rows = vec![DetailRow::Summary];
    for (i, e) in events.iter().take(DETAIL_MAX).enumerate() {
        let keep = match filter {
            None => true,
            Some(RowFilter::Name(n)) => e.name.as_ref() == n,
            Some(RowFilter::Failed) => e.status.failed(),
        };
        if keep {
            rows.push(DetailRow::Event(i));
        }
    }
    if reversed {
        rows[1..].reverse();
    }
    if rows.len() == 1 {
        rows.push(DetailRow::Empty);
    }
    if events.len() > DETAIL_MAX {
        rows.push(DetailRow::Limit);
    }
    rows
}

pub(super) fn detail_anchor(
    old: &[DetailRow],
    new: &[DetailRow],
    offset: ListOffset,
) -> ListOffset {
    old.get(offset.item_ix)
        .and_then(|row| new.iter().position(|next| next == row))
        .map(|item_ix| ListOffset { item_ix, ..offset })
        .unwrap_or_default()
}

pub(super) fn replay_at_top(offset: ListOffset) -> bool {
    offset.item_ix == 0 && offset.offset_in_item <= px(4.0)
}

impl DetailList {
    pub(in crate::app) fn new() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(200.0)),
            data: RefCell::new(None),
            rendered: Cell::new(0),
        }
    }

    pub(in crate::app) fn reset(&self) {
        *self.data.borrow_mut() = None;
        self.rendered.set(0);
        self.state.reset(0);
    }

    pub(super) fn prepare(&self, key: DetailListKey, events: &[Event]) {
        let mut data = self.data.borrow_mut();
        if data.as_ref().is_some_and(|d| d.key == key) {
            return;
        }
        // 再生が進んだときは追加された行だけを挿入し、既存行の高さと位置を保つ。
        if let Some(d) = data.as_mut()
            && let (Some(before), Some(after)) = (d.key.replay_visible, key.replay_visible)
            && after >= before
            && key
                == (DetailListKey {
                    replay_visible: Some(after),
                    ..d.key.clone()
                })
        {
            let mut added = replay::matching_rows(events, before, after, key.filter.as_ref());
            for row in &added {
                if let DetailRow::Event(i) = row {
                    d.usage_scale.include(&events[*i].usage);
                }
            }
            if key.reversed {
                added.reverse();
            }
            if !added.is_empty() {
                if d.rows == [DetailRow::Empty] {
                    self.state.splice(0..1, added.len());
                    d.rows = added;
                } else if key.reversed {
                    let offset = self.state.logical_scroll_top();
                    let count = added.len();
                    self.state.splice(0..0, count);
                    d.rows.splice(0..0, added);
                    self.state.scroll_to(ListOffset {
                        item_ix: offset.item_ix + count,
                        ..offset
                    });
                } else {
                    self.state.splice(d.rows.len()..d.rows.len(), added.len());
                    d.rows.extend(added);
                }
            }
            d.key = key;
            return;
        }
        let rows = match key.replay_visible {
            Some(visible) => replay::rows(events, visible, key.filter.as_ref(), key.reversed),
            None => detail_rows(events, key.filter.as_ref(), key.reversed),
        };
        let offset = data
            .as_ref()
            .filter(|d| {
                d.key.sel.session == key.sel.session
                    && d.key.sel.lane == key.sel.lane
                    && d.key.sel.first == key.sel.first
            })
            .map(|d| detail_anchor(&d.rows, &rows, self.state.logical_scroll_top()))
            .unwrap_or_default();
        self.state.splice(0..self.state.item_count(), rows.len());
        self.state.scroll_to(offset);
        let usage_scale = UsageScale::from_rows(&rows, events);
        *data = Some(DetailListData {
            key,
            rows,
            usage_scale,
        });
    }

    pub(super) fn usage_scale(&self) -> UsageScale {
        self.data
            .borrow()
            .as_ref()
            .map_or(UsageScale::default(), |d| d.usage_scale)
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

    pub(super) fn at_latest(&self, reversed: bool) -> bool {
        if !reversed {
            return self.at_bottom();
        }
        let offset = self.state.logical_scroll_top();
        if self
            .data
            .borrow()
            .as_ref()
            .is_some_and(|d| d.key.replay_visible.is_some())
        {
            return self.state.viewport_bounds().size.height > px(0.0) && replay_at_top(offset);
        }
        self.state.viewport_bounds().size.height > px(0.0)
            && (offset.item_ix == 0 || (offset.item_ix == 1 && offset.offset_in_item <= px(4.0)))
    }

    pub(super) fn scroll_to_latest(&self, reversed: bool) {
        self.state.scroll_to(if reversed {
            ListOffset::default()
        } else {
            ListOffset {
                item_ix: self.state.item_count(),
                offset_in_item: px(0.0),
            }
        });
    }
}
