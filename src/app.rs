use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Timelike};
use gpui::{
    AnyElement, AnyView, App, Bounds, ClickEvent, ContentMask, Context, Div, ElementId, Entity, FontWeight,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString, Stateful,
    StyleRefinement, TextRun, WeakEntity, Window, canvas, div, fill, point, prelude::*, px, size,
};

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::db::{Db, JsCache};
use crate::loader;
use crate::icons::{self, Icon};
use crate::index::{self, Indexer, Update};
use crate::layout::{self, Block, Granularity};
use crate::model::{Kind, Store, one_line, truncate};
use crate::theme::{self, c};

const TOOLBAR_H: f32 = 84.0;
const STATUS_H: f32 = 24.0;
const AXIS_H: f32 = 30.0;
const LABEL_W: f32 = 280.0;
const DETAIL_W: f32 = 470.0;
/// 境界ドラッグで変えられる幅の下限
const MIN_PANE_W: f32 = 120.0;
/// 境界のつまみの幅
const HANDLE_W: f32 = 6.0;
const ROW_H: f32 = 24.0;
const HOUR_LABEL_W: f32 = 52.0;
const MIN_BLOCK: f32 = 3.0;
/// 詳細パネルに並べるイベント数の上限
const DETAIL_MAX: usize = 300;
/// リアルタイムファイルの変更を確認する間隔
const POLL: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Day,
    Week,
    Timeline,
}

#[derive(Clone, Copy, PartialEq)]
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
    label: Option<SharedString>,
}

/// 詳細パネル。選択が変わったときだけ描き直すよう、別ビューとしてキャッシュする
pub struct DetailView {
    app: WeakEntity<Rireki>,
}

impl Render for DetailView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.detail(cx)),
            None => div().into_any_element(),
        }
    }
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
    Label { x: f32, w: f32 },
    Detail { x: f32, w: f32 },
}

pub struct Rireki {
    store: Store,
    /// 表示順に並べたセッション
    order: Vec<usize>,
    /// `codex exec` などで起動された子セッション → 起動元 (セッション, レーン, 時刻)
    exec_parent: std::collections::HashMap<usize, (usize, usize, f64)>,
    loading: Option<(usize, usize)>,
    /// 詳細表示で input / output を引くための読み取り専用接続
    reader: Option<Db>,
    /// exec の JS をモック実行した結果のキャッシュ
    js_cache: Option<JsCache>,
    detail_cache: Option<DetailCache>,
    detail_view: Entity<DetailView>,
    /// 直前のフレームで描いたブロック（クリック・ホバーの当たり判定用）
    hits: Vec<((f32, f32, f32, f32), Selection)>,
    /// ブロックを描いた canvas の左上（ウィンドウ座標）
    hit_origin: Rc<Cell<Point<Pixels>>>,
    hovered: Option<Selection>,
    mode: Mode,
    // タイムライン
    t0: f64,
    spp: f64,
    row_scroll: f32,
    // カレンダー
    day0: NaiveDate,
    px_per_hour: f32,
    cal_scroll: f32,
    gran: Granularity,
    show_claude: bool,
    show_codex: bool,
    selected: Option<Selection>,
    /// 詳細パネルで「戻る」ための選択履歴
    history: Vec<Selection>,
    hover: Option<String>,
    drag: Option<Drag>,
    /// 左のセッション一覧と右の詳細パネルの幅
    label_w: f32,
    detail_w: f32,
    resize: Option<Resize>,
    /// 折りたたんだディレクトリ
    collapsed: std::collections::HashSet<String>,
    /// 注目中のディレクトリ。設定中はこのディレクトリのセッションだけを表示する
    focus: Option<String>,
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
                if first {
                    first = false;
                    if this
                        .update(cx, |app, cx| {
                            app.loading = None;
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                bg.timer(POLL).await;
            }
        })
        .detach();

