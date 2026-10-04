//! 詳細パネル。選択したブロックのセッション情報と、イベントごとの入力・結果を並べる

use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Div, ElementId, FontWeight, SharedString, WeakEntity, Window, div,
    prelude::*, px,
};

use super::widgets::{code_box, dot, project_tile, tip};
use super::{DETAIL_MAX, HEADER_H, INNER_RADIUS, Rireki, Selection, fmt_dur, local, tilde};
use crate::db::JsCache;
use crate::icons::{self, Icon};
use crate::loader;
use crate::model::{Kind, Session, Source, Usage, UsageSum, truncate};
use crate::theme::{self, c};

/// 詳細パネル。選択が変わったときだけ描き直すよう、別ビューとしてキャッシュする
pub struct DetailView {
    pub(super) app: WeakEntity<Rireki>,
}

impl Render for DetailView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.detail(cx)),
            None => div().into_any_element(),
        }
    }
}

/// セッションを再開するコマンド（作業ディレクトリに移ってから再開する）
fn resume_command(s: &Session) -> String {
    let cwd = format!("'{}'", s.cwd.replace('\'', "'\\''"));
    match s.source {
        Source::Claude => format!("cd {cwd} && claude --resume {}", s.id),
        Source::Codex => format!("cd {cwd} && codex resume {}", s.id),
    }
}

impl Rireki {
    pub(super) fn detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = div()
            .size_full()
            .flex()
            .flex_col()
            .rounded_r(px(INNER_RADIUS))
            .bg(c(theme::PANEL))
            .border_l_1()
            .border_color(c(theme::BORDER))
            .text_sm()
            .text_color(c(theme::TEXT));
        let Some(sel) = self.selected.filter(|&s| self.valid(s)) else {
            return panel.into_any_element();
        };
        let s = &self.store.sessions[sel.session];
        let lane = &s.lanes[sel.lane];
        let evs = &lane.events[sel.first..=sel.last];
        let single = evs.len() == 1;
        let title = if single { evs[0].name.clone() } else { format!("{} 件のイベント", evs.len()) };

        let header = div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .pl_3()
            .pr_2()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(self.ghost_button(
                "d-back",
                Icon::ArrowLeft,
                "戻る",
                Some("⌘["),
                !self.back.is_empty(),
                cx,
                |t, w, cx| t.go_back(w, cx),
            ))
            .child(self.ghost_button(
                "d-forward",
                Icon::ArrowRight,
                "進む",
                Some("⌘]"),
                !self.forward.is_empty(),
                cx,
                |t, w, cx| t.go_forward(w, cx),
            ))
            .child(self.ghost_button("d-locate", Icon::Locate, "選択位置を表示", None, true, cx, |t, w, _| {
                if let Some(sel) = t.selected {
                    t.reveal(sel, true, w);
                }
            }))
            .child(
                self.ghost_button("d-close", Icon::X, "閉じる", Some("esc"), true, cx, |t, _, cx| {
                    t.close_detail(cx)
                }),
            );

        let mut body = div()
            .id("detail-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.session_card(sel, cx));
        if sel.lane != 0 {
            body = body.child(self.subagent_row(sel, cx));
        }
        body = body.child(self.selection_summary(sel));
        body = body.children(self.event_cards(sel, cx));
        if evs.len() > DETAIL_MAX {
            body = body.child(
                div().text_xs().text_color(c(theme::MUTED)).child(format!("… 他 {} 件", evs.len() - DETAIL_MAX)),
            );
        }
        panel.child(header).child(body).into_any_element()
    }

