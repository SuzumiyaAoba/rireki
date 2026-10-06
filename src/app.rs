//! メインウィンドウ（GPUI のビュー）。Slack のように、左端のアイコンレール・サイドバー・メイン・詳細パネルを並べる
//!
//! - 上端の帯: macOS のタイトルバー領域。信号機ボタンと状態表示だけを置き、ドラッグでウィンドウを動かせる
//! - レール (`rail.rs`): 読み込み元（すべて / Claude / Codex）と表示（タイムライン / カレンダー / 統計）の切り替え
//! - サイドバー (`sidebar.rs`): 検索欄と、進行中・プロジェクト・セッションの一覧、または検索結果
//! - メイン: タイムライン (`timeline.rs`) / カレンダー (`calendar.rs`) と見出し・凡例、または統計 (`statistics.rs`)
//! - 詳細パネル (`detail.rs`): 選択したブロックの内容。選択している間だけ開く
//! - 重ねて出すもの (`overlay.rs`): ホバーカード、まとめ方のメニュー、ショートカット一覧

mod calendar;
mod cost;
mod detail;
mod header;
mod keymap;
mod overlay;
mod profile;
mod rail;
mod range;
mod rich;
mod sidebar;
mod statistics;
mod timeline;
mod widgets;

use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{
    AnyView, App, Bounds, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollHandle, ScrollWheelEvent, StyleRefinement, Window, div, point, prelude::*, px,
};

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use crate::index::{self, Delta, ViewLoader};
use crate::input::{InputEvent, TextInput};
use crate::layout::Granularity;
use crate::model::{Session, Snapshot, Source};
use crate::prefs::{self, DetailTab, Mode, Prefs, SourceFilter};
use crate::search;
use crate::theme::{self, c};