        let today = Local::now().date_naive();
        let weak = cx.weak_entity();
        let detail_view = cx.new(|_| DetailView { app: weak });
        Self {
            store: Store::default(),
            order: Vec::new(),
            exec_parent: Default::default(),
            loading: Some((0, 0)),
            reader: None,
            js_cache: JsCache::open().ok(),
            detail_cache: None,
            detail_view,
            hits: Vec::new(),
            hit_origin: Rc::new(Cell::new(Point::default())),
            hovered: None,
            mode: Mode::Timeline,
            t0: now() - 86400.0 * 0.75,
            spp: 60.0,
            row_scroll: 0.0,
            day0: today,
            px_per_hour: 60.0,
            cal_scroll: 8.0 * 60.0,
            gran: Granularity::Auto,
            show_claude: true,
            show_codex: true,
            selected: None,
            history: Vec::new(),
            hover: None,
            drag: None,
            label_w: LABEL_W,
            detail_w: DETAIL_W,
            resize: None,
            collapsed: Default::default(),
            focus: None,
        }
    }

    /// 読み込み結果を Store に反映する
    fn apply(&mut self, updates: Vec<Update>, cx: &mut Context<Self>) -> bool {
        let touched = self.store.replace(updates.into_iter().map(|u| (u.origin, u.items)));
        if touched.is_empty() {
            return false;
        }
        self.rebuild_order();
        // 選択中のセッションが更新されたら、範囲を確かめて詳細を作り直す
        if let Some(sel) = self.selected.filter(|s| touched.contains(&s.session)) {
            let lanes = &self.store.sessions[sel.session].lanes;
            let ok = lanes.get(sel.lane).is_some_and(|l| sel.last < l.events.len());
            if ok {
                self.select(sel, cx);
            } else {
                self.selected = None;
                self.detail_cache = None;
                self.detail_changed(cx);
            }
        }
        true
    }

    fn detail_changed(&self, cx: &mut Context<Self>) {
        self.detail_view.update(cx, |_, cx| cx.notify());
    }

    fn rebuild_order(&mut self) {
        let mut order: Vec<usize> = (0..self.store.sessions.len())
            .filter(|&i| self.store.visible(i))
            .filter(|&i| match self.store.sessions[i].source {
                crate::model::Source::Claude => self.show_claude,
                crate::model::Source::Codex => self.show_codex,
            })
            .filter(|&i| self.focus.as_ref().is_none_or(|f| *f == self.store.sessions[i].cwd))
            .collect();
        order.sort_by(|&a, &b| self.store.sessions[a].start.total_cmp(&self.store.sessions[b].start));
        self.order = order;
        self.exec_parent = self.find_exec_parents();
    }

    /// 非対話で起動されたセッションを、同じディレクトリの他セッションで
    /// その時刻に実行中だったツール呼び出しの子とみなす
    fn find_exec_parents(&self) -> std::collections::HashMap<usize, (usize, usize, f64)> {
        // 起動コマンドはすぐ制御を返すことがあるので、呼び出し終了後もしばらくは候補にする
        const SLACK: f64 = 30.0;
        let is_launch = |e: &crate::model::Event| e.kind == crate::model::Kind::Tool;
        let mut by_cwd: std::collections::HashMap<&str, Vec<usize>> = Default::default();
        for &si in &self.order {
            by_cwd.entry(self.store.sessions[si].cwd.as_str()).or_default().push(si);
        }
        let mut out = std::collections::HashMap::new();
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
        let mut children: std::collections::HashMap<usize, Vec<usize>> = Default::default();
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
        let mut first_seen: std::collections::HashMap<&str, usize> = Default::default();
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
        children: &std::collections::HashMap<usize, Vec<usize>>,
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

    fn plot_width(&self, window: &Window) -> f32 {
        let w: f32 = window.viewport_size().width.into();
        (w - self.label_w - self.detail_w).max(100.0)
    }

    fn plot_height(&self, window: &Window) -> f32 {
        let h: f32 = window.viewport_size().height.into();
        (h - TOOLBAR_H - STATUS_H - AXIS_H).max(100.0)
    }

    fn set_range(&mut self, start: f64, end: f64, window: &Window) {
        let w = self.plot_width(window) as f64;
        self.spp = ((end - start) / w).max(0.01);
        self.t0 = start;
        self.row_scroll = 0.0;
    }

    fn select(&mut self, sel: Selection, cx: &mut Context<Self>) {
        self.detail_changed(cx);
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
        let texts = evs
            .iter()
            .take(DETAIL_MAX)
            .map(|e| {
                let (input, output) = match e.row.and_then(|r| fetched.remove(&r)) {
                    Some(v) => v,
                    None => (e.input.clone(), e.output.clone()),
                };
                let input = match input.strip_prefix(loader::JS_MARK) {
                    Some(js) => exec_detail_cached(self.js_cache.as_ref(), js),
                    None => input,
                };
                (truncate(&input, limit), truncate(&output, limit))
            })
            .collect();
        self.detail_cache = Some(DetailCache { sel, texts });
    }

    /// 現在の選択を履歴に積んでから選択する
    fn navigate(&mut self, sel: Selection, cx: &mut Context<Self>) {
        if let Some(cur) = self.selected.filter(|c| *c != sel) {
            self.history.push(cur);
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.select(sel, cx);
    }

    /// 履歴を遡って直前の選択に戻る（更新で無効になった項目は飛ばす）
    fn back(&mut self, cx: &mut Context<Self>) {
        while let Some(sel) = self.history.pop() {
            let ok = self
                .store
                .sessions
                .get(sel.session)
                .and_then(|s| s.lanes.get(sel.lane))
                .is_some_and(|l| sel.last < l.events.len());
            if ok {
                self.select(sel, cx);
                return;
            }
        }
        self.detail_changed(cx);
    }

    /// 選択したレーンが見えるようにタイムラインを移動
    fn reveal(&mut self, session: usize, lane: usize, window: &Window) {
        let Some((a, b)) = self.store.sessions[session].lanes[lane].range() else {
            return;
        };
        let pad = (b - a).max(60.0) * 0.1;
        if self.mode != Mode::Timeline {
            self.mode = Mode::Timeline;
        }
        self.set_range(a - pad, b + pad, window);
    }

    fn step(&mut self, dir: f64, window: &Window) {
        match self.mode {
            Mode::Day => self.day0 += Duration::days(dir as i64),
            Mode::Week => self.day0 += Duration::days(7 * dir as i64),
            Mode::Timeline => self.t0 += dir * self.spp * self.plot_width(window) as f64,
        }
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
        let mx: f32 = pos.x.into();
        let my: f32 = pos.y.into();
        match self.mode {
            Mode::Timeline => {
                let x = (mx - self.label_w).clamp(0.0, self.plot_width(window)) as f64;
                let anchor = self.t0 + x * self.spp;
                self.spp = (self.spp * f).clamp(0.005, 86400.0 * 3.0);
                self.t0 = anchor - x * self.spp;
            }
            Mode::Day | Mode::Week => {
                let y = my - TOOLBAR_H - AXIS_H;
                let anchor_h = (y + self.cal_scroll) / self.px_per_hour;
                self.px_per_hour = (self.px_per_hour * f as f32).clamp(20.0, 20000.0);
                self.cal_scroll = anchor_h * self.px_per_hour - y;
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
                self.hover = h.map(|sel| {
                    let lane = &self.store.sessions[sel.session].lanes[sel.lane];
                    let b = Block { start: 0.0, end: 0.0, first: sel.first, last: sel.last };
                    format!("{} ({})", one_line(&layout::label(&lane.events, &b), 120), sel.last - sel.first + 1)
                });
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

    fn on_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        if !d.moved
            && let Some(sel) = self.hit(e.position)
        {
            self.navigate(sel, cx);
            cx.notify();
        }
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
            return;
        }
        let win: f32 = window.viewport_size().width.into();
        let x: f32 = e.position.x.into();
        // 中央のプロットが最低 100px 残るようにする
        match r {
            Resize::Label { x: x0, w } => {
                let max = (win - self.detail_w - 100.0).max(MIN_PANE_W);
                self.label_w = (w + x - x0).clamp(MIN_PANE_W, max);
            }
            Resize::Detail { x: x0, w } => {
                let max = (win - self.label_w - 100.0).max(MIN_PANE_W);
                self.detail_w = (w - (x - x0)).clamp(MIN_PANE_W, max);
                self.detail_view.update(cx, |_, cx| cx.notify());
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// 境界のつまみ。ドラッグで幅を変える
    fn resize_handle(&self, id: &'static str, cx: &mut Context<Self>, f: fn(&Self, f32) -> Resize) -> Stateful<Div> {
        div()
            .id(id)
            .w(px(HANDLE_W))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .hover(|s| s.bg(c(theme::BORDER)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    let r = f(t, e.position.x.into());
                    t.start_resize(r, cx);
                }),
            )
    }

    // ------------------------------------------------------------ 描画部品

    fn button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        self.icon_button(id, None, label, active, cx, f)
    }

    /// アイコン付きボタン。`label` が空ならアイコンだけを表示する
    fn icon_button(
        &self,
        id: &'static str,
        icon: Option<Icon>,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let label: SharedString = label.into();
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(c(if active { theme::ACCENT } else { theme::BORDER }))
            .bg(c(if active { 0x24324a } else { theme::PANEL }))
            .text_color(c(theme::TEXT))
            .text_sm()
            .cursor_pointer()
            .hover(|s| s.bg(c(0x2a2f3a)))
            .when_some(icon, |d, i| d.child(icons::icon(i, if active { theme::ACCENT } else { theme::TEXT })))
            .when(!label.is_empty(), |d| d.child(label))
            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                f(this, w, cx);
                cx.notify();
            }))
    }

    fn block_paint(&self, sel: Selection, rect: (f32, f32, f32, f32), vertical: bool) -> BlockPaint {
        let (_, _, w, h) = rect;
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let b = Block { start: 0.0, end: 0.0, first: sel.first, last: sel.last };
        let color = theme::event_color(&lane.events[layout::dominant(&lane.events, &b)]);
        let selected = self.selected.is_some_and(|s| {
            s.session == sel.session && s.lane == sel.lane && s.first <= sel.last && sel.first <= s.last
        });
        let show_text = if vertical { h >= 14.0 && w >= 24.0 } else { w >= 28.0 };
        // 幅に収まる程度の文字数だけ整形する
        let label = show_text.then(|| one_line(&layout::label(&lane.events, &b), (w / 5.0) as usize + 1).into());
        BlockPaint {
            rect,
            sel,
            color,
            multi: sel.last > sel.first,
            selected,
            hovered: self.hovered == Some(sel),
            label,
        }
    }

    /// ブロックを 1 つの canvas にまとめて描き、当たり判定用に位置を覚えておく
    fn blocks_canvas(&mut self, blocks: Vec<BlockPaint>) -> impl IntoElement {
        self.hits = blocks.iter().map(|b| (b.rect, b.sel)).collect();
        let origin = self.hit_origin.clone();
        canvas(
            move |bounds, _, _| origin.set(bounds.origin),
            move |bounds, _, window: &mut Window, cx: &mut App| {
                let font = window.text_style().font();
                let font_size = window.rem_size() * 0.75;
                let line_h = px(16.0);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for b in &blocks {
                        let (x, y, w, h) = b.rect;
                        let r = Bounds::new(bounds.origin + point(px(x), px(y)), size(px(w), px(h)));
                        let col = c(b.color);
                        let alpha = if b.hovered {
                            1.0
                        } else if b.multi {
                            0.75
                        } else {
                            0.9
                        };
                        let mut q = fill(r, col.opacity(alpha)).corner_radii(px(2.0));
                        if b.selected {
                            q = q.border_widths(px(1.0)).border_color(gpui::white());
                        } else if b.multi {
                            q = q.border_widths(gpui::Edges { left: px(2.0), ..Default::default() }).border_color(col);
                        }
                        window.paint_quad(q);
                        let Some(label) = &b.label else { continue };
                        let run = TextRun {
                            len: label.len(),
                            font: font.clone(),
                            color: gpui::black(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line = window.text_system().shape_line(label.clone(), font_size, &[run], None);
                        let top = if h > line_h.into() { (h - f32::from(line_h)) / 2.0 } else { 0.0 };
                        window.with_content_mask(Some(ContentMask { bounds: r }), |window| {
                            let _ = line.paint(r.origin + point(px(4.0), px(top)), line_h, window, cx);
                        });
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    // ------------------------------------------------------------ ツールバー

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let mode = self.mode;
        let range_label = match mode {
            Mode::Day => self.day0.format("%Y-%m-%d (%a)").to_string(),
            Mode::Week => {
                let mon = self.week_start();
                format!("{} – {}", mon.format("%Y-%m-%d"), (mon + Duration::days(6)).format("%m-%d"))
            }
            Mode::Timeline => {
                let end = self.t0 + self.spp * self.plot_width(window) as f64;
                format!(
                    "{} – {}  ({} / 100px)",
                    local(self.t0).format("%Y-%m-%d %H:%M"),
                    local(end).format("%m-%d %H:%M"),
                    fmt_dur(self.spp * 100.0)
                )
            }
        };

        let presets: [(&'static str, &'static str, f64); 6] = [
            ("p1h", "1時間", 3600.0),
            ("p6h", "6時間", 6.0 * 3600.0),
            ("p1d", "1日", 86400.0),
            ("p1w", "1週", 7.0 * 86400.0),
            ("p1m", "1ヶ月", 30.0 * 86400.0),
            ("pall", "全期間", 0.0),
        ];

        let mut row1 = div().flex().items_center().gap_1p5();
        row1 = row1
            .child(div().text_color(c(theme::ACCENT)).font_weight(FontWeight::BOLD).mr_2().child("rireki"))
            .child(self.icon_button("m-day", Some(Icon::CalendarDay), "日", mode == Mode::Day, cx, |t, _, _| {
                t.mode = Mode::Day
            }))
            .child(self.icon_button("m-week", Some(Icon::CalendarWeek), "週", mode == Mode::Week, cx, |t, _, _| {
                t.mode = Mode::Week
            }))
            .child(self.icon_button(
                "m-tl",
                Some(Icon::Timeline),
                "タイムライン",
                mode == Mode::Timeline,
                cx,
                |t, _, _| t.mode = Mode::Timeline,
            ))
            .child(div().w_3())
            .child(self.icon_button("prev", Some(Icon::ChevronLeft), "", false, cx, |t, w, _| t.step(-1.0, w)))
            .child(self.icon_button("today", Some(Icon::Today), "今日", false, cx, |t, w, _| {
                t.day0 = Local::now().date_naive();
                if t.mode == Mode::Timeline {
                    let width = t.spp * t.plot_width(w) as f64;
                    t.t0 = now() - width * 0.85;
                }
            }))
            .child(self.icon_button("next", Some(Icon::ChevronRight), "", false, cx, |t, w, _| t.step(1.0, w)))
            .child(div().w_3());
        for (id, label, secs) in presets {
            row1 = row1.child(self.button(id, label, false, cx, move |t, w, _| {
                t.mode = Mode::Timeline;
                if secs == 0.0 {
                    if let Some((a, b)) = t.store.time_bounds() {
                        t.set_range(a, b, w);
                    }
                } else {
                    let n = now();
                    t.set_range(n - secs * 0.95, n + secs * 0.05, w);
                }
            }));
        }
        row1 = row1
            .child(div().w_3())
            .child(self.icon_button("zin", Some(Icon::ZoomIn), "", false, cx, |t, w, _| t.zoom(0.5, w)))
            .child(self.icon_button("zout", Some(Icon::ZoomOut), "", false, cx, |t, w, _| t.zoom(2.0, w)))
            .child(div().ml_3().text_sm().text_color(c(theme::MUTED)).child(range_label));

        let mut row2 = div().flex().items_center().gap_1p5().text_sm();
        row2 = row2.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(c(theme::MUTED))
                .child(icons::icon(Icon::Layers, theme::MUTED))
                .child("まとめ方"),
        );
        for g in Granularity::ALL {
            let id: &'static str = match g {
                Granularity::Auto => "g-auto",
                Granularity::SameTool => "g-same",
                Granularity::Turn => "g-turn",
                Granularity::Call => "g-call",
            };
            row2 = row2.child(self.button(id, g.label(), self.gran == g, cx, move |t, _, _| t.gran = g));
        }
        row2 = row2
            .child(div().w_3())
            .child(self.icon_button("f-claude", Some(Icon::Claude), "Claude Code", self.show_claude, cx, |t, _, _| {
                t.show_claude = !t.show_claude;
                t.rebuild_order();
            }))
            .child(self.icon_button("f-codex", Some(Icon::Codex), "Codex", self.show_codex, cx, |t, _, _| {
                t.show_codex = !t.show_codex;
                t.rebuild_order();
            }))
            .child(div().w_3());
        if let Some(f) = &self.focus {
            let name = f.rsplit('/').find(|s| !s.is_empty()).unwrap_or(f).to_string();
            row2 = row2
                .child(self.icon_button(
                    "unfocus",
                    Some(Icon::Folder),
                    format!("{name} のみ ✕"),
                    true,
                    cx,
                    |t, _, _| {
                        t.focus = None;
                        t.rebuild_order();
                    },
                ))
                .child(div().w_3());
        }
        for (name, col) in theme::LEGEND {
            row2 = row2.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(div().size_2p5().rounded_sm().bg(c(*col)))
                    .child(*name),
            );
        }

        div()
            .h(px(TOOLBAR_H))
            .flex()
            .flex_col()
            .justify_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .child(row1)
            .child(row2)
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
                let h = self.plot_height(window);
                let center = (self.cal_scroll + h / 2.0) / self.px_per_hour;
                self.px_per_hour = (self.px_per_hour / f as f32).clamp(20.0, 20000.0);
                self.cal_scroll = (center * self.px_per_hour - h / 2.0).max(0.0);
            }
        }
    }

    fn week_start(&self) -> NaiveDate {
        self.day0 - Duration::days(self.day0.weekday().num_days_from_monday() as i64)
    }

    // ------------------------------------------------------------ タイムライン

    fn timeline(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let pw = self.plot_width(window);
        let ph = self.plot_height(window);
        let v0 = self.t0;
        let v1 = self.t0 + self.spp * pw as f64;
        let tracks = self.rows_in(v0, v1);
        let max_scroll = (tracks.len() as f32 * ROW_H - ph + ROW_H).max(0.0);
        self.row_scroll = self.row_scroll.min(max_scroll);
        let first_row = (self.row_scroll / ROW_H) as usize;
        let rows_visible = (ph / ROW_H) as usize + 2;
        let spp = self.spp;
        let x_of = move |t: f64| ((t - v0) / spp) as f32;

        // 時間軸
        let mut axis = div().relative().h(px(AXIS_H)).w(px(pw)).overflow_hidden();
        let mut grid = div().absolute().top_0().left_0().size_full();
        for (t, label, major) in ticks(v0, v1, self.spp) {
            let x = x_of(t);
            axis = axis.child(
                div()
                    .absolute()
                    .left(px(x + 3.0))
                    .top(px(if major { 2.0 } else { 14.0 }))
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(c(if major { theme::TEXT } else { theme::MUTED }))
                    .child(label),
            );
            grid = grid.child(div().absolute().left(px(x)).top_0().bottom_0().w(px(1.0)).bg(c(if major {
                0x353b47
            } else {
                theme::GRID
            })));
        }
        let nx = x_of(now());
        if (0.0..pw).contains(&nx) {
            grid = grid.child(div().absolute().left(px(nx)).top_0().bottom_0().w(px(1.0)).bg(c(theme::NOW)));
        }

        let mut labels = div().relative().w(px(self.label_w)).h(px(ph)).overflow_hidden();
        let mut plot = div().relative().w(px(pw)).h(px(ph)).overflow_hidden().child(grid);
        let mut blocks: Vec<BlockPaint> = Vec::new();
        let mut row_y_of_lane = std::collections::HashMap::new();

        for (row, r) in tracks.iter().enumerate().skip(first_row).take(rows_visible) {
            let y = row as f32 * ROW_H - self.row_scroll;
            let tr = match r {
                Row::Header { cwd, sessions, collapsed } => {
                    let name = cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or(cwd).to_string();
                    let (cwd_c, collapsed) = (cwd.clone(), *collapsed);
                    labels = labels.child(
                        div()
                            .id(ElementId::NamedInteger("grp".into(), row as u64))
                            .absolute()
                            .top(px(y))
                            .left_0()
                            .w(px(self.label_w))
                            .h(px(ROW_H))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .bg(c(0x20242d))
                            .border_t_1()
                            .border_color(c(0x3a4150))
                            .text_xs()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .cursor_pointer()
                            .hover(|d| d.bg(c(0x283040)))
                            .child(icons::icon(
                                if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                                theme::MUTED,
                            ))
                            .child(icons::icon(Icon::Folder, theme::ACCENT))
                            .child(div().font_weight(FontWeight::BOLD).text_color(c(theme::ACCENT)).child(name))
                            .child(div().text_color(c(theme::MUTED)).child(format!("{} セッション", sessions.len())))
                            .child({
                                let (cwd_f, focused) = (cwd.clone(), self.focus.as_ref() == Some(cwd));
                                div()
                                    .id(ElementId::NamedInteger("grp-focus".into(), row as u64))
                                    .ml_auto()
                                    .px_1()
                                    .rounded_sm()
                                    .text_color(c(if focused { theme::ACCENT } else { theme::MUTED }))
                                    .hover(|d| d.bg(c(0x3a4150)).text_color(c(theme::TEXT)))
                                    .child(if focused { "解除" } else { "注目" })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.focus = if this.focus.as_ref() == Some(&cwd_f) {
                                            None
                                        } else {
                                            Some(cwd_f.clone())
                                        };
                                        this.rebuild_order();
                                        this.row_scroll = 0.0;
                                        cx.notify();
                                    }))
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if !this.collapsed.remove(&cwd_c) {
                                    this.collapsed.insert(cwd_c.clone());
                                }
                                cx.notify();
                            })),
                    );
                    plot = plot.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(y))
                            .h(px(ROW_H))
                            .bg(c(0x1b1e25))
                            .border_t_1()
                            .border_color(c(0x3a4150)),
                    );
                    // 折りたたみ中はグループ内のメインレーンを 1 行に重ねて描く
                    if collapsed {
                        for &si in sessions {
                            let lane = &self.store.sessions[si].lanes[0];
                            for b in layout::aggregate(lane, (v0, v1), self.spp, self.gran) {
                                let x = x_of(b.start).max(-2.0);
                                let w = (x_of(b.end).min(pw + 2.0) - x).max(MIN_BLOCK);
                                let sel = Selection { session: si, lane: 0, first: b.first, last: b.last };
                                blocks.push(self.block_paint(sel, (x, y + 3.0, w, ROW_H - 6.0), false));
                            }
                        }
                    }
                    continue;
                }
                Row::Lane(t) => t,
            };
            row_y_of_lane.insert((tr.session, tr.lane), y);
            let s = &self.store.sessions[tr.session];
            let lane = &s.lanes[tr.lane];
            let new_session = row > 0 && matches!(&tracks[row - 1], Row::Lane(p) if p.session != tr.session);

            let (title, sub) = if tr.lane == 0 {
                let t = if s.title.is_empty() { "(無題)" } else { s.title.as_str() };
                (one_line(t, 60), false)
            } else {
                (format!("↳ {}", one_line(&lane.name, 60)), true)
            };
            let (badge, badge_color) = icons::source_icon(s.source);
            let (session, lane_i) = (tr.session, tr.lane);
            let indent = 16.0 + 16.0 * tr.depth as f32;
            let n = lane.events.len();
            labels = labels.child(
                div()
                    .id(ElementId::NamedInteger("lbl".into(), row as u64))
                    .absolute()
                    .top(px(y))
                    .left_0()
                    .w(px(self.label_w))
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .pl(px(indent))
                    .when(new_session, |d| d.border_t_1().border_color(c(theme::BORDER)))
                    .text_xs()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|d| d.bg(c(0x232733)))
                    .when(!sub, |d| d.child(icons::icon(badge, badge_color)))
                    .child(div().text_color(c(if sub { 0xb9a3f0 } else { theme::TEXT })).child(title))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.navigate(Selection { session, lane: lane_i, first: 0, last: n - 1 }, cx);
                        cx.notify();
                    })),
            );
            if new_session {
                plot = plot.child(div().absolute().left_0().right_0().top(px(y)).h(px(1.0)).bg(c(theme::BORDER)));
            }
            for b in layout::aggregate(lane, (v0, v1), self.spp, self.gran) {
                let x = x_of(b.start).max(-2.0);
                let x2 = x_of(b.end).min(pw + 2.0);
                let w = (x2 - x).max(MIN_BLOCK);
                let sel = Selection { session: tr.session, lane: tr.lane, first: b.first, last: b.last };
                blocks.push(self.block_paint(sel, (x, y + 3.0, w, ROW_H - 6.0), false));
            }
        }
        plot = plot.child(self.blocks_canvas(blocks));

        // サブエージェントの起動位置から子レーンへの接続線
        let lane_rows = tracks.iter().skip(first_row).take(rows_visible).filter_map(|r| match r {
            Row::Lane(t) => Some(t),
            Row::Header { .. } => None,
        });
        for tr in lane_rows.filter(|t| t.lane != 0 || t.link.is_some()) {
            let s = &self.store.sessions[tr.session];
            let lane = &s.lanes[tr.lane];
            let Some(&cy) = row_y_of_lane.get(&(tr.session, tr.lane)) else {
                continue;
            };
            let t0 = lane.events.first().map_or(s.start, |e| e.start);
            let (pkey, t) = match tr.link {
                Some((ps, pl, t)) => ((ps, pl), t),
                None => {
                    ((tr.session, s.parents.get(tr.lane).copied().flatten().unwrap_or(0)), lane.spawn_t.unwrap_or(t0))
                }
            };
            let py = row_y_of_lane.get(&pkey).copied().unwrap_or(-ROW_H);
            let line = c(0xa77bf3).opacity(0.7);
            // 起動位置で親レーンから下ろし、子レーンの先頭まで横に伸ばす
            let x = x_of(t).clamp(0.0, pw - 1.0);
            let top = py + ROW_H / 2.0;
            let mid = cy + ROW_H / 2.0;
            plot =
                plot.child(div().absolute().left(px(x)).top(px(top)).w(px(1.0)).h(px((mid - top).max(0.0))).bg(line));
            plot =
                plot.child(div().absolute().left(px(x - 2.5)).top(px(top - 2.5)).size(px(6.0)).rounded_full().bg(line));
            let x2 = x_of(t0).clamp(0.0, pw);
            if x2 > x {
                plot = plot.child(div().absolute().left(px(x)).top(px(mid)).w(px(x2 - x)).h(px(1.0)).bg(line));
            }
        }

        if tracks.is_empty() {
            plot = plot.child(div().absolute().top(px(40.0)).left(px(20.0)).text_color(c(theme::MUTED)).child(
                if self.loading.is_some() {
                    "読み込み中…"
                } else {
                    "この期間の作業履歴はありません"
                },
            ));
        }

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .child(
                        div()
                            .w(px(self.label_w))
                            .h(px(AXIS_H))
                            .px_2()
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(format!(
                                "{} ディレクトリ · {} レーン",
                                tracks.iter().filter(|r| matches!(r, Row::Header { .. })).count(),
                                tracks.iter().filter(|r| matches!(r, Row::Lane(_))).count()
                            )),
                    )
                    .child(axis)
                    .border_b_1()
                    .border_color(c(theme::BORDER)),
            )
            .child(div().flex().child(labels.border_r_1().border_color(c(theme::BORDER))).child(plot))
            .into_any_element()
    }

    // ------------------------------------------------------------ カレンダー (日 / 週)

    fn calendar(&mut self, window: &Window) -> AnyElement {
        let days = if self.mode == Mode::Day { 1 } else { 7 };
        let first = if self.mode == Mode::Day { self.day0 } else { self.week_start() };
        let total_w = self.plot_width(window) + self.label_w;
        let ph = self.plot_height(window);
        let col_w = (total_w - HOUR_LABEL_W) / days as f32;
        let pph = self.px_per_hour;
        let spp = 3600.0 / pph as f64;
        let scroll = self.cal_scroll;
        let today = Local::now().date_naive();

        let mut header =
            div().flex().h(px(AXIS_H)).border_b_1().border_color(c(theme::BORDER)).child(div().w(px(HOUR_LABEL_W)));
        let mut body = div().relative().h(px(ph)).w(px(total_w)).overflow_hidden();

        // 時刻ラベルとグリッド
        let step_h = [0.25f32, 0.5, 1.0, 2.0, 3.0].into_iter().find(|s| s * pph >= 40.0).unwrap_or(3.0);
        let mut h = 0.0f32;
        while h <= 24.0 {
            let y = h * pph - scroll;
            if y > -20.0 && y < ph + 20.0 {
                let mins = (h * 60.0).round() as u32;
                body = body
                    .child(div().absolute().left(px(HOUR_LABEL_W)).right_0().top(px(y)).h(px(1.0)).bg(c(theme::GRID)))
                    .child(
                        div()
                            .absolute()
                            .left(px(4.0))
                            .top(px(y - 7.0))
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(format!("{:02}:{:02}", mins / 60, mins % 60)),
                    );
            }
            h += step_h;
        }

        let mut blocks: Vec<BlockPaint> = Vec::new();
        let mut now_lines = Vec::new();
        for d in 0..days {
            let date = first + Duration::days(d as i64);
            let ds = midnight(date);
            let de = midnight(date + Duration::days(1));
            let x0 = HOUR_LABEL_W + d as f32 * col_w;
            let is_today = date == today;
            header = header.child(
                div()
                    .w(px(col_w))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(c(if is_today { theme::ACCENT } else { theme::TEXT }))
                    .when(is_today, |d| d.font_weight(FontWeight::BOLD))
                    .child(date.format("%m/%d (%a)").to_string()),
            );
            body = body.child(div().absolute().left(px(x0)).top_0().bottom_0().w(px(1.0)).bg(c(theme::BORDER)));

            let tracks = self.tracks_in(ds, de);
            if tracks.is_empty() {
                continue;
            }
            let tw = (col_w - 4.0) / tracks.len() as f32;
            let v0 = ds + scroll as f64 * spp;
            let v1 = v0 + ph as f64 * spp;
            for (i, tr) in tracks.iter().enumerate() {
                let lane = &self.store.sessions[tr.session].lanes[tr.lane];
                let x = x0 + 2.0 + i as f32 * tw;
                for b in layout::aggregate(lane, (v0.max(ds), v1.min(de)), spp, self.gran) {
                    let y = ((b.start.max(ds) - ds) / spp) as f32 - scroll;
                    let y2 = ((b.end.min(de) - ds) / spp) as f32 - scroll;
                    let sel = Selection { session: tr.session, lane: tr.lane, first: b.first, last: b.last };
                    let w = (tw - 1.0).max(2.0);
                    blocks.push(self.block_paint(sel, (x, y, w, (y2 - y).max(MIN_BLOCK)), true));
                }
            }
            if is_today {
                let y = ((now() - ds) / spp) as f32 - scroll;
                now_lines.push(div().absolute().left(px(x0)).w(px(col_w)).top(px(y)).h(px(2.0)).bg(c(theme::NOW)));
            }
        }
        body = body.child(self.blocks_canvas(blocks)).children(now_lines);

        div().flex().flex_col().child(header).child(body).into_any_element()
    }

    // ------------------------------------------------------------ 詳細パネル

    fn detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = div()
            .id("detail")
            .w(px(self.detail_w))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .text_color(c(theme::TEXT));
        let Some(sel) = self.selected else {
            return panel
                .child(div().text_color(c(theme::MUTED)).child("ブロックをクリックすると詳細を表示します。"))
                .child(
                    div().text_xs().text_color(c(theme::MUTED)).child(
                        "操作: ドラッグ / ホイールで移動、⌘ or Ctrl + ホイールで拡大縮小。レーン名クリックでレーン全体を選択。",
                    ),
                )
                .into_any_element();
        };
        let s = &self.store.sessions[sel.session];
        let lane = &s.lanes[sel.lane];
        let evs = &lane.events[sel.first..=sel.last];
        let start = evs.first().map(|e| e.start).unwrap_or(0.0);
        let end = evs.iter().map(|e| e.end).fold(start, f64::max);
        let single = evs.len() == 1;

        let mut p = panel
            .when(!self.history.is_empty(), |d| {
                d.child(div().flex().child(self.icon_button(
                    "back",
                    Some(Icon::ArrowLeft),
                    format!("戻る ({})", self.history.len()),
                    false,
                    cx,
                    |t, _, cx| t.back(cx),
                )))
            })
            .child({
                let (i, col) = icons::source_icon(s.source);
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(icons::icon(i, col))
                    .child(format!("{} · {}", s.source.label(), s.cwd))
            })
            .child(div().font_weight(FontWeight::BOLD).child(if s.title.is_empty() {
                "(無題)".to_string()
            } else {
                s.title.clone()
            }))
            .child(div().text_xs().text_color(c(theme::MUTED)).child(format!("session {}", s.id)));
        if sel.lane != 0 {
            let (session, parent_id) = (sel.session, lane.spawned_by.clone());
            p = p.child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(icons::icon(Icon::Bot, 0xb9a3f0))
                    .child(div().text_color(c(0xb9a3f0)).child(format!("サブエージェント: {}", lane.name)))
                    .when_some(parent_id, |d, id| {
                        d.child(self.icon_button(
                            "to-parent",
                            Some(Icon::ArrowUp),
                            "起動元",
                            false,
                            cx,
                            move |t, _, cx| {
                                let s = &t.store.sessions[session];
                                if let Some(i) =
                                    s.lanes[0].events.iter().position(|e| e.call_id.as_deref() == Some(id.as_str()))
                                {
                                    t.navigate(Selection { session, lane: 0, first: i, last: i }, cx);
                                }
                            },
                        ))
                    }),
            );
        }
        if !lane.model.is_empty() || !lane.effort.is_empty() {
            let dash = |v: &str| if v.is_empty() { "-".to_string() } else { v.to_string() };
            p = p.child(
                div()
                    .flex()
                    .gap_3()
                    .text_xs()
                    .child(div().text_color(c(theme::MUTED)).child("モデル"))
                    .child(div().child(dash(&lane.model)))
                    .child(div().text_color(c(theme::MUTED)).child("エフォート"))
                    .child(div().child(dash(&lane.effort))),
            );
        }
        p = p.child(
            div().py_1().border_b_1().border_color(c(theme::BORDER)).text_xs().text_color(c(theme::MUTED)).child(
                format!(
                    "{} – {}  ·  {}  ·  {} 件",
                    local(start).format("%m/%d %H:%M:%S"),
                    local(end).format("%H:%M:%S"),
                    fmt_dur(end - start),
                    evs.len()
                ),
            ),
        );

        let texts = self.detail_cache.as_ref().filter(|d| d.sel == sel).map(|d| d.texts.as_slice()).unwrap_or(&[]);
        for (i, e) in evs.iter().enumerate().take(DETAIL_MAX) {
            let (input, output) = texts.get(i).map(|(a, b)| (a.as_str(), b.as_str())).unwrap_or(("", ""));
            let idx = sel.first + i;
            let color = theme::event_color(e);
            let mut card = div()
                .id(ElementId::NamedInteger("ev".into(), idx as u64))
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(c(0x23272f))
                .border_l_2()
                .border_color(c(color))
                .when(!single, |d| d.cursor_pointer().hover(|d| d.bg(c(0x292e38))))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .text_xs()
                        .child(div().text_color(c(theme::MUTED)).child(local(e.start).format("%H:%M:%S").to_string()))
                        .child(div().font_weight(FontWeight::BOLD).text_color(c(color)).child(e.name.clone()))
                        .when(e.end > e.start, |d| {
                            d.child(div().text_color(c(theme::MUTED)).child(fmt_dur(e.end - e.start)))
                        }),
                );
            if !e.summary.is_empty() && e.kind == Kind::Tool || e.kind == Kind::Agent {
                card = card.child(div().text_xs().child(e.summary.clone()));
            }
            if e.kind == Kind::Agent {
                if let Some(li) = e.call_id.as_deref().and_then(|id| s.lane_spawned_by(id)) {
                    let session = sel.session;
                    let n = s.lanes[li].events.len();
                    card = card.child(div().flex().child(self.icon_button(
                        "to-sub",
                        Some(Icon::Bot),
                        format!("サブエージェント ({n} 件) を開く"),
                        false,
                        cx,
                        move |t, w, cx| {
                            t.navigate(Selection { session, lane: li, first: 0, last: n - 1 }, cx);
                            t.reveal(session, li, w);
                        },
                    )));
                }
            }
            if single || e.kind != Kind::Tool {
                card = card.child(code_box(input.to_string(), e.kind == Kind::Tool || e.kind == Kind::Agent));
            }
            if single && !output.is_empty() {
                card = card
                    .child(div().text_xs().text_color(c(theme::MUTED)).child("結果"))
                    .child(code_box(output.to_string(), true));
            }
            let session = sel.session;
            let lane_i = sel.lane;
            if !single {
                card = card.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.navigate(Selection { session, lane: lane_i, first: idx, last: idx }, cx);
                    cx.notify();
                }));
            }
            p = p.child(card);
        }
        if evs.len() > DETAIL_MAX {
            p = p.child(
                div().text_xs().text_color(c(theme::MUTED)).child(format!("… 他 {} 件", evs.len() - DETAIL_MAX)),
            );
        }
        p.into_any_element()
    }
}

