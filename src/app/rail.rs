//! 上端の帯と、左端のアイコンレール（Slack のワークスペース切り替えとナビゲーションに当たる）

use gpui::{AnyElement, ClickEvent, Context, Decorations, Div, FontWeight, MouseButton, Stateful, Window, div, prelude::*, px};

use super::widgets::tip;
use super::{Popover, RAIL_W, Rireki, STRIP_H, fmt_n};
use crate::icons::{self, Icon};
use crate::prefs::{Mode, SourceFilter};
use crate::theme::{self, c};

impl Rireki {
    /// 上端の状態表示。macOS では透明なタイトルバーと重なるので操作部品を置かない。
    pub(super) fn top_strip(&self, window: &Window) -> Div {
        // GNOME など、Wayland のサーバー側タイトルバーを提供しない環境では操作部品を描く。
        let client = cfg!(target_os = "linux") && matches!(window.window_decorations(), Decorations::Client { .. });
        let status = match self.loading {
            Some((d, t)) if t > 0 => format!("索引を更新中 {d}/{t} ファイル"),
            Some(_) => "履歴を読み込み中…".to_string(),
            None => format!("{} セッション · {} プロジェクト", fmt_n(self.scope.len()), self.projects.len()),
        };
        div()
            .relative()
            .h(px(STRIP_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .text_xs()
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::MUTED)).child("rireki"))
            .child(div().text_color(c(theme::SUBTLE)).child("·"))
            .when(self.loading.is_some(), |d| d.child(icons::icon(Icon::Refresh, theme::ACCENT).size_3()))
            .child(div().text_color(c(theme::SUBTLE)).child(status))
            .when(client, |d| {
                let button = |id: &'static str, text: &'static str, title: &'static str| {
                    div().id(id).w(px(30.0)).h_full().flex().items_center().justify_center()
                        .cursor_pointer().hover(|s| s.bg(c(theme::HOVER))).child(text)
                        .tooltip(tip(title, None))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                };
                d.px(px(100.0))
                    .on_mouse_down(MouseButton::Left, |event, window, cx| {
                        if event.click_count == 2 { window.zoom_window(); } else { window.start_window_move(); }
                        cx.stop_propagation();
                    })
                    .child(div().absolute().right(px(6.0)).top_0().h_full().flex()
                        .child(button("window-minimize", "−", "最小化").on_click(|_, w, _| w.minimize_window()))
                        .child(button("window-maximize", "□", "最大化 / 元に戻す").on_click(|_, w, _| w.zoom_window()))
                        .child(button("window-close", "×", "閉じる").on_click(|_, w, _| w.remove_window())))
            })
    }

    pub(super) fn rail(&self, cx: &mut Context<Self>) -> Div {
        let help = self.popover == Some(Popover::Help);
        div()
            .w(px(RAIL_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .pt_1()
            .pb_3()
            .gap_1()
            .child(self.source_tile(SourceFilter::All, cx))
            .child(self.source_tile(SourceFilter::Claude, cx))
            .child(self.source_tile(SourceFilter::Codex, cx))
            .child(div().w(px(28.0)).h(px(1.0)).my_2().bg(c(theme::BORDER)))
            .child(self.nav_item(
                "nav-timeline",
                Icon::Timeline,
                "タイムライン",
                crate::platform::shortcut("⌘1", "Ctrl+1"),
                !self.statistics_open && self.mode == Mode::Timeline,
                cx,
                |t, w, _| t.set_mode(Mode::Timeline, w),
            ))
            .child(self.nav_item(
                "nav-calendar",
                Icon::Calendar,
                "カレンダー",
                crate::platform::shortcut("⌘2 / ⌘3 / M", "Ctrl+2 / Ctrl+3 / M"),
                !self.statistics_open && self.mode != Mode::Timeline,
                cx,
                |t, w, _| t.set_mode(t.calendar, w),
            ))
            .child(self.nav_item(
                "nav-statistics",
                Icon::Chart,
                "統計",
                crate::platform::shortcut("⌘4", "Ctrl+4"),
                self.statistics_open,
                cx,
                |t, w, cx| t.show_statistics(w, cx),
            ))
            .child(div().flex_1())
            .child(self.nav_item("nav-help", Icon::Keyboard, "キー操作", "?", help, cx, |t, _, _| {
                t.popover = if t.popover == Some(Popover::Help) { None } else { Some(Popover::Help) };
            }))
    }

    /// 読み込み元の切り替え。Slack のワークスペースのように角丸の四角を並べ、選択中は輪で囲む
    fn source_tile(&self, s: SourceFilter, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.source == s;
        let (live_claude, live_codex) = self.live_sources;
        let logo = |i: Icon, color: u32| -> AnyElement { icons::icon(i, color).size(px(18.0)).into_any_element() };
        let (content, bg, name, live): (AnyElement, Option<u32>, &'static str, bool) = match s {
            SourceFilter::All => (
                // アイコン本体の周囲に余白があるため、36px の枠より大きく描く。
                icons::app_icon().size(px(44.0)).into_any_element(),
                None,
                "すべての履歴",
                live_claude || live_codex,
            ),
            SourceFilter::Claude => (logo(Icon::Claude, 0xd97757), Some(0x2b211d), "Claude Code", live_claude),
            SourceFilter::Codex => (logo(Icon::Codex, 0xd0d0d0), Some(0x262a31), "Codex", live_codex),
        };
        let id = match s {
            SourceFilter::All => "src-all",
            SourceFilter::Claude => "src-claude",
            SourceFilter::Codex => "src-codex",
        };
        div()
            .id(id)
            .relative()
            .flex_none()
            .size(px(44.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(12.0))
            .border_2()
            .border_color(if active { c(theme::TEXT) } else { gpui::transparent_black() })
            .cursor_pointer()
            .when(!active, |d| d.hover(|s| s.border_color(c(theme::SUBTLE))))
            .child(
                div()
                    .size(px(36.0))
                    .rounded(px(9.0))
                    .when_some(bg, |d, color| d.bg(c(color)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(content),
            )
            .when(live, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(-1.0))
                        .right(px(-1.0))
                        .size(px(12.0))
                        .rounded_full()
                        .bg(c(theme::LIVE))
                        .border_2()
                        .border_color(c(theme::BACKDROP)),
                )
            })
            .tooltip(tip(if live { format!("{name}（進行中のセッションあり）") } else { name.to_string() }, None))
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                t.set_source(s, cx);
                cx.notify();
            }))
    }

    /// アイコンのみのナビゲーション項目。名前とショートカットはツールチップに表示する
    #[allow(clippy::too_many_arguments)]
    fn nav_item(
        &self,
        id: &'static str,
        icon: Icon,
        label: &'static str,
        key: &'static str,
        active: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .group(id)
            .w(px(RAIL_W))
            .flex()
            .items_center()
            .justify_center()
            .py_1()
            .cursor_pointer()
            .child(
                div()
                    .size(px(36.0))
                    .rounded(px(9.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(active, |d| d.bg(gpui::white().opacity(0.14)))
                    .when(!active, |d| d.group_hover(id, |s| s.bg(gpui::white().opacity(0.07))))
                    .child(icons::icon(icon, if active { theme::TEXT } else { theme::MUTED }).size(px(18.0))),
            )
            .tooltip(tip(label, Some(key)))
            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                f(t, w, cx);
                cx.notify();
            }))
    }
}