use detail::{DetailList, DetailView, FileList, RowFilter};
use crate::detail::{DetailCache, DetailLoader, Selection, EVENT_LIMIT as DETAIL_MAX};
use crate::time::{local, midnight};
use crate::view_index;
pub use keymap::init;
use keymap::{
    Back, CancelAnalysisSearch, CloseWindow, DetailAnalysis, DetailEvents, DetailFiles, DetailSession,
    Dismiss, FocusSearch, Forward, GoNext, GoPrev, ToggleDetailFullscreen,
    GoToday, NextEvent, PrevEvent, SearchCancel, SearchConfirm, SearchDown, SearchUp, ShowDay, ShowHelp, ShowTimeline,
    ShowWeek, ShowStatistics, ToggleInViewOnly, ToggleSidebar, ZoomIn, ZoomOut,
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
/// タイムラインの上のミニマップの高さ
const MINIMAP_H: f32 = 30.0;
const AXIS_H: f32 = 30.0;
/// 境界ドラッグで変えられる幅の下限
const MIN_PANE_W: f32 = 160.0;
const MIN_SIDEBAR_W: f32 = 200.0;
const MAX_SIDEBAR_W: f32 = 480.0;
const MIN_DETAIL_W: f32 = 320.0;
/// サイドバー・詳細パネルの開閉にかける秒数
const PANE_ANIM_SECS: f32 = 0.18;
/// メイン列に残す幅
const MAIN_MIN_W: f32 = 360.0;
/// 境界のつまみの幅
const HANDLE_W: f32 = 6.0;
const ROW_H: f32 = 24.0;
const HOUR_LABEL_W: f32 = 52.0;
const MIN_BLOCK: f32 = 3.0;
/// 検索結果に並べるイベント数の上限
const SEARCH_LIMIT: usize = 300;
/// 入力が止まってから検索するまでの待ち時間
const SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(120);
/// リアルタイムファイルの変更を確認する間隔
const POLL: std::time::Duration = std::time::Duration::from_secs(3);

struct Track {
    session: usize,
    lane: usize,
    /// サブエージェントの入れ子の深さ（main = 0）
    depth: usize,
    /// 別セッションとして起動された子の main レーンなら、起動元 (セッション, レーン, 時刻)
    link: Option<(usize, usize, f64)>,
    /// 表示範囲内にイベントがあるか（なければグレーアウトする）
    active: bool,
}

/// タイムラインの 1 行
enum Row {
    /// ディレクトリ単位のグループ見出し
    Header {
        cwd: String,
        sessions: Vec<usize>,
        collapsed: bool,
        /// グループ内に表示範囲内のレーンがあるか
        active: bool,
        /// ディレクトリ表示では見出し行にもイベントを描く
        blocks: Vec<crate::layout::directory::SessionBlock>,
    },
    Lane(Track),
    /// ディレクトリ表示で、同時刻に重なる枠だけを置く追加行
    Packed(Vec<crate::layout::directory::SessionBlock>),
}

impl Row {
    fn packed_blocks(&self) -> &[crate::layout::directory::SessionBlock] {
        match self {
            Self::Header { blocks, .. } | Self::Packed(blocks) => blocks,
            Self::Lane(_) => &[],
        }
    }
}

/// canvas に描くブロック（座標は描画領域の左上基準）
struct BlockPaint {
    rect: (f32, f32, f32, f32),
    sel: Selection,
    color: u32,
    /// 主なイベントの種類のアイコン
    icon: crate::icons::Icon,
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
    /// 表示期間を選ぶカレンダー
    Range,
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

/// ミニマップのキャッシュのキー (期間の始め, 終わり, 本数, data_gen)
type MinimapKey = (u64, u64, usize, u64);

pub struct Rireki {
    store: Snapshot,
    layout_cache: RefCell<crate::layout::Cache>,
    calendar_cache: calendar::Cache,
    /// 読み込み元で絞り込んだセッション（開始順）
    scope: Vec<usize>,
    /// scope をさらにプロジェクトで絞り込んだ、表示するセッション（開始順）
    order: Vec<usize>,
    /// scope のプロジェクト（最近作業した順）
    projects: Vec<Project>,
    /// `codex exec` などで起動された子セッション → 起動元 (セッション, レーン, 時刻)
    exec_parent: HashMap<usize, (usize, usize, f64)>,
    loading: Option<(usize, usize)>,
    profile: Option<profile::State>,
    detail_loader: Arc<Mutex<DetailLoader>>,
    detail_gen: Arc<AtomicU64>,
    detail_task: Option<gpui::Task<()>>,
    detail_follow: bool,
    detail_following: bool,
    focus_handle: FocusHandle,
    search_input: Entity<TextInput>,
    search: Arc<Mutex<search::Index>>,
    search_task: Option<gpui::Task<()>>,
    searching: bool,
    /// 前後の空白を除いた検索語
    query: String,
    /// 検索結果。検索語が空なら None
    results: Option<search::Results>,
    /// 検索結果のうち、キーボードで選んでいる項目
    result_cursor: usize,
    /// 検索語を変えるたびに増やす。待っている間に次の入力があった検索は捨てる
    search_gen: Arc<AtomicU64>,
    sidebar_view: Entity<SidebarView>,
    sidebar_scroll: ScrollHandle,
    navigator_rendered: Cell<usize>,
    navigator_virtual: Cell<bool>,
    navigator_list: sidebar::NavigatorList,
    result_list: gpui::ListState,
    result_list_key: Cell<(u64, usize, u32)>,
    detail_view: Entity<DetailView>,
    detail_cache: Option<DetailCache>,
    /// 表示中の詳細と同じ版のセッションと、読み込み完了時の世代
    detail_snapshot: Option<(Arc<Session>, u64)>,
    /// exec の詳細を整形結果ではなく JS 本文で表示する
    show_raw: bool,
    /// 詳細パネルのイベント一覧の絞り込み（選択が変わると解除する）
    row_filter: Option<RowFilter>,
    /// 詳細パネルで、長い本文を全部表示している箇所
    expanded: HashSet<&'static str>,
    /// 履歴内で入力・結果や思考を展開しているイベント位置
    history_expanded: HashSet<usize>,
    /// 詳細パネルのファイルのタブで開いているファイル
    file_open: Option<String>,
    /// 詳細パネルで開いているタブ
    detail_tab: DetailTab,
    /// 詳細パネルのイベントを新しい順に表示するか
    detail_reversed: bool,
    /// 詳細だけをウィンドウ全体に表示する。通常のパネル幅は保持する。
    detail_fullscreen: bool,
    analysis_state: detail::analysis::State,
    detail_scroll: ScrollHandle,
    detail_scroll_offsets: [gpui::Point<gpui::Pixels>; 4],
    overview_list: detail::overview::OverviewList,
    detail_list: DetailList,
    replay: Option<detail::replay::State>,
    replay_speed: f64,
    replay_task: Option<gpui::Task<()>>,
    replay_autostart: Option<Selection>,
    file_list: FileList,
    /// 詳細パネルを最新のイベントまでスクロールするアニメーション（差し替えると前のものは止まる）
    follow_anim: Option<gpui::Task<()>>,
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
    statistics_open: bool,
    statistics_state: statistics::State,
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
    /// タイムラインで表示範囲内のセッションだけを並べるか
    in_view_only: bool,
    /// ディレクトリごとにメインイベントを最小限の行に詰める
    directory_timeline: bool,
    /// API 換算料金を時間ごとに表示する
    cost_timeline: bool,
    cost_state: cost::State,
    source: SourceFilter,
    /// 読み込み元ごとに、進行中のセッションがあるか (Claude, Codex)
    live_sources: (bool, bool),
    selected: Option<Selection>,
    /// 選択の履歴（戻る / 進む）
    back: Vec<(Selection, DetailTab)>,
    forward: Vec<(Selection, DetailTab)>,
    drag: Option<Drag>,
    sidebar_open: bool,
    sidebar_w: f32,
    /// タイムラインのレーン名の列の幅
    label_w: f32,
    detail_w: f32,
    /// サイドバー・詳細パネルの開き具合（0 = 閉、1 = 開）。開閉のアニメーションで目標へ近づける
    side_t: f32,
    detail_t: f32,
    /// 前回アニメーションを進めた時刻。アニメーション中だけ持つ
    anim_at: Option<Instant>,
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
    /// カレンダーで選んだ表示期間。ミニマップはこの期間全体を表す
    pick: Option<(f64, f64)>,
    /// 期間を選ぶカレンダーで表示している月（1 日）
    picker_month: NaiveDate,
    /// 期間を選ぶカレンダーで、先に選んだ開始日
    picker_from: Option<NaiveDate>,
    /// 期間を選ぶボタンの位置
    picker_anchor: Rc<Cell<Bounds<Pixels>>>,
    /// ミニマップの位置（ウィンドウ座標）
    minimap_bounds: Rc<Cell<Bounds<Pixels>>>,
    minimap_drag: bool,
    /// タイムライン右端の、表示範囲内のレーンの位置を示す縦のミニマップ
    lane_map_bounds: Rc<Cell<Bounds<Pixels>>>,
    lane_map_drag: bool,
    /// ミニマップの棒の高さ。キーは (期間の始め, 終わり, 本数, data_gen)
    minimap_cache: Option<(MinimapKey, Vec<u32>)>,
    /// 表示するセッションが変わるたびに増やす
    data_gen: u64,
    /// 表示中のタイムラインの行。縦スクロールや選択では作り直さない。
    timeline_cache: RefCell<timeline::Cache>,
    /// 下端に出す、表示中の件数
    counts: String,
    /// 最後に保存した画面の状態
    saved: Prefs,
}

fn now() -> f64 {
    Local::now().timestamp_millis() as f64 / 1000.0
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
    pub fn new(window: &mut Window, cx: &mut Context<Self>, mut profile_tasks: Option<Vec<index::Task>>) -> Self {
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
        let profile_ui = std::env::args().any(|a| a == "--profile-ui");
        let args: Vec<_> = std::env::args().collect();
        let profile_roots = profile_ui.then(|| args.iter().position(|a| a == "--profile-idle")).flatten().map(|i| {
            (std::path::PathBuf::from(args.get(i + 1).expect("Claude の探索先を指定してください")),
             std::path::PathBuf::from(args.get(i + 2).expect("Codex の探索先を指定してください")))
        });
        cx.spawn(async move |this, cx| {
            let bg = cx.background_executor().clone();
            // 1. 索引 DB から安定した履歴を読む
            let indexer = bg.spawn(async { Arc::new(Mutex::new(ViewLoader::open())) }).await;
            let ix = indexer.clone();
            let stable = bg.spawn(async move { ix.lock().unwrap().load_stable() }).await;
            let alive = this
                .update(cx, |app, cx| {
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
                let planned = profile_ui.then(std::time::Instant::now);
                let tasks = if let Some(tasks) = profile_tasks.take() {
                    tasks
                } else if let Some((claude, codex)) = &profile_roots {
                    let (claude, codex) = (claude.clone(), codex.clone());
                    bg.spawn(async move { ix.lock().unwrap().plan_in(&claude, &codex) }).await
                } else if profile_ui {
                    Vec::new()
                } else {
                    bg.spawn(async move { ix.lock().unwrap().plan() }).await
                };
                let total = tasks.len();
                assert!(profile_roots.is_none() || total == 0, "アイドル計測の固定ログはすべて索引済みである必要があります");
                if let Some(planned) = planned {
                    println!("{}", serde_json::json!({"ui_profile":"poll", "tasks":total,
                        "ms":planned.elapsed().as_secs_f64()*1000.0}));
                }
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
                            app.detail_changed(cx);
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
            let generation = this.search_gen.fetch_add(1, Ordering::Relaxed) + 1;
            this.search_task = None;
            this.results = (!this.query.is_empty()).then(search::Results::default);
            this.searching = !this.query.is_empty();
            this.sidebar_changed(cx);
            if this.query.is_empty() {
                this.run_search(cx);
                return;
            }
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_DELAY).await;
                let _ = this.update(cx, |t, cx| {
                    if t.search_gen.load(Ordering::Relaxed) == generation {
                        t.result_cursor = 0;
                        t.sidebar_scroll.set_offset(point(px(0.0), px(0.0)));
                        t.navigator_list.state.scroll_to(gpui::ListOffset::default());
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
        let analysis_state = detail::analysis::State::new(cx);
        let mut this = Self {
            store: Snapshot::default(),
            layout_cache: RefCell::new(if profile_ui && std::env::args().any(|a| a == "--profile-full-grouping") {
                crate::layout::Cache::with_full_history()
            } else if profile_ui && std::env::args().any(|a| a == "--profile-exact-group-scale") {
                crate::layout::Cache::with_exact_scales()
            } else { crate::layout::Cache::default() }),
            calendar_cache: calendar::Cache::default(),
            scope: Vec::new(),
            order: Vec::new(),
            projects: Vec::new(),
            exec_parent: Default::default(),
            loading: Some((0, 0)),
            profile: profile_ui.then(profile::State::new),
            detail_loader: Arc::new(Mutex::new(DetailLoader::default())),
            detail_gen: Arc::new(AtomicU64::new(0)),
            detail_task: None,
            detail_follow: false,
            detail_following: false,
            focus_handle,
            search_input,
            search: Arc::new(Mutex::new(search::Index::default())),
            search_task: None,
            searching: false,
            query: String::new(),
            results: None,
            result_cursor: 0,
            search_gen: Arc::new(AtomicU64::new(0)),
            sidebar_view,
            sidebar_scroll: ScrollHandle::new(),
            navigator_rendered: Cell::new(0),
            navigator_virtual: Cell::new(false),
            navigator_list: sidebar::NavigatorList::new(),
            result_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(100.0)),
            result_list_key: Cell::new((0, 0, 0)),
            detail_view,
            detail_cache: None,
            detail_snapshot: None,
            show_raw: false,
            row_filter: None,
            expanded: HashSet::new(),
            file_open: None,
            detail_tab: p.detail_tab,
            detail_reversed: p.detail_reversed,
            detail_fullscreen: false,
            analysis_state,
            detail_scroll: ScrollHandle::new(),
            detail_scroll_offsets: [point(px(0.0), px(0.0)); 4],
            overview_list: detail::overview::OverviewList::new(),
            history_expanded: HashSet::new(),
            detail_list: DetailList::new(),
            replay: None,
            replay_speed: 1.0,
            replay_task: None,
            replay_autostart: None,
            file_list: FileList::new(),
            follow_anim: None,
            copied: None,
            hits: Vec::new(),
            hit_origin: Rc::new(Cell::new(Point::default())),
            hovered: None,
            hover_at: Point::default(),
            mode: p.mode,
            statistics_open: p.statistics,
            statistics_state: statistics::State::new(),
            calendar: if p.calendar == Mode::Day { Mode::Day } else { Mode::Week },
            t0: now(),
            spp: 60.0,
            row_scroll: 0.0,
            day0: today,
            px_per_hour: 60.0,
            cal_scroll: 8.0 * 60.0,
            gran: p.gran,
            in_view_only: p.in_view_only,
            directory_timeline: p.directory_timeline,
            cost_timeline: p.cost_timeline,
            cost_state: cost::State::new(),
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
            side_t: if p.sidebar { 1.0 } else { 0.0 },
            detail_t: 0.0,
            anim_at: None,
            resize: None,
            collapsed: p.collapsed.iter().cloned().collect(),
            focus: p.focus.clone(),
            folded: p.folded.iter().cloned().collect(),
            session_limit: 50,
            popover: None,
            gran_anchor: Rc::new(Cell::new(Bounds::default())),
            pick: None,
            picker_month: today,
            picker_from: None,
            picker_anchor: Rc::new(Cell::new(Bounds::default())),
            minimap_bounds: Rc::new(Cell::new(Bounds::default())),
            minimap_drag: false,
            lane_map_bounds: Rc::new(Cell::new(Bounds::default())),
            lane_map_drag: false,
            minimap_cache: None,
            data_gen: 0,
            timeline_cache: RefCell::default(),
            counts: String::new(),
            saved: p,
        };
        // 現在時刻が右寄りに見えるように始める
        this.go_today(window);
        if std::env::args().any(|a| a == "--profile-ui") {
            this.start_profile(window, cx);
        }
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
            statistics: self.statistics_open,
            calendar: self.calendar,
            sidebar: self.sidebar_open,
            sidebar_w: self.sidebar_w,
            label_w: self.label_w,
            detail_w: self.detail_w,
            gran: self.gran,
            in_view_only: self.in_view_only,
            directory_timeline: self.directory_timeline,
            cost_timeline: self.cost_timeline,
            source: self.source,
            focus: self.focus.clone(),
            collapsed,
            folded,
            detail_tab: self.detail_tab,
            detail_reversed: self.detail_reversed,
        };
        if p != self.saved {
            prefs::save(&p);
            self.saved = p;
        }
    }

    /// 読み込み結果を Store に反映する
    fn apply(&mut self, delta: Delta, cx: &mut Context<Self>) -> bool {
        let started = self.profile.as_ref().map(|_| std::time::Instant::now());
        // 複数イベントの選択がレーンの末尾まで届いていれば、増えたイベントも選択に含めて追従させる
        let follow = self.selected.filter(|s| {
            self.replay.is_none() && s.first < s.last
                && self.valid(*s)
                && s.last + 1 == self.store.sessions[s.session].lanes[s.lane].events.len()
        });
        // 表示順に応じて最新のイベント側にいれば、増えたイベントまでスクロールして追う
        let at_latest = self.detail_at_latest() || self.detail_following || self.detail_follow;
        let Delta { snapshot, touched } = delta;
        if touched.is_empty() {
            return false;
        }
        let old = std::mem::replace(&mut self.store, snapshot);
        // 旧版の大きい本文の解放も UI スレッドで行わない
        cx.background_executor().spawn(async move { drop(old); }).detach();
        self.layout_cache.get_mut().invalidate(&touched);
        self.rebuild_order();
        if !self.query.is_empty() {
            self.refresh_results(cx);
        }
        // 選択中のセッションが更新されたら、範囲を確かめて詳細を作り直す
        if let Some(mut sel) = self.selected.filter(|s| touched.contains(&s.session)) {
            let following = follow == Some(sel);
            if follow == Some(sel) {
                if let Some(l) = self.store.sessions.get(sel.session).and_then(|s| s.lanes.get(sel.lane)) {
                    sel.last = sel.last.max(l.events.len().saturating_sub(1));
                }
                // 選択が伸びただけなので、絞り込みや開閉・スクロール位置は保つ
                self.selected = Some(sel);
            }
            if self.valid(sel) {
                // 再読み込みが終わるまで、本文と集計を同じ版のまま表示する。
                // リプレイ中は記録時刻と本文の版を固定し、終了後に更新を取り込む。
                if self.replay.is_none() {
                    self.refresh_detail(sel, cx);
                    self.detail_follow = following && at_latest && self.detail_tab == DetailTab::Events;
                }
            } else {
                self.cancel_detail_load();
                self.reset_analysis();
                self.detail_fullscreen = false;
                self.selected = None;
                self.clear_detail_cache(cx);
                self.detail_changed(cx);
            }
        }
        self.sidebar_changed(cx);
        if let Some(started) = started {
            println!("{}", serde_json::json!({"ui_profile":"apply", "ms":started.elapsed().as_secs_f64()*1000.0,
                "touched":touched.len(), "sessions":self.store.sessions.len()}));
        }
        true
    }

    /// 表示順に応じて最新のイベント側へ移る。途中でユーザーがスクロールしたら止める
    fn animate_to_latest(&mut self, cx: &mut Context<Self>) {
        if self.detail_uses_list() {
            self.animate_list_to_latest(cx);
            return;
        }
        const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
        const MAX_FRAMES: usize = 40;
        let h = self.detail_scroll.clone();
        self.detail_following = true;
        self.follow_anim = Some(cx.spawn(async move |this, cx| {
            // 増えたイベントがレイアウトされ、max_offset が更新されるのを待つ
            cx.background_executor().timer(FRAME).await;
            let mut last = h.offset().y;
            for i in 0..MAX_FRAMES {
                let cur = h.offset().y;
                if (cur - last).abs() > px(1.0) {
                    break;
                }
                let target = -h.max_offset().height;
                let gap = target - cur;
                let next = if gap.abs() < px(1.0) || i + 1 == MAX_FRAMES { target } else { cur + gap * 0.2 };
                h.set_offset(gpui::point(h.offset().x, next));
                last = next;
                if this.update(cx, |t, cx| t.detail_changed(cx)).is_err() || next == target {
                    break;
                }
                cx.background_executor().timer(FRAME).await;
            }
            let _ = this.update(cx, |app, _| app.detail_following = false);
        }));
    }

    fn detail_changed(&self, cx: &mut Context<Self>) {
        self.detail_view.update(cx, |_, cx| cx.notify());
    }

    fn sidebar_changed(&self, cx: &mut Context<Self>) {
        self.sidebar_view.update(cx, |_, cx| cx.notify());
    }

    fn rebuild_order(&mut self) {
        let started = self.profile.as_ref().map(|_| std::time::Instant::now());
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
        // 新しいセッションほど上に並べる
        order.sort_by(|&a, &b| self.store.sessions[b].start.total_cmp(&self.store.sessions[a].start));
        self.projects = self.projects_of(&scope);
        self.scope = scope;
        self.order = order;
        self.exec_parent = self.find_exec_parents();
        self.live_sources = self.find_live_sources();
        self.data_gen += 1;
        if let Some(started) = started {
            println!("{}", serde_json::json!({"ui_profile":"rebuild_order", "ms":started.elapsed().as_secs_f64()*1000.0,
                "visible":self.order.len(), "links":self.exec_parent.len()}));
        }
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

    fn find_exec_parents(&self) -> HashMap<usize, (usize, usize, f64)> {
        view_index::exec_parents(&self.store.sessions, &self.order)
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
        // 比較のたびにディレクトリ文字列をハッシュせず、各レーンのキーを一度だけ求める。
        out.sort_by_cached_key(|t| first_seen[self.store.sessions[t.session].cwd.as_str()]);
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
        let all = !self.in_view_only;
        if !in_range && kids.is_empty() && !all {
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
        let mut active = vec![false; s.lanes.len()];
        for &(li, _) in &s.tree {
            if in_range && visible(li) {
                active[li] = true;
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
            if show[li] || all {
                out.push(Track {
                    session: si,
                    lane: li,
                    depth: base + depth,
                    link: if li == 0 { link } else { None },
                    active: active[li],
                });
            }
        }
        out.extend(kids);
        out.len() > at
    }

    /// ディレクトリ見出しを挟んだ行。折りたたんだグループはレーンを出さない
    fn build_rows_in(&self, v0: f64, v1: f64) -> Vec<Row> {
        if self.directory_timeline {
            let groups = crate::layout::directory::groups(
                &mut self.layout_cache.borrow_mut(),
                &self.store.sessions,
                &self.order,
                &self.exec_parent,
                crate::layout::directory::View {
                    range: (v0, v1), spp: self.spp, gran: self.gran,
                    min_width: MIN_BLOCK, in_view_only: self.in_view_only,
                },
            );
            let mut rows = Vec::new();
            for group in groups {
                let active = group.rows.iter().any(|r| !r.is_empty());
                let mut packed = group.rows.into_iter();
                rows.push(Row::Header {
                    cwd: group.cwd, sessions: group.sessions, collapsed: false, active,
                    blocks: packed.next().unwrap_or_default(),
                });
                rows.extend(packed.map(Row::Packed));
            }
            return rows;
        }
        let mut rows: Vec<Row> = Vec::new();
        let mut cur: Option<String> = None;
        let mut header = 0;
        let mut seen = HashSet::new();
        for t in self.tracks_in(v0, v1) {
            let cwd = &self.store.sessions[t.session].cwd;
            if cur.as_ref() != Some(cwd) {
                cur = Some(cwd.clone());
                header = rows.len();
                seen.clear();
                rows.push(Row::Header {
                    cwd: cwd.clone(),
                    sessions: Vec::new(),
                    collapsed: self.collapsed.contains(cwd),
                    active: false,
                    blocks: Vec::new(),
                });
            }
            let Some(Row::Header { sessions, collapsed, active, .. }) = rows.get_mut(header)
            else {
                unreachable!()
            };
            *active |= t.active;
            if seen.insert(t.session) {
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
        if self.statistics_open { return (0.0, 0.0); }
        let (side, detail) = self.pane_full_widths(window);
        (side * ease(self.side_t), detail * ease(self.detail_t))
    }

    /// 開ききったときのサイドバーと詳細パネルの幅（アニメーション中の中身はこの幅で描き、外側で切り取る）
    fn pane_full_widths(&self, window: &Window) -> (f32, f32) {
        let w: f32 = window.viewport_size().width.into();
        let room = w - RAIL_W - GAP - 2.0 - MAIN_MIN_W;
        let mut side = if self.side_t > 0.0 { self.sidebar_w } else { 0.0 };
        let mut detail = if self.detail_t > 0.0 { self.detail_w } else { 0.0 };
        if side + detail > room && detail > 0.0 {
            detail = (room - side).clamp(MIN_DETAIL_W, detail);
        }
        if side + detail > room && side > 0.0 {
            side = (room - detail).clamp(MIN_SIDEBAR_W, side);
        }
        (side, detail)
    }

    /// 開閉のアニメーションを 1 フレーム進める。途中なら次のフレームを要求する
    fn step_panes(&mut self, window: &mut Window) {
        let side_goal = if self.sidebar_open { 1.0 } else { 0.0 };
        let detail_goal = if self.selected.is_some() { 1.0 } else { 0.0 };
        if self.side_t == side_goal && self.detail_t == detail_goal {
            self.anim_at = None;
            return;
        }
        let now = Instant::now();
        // 最初のフレームは 1 フレーム分だけ進める
        let dt = self
            .anim_at
            .map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32().min(0.05));
        self.anim_at = Some(now);
        let step = dt / PANE_ANIM_SECS;
        let approach = |t: f32, goal: f32| {
            if t < goal { (t + step).min(goal) } else { (t - step).max(goal) }
        };
        self.side_t = approach(self.side_t, side_goal);
        self.detail_t = approach(self.detail_t, detail_goal);
        window.request_animation_frame();
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
        let minimap = if self.mode == Mode::Timeline {
            MINIMAP_H
        } else {
            0.0
        };
        (h - STRIP_H - GAP - 2.0 - HEADER_H - minimap - FOOTER_H).max(100.0)
    }

    /// タイムラインのブロックを描く領域の幅
    fn plot_width(&self, window: &Window) -> f32 {
        (self.main_width(window) - self.label_w).max(100.0)
    }

    fn plot_height(&self, window: &Window) -> f32 {
        (self.body_height(window) - AXIS_H).max(100.0)
    }

    /// タイムラインの右端を現在時刻から 1 時間先までに制限する
    fn clamp_timeline(&mut self, window: &Window) {
        if self.mode == Mode::Timeline && !self.statistics_open {
            let span = self.spp * self.plot_width(window) as f64;
            self.t0 = self.t0.min(now() + 3600.0 - span);
        }
    }

    fn set_range(&mut self, start: f64, end: f64, window: &Window) {
        let w = self.plot_width(window) as f64;
        self.spp = ((end - start) / w).max(0.01);
        self.t0 = start;
        self.clamp_timeline(window);
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
        if self.selected == Some(sel) && self.detail_cache.is_some() {
            return;
        }
        self.detail_changed(cx);
        self.sidebar_changed(cx);
        if self.selected != Some(sel) {
            self.reset_analysis();
            self.row_filter = None;
            self.expanded.clear();
            self.history_expanded.clear();
            self.file_open = None;
            self.detail_scroll.set_offset(point(px(0.0), px(0.0)));
            self.detail_scroll_offsets = [point(px(0.0), px(0.0)); 4];
            self.overview_list.reset();
            self.detail_list.reset();
            self.file_list.reset();
        }
        self.selected = Some(sel);
        self.request_detail(sel, cx);
    }

    /// 現在の選択を履歴に積んでから選択する。詳細パネルが開いて表示幅が縮むときは、選択が隠れないようにする
    fn navigate(&mut self, sel: Selection, window: &Window, cx: &mut Context<Self>) {
        let opening = self.selected.is_none();
        if let Some(cur) = self.selected.filter(|c| *c != sel || self.detail_tab != DetailTab::Events) {
            self.push_back(cur);
            self.forward.clear();
        }
        self.select(sel, cx);
        self.set_detail_tab(DetailTab::Events, cx);
        if opening {
            self.reveal(sel, false, window);
        }
        cx.notify();
    }

    fn push_back(&mut self, sel: Selection) {
        self.back.push((sel, self.detail_tab));
        if self.back.len() > 100 {
            self.back.remove(0);
        }
    }

    /// 履歴を遡って直前の選択に戻る（更新で無効になった項目は飛ばす）
    fn go_back(&mut self, window: &Window, cx: &mut Context<Self>) {
        while let Some((sel, tab)) = self.back.pop() {
            if self.valid(sel) {
                if let Some(cur) = self.selected {
                    self.forward.push((cur, self.detail_tab));
                }
                self.select(sel, cx);
                self.set_detail_tab(tab, cx);
                self.reveal(sel, false, window);
                break;
            }
        }
        cx.notify();
    }

    fn go_forward(&mut self, window: &Window, cx: &mut Context<Self>) {
        while let Some((sel, tab)) = self.forward.pop() {
            if self.valid(sel) {
                if let Some(cur) = self.selected {
                    self.push_back(cur);
                }
                self.select(sel, cx);
                self.set_detail_tab(tab, cx);
                self.reveal(sel, false, window);
                break;
            }
        }
        cx.notify();
    }

    /// 詳細パネルを閉じる。閉じた選択は「戻る」で開き直せる
    fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_fullscreen = false;
        self.reset_analysis();
        self.cancel_detail_load();
        if let Some(cur) = self.selected.take() {
            self.push_back(cur);
            self.forward.clear();
        }
        self.clear_detail_cache(cx);
        self.sidebar_changed(cx);
        cx.notify();
    }

    /// 選択の時間範囲 (開始, 終了)
    fn sel_range(&self, sel: Selection) -> (f64, f64) {
        let evs = &self.store.sessions[sel.session].lanes[sel.lane].events[sel.first..=sel.last];
        let a = evs.first().map_or(0.0, |e| e.start);
        let end = self.detail_cache.as_ref().filter(|d| d.sel == sel)
            .map_or_else(|| evs.iter().map(|e| e.end).fold(a, f64::max), |d| d.end());
        (a, end)
    }

    /// 選択を表示中のビューに映す。`zoom` なら収まらないときや縮小しすぎのときに拡大率も合わせる。
    /// タイムラインではレーンの行までスクロールし、カレンダーではその日の該当時刻を出す
    fn reveal(&mut self, sel: Selection, zoom: bool, window: &Window) {
        if self.statistics_open {
            self.statistics_open = false;
            self.persist();
        }
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
                self.clamp_timeline(window);
                self.scroll_to_selection(sel, window);
            }
            Mode::Day | Mode::Week => {
                let day = local(a).date_naive();
                let first = if self.mode == Mode::Day { self.day0 } else { self.week_start() };
                let days = if self.mode == Mode::Day { 1 } else { 7 };
                if day < first || day >= first + Duration::days(days) {
                    self.day0 = day;
                }
                let ph = self.plot_height(window);
                let y = ((a - midnight(day)) / 3600.0) as f32 * self.px_per_hour;
                if y < self.cal_scroll || y > self.cal_scroll + ph - 20.0 {
                    self.cal_scroll = calendar::clamp_scroll(y - ph / 3.0, self.px_per_hour, ph);
                }
            }
        }
    }

    /// タイムラインで選択した枠の行へスクロールする。セッション表示では折りたたみも開く。
    fn scroll_to_selection(&mut self, sel: Selection, window: &Window) {
        if self.cost_timeline {
            self.scroll_to_cost_selection(sel, window);
            return;
        }
        let v0 = self.t0;
        let v1 = self.t0 + self.spp * self.plot_width(window) as f64;
        let cwd = self.store.sessions[sel.session].cwd.clone();
        if !self.directory_timeline && self.collapsed.remove(&cwd) {
            self.persist();
        }
        let layout = self.rows_in(v0, v1);
        let rows = &layout.rows;
        let i = rows.iter().position(|r| {
            if self.directory_timeline {
                sel.lane == 0 && r.packed_blocks().iter().any(|b| {
                    b.session == sel.session && b.block.first <= sel.last && sel.first <= b.block.last
                })
            } else {
                matches!(r, Row::Lane(t) if t.session == sel.session && t.lane == sel.lane)
            }
        }).or_else(|| {
            if self.directory_timeline {
                rows.iter().position(|r| matches!(r, Row::Header { cwd: c, .. } if *c == cwd))
            } else {
                None
            }
        });
        let Some(i) = i else { return };
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
        self.set_detail_tab(DetailTab::Session, cx);
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

    fn refresh_results(&mut self, cx: &mut Context<Self>) {
        let generation = self.search_gen.fetch_add(1, Ordering::Relaxed) + 1;
        self.search_task = None;
        self.results = (!self.query.is_empty()).then(search::Results::default);
        self.searching = !self.query.is_empty();
        if self.query.is_empty() { return; }
        let snapshot = self.store.clone();
        let scope = self.order.clone();
        let query = self.query.clone();
        let index = self.search.clone();
        let counter = self.search_gen.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let results = cx.background_executor().spawn(async move {
                let current = || counter.load(Ordering::Relaxed) == generation;
                if !current() { return None; }
                let mut index = index.lock().unwrap();
                index.search_while(&snapshot.sessions, &scope, &query, SEARCH_LIMIT, current)
            }).await;
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

    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.refresh_results(cx);
        if self.results.is_some() {
            self.sidebar_open = true;
        }
        self.sidebar_changed(cx);
        cx.notify();
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_some() && !self.statistics_open && (self.detail_fullscreen || self.detail_tab == DetailTab::Analysis) {
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
        self.result_list.scroll_to_reveal_item(self.result_row(self.result_cursor));
        self.sidebar_changed(cx);
    }

    // ------------------------------------------------------------ 表示の切り替え

    /// 表示を切り替える。タイムラインとカレンダーの間では、見ていた時刻が画面に残るようにする
    fn set_mode(&mut self, mode: Mode, window: &Window) {
        self.detail_fullscreen = false;
        self.statistics_open = false;
        let from = self.mode;
        if from == Mode::Timeline && mode != Mode::Timeline {
            let span = self.spp * self.plot_width(window) as f64;
            let tc = (self.t0 + span / 2.0).min(now());
            self.day0 = local(tc).date_naive();
            self.mode = mode;
            self.scroll_calendar_to(tc, 0.5, window);
        } else if from != Mode::Timeline && mode == Mode::Timeline {
            let ph = self.plot_height(window);
            let tc = midnight(self.day0) + ((self.cal_scroll + ph / 2.0) / self.px_per_hour) as f64 * 3600.0;
            self.mode = mode;
            let span = self.spp * self.plot_width(window) as f64;
            // 現在時刻より先は 15% までしか見せない
            self.t0 = (tc - span / 2.0).min(now() - span * 0.85);
        }
        self.mode = mode;
        self.clamp_timeline(window);
        if mode != Mode::Timeline {
            self.calendar = mode;
        }
        self.popover = None;
        self.persist();
    }

    /// カレンダーを、時刻 `t` が画面の上から `at`（0〜1）の位置に来るようにスクロールする
    fn scroll_calendar_to(&mut self, t: f64, at: f32, window: &Window) {
        let ph = self.plot_height(window);
        let y = ((t - midnight(local(t).date_naive())) / 3600.0) as f32 * self.px_per_hour;
        self.cal_scroll = calendar::clamp_scroll(y - ph * at, self.px_per_hour, ph);
    }

    fn set_source(&mut self, s: SourceFilter, cx: &mut Context<Self>) {
        self.source = s;
        self.rebuild_order();
        if !self.query.is_empty() {
            self.refresh_results(cx);
        }
        self.row_scroll = 0.0;
        self.persist();
        self.sidebar_changed(cx);
    }

    fn set_focus(&mut self, f: Option<String>, cx: &mut Context<Self>) {
        self.focus = f;
        self.rebuild_order();
        if !self.query.is_empty() {
            self.refresh_results(cx);
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

    /// タイムラインで表示範囲内のセッションだけを並べるか、範囲外もグレーアウトして並べるかを切り替える
    fn toggle_in_view_only(&mut self, cx: &mut Context<Self>) {
        self.in_view_only = !self.in_view_only;
        self.row_scroll = 0.0;
        self.persist();
        cx.notify();
    }

    fn toggle_directory_timeline(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.directory_timeline = !self.directory_timeline;
        self.row_scroll = 0.0;
        self.hovered = None;
        if let Some(sel) = self.selected {
            self.scroll_to_selection(sel, window);
        }
        self.persist();
        cx.notify();
    }

    fn step(&mut self, dir: f64, window: &Window) {
        if self.statistics_open { return; }
        match self.mode {
            Mode::Day => self.day0 += Duration::days(dir as i64),
            Mode::Week => self.day0 += Duration::days(7 * dir as i64),
            Mode::Timeline => self.t0 += dir * self.spp * self.plot_width(window) as f64,
        }
        self.clamp_timeline(window);
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
        self.clamp_timeline(window);
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
        if self.statistics_open { return; }
        match self.mode {
            Mode::Timeline => {
                let w = self.plot_width(window) as f64;
                let center = self.t0 + w * self.spp / 2.0;
                self.spp = (self.spp * f).clamp(0.005, 86400.0 * 3.0);
                self.t0 = center - w * self.spp / 2.0;
            }
            _ => {
                let h = self.plot_height(window);
                let center = (self.cal_scroll + h / 2.0) / self.px_per_hour;
                self.px_per_hour = (self.px_per_hour / f as f32).clamp(20.0, 20000.0);
                self.cal_scroll = calendar::clamp_scroll(center * self.px_per_hour - h / 2.0, self.px_per_hour, h);
            }
        }
        self.clamp_timeline(window);
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
                    self.cal_scroll = calendar::clamp_scroll(
                        self.cal_scroll - dy, self.px_per_hour, self.plot_height(window),
                    );
                }
            }
        }
        self.clamp_timeline(window);
        self.hovered = None;
        cx.notify();
    }

    /// トラックパッドのピンチ。`m` は拡大率の差分（正で拡大）
    fn on_pinch(&mut self, m: f64, window: &mut Window, cx: &mut Context<Self>) {
        if self.statistics_open { return; }
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
                self.cal_scroll = calendar::clamp_scroll(
                    anchor_h * self.px_per_hour - my, self.px_per_hour, self.plot_height(window),
                );
            }
        }
        self.clamp_timeline(window);
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
        let ph = self.plot_height(window);
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
                self.cal_scroll = calendar::clamp_scroll(d.scroll - dy, self.px_per_hour, ph);
            }
        }
        let t0 = self.t0;
        self.clamp_timeline(window);
        if self.t0 < t0 {
            // 上限を越えたドラッグ量を残さず、逆方向に動かすとすぐ過去へ戻れるようにする
            if let Some(d) = self.drag.as_mut() {
                d.t0 = self.t0;
                d.origin.x = e.position.x;
            }
        }
        cx.notify();
    }

    fn on_up(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        if d.moved {
            return;
        }
        let Some(sel) = self.hit(e.position) else {
            return;
        };
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
        } else if self.detail_fullscreen {
            self.toggle_detail_fullscreen(window, cx);
        } else if self.statistics_open {
            self.set_mode(self.mode, window);
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

impl Drop for Rireki {
    fn drop(&mut self) {
        self.detail_gen.fetch_add(1, Ordering::Relaxed);
        self.search_gen.fetch_add(1, Ordering::Relaxed);
    }
}

impl Render for Rireki {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.profile_frame(window, cx);
        self.step_panes(window);
        self.clamp_timeline(window);
        let fullscreen = self.detail_fullscreen && self.selected.is_some_and(|s| self.valid(s)) && !self.statistics_open;
        let main_col = if fullscreen {
            div().into_any_element()
        } else if self.statistics_open {
            self.statistics(window, cx).into_any_element()
        } else {
            let main = match self.mode {
                Mode::Timeline if self.cost_timeline => self.cost_timeline_view(window, cx),
                Mode::Timeline => self.timeline(window, cx),
                _ => self.calendar(window, cx),
            };
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
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .child(self.header(window, cx))
                .when(self.mode == Mode::Timeline, |d| d.child(self.minimap(window, cx)))
                .child(body)
                .child(self.footer(window))
                .into_any_element()
        };
        let (side_w, detail_w) = self.pane_widths(window);
        let (side_full, detail_full) = self.pane_full_widths(window);
        let side_shown = !fullscreen && !self.statistics_open && self.side_t > 0.0;
        let detail_shown = !fullscreen && !self.statistics_open && self.detail_t > 0.0;
        // 開ききっているときだけ幅を変えるつまみを出す
        let side_ready = side_shown && self.sidebar_open && self.side_t == 1.0;
        let detail_ready = detail_shown && self.selected.is_some() && self.detail_t == 1.0;

        let mut panel = div()
            .flex_1()
            .min_w_0()
            .mr(px(GAP))
            .when(fullscreen, |d| d.ml(px(GAP)))
            .mb(px(GAP))
            .relative()
            .flex()
            .rounded(px(PANEL_RADIUS))
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::BG))
            .overflow_hidden();
        // 開閉中は中身を開ききった幅で描き、外側の箱の幅で切り取る（中身を押しつぶさない）
        if side_shown {
            panel = panel.child(
                div().w(px(side_w)).h_full().flex_none().overflow_hidden().flex().justify_end().child(
                    AnyView::from(self.sidebar_view.clone())
                        .cached(StyleRefinement::default().w(px(side_full)).h_full().flex_none()),
                ),
            );
        }
        if fullscreen {
            panel = panel.child(AnyView::from(self.detail_view.clone())
                .cached(StyleRefinement::default().w(px(self.detail_content_width(window))).h_full().flex_none()));
        } else {
            panel = panel.child(main_col);
        }
        if detail_shown {
            panel = panel.child(
                div().w(px(detail_w)).h_full().flex_none().overflow_hidden().child(
                    AnyView::from(self.detail_view.clone())
                        .cached(StyleRefinement::default().w(px(detail_full)).h_full().flex_none()),
                ),
            );
        }
        // つまみはドラッグ開始時の実際の幅から動かす
        if side_ready {
            panel = panel.child(
                self.resize_handle("resize-sidebar", cx, move |_, x| Resize::Sidebar { x, w: side_w })
                    .left(px(side_w - HANDLE_W / 2.0)),
            );
        }
        if detail_ready {
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
            .on_action(cx.listener(|t, _: &ToggleDetailFullscreen, window, cx| t.toggle_detail_fullscreen(window, cx)))
            .on_action(cx.listener(|t, _: &CancelAnalysisSearch, window, cx| t.cancel_analysis_search(window, cx)))
            .on_action(cx.listener(|t, _: &ToggleInViewOnly, _, cx| t.toggle_in_view_only(cx)))
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
            .on_action(cx.listener(|t, _: &ShowStatistics, window, cx| t.show_statistics(window, cx)))
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
            .on_action(cx.listener(|t, _: &NextEvent, window, cx| t.step_event(1, window, cx)))
            .on_action(cx.listener(|t, _: &PrevEvent, window, cx| t.step_event(-1, window, cx)))
            .on_action(cx.listener(|t, _: &DetailEvents, _, cx| t.set_detail_tab(DetailTab::Events, cx)))
            .on_action(cx.listener(|t, _: &DetailFiles, _, cx| t.set_detail_tab(DetailTab::Files, cx)))
            .on_action(cx.listener(|t, _: &DetailSession, _, cx| t.set_detail_tab(DetailTab::Session, cx)))
            .on_action(cx.listener(|t, _: &DetailAnalysis, _, cx| t.set_detail_tab(DetailTab::Analysis, cx)))
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
            .child(div().flex().flex_1().min_h_0().when(!fullscreen, |d| d.child(self.rail(cx))).child(panel))
            .children(self.overlays(window, cx))
    }
}

/// 開閉のアニメーションの緩急（ease-out cubic）
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
