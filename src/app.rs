//! メインウィンドウ（GPUI のビュー）。Slack のように、左端のアイコンレール・サイドバー・メイン・詳細パネルを並べる
//!
//! - 上端の帯: 状態表示。macOS では透明なタイトルバーの信号機ボタンと重なる
//! - レール (`rail.rs`): 読み込み元（すべて / Claude / Codex）と表示（タイムライン / カレンダー / 統計）の切り替え
//! - サイドバー (`sidebar.rs`): 検索欄と、進行中・プロジェクト・セッションの一覧、または検索結果
//! - メイン: タイムライン (`timeline.rs`) / カレンダー (`calendar.rs`) と見出し・凡例、または統計 (`statistics.rs`)
//! - 詳細パネル (`detail.rs`): 選択したブロックの内容。選択している間だけ開く
//! - 重ねて出すもの (`overlay.rs`): ホバーカード、表示オプションのメニュー、ショートカット一覧

mod init;
mod data;
mod panes;
mod selection;
mod search_controller;
mod navigation;
mod interaction;
mod format;
use format::{now, fmt_dur, rel_time, when, fmt_n, tilde, base_name, is_live};
mod calendar;
mod cost;
mod detail;
mod header;
mod keymap;
mod overlay;
mod profile;
mod rail;
mod window_frame;
mod range;
mod rich;
mod sidebar;
mod statistics;
mod timeline;
mod track_index;
mod lane_flags;
mod mcp;
mod widgets;

use chrono::{Duration, Local, NaiveDate};
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
    GoToday, NextEvent, PickPeriod, PrevEvent, SearchCancel, SearchConfirm, SearchDown, SearchUp, ShowDay, ShowHelp,
    ShowTimeline, ShowWeek, ShowMonth, ShowStatistics, ToggleInViewOnly, ToggleSidebar, ZoomIn, ZoomOut,
};
use sidebar::SidebarView;
use track_index::TrackIndex;

/// 上端の状態表示（macOS ではタイトルバー領域）の高さ
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
    /// 表示オプション（表示する値・行とブロックのまとめ方・範囲内のみ）
    Display,
    /// キーボードショートカット一覧
    Help,
    /// 表示期間（カレンダーでは移動先の日付）を選ぶカレンダー
    Range,
    /// localhost の MCP サーバーを起動・停止する
    Mcp,
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
    mcp_server: Option<crate::mcp::Server>,
    mcp_busy: bool,
    mcp_error: Option<String>,
    mcp_port: Entity<TextInput>,
    layout_cache: RefCell<crate::layout::Cache>,
    calendar_cache: calendar::Cache,
    /// 読み込み元で絞り込んだセッション（開始順）
    scope: Vec<usize>,
    /// scope をさらにプロジェクトで絞り込んだ、表示するセッション（開始順）
    order: Vec<usize>,
    /// 表示中の全期間。表示順の更新で無効にし、最初の問い合わせで求める。
    order_extent: Cell<Option<(f64, f64)>>,
    /// scope のプロジェクト（最近作業した順）
    projects: Vec<Project>,
    /// `codex exec` などで起動された子セッション → 起動元 (セッション, レーン, 時刻)
    exec_parent: HashMap<usize, (usize, usize, f64)>,
    /// 表示順と起動元の更新時だけ作る、トラック抽出用の親子関係。
    track_index: TrackIndex,
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
    /// 表示オプションのボタンの位置（メニューを出す基準）
    display_anchor: Rc<Cell<Bounds<Pixels>>>,
    /// カレンダーで選んだ表示期間。ミニマップはこの期間全体を表す
    pick: Option<(f64, f64)>,
    /// 期間を選ぶカレンダーで表示している月（1 日）
    picker_month: NaiveDate,
    /// 期間を選ぶカレンダーで、先に選んだ開始日
    picker_from: Option<NaiveDate>,
    /// ツールバーの期間の表記の位置（期間を選ぶカレンダーを出す基準）
    picker_anchor: Rc<Cell<Bounds<Pixels>>>,
    /// ミニマップの位置（ウィンドウ座標）
    minimap_bounds: Rc<Cell<Bounds<Pixels>>>,
    minimap_drag: bool,
    /// タイムライン右端の、表示範囲内のレーンの位置を示す縦のミニマップ
    lane_map_bounds: Rc<Cell<Bounds<Pixels>>>,
    lane_map_drag: bool,
    /// ミニマップの棒の高さ。キーは (期間の始め, 終わり, 本数, data_gen)
    minimap_cache: Option<(MinimapKey, Rc<Vec<u32>>, f32)>,
    /// 表示するセッションが変わるたびに増やす
    data_gen: u64,
    /// 表示中のタイムラインの行。縦スクロールや選択では作り直さない。
    timeline_cache: RefCell<timeline::Cache>,
    /// 下端に出す、表示中の件数
    counts: String,
    /// 最後に保存した画面の状態
    saved: Prefs,
}

impl Rireki {

    fn detail_changed(&self, cx: &mut Context<Self>) {
        self.detail_view.update(cx, |_, cx| cx.notify());
    }

    fn sidebar_changed(&self, cx: &mut Context<Self>) {
        self.sidebar_view.update(cx, |_, cx| cx.notify());
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
        self.clear_statistics_paths_if_unused();
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
            .on_action(cx.listener(|t, _: &ShowMonth, window, cx| {
                t.set_mode(Mode::Month, window);
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
            .on_action(cx.listener(|t, _: &PickPeriod, _, cx| {
                if !t.statistics_open && !t.detail_fullscreen {
                    t.toggle_picker();
                    cx.notify();
                }
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
            .child(self.top_strip(window))
            .child(div().flex().flex_1().min_h_0().when(!fullscreen, |d| d.child(self.rail(cx))).child(panel))
            .children(self.overlays(window, cx))
            .when(cfg!(target_os = "linux") && matches!(window.window_decorations(), gpui::Decorations::Client { .. }),
                |d| d.child(window_frame::resize_border(window)))
    }
}

/// 開閉のアニメーションの緩急（ease-out cubic）
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
