//! セッション全体の入口。指示・応答とエージェントを可視行だけ描く。

use std::cell::RefCell;

use gpui::{AnyElement, ClickEvent, ClipboardItem, Context, Div, ListAlignment, ListOffset, ListState, Stateful, Window, div, list, prelude::*, px};

use super::{Rireki, RowFilter, Selection, resume_command, rich, tilde, badge, cache_hit_stat, fmt_dur, fmt_n, is_live, label, local, stat, usage_cost_label};
use crate::app::widgets::{Marquee, tip};
use crate::icons::{self, Icon};
use crate::model::{Source, one_line};
use crate::theme::{self, c};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Intro,
    Turn(usize),
    NoTurns,
    Agents,
    Lane(usize, usize),
    Information,
}

pub(in crate::app) struct OverviewList {
    state: ListState,
    data: RefCell<Option<(u64, u32, Vec<Row>)>>,
}

impl OverviewList {
    pub(in crate::app) fn new() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(200.0)),
            data: RefCell::new(None),
        }
    }

    pub(in crate::app) fn reset(&self) {
        *self.data.borrow_mut() = None;
        self.state.reset(0);
    }

    pub(in crate::app) fn invalidate_information(&self) {
        let offset = self.state.logical_scroll_top();
        if let Some(index) = self
            .data
            .borrow()
            .as_ref()
            .and_then(|d| d.2.iter().position(|row| *row == Row::Information))
        {
            self.state.splice(index..index + 1, 1);
            self.state.scroll_to(offset);
        }
    }

    pub(in crate::app) fn profile_scroll_to(&self, offset: ListOffset) {
        self.state.scroll_to(offset);
    }
    pub(in crate::app) fn profile_offset(&self) -> ListOffset {
        self.state.logical_scroll_top()
    }

    fn prepare(&self, generation: u64, width: u32, rows: impl FnOnce() -> Vec<Row>) {
        let mut data = self.data.borrow_mut();
        if data.as_ref().is_some_and(|d| (d.0, d.1) == (generation, width)) {
            return;
        }
        let rows = rows();
        let old_offset = self.state.logical_scroll_top();
        let offset = data
            .as_ref()
            .and_then(|d| d.2.get(old_offset.item_ix))
            .and_then(|row| rows.iter().position(|next| next == row))
            .map(|item_ix| ListOffset { item_ix, ..old_offset })
            .unwrap_or_default();
        self.state.splice(0..self.state.item_count(), rows.len());
        self.state.scroll_to(offset);
        *data = Some((generation, width, rows));
    }
}

