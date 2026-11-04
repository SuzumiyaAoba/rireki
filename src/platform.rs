//! OS ごとのキー表記とフォント。

pub const fn shortcut(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") { mac } else { other }
}

pub const MONO: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

/// GUI のダブルクリック起動ではコンソールを開かず、引数を付けた実行（計測の出力など）では親のコンソールを使う。
#[cfg(target_os = "windows")]
pub fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // 親にコンソールがない場合や、標準出力が既にリダイレクトされている場合は失敗しても構わない。
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS); }
}
