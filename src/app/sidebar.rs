//! サイドバー。上に検索欄を置き、その下に進行中のセッション・プロジェクト・セッションの一覧を並べる。
//! 検索語があるときは一覧の代わりに検索結果を出す

use gpui::{
    AnyElement, ClickEvent, Context, Div, ElementId, Focusable, FontWeight, HighlightStyle, SharedString, Stateful,
    StyledText, WeakEntity, Window, div, prelude::*, px,
};

use super::widgets::{dot, kbd, project_tile, tip};
use super::{HEADER_H, INNER_RADIUS, Rireki, is_live, rel_time, when};
use crate::icons::{self, Icon};
use crate::model::one_line;
use crate::prefs::SourceFilter;
use crate::theme::{self, c};

/// サイドバー。一覧や検索結果が変わったときだけ描き直すよう、別ビューとしてキャッシュする
pub struct SidebarView {
    pub(super) app: WeakEntity<Rireki>,
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.sidebar(window, cx)),
            None => div().into_any_element(),
        }
    }
}

/// 一覧の 1 行の高さ
const ROW: f32 = 28.0;

impl Rireki {
    pub(super) fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let placeholder = match (&self.focus, self.source) {
            (Some(f), _) => format!("{} を検索", self.project_name(f)),
            (None, SourceFilter::All) => "すべての履歴を検索".to_string(),
            (None, SourceFilter::Claude) => "Claude Code の履歴を検索".to_string(),
            (None, SourceFilter::Codex) => "Codex の履歴を検索".to_string(),
        };
        self.search_input.update(cx, |i, _| i.set_placeholder(placeholder));
        let body = if self.results.is_some() { self.search_results(cx) } else { self.navigator(cx) };
        div()
            .size_full()
            .flex()
            .flex_col()
            .rounded_l(px(INNER_RADIUS))
            .bg(c(theme::SIDEBAR))
            .border_r_1()
            .border_color(c(theme::BORDER))
            .text_size(px(13.0))
            .child(self.search_box(window, cx))
            .child(body)
            .into_any_element()
    }

    fn search_box(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let focused = self.search_input.focus_handle(cx).is_focused(window);
        let empty = self.query.is_empty();
        let input = self.search_input.clone();
        div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .id("search-box")
                    .flex_1()
                    .min_w_0()
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .rounded_md()
                    .bg(c(theme::BACKDROP))
                    .border_1()
                    .border_color(c(if focused { theme::ACCENT } else { theme::BORDER }))
                    .cursor_text()
                    .child(icons::icon(Icon::Search, if focused { theme::ACCENT } else { theme::MUTED }))
                    .child(input)
                    .when(!empty, |d| {
                        d.child(
                            div()
                                .id("search-clear")
                                .flex_none()
                                .p(px(2.0))
                                .rounded_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(c(theme::HOVER)))
                                .child(icons::icon(Icon::X, theme::MUTED).size_3())
                                .tooltip(tip("検索を解除", Some("esc")))
                                .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    t.clear_search(cx);
                                })),
                        )
                    })
                    .when(empty && !focused, |d| d.child(kbd("⌘K")))
                    .on_click(cx.listener(|t, _: &ClickEvent, window, cx| {
                        if !t.search_input.focus_handle(cx).is_focused(window) {
                            window.focus(&t.search_input.focus_handle(cx));
                        }
                    })),
            )
            .child(self.ghost_button(
                "sidebar-close",
                Icon::PanelLeftClose,
                "サイドバーを隠す",
                Some("⌃⌘S"),
                true,
                cx,
                |t, _, cx| t.toggle_sidebar(cx),
            ))
    }

    /// 折りたためる区分の見出し
    fn section(
        &self,
        key: &'static str,
        title: SharedString,
        count: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let folded = self.folded.contains(key);
        div()
            .id(SharedString::from(format!("sec-{key}")))
            .flex()
            .items_center()
            .gap_1()
            .mx_2()
            .mt_3()
            .mb_0p5()
            .px_1()
            .h(px(22.0))
            .rounded_sm()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(theme::MUTED))
            .cursor_pointer()
            .hover(|s| s.text_color(c(theme::TEXT)))
            .child(icons::icon(if folded { Icon::ChevronRight } else { Icon::ChevronDown }, theme::SUBTLE).size_3())
            .child(div().truncate().child(title))
            .when_some(count, |d, n| {
                d.child(div().text_color(c(theme::SUBTLE)).font_weight(FontWeight::NORMAL).child(n.to_string()))
            })
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                if !t.folded.remove(key) {
                    t.folded.insert(key.to_string());
                }
                t.persist();
                t.sidebar_changed(cx);
            }))
    }

    /// 一覧の行の外枠。選択中は塗りつぶし、それ以外はホバーで薄く色を付ける
    fn row(&self, id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .px_2()
            .min_h(px(ROW))
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(c(theme::SELECT)).text_color(gpui::white()))
            .when(!selected, |d| d.hover(|s| s.bg(gpui::white().opacity(0.06))))
    }

    fn navigator(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut list =
            div().id("sidebar-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.sidebar_scroll).pb_3();
        let sel_session = self.selected.map(|s| s.session);

        // 進行中（プロジェクトの絞り込みに関係なく、読み込み元の範囲で出す）
        let mut live: Vec<usize> =
            self.scope.iter().copied().filter(|&si| is_live(self.store.sessions[si].end)).collect();
        live.sort_by(|&a, &b| self.store.sessions[b].end.total_cmp(&self.store.sessions[a].end));
        if !live.is_empty() {
            list = list.child(self.section("live", "進行中".into(), Some(live.len()), cx));
            if !self.folded.contains("live") {
                for si in live {
                    let s = &self.store.sessions[si];
                    let (logo, logo_color) = icons::source_icon(s.source);
                    let selected = sel_session == Some(si);
                    list = list.child(
                        self.row(ElementId::NamedInteger("live".into(), si as u64), selected)
                            .py_1()
                            .child(dot(theme::LIVE, 7.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().truncate().child(title_of(&s.title)))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_xs()
                                            .text_color(if selected {
                                                gpui::white().opacity(0.8)
                                            } else {
                                                c(theme::MUTED)
                                            })
                                            .child(icons::icon(logo, logo_color).size_3())
                                            .child(div().truncate().child(self.project_name(&s.cwd)))
                                            .child("·")
                                            .child(div().flex_none().child(rel_time(s.end))),
                                    ),
                            )
                            .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.open_session(si, window, cx))),
                    );
                }
            }
        }

        // プロジェクト
        list = list.child(self.section("projects", "プロジェクト".into(), Some(self.projects.len()), cx));
        if !self.folded.contains("projects") {
            let all = self.focus.is_none();
            list = list.child(
                self.row("proj-all", all)
                    .child(
                        div()
                            .size(px(18.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icons::icon(Icon::Layers, if all { theme::TEXT } else { theme::MUTED })),
                    )
                    .child(div().flex_1().truncate().child("すべてのプロジェクト"))
                    .child(count_badge(self.scope.len(), all))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.set_focus(None, cx))),
            );
            for (i, p) in self.projects.iter().enumerate() {
                let selected = self.focus.as_ref() == Some(&p.cwd);
                let cwd = p.cwd.clone();
                list = list.child(
                    self.row(ElementId::NamedInteger("proj".into(), i as u64), selected)
                        .child(div().relative().flex_none().child(project_tile(&p.cwd, 18.0)).when(
                            is_live(p.last),
                            |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .bottom(px(-2.0))
                                        .right(px(-2.0))
                                        .size(px(8.0))
                                        .rounded_full()
                                        .bg(c(theme::LIVE))
                                        .border_1()
                                        .border_color(c(theme::SIDEBAR)),
                                )
                            },
                        ))
                        .child(div().flex_1().truncate().child(p.name.clone()))
                        .child(count_badge(p.sessions, selected))
                        .tooltip(tip(format!("{}\n最終: {}", p.cwd, rel_time(p.last)), None))
                        .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                            let f = if t.focus.as_ref() == Some(&cwd) { None } else { Some(cwd.clone()) };
                            t.set_focus(f, cx);
                        })),
                );
            }
        }

        // セッション（新しい順）
        let title: SharedString = match &self.focus {
            Some(f) => format!("{} のセッション", self.project_name(f)).into(),
            None => "最近のセッション".into(),
        };
        list = list.child(self.section("sessions", title, Some(self.order.len()), cx));
        if !self.folded.contains("sessions") {
            let mut recent = self.order.clone();
            recent.sort_by(|&a, &b| self.store.sessions[b].end.total_cmp(&self.store.sessions[a].end));
            let rest = recent.len().saturating_sub(self.session_limit);
            for si in recent.into_iter().take(self.session_limit) {
                let s = &self.store.sessions[si];
                let (logo, logo_color) = icons::source_icon(s.source);
                let selected = sel_session == Some(si);
                let live = is_live(s.end);
                let sub_label = if self.focus.is_none() {
                    format!("{} · {}", self.project_name(&s.cwd), super::local(s.start).format("%m/%d %H:%M"))
                } else {
                    super::local(s.start).format("%m/%d %H:%M").to_string()
                };
                list = list.child(
                    self.row(ElementId::NamedInteger("ses".into(), si as u64), selected)
                        .child(icons::icon(logo, if selected { theme::TEXT } else { logo_color }))
                        .child(div().flex_1().truncate().child(title_of(&s.title)))
                        .when(live, |d| d.child(dot(theme::LIVE, 7.0)))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(if selected { gpui::white().opacity(0.8) } else { c(theme::SUBTLE) })
                                .child(rel_time(s.end)),
                        )
                        .tooltip(tip(format!("{}\n{sub_label}", title_of(&s.title)), None))
                        .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.open_session(si, window, cx))),
                );
            }
            if rest > 0 {
                list = list.child(
                    div()
                        .id("more-sessions")
                        .mx_2()
                        .mt_1()
                        .px_2()
                        .h(px(ROW))
                        .flex()
                        .items_center()
                        .rounded_md()
                        .text_xs()
                        .text_color(c(theme::ACCENT))
                        .cursor_pointer()
                        .hover(|s| s.bg(gpui::white().opacity(0.06)))
                        .child(format!("さらに表示（残り {rest} 件）"))
                        .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                            t.session_limit += 100;
                            t.sidebar_changed(cx);
                        })),
                );
            }
        }
        list.into_any_element()
    }

    /// 検索結果の i 番目の項目が、スクロール領域の何番目の子か（`search_results` の並びと合わせる）
    pub(super) fn result_row(&self, i: usize) -> usize {
        let Some(r) = &self.results else { return 0 };
        let ns = r.sessions.len();
        // 先頭は件数の行
        let mut row = 1 + usize::from(ns > 0);
        if i < ns {
            return row + i;
        }
        row += ns + usize::from(!r.events.is_empty());
        row + (i - ns)
    }

    fn search_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.results else { return div().into_any_element() };
        let terms: Vec<String> = self.query.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
        let filtered = self.focus.is_some() || self.source != SourceFilter::All;
        let mut list =
            div().id("sidebar-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.sidebar_scroll).pb_3();
        let summary = if r.len() == 0 {
            "一致する履歴はありません".to_string()
        } else if r.total > r.events.len() {
            format!("{} 件（新しい {} 件を表示）", r.sessions.len() + r.total, r.events.len())
        } else {
            format!("{} 件", r.len())
        };
        list = list.child(
            div()
                .mx_3()
                .mt_2()
                .mb_1()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .text_color(c(theme::MUTED))
                .child(summary)
                .when(filtered, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(icons::icon(Icon::Filter, theme::SUBTLE).size_3())
                            .child(div().text_color(c(theme::SUBTLE)).child("絞り込み中の範囲から検索"))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("search-all")
                                    .text_color(c(theme::ACCENT))
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .child("すべてから検索")
                                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                        t.source = SourceFilter::All;
                                        t.set_focus(None, cx);
                                        t.refresh_results();
                                        t.sidebar_changed(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                }),
        );
        let ns = r.sessions.len();
        if ns > 0 {
            list = list.child(result_heading("セッション", ns));
            for (i, &si) in r.sessions.iter().enumerate() {
                let s = &self.store.sessions[si];
                let (logo, logo_color) = icons::source_icon(s.source);
                let title = title_of(&s.title);
                list = list.child(
                    self.result_row_frame(i)
                        .child(icons::icon(logo, logo_color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().child(highlight(&title, &terms)))
                                .child(div().text_xs().text_color(c(theme::MUTED)).truncate().child(format!(
                                    "{} · {}",
                                    self.project_name(&s.cwd),
                                    rel_time(s.end)
                                ))),
                        )
                        .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.click_result(i, window, cx))),
                );
            }
        }
        if !r.events.is_empty() {
            list = list.child(result_heading("イベント", r.total));
            for (k, h) in r.events.iter().enumerate() {
                let i = ns + k;
                let s = &self.store.sessions[h.session];
                let e = &s.lanes[h.lane].events[h.event];
                let color = theme::event_color(e);
                // 一致した位置が要約の中なら、その周辺を切り出して強調する
                let name_len = e.name.len() as u32;
                let summary: SharedString = if h.at.0 > name_len {
                    let a = (h.at.0 - name_len - 1) as usize;
                    let b = (h.at.1 - name_len - 1) as usize;
                    snippet(&e.summary, a..b).into()
                } else {
                    one_line(&e.summary, 120).into()
                };
                let lane_name = (h.lane != 0).then(|| s.lanes[h.lane].name.clone());
                list = list.child(
                    self.result_row_frame(i)
                        .items_start()
                        .child(div().pt(px(5.0)).child(dot(color, 7.0)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
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
                                                .child(highlight(&e.name, &terms)),
                                        )
                                        .child(div().flex_1().min_w_0().truncate().text_color(c(theme::MUTED)).child(
                                            match &lane_name {
                                                Some(l) => format!("{} › {}", title_of(&s.title), one_line(l, 40)),
                                                None => title_of(&s.title),
                                            },
                                        ))
                                        .child(div().flex_none().text_color(c(theme::SUBTLE)).child(when(e.start))),
                                )
                                .when(!e.summary.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .line_clamp(2)
                                            .text_color(c(theme::TEXT))
                                            .child(highlight(&summary, &terms)),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.click_result(i, window, cx))),
                );
            }
        }
        if r.len() == 0 {
            list = list.child(
                div()
                    .mx_3()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .text_color(c(theme::SUBTLE))
                    .child("セッションのタイトル・ディレクトリと、ツール呼び出しやメッセージの要約から探します。")
                    .child("空白で区切ると、すべての語を含むものに絞り込みます。"),
            );
        }
        list.into_any_element()
    }

    /// 検索結果をクリックで開く。入力欄にフォーカスを戻し、続けて ↑↓ で選べるようにする
    fn click_result(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_result(i, window, cx);
        window.focus(&self.search_input.focus_handle(cx));
    }

    /// 検索結果の 1 項目の外枠。キーボードで選んでいる項目は左に線を引く
    fn result_row_frame(&self, i: usize) -> Stateful<Div> {
        let cursor = self.result_cursor == i;
        let selected = self.selected.is_some_and(|sel| {
            let r = self.results.as_ref();
            let ns = r.map_or(0, |r| r.sessions.len());
            match r {
                Some(r) if i < ns => sel.session == r.sessions[i] && sel.lane == 0,
                Some(r) => r.events.get(i - ns).is_some_and(|h| {
                    sel.session == h.session && sel.lane == h.lane && sel.first == h.event && sel.last == h.event
                }),
                None => false,
            }
        });
        div()
            .id(ElementId::NamedInteger("res".into(), i as u64))
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .px_2()
            .py_1()
            .rounded_md()
            .border_l_2()
            .border_color(if cursor { c(theme::ACCENT) } else { gpui::transparent_black() })
            .cursor_pointer()
            .when(selected, |d| d.bg(c(theme::ACCENT_BG)))
            .when(cursor && !selected, |d| d.bg(gpui::white().opacity(0.05)))
            .hover(|s| s.bg(gpui::white().opacity(0.07)))
    }
}