fn code_box(text: String, mono: bool) -> Div {
    let d = div()
        .min_w_0()
        .overflow_hidden()
        .p_1p5()
        .rounded_sm()
        .bg(c(0x15171c))
        .text_xs()
        .text_color(c(0xc5c9d1))
        .when(mono, |d| d.font_family("Menlo"));
    if !text.starts_with("*** Begin Patch") {
        if !text.lines().take(3).any(|l| yaml_key(l).is_some()) {
            return d.child(text);
        }
        // YAML 風に整形した引数・結果はキーを色分けする
        return d.flex().flex_col().children(text.lines().map(|l| match yaml_key(l) {
            Some(n) => div()
                .min_w_0()
                .flex()
                .child(div().flex_none().text_color(c(0x8ab4f8)).child(l[..n].to_string()))
                .child(div().flex_1().min_w_0().child(l[n..].to_string())),
            None => div().min_w_0().child(if l.is_empty() { " ".to_string() } else { l.to_string() }),
        }));
    }
    // apply_patch は差分として行ごとに色分けする
    d.flex().flex_col().children(text.lines().map(|l| {
        let color = match l.as_bytes().first() {
            _ if l.starts_with("*** ") => 0xe0b341,
            Some(b'+') => 0x7fd18b,
            Some(b'-') => 0xf07c7c,
            Some(b'@') => 0x6fa8dc,
            _ => 0xc5c9d1,
        };
        div().min_w_0().text_color(c(color)).child(if l.is_empty() { " ".to_string() } else { l.to_string() })
    }))
}