impl Rireki {
    pub(super) fn overview_view(&self, sel: Selection, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let session = self.detail_session(sel);
        let summary = &self.cache(sel).unwrap().session_summary;
        self.overview_list.prepare(
            self.shown_detail_generation(),
            self.detail_content_width(window).to_bits(),
            || {
                let mut rows = vec![Row::Intro];
                rows.extend((0..summary.turns[0].len()).map(Row::Turn));
                if summary.turns[0].is_empty() {
                    rows.push(Row::NoTurns);
                }
                rows.push(Row::Agents);
                rows.extend(session.tree.iter().map(|&(lane, depth)| Row::Lane(lane, depth)));
                rows.push(Row::Information);
                rows
            },
        );
        let app = cx.entity().downgrade();
        list(self.overview_list.state.clone(), move |i, _, cx| {
            app.update(cx, |app, cx| app.overview_item(sel, i, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .flex_1()
        .min_h_0()
        .p_3()
        .into_any_element()
    }

    fn overview_item(&self, sel: Selection, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self
            .overview_list
            .data
            .borrow()
            .as_ref()
            .and_then(|d| d.2.get(index))
            .copied()
        else {
            return div().into_any_element();
        };
        let s = self.detail_session(sel);
        let summary = &self.cache(sel).unwrap().session_summary;
        match row {
            Row::Intro => div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .pb_3()
                .child(
                    div()
                        .text_base()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(Marquee::new(
                            "session-title",
                            if s.title.is_empty() {
                                "(無題)".to_string()
                            } else {
                                one_line(&s.title, usize::MAX)
                            },
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .text_xs()
                        .text_color(c(theme::MUTED))
                        .child(self.project_name(&s.cwd))
                        .child(s.source.label())
                        .child(badge(
                            if is_live(s.end) { "進行中" } else { "履歴" },
                            if is_live(s.end) { theme::LIVE } else { theme::MUTED },
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(stat("経過時間", fmt_dur((s.end - s.start).max(0.0)), None))
                        .child(stat("ターン", fmt_n(summary.total.stats.turns), None))
                        .child({
                            // 統計のセッションの順位と同じく、失敗したイベントだけの履歴へ移る
                            let failures = summary.total.stats.failures();
                            let session = sel.session;
                            stat("失敗・拒否・中断", fmt_n(failures), (failures > 0).then_some(theme::ERR))
                                .id("overview-failures")
                                .when(failures > 0, |d| {
                                    d.cursor_pointer()
                                        .hover(|s| s.border_color(c(theme::SUBTLE)))
                                        .tooltip(tip("失敗・拒否・中断したイベントだけを履歴に表示", None))
                                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                            t.open_failures(session, w, cx)
                                        }))
                                })
                        })
                        .child(stat("API換算料金", usage_cost_label(&summary.total.usage), None))
                        .child(cache_hit_stat("overview-cache-hit", &summary.total.usage)),
                )
                .child(label("指示と応答 · クリックでターンの履歴を表示"))
                .into_any_element(),
            Row::NoTurns => div()
                .pb_3()
                .text_xs()
                .text_color(c(theme::MUTED))
                .child("メインレーンに指示は記録されていません")
                .into_any_element(),
            Row::Turn(i) => {
                let turn = &summary.turns[0][i];
                let events = &s.lanes[0].events;
                let first = turn.index;
                let last = summary.turns[0].get(i + 1).map_or(events.len(), |t| t.index) - 1;
                let session = sel.session;
                div()
                    .id(("overview-turn", first))
                    .w_full()
                    .min_w_0()
                    .mb_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(c(theme::SUBTLE))
                            .child(format!(
                                "指示 {} · {}",
                                i + 1,
                                local(events[first].start).format("%H:%M")
                            ))
                            .child(div().flex_1())
                            .when(turn.failures > 0, |d| {
                                d.child(badge(format!("失敗等 {}", turn.failures), theme::ERR))
                            }),
                    )
                    .child(div().text_sm().child(Marquee::new(
                        "prompt",
                        gpui::SharedString::from(events[first].summary.clone()),
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .line_clamp(3)
                            .child(turn.response.map_or_else(
                                || "応答は記録されていません".to_string(),
                                |response| format!("応答 · {}", one_line(&events[response].summary, 400)),
                            )),
                    )
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::HOVER)))
                    .on_click(cx.listener(move |app, _: &ClickEvent, w, cx| {
                        let target = Selection {
                            session,
                            lane: 0,
                            first,
                            last,
                        };
                        if app.valid(target) {
                            app.navigate(target, w, cx);
                            app.reveal(target, false, w);
                        }
                    }))
                    .into_any_element()
            }
            Row::Agents => div().pt_2().pb_2().child(label("エージェント構成")).into_any_element(),
            Row::Lane(lane, depth) => {
                let l = &s.lanes[lane];
                let failures = summary.lanes[lane].stats.failures();
                let session = sel.session;
                div()
                    .id(("overview-lane", lane))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(8.0 + depth as f32 * 14.0))
                    .pr_2()
                    .py_1p5()
                    .mb_1()
                    .rounded_md()
                    .text_xs()
                    .child(icons::icon(if lane == 0 { Icon::Layers } else { Icon::Bot }, theme::SUB).size_3())
                    .child(div().flex_1().min_w_0().truncate().child(if lane == 0 {
                        "メイン".into()
                    } else {
                        l.name.clone()
                    }))
                    .child(
                        div()
                            .text_color(c(theme::SUBTLE))
                            .child(format!("{} 件", fmt_n(l.events.len()))),
                    )
                    .when(failures > 0, |d| {
                        d.child(self.icon_button(
                            gpui::SharedString::from(format!("overview-failed-{lane}")),
                            None,
                            format!("失敗等 {failures}"),
                            false,
                            cx,
                            move |app, w, cx| {
                                cx.stop_propagation();
                                app.open_lane_filtered(session, lane, Some(RowFilter::Failed), w, cx);
                            },
                        ))
                    })
                    .when(!l.events.is_empty(), |d| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(c(theme::HOVER)))
                            .on_click(cx.listener(move |app, _: &ClickEvent, w, cx| {
                                app.open_lane_filtered(session, lane, None, w, cx);
                            }))
                    })
                    .into_any_element()
            }
            Row::Information => div()
                .pt_3()
                .child(self.overview_information(sel, cx))
                .into_any_element(),
        }
    }
}

