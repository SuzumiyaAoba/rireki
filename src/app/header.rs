//! メイン列の見出し（ツールバー）と、下端の凡例
//!
//! ツールバーは左に「移動（前 / 今日 / 次）」「表示中の期間」を、右に「絞り込み中のプロジェクト」「拡大縮小」
//! 「表示の切り替え（イベント / API コスト、またはカレンダーの日 / 週 / 月）」「表示オプション」を置く。
//! 期間の表記は押すと期間（カレンダーでは日付）を選ぶポップオーバーを開く。
//! 幅が足りないときは推定幅で配置を決め、期間の表記を短くし、表示の切り替えを表示オプションのメニューへ移す。
//! 部品を黙って消さず、表示中の期間は最後まで残す。

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use gpui::{ClickEvent, Context, Div, ElementId, FontWeight, Stateful, Window, canvas, div, prelude::*, px};

use super::widgets::{event_icon, project_tile, tip};
use super::{FOOTER_H, HEADER_H, Popover, Rireki, fmt_dur, local, now, tilde};
use crate::icons::{self, Icon};
use crate::prefs::Mode;
use crate::theme::{self, c};

/// ツールバーの部品の高さ
pub(super) const CONTROL_H: f32 = 28.0;
/// 部品の枠の中の項目の高さ（枠線と内側の余白を除く）
const ITEM_H: f32 = CONTROL_H - 6.0;
/// ツールバーの左右の余白と、部品の間隔
const PAD: f32 = 12.0;
const ITEM_GAP: f32 = 8.0;
/// 幅がとても狭いときの余白と間隔
const DENSE_PAD: f32 = 8.0;
const DENSE_GAP: f32 = 6.0;

/// 部品の推定幅。配置を決めるときだけ使う（実際の幅は描画で決まる）
const NAV_W: f32 = 102.0;
const ZOOM_W: f32 = 60.0;
const CALMODE_W: f32 = 94.0;
const SIDEBAR_BUTTON_W: f32 = 26.0;
const OPTIONS_W: f32 = 76.0;
const OPTIONS_ICON_W: f32 = 43.0;
/// 絞り込み中のプロジェクトのチップから名前を除いた幅
const CHIP_W: f32 = 48.0;

/// 表示期間の選択肢（直近の秒数。0 は全期間）
pub(super) const PRESETS: [(&str, f64); 7] = [
    ("直近 1 時間", 3600.0),
    ("直近 6 時間", 6.0 * 3600.0),
    ("直近 24 時間", 86400.0),
    ("直近 7 日間", 7.0 * 86400.0),
    ("直近 30 日間", 30.0 * 86400.0),
    ("直近 90 日間", 90.0 * 86400.0),
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

/// 文字列の推定幅。かな・漢字・全角記号は文字の大きさの 1 倍、英数字は 0.62 倍、空白や区切り記号は細く見積もる。
/// 実際の幅（システムフォント）より少し広めになるようにし、部品がはみ出さないようにする
fn text_w(s: &str, size: f32) -> f32 {
    let em: f32 = s
        .chars()
        .map(|ch| match ch {
            ' ' => 0.3,
            ':' | '/' | '.' | ',' => 0.36,
            '–' => 0.56,
            _ if ch.is_ascii() => 0.62,
            _ => 1.0,
        })
        .sum();
    em * size * 1.04
}

/// 期間の表記のボタンの推定幅（矢印・余白と、`icon` ならアイコンを含む。太字の分を少し足す）
fn title_w(s: &str, icon: bool) -> f32 {
    text_w(s, 14.0) * 1.04 + if icon { 54.0 } else { 34.0 }
}

/// 表示の切り替え（イベント / API コスト）のラベル。狭いときは短くする
fn cost_label(full: bool) -> &'static str {
    if full { "API コスト" } else { "コスト" }
}

fn switch_w(full: bool) -> f32 {
    6.0 + (16.0 + text_w("イベント", 12.0)) + 2.0 + (16.0 + text_w(cost_label(full), 12.0))
}

