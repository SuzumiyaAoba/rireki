//! カレンダー（日 / 週）。縦軸が時刻で、日ごとの列にその日のレーンを並べる

use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{AnyElement, ClickEvent, Context, ElementId, FontWeight, Window, div, prelude::*, px};

use super::header::weekday_ja;
use super::widgets::tip;
use super::{AXIS_H, BlockPaint, HOUR_LABEL_W, MIN_BLOCK, Rireki, Selection, Track, midnight, now};
use std::collections::HashMap;
use std::rc::Rc;

use crate::prefs::Mode;
use crate::theme::{self, c};

/// 00:00 / 24:00 のラベルが上下のバーに隠れないよう、日付の境界に余白を残す。
const TIME_EDGE_PAD: f32 = 16.0;

pub(super) fn clamp_scroll(scroll: f32, px_per_hour: f32, height: f32) -> f32 {
    let min = -TIME_EDGE_PAD;
    let max = (24.0 * px_per_hour - height + TIME_EDGE_PAD).max(min);
    scroll.clamp(min, max)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    first: NaiveDate,
    days: usize,
    generation: u64,
    in_view_only: bool,
}

/// 表示中の日・週だけ保持する。スクロール・選択・幅変更で全イベントを走査し直さない。
#[derive(Default)]
pub(super) struct Cache {
    layout: Option<(Key, Rc<CalendarLayout>)>,
    columns: Option<(f64, Rc<Columns>)>,
}

type Columns = Vec<Vec<(usize, usize)>>;

struct CalendarLayout {
    days: Vec<DayLayout>,
    counts: String,
    empty: bool,
}

struct DayLayout {
    tracks: Rc<Vec<Track>>,
    groups: Vec<GroupSeg>,
    /// 各レーンの (グループ, グループ内の位置)。開始時刻順なので二分探索できる。
    lane_groups: Vec<Vec<(usize, usize)>>,
}

impl DayLayout {
    fn new(tracks: Rc<Vec<Track>>, groups: Vec<GroupSeg>) -> Self {
        let mut lane_groups = vec![Vec::new(); tracks.len()];
        for (k, g) in groups.iter().enumerate() {
            for (slot, &ti) in g.lanes.iter().enumerate() {
                lane_groups[ti].push((k, slot));
            }
        }
        Self { tracks, groups, lane_groups }
    }

    fn group_at(&self, track: usize, start: f64) -> Option<(usize, usize)> {
        let mine = &self.lane_groups[track];
        let i = mine.partition_point(|&(k, _)| self.groups[k].start <= start);
        mine.get(i.saturating_sub(1)).copied()
    }
}

impl Cache {
    fn get(&self, key: Key) -> Option<Rc<CalendarLayout>> {
        self.layout.as_ref().filter(|(k, _)| *k == key).map(|(_, layout)| layout.clone())
    }

    fn set(&mut self, key: Key, days: Vec<DayLayout>) -> Rc<CalendarLayout> {
        let mut sessions = std::collections::HashSet::new();
        let mut lanes = 0;
        let mut previous_tracks = None;
        for day in &days {
            lanes += day.tracks.len();
            if previous_tracks.is_some_and(|previous| Rc::ptr_eq(previous, &day.tracks)) {
                continue;
            }
            sessions.extend(day.tracks.iter().map(|t| t.session));
            previous_tracks = Some(&day.tracks);
        }
        let layout = Rc::new(CalendarLayout {
            days,
            counts: format!("{} セッション · {lanes} レーン", sessions.len()),
            empty: lanes == 0,
        });
        self.layout = Some((key, layout.clone()));
        self.columns = None;
        layout
    }

    fn columns(&mut self, spp: f64) -> Rc<Columns> {
        if let Some((scale, columns)) = &self.columns
            && *scale == spp
        {
            return columns.clone();
        }
        let layout = &self.layout.as_ref().unwrap().1;
        let columns: Rc<Columns> = Rc::new(layout.days.iter().map(|day| {
            let spans: Vec<_> = day.groups.iter()
                .map(|g| (g.start - GROUP_HEADER as f64 * spp, g.end)).collect();
            pack_columns(&spans, spp)
        }).collect());
        self.columns = Some((spp, columns.clone()));
        columns
    }
}

