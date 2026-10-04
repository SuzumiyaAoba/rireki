//! メインウィンドウ（GPUI のビュー）。Slack のように、左端のアイコンレール・サイドバー・メイン・詳細パネルを並べる
//!
//! - 上端の帯: macOS のタイトルバー領域。信号機ボタンと状態表示だけを置き、ドラッグでウィンドウを動かせる
//! - レール (`rail.rs`): 読み込み元（すべて / Claude / Codex）と表示（タイムライン / カレンダー）の切り替え
//! - サイドバー (`sidebar.rs`): 検索欄と、進行中・プロジェクト・セッションの一覧、または検索結果
//! - メイン: 見出しと凡例 (`header.rs`)、タイムライン (`timeline.rs`) / カレンダー (`calendar.rs`)
//! - 詳細パネル (`detail.rs`): 選択したブロックの内容。選択している間だけ開く
//! - 重ねて出すもの (`overlay.rs`): ホバーカード、まとめ方のメニュー、ショートカット一覧

mod calendar;
mod detail;
mod header;
mod keymap;
mod overlay;
mod rail;
mod sidebar;
mod timeline;
mod widgets;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone};
use gpui::{
    AnyView, App, Bounds, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollHandle, ScrollWheelEvent, StyleRefinement, Window, div, point, prelude::*, px,
};

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::db::{Db, JsCache};
use crate::index::{self, Indexer, Update};
use crate::input::{InputEvent, TextInput};
use crate::layout::Granularity;
use crate::model::{Source, Store};
use crate::prefs::{self, Mode, Prefs, SourceFilter};
use crate::search;
use crate::theme::{self, c};

use detail::DetailView;
pub use keymap::init;
use keymap::{
    Back, CloseWindow, Dismiss, FocusSearch, Forward, GoNext, GoPrev, GoToday, SearchCancel, SearchConfirm, SearchDown,
    SearchUp, ShowDay, ShowHelp, ShowTimeline, ShowWeek, ToggleSidebar, ZoomIn, ZoomOut,
};
use sidebar::SidebarView;

/// 上端の帯（macOS のタイトルバー領域）の高さ
pub const STRIP_H: f32 = 34.0;
/// 左端のアイコンレールの幅
const RAIL_W: f32 = 68.0;
/// 内容パネルの右と下の余白。地の色を額縁のように見せる
const GAP: f32 = 6.0;
/// 内容パネルの角の丸み。端に接する子要素は内側の丸み（1px の枠線を除いた分）に合わせる
const PANEL_RADIUS: f32 = 10.0;
const INNER_RADIUS: f32 = PANEL_RADIUS - 1.0;
const HEADER_H: f32 = 46.0;
const FOOTER_H: f32 = 26.0;
const AXIS_H: f32 = 30.0;
/// 境界ドラッグで変えられる幅の下限
const MIN_PANE_W: f32 = 160.0;
const MIN_SIDEBAR_W: f32 = 200.0;
const MAX_SIDEBAR_W: f32 = 480.0;
const MIN_DETAIL_W: f32 = 320.0;
/// メイン列に残す幅
const MAIN_MIN_W: f32 = 360.0;
/// 境界のつまみの幅
const HANDLE_W: f32 = 6.0;
const ROW_H: f32 = 24.0;
const HOUR_LABEL_W: f32 = 52.0;
const MIN_BLOCK: f32 = 3.0;
/// 詳細パネルに並べるイベント数の上限
const DETAIL_MAX: usize = 300;
/// 検索結果に並べるイベント数の上限
const SEARCH_LIMIT: usize = 300;
/// 入力が止まってから検索するまでの待ち時間
const SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(120);
/// リアルタイムファイルの変更を確認する間隔
const POLL: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Debug)]
struct Selection {
    session: usize,
    lane: usize,
    first: usize,
    last: usize,
}

/// 選択中ブロックの詳細テキスト（選択時に一度だけ作る）
struct DetailCache {
    sel: Selection,
    /// 先頭 DETAIL_MAX 件の (input, output)。表示用に切り詰め済み
    texts: Vec<(String, String)>,
    /// exec の JS 本文（生データ表示用）。texts と同じ並び
    raws: Vec<Option<String>>,
}

struct Track {
    session: usize,
    lane: usize,
    /// サブエージェントの入れ子の深さ（main = 0）
    depth: usize,
    /// 別セッションとして起動された子の main レーンなら、起動元 (セッション, レーン, 時刻)
    link: Option<(usize, usize, f64)>,
}

/// タイムラインの 1 行
enum Row {
    /// ディレクトリ単位のグループ見出し
    Header {
        cwd: String,
        sessions: Vec<usize>,
        collapsed: bool,
    },
    Lane(Track),
}

/// canvas に描くブロック（座標は描画領域の左上基準）
struct BlockPaint {
    rect: (f32, f32, f32, f32),
    sel: Selection,
    color: u32,
    multi: bool,
    selected: bool,
    hovered: bool,
    label: Option<gpui::SharedString>,
}

struct Drag {
    origin: Point<Pixels>,
    t0: f64,
    scroll: f32,
    moved: bool,
}

/// 幅を変更中の境界
#[derive(Clone, Copy)]
enum Resize {
    Sidebar { x: f32, w: f32 },
    Label { x: f32, w: f32 },
    Detail { x: f32, w: f32 },
}

/// 重ねて出しているもの
#[derive(Clone, Copy, PartialEq)]
enum Popover {
    /// まとめ方のメニュー
    Granularity,
    /// キーボードショートカット一覧
    Help,
}

/// サイドバーに並べるプロジェクト（作業ディレクトリ）
struct Project {
    cwd: String,
    /// 表示名。同じ名前のディレクトリが複数あれば親ディレクトリを添える
    name: String,
    sessions: usize,
    /// 最後に作業した時刻
    last: f64,
}