/// 日付の表記。`long` なら「10月7日」、そうでなければ「10/7」とし、`year` なら年を添える
fn date_label(d: NaiveDate, long: bool, year: bool) -> String {
    match (long, year) {
        (true, true) => format!("{}年{}月{}日", d.year(), d.month(), d.day()),
        (true, false) => format!("{}月{}日", d.month(), d.day()),
        (false, true) => format!("{}/{}/{}", d.year(), d.month(), d.day()),
        (false, false) => format!("{}/{}", d.month(), d.day()),
    }
}

/// タイムラインの表示範囲の表記。長い順に並べ、幅に収まる最初のものを使う。
/// 2 日以上は日付だけ、2 分未満は秒まで出す
fn timeline_titles(a: DateTime<Local>, b: DateTime<Local>) -> Vec<String> {
    let this_year = Local::now().year();
    let span = (b - a).num_seconds();
    if span >= 2 * 86400 {
        // 終わりがちょうど午前 0 時なら前日までとして書く
        let (da, db) = (a.date_naive(), (b - Duration::seconds(1)).date_naive());
        let year = da.year() != this_year || db.year() != this_year;
        let wd = |d: NaiveDate| format!("（{}）", weekday_ja(d.weekday()));
        return vec![
            format!("{}{} – {}{}", date_label(da, true, year), wd(da), date_label(db, true, year), wd(db)),
            format!("{} – {}", date_label(da, true, year), date_label(db, true, year)),
            format!("{} – {}", date_label(da, false, year), date_label(db, false, year)),
        ];
    }
    let time = if span < 120 { "%H:%M:%S" } else { "%H:%M" };
    let (da, db) = (a.date_naive(), b.date_naive());
    let year = da.year() != this_year || db.year() != this_year;
    let (ta, tb) = (a.format(time), b.format(time));
    let wd = |d: NaiveDate| format!("（{}）", weekday_ja(d.weekday()));
    if da == db {
        vec![
            format!("{}{} {ta} – {tb}", date_label(da, true, year), wd(da)),
            format!("{}{} {ta} – {tb}", date_label(da, false, year), wd(da)),
            format!("{} {ta} – {tb}", date_label(da, false, year)),
            format!("{ta} – {tb}"),
        ]
    } else {
        let mut out = vec![
            format!("{}{} {ta} – {}{} {tb}", date_label(da, true, year), wd(da), date_label(db, true, year), wd(db)),
            format!("{}{} {ta} – {}{} {tb}", date_label(da, false, year), wd(da), date_label(db, false, year), wd(db)),
            format!("{} {ta} – {} {tb}", date_label(da, false, year), date_label(db, false, year)),
        ];
        // 翌日までなら終わりの日付を「翌」で表す
        if da.succ_opt() == Some(db) {
            out.push(format!("{ta} – 翌{tb}"));
        }
        out
    }
}

/// 幅に合わせたツールバーの配置
#[derive(Clone, Copy, Debug, PartialEq)]
struct Plan {
    /// 使う期間の表記（候補の位置）
    title: usize,
    /// 絞り込み中のプロジェクト名の最大幅。None なら頭文字の印だけを出す
    chip: Option<f32>,
    zoom: bool,
    /// イベント / API コストの切り替えを出すか（出すなら長いラベルか）。出さないときは表示オプションのメニューで切り替える
    switch: Option<bool>,
    /// 表示オプションのボタンにラベルを付けるか
    options_label: bool,
    /// 余白と間隔を詰め、期間の表記のアイコンを省く（最後の手段）
    dense: bool,
}

/// ツールバーの配置を決める材料
struct Fit<'a> {
    /// 期間の表記の候補（長い順）
    titles: &'a [String],
    timeline: bool,
    /// サイドバーを開くボタンを置くか
    sidebar_button: bool,
    /// 絞り込み中のプロジェクト名の推定幅
    chip_name: Option<f32>,
}

