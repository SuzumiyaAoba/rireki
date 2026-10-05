//! 統計画面。本文を読み込まず、集計は背景で行いデータと期間が変わったときだけ更新する
//!
//! - 見出し: 期間（7 日・30 日・90 日・全期間）とプロジェクトの絞り込み
//! - 指標のタイル: 値・前の期間との差・読み込み元の割合。押すと推移・時間帯・プロジェクトの指標が変わる
//! - 推移と時間帯 (`charts.rs`): 読み込み元ごとに積んだ列（押すとその期間をタイムラインで開く）と、曜日 × 時刻の濃淡
//! - 一覧 (`lists.rs`): ツール・プロジェクト・モデル（キャッシュ読込率）と、読み込み元の比較表

mod charts;
mod lists;
mod profile;
use crate::statistics::{Amount, Cache, Period, Summary, Totals};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{Local, NaiveDate};
use gpui::{
    ClickEvent, Context, Div, ElementId, FontWeight, ScrollHandle, SharedString, Stateful, Window, div, point,
    prelude::*, px, relative,
};

use super::detail::{fmt_tokens, fmt_usd};
use super::widgets::tip;
use super::{HEADER_H, Rireki, fmt_n, tilde};
use crate::icons::{self, Icon};
use crate::model::UsageSum;
use crate::prefs::SourceFilter;
use crate::theme::{self, c};

/// 内容の外側の余白とカードの間隔
const PAD: f32 = 20.0;
const GAP: f32 = 14.0;
/// 2 列に並べる最小の内容幅
const WIDE: f32 = 900.0;

/// タイルで選び、推移・時間帯・プロジェクトで表す指標
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Metric {
    Tokens,
    Cost,
    Turns,
    Sessions,
    Tools,
}

impl Metric {
    const ALL: [Metric; 5] = [Metric::Tokens, Metric::Cost, Metric::Turns, Metric::Sessions, Metric::Tools];

    fn label(self) -> &'static str {
        match self {
            Self::Tokens => "トークン",
            Self::Cost => "推定料金",
            Self::Turns => "ターン",
            Self::Sessions => "セッション",
            Self::Tools => "ツール呼び出し",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::Tokens => "入力・キャッシュ・出力の合計",
            Self::Cost => "ログの使用量と料金表による推定（USD）。料金不明のモデルは含まない",
            Self::Turns => "ユーザーの発話の数",
            Self::Sessions => "イベントのあるセッションの数。サブエージェントは起動元と合わせて 1 件",
            Self::Tools => "ツール呼び出し（サブエージェントの起動を含む）の数",
        }
    }

    /// 値に添える単位
    fn unit(self) -> &'static str {
        match self {
            Self::Tokens => " トークン",
            Self::Cost => "",
            Self::Turns => " ターン",
            Self::Sessions => " セッション",
            Self::Tools => " 回",
        }
    }

    fn of(self, t: &Totals) -> f64 {
        match self {
            Self::Tokens => t.usage.total() as f64,
            Self::Cost => t.usage.cost,
            Self::Turns => t.turns as f64,
            Self::Sessions => t.sessions as f64,
            Self::Tools => t.tools as f64,
        }
    }

    fn amount(self, a: &Amount) -> f64 {
        match self {
            Self::Tokens => a.tokens as f64,
            Self::Cost => a.cost,
            Self::Turns => a.turns as f64,
            Self::Sessions => a.sessions as f64,
            Self::Tools => a.tools as f64,
        }
    }

    fn format(self, v: f64) -> String {
        match self {
            Self::Tokens => fmt_tokens(v.round() as u64),
            Self::Cost => fmt_usd(v),
            _ => fmt_n(v.round() as usize),
        }
    }

    /// タイルの大きな数字。料金は 100 ドル以上ならセントを省く
    fn headline(self, v: f64) -> String {
        match self {
            Self::Cost if v >= 100.0 => fmt_usd(v.round()).trim_end_matches(".00").to_string(),
            _ => self.format(v),
        }
    }

    /// 目盛りの短い表示
    fn tick(self, v: f64) -> String {
        match self {
            Self::Cost if v >= 1000.0 => format!("${}", short(v)),
            Self::Cost if v.fract() == 0.0 => format!("${v:.0}"),
            Self::Cost => format!("${v:.2}").trim_end_matches('0').to_string(),
            _ => short(v),
        }
    }

    /// 件数を数える指標か（目盛りを整数にする）
    fn count(self) -> bool {
        !matches!(self, Self::Cost)
    }
}