pub struct Rireki {
    store: Store,
    /// 読み込み元で絞り込んだセッション（開始順）
    scope: Vec<usize>,
    /// scope をさらにプロジェクトで絞り込んだ、表示するセッション（開始順）
    order: Vec<usize>,
    /// scope のプロジェクト（最近作業した順）
    projects: Vec<Project>,
    /// `codex exec` などで起動された子セッション → 起動元 (セッション, レーン, 時刻)
    exec_parent: HashMap<usize, (usize, usize, f64)>,
    loading: Option<(usize, usize)>,
    /// 詳細表示で input / output を引くための読み取り専用接続
    reader: Option<Db>,
    /// exec の JS をモック実行した結果のキャッシュ
    js_cache: Option<JsCache>,
    focus_handle: FocusHandle,
    search_input: Entity<TextInput>,
    search: search::Index,
    /// 前後の空白を除いた検索語
    query: String,
    /// 検索結果。検索語が空なら None
    results: Option<search::Results>,
    /// 検索結果のうち、キーボードで選んでいる項目
    result_cursor: usize,
    /// 検索語を変えるたびに増やす。待っている間に次の入力があった検索は捨てる
    search_gen: u64,
    sidebar_view: Entity<SidebarView>,
    sidebar_scroll: ScrollHandle,
    detail_view: Entity<DetailView>,
    detail_cache: Option<DetailCache>,
    /// exec の詳細を整形結果ではなく JS 本文で表示する
    show_raw: bool,
    /// 直前にコピーしたボタンの ID（しばらくチェックマークを出す）
    copied: Option<&'static str>,
    /// 直前のフレームで描いたブロック（クリック・ホバーの当たり判定用）
    hits: Vec<((f32, f32, f32, f32), Selection)>,
    /// ブロックを描いた canvas の左上（ウィンドウ座標）
    hit_origin: Rc<Cell<Point<Pixels>>>,
    hovered: Option<Selection>,
    /// ホバーカードを出す位置（ウィンドウ座標）
    hover_at: Point<Pixels>,
    mode: Mode,
    /// カレンダーで最後に使った表示（日 / 週）
    calendar: Mode,
    // タイムライン
    t0: f64,
    spp: f64,
    row_scroll: f32,
    // カレンダー
    day0: NaiveDate,
    px_per_hour: f32,
    cal_scroll: f32,
    gran: Granularity,
    source: SourceFilter,
    /// 読み込み元ごとに、進行中のセッションがあるか (Claude, Codex)
    live_sources: (bool, bool),
    selected: Option<Selection>,
    /// 選択の履歴（戻る / 進む）
    back: Vec<Selection>,
    forward: Vec<Selection>,
    drag: Option<Drag>,
    sidebar_open: bool,
    sidebar_w: f32,
    /// タイムラインのレーン名の列の幅
    label_w: f32,
    detail_w: f32,
    resize: Option<Resize>,
    /// 折りたたんだディレクトリ
    collapsed: HashSet<String>,
    /// 注目中のディレクトリ。設定中はこのディレクトリのセッションだけを表示する
    focus: Option<String>,
    /// サイドバーで折りたたんだ区分
    folded: HashSet<String>,
    /// サイドバーのセッション一覧に並べる件数
    session_limit: usize,
    popover: Option<Popover>,
    /// まとめ方ボタンの位置（メニューを出す基準）
    gran_anchor: Rc<Cell<Bounds<Pixels>>>,
    /// 下端に出す、表示中の件数
    counts: String,
    /// 最後に保存した画面の状態
    saved: Prefs,
}

fn now() -> f64 {
    Local::now().timestamp_millis() as f64 / 1000.0
}

fn local(t: f64) -> DateTime<Local> {
    Local.timestamp_millis_opt((t * 1000.0) as i64).single().unwrap_or_else(Local::now)
}

fn midnight(d: NaiveDate) -> f64 {
    Local.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().map(|t| t.timestamp() as f64).unwrap_or(0.0)
}

fn fmt_dur(s: f64) -> String {
    let s = s.max(0.0);
    if s < 1.0 {
        format!("{:.0}ms", s * 1000.0)
    } else if s < 60.0 {
        format!("{s:.1}s")
    } else if s < 3600.0 {
        format!("{}m{:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!("{}h{:02}m", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64)
    }
}

/// 一覧に添える短い時刻（「3分前」「14:05」「昨日」「10/02」など）
fn rel_time(t: f64) -> String {
    let d = now() - t;
    if d < 60.0 {
        return "たった今".into();
    }
    if d < 3600.0 {
        return format!("{}分前", (d / 60.0) as u64);
    }
    let lt = local(t);
    let today = Local::now().date_naive();
    if lt.date_naive() == today {
        lt.format("%H:%M").to_string()
    } else if lt.date_naive() == today - Duration::days(1) {
        "昨日".into()
    } else if lt.year() == today.year() {
        lt.format("%-m/%-d").to_string()
    } else {
        lt.format("%Y/%-m/%-d").to_string()
    }
}

/// 検索結果などに添える時刻（今日は時刻だけ、それ以前は日付と時刻）
fn when(t: f64) -> String {
    let lt = local(t);
    let today = Local::now().date_naive();
    if lt.date_naive() == today {
        lt.format("%H:%M").to_string()
    } else if lt.date_naive() == today - Duration::days(1) {
        format!("昨日 {}", lt.format("%H:%M"))
    } else if lt.year() == today.year() {
        lt.format("%-m/%-d %H:%M").to_string()
    } else {
        lt.format("%Y/%-m/%-d").to_string()
    }
}

/// 3 桁ごとに区切った数
fn fmt_n(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// ホームディレクトリを `~` にしたパス
fn tilde(p: &str) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(h.to_str()?).map(str::to_string)) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => p.to_string(),
    }
}

/// パスの末尾のディレクトリ名
fn base_name(p: &str) -> &str {
    p.rsplit('/').find(|s| !s.is_empty()).unwrap_or(p)
}

fn is_live(end: f64) -> bool {
    now() - end < index::LIVE_SECS
}

