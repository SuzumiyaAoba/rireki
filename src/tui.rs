//! 端末フロントエンド。履歴の解析・索引・検索・本文取得は共通コアを使う。

mod effects;
mod inspect;
mod metrics;
mod operations;
mod state;
mod temporal;
mod terminal;
mod view;
mod worker;
mod workspace;

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};

use operations::Operations;
use state::State;
use view::View;
use worker::{Config, Worker};
use workspace::{Effect, Workspace};

enum Command {
    Help,
    Start(Config),
}

fn parse(args: &[String]) -> io::Result<Command> {
    let mut claude = None;
    let mut codex = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let target = match arg.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--claude-root" => &mut claude,
            "--codex-root" => &mut codex,
            _ => {
                return Err(invalid(format!(
                    "不明な TUI オプションです: {arg}（--tui --help で確認できます）"
                )));
            }
        };
        if target.is_some() {
            return Err(invalid(format!("{arg} は一度だけ指定してください")));
        }
        let Some(path) = args
            .next()
            .filter(|path| !path.is_empty() && !path.starts_with("--"))
        else {
            return Err(invalid(format!("{arg} のディレクトリを指定してください")));
        };
        *target = Some(PathBuf::from(path));
    }
    let roots = match (claude, codex) {
        (None, None) => None,
        (Some(claude), Some(codex)) => Some((claude, codex)),
        _ => {
            return Err(invalid(
                "--claude-root と --codex-root は両方指定してください",
            ));
        }
    };
    Ok(Command::Start(Config { roots }))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

pub(super) fn run(args: &[String]) -> io::Result<()> {
    let config = match parse(args)? {
        Command::Help => {
            println!(
                "rireki TUI — 端末でセッション履歴を閲覧\n\n\
使い方: rireki --tui [オプション] または rireki-tui [オプション]\n\n\
  --help, -h             このヘルプを表示\n\
  --claude-root <path>   Claude の projects ディレクトリ\n\
  --codex-root <path>    Codex の sessions ディレクトリ\n\n\
ログルートは両方を指定します。指定時はメモリー上で読み込み、既定の索引 DB は使いません。\n\
省略すると通常の履歴と索引を使い、3秒ごとに変更を確認します。\n\n\
F1: 履歴  F2: タイムライン  F3/F4/F5: 日/週/月\n\
F6: 統計・使用量・料金  F7: MCP  F8: 表示設定\n\
Tab/Shift-Tab: ペイン移動  ↑↓/j/k: 選択  Enter: 開く\n\
PgUp/PgDn: ページ移動  Home/End: 先頭/末尾\n\
/: 検索  Esc: 戻る・検索解除  s: 読み込み元  p: プロジェクト\n\
詳細 1〜6: 概要/履歴/ファイル差分/分析/再生/本文\n\
Ctrl-F: 履歴検索  Ctrl-R: 更新  Ctrl-L: 進行中のみ\n\
?: 操作一覧  q/Ctrl-C: 終了"
            );
            return Ok(());
        }
        Command::Start(config) => config,
    };
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(invalid(
            "TUI には対話できる端末の標準入力と標準出力が必要です。--stats はリダイレクト先でも使えます。",
        ));
    }
    let persistent = config.roots.is_none();
    let mut session = terminal::Session::enter()?;
    let worker = Worker::start(config)?;
    let operations = match Operations::start(persistent) {
        Ok(operations) => operations,
        Err(error) => {
            worker.stop();
            drop(session);
            worker.finish();
            return Err(error);
        }
    };
    let result = event_loop(&mut session, &worker, &operations, persistent);
    worker.stop();
    operations.stop();
    // DB の保存完了や背景処理の停止を待つ前に、画面とキーボードを返す。
    drop(session);
    worker.finish();
    operations.finish();
    result
}

fn event_loop(
    session: &mut terminal::Session,
    worker: &Worker,
    operations: &Operations,
    persistent: bool,
) -> io::Result<()> {
    let mut state = State::default();
    let mut view = View::default();
    let mut workspace = Workspace::new(persistent, &mut state);
    let mut redraw = true;
    loop {
        while let Some(update) = worker.try_recv() {
            state.apply(update);
            redraw = true;
        }
        redraw |= workspace.sync(&mut state);
        while let Some(update) = operations.try_recv() {
            workspace.apply(update, &mut state);
            redraw = true;
        }
        redraw |= workspace.tick();
        for request in state.take_requests() {
            // 詳細・範囲分析は統合ワーカーが取得する。
            if !matches!(request, worker::Request::Detail { .. }) {
                worker.request(request);
            }
        }
        for (kind, generation) in workspace.take_cancellations() {
            operations.cancel(kind, generation);
        }
        for request in workspace.take_requests() {
            operations.request(request);
        }
        for effect in workspace.take_effects() {
            state.status = match effect {
                Effect::Copy(text) => effects::copy(&text),
                Effect::Reveal(path) => {
                    effects::reveal(&path).map(|()| "ログの場所を開きました".into())
                }
            }
            .unwrap_or_else(|error| format!("操作できません: {error}"));
            redraw = true;
        }
        if redraw {
            session
                .terminal
                .draw(|frame| workspace.draw(frame, &mut state, &mut view))?;
            redraw = false;
        }
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if workspace.key(key, &mut state) {
                    return workspace.save(&state);
                }
                redraw = true;
            }
            Event::Paste(text) => {
                workspace.paste(&text, &mut state);
                redraw = true;
            }
            Event::Resize(_, _) | Event::FocusGained => redraw = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn options_keep_default_roots_and_require_explicit_pair() {
        assert!(matches!(
            parse(&[]).unwrap(),
            Command::Start(Config { roots: None })
        ));
        for values in [
            vec!["--unknown"],
            vec!["--claude-root"],
            vec!["--codex-root", "logs"],
            vec!["--claude-root", "--codex-root"],
            vec!["--claude-root", "a", "--claude-root", "b"],
        ] {
            assert!(parse(&args(&values)).is_err());
        }
        let Command::Start(config) = parse(&args(&[
            "--claude-root",
            "日本語 projects",
            "--codex-root",
            "codex/sessions",
        ]))
        .unwrap() else {
            panic!("expected start");
        };
        assert_eq!(
            config.roots,
            Some(("日本語 projects".into(), "codex/sessions".into()))
        );
    }
}