/// 要求した集計の条件 (data_gen, 期間, 今日)
type Key = (u64, Period, NaiveDate);

/// ポインタを重ねているグラフの位置
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hover {
    Bucket(usize),
    /// 時間帯のマス（曜日 × 24 + 時）
    Hour(usize),
    /// 曜日ごと・時ごとの合計
    Weekday(usize),
    HourOfDay(usize),
}

pub(super) struct State {
    period: Period,
    metric: Metric,
    scroll: ScrollHandle,
    /// 要求中の集計の条件
    key: Option<Key>,
    /// 表示中の集計の条件。データの自動更新では前の結果の明るさを保つ
    shown: Option<Key>,
    /// 表示中の集計に使った読み込み元とプロジェクト
    shown_filters: Option<(SourceFilter, Option<String>)>,
    task: Option<gpui::Task<()>>,
    summary: Option<Summary>,
    cache: Arc<Mutex<Cache>>,
    generation: Arc<AtomicU64>,
    hover: Option<Hover>,
    /// プロジェクトを上位だけでなくすべて並べる
    all_projects: bool,
    /// 推移と時間帯を描いた位置（当たり判定用）
    trend_area: charts::Area,
    hours_area: charts::Area,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            period: Period::Month,
            metric: Metric::Tokens,
            scroll: ScrollHandle::new(),
            key: None,
            shown: None,
            shown_filters: None,
            task: None,
            summary: None,
            cache: Arc::new(Mutex::new(Cache::default())),
            generation: Arc::new(AtomicU64::new(0)),
            hover: None,
            all_projects: false,
            trend_area: Default::default(),
            hours_area: Default::default(),
        }
    }

    fn dimmed(&self, source: SourceFilter, focus: Option<&str>) -> bool {
        match (self.shown, self.key, &self.shown_filters) {
            (Some(shown), Some(key), Some((shown_source, shown_focus))) => {
                shown.1 != key.1 || shown.2 != key.2 || *shown_source != source || shown_focus.as_deref() != focus
            }
            _ => false,
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl Rireki {
    pub(super) fn show_statistics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.statistics_open = true;
        self.statistics_state.hover = None;
        self.popover = None;
        self.hovered = None;
        self.drag = None;
        self.resize = None;
        window.focus(&self.focus_handle);
        self.persist();
        cx.notify();
    }

    fn request_statistics(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        let key = (self.data_gen, self.statistics_state.period, today);
        if self.statistics_state.key == Some(key) {
            return;
        }
        // 前の結果は集計が終わるまで保持し、データ更新のたびに明るさを変えない。
        self.statistics_state.key = Some(key);
        let filters = (self.source, self.focus.clone());
        let snapshot = self.store.clone();
        let order = self.order.clone();
        let cache = self.statistics_state.cache.clone();
        let generation = self.statistics_state.generation.clone();
        let version = generation.fetch_add(1, Ordering::Relaxed) + 1;
        let profiling = self.profile.is_some();
        self.statistics_state.task = Some(cx.spawn(async move |this, cx| {
            let summary = cx
                .background_executor()
                .spawn(async move {
                    let started = profiling.then(std::time::Instant::now);
                    let mut cache = cache.lock().unwrap();
                    let summary = cache
                        .build_while(&snapshot, &order, key.1, today, || generation.load(Ordering::Relaxed) == version);
                    if let Some(started) = started {
                        println!(
                            "{}",
                            serde_json::json!({"ui_profile":"statistics_build", "version":version,
                            "completed":summary.is_some(), "rebuilt":cache.rebuilt,
                            "ms":started.elapsed().as_secs_f64()*1000.0})
                        );
                    }
                    summary
                })
                .await;
            let _ = this.update(cx, |t, cx| {
                let state = &mut t.statistics_state;
                if let Some(summary) = summary
                    && state.key == Some(key)
                    && state.generation.load(Ordering::Relaxed) == version
                {
                    state.summary = Some(summary);
                    state.shown = Some(key);
                    state.shown_filters = Some(filters);
                    cx.notify();
                }
            });
        }));
    }

    /// 期間を変える。見出しは固定なので、スクロール位置は保って同じ一覧を見比べられるようにする
    fn set_statistics_period(&mut self, period: Period) {
        self.statistics_state.period = period;
        self.statistics_state.hover = None;
    }

    pub(super) fn statistics(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        self.request_statistics(cx);
        let width = self.main_width(window);
        let inner = width - PAD * 2.0;
        let body = match &self.statistics_state.summary {
            Some(summary) => self.statistics_body(summary, inner, cx),
            None => div()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .py_16()
                .text_sm()
                .text_color(c(theme::MUTED))
                .child(icons::icon(Icon::Refresh, theme::ACCENT).size_4())
                .child("統計を集計中…"),
        };
        div().flex_1().min_w_0().h_full().flex().flex_col().child(self.statistics_header(width, cx)).child(
            div()
                .id("statistics-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.statistics_state.scroll)
                .child(body),
        )
    }

    /// 期間とプロジェクトの絞り込み。下のすべての数字はこの範囲で数える
    fn statistics_header(&self, width: f32, cx: &mut Context<Self>) -> Div {
        let source = match self.source {
            SourceFilter::All => "Claude Code と Codex",
            SourceFilter::Claude => "Claude Code",
            SourceFilter::Codex => "Codex",
        };
        let range = self
            .statistics_state
            .summary
            .as_ref()
            .map(|s| {
                format!(
                    "{} · 稼働 {} / {} 日 · ",
                    date_range(s.from, s.to),
                    fmt_n(s.active_days),
                    fmt_n(s.days() as usize)
                )
            })
            .unwrap_or_default();
        let periods = self.segmented(
            "statistics-period",
            &[(Period::Week, "7日間"), (Period::Month, "30日間"), (Period::Quarter, "90日間"), (Period::All, "全期間")],
            Some(self.statistics_state.period),
            cx,
            |t, period, _, _| t.set_statistics_period(period),
        );
        div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_sm()
            .child(icons::icon(Icon::Chart, theme::ACCENT).size_4())
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child("統計"))
            .children(self.focus_chip(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .when(width >= 640.0, |d| d.child(format!("{range}{source}"))),
            )
            .child(periods)
    }

    fn statistics_body(&self, s: &Summary, inner: f32, cx: &mut Context<Self>) -> Div {
        let wide = inner >= WIDE;
        let stale = self.statistics_state.dimmed(self.source, self.focus.as_deref());
        let pair = |a: Div, b: Div| {
            div()
                .flex()
                .gap(px(GAP))
                .when(!wide, |d| d.flex_col())
                .child(a.flex_1().flex_basis(relative(0.0)))
                .child(b.flex_1().flex_basis(relative(0.0)))
        };
        let half = if wide { (inner - GAP) / 2.0 } else { inner };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .p(px(PAD))
            .when(stale, |d| d.opacity(0.55))
            .when(self.loading.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(c(theme::MUTED))
                        .child(icons::icon(Icon::Refresh, theme::ACCENT).size_3())
                        .child("履歴を読み込み中です。読み込み済みの分だけを集計しています"),
                )
            })
            .child(self.metric_tiles(s, inner, cx));
        if s.total.sessions == 0 {
            body = body.child(
                card()
                    .items_center()
                    .py_12()
                    .gap_2()
                    .child(icons::icon(Icon::Chart, theme::SUBTLE).size_6())
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("この期間の履歴はありません"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child("期間を広げるか、左の読み込み元やプロジェクトの絞り込みを切り替えてください"),
                    ),
            );
        } else {
            body = body
                .child(self.trend_card(s, inner, cx))
                .child(pair(self.hours_card(s, half, cx), self.tools_card(s)))
                .child(pair(self.projects_card(s, cx), self.models_card(s)))
                .child(self.sources_card(s, inner));
        }
        body.child(self.statistics_notes(s))
    }

    /// 指標のタイル。選んだ指標を下のグラフと一覧で表す
    fn metric_tiles(&self, s: &Summary, inner: f32, cx: &mut Context<Self>) -> Div {
        // 5 枚が並ばない幅では 3 枚ずつにする
        let basis = if inner >= 820.0 { 0.18 } else { 0.3 };
        div().flex().flex_wrap().gap(px(GAP)).children(
            Metric::ALL
                .into_iter()
                .map(|metric| self.metric_tile(metric, s, cx).flex_1().flex_basis(relative(basis)).min_w(px(150.0))),
        )
    }

    fn metric_tile(&self, metric: Metric, s: &Summary, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self.statistics_state.metric == metric;
        let value = metric.of(&s.total);
        let comparison = match &s.previous {
            Some(previous) => {
                let before = metric.of(previous);
                let label = format!("前の{}日間 {}", s.days(), metric.format(before));
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x_1p5()
                    .child(div().flex_none().text_color(c(theme::TEXT)).child(delta(value, before)))
                    .child(div().text_color(c(theme::MUTED)).child(label))
            }
            None => {
                let days = s.active_days.max(1) as f64;
                div().text_color(c(theme::MUTED)).child(format!("稼働日あたり {}", metric.format(value / days)))
            }
        };
        let both = self.source == SourceFilter::All;
        let split = [metric.of(&s.sources[0]), metric.of(&s.sources[1])];
        div()
            .id(ElementId::NamedInteger("statistics-metric".into(), metric as u64))
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .pt_3()
            .pb(px(14.0))
            .rounded_lg()
            .border_1()
            .cursor_pointer()
            .when(selected, |d| d.bg(c(theme::ACCENT_BG).opacity(0.6)).border_color(c(theme::ACCENT)))
            .when(!selected, |d| {
                d.bg(c(theme::PANEL)).border_color(c(theme::BORDER)).hover(|d| d.border_color(c(theme::SUBTLE)))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(c(if selected { theme::TEXT } else { theme::MUTED }))
                    .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(metric.label()),
            )
            .child(
                div()
                    .text_size(px(24.0))
                    .line_height(px(30.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(metric.headline(value)),
            )
            .child(div().text_xs().child(comparison))
            .when(both && value > 0.0, |d| d.child(split_bar(split).mt_1()))
            .tooltip(tip(
                if both && value > 0.0 {
                    format!(
                        "{}\nClaude {} ({}) · Codex {} ({})",
                        metric.help(),
                        metric.format(split[0]),
                        percent(split[0] / value),
                        metric.format(split[1]),
                        percent(split[1] / value)
                    )
                } else {
                    metric.help().to_string()
                },
                None,
            ))
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                t.statistics_state.metric = metric;
                t.statistics_state.hover = None;
                cx.notify();
            }))
    }

    /// 集計の前提と、料金・使用量の記録の欠け
    fn statistics_notes(&self, s: &Summary) -> Div {
        let usage = &s.total.usage;
        div()
            .flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .pt_1()
            .text_xs()
            .text_color(c(theme::MUTED))
            .child("ローカル時間で集計 · サブエージェントを含む")
            .when(s.total.sessions > 0, |d| {
                d.child(format!(
                    "使用量の記録があるセッション {} / {}",
                    fmt_n(s.recorded_sessions),
                    fmt_n(s.total.sessions)
                ))
            })
            .when(usage.unpriced > 0, |d| {
                d.child(
                    div()
                        .id("statistics-unpriced")
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icons::icon(Icon::Activity, theme::WARN).size_3())
                        .child(format!(
                            "料金不明のモデルの呼び出し {} 回は推定料金に含めていません",
                            fmt_n(usage.unpriced as usize)
                        ))
                        .tooltip(tip(
                            format!(
                                "料金表にないモデル。{} に追加できます",
                                tilde(&crate::pricing::path().to_string_lossy())
                            ),
                            None,
                        )),
                )
            })
    }
}