    /// セッションの概要（読み込み元・プロジェクト・タイトル・期間・モデル）と操作
    fn session_card(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = &self.store.sessions[sel.session];
        let lane = &s.lanes[sel.lane];
        let (logo, logo_color) = icons::source_icon(s.source);
        let meta = |k: &'static str, v: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(div().w(px(76.0)).flex_none().text_color(c(theme::SUBTLE)).child(k))
                .child(div().flex_1().min_w_0().child(v))
        };
        let model = match (lane.model.is_empty(), lane.effort.is_empty()) {
            (true, true) => None,
            (false, true) => Some(lane.model.clone()),
            (true, false) => Some(format!("エフォート {}", lane.effort)),
            (false, false) => Some(format!("{} · エフォート {}", lane.model, lane.effort)),
        };
        let session_usage = UsageSum::of(s.lanes.iter().flat_map(|l| &l.events));
        let models = UsageSum::by_model(s.lanes.iter().flat_map(|l| &l.events));
        let lane_usage = (s.lanes.len() > 1).then(|| UsageSum::of(&lane.events)).filter(|u| u.calls > 0);
        let resume = resume_command(s);
        let resumed = self.copied == Some("copy-resume");
        let origin = s.lanes.first().map(|l| l.origin.clone()).unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(icons::icon(logo, logo_color))
                    .child(s.source.label())
                    .child("·")
                    .child(project_tile(&s.cwd, 14.0))
                    .child(div().truncate().child(self.project_name(&s.cwd)))
                    .when(s.headless, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(c(theme::BORDER))
                                .child("非対話"),
                        )
                    }),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).child(if s.title.is_empty() {
                "(無題)".to_string()
            } else {
                s.title.clone()
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .child(meta(
                        "期間",
                        div()
                            .child(format!(
                                "{} – {}（{}）",
                                local(s.start).format("%Y/%m/%d %H:%M"),
                                local(s.end).format(if local(s.start).date_naive() == local(s.end).date_naive() {
                                    "%H:%M"
                                } else {
                                    "%m/%d %H:%M"
                                }),
                                fmt_dur(s.end - s.start)
                            ))
                            .into_any_element(),
                    ))
                    .when_some(model, |d, m| d.child(meta("モデル", div().child(m).into_any_element())))
                    .when(session_usage.calls > 0, |d| {
                        d.child(meta("トークン", usage_view(&session_usage).into_any_element()))
                    })
                    .when_some(lane_usage, |d, u| d.child(meta("このレーン", usage_view(&u).into_any_element())))
                    .when(models.len() > 1 || models.first().is_some_and(|m| m.1.unpriced > 0), |d| {
                        d.child(meta("モデル別", model_table(&models).into_any_element()))
                    })
                    .child(meta("ディレクトリ", div().child(tilde(&s.cwd)).into_any_element()))
                    .child(meta(
                        "セッション",
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().min_w_0().truncate().font_family("Menlo").child(s.id.clone()))
                            .child(self.copy_button("copy-id", s.id.clone(), cx).flex_none())
                            .into_any_element(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        self.action_button(
                            "resume",
                            if resumed { Icon::Check } else { Icon::Terminal },
                            if resumed { "コピーしました" } else { "再開コマンドをコピー" },
                        )
                        .tooltip(tip(resume.clone(), None))
                        .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(resume.clone()));
                            t.flash_copied("copy-resume", cx);
                        })),
                    )
                    .when(!origin.is_empty(), |d| {
                        d.child(
                            self.action_button("reveal-log", Icon::FolderOpen, "ログを Finder で表示")
                                .tooltip(tip(origin.clone(), None))
                                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                    cx.reveal_path(std::path::Path::new(&origin));
                                })),
                        )
                    }),
            )
    }

    fn action_button(&self, id: &'static str, icon: Icon, label: &'static str) -> gpui::Stateful<Div> {
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
            .child(icons::icon(icon, if icon == Icon::Check { theme::OK } else { theme::MUTED }))
            .child(label)
    }

    /// サブエージェントのレーンなら、その名前と起動元への移動
    fn subagent_row(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let (session, parent_id) = (sel.session, lane.spawned_by.clone());
        div()
            .flex()
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(c(0xa77bf3).opacity(0.08))
            .border_1()
            .border_color(c(0xa77bf3).opacity(0.3))
            .text_xs()
            .child(icons::icon(Icon::Bot, theme::SUB))
            .child(div().flex_1().min_w_0().text_color(c(theme::SUB)).child(format!("サブエージェント: {}", lane.name)))
            .when_some(parent_id, |d, id| {
                d.child(self.icon_button("to-parent", Some(Icon::ArrowUp), "起動元", false, cx, move |t, w, cx| {
                    let s = &t.store.sessions[session];
                    if let Some(i) = s.lanes[0].events.iter().position(|e| e.call_id.as_deref() == Some(id.as_str())) {
                        let sel = Selection { session, lane: 0, first: i, last: i };
                        t.navigate(sel, w, cx);
                        t.reveal(sel, false, w);
                    }
                }))
            })
    }

    /// 選択範囲の期間と件数、多いツールの内訳
    fn selection_summary(&self, sel: Selection) -> Div {
        let evs = &self.store.sessions[sel.session].lanes[sel.lane].events[sel.first..=sel.last];
        let (start, end) = self.sel_range(sel);
        let mut counts: Vec<(&str, usize, u32)> = Vec::new();
        for e in evs {
            match counts.iter_mut().find(|(n, _, _)| *n == e.name) {
                Some(x) => x.1 += 1,
                None => counts.push((&e.name, 1, theme::event_color(e))),
            }
        }
        counts.sort_by_key(|a| std::cmp::Reverse(a.1));
        let usage = UsageSum::of(evs);
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .pt_1()
            .pb_1p5()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_xs()
            .child(div().text_color(c(theme::MUTED)).child(format!(
                "{} – {}  ·  {}  ·  {} 件",
                local(start).format("%m/%d %H:%M:%S"),
                local(end).format("%H:%M:%S"),
                fmt_dur(end - start),
                evs.len()
            )))
            .when(usage.calls > 0 && evs.len() > 1, |d| d.child(usage_view(&usage)))
            .when(evs.len() > 1, |d| {
                d.child(div().flex().flex_wrap().gap_1().children(counts.iter().take(8).map(|&(name, n, col)| {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_sm()
                        .bg(c(theme::BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .child(dot(col, 7.0))
                        .child(name.to_string())
                        .child(div().text_color(c(theme::SUBTLE)).child(format!("×{n}")))
                })))
            })
    }

    fn event_cards(&self, sel: Selection, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = &self.store.sessions[sel.session];
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        let single = evs.len() == 1;
        let cache = self.detail_cache.as_ref().filter(|d| d.sel == sel);
        let texts = cache.map(|d| d.texts.as_slice()).unwrap_or(&[]);
        let mut out = Vec::new();
        for (i, e) in evs.iter().enumerate().take(DETAIL_MAX) {
            let (input, output) = texts.get(i).map(|(a, b)| (a.as_str(), b.as_str())).unwrap_or(("", ""));
            let raw = cache.and_then(|d| d.raws.get(i)).and_then(|r| r.as_deref());
            let idx = sel.first + i;
            let color = theme::event_color(e);
            let mut card = div()
                .id(ElementId::NamedInteger("ev".into(), idx as u64))
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(c(0x23272f))
                .border_l_2()
                .border_color(c(color))
                .when(!single, |d| d.cursor_pointer().hover(|d| d.bg(c(0x292e38))))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .text_xs()
                        .child(div().text_color(c(theme::MUTED)).child(local(e.start).format("%H:%M:%S").to_string()))
                        .child(div().font_weight(FontWeight::BOLD).text_color(c(color)).child(e.name.clone()))
                        .when(e.end > e.start, |d| {
                            d.child(div().text_color(c(theme::MUTED)).child(fmt_dur(e.end - e.start)))
                        })
                        .when(!e.usage.is_empty(), |d| {
                            d.child(div().flex_1()).child(usage_chip(&e.usage, e.model.as_deref()))
                        }),
                );
            if !e.summary.is_empty() && e.kind == Kind::Tool || e.kind == Kind::Agent {
                card = card.child(div().text_xs().child(e.summary.clone()));
            }
            if e.kind == Kind::Agent
                && let Some(li) = e.call_id.as_deref().and_then(|id| s.lane_spawned_by(id))
            {
                let session = sel.session;
                let n = s.lanes[li].events.len();
                card = card.child(div().flex().child(self.icon_button(
                    SharedString::from(format!("to-sub-{idx}")),
                    Some(Icon::Bot),
                    format!("サブエージェント ({n} 件) を開く"),
                    false,
                    cx,
                    move |t, w, cx| {
                        let sel = Selection { session, lane: li, first: 0, last: n - 1 };
                        t.navigate(sel, w, cx);
                        t.reveal(sel, true, w);
                    },
                )));
            }
            if single && let Some(raw) = raw {
                let show = self.show_raw;
                card = card.child(self.segmented(
                    "rawmode",
                    &[(false, "整形"), (true, "JS")],
                    Some(show),
                    cx,
                    |t, v, _, cx| {
                        t.show_raw = v;
                        t.detail_changed(cx);
                    },
                ));
                if show {
                    card = card.child(self.copyable("copy-js", truncate(raw, 6000), raw.to_string(), true, cx));
                } else {
                    card = card.child(self.copyable("copy-fmt", input.to_string(), input.to_string(), true, cx));
                }
            } else if single {
                let mono = e.kind == Kind::Tool || e.kind == Kind::Agent;
                card = card.child(self.copyable("copy-in", input.to_string(), input.to_string(), mono, cx));
            } else if e.kind != Kind::Tool {
                card = card.child(code_box(input.to_string(), e.kind == Kind::Agent));
            }
            if single && !output.is_empty() {
                card = card.child(div().text_xs().text_color(c(theme::MUTED)).child("結果")).child(self.copyable(
                    "copy-out",
                    output.to_string(),
                    output.to_string(),
                    true,
                    cx,
                ));
            }
            let (session, lane_i) = (sel.session, sel.lane);
            if !single {
                card = card.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.navigate(Selection { session, lane: lane_i, first: idx, last: idx }, window, cx);
                }));
            }
            out.push(card.into_any_element());
        }
        out
    }
}

