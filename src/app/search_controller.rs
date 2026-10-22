//! 検索タスク、検索欄と結果カーソルの制御。

use std::cell::Cell;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use gpui::{
    Context, Entity, Focusable, ListAlignment, ListState, Task, Window, point, prelude::*, px,
};

use super::{DetailTab, Rireki};
use crate::input::{InputEvent, TextInput};
use crate::search;

/// 検索結果に並べるイベント数の上限
const SEARCH_LIMIT: usize = 300;
/// 入力が止まってから検索するまでの待ち時間
const SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(120);

/// 検索欄・背景検索・結果一覧の状態。検索の世代とタスクはここで一緒に更新する。
pub(super) struct State {
    pub(super) input: Entity<TextInput>,
    index: Arc<Mutex<search::Index>>,
    task: Option<Task<()>>,
    pub(super) searching: bool,
    /// 前後の空白を除いた検索語
    pub(super) query: String,
    /// 検索語が空なら None
    pub(super) results: Option<search::Results>,
    /// セッション、イベントの順に並べた結果のうち、キーボードで選んでいる項目
    pub(super) cursor: usize,
    generation: Arc<AtomicU64>,
    pub(super) list: ListState,
    list_key: Cell<(u64, usize, u32)>,
}

impl State {
    pub(super) fn new(cx: &mut Context<Rireki>) -> Self {
        let input = cx.new(|cx| TextInput::new("セッションやツール呼び出しを検索", cx));
        cx.subscribe(&input, |app, input, _: &InputEvent, cx| {
            let query = input.read(cx).text().trim().to_string();
            app.search_query_changed(query, cx);
        })
        .detach();
        Self {
            input,
            index: Arc::new(Mutex::new(search::Index::default())),
            task: None,
            searching: false,
            query: String::new(),
            results: None,
            cursor: 0,
            generation: Arc::new(AtomicU64::new(0)),
            list: ListState::new(0, ListAlignment::Top, px(100.0)),
            list_key: Cell::new((0, 0, 0)),
        }
    }

    /// 古い検索を中断し、現在の検索語に対応する待機状態へ切り替える。
    fn restart(&mut self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.task = None;
        self.results = (!self.query.is_empty()).then(search::Results::default);
        self.searching = !self.query.is_empty();
        generation
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Relaxed) == generation
    }

    fn publish(&mut self, generation: u64, results: search::Results) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.cursor = self.cursor.min(results.len().saturating_sub(1));
        self.results = Some(results);
        self.searching = false;
        true
    }

    /// 旧版の索引は、検索欄が空でもデータ更新時に背景で解放する。
    pub(super) fn index(&self) -> Arc<Mutex<search::Index>> {
        self.index.clone()
    }

    pub(super) fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn prepare_list(&self, count: usize, width: f32) {
        let key = (
            self.generation.load(Ordering::Relaxed),
            count,
            width.to_bits(),
        );
        let old = self.list_key.replace(key);
        if (old.0, old.1) != (key.0, key.1) {
            self.list.reset(count);
        } else if old.2 != key.2 {
            // 折り返しで高さが変わる。現在の項目を保ち、画面外の高さも測り直す。
            self.list.splice(0..count, count);
        }
    }
}

impl Rireki {
    /// 打鍵のたびではなく、入力が少し止まってから検索する。
    fn search_query_changed(&mut self, query: String, cx: &mut Context<Self>) {
        if query == self.search_state.query {
            return;
        }
        self.search_state.query = query;
        let generation = self.search_state.restart();
        self.sidebar_changed(cx);
        if self.search_state.query.is_empty() {
            self.run_search(cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let _ = this.update(cx, |app, cx| {
                if app.search_state.is_current(generation) {
                    app.search_state.cursor = 0;
                    app.sidebar_scroll.set_offset(point(px(0.0), px(0.0)));
                    app.navigator_list
                        .state
                        .scroll_to(gpui::ListOffset::default());
                    app.run_search(cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn refresh_results(&mut self, cx: &mut Context<Self>) {
        let generation = self.search_state.restart();
        if self.search_state.query.is_empty() {
            return;
        }
        let snapshot = self.store.clone();
        let scope = self.order.clone();
        let query = self.search_state.query.clone();
        let index = self.search_state.index.clone();
        let counter = self.search_state.generation.clone();
        self.search_state.task = Some(cx.spawn(async move |this, cx| {
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
                if app.search_state.publish(generation, results) {
                    app.sidebar_changed(cx);
                    cx.notify();
                }
            });
        }));
    }

    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        self.refresh_results(cx);
        if self.search_state.results.is_some() {
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
        window.focus(&self.search_state.input.focus_handle(cx));
        self.search_state
            .input
            .update(cx, |i, cx| i.select_all_text(cx));
        self.persist();
        self.sidebar_changed(cx);
        cx.notify();
    }

    pub(super) fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search_state
            .input
            .update(cx, |i, cx| i.set_text("", cx));
    }

    /// 検索結果の i 番目（セッション → イベントの順）を開く
    pub(super) fn open_result(&mut self, i: usize, window: &Window, cx: &mut Context<Self>) {
        let Some(r) = &self.search_state.results else {
            return;
        };
        let ns = r.sessions.len();
        if i < ns {
            let si = r.sessions[i];
            self.open_session(si, window, cx);
        } else if let Some(h) = r.events.get(i - ns).copied() {
            self.open_event(h.session, h.lane, h.event, window, cx);
        }
        self.search_state.cursor = i;
        self.sidebar_changed(cx);
    }

    pub(super) fn move_cursor(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.search_state.results.as_ref().map_or(0, |r| r.len());
        if n == 0 {
            return;
        }
        self.search_state.cursor =
            (self.search_state.cursor as isize + d).clamp(0, n as isize - 1) as usize;
        self.search_state
            .list
            .scroll_to_reveal_item(self.result_row(self.search_state.cursor));
        self.sidebar_changed(cx);
    }
}
