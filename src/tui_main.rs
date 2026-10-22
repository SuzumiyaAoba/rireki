//! Windows でも呼び出したシェルが終了を待つ、コンソール専用エントリーポイント。

// --all-targets で明示的に選ばれても、共有 TUI の単体テストは rireki 側だけで実行する。
#![cfg(not(test))]

mod tui;

use rireki::{catalog, detail, history, index, model, search};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let Err(error) = tui::run(&args) {
        eprintln!("TUI を起動できません: {error}");
        std::process::exit(1);
    }
}
