//! サイドバー。上に検索欄を置き、その下に進行中のセッション・プロジェクト・セッションの一覧を並べる。
//! 検索語があるときは一覧の代わりに検索結果を出す

mod navigator;
pub(super) use navigator::NavigatorList;
use gpui::{
    AnyElement, ClickEvent, Context, Div, ElementId, Focusable, FontWeight, HighlightStyle, SharedString, Stateful,
    StyledText, WeakEntity, Window, div, prelude::*, px,
};

use super::widgets::{Marquee, dot, event_icon, kbd, project_tile, tip};
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(super) enum NavRow {
    Section(&'static str, usize),
    Live(usize),
    AllProjects,
    Project(usize),
    Session(usize),
    More(usize),
}

/// 一覧の 1 行の高さ
const ROW: f32 = 28.0;

impl Rireki {
    pub(super) fn sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let started = self.profile.as_ref().filter(|p| p.sidebar).map(|_| std::time::Instant::now());
        let placeholder = match (&self.focus, self.source) {
            (Some(f), _) => format!("{} を検索", self.project_name(f)),
            (None, SourceFilter::All) => "すべての履歴を検索".to_string(),
            (None, SourceFilter::Claude) => "Claude Code の履歴を検索".to_string(),
            (None, SourceFilter::Codex) => "Codex の履歴を検索".to_string(),
        };
        self.search_input.update(cx, |i, _| i.set_placeholder(placeholder));
        let body = if self.results.is_some() { self.search_results(window, cx) } else { self.navigator(window, cx) };
        if let Some(started) = started {
            println!("{}", serde_json::json!({"ui_profile":"sidebar_build", "built":self.navigator_rendered.get(),
                "ms":started.elapsed().as_secs_f64()*1000.0}));
        }
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

    fn navigator(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let at = super::now();
        let key = navigator::Key {
            generation: self.data_gen, limit: self.session_limit,
            folded: ["live", "projects", "sessions"].map(|s| self.folded.contains(s)),
            width: self.pane_full_widths(window).0.to_bits(),
        };
        self.navigator_list.prepare(key, at, || {
            let expires = self.scope.iter().map(|&si| self.store.sessions[si].end + crate::index::LIVE_SECS)
                .filter(|&end| end > at).min_by(f64::total_cmp).unwrap_or(f64::INFINITY);
            (self.navigator_rows(), self.projects.iter().map(|p| p.cwd.clone()).collect(), expires)
        });
        self.navigator_rendered.set(0);
        let rows = self.navigator_list.rows().unwrap();
        let offset = self.navigator_list.state.logical_scroll_top();
        let rem = f32::from(window.rem_size());
        let height = f32::from(window.viewport_size().height) - super::STRIP_H - HEADER_H - super::FOOTER_H - super::GAP * 2.0;
        // 画面内に収まる数行では、List の高さ索引や外枠を作る負荷の方が大きい。
        let fits = rows.len() <= 4 && rows.len() as f32 * (rem * 2.0 + 40.0) + rem <= height
            && offset.item_ix == 0 && offset.offset_in_item == px(0.0);
        let eager = self.profile.as_ref().is_some_and(|p| p.eager_sidebar);
        self.navigator_virtual.set(!eager && !fits);
        if eager || fits {
            if !eager { self.sidebar_scroll.set_offset(gpui::point(px(0.0), px(0.0))); }
            let mut list = div().id("sidebar-scroll").flex_1().min_h_0()
                .overflow_y_scroll().track_scroll(&self.sidebar_scroll).pb_3();
            for &row in &**rows {
                list = list.child(self.navigator_item(row, None, cx));
            }
            return list.into_any_element();
        }
        let weak = cx.weak_entity();
        gpui::list(self.navigator_list.state.clone(), move |index, _, cx| {
            weak.upgrade().map_or_else(|| div().into_any_element(), |app| {
                app.update(cx, |app, cx| {
                    app.navigator_list.row(index).map_or_else(|| div().into_any_element(), |row| {
                        // 外枠をパネル幅に広げ、行の余白も高さに含めて見出しや「さらに表示」の間隔を保つ。
                        let previous = index.checked_sub(1).and_then(|i| app.navigator_list.row(i));
                        div().w_full().flex().flex_col().child(app.navigator_item(row, previous, cx)).into_any_element()
                    })
                })
            })
        }).flex_1().min_h_0().pb_3().into_any_element()
    }

    pub(super) fn navigator_rows(&self) -> Vec<NavRow> {
        let mut rows = Vec::new();
        let mut live: Vec<_> = self.scope.iter().copied()
            .filter(|&si| is_live(self.store.sessions[si].end)).collect();
        live.sort_by(|&a, &b| self.store.sessions[b].end.total_cmp(&self.store.sessions[a].end));
        if !live.is_empty() {
            rows.push(NavRow::Section("live", live.len()));
            if !self.folded.contains("live") {
                rows.extend(live.into_iter().map(NavRow::Live));
            }
        }
        rows.push(NavRow::Section("projects", self.projects.len()));
        if !self.folded.contains("projects") {
            rows.push(NavRow::AllProjects);
            rows.extend((0..self.projects.len()).map(NavRow::Project));
        }
        rows.push(NavRow::Section("sessions", self.order.len()));
        if !self.folded.contains("sessions") {
            let mut recent = self.order.clone();
            recent.sort_by(|&a, &b| self.store.sessions[b].end.total_cmp(&self.store.sessions[a].end));
            let rest = recent.len().saturating_sub(self.session_limit);
            rows.extend(recent.into_iter().take(self.session_limit).map(NavRow::Session));
            if rest > 0 { rows.push(NavRow::More(rest)); }
        }
        rows
    }

    fn navigator_item(&self, row: NavRow, previous: Option<NavRow>, cx: &mut Context<Self>) -> AnyElement {
        self.navigator_rendered.set(self.navigator_rendered.get() + 1);
        let sel_session = self.selected.map(|s| s.session);
        match row {
            NavRow::Section(key, count) => {
                let title: SharedString = match key {
                    "live" => "進行中".into(),
                    "projects" => "プロジェクト".into(),
                    _ => self.focus.as_ref().map_or_else(|| "最近のセッション".into(),
                        |f| format!("{} のセッション", self.project_name(f)).into()),
                };
                self.section(key, title, Some(count), cx)
                    .when(matches!(previous, Some(NavRow::Section(_, _))), |d| d.mt(gpui::rems(0.625)))
                    .into_any_element()
            }
            NavRow::Live(si) => {
                let s = &self.store.sessions[si];
                let (logo, logo_color) = icons::source_icon(s.source);
                let selected = sel_session == Some(si);
                self.row(ElementId::NamedInteger("live".into(), si as u64), selected)
                    .py_1()
                    .child(dot(theme::LIVE, 7.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(Marquee::new("title", title_of(&s.title)))
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
                    .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.open_session(si, window, cx))).into_any_element()
            }
            NavRow::AllProjects => {
                let all = self.focus.is_none();
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
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.set_focus(None, cx))).into_any_element()
            }
            NavRow::Project(i) => {
                let p = &self.projects[i];
                let selected = self.focus.as_ref() == Some(&p.cwd);
                let cwd = p.cwd.clone();
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
                    })).into_any_element()
            }
            NavRow::Session(si) => {
                let s = &self.store.sessions[si];
                let (logo, logo_color) = icons::source_icon(s.source);
                let selected = sel_session == Some(si);
                let live = is_live(s.end);
                self.row(ElementId::NamedInteger("ses".into(), si as u64), selected)
                    .child(icons::icon(logo, if selected { theme::TEXT } else { logo_color }))
                    .child(div().flex_1().min_w_0().child(Marquee::new("title", title_of(&s.title))))
                    .when(live, |d| d.child(dot(theme::LIVE, 7.0)))
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(if selected { gpui::white().opacity(0.8) } else { c(theme::SUBTLE) })
                            .child(rel_time(s.end)),
                    )
                    .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.open_session(si, window, cx))).into_any_element()
            }
            NavRow::More(rest) => {
                div()
                    .id("more-sessions")
                    .mx_2()
                    .mt_1()
                    .when(matches!(previous, Some(NavRow::Section(_, _))), |d| d.mt(gpui::rems(0.125)))
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
                    })).into_any_element()
            }
        }
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

    fn search_results(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.results else { return div().into_any_element() };
        let count = 1 + r.sessions.len() + r.events.len()
            + usize::from(!r.sessions.is_empty()) + usize::from(!r.events.is_empty())
            + usize::from(r.is_empty());
        let key = (self.search_gen.load(super::Ordering::Relaxed), count, self.pane_full_widths(window).0.to_bits());
        let old = self.result_list_key.replace(key);
        if (old.0, old.1) != (key.0, key.1) {
            self.result_list.reset(count);
        } else if old.2 != key.2 {
            // 折り返しで高さが変わる。現在の項目を保ち、画面外の高さも測り直す
            self.result_list.splice(0..count, count);
        }
        let terms: Vec<String> = self.query.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
        let weak = cx.weak_entity();
        gpui::list(self.result_list.clone(), move |row, _, cx| {
            weak.upgrade().map_or_else(|| div().into_any_element(), |app| {
                app.update(cx, |app, cx| {
                    div().w_full().flex().flex_col().child(app.search_result_item(row, &terms, cx)).into_any_element()
                })
            })
        }).flex_1().min_h_0().pb_3().into_any_element()
    }

    fn search_result_item(&self, row: usize, terms: &[String], cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = &self.results else { return div().into_any_element() };
        let ns = r.sessions.len();
        if row == 0 {
            let filtered = self.focus.is_some() || self.source != SourceFilter::All;
            let summary = if self.searching { "検索中…".to_string() }
                else if r.is_empty() { "一致する履歴はありません".to_string() }
                else if r.total > r.events.len() {
                    format!("{} 件（新しい {} 件を表示）", r.sessions.len() + r.total, r.events.len())
                } else { format!("{} 件", r.len()) };
            return div()
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
                                        t.sidebar_changed(cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                }).into_any_element();
        }
        let mut row = row - 1;
        if ns > 0 {
            if row == 0 { return result_heading("セッション", ns).into_any_element(); }
            row -= 1;
            if row < ns {
                let i = row;
                let s = &self.store.sessions[r.sessions[i]];
                let (logo, logo_color) = icons::source_icon(s.source);
                let title = title_of(&s.title);
                return self.result_row_frame(i)
                        .child(icons::icon(logo, logo_color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(Marquee::new("title", title.clone()).with_highlights(highlights(&title, terms)))
                                .child(div().text_xs().text_color(c(theme::MUTED)).truncate().child(format!(
                                    "{} · {}",
                                    self.project_name(&s.cwd),
                                    rel_time(s.end)
                                ))),
                        )
                        .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.click_result(i, window, cx))).into_any_element();
            }
            row -= ns;
        }
        if !r.events.is_empty() {
            if row == 0 { return result_heading("イベント", r.total).into_any_element(); }
            let k = row - 1;
            let Some(h) = r.events.get(k) else { return div().into_any_element() };
            let i = ns + k;
            let s = &self.store.sessions[h.session];
            let e = &s.lanes[h.lane].events[h.event];
            let (color, ic) = (theme::event_color(e), theme::event_icon(e));
            let name_len = e.name.len() as u32;
            let summary: SharedString = if h.at.0 > name_len {
                let a = (h.at.0 - name_len - 1) as usize;
                let b = (h.at.1 - name_len - 1) as usize;
                snippet(&e.summary, a..b).into()
            } else { one_line(&e.summary, 120).into() };
            let lane_name = (h.lane != 0).then(|| s.lanes[h.lane].name.clone());
            return self.result_row_frame(i)
                        .items_start()
                        .child(div().pt(px(2.0)).child(event_icon(ic, color, 13.0)))
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
                                                .child(highlight(e.name.clone().into(), terms)),
                                        )
                                        .child(div().flex_1().min_w_0().text_color(c(theme::MUTED)).child(
                                            Marquee::new("session-title", match &lane_name {
                                                Some(l) => format!("{} › {}", title_of(&s.title), one_line(l, usize::MAX)),
                                                None => title_of(&s.title),
                                            }),
                                        ))
                                        .child(div().flex_none().text_color(c(theme::SUBTLE)).child(when(e.start))),
                                )
                                .when(!e.summary.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .line_clamp(2)
                                            .text_color(c(theme::TEXT))
                                            .child(highlight(summary, terms)),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |t, _: &ClickEvent, window, cx| t.click_result(i, window, cx))).into_any_element();
        }
        if r.is_empty() { return div()
                    .mx_3()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .text_color(c(theme::SUBTLE))
                    .child("セッションのタイトル・ディレクトリと、ツール呼び出しやメッセージの要約から探します。")
                    .child("空白で区切ると、すべての語を含むものに絞り込みます。").into_any_element(); }
        div().into_any_element()
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
    if t.is_empty() { "(無題)".to_string() } else { one_line(t, usize::MAX) }
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
fn highlight(text: SharedString, terms: &[String]) -> StyledText {
    let runs = highlights(&text, terms);
    StyledText::new(text).with_highlights(runs)
}