impl Rireki {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // トラックパッドのピンチでズームする（溜まった分はまとめて反映）
        let pinch = crate::pinch::listen();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(mut m) = pinch.recv().await {
                while let Ok(more) = pinch.try_recv() {
                    m += more;
                }
                if this.update_in(cx, |t, window, cx| t.on_pinch(m, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let bg = cx.background_executor().clone();
            // 1. 索引 DB から安定した履歴を読む
            let indexer = bg.spawn(async { Arc::new(Mutex::new(Indexer::open())) }).await;
            let ix = indexer.clone();
            let stable = bg.spawn(async move { ix.lock().unwrap().load_stable() }).await;
            let has_db = indexer.lock().unwrap().has_db();
            let alive = this
                .update(cx, |app, cx| {
                    app.reader = if has_db { Db::open_read().ok() } else { None };
                    app.apply(stable, cx);
                    cx.notify();
                })
                .is_ok();
            if !alive {
                return;
            }
            // 2. 未索引・更新されたファイルを取り込み、以後はリアルタイムファイルを監視する
            let mut first = true;
            let mut minute = 0;
            loop {
                let ix = indexer.clone();
                let tasks = bg.spawn(async move { ix.lock().unwrap().plan() }).await;
                let total = tasks.len();
                let mut done = 0;
                let mut tasks = tasks.into_iter().peekable();
                while tasks.peek().is_some() {
                    let chunk: Vec<_> = tasks.by_ref().take(index::CHUNK).collect();
                    done += chunk.len();
                    let ix = indexer.clone();
                    let updates = bg.spawn(async move { ix.lock().unwrap().process(&chunk) }).await;
                    let alive = this
                        .update(cx, |app, cx| {
                            let loading = (first && done < total).then_some((done, total));
                            // 変化がなければ描き直さない（3 秒ごとのポーリングで毎回全体を描かない）
                            if app.apply(updates, cx) || app.loading != loading {
                                app.loading = loading;
                                cx.notify();
                            }
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
                // 「3分前」などの表示と進行中の印、現在時刻の線を分が変わるごとに更新する
                let m = (now() / 60.0) as i64;
                if first || m != minute {
                    minute = m;
                    let was_first = std::mem::replace(&mut first, false);
                    let alive = this
                        .update(cx, |app, cx| {
                            if was_first {
                                app.loading = None;
                            }
                            app.live_sources = app.find_live_sources();
                            app.sidebar_changed(cx);
                            cx.notify();
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                }
                bg.timer(POLL).await;
            }
        })
        .detach();

        let search_input = cx.new(|cx| TextInput::new("セッションやツール呼び出しを検索", cx));
        // 打鍵のたびではなく、入力が少し止まってから検索する
        cx.subscribe(&search_input, |this, input, _: &InputEvent, cx| {
            let q = input.read(cx).text().trim().to_string();
            if q == this.query {
                return;
            }
            this.query = q;
            this.search_gen += 1;
            let generation = this.search_gen;
            if this.query.is_empty() {
                this.run_search(cx);
                return;
            }
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_DELAY).await;
                let _ = this.update(cx, |t, cx| {
                    if t.search_gen == generation {
                        t.result_cursor = 0;
                        t.sidebar_scroll.set_offset(point(px(0.0), px(0.0)));
                        t.run_search(cx);
                    }
                });
            })
            .detach();
        })
        .detach();

        let p = prefs::load();
        let today = Local::now().date_naive();
        let weak = cx.weak_entity();
        let detail_view = cx.new(|_| DetailView { app: weak.clone() });
        let sidebar_view = cx.new(|_| SidebarView { app: weak });
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let mut this = Self {
            store: Store::default(),
            scope: Vec::new(),
            order: Vec::new(),
            projects: Vec::new(),
            exec_parent: Default::default(),
            loading: Some((0, 0)),
            reader: None,
            js_cache: JsCache::open().ok(),
            focus_handle,
            search_input,
            search: search::Index::default(),
            query: String::new(),
            results: None,
            result_cursor: 0,
            search_gen: 0,
            sidebar_view,
            sidebar_scroll: ScrollHandle::new(),
            detail_view,
            detail_cache: None,
            show_raw: false,
            copied: None,
            hits: Vec::new(),
            hit_origin: Rc::new(Cell::new(Point::default())),
            hovered: None,
            hover_at: Point::default(),
            mode: p.mode,
            calendar: if p.calendar == Mode::Day { Mode::Day } else { Mode::Week },
            t0: now(),
            spp: 60.0,
            row_scroll: 0.0,
            day0: today,
            px_per_hour: 60.0,
            cal_scroll: 8.0 * 60.0,
            gran: p.gran,
            source: p.source,
            live_sources: (false, false),
            selected: None,
            back: Vec::new(),
            forward: Vec::new(),
            drag: None,
            sidebar_open: p.sidebar,
            sidebar_w: p.sidebar_w.clamp(MIN_SIDEBAR_W, MAX_SIDEBAR_W),
            label_w: p.label_w.max(MIN_PANE_W),
            detail_w: p.detail_w.max(MIN_DETAIL_W),
            resize: None,
            collapsed: p.collapsed.iter().cloned().collect(),
            focus: p.focus.clone(),
            folded: p.folded.iter().cloned().collect(),
            session_limit: 50,
            popover: None,
            gran_anchor: Rc::new(Cell::new(Bounds::default())),
            counts: String::new(),
            saved: p,
        };
        // 現在時刻が右寄りに見えるように始める
        this.go_today(window);
        this
    }

    /// 画面の状態が前回の保存から変わっていれば保存する
    fn persist(&mut self) {
        let mut collapsed: Vec<String> = self.collapsed.iter().cloned().collect();
        collapsed.sort();
        let mut folded: Vec<String> = self.folded.iter().cloned().collect();
        folded.sort();
        let p = Prefs {
            mode: self.mode,
            calendar: self.calendar,
            sidebar: self.sidebar_open,
            sidebar_w: self.sidebar_w,
            label_w: self.label_w,
            detail_w: self.detail_w,
            gran: self.gran,
            source: self.source,
            focus: self.focus.clone(),
            collapsed,
            folded,
        };
        if p != self.saved {
            prefs::save(&p);
            self.saved = p;
        }
    }

    /// 読み込み結果を Store に反映する
    fn apply(&mut self, updates: Vec<Update>, cx: &mut Context<Self>) -> bool {
        let touched = self.store.replace(updates.into_iter().map(|u| (u.origin, u.items)));
        if touched.is_empty() {
            return false;
        }
        self.search.invalidate(&touched);
        self.rebuild_order();
        if !self.query.is_empty() {
            self.refresh_results();
        }
        // 選択中のセッションが更新されたら、範囲を確かめて詳細を作り直す
        if let Some(sel) = self.selected.filter(|s| touched.contains(&s.session)) {
            if self.valid(sel) {
                self.select(sel, cx);
            } else {
                self.selected = None;
                self.detail_cache = None;
                self.detail_changed(cx);
            }
        }
        self.sidebar_changed(cx);
        true
    }

    fn detail_changed(&self, cx: &mut Context<Self>) {
        self.detail_view.update(cx, |_, cx| cx.notify());
    }

    fn sidebar_changed(&self, cx: &mut Context<Self>) {
        self.sidebar_view.update(cx, |_, cx| cx.notify());
    }

    fn rebuild_order(&mut self) {
        let scope: Vec<usize> = (0..self.store.sessions.len())
            .filter(|&i| self.store.visible(i))
            .filter(|&i| match (self.source, self.store.sessions[i].source) {
                (SourceFilter::All, _) => true,
                (SourceFilter::Claude, s) => s == Source::Claude,
                (SourceFilter::Codex, s) => s == Source::Codex,
            })
            .collect();
        let mut order: Vec<usize> = scope
            .iter()
            .copied()
            .filter(|&i| self.focus.as_ref().is_none_or(|f| *f == self.store.sessions[i].cwd))
            .collect();
        order.sort_by(|&a, &b| self.store.sessions[a].start.total_cmp(&self.store.sessions[b].start));
        self.projects = self.projects_of(&scope);
        self.scope = scope;
        self.order = order;
        self.exec_parent = self.find_exec_parents();
        self.live_sources = self.find_live_sources();
    }

    /// セッションをプロジェクトごとにまとめる（最近作業した順）
    fn projects_of(&self, scope: &[usize]) -> Vec<Project> {
        let mut by: HashMap<&str, (usize, f64)> = HashMap::new();
        for &si in scope {
            let s = &self.store.sessions[si];
            let e = by.entry(s.cwd.as_str()).or_insert((0, f64::NEG_INFINITY));
            e.0 += 1;
            e.1 = e.1.max(s.end);
        }
        let mut names: HashMap<&str, usize> = HashMap::new();
        for cwd in by.keys() {
            *names.entry(base_name(cwd)).or_default() += 1;
        }
        let mut out: Vec<Project> = by
            .iter()
            .map(|(&cwd, &(sessions, last))| {
                let base = base_name(cwd);
                let name = if names[base] > 1 {
                    // 同じ名前のディレクトリが複数あるときは親ディレクトリで見分ける
                    let parent = cwd.trim_end_matches('/').rsplit('/').nth(1).unwrap_or("");
                    format!("{base} — {parent}")
                } else {
                    base.to_string()
                };
                Project { cwd: cwd.to_string(), name, sessions, last }
            })
            .collect();
        out.sort_by(|a, b| b.last.total_cmp(&a.last));
        out
    }

    fn find_live_sources(&self) -> (bool, bool) {
        let mut out = (false, false);
        for (i, s) in self.store.sessions.iter().enumerate() {
            if self.store.visible(i) && is_live(s.end) {
                match s.source {
                    Source::Claude => out.0 = true,
                    Source::Codex => out.1 = true,
                }
            }
        }
        out
    }

    /// プロジェクトの表示名
    fn project_name(&self, cwd: &str) -> String {
        self.projects.iter().find(|p| p.cwd == cwd).map_or_else(|| base_name(cwd).to_string(), |p| p.name.clone())
    }

    /// 非対話で起動されたセッションを、同じディレクトリの他セッションで
    /// その時刻に実行中だったツール呼び出しの子とみなす
    fn find_exec_parents(&self) -> HashMap<usize, (usize, usize, f64)> {
        // 起動コマンドはすぐ制御を返すことがあるので、呼び出し終了後もしばらくは候補にする
        const SLACK: f64 = 30.0;
        let is_launch = |e: &crate::model::Event| e.kind == crate::model::Kind::Tool;
        let mut by_cwd: HashMap<&str, Vec<usize>> = Default::default();
        for &si in &self.order {
            by_cwd.entry(self.store.sessions[si].cwd.as_str()).or_default().push(si);
        }
        let mut out = HashMap::new();
        for group in by_cwd.values() {
            for &ci in group.iter().filter(|&&ci| self.store.sessions[ci].headless) {
                let t = self.store.sessions[ci].start;
                let mut best: Option<(usize, usize, f64)> = None;
                for &pi in group {
                    let p = &self.store.sessions[pi];
                    if pi == ci || p.start > t || p.end < t - SLACK {
                        continue;
                    }
                    for (li, l) in p.lanes.iter().enumerate() {
                        let hi = l.events.partition_point(|e| e.start <= t);
                        let lo = l.cum_end[..hi].partition_point(|&m| m < t - SLACK);
                        for e in l.events[lo..hi].iter().filter(|e| e.end >= t - SLACK && is_launch(e)) {
                            // 直前に起動したものを優先する
                            if best.is_none_or(|b| e.start > b.2) {
                                best = Some((pi, li, e.start));
                            }
                        }
                    }
                }
                if let Some(b) = best {
                    out.insert(ci, b);
                }
            }
        }
        // 循環を断つ
        let keys: Vec<usize> = out.keys().copied().collect();
        for k in keys {
            let mut cur = k;
            for _ in 0..=out.len() {
                match out.get(&cur) {
                    Some(&(p, _, _)) if p == k => {
                        out.remove(&k);
                        break;
                    }
                    Some(&(p, _, _)) => cur = p,
                    None => break,
                }
            }
        }
        out
    }

    /// 期間内にイベントを持つレーンを、セッション → レーンの順で並べる。
    /// 別セッションとして起動された子は起動元セッションの直下に並べる
    fn tracks_in(&self, v0: f64, v1: f64) -> Vec<Track> {
        let mut children: HashMap<usize, Vec<usize>> = Default::default();
        for &si in &self.order {
            if let Some(&(p, _, _)) = self.exec_parent.get(&si) {
                children.entry(p).or_default().push(si);
            }
        }
        let mut out = Vec::new();
        for &si in &self.order {
            if !self.exec_parent.contains_key(&si) {
                self.session_tracks(si, 0, None, &children, (v0, v1), &mut out);
            }
        }
        // 最初に現れた順を保ったままディレクトリごとにまとめる
        let mut first_seen: HashMap<&str, usize> = Default::default();
        for t in &out {
            let n = first_seen.len();
            first_seen.entry(self.store.sessions[t.session].cwd.as_str()).or_insert(n);
        }
        out.sort_by_key(|t| first_seen[self.store.sessions[t.session].cwd.as_str()]);
        out
    }

    /// セッション si のレーン（と子セッション）を out に積む。何か積んだら true
    fn session_tracks(
        &self,
        si: usize,
        base: usize,
        link: Option<(usize, usize, f64)>,
        children: &HashMap<usize, Vec<usize>>,
        (v0, v1): (f64, f64),
        out: &mut Vec<Track>,
    ) -> bool {
        let s = &self.store.sessions[si];
        let at = out.len();
        let mut kids = Vec::new();
        for &ci in children.get(&si).map(Vec::as_slice).unwrap_or(&[]) {
            self.session_tracks(ci, base + 1, self.exec_parent.get(&ci).copied(), children, (v0, v1), &mut kids);
        }
        let in_range = !(s.end < v0 || s.start > v1);
        if !in_range && kids.is_empty() {
            return false;
        }
        let visible = |li: usize| {
            let l = &s.lanes[li];
            let i = l.events.partition_point(|e| e.start < v0);
            l.events.get(i).is_some_and(|e| e.start <= v1) || (i > 0 && l.cum_end[i - 1] >= v0)
        };
        // 子レーンが見えていれば、親から main までの祖先も並べて紐づきを示す
        let mut show = vec![false; s.lanes.len()];
        if !kids.is_empty() && !show.is_empty() {
            show[0] = true;
        }
        for &(li, _) in &s.tree {
            if in_range && visible(li) {
                let mut cur = Some(li);
                while let Some(c) = cur {
                    if std::mem::replace(&mut show[c], true) && c != li {
                        break;
                    }
                    cur = s.parents.get(c).copied().flatten();
                }
            }
        }
        for &(li, depth) in &s.tree {
            if show[li] {
                out.push(Track { session: si, lane: li, depth: base + depth, link: if li == 0 { link } else { None } });
            }
        }
        out.extend(kids);
        out.len() > at
    }

    /// ディレクトリ見出しを挟んだ行。折りたたんだグループはレーンを出さない
    fn rows_in(&self, v0: f64, v1: f64) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        let mut cur: Option<String> = None;
        for t in self.tracks_in(v0, v1) {
            let cwd = &self.store.sessions[t.session].cwd;
            if cur.as_ref() != Some(cwd) {
                cur = Some(cwd.clone());
                rows.push(Row::Header {
                    cwd: cwd.clone(),
                    sessions: Vec::new(),
                    collapsed: self.collapsed.contains(cwd),
                });
            }
            let Some(Row::Header { sessions, collapsed, .. }) =
                rows.iter_mut().rev().find(|r| matches!(r, Row::Header { .. }))
            else {
                unreachable!()
            };
            if !sessions.contains(&t.session) {
                sessions.push(t.session);
            }
            if !*collapsed {
                rows.push(Row::Lane(t));
            }
        }
        rows
    }

    // ------------------------------------------------------------ 寸法

    /// サイドバーと詳細パネルの実際の幅。ウィンドウが狭いときはメイン列が残るよう、詳細パネル → サイドバーの順に縮める
    fn pane_widths(&self, window: &Window) -> (f32, f32) {
        let w: f32 = window.viewport_size().width.into();
        let room = w - RAIL_W - GAP - 2.0 - MAIN_MIN_W;
        let mut side = if self.sidebar_open { self.sidebar_w } else { 0.0 };
        let mut detail = if self.selected.is_some() { self.detail_w } else { 0.0 };
        if side + detail > room && detail > 0.0 {
            detail = (room - side).clamp(MIN_DETAIL_W, detail);
        }
        if side + detail > room && side > 0.0 {
            side = (room - detail).clamp(MIN_SIDEBAR_W, side);
        }
        (side, detail)
    }

    /// メイン列（見出し・本体・凡例）の幅
    fn main_width(&self, window: &Window) -> f32 {
        let w: f32 = window.viewport_size().width.into();
        let (side, detail) = self.pane_widths(window);
        (w - RAIL_W - GAP - 2.0 - side - detail).max(200.0)
    }

    /// メイン列の本体（見出しと凡例を除く）の高さ
    fn body_height(&self, window: &Window) -> f32 {
        let h: f32 = window.viewport_size().height.into();
        (h - STRIP_H - GAP - 2.0 - HEADER_H - FOOTER_H).max(100.0)
    }

    /// タイムラインのブロックを描く領域の幅
    fn plot_width(&self, window: &Window) -> f32 {
        (self.main_width(window) - self.label_w).max(100.0)
    }

    fn plot_height(&self, window: &Window) -> f32 {
        (self.body_height(window) - AXIS_H).max(100.0)
    }

    fn set_range(&mut self, start: f64, end: f64, window: &Window) {
        let w = self.plot_width(window) as f64;
        self.spp = ((end - start) / w).max(0.01);
        self.t0 = start;
        self.row_scroll = 0.0;
    }

    // ------------------------------------------------------------ 選択

    fn valid(&self, sel: Selection) -> bool {
        self.store
            .sessions
            .get(sel.session)
            .and_then(|s| s.lanes.get(sel.lane))
            .is_some_and(|l| sel.first <= sel.last && sel.last < l.events.len())
    }

    fn select(&mut self, sel: Selection, cx: &mut Context<Self>) {
        self.detail_changed(cx);
        self.sidebar_changed(cx);
        self.selected = Some(sel);
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let evs = &lane.events[sel.first..=sel.last];
        let n = evs.len();
        let limit = if n == 1 {
            6000
        } else if n < 10 {
            1500
        } else {
            300
        };
        let rows: Vec<i64> = evs.iter().take(DETAIL_MAX).filter_map(|e| e.row).collect();
        let mut fetched = self.reader.as_ref().map(|db| db.details(&rows)).unwrap_or_default();
        let mut raws = Vec::new();
        let texts = evs
            .iter()
            .take(DETAIL_MAX)
            .map(|e| {
                let (input, output) = match e.row.and_then(|r| fetched.remove(&r)) {
                    Some(v) => v,
                    None => (e.input.clone(), e.output.clone()),
                };
                let (input, raw) = match input.strip_prefix(crate::loader::JS_MARK) {
                    Some(js) => (detail::exec_detail_cached(self.js_cache.as_ref(), js), Some(js.to_string())),
                    None => (input, None),
                };
                raws.push(raw);
                (crate::model::truncate(&input, limit), crate::model::truncate(&output, limit))
            })
            .collect();
        self.detail_cache = Some(DetailCache { sel, texts, raws });
    }

    /// 現在の選択を履歴に積んでから選択する。詳細パネルが開いて表示幅が縮むときは、選択が隠れないようにする
    fn navigate(&mut self, sel: Selection, window: &Window, cx: &mut Context<Self>) {
        let opening = self.selected.is_none();
        if let Some(cur) = self.selected.filter(|c| *c != sel) {
            self.push_back(cur);
            self.forward.clear();
        }
        self.select(sel, cx);
        if opening {
            self.reveal(sel, false, window);
        }
        cx.notify();
    }

    fn push_back(&mut self, sel: Selection) {
        self.back.push(sel);
        if self.back.len() > 100 {
            self.back.remove(0);
        }
    }

    /// 履歴を遡って直前の選択に戻る（更新で無効になった項目は飛ばす）
    fn go_back(&mut self, window: &Window, cx: &mut Context<Self>) {
        while let Some(sel) = self.back.pop() {
            if self.valid(sel) {
                if let Some(cur) = self.selected {
                    self.forward.push(cur);
                }
                self.select(sel, cx);
                self.reveal(sel, false, window);
                break;
            }
        }
        cx.notify();
    }

    fn go_forward(&mut self, window: &Window, cx: &mut Context<Self>) {
        while let Some(sel) = self.forward.pop() {
            if self.valid(sel) {
                if let Some(cur) = self.selected {
                    self.push_back(cur);
                }
                self.select(sel, cx);
                self.reveal(sel, false, window);
                break;
            }
        }
        cx.notify();
    }

    /// 詳細パネルを閉じる。閉じた選択は「戻る」で開き直せる
    fn close_detail(&mut self, cx: &mut Context<Self>) {
        if let Some(cur) = self.selected.take() {
            self.push_back(cur);
            self.forward.clear();
        }
        self.detail_cache = None;
        self.sidebar_changed(cx);
        cx.notify();
    }

    /// 選択の時間範囲 (開始, 終了)
    fn sel_range(&self, sel: Selection) -> (f64, f64) {
        let evs = &self.store.sessions[sel.session].lanes[sel.lane].events[sel.first..=sel.last];
        let a = evs.first().map_or(0.0, |e| e.start);
        (a, evs.iter().map(|e| e.end).fold(a, f64::max))
    }

    /// 選択を表示中のビューに映す。`zoom` なら収まらないときや縮小しすぎのときに拡大率も合わせる。
    /// タイムラインではレーンの行までスクロールし、カレンダーではその日の該当時刻を出す
    fn reveal(&mut self, sel: Selection, zoom: bool, window: &Window) {
        let (a, b) = self.sel_range(sel);
        match self.mode {
            Mode::Timeline => {
                let w = self.plot_width(window) as f64;
                let span = self.spp * w;
                let dur = b - a;
                // 1 イベントなら前後が分かる程度（30 分）まで寄せ、範囲なら全体が収まるようにする
                let fit =
                    if sel.first == sel.last { (dur * 3.0).max(30.0 * 60.0) } else { (dur * 1.3).max(20.0 * 60.0) };
                if zoom && (fit < span * 0.5 || fit > span) {
                    self.spp = (fit / w).clamp(0.005, 86400.0 * 3.0);
                    self.t0 = (a + b) / 2.0 - fit / 2.0;
                } else if b < self.t0 || a > self.t0 + span {
                    // 見えていなければ拡大率はそのままで中央に寄せる（収まらない長さなら先頭を見せる）
                    self.t0 = if dur > span * 0.9 { a - span * 0.05 } else { (a + b) / 2.0 - span / 2.0 };
                }
                self.scroll_to_lane(sel.session, sel.lane, window);
            }
            Mode::Day | Mode::Week => {
                let day = local(a).date_naive();
                let first = if self.mode == Mode::Day { self.day0 } else { self.week_start() };
                let days = if self.mode == Mode::Day { 1 } else { 7 };
                if day < first || day >= first + Duration::days(days) {
                    self.day0 = day;
                }
                let ph = self.body_height(window) - AXIS_H;
                let y = ((a - midnight(day)) / 3600.0) as f32 * self.px_per_hour;
                if y < self.cal_scroll || y > self.cal_scroll + ph - 20.0 {
                    let max = (24.0 * self.px_per_hour - ph).max(0.0);
                    self.cal_scroll = (y - ph / 3.0).clamp(0.0, max);
                }
            }
        }
    }

    /// タイムラインでレーンの行が見えるように縦にスクロールする（折りたたまれていれば開く）
    fn scroll_to_lane(&mut self, session: usize, lane: usize, window: &Window) {
        let v0 = self.t0;
        let v1 = self.t0 + self.spp * self.plot_width(window) as f64;
        let cwd = self.store.sessions[session].cwd.clone();
        if self.collapsed.remove(&cwd) {
            self.persist();
        }
        let rows = self.rows_in(v0, v1);
        let Some(i) = rows.iter().position(|r| matches!(r, Row::Lane(t) if t.session == session && t.lane == lane))
        else {
            return;
        };
        let y = i as f32 * ROW_H;
        let ph = self.plot_height(window);
        if y < self.row_scroll || y + ROW_H > self.row_scroll + ph {
            self.row_scroll = (y - ph / 3.0).max(0.0);
        }
    }

    /// セッションが隠れる絞り込み（読み込み元・プロジェクト）を外す
    fn unhide(&mut self, si: usize) {
        let s = &self.store.sessions[si];
        let mut changed = false;
        let hidden = match self.source {
            SourceFilter::All => false,
            SourceFilter::Claude => s.source != Source::Claude,
            SourceFilter::Codex => s.source != Source::Codex,
        };
        if hidden {
            self.source = SourceFilter::All;
            changed = true;
        }
        if self.focus.as_ref().is_some_and(|f| *f != s.cwd) {
            self.focus = None;
            changed = true;
        }
        if changed {
            self.rebuild_order();
            self.persist();
        }
    }

    /// セッション全体（main レーン）を選んで表示する
    fn open_session(&mut self, si: usize, window: &Window, cx: &mut Context<Self>) {
        let n = self.store.sessions[si].lanes.first().map_or(0, |l| l.events.len());
        if n == 0 {
            return;
        }
        self.unhide(si);
        let sel = Selection { session: si, lane: 0, first: 0, last: n - 1 };
        self.navigate(sel, window, cx);
        self.reveal(sel, true, window);
        self.sidebar_changed(cx);
    }

    /// 1 つのイベントを選んで表示する
    fn open_event(&mut self, session: usize, lane: usize, event: usize, window: &Window, cx: &mut Context<Self>) {
        self.unhide(session);
        let sel = Selection { session, lane, first: event, last: event };
        self.navigate(sel, window, cx);
        self.reveal(sel, true, window);
    }

    // ------------------------------------------------------------ 検索

    fn refresh_results(&mut self) {
        self.results =
            (!self.query.is_empty()).then(|| self.search.search(&self.store, &self.order, &self.query, SEARCH_LIMIT));
        let n = self.results.as_ref().map_or(0, |r| r.len());
        self.result_cursor = self.result_cursor.min(n.saturating_sub(1));
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.refresh_results();
        if self.results.is_some() {
            self.sidebar_open = true;
        }
        self.sidebar_changed(cx);
        cx.notify();
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popover = None;
        if !self.sidebar_open {
            self.sidebar_open = true;
            self.persist();
        }
        window.focus(&self.search_input.focus_handle(cx));
        self.search_input.update(cx, |i, cx| i.select_all_text(cx));
        self.sidebar_changed(cx);
        cx.notify();
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search_input.update(cx, |i, cx| i.set_text("", cx));
    }

    /// 検索結果の i 番目（セッション → イベントの順）を開く
    fn open_result(&mut self, i: usize, window: &Window, cx: &mut Context<Self>) {
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

    fn move_cursor(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.results.as_ref().map_or(0, |r| r.len());
        if n == 0 {
            return;
        }
        self.result_cursor = (self.result_cursor as isize + d).clamp(0, n as isize - 1) as usize;
        self.sidebar_scroll.scroll_to_item(self.result_row(self.result_cursor));
        self.sidebar_changed(cx);
    }

    // ------------------------------------------------------------ 表示の切り替え

    /// 表示を切り替える。タイムラインとカレンダーの間では、見ていた時刻が画面に残るようにする
    fn set_mode(&mut self, mode: Mode, window: &Window) {
        let from = self.mode;
        if from == Mode::Timeline && mode != Mode::Timeline {
            let span = self.spp * self.plot_width(window) as f64;
            let tc = (self.t0 + span / 2.0).min(now());
            self.day0 = local(tc).date_naive();
            self.mode = mode;
            self.scroll_calendar_to(tc, 0.5, window);
        } else if from != Mode::Timeline && mode == Mode::Timeline {
            let ph = self.body_height(window) - AXIS_H;
            let tc = midnight(self.day0) + ((self.cal_scroll + ph / 2.0) / self.px_per_hour) as f64 * 3600.0;
            self.mode = mode;
            let span = self.spp * self.plot_width(window) as f64;
            // 現在時刻より先は 15% までしか見せない
            self.t0 = (tc - span / 2.0).min(now() - span * 0.85);
        }
        self.mode = mode;
        if mode != Mode::Timeline {
            self.calendar = mode;
        }
        self.popover = None;
        self.persist();
    }

    /// カレンダーを、時刻 `t` が画面の上から `at`（0〜1）の位置に来るようにスクロールする
    fn scroll_calendar_to(&mut self, t: f64, at: f32, window: &Window) {
        let ph = self.body_height(window) - AXIS_H;
        let y = ((t - midnight(local(t).date_naive())) / 3600.0) as f32 * self.px_per_hour;
        let max = (24.0 * self.px_per_hour - ph).max(0.0);
        self.cal_scroll = (y - ph * at).clamp(0.0, max);
    }

    fn set_source(&mut self, s: SourceFilter, cx: &mut Context<Self>) {
        self.source = s;
        self.rebuild_order();
        if !self.query.is_empty() {
            self.refresh_results();
        }
        self.row_scroll = 0.0;
        self.persist();
        self.sidebar_changed(cx);
    }

    fn set_focus(&mut self, f: Option<String>, cx: &mut Context<Self>) {
        self.focus = f;
        self.rebuild_order();
        if !self.query.is_empty() {
            self.refresh_results();
        }
        self.row_scroll = 0.0;
        self.session_limit = 50;
        self.persist();
        self.sidebar_changed(cx);
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.persist();
        cx.notify();
    }

    fn step(&mut self, dir: f64, window: &Window) {
        match self.mode {
            Mode::Day => self.day0 += Duration::days(dir as i64),
            Mode::Week => self.day0 += Duration::days(7 * dir as i64),
            Mode::Timeline => self.t0 += dir * self.spp * self.plot_width(window) as f64,
        }
    }

    fn go_today(&mut self, window: &Window) {
        self.day0 = Local::now().date_naive();
        match self.mode {
            Mode::Timeline => {
                let width = self.spp * self.plot_width(window) as f64;
                self.t0 = now() - width * 0.85;
            }
            Mode::Day | Mode::Week => self.scroll_calendar_to(now(), 0.6, window),
        }
    }

    /// 直近の作業が見えるようにする（表示範囲に何もないとき用）
    fn go_latest(&mut self, window: &Window) {
        let Some(end) = self.order.iter().map(|&si| self.store.sessions[si].end).reduce(f64::max) else {
            return;
        };
        match self.mode {
            Mode::Timeline => {
                let span = (self.spp * self.plot_width(window) as f64).min(86400.0);
                self.set_range(end - span * 0.85, end + span * 0.15, window);
            }
            Mode::Day | Mode::Week => {
                self.day0 = local(end).date_naive();
                self.scroll_calendar_to(end, 0.6, window);
            }
        }
    }

    fn zoom(&mut self, f: f64, window: &Window) {
        match self.mode {
            Mode::Timeline => {
                let w = self.plot_width(window) as f64;
                let center = self.t0 + w * self.spp / 2.0;
                self.spp = (self.spp * f).clamp(0.005, 86400.0 * 3.0);
                self.t0 = center - w * self.spp / 2.0;
            }
            _ => {
                let h = self.body_height(window) - AXIS_H;
                let center = (self.cal_scroll + h / 2.0) / self.px_per_hour;
                self.px_per_hour = (self.px_per_hour / f as f32).clamp(20.0, 20000.0);
                let max = (24.0 * self.px_per_hour - h).max(0.0);
                self.cal_scroll = (center * self.px_per_hour - h / 2.0).clamp(0.0, max);
            }
        }
    }

    fn week_start(&self) -> NaiveDate {
        self.day0 - Duration::days(self.day0.weekday().num_days_from_monday() as i64)
    }

    // ------------------------------------------------------------ 入力

    fn on_wheel(&mut self, e: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let d = e.delta.pixel_delta(px(16.0));
        let (dx, dy): (f32, f32) = (d.x.into(), d.y.into());
        let zoom = e.modifiers.platform || e.modifiers.control || e.modifiers.alt;
        match self.mode {
            Mode::Timeline => {
                if zoom {
                    self.zoom_at(e.position, (-(dy as f64) * 0.01).exp(), window);
                } else if e.modifiers.shift {
                    self.t0 -= (dy + dx) as f64 * self.spp;
                } else {
                    self.t0 -= dx as f64 * self.spp;
                    self.row_scroll = (self.row_scroll - dy).max(0.0);
                }
            }
            Mode::Day | Mode::Week => {
                if zoom {
                    self.zoom_at(e.position, (-(dy as f64) * 0.01).exp(), window);
                } else {
                    self.cal_scroll -= dy;
                    let max = (24.0 * self.px_per_hour - self.plot_height(window)).max(0.0);
                    self.cal_scroll = self.cal_scroll.clamp(0.0, max);
                }
            }
        }
        self.hovered = None;
        cx.notify();
    }

    /// トラックパッドのピンチ。`m` は拡大率の差分（正で拡大）
    fn on_pinch(&mut self, m: f64, window: &mut Window, cx: &mut Context<Self>) {
        let pos = window.mouse_position();
        match self.mode {
            Mode::Timeline => self.zoom_at(pos, 1.0 / (1.0 + m).max(0.1), window),
            Mode::Day | Mode::Week => self.zoom_at(pos, (1.0 + m).max(0.1), window),
        }
        cx.notify();
    }

    /// `pos` を基準に拡大・縮小する。`f` はタイムラインでは spp、カレンダーでは px_per_hour に掛ける倍率
    fn zoom_at(&mut self, pos: Point<Pixels>, f: f64, window: &Window) {
        let o = self.hit_origin.get();
        let mx: f32 = (pos.x - o.x).into();
        let my: f32 = (pos.y - o.y).into();
        match self.mode {
            Mode::Timeline => {
                let x = mx.clamp(0.0, self.plot_width(window)) as f64;
                let anchor = self.t0 + x * self.spp;
                self.spp = (self.spp * f).clamp(0.005, 86400.0 * 3.0);
                self.t0 = anchor - x * self.spp;
            }
            Mode::Day | Mode::Week => {
                let anchor_h = (my + self.cal_scroll) / self.px_per_hour;
                self.px_per_hour = (self.px_per_hour * f as f32).clamp(20.0, 20000.0);
                self.cal_scroll = anchor_h * self.px_per_hour - my;
                let max = (24.0 * self.px_per_hour - self.plot_height(window)).max(0.0);
                self.cal_scroll = self.cal_scroll.clamp(0.0, max);
            }
        }
    }

    fn on_down(&mut self, e: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let scroll = if self.mode == Mode::Timeline { self.row_scroll } else { self.cal_scroll };
        self.drag = Some(Drag { origin: e.position, t0: self.t0, scroll, moved: false });
    }

    /// 位置にあるブロック（上に描いたものを優先）
    fn hit(&self, pos: Point<Pixels>) -> Option<Selection> {
        let o = self.hit_origin.get();
        let (x, y): (f32, f32) = ((pos.x - o.x).into(), (pos.y - o.y).into());
        self.hits
            .iter()
            .rev()
            .find(|((bx, by, bw, bh), _)| x >= *bx && x < bx + bw && y >= *by && y < by + bh)
            .map(|(_, s)| *s)
    }

    fn on_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            let h = self.hit(e.position);
            if h != self.hovered {
                self.hovered = h;
                self.hover_at = e.position;
                cx.notify();
            }
            return;
        }
        let max = (24.0 * self.px_per_hour - self.plot_height(window)).max(0.0);
        let Some(d) = self.drag.as_mut() else { return };
        let dx: f32 = (e.position.x - d.origin.x).into();
        let dy: f32 = (e.position.y - d.origin.y).into();
        if dx.abs() + dy.abs() > 3.0 {
            d.moved = true;
        }
        if !d.moved {
            return;
        }
        self.hovered = None;
        match self.mode {
            Mode::Timeline => {
                self.t0 = d.t0 - dx as f64 * self.spp;
                self.row_scroll = (d.scroll - dy).max(0.0);
            }
            _ => {
                self.cal_scroll = (d.scroll - dy).clamp(0.0, max);
            }
        }
        cx.notify();
    }