/// カードの枠
fn card() -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_lg()
        .bg(c(theme::PANEL))
        .border_1()
        .border_color(c(theme::BORDER))
}

/// カードの見出しと補足の行。右に凡例や読み取り値を `child` で足す
fn card_title(title: impl Into<SharedString>, note: impl Into<SharedString>) -> Div {
    div().flex().items_start().gap_x_4().gap_y_2().child(
        div()
            .flex_1()
            .min_w(px(160.0))
            .flex()
            .flex_col()
            .gap_0p5()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title.into()))
            .child(div().text_xs().text_color(c(theme::MUTED)).truncate().child(note.into())),
    )
}

/// 読み込み元ごとの色
fn series(source: usize) -> u32 {
    if source == 0 { theme::SERIES_CLAUDE } else { theme::SERIES_CODEX }
}

/// 凡例の色見本
fn swatch(color: u32) -> Div {
    div().flex_none().size(px(8.0)).rounded(px(2.0)).bg(c(color))
}

/// 読み込み元の割合の帯。境目は面の色の隙間で分ける
fn split_bar(values: [f64; 2]) -> Div {
    let total = values[0] + values[1];
    div().flex().gap(px(2.0)).h(px(4.0)).w_full().children(
        values.into_iter().enumerate().filter(|(_, v)| *v > 0.0).map(move |(i, v)| {
            div()
                .h_full()
                .flex_basis(relative((v / total) as f32))
                .flex_shrink()
                .min_w(px(2.0))
                .rounded(px(2.0))
                .bg(c(series(i)))
        }),
    )
}