impl Fit<'_> {
    /// 配置 `p` の推定幅
    fn width(&self, p: &Plan) -> f32 {
        // 移動・期間・詰め物は常に置く
        let mut items = vec![NAV_W, title_w(&self.titles[p.title], !p.dense), 0.0];
        if self.sidebar_button {
            items.push(SIDEBAR_BUTTON_W);
        }
        if let Some(name) = self.chip_name {
            items.push(CHIP_W + p.chip.map_or(0.0, |max| name.min(max) + 6.0));
        }
        if p.zoom {
            items.push(ZOOM_W);
        }
        if self.timeline {
            if let Some(full) = p.switch {
                items.push(switch_w(full));
            }
            items.push(if p.options_label { OPTIONS_W } else { OPTIONS_ICON_W });
        } else {
            items.push(CALMODE_W);
        }
        let (pad, gap) = if p.dense { (DENSE_PAD, DENSE_GAP) } else { (PAD, ITEM_GAP) };
        pad * 2.0 + items.iter().sum::<f32>() + gap * (items.len() - 1) as f32
    }

    /// 幅 `avail` に収まるまで、優先度の低いものから順に縮める。表示の切り替えは表示オプションのメニューにもあるので、
    /// 拡大縮小より先に移す。表示中の期間は最後まで残し、それでも収まらなければ表記の末尾を省く
    fn plan(&self, avail: f32) -> Plan {
        let last = self.titles.len() - 1;
        let mut p =
            Plan { title: 0, chip: Some(160.0), zoom: true, switch: Some(true), options_label: true, dense: false };
        let steps: &[fn(&mut Plan, usize)] = if self.timeline {
            &[
                |p, last| p.title = last.min(1),
                |p, _| p.chip = Some(96.0),
                |p, _| p.switch = Some(false),
                |p, _| p.options_label = false,
                |p, last| p.title = last.min(2),
                |p, _| p.chip = None,
                |p, _| p.switch = None,
                |p, _| p.zoom = false,
                |p, last| p.title = last,
                |p, _| p.dense = true,
            ]
        } else {
            &[
                |p, last| p.title = last.min(1),
                |p, _| p.chip = Some(96.0),
                |p, _| p.zoom = false,
                |p, last| p.title = last.min(2),
                |p, _| p.chip = None,
                |p, _| p.dense = true,
            ]
        };
        for step in steps {
            if self.width(&p) <= avail {
                break;
            }
            step(&mut p, last);
        }
        p
    }
}

/// ツールバーの部品の枠。項目を内側の余白付きで並べる（`bg` は地の色）
fn tool_frame(bg: u32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .h(px(CONTROL_H))
        .rounded_md()
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(bg))
}

/// ツールバーの切り替えの選択肢
pub(super) struct Choice<T> {
    pub value: T,
    pub label: &'static str,
    pub tip: &'static str,
    pub key: Option<&'static str>,
}

