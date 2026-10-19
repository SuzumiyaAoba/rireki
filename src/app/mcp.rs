//! GUI からの MCP サーバーの起動・停止と接続情報。

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, MouseButton, MouseDownEvent, Window, div,
    prelude::*, px,
};

use super::Rireki;
use crate::icons::{self, Icon};
use crate::mcp::Server;
use crate::theme::{self, c};

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
                    self.mcp_error =
                        Some("ポート番号は 1〜65535 の整数で指定してください。".into());
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

    pub(super) fn mcp_sheet(&self, cx: &mut Context<Self>) -> AnyElement {
        let running = self.mcp_server.is_some();
        let state = if self.mcp_busy {
            "切り替え中…"
        } else if running {
            "稼働中"
        } else {
            "停止中"
        };
        let close =
            |app: &mut Self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
                app.popover = None;
                window.focus(&app.focus_handle);
                cx.notify();
            };
        let mut panel = div().id("mcp-panel").occlude().w(px(600.0)).p_5().flex().flex_col().gap_4()
            .rounded_xl().bg(c(theme::ELEVATED)).border_1().border_color(c(theme::BORDER)).shadow_lg()
            .text_sm().text_color(c(theme::TEXT))
            .child(div().flex().items_center().gap_2()
                .child(icons::icon(Icon::Plug, theme::ACCENT).size_4())
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("MCP サーバー"))
                .child(div().text_xs().text_color(c(if running { theme::LIVE } else { theme::MUTED })).child(state))
                .child(div().id("mcp-close").p_1().rounded_md().cursor_pointer().hover(|s| s.bg(c(theme::HOVER)))
                    .child(icons::icon(Icon::X, theme::MUTED))
                    .on_click(cx.listener(|app, _: &ClickEvent, window, cx| {
                        app.popover = None;
                        window.focus(&app.focus_handle);
                        cx.notify();
                    }))))
            .child(div().text_color(c(theme::MUTED)).child("Coding Agent から Claude Code / Codex の対話履歴を検索・取得・解析できます。"))
            .child(div().text_xs().text_color(c(theme::MUTED)).child("このアプリが読み込んだすべての履歴を公開します。画面の絞り込みには連動しません。localhost のみで待ち受け、アプリ終了時に停止します。"));
        if let Some(server) = &self.mcp_server {
            panel = panel
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(c(theme::BG))
                        .child(div().flex_1().child(server.url()))
                        .child(self.copy_button("copy-mcp-url", server.url(), cx)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(c(theme::MUTED))
                                .child("接続方式: Streamable HTTP · 履歴は読み取り専用"),
                        )
                        .child(div().text_xs().child("接続設定をコピー"))
                        .child(self.copy_button("copy-mcp-config", server.client_config(), cx)),
                );
        } else if !self.mcp_busy {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child("ポート番号")
                    .child(div().w(px(130.0)).child(self.mcp_port.clone())),
            );
        }
        if self.loading.is_some() {
            panel =
                panel.child(div().text_xs().text_color(c(theme::MUTED)).child(
                    "履歴を読み込み中です。読み込みが進むと MCP の取得結果にも反映されます。",
                ));
        }
        if let Some(error) = &self.mcp_error {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(c(theme::ERR))
                    .child(error.clone()),
            );
        }
        for (name, description) in [
            ("list_sessions", "セッションを検索・一覧取得"),
            ("get_session", "概要・サブエージェント・使用量を取得"),
            ("get_events", "対話本文・ツールの入力と結果を取得"),
            ("search_events", "イベントの名前・要約・モデルを検索"),
            (
                "analyze_history",
                "期間・プロジェクト別に実行結果と使用量を集計",
            ),
        ] {
            panel = panel.child(
                div()
                    .flex()
                    .gap_3()
                    .text_xs()
                    .child(div().w(px(145.0)).text_color(c(theme::ACCENT)).child(name))
                    .child(div().text_color(c(theme::MUTED)).child(description)),
            );
        }
        panel = panel.child(
            div().flex().justify_end().child(
                div()
                    .id("mcp-toggle")
                    .px_4()
                    .py_2()
                    .rounded_md()
                    .bg(c(theme::ACCENT))
                    .text_color(c(theme::BG))
                    .when(!self.mcp_busy, |d| {
                        d.cursor_pointer().hover(|s| s.opacity(0.85))
                    })
                    .child(if self.mcp_busy {
                        "切り替え中…"
                    } else if running {
                        "サーバーを停止"
                    } else {
                        "サーバーを起動"
                    })
                    .on_click(cx.listener(|app, _: &ClickEvent, _, cx| app.toggle_mcp(cx))),
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
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(close))
            .child(panel)
            .into_any_element()
    }
}
