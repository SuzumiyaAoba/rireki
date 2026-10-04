use gpui::{Hsla, rgb};

use crate::icons::Icon;
use crate::model::{Event, Kind};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// ウィンドウの地（上端の帯と左のアイコンレール）。内容パネルより暗くして額縁のように見せる
pub const BACKDROP: u32 = 0x0e1014;
pub const SIDEBAR: u32 = 0x15171c;
pub const BG: u32 = 0x16181d;
pub const PANEL: u32 = 0x1d2027;
/// ポップオーバーやホバーカードなど、浮いている面
pub const ELEVATED: u32 = 0x252933;
pub const BORDER: u32 = 0x2c313b;
pub const GRID: u32 = 0x252932;
pub const TEXT: u32 = 0xd7dae0;
pub const MUTED: u32 = 0x8b93a1;
/// MUTED よりさらに控えめな文字（件数や補足）
pub const SUBTLE: u32 = 0x646c7a;
pub const ACCENT: u32 = 0x5b9cf5;
/// 選択中のボタンの背景
pub const ACCENT_BG: u32 = 0x24324a;
/// サイドバーで選択中の行の背景
pub const SELECT: u32 = 0x2a5aa8;
pub const HOVER: u32 = 0x262a33;
/// 進行中（ログが書き込まれ続けている）セッションの印
pub const LIVE: u32 = 0x3fb950;
pub const NOW: u32 = 0xe5534b;
/// サブエージェント
pub const SUB: u32 = 0xb9a3f0;
pub const OK: u32 = 0x7fd18b;
/// 失敗したツール呼び出し
pub const ERR: u32 = 0xf07c7c;
/// 拒否・中断したツール呼び出し
pub const WARN: u32 = 0xe0a24a;

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

/// イベントのアイコン。色（カテゴリ）より細かく、ツールごとに分ける
pub fn event_icon(e: &Event) -> Icon {
    match e.kind {
        Kind::User => Icon::User,
        Kind::Assistant => Icon::MessageSquare,
        Kind::Thinking => Icon::Brain,
        Kind::Agent => Icon::Bot,
        Kind::Tool => tool_icon(&e.name),
    }
}

pub fn tool_icon(name: &str) -> Icon {
    match name {
        "Read" | "read_file" => Icon::FileText,
        "Grep" | "grep_files" => Icon::TextSearch,
        "Glob" | "LS" | "list_dir" => Icon::FolderSearch,
        "view_image" => Icon::Image,
        "Edit" | "MultiEdit" | "apply_patch" => Icon::FilePen,
        "Write" => Icon::FilePlus,
        "NotebookEdit" => Icon::NotebookPen,
        "Bash" | "BashOutput" | "exec" | "exec_command" | "shell" | "local_shell_call" => Icon::Terminal,
        "write_stdin" => Icon::Keyboard,
        "wait" => Icon::Hourglass,
        "Monitor" => Icon::Activity,
        "WebSearch" | "web_search_call" | "web_search" => Icon::Globe,
        "WebFetch" => Icon::CloudDownload,
        "TodoWrite" | "update_plan" | "TaskCreate" | "TaskUpdate" | "TaskList" => Icon::ListChecks,
        "ExitPlanMode" => Icon::Map,
        "update_goal" => Icon::Target,
        "Skill" => Icon::Sparkles,
        n if n.starts_with("mcp__") => Icon::Plug,
        _ => Icon::Wrench,
    }
}

/// 凡例
pub const LEGEND: &[(&str, u32, Icon)] = &[
    ("User", 0xe0b341, Icon::User),
    ("Assistant", 0x4fb3a5, Icon::MessageSquare),
    ("読み取り", 0x4a8fe7, Icon::FileText),
    ("編集", 0x55b45f, Icon::FilePen),
    ("シェル", 0xe08a3c, Icon::Terminal),
    ("Web", 0x3cc3e0, Icon::Globe),
    ("計画", 0xd96ba0, Icon::ListChecks),
    ("サブエージェント", 0xa77bf3, Icon::Bot),
    ("その他", 0x7f8ea3, Icon::Wrench),
];

/// プロジェクト（作業ディレクトリ）の目印の色。イベントの色と紛れないよう彩度を落としてある
const PROJECT: [u32; 10] =
    [0x5d7fb0, 0x5e9a82, 0xb08f55, 0xa66f97, 0x8270c9, 0xb8714f, 0x4f97a6, 0x949a52, 0xb35f70, 0x737d8f];

pub fn project_color(cwd: &str) -> u32 {
    PROJECT[(crate::jsmock::hash(cwd) as u64 % PROJECT.len() as u64) as usize]
}