impl Rireki {
    fn calendar_layout(&mut self, first: NaiveDate, days: usize) -> Rc<CalendarLayout> {
        let key = Key { first, days, generation: self.data_gen, in_view_only: self.in_view_only };
        if let Some(layout) = self.calendar_cache.get(key) {
            return layout;
        }
        let started = self.profile.as_ref().map(|_| std::time::Instant::now());
        // 全レーン表示では日付で並びや親子関係が変わらないので、週全体で共有する。
        let all_tracks = (!self.in_view_only).then(|| {
            Rc::new(self.tracks_in(midnight(first), midnight(first + Duration::days(days as i64))))
        });
        let all_groups = all_tracks.as_ref().map(|tracks| self.group_of(tracks));
        let days = (0..days).map(|d| {
            let date = first + Duration::days(d as i64);
            let (ds, de) = (midnight(date), midnight(date + Duration::days(1)));
            let tracks = all_tracks.clone().unwrap_or_else(|| Rc::new(self.tracks_in(ds, de)));
            let segments = day_segments(&self.store.sessions, &tracks, ds, de);
            let group;
            let group = match &all_groups {
                Some(group) => group.as_slice(),
                None => {
                    group = self.group_of(&tracks);
                    &group
                }
            };
            let groups = group_segments(&segments, group);
            DayLayout::new(tracks, groups)
        }).collect();
        let layout = self.calendar_cache.set(key, days);
        if let Some(started) = started {
            println!("{}", serde_json::json!({"ui_profile":"calendar_layout", "days":key.days,
                "in_view_only":key.in_view_only, "tracks":layout.days.iter().map(|d| d.tracks.len()).sum::<usize>(),
                "segments":layout.days.iter().map(|d| d.groups.len()).sum::<usize>(),
                "ms":started.elapsed().as_secs_f64()*1000.0}));
        }
        layout
    }