/// 前の期間との差（▲ 12% / ▼ 3.4% / ±0%）。増減の良し悪しは決めつけない
fn delta(now: f64, before: f64) -> String {
    if before <= 0.0 {
        return if now > 0.0 { "新規".into() } else { "—".into() };
    }
    let r = (now - before) / before * 100.0;
    let digits = if r.abs() < 10.0 { 1 } else { 0 };
    match r {
        r if r.abs() < 0.05 => "±0%".into(),
        r if r > 0.0 => format!("▲ {r:.digits$}%"),
        r => format!("▼ {:.digits$}%", -r),
    }
}

/// 割合（12% / 2.4% / <0.1%）
fn percent(r: f64) -> String {
    let p = r * 100.0;
    match p {
        p if p <= 0.0 => "0%".into(),
        p if p < 0.1 => "<0.1%".into(),
        p if p < 10.0 => format!("{p:.1}%"),
        p => format!("{p:.0}%"),
    }
}

/// 目盛り用の短い数（950 / 1.2k / 35k / 1.5M / 2B）
fn short(v: f64) -> String {
    let (n, unit) = match v.abs() {
        a if a >= 1e9 => (v / 1e9, "B"),
        a if a >= 1e6 => (v / 1e6, "M"),
        a if a >= 1e3 => (v / 1e3, "k"),
        _ => return format!("{}", (v * 100.0).round() / 100.0),
    };
    let s = if n.abs() >= 10.0 || n.fract() == 0.0 { format!("{n:.0}") } else { format!("{n:.1}") };
    format!("{}{unit}", s.trim_end_matches(".0"))
}

