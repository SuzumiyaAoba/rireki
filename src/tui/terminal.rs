//! raw mode と代替画面の所有権。正常終了・初期化エラー・panic で端末を戻す。

use std::io::{self, Stdout};
use std::panic::PanicHookInfo;
use std::sync::Arc;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

type PanicHook = Arc<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

pub(super) struct Session {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    _guard: Guard,
}

struct Guard {
    previous_hook: Option<PanicHook>,
}

/// 各操作は独立して試み、一つの失敗が raw mode の解除を妨げないようにする。
fn restore() {
    let mut stdout = io::stdout();
    let _ = execute!(stdout, DisableBracketedPaste);
    let _ = execute!(stdout, Show);
    let _ = execute!(stdout, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

impl Session {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        // 初期化途中に失敗しても、この時点から必ず端末を戻す。
        let mut guard = Guard {
            previous_hook: None,
        };
        let previous: PanicHook = Arc::from(std::panic::take_hook());
        let original = previous.clone();
        let terminal_thread = std::thread::current().id();
        std::panic::set_hook(Box::new(move |info| {
            // 背景タスクの panic は、そのタスク側がエラーとして回収する。
            let thread = std::thread::current();
            if thread.id() == terminal_thread {
                restore();
            } else if thread
                .name()
                .is_some_and(|name| name.starts_with("rireki-tui-"))
            {
                return;
            }
            original(info);
        }));
        guard.previous_hook = Some(previous);
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            _guard: guard,
        })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
        // set_hook は panic の巻き戻し中には呼べない。
        if !std::thread::panicking()
            && let Some(previous) = self.previous_hook.take()
        {
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}