    pub(super) fn calendar(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let week = self.mode == Mode::Week;
        let days = if week { 7 } else { 1 };
        let first = if week { self.week_start() } else { self.day0 };
        let total_w = self.main_width(window);
        let ph = self.plot_height(window);
        let col_w = (total_w - HOUR_LABEL_W) / days as f32;
        let pph = self.px_per_hour;
        let spp = 3600.0 / pph as f64;
        self.cal_scroll = clamp_scroll(self.cal_scroll, pph, ph);
        let scroll = self.cal_scroll;
        let today = Local::now().date_naive();
        let layout = self.calendar_layout(first, days);
        let columns = self.calendar_cache.columns(spp);

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
        let mut frames = Vec::new();
        for d in 0..days {
            let date = first + Duration::days(d as i64);
            let ds = midnight(date);
            let de = midnight(date + Duration::days(1));
            let x0 = HOUR_LABEL_W + d as f32 * col_w;
            let is_today = date == today;
            let weekend = matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
            header = header.child(
                div()
                    .id(ElementId::NamedInteger("day".into(), d as u64))
                    .w(px(col_w))
                    .flex()
                    .items_center()
                    .justify_center()
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
                            .min_w(px(22.0))
                            .h(px(22.0))
                            .px_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .when(is_today, |d| d.bg(c(theme::ACCENT)).text_color(gpui::white()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if week { date.day().to_string() } else { date.format("%-m/%-d").to_string() }),
                    )
                    .when(week, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(c(theme::HOVER)))
                            .tooltip(tip("この日を日表示で開く", Some("D")))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                t.day0 = date;
                                t.set_mode(Mode::Day, w);
                                cx.notify();
                            }))
                    }),
            );
            body = body.child(div().absolute().left(px(x0)).top_0().bottom_0().w(px(1.0)).bg(c(theme::BORDER)));
            if is_today {
                body = body.child(
                    div().absolute().left(px(x0)).w(px(col_w)).top_0().bottom_0().bg(c(theme::ACCENT).opacity(0.03)),
                );
            }

            let day = &layout.days[d];
            let tracks = &day.tracks;
            if tracks.is_empty() {
                continue;
            }
            let v0 = ds + scroll as f64 * spp;
            let v1 = v0 + ph as f64 * spp;
            // セッション（メイン + サブエージェント）を 1 つのグループにし、手の空いた時間で区切った
            // グループの区間どうしで重なるものだけを横に並べる。グループの中はレーンごとに分ける
            let groups = &day.groups;
            // 見出しの分だけ上に広げて詰める
            let cols = &columns[d];
            let gw_of = |k: usize| (col_w - 4.0) / cols[k].1 as f32;
            for (k, g) in groups.iter().enumerate() {
                let gw = gw_of(k);
                let gx = x0 + 2.0 + cols[k].0 as f32 * gw;
                let y = ((g.start - ds) / spp) as f32 - scroll - 2.0;
                let y2 = ((g.end - ds) / spp) as f32 - scroll + 2.0;
                if y2 < 0.0 || y > ph {
                    continue;
                }
                // 枠・見出し・左の色帯は Coding Agent（Claude / Codex）の色
                let sess = &self.store.sessions[tracks[g.lanes[0]].session];
                let (logo, color) = crate::icons::source_icon(sess.source);
                let fw = (gw - 2.0).max(3.0);
                let top = y - GROUP_HEADER;
                let h = (y2 - top).max(GROUP_HEADER + MIN_BLOCK);
                frames.push(
                    div()
                        .absolute()
                        .left(px(gx))
                        .top(px(top))
                        .w(px(fw))
                        .h(px(h))
                        .rounded(px(4.0))
                        .bg(c(color).opacity(0.07))
                        .border_1()
                        .border_color(c(color).opacity(0.45)),
                );
                frames.push(
                    div().absolute().left(px(gx)).top(px(top)).w(px(3.0)).h(px(h)).rounded_l(px(4.0)).bg(c(color)),
                );
                // 見出し: エージェントのロゴ・セッション名・サブエージェントの数
                let subs = g.lanes.iter().filter(|&&ti| tracks[ti].depth > 0 || tracks[ti].link.is_some()).count();
                let title = if sess.title.is_empty() { "(無題)" } else { sess.title.as_str() };
                frames.push(
                    div()
                        .absolute()
                        .left(px(gx + 3.0))
                        .top(px(top))
                        .w(px(fw - 3.0))
                        .h(px(GROUP_HEADER))
                        .rounded_tr(px(4.0))
                        .bg(c(color).opacity(0.18))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .overflow_hidden()
                        .text_xs()
                        .text_color(c(theme::TEXT))
                        .child(crate::icons::icon(logo, color))
                        .child(div().flex_1().min_w_0().truncate().child(crate::model::one_line(title, 80)))
                        .when(subs > 0 && fw > 90.0, |d| {
                            d.child(div().flex_none().text_color(c(theme::SUB)).child(format!("+{subs} サブ")))
                        }),
                );
                // サブエージェントのレーンは紫で塗り分け、区切り線を引く
                let lw = (gw - 2.0 - GROUP_INSET) / g.lanes.len() as f32;
                for (slot, &ti) in g.lanes.iter().enumerate() {
                    let sub = tracks[ti].depth > 0 || tracks[ti].link.is_some();
                    if !sub {
                        continue;
                    }
                    let lx = gx + GROUP_INSET + slot as f32 * lw - 1.0;
                    frames.push(
                        div()
                            .absolute()
                            .left(px(lx))
                            .top(px(y))
                            .w(px(lw))
                            .h(px((y2 - y).max(1.0)))
                            .bg(c(theme::SUB).opacity(0.07))
                            .border_l_1()
                            .border_color(c(theme::SUB).opacity(0.5)),
                    );
                }
            }
            for (ti, tr) in tracks.iter().enumerate() {
                if day.lane_groups[ti].is_empty() {
                    continue;
                }
                let lane = &self.store.sessions[tr.session].lanes[tr.lane];
                let view = (v0.max(ds), v1.min(de));
                let hi = lane.events.partition_point(|e| e.start <= view.1);
                if hi == 0 || lane.cum_end[hi - 1] < view.0 {
                    continue;
                }
                let grouped = self.layout_cache.borrow_mut().aggregate((tr.session, tr.lane), lane, view, spp, self.gran);
                for b in grouped {
                    let bs = b.start.max(ds);
                    let Some((k, slot)) = day.group_at(ti, bs) else {
                        continue;
                    };
                    let g = &groups[k];
                    let gw = gw_of(k);
                    let lw = (gw - 2.0 - GROUP_INSET) / g.lanes.len() as f32;
                    let x = x0 + 2.0 + cols[k].0 as f32 * gw + GROUP_INSET + slot as f32 * lw;
                    let y = ((bs - ds) / spp) as f32 - scroll;
                    let y2 = ((b.end.min(de) - ds) / spp) as f32 - scroll;
                    let sel = Selection { session: tr.session, lane: tr.lane, first: b.first, last: b.last };
                    let w = (lw - 2.0).max(2.0);
                    blocks.push(self.block_paint(sel, (x, y, w, (y2 - y).max(MIN_BLOCK)), true));
                }
            }
            if is_today {
                let y = ((now() - ds) / spp) as f32 - scroll;
                now_lines.push(div().absolute().left(px(x0)).w(px(col_w)).top(px(y)).h(px(2.0)).bg(c(theme::NOW)));
                now_lines.push(
                    div().absolute().left(px(x0 - 4.0)).top(px(y - 3.0)).size(px(8.0)).rounded_full().bg(c(theme::NOW)),
                );
            }
        }
        self.counts = layout.counts.clone();
        // 見えている時間帯に無くても、その日に作業があれば「なし」とは出さない
        let empty = layout.empty;
        body = body.children(frames).child(self.blocks_canvas(blocks)).children(now_lines);
        if empty {
            body = body.child(self.empty_state(cx));
        }
        div().flex().flex_col().child(header).child(body).into_any_element()
    }
}

