//! ワークスペースの枠、画面切り替え、設定・MCP・ダイアログの描画。

use super::super::view::View;
use super::{Dialog, Focus, Page, State, Workspace, inspect, metrics, project_choices, temporal};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

impl Workspace {
    pub fn draw(&mut self, frame: &mut Frame<'_>, state: &mut State, view: &mut View) {
        let area = frame.area();
        if area.is_empty() {
            return;
        }
        let [nav, header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(if area.height >= 10 { 2 } else { 1 }),
            Constraint::Min(0),
            Constraint::Length(if area.height >= 10 { 2 } else { 1 }),
        ])
        .areas(area);
        let active = match self.page {
            Page::History => 1,
            Page::Temporal => match self.temporal.mode() {
                temporal::Mode::Timeline => 2,
                temporal::Mode::Day => 3,
                temporal::Mode::Week => 4,
                temporal::Mode::Month => 5,
            },
            Page::Statistics => 6,
            Page::Mcp => 7,
            Page::Settings => 8,
        };
        let labels = ["履歴", "時間軸", "日", "週", "月", "統計", "MCP", "設定"];
        let tabs: Vec<_> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                ratatui::text::Span::styled(
                    format!(" F{}{} ", i + 1, label),
                    if active == i + 1 {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::Cyan)
                    },
                )
            })
            .collect();
        frame.render_widget(Line::from(tabs), nav);
        view.header(frame, header, state);
        match self.page {
            Page::History => self.draw_history(frame, body, state, view),
            Page::Temporal => {
                self.temporal
                    .draw(frame, body, &state.snapshot, &state.catalog.order)
            }
            Page::Statistics => metrics::draw(frame, body, &mut self.metrics, &state.snapshot),
            Page::Mcp => self.draw_mcp(frame, body),
            Page::Settings => self.draw_settings(frame, body, state),
        }
        let loading = if self.inspect_loading && self.page == Page::History {
            "詳細読み込み中…  "
        } else {
            ""
        };
        let status = format!(
            "{loading}{}{}",
            if state.live_only {
                "進行中のみ  "
            } else {
                ""
            },
            state.status
        );
        let hints = if self.page == Page::History {
            " Tab:ペイン  1〜6:詳細タブ  /:検索  s/p/P:絞込  B:サイドバー  Shift-Enter:全画面  ?:操作  q:終了"
        } else {
            " F1〜F8:画面  s/p/P:絞込  ?:全操作  q:終了"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(clean(&status)),
                Line::styled(hints, Style::default().fg(Color::Cyan)),
            ])
            .scroll((u16::from(footer.height == 1), 0)),
            footer,
        );
        if self.help {
            self.draw_help(frame, area);
        }
        if let Some(dialog) = &self.dialog {
            draw_dialog(frame, area, dialog, state);
        }
    }

    fn draw_history(&mut self, frame: &mut Frame<'_>, body: Rect, state: &mut State, view: &View) {
        if self.fullscreen {
            self.draw_inspect(frame, body);
            return;
        }
        if body.width >= 100 && body.height >= 12 {
            let right = if self.sidebar {
                let [sessions, right] = Layout::horizontal([
                    Constraint::Percentage(self.sidebar_width),
                    Constraint::Min(0),
                ])
                .areas(body);
                view.sessions(frame, sessions, state);
                right
            } else {
                body
            };
            let [events, detail] = Layout::vertical([
                Constraint::Percentage(self.events_height),
                Constraint::Min(0),
            ])
            .areas(right);
            view.events(frame, events, state);
            self.draw_inspect(frame, detail);
        } else {
            match state.focus {
                Focus::Sessions if self.sidebar => view.sessions(frame, body, state),
                Focus::Sessions | Focus::Events => view.events(frame, body, state),
                Focus::Detail => self.draw_inspect(frame, body),
            }
        }
    }

    fn draw_inspect(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if let (Some(selection), Some(session)) = (self.selection, &self.selected_session) {
            self.inspector.draw(
                frame,
                area,
                inspect::Context {
                    session,
                    session_index: selection.session,
                    selection,
                    cache: self.cache.as_deref(),
                    revision: self.cache_revision,
                },
            );
        } else {
            frame.render_widget(
                Paragraph::new("セッションとイベントを選択してください")
                    .block(Block::default().borders(Borders::ALL).title("詳細")),
                area,
            );
        }
    }

    fn draw_mcp(&self, frame: &mut Frame<'_>, area: Rect) {
        let status = if self.mcp_busy {
            "切り替え中…"
        } else if self.mcp_url.is_some() {
            "起動中"
        } else {
            "停止中"
        };
        let mut lines = vec![
            format!("状態: {status}  ポート: {}", self.mcp_port),
            "Enter: 起動/停止  e: ポート編集  y: 接続設定コピー  u: URLコピー".into(),
            "ローカルホスト限定・読み取り専用。現在読み込んだ履歴を共有します。".into(),
            String::new(),
        ];
        if let Some(error) = &self.mcp_error {
            lines.push(error.clone());
        }
        if let Some(url) = &self.mcp_url {
            lines.push(url.clone());
            lines.extend(self.mcp_config.lines().map(str::to_owned));
        }
        lines.extend([
            String::new(),
            "公開ツール".into(),
            "list_sessions — セッションの検索・一覧".into(),
            "get_session — メタデータ・サブエージェント・使用量".into(),
            "get_events — 本文・ツールの入力と結果".into(),
            "search_events — イベントの検索".into(),
            "analyze_history — 使用量・実行結果の集計".into(),
        ]);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .map(|line| Line::raw(clean(&line)))
                    .collect::<Vec<_>>(),
            )
            .scroll((self.mcp_scroll, 0))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" MCP サーバー "),
            ),
            area,
        );
    }

    fn draw_settings(&self, frame: &mut Frame<'_>, area: Rect, state: &State) {
        let rows = [
            format!("サイドバー: {}", on(self.sidebar)),
            format!("詳細の全画面: {}", on(self.fullscreen)),
            format!("サイドバーの幅: {}%", self.sidebar_width),
            format!("イベント一覧の高さ: {}%", self.events_height),
            format!("進行中のセッションのみ: {}", on(state.live_only)),
            format!("読み込み元: {:?}", state.source),
            format!(
                "プロジェクト: {}",
                state.project.as_deref().unwrap_or("すべて")
            ),
            "表示設定を既定に戻す".into(),
        ];
        let mut lines = vec![
            Line::raw("↑↓:選択  Enter/左右:変更  Esc:履歴へ戻る"),
            Line::default(),
        ];
        for (i, row) in rows.iter().enumerate() {
            lines.push(Line::styled(
                format!(
                    "{} {}",
                    if i == self.settings_row { "›" } else { " " },
                    clean(row)
                ),
                if i == self.settings_row {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ));
        }
        lines.push(Line::default());
        lines.push(Line::raw(if self.persistent {
            "終了時に表示・絞り込み・詳細タブを tui.json に保存します。"
        } else {
            "明示ログルートのため、通常の設定ファイルは読み書きしません。"
        }));
        lines.push(Line::raw(
            "料金は共通の prices.json のモデル別単価で計算します。",
        ));
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((
                    (self.settings_row + 4)
                        .saturating_sub(usize::from(area.height.saturating_sub(2)))
                        .min(u16::MAX as usize) as u16,
                    0,
                ))
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" 表示設定 ")),
            area,
        );
    }

    fn draw_help(&self, frame: &mut Frame<'_>, area: Rect) {
        let popup = popup(area);
        frame.render_widget(Clear, popup);
        let mut help = HELP.to_owned();
        help.push_str("\n\n時間軸・カレンダーの追加操作\n");
        for (key, action) in temporal::HELP {
            help.push_str(&format!("{key}: {action}\n"));
        }
        frame.render_widget(
            Paragraph::new(help)
                .wrap(Wrap { trim: false })
                .scroll((self.help_scroll, 0))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" 操作一覧 · ↑↓/PgUp/PgDn:スクロール Esc:閉じる "),
                ),
            popup,
        );
    }
}

