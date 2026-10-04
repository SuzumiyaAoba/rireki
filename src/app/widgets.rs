//! 画面の部品（ボタン・切り替え・ツールチップ・コード表示など）

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

    /// 右上にコピーボタンを重ねた code_box。`copy` がクリップボードに入る（表示用に切り詰める前の本文）
    pub(super) fn copyable(
        &self,
        id: &'static str,
        text: String,
        copy: String,
        mono: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .relative()
            .min_w_0()
            .child(code_box(text, mono))
            .child(self.copy_button(id, copy, cx).absolute().top_1().right_1())
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

pub(super) fn code_box(text: String, mono: bool) -> Div {
    let d = div()
        .min_w_0()
        .overflow_hidden()
        .p_1p5()
        .rounded_sm()
        .bg(c(0x15171c))
        .text_xs()
        .text_color(c(0xc5c9d1))
        .when(mono, |d| d.font_family("Menlo"));
    if !text.starts_with("*** Begin Patch") {
        if !text.lines().take(3).any(|l| yaml_key(l).is_some()) {
            return d.child(text);
        }
        // YAML 風に整形した引数・結果はキーを色分けする
        return d.flex().flex_col().children(text.lines().map(|l| {
            match yaml_key(l) {
                Some(n) => div()
                    .min_w_0()
                    .flex()
                    .child(div().flex_none().text_color(c(0x8ab4f8)).child(l[..n].to_string()))
                    .child(div().flex_1().min_w_0().child(l[n..].to_string())),
                None => div().min_w_0().child(if l.is_empty() { " ".to_string() } else { l.to_string() }),
            }
        }));
    }
    // apply_patch は差分として行ごとに色分けする
    d.flex().flex_col().children(text.lines().map(|l| {
        let color = match l.as_bytes().first() {
            _ if l.starts_with("*** ") => 0xe0b341,
            Some(b'+') => 0x7fd18b,
            Some(b'-') => 0xf07c7c,
            Some(b'@') => 0x6fa8dc,
            _ => 0xc5c9d1,
        };
        div().min_w_0().text_color(c(color)).child(if l.is_empty() { " ".to_string() } else { l.to_string() })
    }))
}

/// `  - key: value` 形式の行なら、キー（コロンまで）の終端位置を返す
fn yaml_key(l: &str) -> Option<usize> {
    let body = l.trim_start_matches(' ');
    let body = body.strip_prefix("- ").unwrap_or(body);
    let start = l.len() - body.len();
    let n = body.bytes().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')).count();
    if n == 0 || !body.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    match &body.as_bytes()[n..] {
        [b':'] | [b':', b' ', ..] => Some(start + n + 1),
        _ => None,
    }
}