impl Rireki {
    /// 表示中のセッション全体の期間
    pub(super) fn order_bounds(&self) -> Option<(f64, f64)> {
        if self.order.is_empty() { return None; }
        let (a, b) = self.order_extent.get().unwrap_or_else(|| {
            let ss = self.order.iter().map(|&si| &self.store.sessions[si]);
            let bounds = ss.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), s| (a.min(s.start), b.max(s.end)));
            self.order_extent.set(Some(bounds));
            bounds
        });
        a.is_finite().then_some((a, b))
    }

    pub(super) fn apply_preset(&mut self, i: usize, window: &Window) {
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

    /// 今の表示範囲に一致する表示期間の選択肢。「直近」なので、同じ長さでも現在時刻を含まなければ当てはめない
    pub(super) fn active_preset(&self, window: &Window) -> Option<usize> {
        let span = self.spp * self.plot_width(window) as f64;
        let recent = (self.t0..=self.t0 + span).contains(&now());
        PRESETS.iter().position(|&(_, secs)| {
            if secs > 0.0 {
                recent && (span - secs).abs() < secs * 0.02
            } else {
                self.order_bounds().is_some_and(|(a, b)| ((b - a) * 1.04 - span).abs() < span * 0.02)
            }
        })
    }

    /// 表示中の期間の表記。長い順に並べる
    fn titles(&self, window: &Window) -> Vec<String> {
        let this_year = Local::now().year();
        match self.mode {
            Mode::Day => {
                let d = self.day0;
                let wd = weekday_ja(d.weekday());
                let year = d.year() != this_year;
                vec![
                    format!("{}（{wd}）", date_label(d, true, true)),
                    format!("{}（{wd}）", date_label(d, true, year)),
                    format!("{}（{wd}）", date_label(d, false, year)),
                ]
            }
            Mode::Month => vec![format!("{}年{}月", self.day0.year(), self.day0.month())],
            Mode::Week => {
                let a = self.week_start();
                let b = a + Duration::days(6);
                let year = a.year() != this_year || b.year() != this_year;
                let long = if a.year() != b.year() {
                    format!("{} – {}", date_label(a, true, true), date_label(b, true, true))
                } else if a.month() != b.month() {
                    format!("{} – {}月{}日", date_label(a, true, true), b.month(), b.day())
                } else {
                    format!("{} – {}日", date_label(a, true, true), b.day())
                };
                let medium = if a.year() != b.year() || a.month() != b.month() {
                    format!("{} – {}", date_label(a, true, year), date_label(b, true, year && a.year() != b.year()))
                } else {
                    format!("{} – {}日", date_label(a, true, year), b.day())
                };
                vec![long, medium, format!("{} – {}", date_label(a, false, year), date_label(b, false, year))]
            }
            Mode::Timeline => {
                timeline_titles(local(self.t0), local(self.t0 + self.spp * self.plot_width(window) as f64))
            }
        }
    }

    /// 幅 `avail` に収まるよう、優先度の低い部品から短くする
    fn toolbar_plan(&self, titles: &[String], avail: f32) -> Plan {
        let fit = Fit {
            titles,
            timeline: self.mode == Mode::Timeline,
            sidebar_button: !self.sidebar_open,
            chip_name: self.focus.as_ref().map(|f| text_w(&self.project_name(f), 12.0) * 1.05),
        };
        fit.plan(avail)
    }

    /// ツールバーのアイコンだけの項目。説明とショートカットはツールチップに出す
    #[allow(clippy::too_many_arguments)]
    fn tool_icon(
        &self,
        id: &'static str,
        icon: Icon,
        tip_text: &'static str,
        key: Option<&'static str>,
        enabled: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(26.0))
            .h(px(ITEM_H))
            .rounded(px(4.0))
            .child(icons::icon(icon, if enabled { theme::MUTED } else { theme::SUBTLE }).size_4())
            .tooltip(tip(tip_text, key))
            .when(enabled, |d| {
                d.cursor_pointer().hover(|s| s.bg(c(theme::HOVER))).on_click(cx.listener(
                    move |t, _: &ClickEvent, w, cx| {
                        f(t, w, cx);
                        cx.notify();
                    },
                ))
            })
    }

    /// ツールバーの切り替え。選んだ項目を浮き出させ、ほかは押せる見た目にする
    pub(super) fn tool_segmented<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        items: &[Choice<T>],
        current: T,
        cx: &mut Context<Self>,
        f: fn(&mut Self, T, &mut Window, &mut Context<Self>),
    ) -> Div {
        tool_frame(theme::BACKDROP).children(items.iter().enumerate().map(|(i, item)| {
            let on = current == item.value;
            let v = item.value;
            div()
                .id(ElementId::NamedInteger(id.into(), i as u64))
                .flex()
                .flex_none()
                .items_center()
                .h(px(ITEM_H))
                .px_2()
                .rounded(px(4.0))
                .text_xs()
                .whitespace_nowrap()
                .text_color(c(if on { theme::TEXT } else { theme::MUTED }))
                .when(on, |d| d.bg(c(theme::ELEVATED)).font_weight(FontWeight::SEMIBOLD))
                .when(!on, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::HOVER)).text_color(c(theme::TEXT))))
                .child(item.label)
                .when(!item.tip.is_empty(), |d| d.tooltip(tip(item.tip, item.key)))
                .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    f(t, v, w, cx);
                    cx.notify();
                }))
        }))
    }

    /// 前 / 今日 / 次
    fn nav_group(&self, cx: &mut Context<Self>) -> Div {
        let timeline = self.mode == Mode::Timeline;
        tool_frame(theme::PANEL)
            .child(
                self.tool_icon("prev", Icon::ChevronLeft, "前の期間", Some("←"), true, cx, |t, w, _| {
                    t.step(-1.0, w)
                }),
            )
            .child(
                div()
                    .id("today")
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(ITEM_H))
                    .px_2()
                    .rounded(px(4.0))
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
            .child(
                self.tool_icon("next", Icon::ChevronRight, "次の期間", Some("→"), true, cx, |t, w, _| {
                    t.step(1.0, w)
                }),
            )
    }

    /// 表示中の期間。押すと期間（カレンダーでは移動先の日付）を選ぶポップオーバーを開く
    fn period_button(&self, title: String, icon: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let anchor = self.picker_anchor.clone();
        let open = self.popover == Some(Popover::Range);
        let timeline = self.mode == Mode::Timeline;
        let picked = timeline && self.pick.is_some();
        div()
            .id("period")
            .relative()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .h(px(CONTROL_H))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .when(open, |d| d.bg(c(theme::HOVER)))
            .hover(|s| s.bg(c(theme::HOVER)))
            .when(icon, |d| {
                d.child(icons::icon(
                    if timeline { Icon::CalendarRange } else { Icon::Calendar },
                    if open || picked { theme::ACCENT } else { theme::MUTED },
                ))
            })
            .child(div().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(icons::icon(Icon::ChevronDown, theme::MUTED).size_3())
            .child(canvas(move |b, _, _| anchor.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .tooltip(tip(if timeline { "表示する期間を選ぶ" } else { "日付を選んで移動" }, Some("G")))
            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.toggle_picker();
                cx.notify();
            }))
    }

    /// 期間を選ぶポップオーバーを開閉する
    pub(super) fn toggle_picker(&mut self) {
        if self.popover == Some(Popover::Range) {
            self.popover = None;
        } else {
            self.open_picker();
        }
    }

    /// 拡大 / 縮小。月の表示では使えないが、切り替えで並びが動かないよう枠は残す
    fn zoom_group(&self, cx: &mut Context<Self>) -> Div {
        let on = self.mode != Mode::Month;
        tool_frame(theme::PANEL)
            .child(self.tool_icon("zoom-out", Icon::ZoomOut, "縮小", Some("−"), on, cx, |t, w, _| t.zoom(2.0, w)))
            .child(self.tool_icon("zoom-in", Icon::ZoomIn, "拡大", Some("+"), on, cx, |t, w, _| t.zoom(0.5, w)))
    }

    /// イベント / API コストの切り替え
    pub(super) fn metric_switch(&self, full: bool, cx: &mut Context<Self>) -> Div {
        self.tool_segmented(
            "metric",
            &[
                Choice {
                    value: false, label: "イベント", tip: "ツール呼び出しやメッセージを時系列で表示", key: None
                },
                Choice {
                    value: true,
                    label: cost_label(full),
                    tip: "API 換算料金をセッション・ディレクトリ別に時系列で表示（USD）",
                    key: None,
                },
            ],
            self.cost_timeline,
            cx,
            |t, v, _, cx| {
                if t.cost_timeline != v {
                    t.toggle_cost_timeline(cx);
                }
            },
        )
    }

    /// 表示オプションの今の設定（ツールチップ用）
    fn display_summary(&self) -> String {
        let mut parts = vec![
            if self.cost_timeline { "API コスト" } else { "イベント" },
            if self.directory_timeline { "ディレクトリごと" } else { "セッションごと" },
        ];
        if !self.cost_timeline {
            parts.push(self.gran.label());
        }
        if self.in_view_only {
            parts.push("表示範囲内のセッションだけ");
        }
        format!("表示オプション（{}）", parts.join(" · "))
    }

    /// 表示オプション（表示する値・行のまとめ方・ブロックのまとめ方・範囲内のみ）のメニューを開くボタン
    fn display_button(&self, label: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let anchor = self.display_anchor.clone();
        let open = self.popover == Some(Popover::Display);
        div()
            .id("display-options")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(CONTROL_H))
            .px(px(if label { 8.0 } else { 6.0 }))
            .rounded_md()
            .border_1()
            .border_color(c(if open { theme::ACCENT } else { theme::BORDER }))
            .bg(c(if open { theme::ACCENT_BG } else { theme::PANEL }))
            .text_xs()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(Icon::SlidersHorizontal, if open { theme::ACCENT } else { theme::MUTED }))
            .when(label, |d| d.child("表示"))
            .child(icons::icon(Icon::ChevronDown, theme::MUTED).size_3())
            .child(canvas(move |b, _, _| anchor.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            // 範囲外のセッションを隠しているときは印を付け、メニューを開かなくても分かるようにする
            .when(self.in_view_only, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-4.0))
                        .right(px(-4.0))
                        .size(px(11.0))
                        .rounded_full()
                        .bg(c(theme::ACCENT))
                        .border_2()
                        .border_color(c(theme::BG)),
                )
            })
            .tooltip(tip(self.display_summary(), None))
            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.popover = if t.popover == Some(Popover::Display) { None } else { Some(Popover::Display) };
                cx.notify();
            }))
    }

    /// 注目中のプロジェクトを示し、×で絞り込みを解除するチップ
    pub(super) fn focus_chip(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        self.focus_chip_sized(Some(160.0), cx)
    }

    /// `name_max` が None なら名前を省き、頭文字の印だけにする
    fn focus_chip_sized(&self, name_max: Option<f32>, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let f = self.focus.as_ref()?;
        Some(
            div()
                .id("focus-chip")
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .pl_1()
                .pr(px(2.0))
                .h(px(CONTROL_H))
                .rounded_md()
                .bg(c(theme::ACCENT_BG))
                .border_1()
                .border_color(c(theme::ACCENT).opacity(0.45))
                .text_xs()
                .child(project_tile(f, 16.0))
                .when_some(name_max, |d, max| {
                    d.child(
                        div().max_w(px(max)).truncate().font_weight(FontWeight::SEMIBOLD).child(self.project_name(f)),
                    )
                })
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
                )
                .tooltip(tip(format!("{} のセッションだけを表示中", tilde(f)), None)),
        )
    }

    pub(super) fn header(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let timeline = self.mode == Mode::Timeline;
        let mut titles = self.titles(window);
        let plan = self.toolbar_plan(&titles, self.main_width(window));
        let title = titles.swap_remove(plan.title);
        let mut row = div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(if plan.dense { DENSE_GAP } else { ITEM_GAP }))
            .px(px(if plan.dense { DENSE_PAD } else { PAD }))
            .border_b_1()
            .border_color(c(theme::BORDER));
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
        // 右寄せは詰め物で行う（gap のある行で ml_auto を使うと gap が効かない）。
        // 絞り込みのチップは右側に置き、横移動で期間の表記の長さが変わっても動かないようにする
        row = row.child(self.nav_group(cx)).child(self.period_button(title, !plan.dense, cx)).child(div().flex_1());
        if let Some(chip) = self.focus_chip_sized(plan.chip, cx) {
            row = row.child(chip);
        }
        row = row.when(plan.zoom, |d| d.child(self.zoom_group(cx)));
        if timeline {
            row.when_some(plan.switch, |d, full| d.child(self.metric_switch(full, cx)))
                .child(self.display_button(plan.options_label, cx))
        } else {
            row.child(self.tool_segmented(
                "calmode",
                &[
                    Choice { value: Mode::Day, label: "日", tip: "1 日ずつ表示", key: Some("D") },
                    Choice { value: Mode::Week, label: "週", tip: "1 週間ずつ表示", key: Some("W") },
                    Choice { value: Mode::Month, label: "月", tip: "1 か月ずつ表示", key: Some("M") },
                ],
                self.mode,
                cx,
                |t, m, w, _| t.set_mode(m, w),
            ))
        }
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
        if self.mode != Mode::Timeline {
            legend = self.calendar_legend(room);
            room = 0.0;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn timeline_titles_shorten_from_the_weekday_and_keep_the_times() {
        let y = Local::now().year();
        let t = timeline_titles(at(y, 10, 7, 3, 39), at(y, 10, 7, 18, 40));
        let wd = weekday_ja(at(y, 10, 7, 0, 0).weekday());
        assert_eq!(t[0], format!("10月7日（{wd}） 03:39 – 18:40"));
        assert_eq!(t[1], format!("10/7（{wd}） 03:39 – 18:40"));
        assert_eq!(t[2], "10/7 03:39 – 18:40");
        assert_eq!(t[3], "03:39 – 18:40");
        // 長い順に並ぶ
        assert!(t.windows(2).all(|w| title_w(&w[0], true) >= title_w(&w[1], true)));
    }

    #[test]
    fn timeline_titles_use_dates_for_long_spans_and_years_outside_this_year() {
        let y = Local::now().year();
        // 午前 0 時で終わる期間は前日までと書く
        let t = timeline_titles(at(y, 10, 1, 0, 0), at(y, 10, 8, 0, 0));
        assert_eq!(t[2], "10/1 – 10/7");
        let t = timeline_titles(at(y - 1, 12, 30, 0, 0), at(y, 1, 2, 0, 0));
        assert_eq!(t[2], format!("{}/12/30 – {y}/1/1", y - 1));
        // 日をまたぐ短い期間は両方の日付を出し、最も短い表記では終わりを「翌」で表す
        let t = timeline_titles(at(y, 10, 6, 23, 25), at(y, 10, 7, 18, 48));
        assert_eq!(t[2], "10/6 23:25 – 10/7 18:48");
        assert_eq!(t[3], "23:25 – 翌18:48");
        // 短い期間は秒まで出す
        let t = timeline_titles(at(y, 10, 7, 3, 39), at(y, 10, 7, 3, 40));
        assert!(t[3].ends_with(":00 – 03:40:00"));
    }

    /// 確認に使う期間の表記（同じ日・日をまたぐ・数日・カレンダーの週）
    fn sample_titles() -> Vec<(bool, Vec<String>)> {
        let y = Local::now().year();
        vec![
            (true, timeline_titles(at(y, 10, 7, 3, 39), at(y, 10, 7, 18, 40))),
            (true, timeline_titles(at(y, 10, 6, 23, 25), at(y, 10, 7, 18, 48))),
            (true, timeline_titles(at(y, 9, 8, 0, 0), at(y, 10, 8, 0, 0))),
            (false, vec![format!("{y}年10月5日 – 11日"), "10月5日 – 11日".into(), "10/5 – 10/11".into()]),
            (false, vec![format!("{y}年10月7日（水）"), "10月7日（水）".into(), "10/7（水）".into()]),
        ]
    }

    #[test]
    fn toolbar_keeps_every_control_when_wide() {
        for (timeline, titles) in sample_titles() {
            let chip_name = Some(text_w("rireki", 12.0) * 1.05);
            let fit = Fit { titles: &titles, timeline, sidebar_button: true, chip_name };
            let p = fit.plan(1200.0);
            let all =
                Plan { title: 0, chip: Some(160.0), zoom: true, switch: Some(true), options_label: true, dense: false };
            assert_eq!(p, all, "{titles:?}");
        }
    }

    #[test]
    fn toolbar_fits_the_main_column_and_only_shrinks_as_it_narrows() {
        for (timeline, titles) in sample_titles() {
            for (sidebar_button, chip_name) in [(false, None), (true, Some(text_w("rireki", 12.0) * 1.05))] {
                let fit = Fit { titles: &titles, timeline, sidebar_button, chip_name };
                // メイン列の最小幅でも、期間の表記を省かずに収まる
                let narrowest = fit.plan(super::super::MAIN_MIN_W);
                if !sidebar_button {
                    assert!(fit.width(&narrowest) <= super::super::MAIN_MIN_W, "{titles:?} {narrowest:?}");
                }
                let mut prev: Option<Plan> = None;
                let mut avail = 1400.0;
                while avail >= 300.0 {
                    let p = fit.plan(avail);
                    assert!(fit.width(&p) <= avail || p.dense, "{avail} {p:?}");
                    // タイムラインでは、拡大縮小を表示の切り替えより後まで残す
                    assert!(!timeline || p.zoom || p.switch.is_none(), "{avail} {p:?}");
                    if let Some(q) = prev {
                        // 狭くするほど表記は短くなり、一度隠した部品は戻らない
                        assert!(
                            p.title >= q.title && (q.zoom || !p.zoom) && (q.switch.is_some() || p.switch.is_none())
                        );
                        assert!((q.options_label || !p.options_label) && (!q.dense || p.dense));
                    }
                    prev = Some(p);
                    avail -= 4.0;
                }
            }
        }
    }
}
