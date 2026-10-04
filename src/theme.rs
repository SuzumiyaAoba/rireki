use gpui::{Hsla, rgb};

use crate::model::{Event, Kind};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub const BG: u32 = 0x16181d;
pub const PANEL: u32 = 0x1d2027;
pub const BORDER: u32 = 0x2c313b;
pub const GRID: u32 = 0x252932;
pub const TEXT: u32 = 0xd7dae0;
pub const MUTED: u32 = 0x8b93a1;
pub const ACCENT: u32 = 0x5b9cf5;
pub const NOW: u32 = 0xe5534b;

pub fn event_color(e: &Event) -> u32 {
    match e.kind {
        Kind::User => 0xe0b341,
        Kind::Assistant => 0x4fb3a5,
        Kind::Thinking => 0x6b6f7a,
        Kind::Agent => 0xa77bf3,
        Kind::Tool => tool_color(&e.name),
    }
}

pub fn tool_color(name: &str) -> u32 {
    match name {
        "Read" | "Grep" | "Glob" | "LS" | "read_file" | "list_dir" | "view_image" | "grep_files" => 0x4a8fe7,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch" => 0x55b45f,
        "Bash" | "BashOutput" | "exec" | "exec_command" | "shell" | "local_shell_call" | "write_stdin" | "wait" | "Monitor" => {
            0xe08a3c
        }
        "WebSearch" | "WebFetch" | "web_search_call" | "web_search" => 0x3cc3e0,
        "TodoWrite" | "update_plan" | "update_goal" | "TaskCreate" | "TaskUpdate" | "TaskList" | "ExitPlanMode" => 0xd96ba0,
        n if n.starts_with("mcp__") => 0x8a94a6,
        _ => 0x7f8ea3,
    }
}

/// 凡例
pub const LEGEND: &[(&str, u32)] = &[
    ("User", 0xe0b341),
    ("Assistant", 0x4fb3a5),
    ("読み取り", 0x4a8fe7),
    ("編集", 0x55b45f),
    ("シェル", 0xe08a3c),
    ("Web", 0x3cc3e0),
    ("計画", 0xd96ba0),
    ("サブエージェント", 0xa77bf3),
    ("その他", 0x7f8ea3),
];