impl Rireki {
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

/// 各レーンがその日 [ds, de) に動いていた区間 (tracks の位置, 開始, 終了)。
/// IDLE より長く手が空いたところで区切る（1 日中開いているセッションが列を占有しないように）
fn day_segments(sessions: &[std::sync::Arc<crate::model::Session>], tracks: &[Track], ds: f64, de: f64) -> Vec<(usize, f64, f64)> {
    const IDLE: f64 = 15.0 * 60.0;
    let mut out = Vec::new();
    for (ti, t) in tracks.iter().enumerate() {
        let lane = &sessions[t.session].lanes[t.lane];
        if lane.range().is_none_or(|(start, end)| end < ds || start >= de) {
            continue;
        }
        let i = lane.cum_end.partition_point(|&e| e < ds);
        let j = lane.events.partition_point(|e| e.start < de);
        let mut cur: Option<(f64, f64)> = None;
        for e in &lane.events[i.min(j)..j] {
            let (s, en) = (e.start.max(ds), e.end.min(de));
            cur = match cur {
                Some((a, b)) if s <= b + IDLE => Some((a, b.max(en))),
                Some(seg) => {
                    out.push((ti, seg.0, seg.1));
                    Some((s, en))
                }
                None => Some((s, en)),
            };
        }
        if let Some((a, b)) = cur {
            out.push((ti, a, b));
        }
    }
    out
}

/// グループ枠の左の色帯の分だけ、中のブロックを右にずらす
const GROUP_INSET: f32 = 4.0;
/// グループ枠の見出しの高さ
const GROUP_HEADER: f32 = 16.0;

/// グループ（セッション）の区間
struct GroupSeg {
    start: f64,
    end: f64,
    /// この区間に動いていたレーン（tracks の位置、tracks の順 = メインが先）
    lanes: Vec<usize>,
}

/// レーンの区間をグループごとにまとめ、重なる・近い区間をつなげる
fn group_segments(segs: &[(usize, f64, f64)], group: &[usize]) -> Vec<GroupSeg> {
    const IDLE: f64 = 15.0 * 60.0;
    let mut by: std::collections::BTreeMap<usize, Vec<(usize, f64, f64)>> = Default::default();
    for &(ti, a, b) in segs {
        by.entry(group[ti]).or_default().push((ti, a, b));
    }
    let mut out = Vec::new();
    for (_, mut v) in by {
        v.sort_by(|x, y| x.1.total_cmp(&y.1));
        let mut cur: Option<GroupSeg> = None;
        for (ti, a, b) in v {
            match &mut cur {
                Some(g) if a <= g.end + IDLE => {
                    g.end = g.end.max(b);
                    if !g.lanes.contains(&ti) {
                        g.lanes.push(ti);
                    }
                }
                _ => {
                    out.extend(cur.take());
                    cur = Some(GroupSeg { start: a, end: b, lanes: vec![ti] });
                }
            }
        }
        out.extend(cur);
    }
    for g in &mut out {
        g.lanes.sort_unstable();
    }
    out
}

/// 時間帯が重なる区間だけを横に並べる。各区間の (列, その重なりの列数) を返す。
/// 見た目で接しないよう、間が 8px 未満のものも重なりとみなす
fn pack_columns(spans: &[(f64, f64)], spp: f64) -> Vec<(usize, usize)> {
    let pad = 8.0 * spp;
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by(|&a, &b| spans[a].0.total_cmp(&spans[b].0).then(a.cmp(&b)));
    let mut out = vec![(0, 1); spans.len()];
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
        for k in members {
            out[k].1 = cols.len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(session: usize, lane: usize) -> Track {
        Track { session, lane, depth: 0, link: None, active: false }
    }

    fn key() -> Key {
        Key {
            first: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
            days: 7,
            generation: 1,
            in_view_only: true,
        }
    }

    #[test]
    fn packs_non_overlapping_into_one_column() {
        let r = pack_columns(&[(0.0, 100.0), (200.0, 300.0), (50.0, 150.0)], 1.0);
        assert_eq!(r, vec![(0, 2), (0, 1), (1, 2)]);
    }

    #[test]
    fn indexed_groups_preserve_group_and_lane_positions() {
        let groups = group_segments(&[
            (0, 100.0, 200.0), (1, 120.0, 300.0),
            (0, 1500.0, 1600.0), (1, 1550.0, 1700.0),
            (0, 3000.0, 3100.0),
            (2, 90.0, 180.0), (2, 1500.0, 1600.0),
        ], &[0, 0, 2, 3]);
        let day = DayLayout::new(Rc::new((0..4).map(|i| track(i, 0)).collect()), groups);
        for ti in 0..day.tracks.len() {
            let mine: Vec<_> = (0..day.groups.len()).filter(|&k| day.groups[k].lanes.contains(&ti)).collect();
            for start in [-1.0, 90.0, 100.0, 500.0, 1499.0, 1500.0, 1600.0, 3000.0, 5000.0] {
                let expected = mine.iter().rev().find(|&&k| day.groups[k].start <= start)
                    .or(mine.first()).map(|&k| (k, day.groups[k].lanes.iter().position(|&t| t == ti).unwrap()));
                assert_eq!(day.group_at(ti, start), expected, "track={ti}, start={start}");
            }
        }
    }

    #[test]
    fn layout_reuses_only_the_same_dates_data_and_filter() {
        let mut cache = Cache::default();
        let key = key();
        let layout = cache.set(key, vec![DayLayout::new(Rc::new(vec![track(0, 0)]), vec![])]);
        assert!(Rc::ptr_eq(&layout, &cache.get(key).unwrap()));
        for changed in [
            Key { first: key.first + Duration::days(7), ..key },
            Key { days: 1, ..key },
            Key { generation: key.generation + 1, ..key },
            Key { in_view_only: false, ..key },
        ] {
            assert!(cache.get(changed).is_none());
        }
        let old = Rc::downgrade(&layout);
        drop(layout);
        cache.set(Key { days: 1, ..key }, Vec::new());
        assert!(old.upgrade().is_none(), "以前の週を保持し続けない");
    }

    #[test]
    fn zoom_reuses_segments_and_recalculates_columns() {
        let mut cache = Cache::default();
        let layout = cache.set(key(), vec![DayLayout::new(Rc::new(vec![track(0, 0), track(1, 0)]), vec![
            GroupSeg { start: 100.0, end: 200.0, lanes: vec![0] },
            GroupSeg { start: 300.0, end: 400.0, lanes: vec![1] },
        ])]);
        let columns = cache.columns(1.0);
        assert!(Rc::ptr_eq(&columns, &cache.columns(1.0)));
        assert_eq!(columns[0], vec![(0, 1), (0, 1)]);
        let zoomed = cache.columns(10.0);
        assert_eq!(zoomed[0], vec![(0, 2), (1, 2)]);
        assert!(Rc::ptr_eq(&layout, &cache.get(key()).unwrap()));
        cache.set(Key { generation: 2, ..key() }, vec![DayLayout::new(Rc::default(), Vec::new())]);
        assert!(cache.columns(10.0)[0].is_empty(), "同じ倍率でもデータ更新後は再配置する");
    }

    #[test]
    fn counts_include_inactive_lanes_and_deduplicate_sessions_across_days() {
        let mut cache = Cache::default();
        let layout = cache.set(key(), vec![
            DayLayout::new(Rc::new(vec![track(0, 0), track(0, 1), track(1, 0)]), Vec::new()),
            DayLayout::new(Rc::new(vec![track(0, 0)]), Vec::new()),
        ]);
        assert_eq!(layout.counts, "2 セッション · 4 レーン");
        assert!(!layout.empty);
        assert!(cache.set(key(), Vec::new()).empty);
    }

    #[test]
    fn shared_tracks_still_count_every_day_without_recounting_sessions() {
        let mut cache = Cache::default();
        let tracks = Rc::new(vec![track(0, 0), track(0, 1), track(1, 0)]);
        let layout = cache.set(key(), (0..7).map(|_| DayLayout::new(tracks.clone(), vec![])).collect());
        assert_eq!(layout.counts, "2 セッション · 21 レーン");
    }

    #[test]
    fn daily_segments_preserve_midnight_boundaries_overlaps_and_idle_gaps() {
        use crate::model::{Event, Kind, Lane, Session, Source};
        let mut lane = Lane::new("main");
        for (start, end) in [(-100.0, 0.0), (100.0, 200.0), (2000.0, 90_000.0),
            (2500.0, 2600.0), (86_400.0, 87_000.0), (200_000.0, 200_100.0)] {
            lane.events.push(Event {
                start, end, kind: Kind::Tool, name: "exec_command".into(), summary: Default::default(),
                input: String::new(), output: String::new(), call_id: None,
                text_ref: Default::default(), usage: Default::default(), model: None,
                status: Default::default(),
            });
        }
        let mut session = Session {
            id: String::new(), source: Source::Codex, cwd: String::new(), headless: false,
            title: String::new(), branch: String::new(), version: String::new(),
            lanes: vec![lane, Lane::new("empty")], start: 0.0, end: 0.0,
            parents: Vec::new(), tree: Vec::new(),
        };
        session.finalize();
        let sessions = [std::sync::Arc::new(session)];
        let tracks = [track(0, 0), track(0, 1)];
        assert_eq!(day_segments(&sessions, &tracks, 0.0, 86_400.0),
            [(0, 0.0, 200.0), (0, 2000.0, 86_400.0)]);
        assert_eq!(day_segments(&sessions, &tracks, 86_400.0, 172_800.0),
            [(0, 86_400.0, 90_000.0)]);
        assert!(day_segments(&sessions, &tracks, 200_100.1, 300_000.0).is_empty());
        assert!(day_segments(&sessions, &tracks, -1000.0, -100.0).is_empty());
        assert_eq!(day_segments(&sessions, &tracks, 200_100.0, 300_000.0),
            [(0, 200_100.0, 200_100.0)]);
    }
}