fn highlights(text: &str, terms: &[String]) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    if terms.is_empty() || text.is_empty() { return Vec::new(); }
    let mut ranges = {
        // ASCIIの小文字化は一致するたびに繰り返さず、一回だけ行う。
        let lower = text.to_ascii_lowercase();
        let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
        if lower.len() < 64 {
            for t in terms {
                let mut from = 0;
                while let Some(i) = memchr::memmem::find(&lower.as_bytes()[from..], t.as_bytes()) {
                    let start = from + i;
                    let end = start + t.len();
                    if !text.is_char_boundary(start) || !text.is_char_boundary(end) { break; }
                    ranges.push(start..end);
                    from = end;
                    if from >= text.len() { break; }
                }
            }
        } else {
            for t in terms {
                // 同じ長さなら全文の比較だけで決まる。全面一致なら他の語も同じ範囲に含まれる。
                if t.len() >= lower.len() {
                    if t.as_bytes() == lower.as_bytes() {
                        ranges.clear();
                        ranges.push(0..text.len());
                        break;
                    }
                    continue;
                }
                // 長い表示文字列では、検索器を語ごとに一度だけ準備する。
                for start in memchr::memmem::find_iter(lower.as_bytes(), t.as_bytes()) {
                    let end = start + t.len();
                    if !text.is_char_boundary(start) || !text.is_char_boundary(end) { break; }
                    ranges.push(start..end);
                    if end >= text.len() { break; }
                }
            }
        }
        ranges
    };
    // 同じ開始の並びに依存せず最大の終了を使うので、安定ソートの作業領域は不要。
    ranges.sort_unstable_by_key(|r| r.start);
    let mut merged = 0;
    for i in 0..ranges.len() {
        if merged > 0 && ranges[i].start <= ranges[merged - 1].end {
            ranges[merged - 1].end = ranges[merged - 1].end.max(ranges[i].end);
        } else {
            ranges[merged] = ranges[i].clone();
            merged += 1;
        }
    }
    ranges.truncate(merged);
    let style = HighlightStyle {
        color: Some(gpui::white()),
        background_color: Some(c(theme::ACCENT).opacity(0.35)),
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    ranges.into_iter().map(|r| (r, style)).collect()
}

