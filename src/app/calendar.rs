//! カレンダー（日 / 週）。縦軸が時刻で、日ごとの列にその日のレーンを並べる

use chrono::{Datelike, Duration, Local};
use gpui::{AnyElement, ClickEvent, Context, ElementId, FontWeight, Window, div, prelude::*, px};

use super::header::weekday_ja;
use super::widgets::tip;
use super::{AXIS_H, BlockPaint, HOUR_LABEL_W, MIN_BLOCK, Rireki, Selection, Track, midnight, now};
use std::collections::HashMap;

use crate::layout;
use crate::prefs::Mode;
use crate::theme::{self, c};

impl Rireki {
    pub(super) fn calendar(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let week = self.mode == Mode::Week;
        let days = if week { 7 } else { 1 };
        let first = if week { self.week_start() } else { self.day0 };
        let total_w = self.main_width(window);
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
        let mut frames = Vec::new();
        let mut sessions = std::collections::HashSet::new();
        let mut lanes = 0;
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

            let tracks = self.tracks_in(ds, de);
            lanes += tracks.len();
            sessions.extend(tracks.iter().map(|t| t.session));
            if tracks.is_empty() {
                continue;
            }
            let v0 = ds + scroll as f64 * spp;
            let v1 = v0 + ph as f64 * spp;
            // セッション（メイン + サブエージェント）を 1 つのグループにし、手の空いた時間で区切った
            // グループの区間どうしで重なるものだけを横に並べる。グループの中はレーンごとに分ける
            let groups = group_segments(&self.day_segments(&tracks, ds, de), self.group_of(&tracks));
            // 見出しの分だけ上に広げて詰める
            let cols = pack_columns(&groups.iter().map(|g| (g.start - GROUP_HEADER as f64 * spp, g.end)).collect::<Vec<_>>(), spp);
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
                let lane = &self.store.sessions[tr.session].lanes[tr.lane];
                let mine: Vec<usize> = (0..groups.len()).filter(|&k| groups[k].lanes.contains(&ti)).collect();
                for b in layout::aggregate(lane, (v0.max(ds), v1.min(de)), spp, self.gran) {
                    let bs = b.start.max(ds);
                    let Some(k) = mine.iter().rev().find(|&&k| groups[k].start <= bs).or(mine.first()).copied() else {
                        continue;
                    };
                    let g = &groups[k];
                    let gw = gw_of(k);
                    let lw = (gw - 2.0 - GROUP_INSET) / g.lanes.len() as f32;
                    let slot = g.lanes.iter().position(|&t| t == ti).unwrap_or(0);
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
        self.counts = format!("{} セッション · {lanes} レーン", sessions.len());
        // 見えている時間帯に無くても、その日に作業があれば「なし」とは出さない
        let empty = lanes == 0;
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

    /// 各レーンがその日 [ds, de) に動いていた区間 (tracks の位置, 開始, 終了)。
    /// IDLE より長く手が空いたところで区切る（1 日中開いているセッションが列を占有しないように）
    fn day_segments(&self, tracks: &[Track], ds: f64, de: f64) -> Vec<(usize, f64, f64)> {
        const IDLE: f64 = 15.0 * 60.0;
        let mut out = Vec::new();
        for (ti, t) in tracks.iter().enumerate() {
            let lane = &self.store.sessions[t.session].lanes[t.lane];
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
fn group_segments(segs: &[(usize, f64, f64)], group: Vec<usize>) -> Vec<GroupSeg> {
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
    use super::pack_columns;

    #[test]
    fn packs_non_overlapping_into_one_column() {
        let r = pack_columns(&[(0.0, 100.0), (200.0, 300.0), (50.0, 150.0)], 1.0);
        assert_eq!(r, vec![(0, 2), (0, 1), (1, 2)]);
    }
}