/// トークン数を短く（1,234 / 12.3k / 1.23M）
fn fmt_tokens(n: u64) -> String {
    match n {
        0..10_000 => super::fmt_n(n as usize),
        10_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// 使用量の内訳の色: 入力・キャッシュ読込・キャッシュ書込・出力
const USAGE_COLORS: [u32; 4] = [0x5b9cf5, 0x3f6a9e, 0xd29a4a, 0x7fd18b];

/// 使用量の合計・内訳の帯・凡例
fn usage_view(u: &UsageSum) -> Div {
    let total = u.total().max(1) as f32;
    let parts = [
        ("入力", u.input, USAGE_COLORS[0]),
        ("キャッシュ読込", u.cache_read, USAGE_COLORS[1]),
        ("キャッシュ書込", u.cache_write, USAGE_COLORS[2]),
        ("出力", u.output, USAGE_COLORS[3]),
    ];
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .gap_1p5()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(fmt_tokens(u.total())))
                .when(u.calls > u.unpriced, |d| {
                    d.child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::OK)).child(fmt_usd(u.cost)))
                })
                .child(div().text_color(c(theme::SUBTLE)).child(format!("· {} 回", super::fmt_n(u.calls as usize))))
                .when(u.unpriced > 0, |d| {
                    d.child(
                        div()
                            .id("unpriced")
                            .text_color(c(theme::SUBTLE))
                            .child(format!("（料金不明 {} 回）", super::fmt_n(u.unpriced as usize)))
                            .tooltip(tip(
                                format!(
                                    "料金表にないモデル。{} に追加できます",
                                    tilde(&crate::pricing::path().to_string_lossy())
                                ),
                                None,
                            )),
                    )
                }),
        )
        .child(
            div().flex().h(px(6.0)).w_full().rounded_sm().overflow_hidden().bg(c(theme::GRID)).children(
                parts.iter().filter(|p| p.1 > 0).map(|&(_, n, col)| {
                    div().h_full().flex_basis(gpui::relative(n as f32 / total)).flex_shrink_0().bg(c(col))
                }),
            ),
        )
        .child(div().flex().flex_wrap().gap_x_2().gap_y_0p5().text_color(c(theme::MUTED)).children(
            parts.iter().filter(|p| p.1 > 0).map(|&(label, n, col)| {
                div().flex().items_center().gap_1().child(dot(col, 7.0)).child(format!("{label} {}", fmt_tokens(n)))
            }),
        ))
        .when(u.reasoning > 0, |d| {
            d.child(div().text_color(c(theme::SUBTLE)).child(format!("出力のうち推論 {}", fmt_tokens(u.reasoning))))
        })
}

