//! カレンダー（日 / 週）。縦軸が時刻で、日ごとの列にその日のレーンを並べる

use chrono::{Datelike, Duration, Local};
use gpui::{AnyElement, ClickEvent, Context, ElementId, FontWeight, Window, div, prelude::*, px};

use super::header::weekday_ja;
use super::widgets::tip;
use super::{AXIS_H, BlockPaint, HOUR_LABEL_W, MIN_BLOCK, Rireki, Selection, midnight, now};
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
                now_lines.push(
                    div().absolute().left(px(x0 - 4.0)).top(px(y - 3.0)).size(px(8.0)).rounded_full().bg(c(theme::NOW)),
                );
            }
        }
        self.counts = format!("{} セッション · {lanes} レーン", sessions.len());
        // 見えている時間帯に無くても、その日に作業があれば「なし」とは出さない
        let empty = lanes == 0;
        body = body.child(self.blocks_canvas(blocks)).children(now_lines);
        if empty {
            body = body.child(self.empty_state(cx));
        }
        div().flex().flex_col().child(header).child(body).into_any_element()
    }
}
