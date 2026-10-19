//! 単一イベントの本文とツールの入出力の描画。

use super::{
    AnyElement, ClickEvent, Context, Div, Event, FOLD_DIFF, FOLD_LINES, Icon, Kind,
    MARKDOWN_OUTPUT, Marquee, Rireki, Selection, SharedString, Stateful, Status, UsageScale, badge,
    c, div, fmt_dur, fmt_n, icons, is_live, label, local, one_line, px, rich, status_label, theme,
    tip, usage_input, usage_meter,
};
use gpui::prelude::*;

impl Rireki {
    /// 1 イベントの詳細。時刻・トークン数・きっかけの指示と、入力・結果
    pub(super) fn event_view(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[sel.lane];
        let e = &lane.events[sel.first];
        let cache = self.cache(sel);
        let (input, output) = cache
            .and_then(|d| d.texts.first())
            .map_or(("", ""), |t| (t.0.as_str(), t.1.as_str()));
        let raw = cache
            .and_then(|d| d.raws.first())
            .and_then(|r| r.as_deref());
        let mut col = div().flex().flex_col().gap_3().min_w_0();

        // 時刻・所要時間（位置は見出しに出す）
        let when = if e.end > e.start {
            format!(
                "{} → {}",
                local(e.start).format("%m/%d %H:%M:%S"),
                local(e.end).format("%H:%M:%S")
            )
        } else {
            local(e.start).format("%m/%d %H:%M:%S").to_string()
        };
        col = col.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x_3()
                .gap_y_0p5()
                .text_xs()
                .text_color(c(theme::MUTED))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icons::icon(Icon::Clock, theme::SUBTLE).size_3())
                        .child(when),
                )
                .when(e.end > e.start, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(icons::icon(Icon::Hourglass, theme::SUBTLE).size_3())
                            .child(fmt_dur(e.end - e.start)),
                    )
                }),
        );
        if !e.usage.is_empty() {
            col = col.child(usage_meter(
                "usage-ev",
                &e.usage,
                e.model.as_deref(),
                UsageScale {
                    input: usage_input(&e.usage),
                    output: e.usage.output as u64,
                },
            ));
        }

        // きっかけになったユーザーの指示
        let turns = &self.cache(sel).unwrap().session_summary.turns[sel.lane];
        let previous_turn = turns
            .partition_point(|turn| turn.index < sel.first)
            .checked_sub(1)
            .map(|index| turns[index].index);
        if e.kind != Kind::User
            && let Some(ui) = previous_turn
        {
            let u = &lane.events[ui];
            let (session, lane_i) = (sel.session, sel.lane);
            col = col.child(
                div()
                    .id("turn-prompt")
                    .flex()
                    .items_start()
                    .gap_1p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(c(theme::event_color(u)).opacity(0.07))
                    .border_l_2()
                    .border_color(c(theme::event_color(u)).opacity(0.7))
                    .text_xs()
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::event_color(u)).opacity(0.12)))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::MUTED))
                            .child("選択範囲外の指示"),
                    )
                    .child(div().flex_1().min_w_0().child(Marquee::new(
                        "prompt",
                        SharedString::from(u.summary.clone()),
                    )))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(local(u.start).format("%H:%M").to_string()),
                    )
                    .tooltip(tip("このイベントのきっかけになった指示を開く", None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        let sel = Selection {
                            session,
                            lane: lane_i,
                            first: ui,
                            last: ui,
                        };
                        t.navigate(sel, w, cx);
                        t.reveal(sel, false, w);
                    })),
            );
        }

        match e.kind {
            Kind::User | Kind::Assistant | Kind::Thinking => {
                let color = if e.kind == Kind::Thinking {
                    theme::MUTED
                } else {
                    theme::TEXT
                };
                col = col.child(
                    self.titled(
                        None,
                        "copy-in",
                        input,
                        rich::markdown(input, color)
                            .text_size(px(13.0))
                            .into_any_element(),
                        cx,
                    ),
                );
            }
            Kind::Tool | Kind::Agent => {
                col = col.child(self.tool_input(e, input, raw, &s.cwd, cx));
                if e.kind == Kind::Agent
                    && let Some(li) = e.call_id.as_deref().and_then(|id| s.lane_spawned_by(id))
                {
                    col = col.child(self.subagent_button(sel, li, cx));
                }
                let lines = cache
                    .and_then(|d| d.out_lines.first())
                    .copied()
                    .unwrap_or(0);
                col = col.child(self.tool_output(e, output, lines, is_live(s.end), &s.cwd, cx));
            }
        }
        col
    }

    /// 見出しとコピーボタンを付けた本文
    pub(super) fn titled(
        &self,
        head: Option<Div>,
        copy_id: &'static str,
        copy: &str,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_h(px(22.0))
                    .child(head.unwrap_or_else(div))
                    .child(div().flex_1())
                    .when(!copy.is_empty(), |d| {
                        d.child(self.copy_button(copy_id, copy.to_string(), cx).flex_none())
                    }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{copy_id}-body")))
                    .min_w_0()
                    .child(body),
            )
    }

    /// 「残り N 行を表示」。押すと `id` の本文を全部出す
    pub(super) fn expand_button(
        &self,
        id: &'static str,
        hidden: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("more-{id}")))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .text_xs()
            .text_color(c(theme::ACCENT))
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(Icon::ChevronDown, theme::ACCENT).size_3())
            .child(format!("残り {} 行を表示", fmt_n(hidden)))
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                t.expanded.insert(id);
                t.detail_changed(cx);
            }))
    }

    /// 長い本文を `FOLD_LINES` 行で畳んだ等幅の箱
    pub(super) fn folded_mono(
        &self,
        id: &'static str,
        text: &str,
        flow: rich::TextFlow,
        cx: &mut Context<Self>,
    ) -> Div {
        let (shown, hidden) = if self.expanded.contains(id) {
            (text, 0)
        } else {
            rich::head_lines(text, FOLD_LINES)
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(rich::mono_box(id, shown, flow))
            .when(hidden > 0, |d| d.child(self.expand_button(id, hidden, cx)))
    }

    /// ツールの入力。差分・コマンド・Markdown（計画やサブエージェントへの指示）・引数で見せ方を変える
    pub(super) fn tool_input(
        &self,
        e: &Event,
        input: &str,
        raw: Option<&str>,
        cwd: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        // 要約が入力に含まれていなければ（コマンドの説明など）見出しの下に添える。
        // exec の要約は呼び出しを並べたものなので、整形した入力と重なる
        let key = e.summary.trim_end_matches('…');
        let caption =
            (raw.is_none() && !key.is_empty() && !one_line(input, usize::MAX).contains(key))
                .then(|| e.summary.clone());
        let markdown = matches!(e.name.as_ref(), "ExitPlanMode" | "Agent" | "Task");
        let (title, body): (&str, AnyElement) = if let Some(raw) = raw {
            // exec は整形した呼び出しと JS 本文を切り替える
            let show = self.show_raw;
            let toggle = self.segmented(
                "rawmode",
                &[(false, "整形"), (true, "JS")],
                Some(show),
                cx,
                |t, v, _, cx| {
                    t.show_raw = v;
                    t.detail_changed(cx);
                },
            );
            let text = if show { raw } else { input };
            let body = div()
                .flex()
                .flex_col()
                .gap_1p5()
                .min_w_0()
                .child(toggle)
                .child(self.folded_mono("in", text, rich::TextFlow::Scroll, cx));
            ("入力", body.into_any_element())
        } else if rich::is_patch(input) {
            let files = rich::parse_patch(input);
            let limit = if self.expanded.contains("in") {
                usize::MAX
            } else {
                FOLD_DIFF
            };
            let (view, hidden) = rich::diff_view(&files, cwd, limit);
            let body = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(view)
                .when(hidden > 0, |d| {
                    d.child(self.expand_button("in", hidden, cx))
                });
            ("変更", body.into_any_element())
        } else if markdown {
            (
                "内容",
                rich::markdown(input, theme::TEXT)
                    .text_size(px(13.0))
                    .into_any_element(),
            )
        } else if input.starts_with("$ ") {
            (
                "コマンド",
                self.folded_mono("in", input, rich::TextFlow::Scroll, cx)
                    .into_any_element(),
            )
        } else {
            (
                "入力",
                self.folded_mono("in", input, rich::TextFlow::Wrap, cx)
                    .into_any_element(),
            )
        };
        let copy = if self.show_raw {
            raw.unwrap_or(input)
        } else {
            input
        };
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .min_w_0()
            .when_some(caption, |d, cap| {
                d.child(div().text_sm().child(SharedString::from(cap)))
            })
            .child(self.titled(Some(label(title)), "copy-in", copy, body, cx))
    }

    /// ツールの結果。失敗なら枠を赤くし、内容に合わせて Markdown・行番号付き・差分・ANSI の色で出す
    pub(super) fn tool_output(
        &self,
        e: &Event,
        output: &str,
        lines: usize,
        live: bool,
        cwd: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = status_label(e, live, output);
        let head = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(label("結果"))
            .when(lines > 1, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(c(theme::SUBTLE))
                        .child(format!("{} 行", fmt_n(lines))),
                )
            })
            .when_some(status.clone(), |d, (l, col)| d.child(badge(l, col)));
        let body: AnyElement = if output.is_empty() {
            let note = match e.status {
                Status::Pending if live => "結果はまだありません",
                Status::Pending => "結果は記録されていません",
                _ => "出力なし",
            };
            div()
                .text_xs()
                .text_color(c(theme::SUBTLE))
                .child(note)
                .into_any_element()
        } else if MARKDOWN_OUTPUT.contains(&e.name.as_ref()) {
            rich::markdown(output, theme::TEXT)
                .text_size(px(13.0))
                .into_any_element()
        } else if rich::is_patch(output) {
            let files = rich::parse_patch(output);
            let limit = if self.expanded.contains("out") {
                usize::MAX
            } else {
                FOLD_DIFF
            };
            let (view, hidden) = rich::diff_view(&files, cwd, limit);
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(view)
                .when(hidden > 0, |d| {
                    d.child(self.expand_button("out", hidden, cx))
                })
                .into_any_element()
        } else if let Some(rows) = rich::numbered(output) {
            let all = self.expanded.contains("out");
            let hidden = if all {
                0
            } else {
                rows.len().saturating_sub(FOLD_LINES)
            };
            let shown = if all {
                &rows[..]
            } else {
                &rows[..rows.len().min(FOLD_LINES)]
            };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(rich::numbered_view(shown))
                .when(hidden > 0, |d| {
                    d.child(self.expand_button("out", hidden, cx))
                })
                .into_any_element()
        } else {
            self.folded_mono("out", output, rich::TextFlow::Scroll, cx)
                .into_any_element()
        };
        let failed = e.status.failed();
        self.titled(Some(head), "copy-out", output, body, cx).when(
            failed && !output.is_empty(),
            |d| {
                d.pl_2()
                    .border_l_2()
                    .border_color(c(status.map_or(theme::ERR, |s| s.1)).opacity(0.6))
            },
        )
    }

    /// サブエージェントのレーンを開くボタン（件数・期間・失敗を添える）
    pub(super) fn subagent_button(
        &self,
        sel: Selection,
        li: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let session = sel.session;
        let l = &self.detail_session(sel).lanes[li];
        let n = l.events.len();
        let (a, b) = l.range().unwrap_or((0.0, 0.0));
        let st = self.detail_cache.as_ref().unwrap().session_summary.lanes[li].stats;
        let mut text = format!(
            "サブエージェントを開く · {} 件 · {}",
            fmt_n(n),
            fmt_dur(b - a)
        );
        if st.failures() > 0 {
            text.push_str(&format!(" · 失敗 {}", st.failures()));
        }
        self.icon_button(
            SharedString::from(format!("to-sub-{li}")),
            Some(Icon::Bot),
            text,
            false,
            cx,
            move |t, w, cx| {
                let sel = Selection {
                    session,
                    lane: li,
                    first: 0,
                    last: n - 1,
                };
                if !t.valid(sel) {
                    return;
                }
                t.navigate(sel, w, cx);
                t.reveal(sel, true, w);
            },
        )
    }

    // ------------------------------------------------------------ 複数
}