/// `  - key: value` 形式の行なら、キー（コロンまで）の終端位置を返す
fn yaml_key(l: &str) -> Option<usize> {
    let body = l.trim_start_matches(' ');
    let body = body.strip_prefix("- ").unwrap_or(body);
    let start = l.len() - body.len();
    let n = body.bytes().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')).count();
    if n == 0 || !body.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    match &body.as_bytes()[n..] {
        [b':'] | [b':', b' ', ..] => Some(start + n + 1),
        _ => None,
    }
}

/// 目盛り (時刻, ラベル, 主目盛りか)
fn ticks(v0: f64, v1: f64, spp: f64) -> Vec<(f64, String, bool)> {
    const STEPS: &[f64] = &[
        1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0, 10800.0, 21600.0, 43200.0, 86400.0, 172800.0,
        604800.0, 2592000.0,
    ];
    let step = STEPS.iter().copied().find(|s| s / spp >= 90.0).unwrap_or(2592000.0);
    let offset = local(v0).offset().local_minus_utc() as f64;
    let mut t = ((v0 + offset) / step).floor() * step - offset;
    let mut out = Vec::new();
    while t <= v1 && out.len() < 400 {
        let lt = local(t);
        let is_day = lt.hour() == 0 && lt.minute() == 0 && lt.second() == 0;
        let label = if step >= 86400.0 {
            lt.format("%m/%d").to_string()
        } else if is_day {
            lt.format("%m/%d").to_string()
        } else if step < 60.0 {
            lt.format("%H:%M:%S").to_string()
        } else {
            lt.format("%H:%M").to_string()
        };
        out.push((t, label, is_day));
        t += step;
    }
    out
}