impl Rireki {
    /// 概要末尾の基本情報と再開・ログの操作。モデルはメインレーンの属性を表示する。
    pub(super) fn overview_information(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = self.detail_session(sel);
        let lane = &s.lanes[0];
        let meta = |k: &'static str, v: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .w(px(76.0))
                        .flex_none()
                        .text_color(c(theme::SUBTLE))
                        .child(k),
                )
                .child(div().flex_1().min_w_0().child(v))
        };
        let model = match (lane.model.is_empty(), lane.effort.is_empty()) {
            (true, true) => None,
            (false, true) => Some(lane.model.clone()),
            (true, false) => Some(lane.effort.clone()),
            (false, false) => Some(format!("{} {}", lane.model, lane.effort)),
        };
        let version = (!s.version.is_empty()).then(|| match s.source {
            Source::Claude => format!("Claude Code {}", s.version),
            Source::Codex => format!("Codex CLI {}", s.version),
        });
        let session = sel.session;
        let resume = resume_command(s);
        let resumed = self.copied == Some("copy-resume");
        let origin = s
            .lanes
            .first()
            .map(|l| l.origin.clone())
            .unwrap_or_default();
        let main_n = s.lanes[0].events.len();
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_1p5()
            .child(
                self.action_button(
                    "resume",
                    if resumed { Icon::Check } else { Icon::Terminal },
                    if resumed {
                        "コピーしました"
                    } else {
                        "再開コマンドをコピー"
                    },
                )
                .tooltip(tip(resume.clone(), None))
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(resume.clone()));
                    t.flash_copied("copy-resume", cx);
                })),
            )
            .when(main_n > 0, |d| {
                d.child(
                    self.action_button("all-events", Icon::Layers, "全イベントを一覧")
                        .tooltip(tip("メインのレーン全体を選ぶ", None))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                            t.open_lane_filtered(session, 0, None, w, cx)
                        })),
                )
            })
            .when(!origin.is_empty(), |d| {
                d.child(
                    self.action_button("reveal-log", Icon::FolderOpen, "ログを Finder で表示")
                        .tooltip(tip(origin.clone(), None))
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.reveal_path(std::path::Path::new(&origin));
                        })),
                )
            });
        let section = |name: &'static str| {
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .pt_2()
                .border_t_1()
                .border_color(c(theme::BORDER))
                .child(label(name))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                section("基本情報").child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_xs()
                        .when_some(model, |d, m| {
                            d.child(meta("モデル", div().child(m).into_any_element()))
                        })
                        .when(!s.branch.is_empty(), |d| {
                            d.child(meta(
                                "ブランチ",
                                div().child(s.branch.clone()).into_any_element(),
                            ))
                        })
                        .child(meta(
                            "ディレクトリ",
                            div().child(tilde(&s.cwd)).into_any_element(),
                        ))
                        .when(self.expanded.contains("overview-meta"), |d| {
                            d.child(meta(
                                "セッション",
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(rich::MONO)
                                            .child(s.id.clone()),
                                    )
                                    .child(
                                        self.copy_button("copy-id", s.id.clone(), cx).flex_none(),
                                    )
                                    .into_any_element(),
                            ))
                        })
                        .when(self.expanded.contains("overview-meta"), |d| {
                            d.when_some(version, |d, v| {
                                d.child(meta("バージョン", div().child(v).into_any_element()))
                            })
                        })
                        .when(s.headless, |d| {
                            d.child(meta("起動", div().child("非対話").into_any_element()))
                        }),
                ),
            )
            .child(self.icon_button(
                "overview-meta-toggle",
                Some(Icon::ChevronDown),
                if self.expanded.contains("overview-meta") {
                    "詳細情報を閉じる"
                } else {
                    "ID・バージョンを表示"
                },
                false,
                cx,
                |t, _, cx| {
                    if !t.expanded.remove("overview-meta") {
                        t.expanded.insert("overview-meta");
                    }
                    t.overview_list.invalidate_information();
                    t.detail_changed(cx);
                },
            ))
            .child(actions)
    }

    fn action_button(&self, id: &'static str, icon: Icon, label: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .text_xs()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(
                icon,
                if icon == Icon::Check {
                    theme::OK
                } else {
                    theme::MUTED
                },
            ))
            .child(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_keeps_the_information_anchor_when_turns_arrive_and_width_changes() {
        let list = OverviewList::new();
        list.prepare(1, 440, || vec![Row::Intro, Row::Turn(0), Row::Agents, Row::Information]);
        list.state.scroll_to(ListOffset {
            item_ix: 3,
            offset_in_item: px(9.0),
        });
        list.prepare(2, 320, || {
            vec![Row::Intro, Row::Turn(0), Row::Turn(1), Row::Agents, Row::Information]
        });
        let offset = list.state.logical_scroll_top();
        assert_eq!((offset.item_ix, offset.offset_in_item), (4, px(9.0)));
        list.invalidate_information();
        assert_eq!(list.state.logical_scroll_top().offset_in_item, px(9.0));
        list.reset();
        list.prepare(3, 440, || vec![Row::Intro, Row::NoTurns, Row::Information]);
        assert_eq!(list.state.logical_scroll_top().item_ix, 0);
    }
}
