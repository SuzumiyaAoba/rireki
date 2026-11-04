//! GUI からの MCP サーバーの起動・停止と接続情報。

use gpui::{
    AnyElement, ClickEvent, Context, Div, Focusable, FontWeight, MouseButton, MouseDownEvent, Stateful,
    Window, div, prelude::*, px,
};

use super::widgets::{dot, tip};
use super::{Dismiss, Rireki, SearchCancel};
use crate::icons::{self, Icon};
use crate::mcp::Server;
use crate::platform::MONO;
use crate::theme::Scale as _;
use crate::theme::{self, c};

const TOOLS: [(Icon, &str, &str); 5] = [
    (Icon::ListChecks, "list_sessions", "セッションを検索・一覧取得"),
    (
        Icon::FileText,
        "get_session",
        "概要・サブエージェント・使用量を取得",
    ),
    (
        Icon::MessageSquare,
        "get_events",
        "対話本文・ツールの入力と結果を取得",
    ),
    (
        Icon::Search,
        "search_events",
        "イベントの名前・要約・モデルを検索",
    ),
    (
        Icon::Chart,
        "analyze_history",
        "期間・プロジェクト別に実行結果と使用量を集計",
    ),
];

impl Rireki {
    fn toggle_mcp(&mut self, cx: &mut Context<Self>) {
        if self.mcp_busy {
            return;
        }
        let server = self.mcp_server.take();
        let port = if server.is_none() {
            match self.mcp_port.read(cx).text().trim().parse::<u16>() {
                Ok(port) if port > 0 => port,
                _ => {
                    self.mcp_error = Some("ポート番号は 1〜65535 の整数で指定してください。".into());
                    cx.notify();
                    return;
                }
            }
        } else {
            0
        };
        self.mcp_busy = true;
        self.mcp_error = None;
        let snapshot = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                if let Some(server) = server { drop(server); Ok(None) }
                else { Server::start(snapshot, port).map(Some) }
            }).await;
            let _ = this.update(cx, |app, cx| {
                app.mcp_busy = false;
                match result {
                    Ok(server) => {
                        // 起動処理の間に読み込みが進んだ場合も、最新の版で公開する。
                        if let Some(server) = &server { server.update(app.store.clone()); }
                        app.mcp_server = server;
                    }
                    Err(error) => app.mcp_error = Some(format!("起動できませんでした: {error}。ポートが使用中の場合は別の番号を指定してください。")),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    pub(super) fn mcp_sheet(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let running = self.mcp_server.is_some();
        let close = |app: &mut Self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
            app.close_mcp(window, cx);
        };
        let panel = div()
            .id("mcp-panel")
            .occlude()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|app, _: &Dismiss, window, cx| {
                cx.stop_propagation();
                app.close_mcp(window, cx);
            }))
            .on_action(cx.listener(|app, _: &SearchCancel, window, cx| {
                cx.stop_propagation();
                app.close_mcp(window, cx);
            }))
            .w(px(660.0))
            .max_w((window.viewport_size().width - px(48.0)).max(px(0.0)))
            .max_h((window.viewport_size().height - px(48.0)).max(px(0.0)))
            .flex()
            .flex_col()
            .overflow_hidden()
            .radius_panel()
            .bg(c(theme::PANEL))
            .border_1()
            .border_color(c(theme::LINE))
            .shadow_lg()
            .text_body()
            .text_color(c(theme::TEXT))
            // 内容をスクロールしても、見出しと操作ボタンは残す。
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(c(theme::LINE))
                    .child(
                        div()
                            .id("mcp-title")
                            .flex_1()
                            .min_w_0()
                            .text_heading()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("MCP サーバー")
                            .tooltip(tip("読み込んだすべての履歴が対象です。画面の絞り込みは反映されません。", None)),
                    )
                    .child(
                        div()
                            .id("mcp-close")
                            .flex_none()
                            .size_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .radius_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(c(theme::RAISED)))
                            .tooltip(tip("閉じる", Some("esc")))
                            .child(icons::icon(Icon::X, theme::MUTED).size_4())
                            .on_click(
                                cx.listener(|app, _: &ClickEvent, window, cx| app.close_mcp(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("mcp-content")
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .px_5()
                    .py_4()
                    .child(self.mcp_connection(window, cx))
                    .child(mcp_tools())
                    .when(self.loading.is_some(), |d| {
                        d.child(div().flex_none().text_small().text_color(c(theme::MUTED)).child("履歴を読み込み中…"))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_4()
                    .px_5()
                    .py_4()
                    .border_t_1()
                    .border_color(c(theme::LINE))
                    .child(
                        div()
                            .id("mcp-toggle")
                            .flex_none()
                            .min_w(px(156.0))
                            .h(px(38.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .px_4()
                            .radius_md()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(c(if running { theme::PANEL } else { theme::SELECT }))
                            .text_color(c(if running { theme::ERR } else { theme::TEXT }))
                            .border_1()
                            .border_color(c(if running { theme::ERR } else { theme::ACCENT }))
                            .when(self.mcp_busy, |d| d.opacity(theme::alpha::DISABLED))
                            .child(if self.mcp_busy {
                                "切り替え中…"
                            } else if running {
                                "サーバーを停止"
                            } else {
                                "サーバーを起動"
                            })
                            .when(!self.mcp_busy, |d| {
                                d.cursor_pointer()
                                    .hover(|s| s.border_color(c(theme::TEXT)))
                                    .tooltip(tip("アプリを終了するまで、この画面を閉じても接続を維持します。", None))
                                    .on_click(cx.listener(|app, _: &ClickEvent, _, cx| app.toggle_mcp(cx)))
                            }),
                    ),
            );
        div()
            .id("mcp-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(theme::SCRIM))
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(close))
            .child(panel)
            .into_any_element()
    }

    fn close_mcp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popover = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn mcp_connection(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let (state, color) = if self.mcp_busy {
            ("切り替え中", theme::WARN)
        } else if self.mcp_server.is_some() {
            ("稼働中", theme::LIVE)
        } else {
            ("停止中", theme::MUTED)
        };
        let mut connection = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(dot(color, 8.0))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(state))
                    .child(div().flex_1())
                    .child(div().text_small().text_color(c(theme::MUTED)).child("Streamable HTTP")),
            );
        if let Some(server) = &self.mcp_server {
            connection = connection
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_small()
                                .text_color(c(theme::MUTED))
                                .child("エンドポイント"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .pl_3()
                                .pr_2()
                                .py_2()
                                .radius_md()
                                .bg(c(theme::BACKDROP))
                                .border_1()
                                .border_color(c(theme::LINE))
                                .child(
                                    div()
                                        .id("mcp-url")
                                        .flex_1()
                                        .min_w_0()
                                        .font_family(MONO)
                                        .text_body()
                                        .child(server.url())
                                        .tooltip(tip(
                                            "この端末からの接続だけを受け付け、履歴の読み取り以外は行いません。",
                                            None,
                                        )),
                                )
                                .child(self.mcp_copy_button(
                                    "copy-mcp-url",
                                    "URL をコピー",
                                    server.url(),
                                    cx,
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div().flex_1(),
                        )
                        .child(self.mcp_copy_button(
                            "copy-mcp-config",
                            "接続設定をコピー",
                            server.client_config(),
                            cx,
                        )),
                );
        } else if self.mcp_busy {
            connection = connection.child(
                div()
                    .h(px(84.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_small()
                    .text_color(c(theme::MUTED))
                    .child("切り替えています…"),
            );
        } else {
            let focus = self.mcp_port.read(cx).focus_handle(cx);
            connection = connection.child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child("ポート番号"),
                    )
                    .child(
                        div()
                            .id("mcp-port-field")
                            .w(px(112.0))
                            .h(px(38.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .px_3()
                            .font_family(MONO)
                            .radius_md()
                            .bg(c(theme::BACKDROP))
                            .border_1()
                            .border_color(c(if self.mcp_error.is_some() {
                                theme::ERR
                            } else if focus.is_focused(window) {
                                theme::ACCENT
                            } else {
                                theme::LINE
                            }))
                            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                                window.focus(&focus);
                                window.prevent_default();
                            })
                            .child(self.mcp_port.clone()),
                    ),
            );
        }
        if let Some(error) = &self.mcp_error {
            connection = connection.child(
                div()
                    .flex()
                    .items_start()
                    .text_small()
                    .text_color(c(theme::ERR))
                    .child(div().flex_1().min_w_0().child(error.clone())),
            );
        }
        connection
    }

    fn mcp_copy_button(
        &self,
        id: &'static str,
        label: &'static str,
        text: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let done = self.copied == Some(id);
        self.copy_button(id, text, cx)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .h_8()
            .px_3()
            .radius_md()
            .text_small()
            .text_color(c(theme::TEXT))
            .child(if done { "コピーしました" } else { label })
    }
}

fn mcp_tools() -> Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("ツール")),
        )
        .child(
            div()
                .children(TOOLS.iter().enumerate().map(|(i, &(_, name, description))| {
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_2()
                        .when(i > 0, |d| d.border_t_1().border_color(c(theme::LINE_SOFT)))
                        .child(div().flex_1().min_w_0().text_small().child(description))
                        .child(
                            div()
                                .flex_none()
                                .font_family(MONO)
                                .text_axis()
                                .text_color(c(theme::MUTED))
                                .child(name),
                        )
                })),
        )
}
