//! メイン列の見出し（期間の移動・表示範囲・拡大縮小・まとめ方）と、下端の凡例

use chrono::{Datelike, Duration, Weekday};
use gpui::{ClickEvent, Context, Div, FontWeight, Window, canvas, div, prelude::*, px};

use super::widgets::{event_icon, project_tile, tip};
use super::{FOOTER_H, HEADER_H, Popover, Rireki, fmt_dur, local, now};
use crate::icons::{self, Icon};
use crate::prefs::Mode;
use crate::theme::{self, c};

/// タイムラインの表示範囲の選択肢（秒。0 は全期間）
const PRESETS: [(&str, f64); 6] = [
    ("1時間", 3600.0),
    ("6時間", 6.0 * 3600.0),
    ("1日", 86400.0),
    ("1週", 7.0 * 86400.0),
    ("1ヶ月", 30.0 * 86400.0),
    ("全期間", 0.0),
];

pub(super) fn weekday_ja(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "月",
        Weekday::Tue => "火",
        Weekday::Wed => "水",
        Weekday::Thu => "木",
        Weekday::Fri => "金",
        Weekday::Sat => "土",
        Weekday::Sun => "日",
    }
}

impl Rireki {
    /// 表示中のセッション全体の期間
    fn order_bounds(&self) -> Option<(f64, f64)> {
        let ss = self.order.iter().map(|&si| &self.store.sessions[si]);
        let (a, b) = ss.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| (a.min(s.start), b.max(s.end)));
        a.is_finite().then_some((a, b))
    }

    fn apply_preset(&mut self, i: usize, window: &Window) {
        self.mode = Mode::Timeline;
        self.pick = None;
        let secs = PRESETS[i].1;
        if secs > 0.0 {
            let n = now();
            self.set_range(n - secs * 0.95, n + secs * 0.05, window);
        } else if let Some((a, b)) = self.order_bounds() {
            let pad = (b - a) * 0.02;
            self.set_range(a - pad, b + pad, window);
        }
        self.persist();
    }

    /// 今の表示幅に一致する表示範囲の選択肢
    fn active_preset(&self, window: &Window) -> Option<usize> {
        let span = self.spp * self.plot_width(window) as f64;
        PRESETS.iter().position(|&(_, secs)| {
            if secs > 0.0 {
                (span - secs).abs() < secs * 0.02
            } else {
                self.order_bounds().is_some_and(|(a, b)| ((b - a) * 1.04 - span).abs() < span * 0.02)
            }
        })
    }

    fn title(&self, window: &Window, compact: bool) -> String {
        match self.mode {
            Mode::Day => {
                let d = self.day0;
                format!("{}年{}月{}日（{}）", d.year(), d.month(), d.day(), weekday_ja(d.weekday()))
            }
            Mode::Week => {
                let a = self.week_start();
                let b = a + Duration::days(6);
                if a.month() == b.month() {
                    format!("{}年{}月{}日 – {}日", a.year(), a.month(), a.day(), b.day())
                } else if a.year() == b.year() {
                    format!("{}年{}月{}日 – {}月{}日", a.year(), a.month(), a.day(), b.month(), b.day())
                } else {
                    format!("{} – {}", a.format("%Y/%-m/%-d"), b.format("%Y/%-m/%-d"))
                }
            }
            Mode::Timeline => {
                let (a, b) = (local(self.t0), local(self.t0 + self.spp * self.plot_width(window) as f64));
                let same_day = a.date_naive() == b.date_naive();
                let wd = |d: &chrono::DateTime<chrono::Local>| {
                    if compact { String::new() } else { format!("（{}）", weekday_ja(d.weekday())) }
                };
                if same_day {
                    format!("{}{} {} – {}", a.format("%-m/%-d"), wd(&a), a.format("%H:%M"), b.format("%H:%M"))
                } else {
                    format!("{}{} – {}{}", a.format("%-m/%-d %H:%M"), wd(&a), b.format("%-m/%-d %H:%M"), wd(&b))
                }
            }
        }
    }

    /// 注目中のプロジェクトを示し、×で絞り込みを解除するチップ
    pub(super) fn focus_chip(&self, cx: &mut Context<Self>) -> Option<Div> {
        let f = self.focus.as_ref()?;
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .pl_1()
                .pr(px(2.0))
                .h(px(26.0))
                .rounded_md()
                .bg(c(theme::ACCENT_BG))
                .border_1()
                .border_color(c(theme::ACCENT).opacity(0.45))
                .text_xs()
                .child(project_tile(f, 16.0))
                .child(
                    div()
                        .max_w(px(160.0))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.project_name(f)),
                )
                .child(
                    div()
                        .id("focus-clear")
                        .p(px(3.0))
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::HOVER)))
                        .child(icons::icon(Icon::X, theme::MUTED).size_3())
                        .tooltip(tip("プロジェクトの絞り込みを解除", None))
                        .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.set_focus(None, cx);
                            cx.notify();
                        })),
                ),
        )
    }

    pub(super) fn header(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let timeline = self.mode == Mode::Timeline;
        let w = self.main_width(window);
        let mut row = div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(if w < 650.0 { 4.0 } else { 8.0 }))
            .px(px(if w < 650.0 { 8.0 } else { 12.0 }))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_sm();
        if !self.sidebar_open {
            row = row.child(self.ghost_button(
                "sidebar-open",
                Icon::PanelLeftOpen,
                "サイドバーを表示",
                Some("⌃⌘S"),
                true,
                cx,
                |t, _, cx| t.toggle_sidebar(cx),
            ));
        }
        if let Some(chip) = self.focus_chip(cx) {
            row = row.child(chip);
        }
        let nav = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.0))
            .child(self.ghost_button("prev", Icon::ChevronLeft, "前の期間", Some("←"), true, cx, |t, w, _| {
                t.step(-1.0, w)
            }))
            .child(
                div()
                    .id("today")
                    .px_2()
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .text_xs()
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::HOVER)))
                    .child("今日")
                    .tooltip(tip(if timeline { "現在時刻へ移動" } else { "今日へ移動" }, Some("T")))
                    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| {
                        t.go_today(w);
                        cx.notify();
                    })),
            )
            .child(self.ghost_button("next", Icon::ChevronRight, "次の期間", Some("→"), true, cx, |t, w, _| {
                t.step(1.0, w)
            }));
        row = row.child(nav).child(
            div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(self.title(window, w < 900.0)),
        );
        if timeline {
            if w >= 650.0 {
                // 狭いときは「6時間」「1ヶ月」を省く
                let items: Vec<(usize, &'static str)> = PRESETS
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| w >= 760.0 || !matches!(i, 1 | 4))
                    .map(|(i, p)| (i, p.0))
                    .collect();
                row =
                    row.child(self.segmented("preset", &items, self.active_preset(window), cx, |t, i, w, _| {
                        t.apply_preset(i, w)
                    }));
            }
        } else {
            row = row.child(self.segmented(
                "calmode",
                &[(Mode::Day, "日"), (Mode::Week, "週")],
                Some(self.mode),
                cx,
                |t, m, w, _| t.set_mode(m, w),
            ));
        }
        let zoom = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.0))
            .child(self.ghost_button("zoom-out", Icon::ZoomOut, "縮小", Some("−"), true, cx, |t, w, _| t.zoom(2.0, w)))
            .child(self.ghost_button("zoom-in", Icon::ZoomIn, "拡大", Some("+"), true, cx, |t, w, _| t.zoom(0.5, w)));
        let anchor = self.gran_anchor.clone();
        let open = self.popover == Some(Popover::Granularity);
        let gran = div()
            .id("gran")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(26.0))
            .rounded_md()
            .border_1()
            .border_color(c(if open { theme::ACCENT } else { theme::BORDER }))
            .when(open, |d| d.bg(c(theme::ACCENT_BG)))
            .text_xs()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(Icon::Layers, theme::MUTED))
            .when(w >= 760.0, |d| d.child(div().text_color(c(theme::MUTED)).child("まとめ方")))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.gran.label()))
            .child(icons::icon(Icon::ChevronDown, theme::MUTED).size_3())
            .child(canvas(move |b, _, _| anchor.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .tooltip(tip("ブロックのまとめ方", None))
            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.popover = if t.popover == Some(Popover::Granularity) { None } else { Some(Popover::Granularity) };
                cx.notify();
            }));
        let on = self.in_view_only;
        let directory = self.directory_timeline;
        let cost = self.cost_timeline;
        let cost_view = self.icon_button("cost-timeline", Some(Icon::Chart),
            if w >= 900.0 || cost { "API コスト" } else { "" }, cost, cx,
            |t, _, cx| t.toggle_cost_timeline(cx))
            .tooltip(tip(if cost { "イベントのタイムラインに戻す" } else {
                "API 換算料金をセッション・ディレクトリ別に時系列で表示（USD）"
            }, None));
        let directory_view = div()
            .id("directory-timeline")
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(26.0))
            .rounded_md()
            .border_1()
            .border_color(c(if directory { theme::ACCENT } else { theme::BORDER }))
            .when(directory, |d| d.bg(c(theme::ACCENT_BG)))
            .text_xs()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(Icon::Folder, if directory { theme::ACCENT } else { theme::MUTED }))
            .when(w >= 900.0, |d| d.child("ディレクトリ単位"))
            .tooltip(tip(
                if directory {
                    "セッションごとの表示に戻す"
                } else if cost {
                    "ディレクトリ単位で全セッション・全レーンの API 換算料金を合計して表示"
                } else {
                    "ディレクトリ単位でメインイベントを詰めて表示（サブエージェントは省略、重なる枠だけ別の行に配置）"
                },
                None,
            ))
            .on_click(cx.listener(|t, _: &ClickEvent, window, cx| t.toggle_directory_timeline(window, cx)));
        let in_view = div()
            .id("in-view-only")
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(26.0))
            .rounded_md()
            .border_1()
            .border_color(c(if on { theme::ACCENT } else { theme::BORDER }))
            .when(on, |d| d.bg(c(theme::ACCENT_BG)))
            .text_xs()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(Icon::Filter, if on { theme::ACCENT } else { theme::MUTED }))
            .when(w >= 760.0, |d| d.child("範囲内のみ"))
            .tooltip(tip(
                if on {
                    "表示範囲外のセッションもグレーアウトして表示する"
                } else {
                    "表示範囲内のセッションだけを表示する"
                },
                Some("V"),
            ))
            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.toggle_in_view_only(cx)));
        row.child(self.range_button(cx))
            .when(w >= 700.0, |d| d.child(zoom))
            .when(timeline, |d| d.child(cost_view).child(directory_view).child(in_view))
            .when(!timeline || !cost, |d| d.child(gran))
    }

    /// 下端の帯。表示中の件数と目盛りの間隔、ブロックの色の凡例
    pub(super) fn footer(&self, window: &Window) -> Div {
        if self.mode == Mode::Timeline && self.cost_timeline {
            return self.cost_footer(window);
        }
        let w = self.main_width(window);
        let left = if self.mode == Mode::Timeline {
            format!("{}  ·  目盛り {} / 100px", self.counts, fmt_dur(self.spp * 100.0))
        } else {
            self.counts.clone()
        };
        // 凡例は幅に収まる分だけ並べる
        let text_w = |s: &str| s.chars().map(|ch| if ch.is_ascii() { 6.5 } else { 11.5 }).sum::<f32>();
        let mut room = w - 32.0 - text_w(&left) - 24.0;
        let mut legend = Vec::new();
        for (name, col, ic) in theme::LEGEND {
            let need = 12.0 + 4.0 + text_w(name) + 12.0;
            if room < need {
                break;
            }
            room -= need;
            legend
                .push(div().flex().flex_none().items_center().gap_1().child(event_icon(*ic, *col, 12.0)).child(*name));
        }
        div()
            .h(px(FOOTER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_xs()
            .text_color(c(theme::MUTED))
            .child(div().flex_none().whitespace_nowrap().child(left))
            .child(div().flex_1())
            .children(legend)
    }
}