fn title_of(t: &str) -> String {
    if t.is_empty() { "(無題)".to_string() } else { one_line(t, 120) }
}

fn count_badge(n: usize, selected: bool) -> Div {
    div()
        .flex_none()
        .text_xs()
        .text_color(if selected { gpui::white().opacity(0.8) } else { c(theme::SUBTLE) })
        .child(super::fmt_n(n))
}

fn result_heading(title: &'static str, n: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .mx_3()
        .mt_3()
        .mb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c(theme::MUTED))
        .child(title)
        .child(div().font_weight(FontWeight::NORMAL).text_color(c(theme::SUBTLE)).child(n.to_string()))
}

/// 検索語に一致する部分を強調した文字列
fn highlight(text: &str, terms: &[String]) -> StyledText {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for t in terms {
        let mut from = 0;
        while let Some(r) = crate::search::find(&text[from..], t) {
            ranges.push(from + r.start..from + r.end);
            from += r.end;
            if from >= text.len() {
                break;
            }
        }
    }
    ranges.sort_by_key(|r| r.start);
    // 重なりをまとめる
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(m) if r.start <= m.end => m.end = m.end.max(r.end),
            _ => merged.push(r),
        }
    }
    let style = HighlightStyle {
        color: Some(gpui::white()),
        background_color: Some(c(theme::ACCENT).opacity(0.35)),
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    StyledText::new(SharedString::from(text.to_string())).with_highlights(merged.into_iter().map(|r| (r, style)))
}

/// 一致した位置（`range`）が見えるよう、長い文字列の前後を省いた 1 行
fn snippet(text: &str, range: std::ops::Range<usize>) -> String {
    const BEFORE: usize = 24;
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // 空白をまとめると位置がずれるので、一致した文字列で探し直す
    let needle = text.get(range).unwrap_or("");
    let at = line.find(needle).unwrap_or(0);
    let chars_before = line[..at].chars().count();
    if chars_before <= BEFORE {
        return one_line(&line, 160);
    }
    let skip = chars_before - BEFORE;
    let start = line.char_indices().nth(skip).map_or(0, |(i, _)| i);
    format!("…{}", one_line(&line[start..], 160))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_keeps_match_visible() {
        let long = format!("{} cargo test {}", "a".repeat(100), "b".repeat(10));
        let s = snippet(&long, 101..106);
        assert!(s.starts_with('…'));
        assert!(s.contains("cargo test"));
        assert_eq!(snippet("cargo build", 0..5), "cargo build");
    }

    #[test]
    fn base_names() {
        assert_eq!(super::super::base_name("/Users/a/rireki/"), "rireki");
        assert_eq!(super::super::base_name("/"), "/");
    }
}
