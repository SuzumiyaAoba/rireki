//! 対話履歴の行、選択範囲の見出しとイベントの表示。

use super::{
    AnyElement, ClickEvent, Context, DETAIL_MAX, DetailListKey, DetailRow, Div, ElementId, Event,
    FontWeight, Icon, Kind, Marquee, Rireki, Selection, SharedString, Stateful, Window, badge, c,
    div, event_icon, fmt_dur, fmt_n, is_live, label, list, local, one_line, px, rich, status_label,
    theme, tip, usage_meter,
};
use gpui::prelude::*;

impl Rireki {
    /// 選択範囲のイベントを時刻順に並べる一覧。
    pub(super) fn event_list(
        &self,
        sel: Selection,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.detail_session(sel);
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        self.detail_list.prepare(
            DetailListKey {
                sel,
                generation: self.shown_detail_generation(),
                filter: self.row_filter.clone(),
                reversed: self.detail_reversed,
                width: self.detail_content_width(window).to_bits(),
                live: is_live(s.end),
                replay_visible: self.replay.as_ref().map(|replay| replay.visible(evs)),
            },
            evs,
        );
        let app = cx.entity().downgrade();
        if self
            .replay
            .as_ref()
            .is_some_and(|replay| replay.scroll_pending.replace(false))
        {
            // 行の追加とレイアウトが済んでから追う。再生中の追加では進行中のアニメーションを保つ。
            let following = app.clone();
            cx.defer(move |cx| {
                let _ = following.update(cx, |app, cx| {
                    if app.selected == Some(sel)
                        && app
                            .replay
                            .as_ref()
                            .is_some_and(|replay| replay.following && !replay.check_follow)
                        && !app.detail_following
                    {
                        app.animate_to_latest(cx);
                    }
                });
            });
        }
        self.detail_list.rendered.set(0);
        let scrolling = app.clone();
        self.detail_list
            .state
            .set_scroll_handler(move |_, window, cx| {
                let _ = scrolling.update(cx, |app, cx| {
                    app.cancel_detail_follow();
                    if let Some(replay) = &mut app.replay {
                        replay.following = false;
                        replay.check_follow = true;
                        replay.scroll_pending.set(false);
                        app.detail_changed(cx);
                        // ListState はこのコールバック中に借用されているので、端の判定を遅延する。
                        cx.defer_in(window, move |app, _, cx| {
                            if app.selected == Some(sel) && app.replay.is_some() {
                                app.check_replay_follow(cx);
                            }
                        });
                    }
                });
            });
        list(self.detail_list.state.clone(), move |row, _, cx| {
            app.update(cx, |app, cx| app.detail_list_item(sel, row, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .w_full()
        .min_w_0()
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    /// イベント行の直前に見出し・並び順・再生操作をまとめる
    pub(super) fn event_list_header(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(label("履歴の表示"))
                    .child(
                        div()
                            .id("detail-order")
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                self.icon_button(
                                    "detail-order-oldest",
                                    Some(Icon::ArrowDown),
                                    "古い順",
                                    !self.detail_reversed,
                                    cx,
                                    |t, _, cx| t.set_detail_order(false, cx),
                                )
                                .tooltip(tip("時刻の古いイベントから表示", None)),
                            )
                            .child(
                                self.icon_button(
                                    "detail-order-newest",
                                    Some(Icon::ArrowUp),
                                    "新しい順",
                                    self.detail_reversed,
                                    cx,
                                    |t, _, cx| t.set_detail_order(true, cx),
                                )
                                .tooltip(tip("時刻の新しいイベントから表示", None)),
                            ),
                    ),
            )
            .child(self.replay_controls(sel, cx))
    }

    pub(super) fn detail_list_item(
        &self,
        sel: Selection,
        row: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.valid(sel) || self.cache(sel).is_none() {
            return div().into_any_element();
        }
        let item = self
            .detail_list
            .data
            .borrow()
            .as_ref()
            .and_then(|d| d.rows.get(row))
            .copied();
        self.detail_list
            .rendered
            .set(self.detail_list.rendered.get() + 1);
        let i = match item {
            Some(DetailRow::Summary) => {
                return div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pb_2()
                    .child(self.selection_summary(sel, cx))
                    .child(self.history_context(sel, cx))
                    .child(self.event_list_header(sel, cx))
                    .into_any_element();
            }
            Some(DetailRow::Empty) => {
                return div()
                    .py_2()
                    .text_xs()
                    .text_color(c(theme::SUBTLE))
                    .child(
                        if self
                            .replay
                            .as_ref()
                            .is_some_and(|replay| !replay.is_finished())
                        {
                            "次のイベントを待っています…"
                        } else {
                            "該当するイベントはありません"
                        },
                    )
                    .into_any_element();
            }
            Some(DetailRow::Limit) => {
                return div()
                    .pt_1()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(format!(
                        "先頭 {} 件を表示しています（全 {} 件）",
                        fmt_n(DETAIL_MAX),
                        fmt_n(sel.last - sel.first + 1)
                    ))
                    .into_any_element();
            }
            Some(DetailRow::Event(i)) => i,
            None => return div().into_any_element(),
        };
        let s = self.detail_session(sel);
        let e = &s.lanes[sel.lane].events[sel.first + i];
        let cache = self.cache(sel);
        let touched = cache
            .and_then(|d| d.touched.get(i))
            .map_or(&[][..], Vec::as_slice);
        let (preview, output) = cache
            .and_then(|d| d.texts.get(i))
            .map_or(("", ""), |t| (t.0.as_str(), t.1.as_str()));
        if e.kind == Kind::User && self.row_filter.is_none() {
            self.turn_row(sel, i, preview, cx).into_any_element()
        } else {
            self.event_row(sel, i, (preview, output), touched, is_live(s.end), cx)
                .into_any_element()
        }
    }

    pub(super) fn history_context(&self, sel: Selection, cx: &mut Context<Self>) -> AnyElement {
        let lane = &self.detail_session(sel).lanes[sel.lane];
        if lane.events[sel.first].kind == Kind::User {
            return div().into_any_element();
        }
        let turns = &self.cache(sel).unwrap().session_summary.turns[sel.lane];
        let previous = turns
            .partition_point(|turn| turn.index < sel.first)
            .checked_sub(1);
        let Some(index) = previous.map(|i| turns[i].index) else {
            return div().into_any_element();
        };
        div()
            .id("history-context")
            .p_2()
            .rounded_md()
            .bg(c(theme::BG))
            .flex()
            .flex_col()
            .gap_1()
            .child(label("選択範囲外の指示 · 文脈として表示"))
            .child(
                div()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(Marquee::new(
                        "prompt",
                        SharedString::from(lane.events[index].summary.clone()),
                    )),
            )
            .cursor_pointer()
            .hover(|d| d.bg(c(theme::HOVER)))
            .on_click(cx.listener(move |app, _: &ClickEvent, w, cx| {
                app.navigate(
                    Selection {
                        first: index,
                        last: index,
                        ..sel
                    },
                    w,
                    cx,
                );
            }))
            .into_any_element()
    }

    /// ユーザーの指示の行。次の指示までの件数・時間・失敗を添え、本文を大きめに出す（ターンの区切り）
    pub(super) fn turn_row(
        &self,
        sel: Selection,
        i: usize,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let lane = &self.detail_session(sel).lanes[sel.lane];
        let idx = sel.first + i;
        let e = &lane.events[idx];
        let turns = &self.cache(sel).unwrap().session_summary.turns[sel.lane];
        let visible = self.replay.as_ref().map_or(sel.last + 1, |replay| {
            sel.first + replay.visible(&lane.events[sel.first..=sel.last])
        });
        let next = turns
            .iter()
            .find(|t| t.index > idx)
            .map_or(lane.events.len(), |t| t.index);
        let end = next.min(sel.last + 1).min(visible);
        let selected = &lane.events[idx + 1..end];
        let until = selected.iter().map(|e| e.end).fold(e.start, f64::max);
        let failures = selected.iter().filter(|e| e.status.failed()).count();
        let mut facts = vec![format!("{} 件", fmt_n(selected.len()))];
        if until > e.start {
            facts.push(fmt_dur(until - e.start));
        }
        let color = theme::event_color(e);
        let text = if text.is_empty() {
            e.summary.as_ref()
        } else {
            text
        };
        let content = div()
            .my_1()
            .px_2p5()
            .py_2()
            .rounded_lg()
            .bg(c(color).opacity(0.08))
            .border_1()
            .border_color(c(color).opacity(0.25))
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .whitespace_normal()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(color))
                            .child("指示"),
                    )
                    .child(div().flex_1())
                    .when(failures > 0, |d| {
                        d.child(badge(format!("失敗等 {}", failures), theme::ERR))
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(facts.join(" · ")),
                    ),
            )
            .child(rich::markdown(text, theme::TEXT).text_size(px(13.0)))
            .when(!e.usage.is_empty(), |d| {
                d.child(usage_meter(
                    ElementId::NamedInteger("usage".into(), idx as u64),
                    &e.usage,
                    e.model.as_deref(),
                    self.detail_list.usage_scale(),
                ))
            });
        self.rail_row(sel, idx, e, true, content.into_any_element(), cx)
    }

    /// 一覧の 1 行。共通の尺度で入力・出力の使用量を、本文の開閉に関係なく出す。
    pub(super) fn event_row(
        &self,
        sel: Selection,
        i: usize,
        (text, output): (&str, &str),
        touched: &[(String, usize, usize)],
        live: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let idx = sel.first + i;
        let s = self.detail_session(sel);
        let e = &s.lanes[sel.lane].events[idx];
        let foldable = matches!(e.kind, Kind::Tool | Kind::Agent | Kind::Thinking);
        let expanded = !foldable || self.history_expanded.contains(&idx);
        let status = status_label(e, live, output);
        let caption = if text.is_empty() {
            e.summary.as_ref()
        } else {
            text
        };
        let content = div()
            .id(("history-content", idx))
            .flex_1()
            .min_w_0()
            .py_2()
            .pr_1()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .when(foldable, |d| {
                        d.child(self.icon_button(
                            SharedString::from(format!("history-toggle-{idx}")),
                            Some(if expanded {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            }),
                            e.name.to_string(),
                            expanded,
                            cx,
                            move |t, _, cx| {
                                cx.stop_propagation();
                                if !t.history_expanded.remove(&idx) {
                                    t.history_expanded.insert(idx);
                                }
                                let offset = t.detail_list.state.logical_scroll_top();
                                let count = t.detail_list.state.item_count();
                                t.detail_list.state.splice(0..count, count);
                                t.detail_list.state.scroll_to(offset);
                                t.detail_changed(cx);
                            },
                        ))
                    })
                    .when(!foldable, |d| d.child(label(e.name.to_string())))
                    .when_some(status, |d, (l, color)| d.child(badge(l, color)))
                    .child(div().flex_1())
                    .when(e.end > e.start, |d| {
                        d.child(
                            div()
                                .text_color(c(theme::SUBTLE))
                                .child(fmt_dur(e.end - e.start)),
                        )
                    })
                    .child(
                        self.ghost_button(
                            SharedString::from(format!("history-open-{idx}")),
                            Icon::ArrowRight,
                            "このイベントの詳細を開く",
                            None,
                            true,
                            cx,
                            move |t, w, cx| {
                                cx.stop_propagation();
                                t.navigate(
                                    Selection {
                                        first: idx,
                                        last: idx,
                                        ..sel
                                    },
                                    w,
                                    cx,
                                );
                                t.reveal(
                                    Selection {
                                        first: idx,
                                        last: idx,
                                        ..sel
                                    },
                                    false,
                                    w,
                                );
                            },
                        )
                        .size(px(20.0)),
                    ),
            )
            .when(!expanded, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(c(theme::MUTED))
                        .line_clamp(1)
                        .child(one_line(&e.summary, 160)),
                )
            })
            .when(
                expanded && matches!(e.kind, Kind::User | Kind::Assistant | Kind::Thinking),
                |d| {
                    d.child(
                        rich::markdown(
                            caption,
                            if e.kind == Kind::Thinking {
                                theme::MUTED
                            } else {
                                theme::TEXT
                            },
                        )
                        .text_size(px(13.0)),
                    )
                },
            )
            .when(expanded && e.kind.is_tool(), |d| {
                let raw = self
                    .cache(sel)
                    .and_then(|c| c.raws.get(i))
                    .and_then(|raw| raw.as_deref());
                d.child(self.tool_input(e, text, raw, &s.cwd, cx))
                    .child(self.tool_output(e, output, output.lines().count(), live, &s.cwd, cx))
                    .when(e.kind == Kind::Agent, |d| {
                        d.when_some(
                            e.call_id.as_deref().and_then(|id| s.lane_spawned_by(id)),
                            |d, lane| d.child(self.subagent_button(sel, lane, cx)),
                        )
                    })
            })
            .when(!e.usage.is_empty(), |d| {
                d.child(usage_meter(
                    ElementId::NamedInteger("usage".into(), idx as u64),
                    &e.usage,
                    e.model.as_deref(),
                    self.detail_list.usage_scale(),
                ))
            });
        let _ = touched;
        self.rail_row(sel, idx, e, false, content.into_any_element(), cx)
    }

    /// 一覧の行の外枠。左に時刻、縦の線の上にイベントの種類のアイコン、右に内容を並べる。
    /// 押すとそのイベントだけを選ぶ
    pub(super) fn rail_row(
        &self,
        sel: Selection,
        idx: usize,
        e: &Event,
        turn: bool,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (session, lane) = (sel.session, sel.lane);
        let color = theme::event_color(e);
        let node = if turn { 22.0 } else { 18.0 };
        div()
            .id(ElementId::NamedInteger("ev".into(), idx as u64))
            // List は行を独立して計測するため、幅を指定しないと本文の長さまで広がる。
            .w_full()
            .min_w_0()
            .flex()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|d| d.bg(c(theme::HOVER)))
            .child(
                div()
                    .flex_none()
                    .w(px(58.0))
                    .whitespace_nowrap()
                    .pt(px(if turn { 12.0 } else { 7.0 }))
                    .pl_1()
                    .text_xs()
                    .text_color(c(theme::SUBTLE))
                    .child(local(e.start).format("%H:%M:%S").to_string()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(22.0))
                    .relative()
                    .flex()
                    .justify_center()
                    // 行をまたいでつながる縦の線
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(10.5))
                            .w(px(1.0))
                            .bg(c(theme::BORDER)),
                    )
                    .child(
                        div()
                            .mt(px(if turn { 8.0 } else { 5.0 }))
                            .size(px(node))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(c(theme::PANEL))
                            .border_1()
                            .border_color(c(color).opacity(0.5))
                            .child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(c(color).opacity(0.16))
                                    .child(event_icon(
                                        theme::event_icon(e),
                                        color,
                                        if turn { 12.0 } else { 10.0 },
                                    )),
                            ),
                    ),
            )
            .child(div().flex_1().min_w_0().flex().flex_col().child(content))
            .when(turn, |d| {
                d.on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                    t.navigate(
                        Selection {
                            session,
                            lane,
                            first: idx,
                            last: idx,
                        },
                        w,
                        cx,
                    );
                }))
            })
    }
}
