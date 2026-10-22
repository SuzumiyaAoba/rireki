//! カレンダー（日 / 週 / 月）。
//!
//! - 日・週: 縦軸が時刻。セッション（メイン + サブエージェント + 起動した子セッション）が動いていた区間を
//!   1 つのブロックにし、上端を開始時刻にそろえて枠の中に名前を書く。色はプロジェクトの色。
//!   幅に余裕があれば、ユーザーの指示をその時刻の位置に並べる（押すとそのターンを開く）。
//! - 月: 日ごとの稼働時間を濃淡で示し、その日のセッションを開始順に並べる。

mod segments;
mod columns;
mod day;
mod month;
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
mod tests;
