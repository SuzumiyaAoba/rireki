//! 重ねて出すもの: ブロックのホバーカード、表示オプションのメニュー、キーボードショートカット一覧

use gpui::{
    AnyElement, ClickEvent, Context, Corner, Div, FontWeight, MouseButton, MouseDownEvent, SharedString, Window, anchored, deferred,
    div, point, prelude::*, px,
};

use super::header::Choice;
use super::widgets::{Marquee, event_icon, kbd};
use super::{Popover, Rireki, fmt_dur, local};
use crate::icons::{self, Icon};
use crate::layout::{self, Block, Granularity};
use crate::model::one_line;
use crate::theme::{self, c};

impl Rireki {
    pub(super) fn overlays(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        match self.popover {
            Some(Popover::Display) => {
                out.push(self.backdrop(cx).into_any_element());
                out.push(self.display_menu(cx));
            }
            Some(Popover::Range) => {
                out.push(self.backdrop(cx).into_any_element());
                out.push(self.range_menu(window, cx));
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

    /// 表示オプション。表示する値・行のまとめ方・ブロックのまとめ方・範囲内のみをまとめて切り替える。
    /// 結果を見ながら続けて変えられるよう、選んでも閉じない
    fn display_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let b = self.display_anchor.get();
        let cost = self.cost_timeline;
        let heading = |text: &'static str| {
            div()
                .px_2()
                .pt_1()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(theme::MUTED))
                .child(text)
        };
        let note = |text: &'static str| div().px_2().pb_1().text_xs().text_color(c(theme::MUTED)).child(text);
        // 見出しと切り替えを横に並べた行
        let labeled = |label: &'static str, control: Div| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .child(div().w(px(72.0)).flex_none().text_xs().text_color(c(theme::MUTED)).child(label))
                .child(control)
        };
        let divider = || div().mx_2().my_1p5().h(px(1.0)).bg(c(theme::BORDER));
        let metric = self.tool_segmented(
            "display-metric",
            &[
                Choice { value: false, label: "イベント", tip: "", key: None },
                Choice { value: true, label: "API コスト", tip: "", key: None },
            ],
            cost,
            cx,
            |t, v, _, cx| {
                if t.cost_timeline != v {
                    t.toggle_cost_timeline(cx);
                    t.popover = Some(Popover::Display);
                }
            },
        );
        let rows = self.tool_segmented(
            "display-rows",
            &[
                Choice { value: false, label: "セッション", tip: "", key: None },
                Choice { value: true, label: "ディレクトリ", tip: "", key: None },
            ],
            self.directory_timeline,
            cx,
            |t, v, w, cx| {
                if t.directory_timeline != v {
                    t.toggle_directory_timeline(w, cx);
                }
            },
        );
        let metric_note = if cost {
            "区間ごとの API 換算料金（USD）を棒で表示"
        } else {
            "ツール呼び出しやメッセージをブロックで表示"
        };
        let rows_note = match (self.directory_timeline, cost) {
            (false, _) => "セッションとサブエージェントを 1 行ずつ並べる",
            (true, false) => "メインのイベントだけを詰めて並べる",
            (true, true) => "ディレクトリごとに全セッションの料金を合計",
        };
        // ブロックのまとめ方は、API コストの表示では使わない
        let grans =
            Granularity::ALL.into_iter().map(|g| {
                let on = self.gran == g;
                div()
                    .id(g.label())
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(!cost, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::HOVER))))
                    .child(div().w(px(14.0)).pt(px(1.0)).flex_none().when(on, |d| {
                        d.child(icons::icon(Icon::Check, if cost { theme::SUBTLE } else { theme::ACCENT }))
                    }))
                    .child(
                        div()
                            .w(px(64.0))
                            .flex_none()
                            .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .text_color(c(if cost { theme::SUBTLE } else { theme::TEXT }))
                            .child(g.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(c(if cost { theme::SUBTLE } else { theme::MUTED }))
                            .child(g.description()),
                    )
                    .when(!cost, |d| {
                        d.on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                            t.gran = g;
                            t.persist();
                            cx.notify();
                        }))
                    })
            });
        let on = self.in_view_only;
        let in_view = div()
            .id("display-in-view")
            .flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            // 切り替えスイッチ
            .child(
                div()
                    .mt(px(1.0))
                    .flex_none()
                    .flex()
                    .when(on, |d| d.justify_end())
                    .w(px(28.0))
                    .h(px(16.0))
                    .p(px(2.0))
                    .rounded_full()
                    .bg(c(if on { theme::ACCENT } else { theme::BORDER }))
                    .child(div().size(px(12.0)).rounded_full().bg(gpui::white())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_xs().child("表示範囲内のセッションだけ"))
                    .child(div().text_xs().text_color(c(theme::MUTED)).child(if on {
                        "範囲にイベントのないセッションを隠す"
                    } else {
                        "範囲外のセッションもグレーで並べる"
                    })),
            )
            .child(kbd("V").flex_none())
            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.toggle_in_view_only(cx)));
        deferred(
            anchored()
                .position(point(b.right(), b.bottom() + px(4.0)))
                .anchor(Corner::TopRight)
                .snap_to_window_with_margin(px(8.0))
                .child(
                    div()
                        .id("display-menu")
                        .occlude()
                        .w(px(380.0))
                        .p_1()
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .bg(c(theme::ELEVATED))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .shadow_lg()
                        .text_color(c(theme::TEXT))
                        .child(heading("表示オプション"))
                        .child(labeled("表示する値", metric))
                        .child(note(metric_note).pl(px(88.0)))
                        .child(labeled("行", rows))
                        .child(note(rows_note).pl(px(88.0)))
                        .child(divider())
                        .child(heading(if cost {
                            "ブロックのまとめ方（API コストの表示では使いません）"
                        } else {
                            "ブロックのまとめ方"
                        }))
                        .children(grans)
                        .child(divider())
                        .child(in_view),
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
                    ("M", "月"),
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
                    ("G", "期間・日付を選ぶ"),
                    ("⌘= ⌘- · + −", "拡大 / 縮小"),
                    ("ドラッグ · スクロール", "表示範囲を動かす"),
                    ("⌘ + スクロール · ピンチ", "マウス位置を中心に拡大縮小"),
                    ("ダブルクリック", "ブロックの範囲まで拡大（カレンダーではタイムラインで開く）"),
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
