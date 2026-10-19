//! カレンダー（日 / 週 / 月）。
//!
//! - 日・週: 縦軸が時刻。セッション（メイン + サブエージェント + 起動した子セッション）が動いていた区間を
//!   1 つのブロックにし、上端を開始時刻にそろえて枠の中に名前を書く。色はプロジェクトの色。
//!   幅に余裕があれば、ユーザーの指示をその時刻の位置に並べる（押すとそのターンを開く）。
//! - 月: 日ごとの稼働時間を濃淡で示し、その日のセッションを開始順に並べる。

mod segments;
mod columns;
use segments::*;
use columns::*;

use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{AnyElement, ClickEvent, Context, ElementId, FontWeight, SharedString, Window, canvas, div, prelude::*, px};

use super::header::weekday_ja;
use super::widgets::{dot, project_tile, tip};
use super::{HOUR_LABEL_W, Rireki, Selection, Track, is_live, local, midnight, now};
use std::collections::HashMap;
use std::rc::Rc;

use crate::model::{Kind, one_line};
use crate::prefs::Mode;
use crate::theme::{self, c};

/// 00:00 / 24:00 のラベルが上下のバーに隠れないよう、日付の境界に余白を残す。
const TIME_EDGE_PAD: f32 = 16.0;
/// 日の見出し（曜日・日付と、その日の稼働時間・件数）の高さ
pub(super) const DAY_HEAD_H: f32 = 44.0;
/// 月表示の曜日の見出しの高さ
const MONTH_HEAD_H: f32 = 26.0;
/// ブロックの最小の高さ。短いセッションでも名前を 1 行読めるようにする
const MIN_H: f32 = 18.0;
/// 手が空いたとみなす長さ。これより長く止まった所でブロックを分ける
const IDLE: f64 = 15.0 * 60.0;
/// 指示を枠の中に並べる最小の幅
const PROMPT_MIN_W: f32 = 150.0;
const USER: u32 = 0xe0b341;

pub(super) fn clamp_scroll(scroll: f32, px_per_hour: f32, height: f32) -> f32 {
    let min = -TIME_EDGE_PAD;
    let max = (24.0 * px_per_hour - height + TIME_EDGE_PAD).max(min);
    scroll.clamp(min, max)
}

/// 月表示の最初の日（その月の 1 日を含む週の月曜日）
pub(super) fn month_grid_start(d: NaiveDate) -> NaiveDate {
    let first = d.with_day(1).unwrap_or(d);
    first - Duration::days(first.weekday().num_days_from_monday() as i64)
}

/// 3h12m / 45m のような短い長さ
pub(super) fn fmt_hm(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if m < 60 { format!("{m}m") } else { format!("{}h{:02}m", m / 60, m % 60) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    first: NaiveDate,
    days: usize,
    generation: u64,
}

use crate::model::SessionIndexHasher as SessionHash;

/// 表示中の期間だけ保持する。スクロール・選択・幅変更で全イベントを走査し直さない。
#[derive(Default)]
pub(super) struct Cache {
    layout: Option<(Key, Rc<CalendarLayout>)>,
    columns: Option<(f64, Rc<Columns>)>,
    /// 期間を移ったばかりで、作業のある時刻へのスクロールを待っている
    seek: bool,
}

/// 各区間の (列, 右へ広げた列数, その重なりの列数)
/// 当たり判定の矩形と、押したときの選択
type Hit = ((f32, f32, f32, f32), Selection);

type Columns = Vec<Vec<(usize, usize, usize)>>;

pub(super) struct CalendarLayout {
    days: Vec<DayLayout>,
    counts: String,
    empty: bool,
    /// 期間内のプロジェクトと稼働時間（長い順）。下端の凡例に使う
    pub(super) projects: Vec<(String, f64)>,
}

struct DayLayout {
    tracks: Rc<Vec<Track>>,
    groups: Vec<GroupSeg>,
    /// ブロックを重ねずに数えた稼働時間（秒）
    active: f64,
    /// その日に動いていたセッション（起動元でまとめた数）
    sessions: usize,
}

impl DayLayout {
    fn new(tracks: Rc<Vec<Track>>, mut groups: Vec<GroupSeg>) -> Self {
        for g in &mut groups {
            g.session = tracks[g.lanes[0]].session;
        }
        groups.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.session.cmp(&b.session)));
        let mut active = 0.0;
        let mut cur: Option<(f64, f64)> = None;
        for g in &groups {
            cur = match cur {
                Some((a, b)) if g.start <= b => Some((a, b.max(g.end))),
                Some((a, b)) => {
                    active += b - a;
                    Some((g.start, g.end))
                }
                None => Some((g.start, g.end)),
            };
        }
        if let Some((a, b)) = cur {
            active += b - a;
        }
        let sessions = Self::session_count(&groups);
        Self { tracks, groups, active, sessions }
    }

    /// 代表のセッション番号を数える。ビットが同じでも、完全な番号で重複を判定する。
    fn session_count(groups: &[GroupSeg]) -> usize {
        fn sorted(groups: &[GroupSeg]) -> usize {
            let mut sessions: Vec<usize> = groups.iter().map(|g| g.session).collect();
            sessions.sort_unstable();
            sessions.dedup();
            sessions.len()
        }
        let Some(first) = groups.first() else { return 0; };
        let mut previous = first.session;
        let mut count = 1;
        let mut ascending = true;
        let mut descending = true;
        for g in &groups[1..] {
            ascending &= previous <= g.session;
            descending &= previous >= g.session;
            if !ascending && !descending { break; }
            count += usize::from(previous != g.session);
            previous = g.session;
        }
        if ascending || descending { return count; }
        if groups.len() <= 128 {
            if count > 32 { return sorted(groups); }
            let mut sample_mask = 0u64;
            for (i, g) in groups.iter().enumerate().take(8) {
                let bit = 1u64 << (g.session & 63);
                if sample_mask & bit != 0 && groups[..i].iter().any(|previous|
                    previous.session != g.session && (previous.session ^ g.session) & 63 == 0) {
                    return sorted(groups);
                }
                sample_mask |= bit;
            }
        }
        if groups.len() > 128 {
            let mut sessions: std::collections::HashSet<usize, std::hash::BuildHasherDefault<SessionHash>> =
                std::collections::HashSet::with_capacity_and_hasher(groups.len(), Default::default());
            sessions.extend(groups.iter().map(|g| g.session));
            sessions.len()
        } else {
            let mut sessions = Vec::with_capacity(groups.len());
            let mut mask = 0u64;
            for g in groups {
                let bit = 1u64 << (g.session & 63);
                if mask & bit == 0 || !sessions.contains(&g.session) {
                    sessions.push(g.session);
                    mask |= bit;
                }
            }
            sessions.len()
        }
    }

    /// 月表示に使う最初の区間だけを、開始順にセッションごとに一つ返す。
    fn month_sessions(&self, limit: usize) -> impl Iterator<Item = &GroupSeg> {
        let mut seen = Vec::new();
        self.groups.iter().filter(move |group| {
            if seen.contains(&group.session) { return false; }
            seen.push(group.session);
            true
        }).take(limit)
    }
}

impl Cache {
    fn get(&self, key: Key) -> Option<Rc<CalendarLayout>> {
        self.layout.as_ref().filter(|(k, _)| *k == key).map(|(_, layout)| layout.clone())
    }