/// 期間の表示（2026/9/6 – 10/5）
fn date_range(from: NaiveDate, to: NaiveDate) -> String {
    use chrono::Datelike;
    if from == to {
        to.format("%Y/%-m/%-d").to_string()
    } else if from.year() == to.year() {
        format!("{} – {}", from.format("%Y/%-m/%-d"), to.format("%-m/%-d"))
    } else {
        format!("{} – {}", from.format("%Y/%-m/%-d"), to.format("%Y/%-m/%-d"))
    }
}

/// 料金。料金の分かる呼び出しがなければ「—」
fn cost_label(u: &UsageSum) -> String {
    if u.calls > u.unpriced { fmt_usd(u.cost) } else { "—".into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_usage_has_no_cost_label() {
        assert_eq!(cost_label(&UsageSum::default()), "—");
    }

    #[test]
    fn deltas_and_ratios_are_short_and_neutral() {
        assert_eq!(delta(112.0, 100.0), "▲ 12%");
        assert_eq!(delta(96.6, 100.0), "▼ 3.4%");
        assert_eq!(delta(100.0, 100.0), "±0%");
        assert_eq!(delta(5.0, 0.0), "新規");
        assert_eq!(delta(0.0, 0.0), "—");
        assert_eq!(percent(0.973), "97%");
        assert_eq!(percent(0.024), "2.4%");
        assert_eq!(percent(0.0004), "<0.1%");
        assert_eq!(short(1_200.0), "1.2k");
        assert_eq!(short(35_000.0), "35k");
        assert_eq!(short(2_000_000_000.0), "2B");
        assert_eq!(short(250.0), "250");
        assert_eq!(Metric::Cost.tick(0.5), "$0.5");
        assert_eq!(Metric::Cost.tick(1500.0), "$1.5k");
        assert_eq!(Metric::Cost.headline(7435.09), "$7,435");
        assert_eq!(Metric::Cost.headline(12.345), "$12.35");
    }
}