fn on(value: bool) -> &'static str {
    if value { "表示" } else { "非表示" }
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn popup(area: Rect) -> Rect {
    let width = area.width.min(110);
    let height = area.height.saturating_sub(2).max(1).min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_dialog(frame: &mut Frame<'_>, area: Rect, dialog: &Dialog, state: &State) {
    let popup = popup(area);
    frame.render_widget(Clear, popup);
    let (title, lines) = match dialog {
        Dialog::Projects { query, selected } => {
            let mut lines = vec![
                Line::raw(format!("絞り込み: {}", clean(query))),
                Line::raw("文字入力・↑↓で選択、Enterで適用、Escで閉じる"),
            ];
            let start = selected.saturating_sub(usize::from(popup.height).saturating_sub(5));
            for (i, cwd) in project_choices(state, query)
                .iter()
                .enumerate()
                .skip(start)
                .take(usize::from(popup.height).saturating_sub(4))
            {
                lines.push(Line::styled(
                    format!(
                        "{} {}",
                        if i == *selected { "›" } else { " " },
                        clean(cwd.as_deref().unwrap_or("全プロジェクト"))
                    ),
                    if i == *selected {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ));
            }
            (" プロジェクト選択 ", lines)
        }
        Dialog::Export { path, .. } => (
            " JSON を新規ファイルに保存 ",
            vec![
                Line::raw(clean(path)),
                Line::raw("Enter: 保存  Ctrl-U: 消去  Esc: 取消（既存ファイルは上書きしません）"),
            ],
        ),
        Dialog::Port(port) => (
            " MCP のポート番号 ",
            vec![
                Line::raw(clean(port)),
                Line::raw("Enter: 確定  Ctrl-U: 消去  Esc: 取消"),
            ],
        ),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        popup,
    );
}
const HELP: &str = "共通\nF1:履歴 F2:タイムライン F3:日 F4:週 F5:月 F6:統計 F7:MCP F8:設定\ns:読み込み元 p:次のプロジェクト P:プロジェクトを検索して選択 Ctrl-L:進行中のみ\nB:サイドバー表示 Shift-Enter:詳細全画面 Alt-[/Alt-]:閲覧履歴の戻る/進む\nCtrl-F:履歴検索 Ctrl-R:再読込 q/Ctrl-C:終了（検索入力中のqは文字入力） ?:この一覧\n\n履歴\nTab/Shift-Tab:ペイン移動 ↑↓/j/k:選択 Enter:次のペイン\n/:履歴検索 Esc:検索解除/戻る r:更新 Home/End/PgUp/PgDn:移動\nv:同レーンの範囲の開始/確定 a/I:選択レーンの全イベント\n\n詳細\n1:概要 2:履歴 3:ファイル 4:分析 5:再生 6:本文\n↑↓/j/k:選択・スクロール Enter:開く/展開 [ ]:レーン b:起動元 u:直前の指示\nt:種別 f:結果 o:並べ替え n:名前 /:分析検索\nSpace:再生/停止 +/-:再生速度 R:巻き戻し y:コピー Y:出力コピー e:JSON保存 c:再開コマンドコピー\nx:本文・差分展開 z:raw表示 l:行番号 d:子エージェント C:セッションID g:ログの場所\n, / .:前/次の500件 0:分析フィルタ解除 a:現在レーン全体\n\nタイムライン・カレンダー\nd/w/m:日/週/月 c:イベント/API料金 o:レーン/ディレクトリ v:範囲内のみ\na:粒度 Space:プロジェクト折畳 左右:期間移動 +/-:拡大縮小\nu:カレンダーのプロジェクト別稼働時間\nt:今日 L:最新 g:日付・範囲入力 0:全期間 Alt-1〜6:直近期間\n</>:ブロック選択 z:選択範囲に拡大 Enter:詳細 Home/End/PgUp/PgDn:縦移動\n\n統計\n[/]:7/30/90日/全期間 m/M:指標 Tab:概要/推移/時間帯/比較/モデル/プロジェクト/ツール/料金\n↑↓/j/k:項目選択 Enter:期間・プロジェクト・イベントへ移動 c:API料金\n左右/h/l:横スクロール（時間帯・料金はセル移動） Alt-左右:横スクロール\n\nMCP\nEnter:起動/停止 e:ポート番号 y:クライアント設定コピー u:URLコピー\n設定\n↑↓:設定選択 Enter/左右:変更。終了時に保存します。";