    fn on_up(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        if d.moved {
            return;
        }
        let Some(sel) = self.hit(e.position) else { return };
        if e.click_count >= 2 && self.mode == Mode::Timeline {
            // ダブルクリックでブロックの範囲まで拡大する
            let (a, b) = self.sel_range(sel);
            let pad = ((b - a) * 0.1).max(5.0);
            let w = self.plot_width(window) as f64;
            self.spp = ((b - a + 2.0 * pad) / w).clamp(0.005, 86400.0 * 3.0);
            self.t0 = a - pad;
        }
        self.navigate(sel, window, cx);
    }

    fn start_resize(&mut self, r: Resize, cx: &mut Context<Self>) {
        self.resize = Some(r);
        self.drag = None;
        cx.stop_propagation();
    }

    fn on_resize_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.resize else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.resize = None;
            self.persist();
            return;
        }
        let x: f32 = e.position.x.into();
        let win: f32 = window.viewport_size().width.into();
        // メイン列が残るようにする
        let room = |others: f32| win - RAIL_W - GAP - 2.0 - others - MAIN_MIN_W;
        let (side, detail) = self.pane_widths(window);
        match r {
            Resize::Sidebar { x: x0, w } => {
                let max = room(detail).clamp(MIN_SIDEBAR_W, MAX_SIDEBAR_W);
                self.sidebar_w = (w + x - x0).clamp(MIN_SIDEBAR_W, max);
            }
            Resize::Label { x: x0, w } => {
                let max = (self.main_width(window) - 100.0).max(MIN_PANE_W);
                self.label_w = (w + x - x0).clamp(MIN_PANE_W, max);
            }
            Resize::Detail { x: x0, w } => {
                self.detail_w = (w - (x - x0)).clamp(MIN_DETAIL_W, room(side).max(MIN_DETAIL_W));
            }
        }
        self.detail_changed(cx);
        self.sidebar_changed(cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn end_resize(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if self.resize.take().is_some() {
            self.persist();
        }
    }

    /// 境界のつまみ。ドラッグで幅を変える
    fn resize_handle(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
        f: impl Fn(&Self, f32) -> Resize + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(HANDLE_W))
            .cursor_col_resize()
            .hover(|s| s.bg(c(theme::ACCENT).opacity(0.5)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    let r = f(t, e.position.x.into());
                    t.start_resize(r, cx);
                }),
            )
    }

    // ------------------------------------------------------------ キー操作

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popover.take().is_some() {
            cx.notify();
        } else if self.selected.is_some() {
            self.close_detail(cx);
        } else if !self.query.is_empty() {
            self.clear_search(cx);
        } else {
            window.focus(&self.focus_handle);
        }
    }

    fn cancel_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popover.take().is_some() {
            cx.notify();
        } else if self.search_input.read(cx).text().is_empty() {
            window.focus(&self.focus_handle);
            self.sidebar_changed(cx);
        } else {
            self.clear_search(cx);
        }
    }
}