    fn set<'a>(&mut self, key: Key, days: Vec<DayLayout>, cwd: impl Fn(usize) -> &'a str) -> Rc<CalendarLayout> {
        let moved = self.layout.as_ref().is_none_or(|(k, _)| (k.first, k.days) != (key.first, key.days));
        self.seek |= moved;
        let mut sessions: std::collections::HashSet<usize, std::hash::BuildHasherDefault<SessionHash>> = Default::default();
        let mut active = 0.0;
        let mut projects: HashMap<&str, f64> = HashMap::new();
        for day in &days {
            active += day.active;
            for g in &day.groups {
                sessions.insert(g.session);
                *projects.entry(cwd(g.session)).or_default() += g.end - g.start;
            }
        }
        let mut projects: Vec<_> = projects.into_iter().map(|(cwd, active)| (cwd.to_owned(), active)).collect();
        projects.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let layout = Rc::new(CalendarLayout {
            empty: sessions.is_empty(),
            counts: format!("{} セッション · 稼働 {}", sessions.len(), fmt_hm(active)),
            days,
            projects,
        });
        self.layout = Some((key, layout.clone()));
        self.columns = None;
        layout
    }

    /// 表示中の期間のレイアウト（凡例用）
    pub(super) fn current(&self) -> Option<Rc<CalendarLayout>> {
        self.layout.as_ref().map(|(_, l)| l.clone())
    }

    fn columns(&mut self, spp: f64) -> Rc<Columns> {
        if let Some((scale, columns)) = &self.columns
            && *scale == spp
        {
            return columns.clone();
        }
        let layout = &self.layout.as_ref().unwrap().1;
        let columns: Rc<Columns> = Rc::new(
            layout
                .days
                .iter()
                .map(|day| {
                    let spans: Vec<_> =
                        day.groups.iter().map(|g| (g.start, g.end.max(g.start + MIN_H as f64 * spp))).collect();
                    pack_columns(&spans, spp)
                })
                .collect(),
        );
        self.columns = Some((spp, columns.clone()));
        columns
    }
}

/// 押したときに開く範囲と、ブロックの見た目に使う情報
struct BlockInfo {
    sel: Option<Selection>,
    title: String,
    cwd: String,
    source: crate::model::Source,
    subs: usize,
    live: bool,
}

impl Rireki {
    fn calendar_layout(&mut self, first: NaiveDate, days: usize) -> Rc<CalendarLayout> {
        let key = Key { first, days, generation: self.data_gen };
        if let Some(layout) = self.calendar_cache.get(key) {
            return layout;
        }
        let started = self.profile.as_ref().map(|_| std::time::Instant::now());
        // 表示範囲に関係なく、期間内に動いていたレーンだけを並べる（範囲外のセッションは出さない）
        let v0 = midnight(first);
        let v1 = midnight(first + Duration::days(days as i64));
        let tracks = Rc::new(self.tracks_for(v0, v1, false));
        let group = self.group_of(&tracks);
        let days: Vec<DayLayout> = if tracks.is_empty() {
            (0..days).map(|_| DayLayout::new(tracks.clone(), Vec::new())).collect()
        } else if days == 1 {
            vec![DayLayout::new(tracks.clone(), group_segments(&day_segments(&self.store.sessions, &tracks, v0, v1), &group))]
        } else {
            let edges: Vec<f64> = (0..=days).map(|day| {
                if day == 0 { v0 } else if day == days { v1 } else { midnight(first + Duration::days(day as i64)) }
            }).collect();
            let spans = sparse_track_days(&edges, tracks.iter().map(|track| self.store.sessions[track.session].lanes[track.lane].range()));
            (0..days).map(|day| {
                let (ds, de) = (edges[day], edges[day + 1]);
                let segments = if let Some(spans) = &spans {
                    indexed_segments(&self.store.sessions, &tracks,
                        spans.iter().enumerate().filter_map(|(ti, &(start, end))| (start <= day && day < end).then_some(ti)), ds, de)
                } else {
                    day_segments(&self.store.sessions, &tracks, ds, de)
                };
                DayLayout::new(tracks.clone(), group_segments(&segments, &group))
            }).collect()
        };
        let sessions = &self.store.sessions;
        let layout = self.calendar_cache.set(key, days, |si| sessions[si].cwd.as_str());
        if let Some(started) = started {
            println!("{}", serde_json::json!({"ui_profile":"calendar_layout", "days":key.days,
                "tracks":tracks.len(),
                "segments":layout.days.iter().map(|d| d.groups.len()).sum::<usize>(),
                "ms":started.elapsed().as_secs_f64()*1000.0}));
        }
        layout
    }

    /// ブロックを押したときの選択。起動元セッションのメインを優先し、その区間のイベントを選ぶ
    fn group_selection(&self, tracks: &[Track], g: &GroupSeg) -> Option<Selection> {
        g.lanes.iter().find_map(|&ti| {
            let t = &tracks[ti];
            let lane = &self.store.sessions[t.session].lanes[t.lane];
            let a = lane.cum_end.partition_point(|&e| e < g.start);
            let b = lane.events.partition_point(|e| e.start <= g.end);
            (a < b).then_some(Selection { session: t.session, lane: t.lane, first: a, last: b - 1 })
        })
    }

    fn block_info(&self, tracks: &[Track], g: &GroupSeg) -> BlockInfo {
        let s = &self.store.sessions[g.session];
        BlockInfo {
            sel: self.group_selection(tracks, g),
            title: if s.title.is_empty() { "(無題)".into() } else { one_line(&s.title, 200) },
            cwd: s.cwd.clone(),
            source: s.source,
            subs: g.lanes.iter().filter(|&&ti| tracks[ti].depth > 0 || tracks[ti].link.is_some()).count(),
            live: g.lanes.iter().any(|&ti| is_live(self.store.sessions[tracks[ti].session].end)),
        }
    }

    pub(super) fn calendar(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.mode == Mode::Month {
            return self.month(window, cx);
        }
        let week = self.mode == Mode::Week;
        let days = if week { 7 } else { 1 };
        let first = if week { self.week_start() } else { self.day0 };
        let total_w = self.main_width(window);
        let ph = self.plot_height(window);
        let col_w = (total_w - HOUR_LABEL_W) / days as f32;
        let pph = self.px_per_hour;
        let spp = 3600.0 / pph as f64;
        let layout = self.calendar_layout(first, days);
        // 期間を移ったとき、見えている時刻に作業がなければ最初の作業の少し前まで動かす
        if std::mem::take(&mut self.calendar_cache.seek) {
            let (v0, v1) = (self.cal_scroll as f64 * spp, (self.cal_scroll + ph) as f64 * spp);
            let mut visible = false;
            let mut earliest = f64::INFINITY;
            for (d, day) in layout.days.iter().enumerate() {
                let ds = midnight(first + Duration::days(d as i64));
                for g in &day.groups {
                    visible |= g.end - ds >= v0 && g.start - ds <= v1;
                    earliest = earliest.min(g.start - ds);
                }
            }
            if !visible && earliest.is_finite() {
                self.cal_scroll = (earliest / spp) as f32 - pph * 0.5;
            }
        }
        self.cal_scroll = clamp_scroll(self.cal_scroll, pph, ph);
        let scroll = self.cal_scroll;
        let today = Local::now().date_naive();
        let columns = self.calendar_cache.columns(spp);
        let y_of = |ds: f64, t: f64| ((t - ds) / spp) as f32 - scroll;

        let mut header = div()
            .flex()
            .h(px(DAY_HEAD_H))
            .flex_none()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().w(px(HOUR_LABEL_W)).flex_none());
        let origin = self.hit_origin.clone();
        let mut body = div()
            .relative()
            .h(px(ph))
            .w(px(total_w))
            .overflow_hidden()
            .child(canvas(move |b, _, _| origin.set(b.origin), |_, _, _, _| {}).absolute().top_0().left_0().size_full());

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