impl Render for Rireki {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.mode {
            Mode::Timeline => self.timeline(window, cx),
            _ => self.calendar(window),
        };
        let status = match self.loading {
            Some((d, t)) if t > 0 => format!("索引を更新中 {d}/{t} ファイル · {} セッション", self.order.len()),
            Some(_) => "履歴を読み込み中…".to_string(),
            None => format!("{} セッション", self.order.len()),
        };
        let label_handle = self
            .resize_handle("resize-label", cx, |t, x| Resize::Label { x, w: t.label_w })
            .absolute()
            .top_0()
            .left(px(self.label_w - HANDLE_W / 2.0));
        let detail_handle = self.resize_handle("resize-detail", cx, |t, x| Resize::Detail { x, w: t.detail_w });
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_mouse_move(cx.listener(Self::on_resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|t, _: &MouseUpEvent, _, _| t.resize = None))
            .bg(c(theme::BG))
            .text_color(c(theme::TEXT))
            .child(self.toolbar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("main")
                            .flex_1()
                            .overflow_hidden()
                            .on_scroll_wheel(cx.listener(Self::on_wheel))
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
                            .on_mouse_move(cx.listener(Self::on_move))
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_up))
                            .relative()
                            .child(main)
                            .when(self.mode == Mode::Timeline, |d| d.child(label_handle)),
                    )
                    .child(detail_handle)
                    .child(
                        AnyView::from(self.detail_view.clone())
                            .cached(StyleRefinement::default().w(px(self.detail_w)).h_full().flex_none()),
                    ),
            )
            .child(
                div()
                    .h(px(STATUS_H))
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_3()
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .bg(c(theme::PANEL))
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(status)
                    .when_some(self.hover.clone(), |d, h| d.child(div().text_color(c(theme::TEXT)).child(h))),
            )
    }
}

/// exec の JS を整形する。初回はモック実行してキャッシュに保存する
fn exec_detail_cached(cache: Option<&JsCache>, js: &str) -> String {
    let h = crate::jsmock::hash(js);
    if let Some(d) = cache.and_then(|c| c.get(h)) {
        return d;
    }
    let d = loader::exec_detail(js);
    if let Some(c) = cache {
        c.put(h, &d);
    }
    d
}