impl Focusable for Rireki {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Rireki {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.mode {
            Mode::Timeline => self.timeline(window, cx),
            _ => self.calendar(window, cx),
        };
        let detail_open = self.selected.is_some();
        let (side_w, detail_w) = self.pane_widths(window);
        let label_handle = self
            .resize_handle("resize-label", cx, |t, x| Resize::Label { x, w: t.label_w })
            .left(px(self.label_w - HANDLE_W / 2.0));
        let body = div()
            .id("main")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .relative()
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
            .on_hover(cx.listener(|t, hovered: &bool, _, cx| {
                if !*hovered && t.hovered.take().is_some() {
                    cx.notify();
                }
            }))
            .child(main)
            .when(self.mode == Mode::Timeline, |d| d.child(label_handle));
        // 背景はパネルのものを使う（角に接するので、塗ると角丸の枠線を覆ってしまう）
        let main_col = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(self.header(window, cx))
            .child(body)
            .child(self.footer(window));

        let mut panel = div()
            .flex_1()
            .min_w_0()
            .mr(px(GAP))
            .mb(px(GAP))
            .relative()
            .flex()
            .rounded(px(PANEL_RADIUS))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::BG))
            .overflow_hidden();
        if self.sidebar_open {
            panel = panel.child(
                AnyView::from(self.sidebar_view.clone())
                    .cached(StyleRefinement::default().w(px(side_w)).h_full().flex_none()),
            );
        }
        panel = panel.child(main_col);
        if detail_open {
            panel = panel.child(
                AnyView::from(self.detail_view.clone())
                    .cached(StyleRefinement::default().w(px(detail_w)).h_full().flex_none()),
            );
        }
        // つまみはドラッグ開始時の実際の幅から動かす
        if self.sidebar_open {
            panel = panel.child(
                self.resize_handle("resize-sidebar", cx, move |_, x| Resize::Sidebar { x, w: side_w })
                    .left(px(side_w - HANDLE_W / 2.0)),
            );
        }
        if detail_open {
            panel = panel.child(
                self.resize_handle("resize-detail", cx, move |_, x| Resize::Detail { x, w: detail_w })
                    .right(px(detail_w - HANDLE_W / 2.0)),
            );
        }

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .key_context("Rireki")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .on_action(cx.listener(|t, _: &FocusSearch, window, cx| t.focus_search(window, cx)))
            .on_action(cx.listener(|t, _: &ToggleSidebar, _, cx| t.toggle_sidebar(cx)))
            .on_action(cx.listener(|t, _: &ShowTimeline, window, cx| {
                t.set_mode(Mode::Timeline, window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &ShowDay, window, cx| {
                t.set_mode(Mode::Day, window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &ShowWeek, window, cx| {
                t.set_mode(Mode::Week, window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &GoToday, window, cx| {
                t.go_today(window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &GoPrev, window, cx| {
                t.step(-1.0, window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &GoNext, window, cx| {
                t.step(1.0, window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &ZoomIn, window, cx| {
                t.zoom(0.5, window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &ZoomOut, window, cx| {
                t.zoom(2.0, window);
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &Back, window, cx| t.go_back(window, cx)))
            .on_action(cx.listener(|t, _: &Forward, window, cx| t.go_forward(window, cx)))
            .on_action(cx.listener(|t, _: &Dismiss, window, cx| t.dismiss(window, cx)))
            .on_action(cx.listener(|t, _: &ShowHelp, _, cx| {
                t.popover = if t.popover == Some(Popover::Help) { None } else { Some(Popover::Help) };
                cx.notify();
            }))
            .on_action(cx.listener(|t, _: &SearchUp, _, cx| t.move_cursor(-1, cx)))
            .on_action(cx.listener(|t, _: &SearchDown, _, cx| t.move_cursor(1, cx)))
            .on_action(cx.listener(|t, _: &SearchConfirm, window, cx| {
                let i = t.result_cursor;
                t.open_result(i, window, cx);
            }))
            .on_action(cx.listener(|t, _: &SearchCancel, window, cx| t.cancel_search(window, cx)))
            .on_mouse_move(cx.listener(Self::on_resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::end_resize))
            .bg(c(theme::BACKDROP))
            .text_color(c(theme::TEXT))
            .child(self.top_strip())
            .child(div().flex().flex_1().min_h_0().child(self.rail(cx)).child(panel))
            .children(self.overlays(window, cx))
    }
}
