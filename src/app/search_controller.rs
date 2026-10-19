//! 検索タスク、検索欄と結果カーソルの制御。

use super::{Context, DetailTab, Ordering, Rireki, SEARCH_LIMIT, Window, search};
use gpui::Focusable;

impl Rireki {
    // ------------------------------------------------------------ 検索

    pub(super) fn refresh_results(&mut self, cx: &mut Context<Self>) {
        let generation = self.search_gen.fetch_add(1, Ordering::Relaxed) + 1;
        self.search_task = None;
        self.results = (!self.query.is_empty()).then(search::Results::default);
        self.searching = !self.query.is_empty();
        if self.query.is_empty() {
            return;
        }
        let snapshot = self.store.clone();
        let scope = self.order.clone();
        let query = self.query.clone();
        let index = self.search.clone();
        let counter = self.search_gen.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    let current = || counter.load(Ordering::Relaxed) == generation;
                    if !current() {
                        return None;
                    }
                    let mut index = index.lock().unwrap();
                    index.search_while(&snapshot.sessions, &scope, &query, SEARCH_LIMIT, current)
                })
                .await;
            let Some(results) = results else { return };
            let _ = this.update(cx, |app, cx| {
                if app.search_gen.load(Ordering::Relaxed) == generation {
                    let n = results.len();
                    app.result_cursor = app.result_cursor.min(n.saturating_sub(1));
                    app.results = Some(results);
                    app.searching = false;
                    app.sidebar_changed(cx);
                    cx.notify();
                }
            });
        }));
    }

    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        self.refresh_results(cx);
        if self.results.is_some() {
            self.sidebar_open = true;
        }
        self.sidebar_changed(cx);
        cx.notify();
    }

    pub(super) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some()
            && !self.statistics_open
            && (self.detail_fullscreen || self.detail_tab == DetailTab::Analysis)
        {
            self.set_detail_tab(DetailTab::Analysis, cx);
            self.focus_analysis_search(window, cx);
            return;
        }
        self.statistics_open = false;
        self.popover = None;
        if !self.sidebar_open {
            self.sidebar_open = true;
            self.persist();
        }
        window.focus(&self.search_input.focus_handle(cx));
        self.search_input.update(cx, |i, cx| i.select_all_text(cx));
        self.persist();
        self.sidebar_changed(cx);
        cx.notify();
    }

    pub(super) fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search_input.update(cx, |i, cx| i.set_text("", cx));
    }

    /// 検索結果の i 番目（セッション → イベントの順）を開く
    pub(super) fn open_result(&mut self, i: usize, window: &Window, cx: &mut Context<Self>) {
        let Some(r) = &self.results else { return };
        let ns = r.sessions.len();
        if i < ns {
            let si = r.sessions[i];
            self.open_session(si, window, cx);
        } else if let Some(h) = r.events.get(i - ns).copied() {
            self.open_event(h.session, h.lane, h.event, window, cx);
        }
        self.result_cursor = i;
        self.sidebar_changed(cx);
    }

    pub(super) fn move_cursor(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.results.as_ref().map_or(0, |r| r.len());
        if n == 0 {
            return;
        }
        self.result_cursor = (self.result_cursor as isize + d).clamp(0, n as isize - 1) as usize;
        self.result_list
            .scroll_to_reveal_item(self.result_row(self.result_cursor));
        self.sidebar_changed(cx);
    }
}
