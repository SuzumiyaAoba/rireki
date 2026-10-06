//! 重ねて出すもの: ブロックのホバーカード、まとめ方のメニュー、キーボードショートカット一覧

use gpui::{
    AnyElement, ClickEvent, Context, Corner, Div, FontWeight, MouseButton, MouseDownEvent, SharedString, Window, anchored, deferred,
    div, point, prelude::*, px,
};

use super::widgets::{Marquee, event_icon};
use super::{Popover, Rireki, fmt_dur, local};
use crate::icons::{self, Icon};
use crate::layout::{self, Block, Granularity};
use crate::model::one_line;
use crate::theme::{self, c};

impl Rireki {
    pub(super) fn overlays(&self, _: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        match self.popover {
            Some(Popover::Granularity) => {
                out.push(self.backdrop(cx).into_any_element());
                out.push(self.gran_menu(cx));
            }
            Some(Popover::Range) => {
                out.push(self.backdrop(cx).into_any_element());
                out.push(self.range_menu(cx));
            }
            Some(Popover::Help) => out.push(self.help_sheet(cx)),
            None => out.extend(self.hover_card()),
        }
        out
    }

    /// ポップオーバーの外側。クリックで閉じ、下の要素には届かせない
    fn backdrop(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let close = |t: &mut Self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
            t.popover = None;
            cx.notify();
        };
        div()
            .id("backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(close))
            .on_mouse_down(MouseButton::Right, cx.listener(close))
    }

