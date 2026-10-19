//! 画面計測用の詳細操作と観測。

use super::{Context, DetailRow, Rireki, RowFilter, Window};
use std::sync::atomic::Ordering;

impl Rireki {
    pub(in crate::app) fn detail_profile_open_file(&mut self, cx: &mut Context<Self>) {
        self.file_open = self
            .detail_cache
            .as_ref()
            .and_then(|d| d.files.first())
            .map(|f| f.path.clone());
        self.detail_changed(cx);
    }

    pub(in crate::app) fn detail_profile_scroll_files(
        &self,
        delta: gpui::Pixels,
        cx: &mut Context<Self>,
    ) {
        if self.profile_eager_files() {
            let h = &self.detail_scroll;
            h.set_offset(gpui::point(
                h.offset().x,
                (h.offset().y - delta).max(-h.max_offset().height),
            ));
        } else {
            self.file_list.state.scroll_by(delta);
        }
        self.detail_changed(cx);
    }

    pub(in crate::app) fn detail_profile_reorder_files(&mut self, cx: &mut Context<Self>) {
        if let Some(cache) = &mut self.detail_cache
            && cache.files.len() > 1
        {
            cache.files.swap(0, 1);
            let generation = self.detail_gen.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some((_, shown)) = &mut self.detail_snapshot {
                *shown = generation;
            }
            self.detail_changed(cx);
        }
    }

    pub(in crate::app) fn detail_profile_open_file_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(sel) = self.selected
            && let Some(event) = self.file_list.top_event()
        {
            self.open_edit(sel.session, sel.lane, sel.first + event, window, cx);
        }
    }

    pub(in crate::app) fn detail_profile_state(&self) -> serde_json::Value {
        let offset = self.detail_list.state.logical_scroll_top();
        let data = self.detail_list.data.borrow();
        let top_event = data
            .as_ref()
            .and_then(|d| d.rows.get(offset.item_ix).map(|r| (d.key.sel, r)))
            .and_then(|(sel, row)| match row {
                DetailRow::Event(i) => Some(sel.first + i),
                _ => None,
            });
        let opened = self.detail_cache.as_ref().and_then(|d| {
            let path = self.file_open.as_ref()?;
            let file = d.files.iter().find(|f| &f.path == path)?;
            let rows: usize = file
                .events
                .iter()
                .filter_map(|i| d.patches.get(i))
                .flat_map(|ps| ps.iter().filter(|p| &p.path == path))
                .map(|p| p.rows.len())
                .sum();
            Some((file.events.len(), rows))
        });
        serde_json::json!({
            "list_count":self.detail_list.state.item_count(), "rendered":self.detail_list.rendered.get(),
            "item_ix":offset.item_ix, "offset_px":f32::from(offset.offset_in_item), "top_event":top_event,
            "at_bottom":self.detail_at_bottom(), "tab":format!("{:?}", self.detail_tab),
            "following":self.detail_following, "follow_pending":self.detail_follow,
            "files":self.detail_cache.as_ref().map(|d| d.files.len()),
            "summary":self.detail_cache.as_ref().map(|d| serde_json::json!({
                "selection_tools":d.selection_summary.total.stats.tools,
                "selection_turns":d.selection_summary.total.stats.turns,
                "selection_tokens":d.selection_summary.total.usage.total(),
                "session_tools":d.session_summary.total.stats.tools,
                "session_turns":d.session_summary.total.stats.turns,
                "session_tokens":d.session_summary.total.usage.total(),
                "models":d.session_summary.models.len(), "lanes":d.session_summary.lanes.len(),
            })),
            "file_list_count":self.file_list.state.item_count(), "file_rendered":self.file_list.rendered.get(),
            "file_item_ix":self.file_list.state.logical_scroll_top().item_ix,
            "file_offset_px":f32::from(self.file_list.state.logical_scroll_top().offset_in_item),
            "file_top_event":self.file_list.top_event(),
            "file_open":self.file_open.is_some(), "open_edits_rows":opened,
            "expanded_in":self.expanded.contains("in"), "expanded_out":self.expanded.contains("out"),
            "expanded_diff":self.expanded.contains("file-diff"),
            "text_chars":self.detail_cache.as_ref().and_then(|d| d.texts.first()).map(|(a,b)| (a.chars().count(),b.chars().count())),
            "selected_count":self.selected.map(|s| s.last - s.first + 1),
            "selected_kind":self.selected.filter(|&s| self.valid(s)).map(|s| format!("{:?}", self.store.sessions[s.session].lanes[s.lane].events[s.first].kind)),
            "row_filter":match &self.row_filter { None => "none", Some(RowFilter::Failed) => "failed", Some(RowFilter::Name(_)) => "name" },
        })
    }

    pub(in crate::app) fn detail_profile_click(
        &self,
    ) -> Option<(gpui::Point<gpui::Pixels>, usize)> {
        let list = &self.detail_list.state;
        let viewport = list.viewport_bounds();
        let data = self.detail_list.data.borrow();
        let data = data.as_ref()?;
        data.rows
            .iter()
            .enumerate()
            .skip(list.logical_scroll_top().item_ix)
            .find_map(|(row, kind)| {
                let DetailRow::Event(i) = kind else {
                    return None;
                };
                let bounds = list.bounds_for_item(row)?;
                let center = bounds.center();
                (center.y >= viewport.top() && center.y < viewport.bottom())
                    .then_some((center, data.key.sel.first + i))
            })
    }
}