/// 料金（USD）
fn fmt_usd(v: f64) -> String {
    if v < 0.01 && v > 0.0 {
        format!("${v:.4}")
    } else {
        format!("${v:.2}")
    }
}

/// モデルごとのトークン数と料金
fn model_table(models: &[(String, UsageSum)]) -> Div {
    div().flex().flex_col().gap_0p5().children(models.iter().map(|(m, u)| {
        div()
            .flex()
            .gap_2()
            .child(div().flex_1().min_w_0().truncate().child(if m.is_empty() {
                "(不明)".to_string()
            } else {
                m.clone()
            }))
            .child(div().flex_none().text_color(c(theme::MUTED)).child(fmt_tokens(u.total())))
            .child(div().flex_none().w(px(64.0)).flex().justify_end().child(if u.unpriced > 0 && u.cost == 0.0 {
                div().text_color(c(theme::SUBTLE)).child("—")
            } else {
                div().text_color(c(theme::OK)).child(fmt_usd(u.cost))
            }))
    }))
}

/// イベント見出しに添える使用量（入力はキャッシュ込み）
fn usage_chip(u: &Usage, model: Option<&str>) -> gpui::Stateful<Div> {
    let input = u.input as u64 + u.cache_read as u64 + u.cache_write as u64;
    let mut detail = format!(
        "入力 {} / キャッシュ読込 {} / キャッシュ書込 {} / 出力 {}",
        super::fmt_n(u.input as usize),
        super::fmt_n(u.cache_read as usize),
        super::fmt_n(u.cache_write as usize),
        super::fmt_n(u.output as usize)
    );
    if u.reasoning > 0 {
        detail.push_str(&format!("（うち推論 {}）", super::fmt_n(u.reasoning as usize)));
    }
    if let Some(m) = model {
        detail.push_str(&format!("\n{m}"));
        if let Some(p) = crate::pricing::price(m) {
            detail.push_str(&format!(" · {}", fmt_usd(p.cost(u))));
        }
    }
    div()
        .id(SharedString::from(format!("usage-{detail}")))
        .text_color(c(theme::SUBTLE))
        .child(format!("↑{} ↓{}", fmt_tokens(input), fmt_tokens(u.output as u64)))
        .tooltip(tip(detail, None))
}

/// exec の JS を整形する。初回はモック実行してキャッシュに保存する
pub(super) fn exec_detail_cached(cache: Option<&JsCache>, js: &str) -> String {
    let h = crate::jsmock::hash(js);
    if let Some(d) = cache.and_then(|c| c.get(h)) {
        return d;
    }
    let d = loader::exec_detail(js);
    if let Some(c) = cache {
        c.put(h, &d);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_quotes_cwd() {
        let s = Session {
            id: "abc".into(),
            source: Source::Claude,
            cwd: "/w/it's here".into(),
            headless: false,
            title: String::new(),
            lanes: Vec::new(),
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        assert_eq!(resume_command(&s), r#"cd '/w/it'\''s here' && claude --resume abc"#);
        let s = Session { source: Source::Codex, cwd: "/w".into(), ..s };
        assert_eq!(resume_command(&s), "cd '/w' && codex resume abc");
    }
}