        let mut hits = Vec::new();
        let mut blocks = Vec::new();
        let mut now_lines = Vec::new();
        for d in 0..days {
            let date = first + Duration::days(d as i64);
            let ds = midnight(date);
            let x0 = HOUR_LABEL_W + d as f32 * col_w;
            let is_today = date == today;
            let day = &layout.days[d];
            header = header.child(self.day_head(d, date, day, col_w, week, cx));
            body = body.child(div().absolute().left(px(x0)).top_0().bottom_0().w(px(1.0)).bg(c(theme::BORDER)));
            if is_today {
                body = body
                    .child(div().absolute().left(px(x0)).w(px(col_w)).top_0().bottom_0().bg(c(theme::ACCENT).opacity(0.05)));
            }
            let cols = &columns[d];
            for (k, g) in day.groups.iter().enumerate() {
                let gw = (col_w - 4.0) / cols[k].2 as f32;
                let x = x0 + 2.0 + cols[k].0 as f32 * gw;
                let gw = gw * cols[k].1 as f32;
                let y = y_of(ds, g.start);
                let h = (y_of(ds, g.end) - y).max(MIN_H);
                if y + h < 0.0 || y > ph {
                    continue;
                }
                let w = (gw - 2.0).max(3.0);
                let info = self.block_info(&day.tracks, g);
                if let Some(sel) = info.sel {
                    hits.push(((x, y, w, h), sel));
                }
                // 前日から続く・翌日へ続く辺は角を丸めない
                let cont = (g.start <= ds, g.end >= ds + 86_399.0);
                let prompts =
                    if w >= PROMPT_MIN_W { self.prompt_rows(&info, (x, y, w, h), &|t| y_of(ds, t), &mut hits) } else { Vec::new() };
                blocks.push(self.session_block(&info, (x, y, w, h), cont, prompts, (d, k)));
            }
            if is_today {
                let y = y_of(ds, now());
                now_lines.push(div().absolute().left(px(x0)).w(px(col_w)).top(px(y)).h(px(2.0)).bg(c(theme::NOW)));
                now_lines.push(
                    div().absolute().left(px(x0 - 4.0)).top(px(y - 3.0)).size(px(8.0)).rounded_full().bg(c(theme::NOW)),
                );
            }
        }
        self.hits = hits;
        self.counts = layout.counts.clone();
        body = body.children(blocks).children(now_lines);
        if layout.empty {
            body = body.child(self.empty_state(cx));
        }
        div().flex().flex_col().child(header).child(body).into_any_element()
    }

    /// 日の見出し。曜日・日付と、その日の稼働時間・セッション数
    fn day_head(
        &self,
        d: usize,
        date: NaiveDate,
        day: &DayLayout,
        w: f32,
        week: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_today = date == Local::now().date_naive();
        let weekend = matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        let summary = if day.groups.is_empty() {
            "—".to_string()
        } else if w >= 110.0 {
            format!("{} · {} 件", fmt_hm(day.active), day.sessions)
        } else {
            fmt_hm(day.active)
        };
        div()
            .id(ElementId::NamedInteger("day".into(), d as u64))
            .w(px(w))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_sm()
                    .text_color(c(if is_today {
                        theme::ACCENT
                    } else if weekend {
                        theme::MUTED
                    } else {
                        theme::TEXT
                    }))
                    .child(div().text_xs().child(weekday_ja(date.weekday())))
                    .child(
                        div()
                            .min_w(px(20.0))
                            .h(px(20.0))
                            .px_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .when(is_today, |d| d.bg(c(theme::ACCENT)).text_color(gpui::white()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if week { date.day().to_string() } else { date.format("%-m/%-d").to_string() }),
                    ),
            )
            .child(div().text_xs().text_color(c(theme::SUBTLE)).child(summary))
            .when(week, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(c(theme::HOVER)))
                    .tooltip(tip("この日を日表示で開く", Some("D")))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        t.day0 = date;
                        t.set_mode(Mode::Day, w);
                        cx.notify();
                    }))
            })
    }

    /// セッションのブロック。上端が開始時刻。名前・時刻・プロジェクトを高さに応じて出す
    fn session_block(
        &self,
        info: &BlockInfo,
        (x, y, w, h): (f32, f32, f32, f32),
        (from_prev, to_next): (bool, bool),
        prompts: Vec<AnyElement>,
        (d, k): (usize, usize),
    ) -> AnyElement {
        let color = theme::project_color(&info.cwd);
        let selected = info.sel.is_some_and(|s| self.selected.is_some_and(|x| x.session == s.session));
        let hovered = info.sel.is_some_and(|s| self.hovered == Some(s));
        let (logo, logo_color) = crate::icons::source_icon(info.source);
        let radius = px(4.0);
        // ブロックの上端が画面より上にあるときも、名前は見える位置に留める
        let sticky = (-y).clamp(0.0, (h - MIN_H).max(0.0));
        let text = w >= 28.0;
        let mut inner = div()
            .absolute()
            .top(px(sticky))
            .left(px(3.0))
            .right_0()
            .flex()
            .flex_col()
            .px_1()
            .pt(px(1.0))
            .text_xs()
            .overflow_hidden()
            .when(text, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(16.0))
                        .when(w >= 60.0, |d| d.child(crate::icons::icon(logo, logo_color).size(px(11.0)).flex_none()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(theme::TEXT))
                                .child(info.title.clone()),
                        )
                        .when(info.live, |d| d.child(dot(theme::LIVE, 6.0))),
                )
            });
        if text && h - sticky >= 32.0 {
            let meta = match info.sel {
                Some(_) if info.subs > 0 && w >= 120.0 => format!("{} · +{} サブ", self.project_name(&info.cwd), info.subs),
                _ => self.project_name(&info.cwd),
            };
            inner = inner.child(div().h(px(14.0)).truncate().text_color(c(theme::MUTED)).child(meta));
        }
        div()
            .id(ElementId::NamedInteger(SharedString::from(format!("cal-block-{d}")), k as u64))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .when(!from_prev, |d| d.rounded_t(radius))
            .when(!to_next, |d| d.rounded_b(radius))
            .overflow_hidden()
            .bg(c(color).opacity(if selected || hovered { 0.42 } else { 0.24 }))
            .border_1()
            .border_color(if selected {
                gpui::white()
            } else if hovered {
                gpui::white().opacity(0.6)
            } else {
                c(color).opacity(0.55)
            })
            .child(div().absolute().left_0().top_0().bottom_0().w(px(3.0)).bg(c(color)))
            .children(prompts)
            .child(inner)
            .into_any_element()
    }

    /// 幅のあるブロックの中に、ユーザーの指示をその時刻の位置に並べる。重なるものは省く
    fn prompt_rows(
        &self,
        info: &BlockInfo,
        (bx, by, w, h): (f32, f32, f32, f32),
        y_of: &dyn Fn(f64) -> f32,
        hits: &mut Vec<Hit>,
    ) -> Vec<AnyElement> {
        let Some(sel) = info.sel else { return Vec::new() };
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let users: Vec<usize> = (sel.first..=sel.last).filter(|&i| lane.events[i].kind == Kind::User).collect();
        // 名前と（あれば）プロジェクトの行の下から並べる
        let top = by.max(0.0) + if h >= 32.0 { 32.0 } else { 17.0 };
        let mut last = f32::NEG_INFINITY;
        let mut out = Vec::new();
        for (n, &i) in users.iter().enumerate() {
            let y = y_of(lane.events[i].start).max(top);
            if y < last + 15.0 || y + 14.0 > by + h {
                continue;
            }
            last = y;
            let end = users.get(n + 1).map_or(sel.last, |&j| j - 1);
            let turn = Selection { session: sel.session, lane: sel.lane, first: i, last: end };
            hits.push(((bx + 4.0, y, w - 6.0, 14.0), turn));
            let hovered = self.hovered == Some(turn);
            let selected = self.selected == Some(turn);
            out.push(
                div()
                    .absolute()
                    .top(px(y - by))
                    .left(px(4.0))
                    .right(px(2.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .when(hovered || selected, |d| d.bg(gpui::white().opacity(if selected { 0.16 } else { 0.08 })))
                    .text_xs()
                    .child(dot(USER, 5.0))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(local(lane.events[i].start).format("%H:%M").to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c(theme::TEXT))
                            .child(one_line(&lane.events[i].summary, 120)),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// 月表示。日ごとの稼働時間を濃淡で示し、その日のセッションを開始順に並べる
    fn month(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let first = month_grid_start(self.day0);
        let month = self.day0.month();
        let total_w = self.main_width(window);
        let ph = self.plot_height(window) + DAY_HEAD_H - MONTH_HEAD_H;
        let layout = self.calendar_layout(first, 42);
        self.calendar_cache.seek = false;
        self.hits.clear();
        self.counts = layout.counts.clone();
        let cell_w = total_w / 7.0;
        let cell_h = ph / 6.0;
        let max = layout.days.iter().map(|d| d.active).fold(0.0, f64::max).max(1.0);
        let today = Local::now().date_naive();
        let rows_fit = ((cell_h - 26.0) / 17.0).floor().max(0.0) as usize;
        let head = div().flex().h(px(MONTH_HEAD_H)).flex_none().border_b_1().border_color(c(theme::BORDER)).children(
            (0..7).map(|i| {
                let wd = (first + Duration::days(i)).weekday();
                div()
                    .w(px(cell_w))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(c(if i >= 5 { theme::MUTED } else { theme::TEXT }))
                    .child(weekday_ja(wd))
            }),
        );
        let mut grid = div().flex().flex_col().h(px(ph)).overflow_hidden();
        for r in 0..6 {
            let mut row = div().flex().w(px(total_w)).h(px(cell_h)).flex_none();
            for k in 0..7 {
                let i = r * 7 + k;
                let date = first + Duration::days(i as i64);
                let day = &layout.days[i];
                let in_month = date.month() == month;
                let shown = if day.sessions > rows_fit { rows_fit.saturating_sub(1) } else { day.sessions };
                let heat = (day.active / max).sqrt() as f32;
                let mut cell = div()
                    .id(ElementId::NamedInteger("month-day".into(), i as u64))
                    .w(px(cell_w))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .p_1()
                    .overflow_hidden()
                    .border_r_1()
                    .border_b_1()
                    .border_color(c(theme::GRID))
                    .when(day.active > 0.0, |d| d.bg(c(theme::HEAT[2]).opacity(0.05 + 0.2 * heat)))
                    .when(!in_month, |d| d.opacity(0.45))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::HOVER)))
                    .tooltip(tip("この日を日表示で開く", None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        t.day0 = date;
                        t.set_mode(Mode::Day, w);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(20.0))
                            .text_xs()
                            .child(
                                div()
                                    .min_w(px(20.0))
                                    .h(px(20.0))
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .when(date == today, |d| d.bg(c(theme::ACCENT)).text_color(gpui::white()))
                                    .child(if date.day() == 1 {
                                        format!("{}/{}", date.month(), date.day())
                                    } else {
                                        date.day().to_string()
                                    }),
                            )
                            .child(div().flex_1())
                            .when(day.active > 0.0, |d| {
                                d.child(div().text_color(c(theme::MUTED)).child(fmt_hm(day.active)))
                            }),
                    );
                for g in day.month_sessions(shown) {
                    let si = g.session;
                    let start = g.start;
                    let info = self.block_info(&day.tracks, g);
                    let sel = info.sel;
                    let s = &self.store.sessions[si];
                    let selected = self.selected.is_some_and(|x| x.session == si);
                    cell = cell.child(
                        div()
                            .id(ElementId::NamedInteger(SharedString::from(format!("month-chip-{i}")), si as u64))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .h(px(16.0))
                            .px_1()
                            .rounded_sm()
                            .text_xs()
                            .when(selected, |d| d.bg(c(theme::SELECT)))
                            .hover(|d| d.bg(gpui::white().opacity(0.1)))
                            .child(dot(theme::project_color(&s.cwd), 7.0))
                            .when(cell_w >= 120.0, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_color(c(theme::SUBTLE))
                                        .child(local(start).format("%H:%M").to_string()),
                                )
                            })
                            .child(div().flex_1().min_w_0().truncate().child(info.title.clone()))
                            .when(info.live, |d| d.child(dot(theme::LIVE, 6.0)))
                            .tooltip(tip(
                                format!("{} · {}", self.project_name(&s.cwd), info.title),
                                None,
                            ))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                cx.stop_propagation();
                                if let Some(sel) = sel {
                                    t.navigate(sel, w, cx);
                                }
                            })),
                    );
                }
                if shown < day.sessions {
                    cell = cell.child(
                        div()
                            .px_1()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(format!("ほか {} 件", day.sessions - shown)),
                    );
                }
                row = row.child(cell);
            }
            grid = grid.child(row);
        }
        let mut view = div().relative().flex().flex_col().w(px(total_w)).child(head).child(grid);
        if layout.empty {
            view = view.child(self.empty_state(cx));
        }
        view.into_any_element()
    }

    /// 下端の凡例。期間内のプロジェクトを稼働時間の長い順に、色と名前で示す
    pub(super) fn calendar_legend(&self, room: f32) -> Vec<gpui::Div> {
        let Some(layout) = self.calendar_cache.current() else { return Vec::new() };
        let mut room = room;
        let mut out = Vec::new();
        for (cwd, secs) in &layout.projects {
            let name = self.project_name(cwd);
            let label = format!("{name} {}", fmt_hm(*secs));
            let need = 14.0 + 4.0 + label.chars().map(|ch| if ch.is_ascii() { 6.5 } else { 11.5 }).sum::<f32>() + 12.0;
            if room < need {
                break;
            }
            room -= need;
            out.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(project_tile(cwd, 12.0))
                    .child(name)
                    .child(div().text_color(c(theme::SUBTLE)).child(fmt_hm(*secs))),
            );
        }
        out
    }

    /// 各トラックのグループ（tracks の位置で表した代表）。別セッションとして起動された子は起動元のグループに入れる
    fn group_of(&self, tracks: &[Track]) -> Vec<usize> {
        let mut first: HashMap<usize, usize> = HashMap::new();
        let mut out = Vec::with_capacity(tracks.len());
        for (ti, t) in tracks.iter().enumerate() {
            let root = t.link.and_then(|(ps, _, _)| first.get(&ps).map(|&g| out[g])).unwrap_or(ti);
            let g = *first.entry(t.session).or_insert(ti);
            out.push(if g == ti { root } else { out[g] });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(session: usize, lane: usize) -> Track {
        Track { session, lane, depth: 0, link: None, active: false }
    }

    fn key() -> Key {
        Key { first: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(), days: 7, generation: 1 }
    }

    fn seg(start: f64, end: f64, lanes: Vec<usize>) -> GroupSeg {
        GroupSeg { start, end, lanes, session: 0 }
    }

    #[test]
    fn packs_non_overlapping_into_one_column() {
        let r = pack_columns(&[(0.0, 100.0), (200.0, 300.0), (50.0, 150.0)], 1.0);
        assert_eq!(r, vec![(0, 1, 2), (0, 1, 1), (1, 1, 2)]);
        // 右の列が空いている時間帯のものは広げる
        let r = pack_columns(&[(0.0, 300.0), (0.0, 100.0), (50.0, 150.0), (200.0, 260.0)], 1.0);
        assert_eq!(r, vec![(0, 1, 3), (1, 1, 3), (2, 1, 3), (1, 2, 3)]);
    }

    #[test]
    fn groups_join_lanes_of_the_same_group_and_split_on_idle() {
        let groups = group_segments(
            &[(0, 100.0, 200.0), (1, 120.0, 300.0), (0, 1500.0, 1600.0), (0, 3000.0, 3100.0), (2, 90.0, 180.0)],
            &[0, 0, 2],
        );
        let spans: Vec<_> = groups.iter().map(|g| (g.start, g.end, g.lanes.clone())).collect();
        assert_eq!(spans, vec![(100.0, 300.0, vec![0, 1]), (1500.0, 1600.0, vec![0]), (3000.0, 3100.0, vec![0]), (90.0, 180.0, vec![2])]);
    }

    #[test]
    fn day_layout_counts_union_of_active_time_and_root_sessions() {
        let tracks = Rc::new(vec![track(0, 0), track(0, 1), track(1, 0)]);
        let day = DayLayout::new(
            tracks,
            vec![seg(100.0, 200.0, vec![0, 1]), seg(150.0, 400.0, vec![2]), seg(1000.0, 1100.0, vec![0])],
        );
        assert_eq!(day.active, 400.0);
        assert_eq!(day.sessions, 2);
        assert_eq!(day.groups.iter().map(|g| g.session).collect::<Vec<_>>(), vec![0, 1, 0]);
    }

    #[test]
    fn day_layout_counts_full_representatives_with_aliasing_bits_and_repeated_lanes() {
        for indices in [[0, 1, 2, 3, 4, 5, 0], [0, 64, usize::MAX, 63, 128, usize::MAX - 64, 0]] {
            let tracks: Rc<Vec<Track>> = Rc::new(indices.into_iter().enumerate().map(|(i, session)| track(session, usize::from(i == 6))).collect());
            for order in [[0, 6, 1, 2, 3, 4, 5], [5, 4, 3, 2, 1, 6, 0], [0, 5, 3, 1, 6, 4, 2]] {
                for length in [0, 1, 7, 64, 129, 257] {
                    let groups = (0..length).map(|i| {
                        let lane = order[i % order.len()];
                        seg(i as f64 * 10.0, i as f64 * 10.0 + 5.0, if lane == 0 { vec![0, 2] } else { vec![lane] })
                    }).collect();
                    let day = DayLayout::new(tracks.clone(), groups);
                    assert_eq!(day.sessions, if length == 0 { 0 } else if length == 1 { 1 } else { 6 });
                    assert_eq!(day.active.to_bits(), (length as f64 * 5.0).to_bits());
                    assert!(Rc::ptr_eq(&day.tracks, &tracks));
                    for (i, group) in day.groups.iter().enumerate() {
                        assert_eq!(group.session, indices[order[i % order.len()]]);
                        assert_eq!(group.start.to_bits(), (i as f64 * 10.0).to_bits());
                        assert_eq!(group.end.to_bits(), (i as f64 * 10.0 + 5.0).to_bits());
                    }
                }
            }
        }
    }

    #[test]
    fn month_sessions_keep_first_interval_order_and_selection_when_clipped() {
        let day = DayLayout::new(
            Rc::new(vec![track(9, 0), track(7, 0), track(9, 1), track(usize::MAX, 0)]),
            vec![
                seg(100.0, 200.0, vec![0]), seg(50.0, 100.0, vec![1]),
                seg(300.0, 350.0, vec![2]), seg(500.0, 600.0, vec![3]), seg(100.0, 180.0, vec![1]),
            ],
        );
        assert_eq!(day.sessions, 3);
        assert!(day.month_sessions(0).next().is_none());
        let expected = [(7, 50.0_f64, 100.0_f64, vec![1]), (9, 100.0, 200.0, vec![0]), (usize::MAX, 500.0, 600.0, vec![3])];
        for limit in [1, 2, 3, usize::MAX] {
            let actual: Vec<_> = day.month_sessions(limit).map(|g| (g.session, g.start.to_bits(), g.end.to_bits(), g.lanes.clone())).collect();
            let expected: Vec<_> = expected.iter().take(limit).map(|(si, start, end, lanes)| (*si, start.to_bits(), end.to_bits(), lanes.clone())).collect();
            assert_eq!(actual, expected);
        }
        let first = day.month_sessions(2).collect::<Vec<_>>();
        assert!(std::ptr::eq(first[0], &day.groups[0]));
        assert!(std::ptr::eq(first[1], &day.groups[2]));
    }

    #[test]
    fn layout_reuses_only_the_same_dates_and_data() {
        let mut cache = Cache::default();
        let key = key();
        let layout = cache.set(key, vec![DayLayout::new(Rc::new(vec![track(0, 0)]), vec![])], |_| "");
        assert!(Rc::ptr_eq(&layout, &cache.get(key).unwrap()));
        assert!(std::mem::take(&mut cache.seek), "新しい期間では作業のある時刻を探す");
        for changed in [
            Key { first: key.first + Duration::days(7), ..key },
            Key { days: 1, ..key },
            Key { generation: key.generation + 1, ..key },
        ] {
            assert!(cache.get(changed).is_none());
        }
        cache.set(Key { generation: 2, ..key }, Vec::new(), |_| "");
        assert!(!cache.seek, "同じ期間のデータ更新ではスクロールを動かさない");
        let old = Rc::downgrade(&layout);
        drop(layout);
        cache.set(Key { days: 1, ..key }, Vec::new(), |_| "");
        assert!(old.upgrade().is_none(), "以前の期間を保持し続けない");
    }

    #[test]
    fn counts_and_legend_cover_only_sessions_with_activity() {
        let mut cache = Cache::default();
        let tracks = Rc::new(vec![track(0, 0), track(1, 0), track(2, 0)]);
        let layout = cache.set(
            key(),
            vec![
                DayLayout::new(tracks.clone(), vec![seg(0.0, 3600.0, vec![0]), seg(1800.0, 5400.0, vec![1])]),
                DayLayout::new(tracks, vec![seg(0.0, 600.0, vec![0])]),
            ],
            |si| if si == 1 { "/b" } else { "/a" },
        );
        assert_eq!(layout.counts, "2 セッション · 稼働 1h40m");
        assert_eq!(layout.projects, vec![("/a".to_string(), 4200.0), ("/b".to_string(), 3600.0)]);
        assert!(!layout.empty);
        assert!(cache.set(key(), Vec::new(), |_| "").empty);
    }

    #[test]
    fn counts_keep_distinct_high_indices_and_merge_repeated_lanes_and_days() {
        let tracks = Rc::new(vec![
            track(0, 0), track(1, 0), track(1, 1),
            track(usize::MAX / 2 + 1, 0), track(usize::MAX - 1, 0), track(usize::MAX, 0),
        ]);
        let days = (0..2).map(|_| {
            let groups = (0..tracks.len()).map(|i| seg(i as f64 * 120.0, i as f64 * 120.0 + 60.0, vec![i])).collect();
            DayLayout::new(tracks.clone(), groups)
        }).collect();
        let layout = Cache::default().set(key(), days, |_| "/all");
        assert_eq!(layout.counts, "5 セッション · 稼働 12m");
        assert_eq!(layout.projects, vec![("/all".to_string(), 720.0)]);
        assert!(layout.days.iter().all(|day| day.sessions == 5));
        assert!(!layout.empty);
    }

    #[test]
    fn zoom_reuses_segments_and_recalculates_columns_with_minimum_height() {
        let mut cache = Cache::default();
        let tracks = Rc::new(vec![track(0, 0), track(1, 0)]);
        let layout = cache.set(
            key(),
            vec![DayLayout::new(tracks, vec![seg(100.0, 200.0, vec![0]), seg(300.0, 400.0, vec![1])])],
            |_| "",
        );
        assert_eq!(cache.columns(1.0)[0], vec![(0, 1, 1), (0, 1, 1)]);
        // 縮小すると最小の高さで重なるので横に並べる
        assert_eq!(cache.columns(10.0)[0], vec![(0, 1, 2), (1, 1, 2)]);
        assert!(Rc::ptr_eq(&layout, &cache.get(key()).unwrap()));
    }

    #[test]
    fn legend_owns_names_and_preserves_equal_names_ties_and_addition_order() {
        let mut names = vec!["/b".to_string(), "/b".to_string(), "/a".to_string()];
        assert_ne!(names[0].as_ptr(), names[1].as_ptr());
        let tracks = Rc::new(vec![track(0, 0), track(1, 0), track(2, 0)]);
        let expected = (0.1_f64 + 0.2) + 0.4;
        let layout = Cache::default().set(
            key(),
            vec![
                DayLayout::new(tracks.clone(), vec![seg(0.0, 0.1, vec![0]), seg(0.0, expected, vec![2])]),
                DayLayout::new(tracks.clone(), vec![seg(0.0, 0.2, vec![1])]),
                DayLayout::new(tracks, vec![seg(0.0, 0.4, vec![0])]),
            ],
            |si| names[si].as_str(),
        );
        names.clear();
        assert_eq!(layout.projects.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), vec!["/a", "/b"]);
        assert!(layout.projects.iter().all(|(_, active)| active.to_bits() == expected.to_bits()));
        assert_ne!(expected.to_bits(), (0.1_f64 + 0.4 + 0.2).to_bits());
    }

    #[test]
    fn month_grid_starts_on_the_monday_before_the_first() {
        let d = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        assert_eq!(month_grid_start(d(10, 17)), d(9, 28));
        assert_eq!(month_grid_start(d(6, 30)), d(6, 1));
        assert_eq!(fmt_hm(45.0 * 60.0), "45m");
        assert_eq!(fmt_hm(3.0 * 3600.0 + 12.0 * 60.0), "3h12m");
    }

    #[test]
    fn daily_segments_preserve_midnight_boundaries_overlaps_and_idle_gaps() {
        use crate::model::{Event, Lane, Session, Source};
        let mut lane = Lane::new("main");
        for (start, end) in
            [(-100.0, 0.0), (100.0, 200.0), (2000.0, 90_000.0), (2500.0, 2600.0), (86_400.0, 87_000.0), (200_000.0, 200_100.0)]
        {
            lane.events.push(Event {
                start,
                end,
                kind: Kind::Tool,
                name: "exec_command".into(),
                summary: Default::default(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: Default::default(),
                usage: Default::default(),
                model: None,
                status: Default::default(),
            });
        }
        let mut session = Session {
            id: String::new(),
            source: Source::Codex,
            cwd: String::new(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![lane, Lane::new("empty")],
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        session.finalize();
        let sessions = [std::sync::Arc::new(session)];
        let tracks = [track(0, 0), track(0, 1)];
        assert_eq!(day_segments(&sessions, &tracks, 0.0, 86_400.0), [(0, 0.0, 200.0), (0, 2000.0, 86_400.0)]);
        assert_eq!(day_segments(&sessions, &tracks, 86_400.0, 172_800.0), [(0, 86_400.0, 90_000.0)]);
        assert!(day_segments(&sessions, &tracks, 200_100.1, 300_000.0).is_empty());
        assert!(day_segments(&sessions, &tracks, -1000.0, -100.0).is_empty());
        assert_eq!(day_segments(&sessions, &tracks, 200_100.0, 300_000.0), [(0, 200_100.0, 200_100.0)]);
    }

    #[test]
    fn track_day_ranges_preserve_direct_boundary_predicates() {
        let ranges = [
            None, Some((-10.0, 0.0)), Some((0.0, 0.0)), Some((0.0, 10.0)),
            Some((10.0, 10.0)), Some((10.0, 20.0)), Some((20.0, 30.0)), Some((30.0, 40.0)),
            Some((-10.0, 40.0)), Some((20.0, 10.0)), Some((f64::NEG_INFINITY, f64::INFINITY)),
            Some((f64::INFINITY, f64::INFINITY)), Some((f64::NEG_INFINITY, f64::NEG_INFINITY)),
            Some((f64::NAN, 10.0)), Some((10.0, f64::NAN)), Some((f64::NAN, f64::NAN)),
        ];
        for edges in [
            vec![], vec![0.0], vec![0.0, 10.0, 20.0, 30.0], vec![0.0, 10.0, 10.0, 20.0],
            vec![20.0, 10.0, 0.0], vec![10.0, 0.0, 30.0], vec![0.0, f64::NAN, 20.0],
            vec![f64::NEG_INFINITY, 0.0, f64::INFINITY],
        ] {
            let spans = track_days(&edges, ranges.iter().copied());
            let sparse = sparse_track_days(&edges, ranges.iter().copied());
            let ordered = edges.windows(2).all(|pair| pair[0] <= pair[1]);
            for (ti, &range) in ranges.iter().enumerate() {
                let (start, end) = spans[ti];
                assert!(start <= end && end <= edges.len().saturating_sub(1));
                for (day, pair) in edges.windows(2).enumerate() {
                    let expected = range.is_some_and(|(a, b)| !(b < pair[0] || a >= pair[1]));
                    let selected = start <= day && day < end;
                    if ordered { assert_eq!(selected, expected, "{ti} {day} {edges:?}"); }
                    else { assert!(!expected || selected); }
                    let filtered = sparse.as_ref().is_none_or(|spans| {
                        let (start, end) = spans[ti]; start <= day && day < end
                    });
                    assert_eq!(filtered && expected, expected);
                }
            }
        }
        assert!(sparse_track_days(&[0.0, 10.0, 20.0], [Some((-1.0, 30.0)); 8].into_iter()).is_none());
        assert!(sparse_track_days(&[0.0, 10.0, 20.0, 30.0], [Some((1.0, 2.0)), Some((21.0, 22.0))].into_iter()).is_some());
    }

    #[test]
    fn selected_day_tracks_preserve_indices_midnight_points_and_sparse_events() {
        use crate::model::{Event, Lane, Session, Source};
        let lane = |name, ranges: &[(f64, f64)]| {
            let mut lane = Lane::new(name);
            for &(start, end) in ranges {
                lane.events.push(Event {
                    start, end, kind: Kind::Tool, name: "exec_command".into(), summary: Default::default(),
                    input: String::new(), output: String::new(), call_id: None, text_ref: Default::default(),
                    usage: Default::default(), model: None, status: Default::default(),
                });
            }
            lane
        };
        let mut session = Session {
            id: String::new(), source: Source::Codex, cwd: String::new(), headless: false,
            title: String::new(), branch: String::new(), version: String::new(),
            lanes: vec![
                lane("main", &[(-100.0, 0.0), (100.0, 200.0), (2000.0, 90_000.0), (2500.0, 2600.0), (86_400.0, 87_000.0), (200_000.0, 200_100.0)]),
                lane("point", &[(86_400.0, 86_400.0), (300_000.0, 350_000.0)]),
                lane("empty", &[]),
            ],
            start: 0.0, end: 0.0, parents: Vec::new(), tree: Vec::new(),
        };
        session.finalize();
        let sessions = [std::sync::Arc::new(session)];
        let tracks = [track(0, 2), track(0, 1), track(0, 0)];
        let edges = [0.0, 86_400.0, 172_800.0, 259_200.0, 345_600.0, 432_000.0];
        let spans = track_days(&edges, tracks.iter().map(|t| sessions[t.session].lanes[t.lane].range()));
        let bits = |segments: Vec<(usize, f64, f64)>| segments.into_iter().map(|(ti, a, b)| (ti, a.to_bits(), b.to_bits())).collect::<Vec<_>>();
        for (day, pair) in edges.windows(2).enumerate() {
            let selected = spans.iter().enumerate().filter_map(|(ti, &(start, end))| (start <= day && day < end).then_some(ti));
            assert_eq!(bits(indexed_segments(&sessions, &tracks, selected, pair[0], pair[1])), bits(day_segments(&sessions, &tracks, pair[0], pair[1])));
        }
        assert_eq!(day_segments(&sessions, &tracks, 86_400.0, 172_800.0), [(1, 86_400.0, 86_400.0), (2, 86_400.0, 90_000.0)]);
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::segment_bounds;
    use crate::model::{Event, Kind, Lane, Session, Source};

    fn lane(ranges: &[(f64, f64)]) -> Lane {
        let mut lane = Lane::new("main");
        lane.events = ranges.iter().map(|&(start, end)| Event {
            start, end, kind: Kind::Tool, name: "exec_command".into(), summary: Default::default(),
            input: String::new(), output: String::new(), call_id: None, text_ref: Default::default(),
            usage: Default::default(), model: None, status: Default::default(),
        }).collect();
        lane.finish();
        let mut session = Session {
            id: String::new(), source: Source::Codex, cwd: String::new(), headless: false,
            title: String::new(), branch: String::new(), version: String::new(), lanes: vec![lane],
            start: 0.0, end: 0.0, parents: Vec::new(), tree: Vec::new(),
        };
        session.finalize();
        session.lanes.pop().unwrap()
    }

    #[test]
    fn bounds_keep_overlaps_inclusive_ends_and_exclusive_starts() {
        let lane = lane(&[(-5.0, 0.0), (0.0, 20.0), (10.0, 10.0), (30.0, 31.0)]);
        for (ds, de, expected) in [
            (0.0, 40.0, (0, 4)), (0.0, 30.0, (0, 3)), (1.0, 11.0, (1, 3)),
            (20.0, 30.0, (1, 3)), (21.0, 30.0, (3, 3)), (31.0, 40.0, (3, 4)),
            (32.0, 40.0, (4, 4)), (f64::NEG_INFINITY, f64::INFINITY, (0, 4)),
            (f64::NAN, 100.0, (0, 4)), (20.0, f64::NAN, (1, 0)), (40.0, 0.0, (4, 1)),
        ] {
            assert_eq!(segment_bounds(&lane, ds, de), expected, "{ds:?} {de:?}");
        }
        assert_eq!(segment_bounds(&Lane::new("empty"), 0.0, 1.0), (0, 0));
    }

    #[test]
    fn bounds_keep_original_search_for_nan_starts_in_sorted_events() {
        for ranges in [
            vec![(-f64::NAN, f64::NAN), (-f64::NAN, f64::NAN), (-f64::NAN, f64::NAN), (1.0, 2.0)],
            vec![(0.0, 1.0), (f64::NAN, f64::NAN)],
            vec![(-f64::NAN, 2.0), (0.0, 3.0), (f64::NAN, 4.0)],
        ] {
            let lane = lane(&ranges);
            for (ds, de) in [(0.0, 10.0), (-1.0, 1.0), (f64::NEG_INFINITY, f64::INFINITY), (f64::NAN, f64::NAN)] {
                let expected = (lane.cum_end.partition_point(|&end| end < ds), lane.events.partition_point(|e| e.start < de));
                assert_eq!(segment_bounds(&lane, ds, de), expected);
            }
        }
    }
}

#[cfg(test)]
mod group_sort_tests {
    use super::group_segments;

    #[test]
    fn groups_keep_key_order_signed_zero_idle_boundary_and_unique_sorted_lanes() {
        let segments = [(2,100.0,200.0),(0,0.0,5.0),(1,5.0,10.0),(0,900.0,910.0),(0,5000.0,5010.0),(1,-0.0,20.0)];
        let grouped=group_segments(&segments,&[17,17,0]);
        let actual:Vec<_>=grouped.iter().map(|g|(g.start.to_bits(),g.end.to_bits(),g.lanes.clone())).collect();
        assert_eq!(actual,vec![(100.0_f64.to_bits(),200.0_f64.to_bits(),vec![2]),
            ((-0.0_f64).to_bits(),910.0_f64.to_bits(),vec![0,1]),(5000.0_f64.to_bits(),5010.0_f64.to_bits(),vec![0])]);
        assert!(grouped.iter().all(|g|g.session==0));
        assert!(group_segments(&[],&[]).is_empty());
    }

    #[test]
    fn large_few_group_inputs_keep_ordered_and_reversed_segments_with_high_keys() {
        let group=[usize::MAX;256];
        let mut segments:Vec<_>=(0..256).map(|ti|(ti,ti as f64*1000.0,ti as f64*1000.0+10.0)).collect();
        for reverse in [false,true] {
            if reverse {segments.reverse();}
            let result=group_segments(&segments,&group);assert_eq!(result.len(),256);
            for (ti,g) in result.iter().enumerate() {
                assert_eq!((g.start,g.end,g.session),(ti as f64*1000.0,ti as f64*1000.0+10.0,0));
                assert_eq!(g.lanes,[ti]);
            }
        }
    }
}

#[cfg(test)]
mod lane_membership_tests {
    use super::group_segments;

    #[test]
    fn grouped_lanes_keep_colliding_indices_and_remove_repeats() {
        for count in [1usize, 31, 63, 64, 65, 257] {
            let lanes: Vec<_> = (0..count).map(|i| i * 64).collect();
            let group = vec![usize::MAX; lanes.last().copied().unwrap() + 1];
            let segments: Vec<_> = lanes.iter().rev().flat_map(|&ti| [(ti, 0.0, 10.0), (ti, 0.0, 10.0)]).collect();
            let result = group_segments(&segments, &group);
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].lanes, lanes);
            assert_eq!((result[0].start, result[0].end, result[0].session), (0.0, 10.0, 0));
        }
    }

    #[test]
    fn grouped_lanes_reset_membership_at_idle_gaps_and_group_boundaries() {
        for keys in [1usize, 8, 17] {
            for count in [63usize, 64, 65, 257] {
                let group: Vec<_> = (0..count * keys * 64).map(|i| (i / 64) % keys).collect();
                let mut segments = Vec::new();
                for start in [0.0, 1010.0] {
                    for i in (0..count * keys).rev() {
                        segments.extend([(i * 64, start, start + 10.0); 2]);
                    }
                }
                let result = group_segments(&segments, &group);
                assert_eq!(result.len(), keys * 2);
                for key in 0..keys {
                    let lanes: Vec<_> = (0..count).map(|i| (i * keys + key) * 64).collect();
                    for (phase, start) in [0.0, 1010.0].into_iter().enumerate() {
                        let g = &result[key * 2 + phase];
                        assert_eq!(g.lanes, lanes, "{keys} {count} {key} {phase}");
                        assert_eq!((g.start, g.end, g.session), (start, start + 10.0, 0));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod column_index_tests {
    use super::pack_columns;
fn pack_columns_reference(spans: &[(f64, f64)], spp: f64) -> Vec<(usize, usize, usize)> {
    let pad = 8.0 * spp;
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by(|&a, &b| spans[a].0.total_cmp(&spans[b].0).then(a.cmp(&b)));
    let mut out = vec![(0, 1, 1); spans.len()];
    let mut i = 0;
    while i < order.len() {
        // 重なりの連鎖（クラスタ）ごとに列を割り当てる
        let mut cols: Vec<f64> = Vec::new();
        let mut members = Vec::new();
        let mut cluster_end = f64::MIN;
        while i < order.len() {
            let k = order[i];
            let (s, e) = spans[k];
            if !members.is_empty() && s >= cluster_end + pad {
                break;
            }
            let e = e.max(s + pad);
            let col = match cols.iter().position(|&ce| ce + pad <= s) {
                Some(c) => {
                    cols[c] = e;
                    c
                }
                None => {
                    cols.push(e);
                    cols.len() - 1
                }
            };
            out[k].0 = col;
            members.push(k);
            cluster_end = cluster_end.max(e);
            i += 1;
        }
        let n = cols.len();
        for &k in &members {
            out[k].2 = n;
        }
        for &k in &members {
            let (s, e) = (spans[k].0, spans[k].1.max(spans[k].0 + pad));
            let blocked = |c: usize| {
                members.iter().any(|&o| {
                    o != k && out[o].0 == c && spans[o].0 < e + pad && s < spans[o].1.max(spans[o].0 + pad) + pad
                })
            };
            out[k].1 = (out[k].0 + 1..n).take_while(|&c| !blocked(c)).count() + 1;
        }
    }
    out
}

    #[test]
    fn indexed_columns_choose_the_leftmost_free_column_at_the_exact_end() {
        let mut spans: Vec<_> = (0..64).map(|i| (0.0, if i == 0 {10000.0} else {1000.0-i as f64})).collect();
        spans.extend([(1100.0,1101.0),(1110.0,1111.0),(1116.0,1117.0)]);
        let actual=pack_columns(&spans,1.0);
        assert_eq!(actual,pack_columns_reference(&spans,1.0));
        assert_eq!(&actual[64..],&[(1,1,64),(2,62,64),(1,1,64)]);
    }

    #[test]
    fn indexed_columns_keep_nan_infinity_signed_zero_and_unusual_padding() {
        for count in [0usize,1,31,32,33,63,64,65,128] {
            for spp in [-1.0,-0.0,0.0,0.125,1.0,f64::INFINITY,f64::NEG_INFINITY,f64::NAN] {
                for shape in 0..4 {
                    let spans:Vec<_>=(0..count).map(|i| {
                        let s=match shape {0=>if i%2==0 {-0.0} else {0.0},1=>if i%5==0 {-f64::NAN} else {i as f64},2=>if i%5==0 {f64::NAN} else {i as f64},_=>if i%2==0 {f64::NEG_INFINITY} else {f64::INFINITY}};
                        let e=if i%3==0 {f64::NAN} else {s+50.0};(s,e)
                    }).collect();
                    assert_eq!(pack_columns(&spans,spp),pack_columns_reference(&spans,spp),"{count} {spp} {shape}");
                }
            }
        }
    }

    #[test]
    fn indexed_columns_reset_all_working_ranges_between_clusters() {
        let mut spans=vec![(0.0,10.0);64];
        spans.extend((1..=64).map(|i|(i as f64*1000.0,i as f64*1000.0+10.0)));
        spans.extend([(1000000.0,1000010.0);128]);
        let actual=pack_columns(&spans,0.125);
        assert_eq!(actual,pack_columns_reference(&spans,0.125));
        for (i,&v) in actual[..64].iter().enumerate() {assert_eq!(v,(i,1,64));}
        assert!(actual[64..128].iter().all(|&v|v==(0,1,1)));
        for (i,&v) in actual[128..].iter().enumerate() {assert_eq!(v,(i,1,128));}
    }
    #[test]
    fn padded_disjoint_columns_keep_exact_boundaries_and_clamped_ends() {
        for count in [2usize, 63, 64, 65] {
            let spans: Vec<_> = (0..count).map(|i| {
                let start = i as f64 * 16.0;
                let end = if i + 1 == count { f64::INFINITY }
                    else if i % 3 == 0 { f64::NAN }
                    else { start - 10.0 };
                (start, end)
            }).collect();
            let actual = pack_columns(&spans, 1.0);
            assert_eq!(actual, vec![(0, 1, 1); count]);
            assert_eq!(actual, pack_columns_reference(&spans, 1.0));
        }
        let spans = [(0.0, 8.0), (16.0, 16.0)];
        assert_eq!(pack_columns(&spans, 1.0), vec![(0, 1, 1); 2]);
        let spans = [(0.0, 8.5), (16.0, 16.0)];
        assert_eq!(pack_columns(&spans, 1.0), vec![(0, 1, 2), (1, 1, 2)]);
        assert_eq!(pack_columns(&spans, 1.0), pack_columns_reference(&spans, 1.0));
    }

    #[test]
    fn wide_columns_keep_strict_overlap_boundaries_after_switching_queries() {
        let mut spans = vec![(0.0, 5.0); 128];
        spans[0].1 = 1e9;
        spans.extend((0..256).map(|i| {
            let start = 100.0 + i as f64 * 10.0;
            (start, start + 1.0)
        }));
        let start = 2660.0;
        spans.extend([
            (start, start), (start, start + 1.0),
            (start + 2.0, start + 3.0),
            (start + 4.0, 1e8), (start + 4.0, start + 5.0),
        ]);
        spans.extend((0..512).map(|i| {
            let start = 500000.0 + i as f64 * 10.0;
            (start, start + 1.0)
        }));
        // The empty columns exhaust the scan budget before record 386.
        let actual = pack_columns(&spans, 0.125);
        assert_eq!(actual, pack_columns_reference(&spans, 0.125));
        // Both the old padded end and the future start touch this record.
        assert_eq!(actual[386], (1, 127, 128));
        spans.reverse();
        assert_eq!(pack_columns(&spans, 0.125), pack_columns_reference(&spans, 0.125));
    }

    #[test]
    fn wide_column_queries_reset_the_tree_between_different_cluster_sizes() {
        let mut spans = Vec::new();
        for (start, columns) in [(0.0, 64), (1000000.0, 129)] {
            let first = spans.len();
            spans.extend(std::iter::repeat_n((start, start + 5.0), columns));
            spans[first].1 = start + 100000.0;
            spans.extend((0..256).map(|i| {
                let s = start + 100.0 + i as f64 * 10.0;
                (s, s + if columns == 129 && i % 7 == 0 { 1000.0 } else { 1.0 })
            }));
        }
        spans.extend([(2000000.0, 2000001.0); 4]);
        for reversed in [false, true] {
            if reversed { spans.reverse(); }
            assert_eq!(pack_columns(&spans, 0.125), pack_columns_reference(&spans, 0.125));
        }
    }

    #[test]
    fn equal_starts_only_skip_expansion_when_the_minimum_height_overlaps() {
        let spans: Vec<_> = (0..192).map(|i| {
            (if i % 2 == 0 { -0.0 } else { 0.0 }, if i % 3 == 0 { 100.0 } else { 0.0 })
        }).collect();
        for spp in [-0.0, 0.0, 0.125, 1.0] {
            let actual = pack_columns(&spans, spp);
            assert_eq!(actual, pack_columns_reference(&spans, spp));
            if spp > 0.0 { assert!(actual.iter().all(|&(_, width, columns)| width == 1 && columns == spans.len())); }
            else { assert!(actual.iter().any(|&(_, width, _)| width > 1)); }
        }
        // A positive pad can round away at a large finite start.
        let spans: Vec<_> = (0..192).map(|i| (1e300, if i % 3 == 0 { 2e300 } else { 1e300 })).collect();
        for spp in [0.125, 1.0] {
            let actual = pack_columns(&spans, spp);
            assert_eq!(actual, pack_columns_reference(&spans, spp));
            assert!(actual.iter().any(|&(_, width, _)| width > 1));
        }
    }

    #[test]
    fn narrow_columns_keep_widths_when_ends_are_ordered_and_clusters_reset() {
        for count in [127usize, 128, 512] {
            let mut spans = Vec::new();
            for (base, columns) in [(0.0, 7), (1000000.0, 2), (2000000.0, 31)] {
                spans.extend((0..count).map(|i| {
                    let start = base + i as f64 * 4.0;
                    let duration = 5.0f64.max((columns - 1) as f64 * 4.0 + 1.0 - i as f64 * 0.25);
                    (start, start + duration)
                }));
            }
            for spp in [0.0, 0.125, 1.0] {
                let actual = pack_columns(&spans, spp);
                assert_eq!(actual, pack_columns_reference(&spans, spp), "{count} {spp}");
                if count == 512 && spp == 0.125 {
                    assert!(actual[..count].iter().any(|&(_, width, _)| width > 1));
                }
            }
            spans.reverse();
            assert_eq!(pack_columns(&spans, 0.125), pack_columns_reference(&spans, 0.125));
        }
        let mut ordered: Vec<_> = (0..4096).map(|i| {
            let start = i as f64 * 4.0;
            (start, start + 5.0f64.max(25.0 - i as f64 * 0.25))
        }).collect();
        for reversed in [false, true] {
            if reversed { ordered.reverse(); }
            let actual = pack_columns(&ordered, 0.125);
            assert_eq!(actual, pack_columns_reference(&ordered, 0.125));
            assert!(actual.iter().any(|&(_, width, _)| width > 1));
        }
        // Unordered effective ends must retain the indexed lookup.
        let mut spans: Vec<_> = (0..512).map(|i| {
            let start = i as f64 * 10.0;
            (start, start + if i % 7 == 0 { 100.0 } else { 15.0 })
        }).collect();
        for reversed in [false, true] {
            if reversed { spans.reverse(); }
            assert_eq!(pack_columns(&spans, 0.125), pack_columns_reference(&spans, 0.125));
        }
    }

}