/// 一致した位置（`range`）が見えるよう、長い文字列の前後を省いた 1 行
fn snippet(text: &str, range: std::ops::Range<usize>) -> String {
    const BEFORE: usize = 24;
    const LIMIT: usize = 160;
    let needle = text.get(range).unwrap_or("");
    // 検索結果の一致位置は空白のない一語。複数語の範囲は従来どおり整形後に探す。
    if needle.chars().any(char::is_whitespace) {
        let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let at = line.find(needle).unwrap_or(0);
        let chars_before = line[..at].chars().count();
        if chars_before <= BEFORE { return one_line(&line, LIMIT); }
        let start = line.char_indices().nth(chars_before - BEFORE).map_or(0, |(i, _)| i);
        return format!("…{}", one_line(&line[start..], LIMIT));
    }
    // 空白をまとめても一語の最初の一致は変わらない。前後の表示に必要な文字だけ作る。
    let at = text.find(needle).unwrap_or(0);
    let mut before = ['\0'; BEFORE + 1];
    let mut count = 0;
    let mut space = false;
    for c in text[..at].chars().rev() {
        if c.is_whitespace() { space = true; continue; }
        if space {
            before[count] = ' ';
            count += 1;
            if count == before.len() { break; }
            space = false;
        }
        before[count] = c;
        count += 1;
        if count == before.len() { break; }
    }
    let leading = count > BEFORE;
    let mut chars = ['\0'; LIMIT];
    let mut len = count.min(BEFORE);
    // 切り出しの先頭が区切りなら、one_lineと同じく先頭の空白を除く。
    if leading && before[len - 1] == ' ' { len -= 1; }
    for (i, &c) in before[..len].iter().rev().enumerate() { chars[i] = c; }
    let mut space = false;
    let mut trailing = false;
    for c in text[at..].chars() {
        if c.is_whitespace() { space |= len > 0; continue; }
        if space {
            if len == LIMIT { trailing = true; break; }
            chars[len] = ' ';
            len += 1;
            space = false;
        }
        if len == LIMIT { trailing = true; break; }
        chars[len] = c;
        len += 1;
    }
    let bytes = chars[..len].iter().map(|c| c.len_utf8()).sum::<usize>()
        + usize::from(leading) * '…'.len_utf8() + usize::from(trailing) * '…'.len_utf8();
    let mut out = String::with_capacity(bytes);
    if leading { out.push('…'); }
    for &c in &chars[..len] { out.push(c); }
    if trailing { out.push('…'); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_keeps_unicode_and_repeated_matches_at_search_boundaries() {
        let terms = ["cargo", "日本語", "a", "aa", "aaa"].map(str::to_string);
        // 29/30/31文字の接頭辞で、表示文字列が63/64/65バイトになる。
        for padding in [0, 29, 30, 31, 32, 63, 64, 65, 1024] {
            let text = format!("{} 日本語 cargo CARGO cargo aaaaa", "z".repeat(padding));
            let p = padding + 1;
            let expected = vec![p..p + 9, p + 10..p + 15, p + 16..p + 21, p + 22..p + 27, p + 28..p + 33];
            let got: Vec<_> = highlights(&text, &terms).into_iter().map(|(r, _)| r).collect();
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn highlight_whole_text_covers_other_terms_and_skips_longer_needles() {
        for padding in [0, 64, 1024] {
            let text = format!("{} CARGO 日本語 aaaaa🙂", "z".repeat(padding));
            let full = text.to_ascii_lowercase();
            for terms in [
                vec![full.clone(), "cargo".to_string(), "日本語".to_string()],
                vec!["a".to_string(), "aa".to_string(), full.clone()],
                vec!["none".to_string(), full.clone(), "aaa".to_string()],
            ] {
                let got: Vec<_> = highlights(&text, &terms).into_iter().map(|(r, _)| r).collect();
                let expected: Vec<_> = std::iter::once(0..text.len()).collect();
                assert_eq!(got, expected);
            }
            let terms = vec!["x".repeat(text.len() + 1), full.replace("cargo", "other"), "cargo".to_string()];
            let got: Vec<_> = highlights(&text, &terms).into_iter().map(|(r, _)| r).collect();
            let expected: Vec<_> = std::iter::once(padding + 1..padding + 6).collect();
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn highlight_keeps_shared_text_alive_without_copying() {
        let source: std::sync::Arc<str> = std::sync::Arc::from("日本語 CARGO cargo".repeat(128));
        let terms = vec!["cargo".to_string()];
        let styled = highlight(source.clone().into(), &terms);
        assert_eq!(std::sync::Arc::strong_count(&source), 2);
        drop(styled);
        assert_eq!(std::sync::Arc::strong_count(&source), 1);
    }

    #[test]
    fn highlight_preserves_case_unicode_and_merged_ranges() {
        let expected_style = HighlightStyle {
            color: Some(gpui::white()),
            background_color: Some(c(theme::ACCENT).opacity(0.35)),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let cases = [
            ("Cargo cargo CARGO", vec!["cargo"], vec![0..5, 6..11, 12..17]),
            ("aaaaa ababa", vec!["a", "aa", "aaa", "ab", "aba", "ba"], vec![0..5, 6..11]),
            ("日本語🙂日本語", vec!["日本語", "🙂", "日本語"], std::iter::once(0..22).collect()),
            ("é e\u{301}", vec!["é", "e\u{301}"], vec![0..2, 3..6]),
            ("\0\tCARGO\n", vec!["\0", "cargo"], vec![0..1, 2..7]),
            ("Cargo", vec!["Cargo", "none"], vec![]),
            ("cargo", vec![], vec![]),
            ("", vec!["cargo"], vec![]),
        ];
        for (text, terms, ranges) in cases {
            let terms: Vec<_> = terms.into_iter().map(str::to_string).collect();
            let expected: Vec<_> = ranges.into_iter().map(|r| (r, expected_style)).collect();
            assert_eq!(highlights(text, &terms), expected);
        }
    }

    #[test]
    fn snippet_keeps_match_visible() {
        let long = format!("{} cargo test {}", "a".repeat(100), "b".repeat(10));
        let s = snippet(&long, 101..106);
        assert!(s.starts_with('…'));
        assert!(s.contains("cargo test"));
        assert_eq!(snippet("cargo build", 0..5), "cargo build");
    }


    #[test]
    fn snippet_preserves_unicode_whitespace_and_clipping() {
        fn reference(text: &str, range: std::ops::Range<usize>) -> String {
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

        let samples = ["", " \t\n\u{a0}", "cargo cargo\tbuild cargo build", "日本語\u{2003}検索\u{a0}語", "é🙂👩\u{200d}💻\u{301} 日\r\n本", "\0\u{85}字\u{2028}文字"];
        for text in samples {
            // 文字境界を外した位置と逆向きの範囲も、従来の空のneedleとして扱う。
            for a in 0..=text.len() + 1 {
                for b in 0..=text.len() + 1 {
                    assert_eq!(snippet(text, a..b), reference(text, a..b));
                }
            }
        }
        for n in [23, 24, 25, 26, 159, 160, 161, 1024] {
            for pad in ["a", "字", "🙂", "a ", "a\t\n", "\u{2003}字\u{a0}"] {
                let text = format!(" {}cargo{} ", pad.repeat(n), pad.repeat(n));
                let at = text.find("cargo").unwrap();
                for range in [at..at + 5, at + 1..at + 4, 0..0, 0..text.len(), at - 1..at + 5] {
                    assert_eq!(snippet(&text, range.clone()), reference(&text, range));
                }
            }
        }
    }

    #[test]
    fn base_names() {
        assert_eq!(super::super::base_name("/Users/a/rireki/"), "rireki");
        assert_eq!(super::super::base_name("/"), "/");
    }
}