    /// ブロックにマウスを重ねたときのカード。イベント名・要約・時刻・セッションを出す
    fn hover_card(&self) -> Option<AnyElement> {
        let sel = self.hovered.filter(|&s| self.drag.is_none() && self.valid(s))?;
        let s = &self.store.sessions[sel.session];
        let lane = &s.lanes[sel.lane];
        let evs = &lane.events[sel.first..=sel.last];
        let b = Block { start: 0.0, end: 0.0, first: sel.first, last: sel.last };
        let dom = &lane.events[layout::dominant(&lane.events, &b)];
        let (color, ic) = (theme::event_color(dom), theme::event_icon(dom));
        let (start, end) = self.sel_range(sel);
        let single = evs.len() == 1;
        let head = if single { evs[0].name.to_string() } else { format!("{} 件", evs.len()) };
        let body: SharedString = if single { evs[0].summary.clone().into() } else { SharedString::default() };
        // まとめたブロックは、多い順に名前ごとの件数を並べる
        let mut counts: Vec<(&str, usize, u32, Icon)> = Vec::new();
        if !single {
            for e in evs {
                match counts.iter_mut().find(|(n, ..)| *n == e.name.as_ref()) {
                    Some(x) => x.1 += 1,
                    None => counts.push((&e.name, 1, theme::event_color(e), theme::event_icon(e))),
                }
            }
            counts.sort_by_key(|a| std::cmp::Reverse(a.1));
        }
        let session = if s.title.is_empty() { "(無題)".to_string() } else { one_line(&s.title, usize::MAX) };
        let session = if sel.lane == 0 { session } else { format!("{session} › {}", one_line(&lane.name, usize::MAX)) };
        let card = div()
            .id(SharedString::from(format!("hover-card-{}-{}-{}-{}", sel.session, sel.lane, sel.first, sel.last)))
            .w(px(320.0))
            .flex()
            .flex_col()
            .gap_1()
            .px_2p5()
            .py_2()
            .rounded_lg()
            .bg(c(theme::ELEVATED))
            .border_1()
            .border_color(c(theme::BORDER))
            .shadow_lg()
            .text_xs()
            .text_color(c(theme::TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(event_icon(ic, color, 14.0))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(color)).child(head))
                    .child(div().flex_1())
                    .when(end > start, |d| d.child(div().text_color(c(theme::MUTED)).child(fmt_dur(end - start)))),
            )
            .when(!body.is_empty(), |d| d.child(Marquee::new("hover-body", one_line(&body, usize::MAX)).on_show()))
            .when(!counts.is_empty(), |d| {
                d.child(div().flex().flex_wrap().gap_x_2().gap_y_0p5().children(counts.iter().take(6).map(
                    |&(name, n, col, ic)| {
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(event_icon(ic, col, 11.0))
                            .child(name.to_string())
                            .child(div().text_color(c(theme::SUBTLE)).child(format!("×{n}")))
                    },
                )))
            })
            .child(div().text_color(c(theme::MUTED)).child(format!(
                "{} – {}",
                local(start).format("%m/%d %H:%M:%S"),
                local(end).format("%H:%M:%S")
            )))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(c(theme::SUBTLE))
                    .child(icons::icon(icons::source_icon(s.source).0, theme::SUBTLE).size_3())
                    .child(div().flex_1().min_w_0().child(Marquee::new("hover-session", session).on_show())),
            );
        Some(
            deferred(anchored().position(self.hover_at + point(px(14.0), px(18.0))).child(card))
                .with_priority(1)
                .into_any_element(),
        )
    }

    fn gran_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let b = self.gran_anchor.get();
        let items = Granularity::ALL.into_iter().map(|g| {
            let on = self.gran == g;
            div()
                .id(g.label())
                .flex()
                .items_start()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::HOVER)))
                .child(
                    div()
                        .w(px(14.0))
                        .pt(px(2.0))
                        .flex_none()
                        .when(on, |d| d.child(icons::icon(Icon::Check, theme::ACCENT))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(g.label()))
                        .child(div().text_xs().text_color(c(theme::MUTED)).child(g.description())),
                )
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    t.gran = g;
                    t.popover = None;
                    t.persist();
                    cx.notify();
                }))
        });
        deferred(
            anchored().position(point(b.right(), b.bottom() + px(4.0))).anchor(Corner::TopRight).child(
                div()
                    .id("gran-menu")
                    .occlude()
                    .w(px(300.0))
                    .p_1()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .bg(c(theme::ELEVATED))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .shadow_lg()
                    .text_color(c(theme::TEXT))
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .pb_1p5()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(theme::MUTED))
                            .child("ブロックのまとめ方"),
                    )
                    .children(items),
            ),
        )
        .with_priority(2)
        .into_any_element()
    }

    fn help_sheet(&self, cx: &mut Context<Self>) -> AnyElement {
        const GROUPS: &[(&str, &[(&str, &str)])] = &[
            (
                "表示",
                &[
                    ("⌘1", "タイムライン"),
                    ("⌘2 · D", "日"),
                    ("⌘3 · W", "週"),
                    ("⌘4", "統計"),
                    ("⌃⌘S · ⌘B", "サイドバーの表示を切り替え"),
                    ("V", "表示範囲内のセッションだけを表示"),
                ],
            ),
            (
                "移動",
                &[
                    ("⌘T · T", "今日（現在時刻）へ"),
                    ("⌘← ⌘→ · ← →", "前 / 次の期間"),
                    ("⌘= ⌘- · + −", "拡大 / 縮小"),
                    ("ドラッグ · スクロール", "表示範囲を動かす"),
                    ("⌘ + スクロール · ピンチ", "マウス位置を中心に拡大縮小"),
                    ("ダブルクリック", "ブロックの範囲まで拡大"),
                ],
            ),
            (
                "選択",
                &[
                    ("クリック", "詳細を開く"),
                    ("J · K", "次 / 前のイベント"),
                    ("1 · 2 · 3 · 4", "詳細のタブ（概要 / 履歴 / 変更 / 分析）"),
                    ("⇧⌘↵", "詳細の全画面表示 / 通常表示"),
                    ("⌘F（分析タブ）", "選択範囲のイベントを検索"),
                    ("⌘[ ⌘]", "選択の履歴を戻る / 進む"),
                    ("esc", "全画面を解除 / 詳細を閉じる"),
                ],
            ),
            ("検索", &[("⌘K · ⌘F", "検索欄へ"), ("↑ ↓", "結果を選ぶ"), ("enter", "結果を開く"), ("esc", "検索を解除")]),
        ];
        let group = |title: &'static str, rows: &'static [(&'static str, &'static str)]| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::MUTED)).mb_1().child(title),
                )
                .children(rows.iter().map(|&(k, v)| {
                    div().flex().items_center().gap_3().text_xs().child(div().flex_1().child(v)).child(
                        div()
                            .flex_none()
                            .px_1p5()
                            .rounded_sm()
                            .bg(c(theme::BG))
                            .border_1()
                            .border_color(c(theme::BORDER))
                            .text_color(c(theme::MUTED))
                            .child(k),
                    )
                }))
        };
        let close = |t: &mut Self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
            t.popover = None;
            cx.notify();
        };
        div()
            .id("help-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(close))
            .child(
                div()
                    .id("help")
                    .occlude()
                    .w(px(620.0))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .rounded_xl()
                    .bg(c(theme::ELEVATED))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .shadow_lg()
                    .text_color(c(theme::TEXT))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icons::icon(Icon::Keyboard, theme::MUTED).size_4())
                            .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("キーボードショートカット"))
                            .child(
                                div()
                                    .id("help-close")
                                    .p_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(c(theme::HOVER)))
                                    .child(icons::icon(Icon::X, theme::MUTED))
                                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                        t.popover = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_6()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .child(group(GROUPS[0].0, GROUPS[0].1))
                                    .child(group(GROUPS[2].0, GROUPS[2].1)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .child(group(GROUPS[1].0, GROUPS[1].1))
                                    .child(group(GROUPS[3].0, GROUPS[3].1)),
                            ),
                    ),
            )
            .into_any_element()
    }
}
