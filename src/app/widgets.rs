//! 画面の部品（ボタン・切り替え・ツールチップ・コード表示など）

mod marquee;
pub(super) use marquee::Marquee;

use gpui::{
    AnyView, App, ClickEvent, ClipboardItem, Context, Div, ElementId, FontWeight, SharedString, Stateful, Window, div,
    prelude::*, px,
};

use super::Rireki;
use crate::icons::{self, Icon};
use crate::theme::{self, c};

/// 短い説明と、あればショートカットキーを添えたツールチップ
pub(super) struct Tip {
    text: SharedString,
    key: Option<&'static str>,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(c(theme::ELEVATED))
            .border_1()
            .border_color(c(theme::BORDER))
            .shadow_md()
            .text_xs()
            .text_color(c(theme::TEXT))
            .child(self.text.clone())
            .when_some(self.key, |d, k| d.child(kbd(k)))
    }
}

/// `.tooltip(...)` に渡すツールチップの生成関数
pub(super) fn tip(
    text: impl Into<SharedString>,
    key: Option<&'static str>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip { text: text.clone(), key }).into()
}

struct PromptTip {
    prompt: SharedString,
    metadata: SharedString,
    hovered_since: std::time::Instant,
}

impl Render for PromptTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(360.0)).flex().flex_col().gap_1().px_2().py_1().rounded_md()
            .bg(c(theme::ELEVATED)).border_1().border_color(c(theme::BORDER)).shadow_md()
            .text_xs().text_color(c(theme::TEXT))
            .child(Marquee::new("prompt", self.prompt.clone()).on_show_since(self.hovered_since))
            .child(div().text_color(c(theme::MUTED)).child(self.metadata.clone()))
    }
}

/// プロンプトを切り詰めずに流し、日時などの補足は固定して表示する。
pub(super) fn prompt_tip(
    prompt: impl Into<SharedString>,
    metadata: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let prompt = prompt.into();
    let metadata = metadata.into();
    move |_, cx| cx.new(|_| PromptTip {
        prompt: prompt.clone(), metadata: metadata.clone(),
        // GPUI 0.2 のツールチップ表示までの 500ms も、開始までの 1 秒に含める。
        hovered_since: std::time::Instant::now() - std::time::Duration::from_millis(500),
    }).into()
}

/// キーの表記（⌘K など）
pub(super) fn kbd(k: &'static str) -> Div {
    div()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(c(theme::BORDER))
        .bg(c(theme::BG))
        .text_xs()
        .text_color(c(theme::MUTED))
        .child(k)
}

/// 丸い印（進行中の印や凡例など）
pub(super) fn dot(color: u32, size: f32) -> Div {
    div().flex_none().size(px(size)).rounded_full().bg(c(color))
}

/// イベントの種類のアイコンを、その種類の色で出す
pub(super) fn event_icon(i: crate::icons::Icon, color: u32, size: f32) -> gpui::Svg {
    crate::icons::icon(i, color).size(px(size))
}

/// プロジェクトの目印。ディレクトリ名の頭文字を色付きの角丸四角に入れる
pub(super) fn project_tile(cwd: &str, size: f32) -> Div {
    let initial: String =
        super::base_name(cwd).chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(size * 0.28))
        .bg(c(theme::project_color(cwd)))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.62))
        .font_weight(FontWeight::BOLD)
        .text_color(gpui::white())
        .child(initial)
}

impl Rireki {
    /// 枠付きのボタン。`label` が空ならアイコンだけを表示する
    pub(super) fn icon_button(
        &self,
        id: impl Into<ElementId>,
        icon: Option<Icon>,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let label: SharedString = label.into();
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(c(if active { theme::ACCENT } else { theme::BORDER }))
            .bg(c(if active { theme::ACCENT_BG } else { theme::PANEL }))
            .text_color(c(theme::TEXT))
            .text_xs()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .when_some(icon, |d, i| d.child(icons::icon(i, if active { theme::ACCENT } else { theme::TEXT })))
            .when(!label.is_empty(), |d| d.child(label))
            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                f(this, w, cx);
                cx.notify();
            }))
    }

    /// 枠のないアイコンだけのボタン。ツールチップに説明とショートカットを出す
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ghost_button(
        &self,
        id: impl Into<ElementId>,
        icon: Icon,
        tip_text: &'static str,
        key: Option<&'static str>,
        enabled: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.0))
            .rounded_md()
            .child(icons::icon(icon, if enabled { theme::MUTED } else { theme::SUBTLE }).size_4())
            .tooltip(tip(tip_text, key))
            .when(enabled, |d| {
                d.cursor_pointer().hover(|s| s.bg(c(theme::HOVER))).on_click(cx.listener(
                    move |this, _: &ClickEvent, w, cx| {
                        f(this, w, cx);
                        cx.notify();
                    },
                ))
            })
    }

    /// 区切られた選択肢。`current` に一致するものを選択中として表示する
    pub(super) fn segmented<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        items: &[(T, &'static str)],
        current: Option<T>,
        cx: &mut Context<Self>,
        f: fn(&mut Self, T, &mut Window, &mut Context<Self>),
    ) -> Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .p(px(2.0))
            .gap(px(2.0))
            .rounded_md()
            .bg(c(theme::BACKDROP))
            .border_1()
            .border_color(c(theme::BORDER))
            .children(items.iter().enumerate().map(|(i, &(v, label))| {
                let on = current == Some(v);
                div()
                    .id(ElementId::NamedInteger(id.into(), i as u64))
                    .px_2()
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .text_color(c(if on { theme::TEXT } else { theme::MUTED }))
                    .when(on, |d| d.bg(c(theme::HOVER)).font_weight(FontWeight::SEMIBOLD))
                    .hover(|d| d.text_color(c(theme::TEXT)))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                        f(this, v, w, cx);
                        cx.notify();
                    }))
            }))
    }

    /// クリップボードにコピーするボタン。押した後しばらくチェックマークを出す
    pub(super) fn copy_button(&self, id: &'static str, copy: String, cx: &mut Context<Self>) -> Stateful<Div> {
        let done = self.copied == Some(id);
        div()
            .id(id)
            .p_1()
            .rounded_sm()
            .bg(c(theme::PANEL))
            .border_1()
            .border_color(c(theme::BORDER))
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(
                if done { Icon::Check } else { Icon::Copy },
                if done { theme::OK } else { theme::MUTED },
            ))
            .tooltip(tip(if done { "コピーしました" } else { "コピー" }, None))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                this.flash_copied(id, cx);
            }))
    }

    /// コピーしたことをしばらく表示する
    pub(super) fn flash_copied(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.copied = Some(id);
        self.detail_changed(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1200)).await;
            let _ = this.update(cx, |this, cx| {
                if this.copied == Some(id) {
                    this.copied = None;
                    this.detail_changed(cx);
                }
            });
        })
        .detach();
    }
}
