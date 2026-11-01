use gpui::{Hsla, rgb};

use crate::icons::Icon;
use crate::model::{Event, Kind};

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

// 色の名前と値は design/src/tokens.ts に揃える。テストが一致を検査する。
// グレーは OKLCH の色相 265・彩度 0.014 以下で、明度だけを動かしている。

/// ウィンドウの地（上端の帯と左のアイコンレール）。内容パネルより暗くして額縁のように見せる
pub const BACKDROP: u32 = 0x0d0f13;
pub const BG: u32 = 0x16181d;
pub const PANEL: u32 = 0x1f2227;
/// ポップオーバーやホバーカードなど、浮いている面
pub const RAISED: u32 = 0x282b31;
pub const HOVER: u32 = 0x32353c;
pub const LINE: u32 = 0x3a3d44;
pub const LINE_SOFT: u32 = 0x282b31;
pub const TEXT: u32 = 0xd8dbe1;
pub const MUTED: u32 = 0xa3a8b1;
/// MUTED よりさらに控えめな文字（件数や補足）。PANEL 上で 4.5:1 以上。RAISED・HOVER の上では使わない
pub const SUBTLE: u32 = 0x8b8f98;
pub const ACCENT: u32 = 0x5b9cf5;
/// 選択中のボタンの背景
pub const ACCENT_BG: u32 = 0x22334f;
/// サイドバーで選択中の行と、範囲の端
pub const SELECT: u32 = 0x2a5aa8;
/// 進行中（ログが書き込まれ続けている）セッションの印
pub const LIVE: u32 = 0x3fb950;
/// 現在時刻の線。赤は失敗だけに使うので、文字と同じ色にする
pub const NOW: u32 = 0xd8dbe1;
/// 失敗したツール呼び出し
pub const ERR: u32 = 0xf07c7c;
/// 拒否・中断したツール呼び出し
pub const WARN: u32 = 0xe0a24a;
/// 差分の追加と削除。赤緑の慣習に従う例外
pub const DIFF_ADD: u32 = 0x7fd18b;
#[allow(dead_code)] // 画面の作り直しで使い始めたら外す
pub const DIFF_DEL: u32 = 0xf07c7c;
/// ロゴの地色。ブランドの色なので他には使わない
pub const LOGO_CLAUDE: u32 = 0xd97757;
pub const LOGO_CODEX: u32 = 0xd0d0d0;
/// 画面の作り直しが終わるまでの移行用。置き換えたら削除する
pub const OK: u32 = DIFF_ADD;

/// 角丸は 4 段。正円は点だけに使う
pub mod radius {
    pub const SM: f32 = 3.0;
    pub const MD: f32 = 6.0;
    pub const PANEL: f32 = 10.0;
    pub const INNER: f32 = 9.0;
}

/// 文字サイズは 5 段。AXIS は軸・時刻・件数・キー表記などの補助の文字
pub mod size {
    pub const AXIS: f32 = 11.0;
    pub const SMALL: f32 = 12.0;
    pub const BODY: f32 = 13.0;
    pub const HEADING: f32 = 16.0;
    pub const VALUE: f32 = 20.0;
}

/// 透明度は意味ごとに 1 つ。これ以外の透明度で色を作らない
#[allow(dead_code)] // 画面の作り直しで使い始めたら外す
pub mod alpha {
    pub const DISABLED: f32 = 0.4;
    pub const STALE: f32 = 0.55;
    pub const RANGE: f32 = 0.2;
    pub const DIFF: f32 = 0.12;
}

/// 文字サイズと角丸を、段階の名前で指定する。gpui の text_xs / rounded_md など任意の値は使わない
pub trait Scale: gpui::Styled + Sized {
    fn text_axis(self) -> Self {
        self.text_size(gpui::px(size::AXIS))
    }
    fn text_small(self) -> Self {
        self.text_size(gpui::px(size::SMALL))
    }
    fn text_body(self) -> Self {
        self.text_size(gpui::px(size::BODY))
    }
    fn text_heading(self) -> Self {
        self.text_size(gpui::px(size::HEADING))
    }
    fn text_value(self) -> Self {
        self.text_size(gpui::px(size::VALUE))
    }
    fn radius_sm(self) -> Self {
        self.rounded(gpui::px(radius::SM))
    }
    fn radius_md(self) -> Self {
        self.rounded(gpui::px(radius::MD))
    }
    fn radius_inner(self) -> Self {
        self.rounded(gpui::px(radius::INNER))
    }
    fn radius_panel(self) -> Self {
        self.rounded(gpui::px(radius::PANEL))
    }
}

impl<T: gpui::Styled> Scale for T {}

/// イベントの色。判別できる色相は 5 つまで（ユーザー・参照・編集・シェル・エージェント）。
/// 応答・思考・その他は無彩色の濃淡。Claude の橙・Codex の青緑・失敗の赤は使わない
pub const USER: u32 = 0xaa8d00;
pub const ASSISTANT: u32 = 0xb3b7c1;
pub const THINKING: u32 = 0x565a62;
pub const REFERENCE: u32 = 0x00719e;
pub const EDIT: u32 = 0x477200;
pub const SHELL: u32 = 0x967cdb;
pub const AGENT: u32 = 0x953d7c;
pub const OTHER: u32 = 0x82868f;

pub struct Category {
    /// design/src/tokens.ts の eventGroups のキー
    pub key: &'static str,
    pub label: &'static str,
    pub color: u32,
}

pub const CATEGORIES: [Category; 8] = [
    Category { key: "user", label: "ユーザー", color: USER },
    Category { key: "assistant", label: "応答", color: ASSISTANT },
    Category { key: "thinking", label: "思考", color: THINKING },
    Category { key: "reference", label: "参照", color: REFERENCE },
    Category { key: "edit", label: "編集", color: EDIT },
    Category { key: "shell", label: "シェル", color: SHELL },
    Category { key: "agent", label: "エージェント", color: AGENT },
    Category { key: "other", label: "その他", color: OTHER },
];

pub fn event_color(e: &Event) -> u32 {
    match e.kind {
        Kind::User => USER,
        Kind::Assistant => ASSISTANT,
        Kind::Thinking => THINKING,
        Kind::Agent => AGENT,
        Kind::Tool => tool_color(&e.name),
    }
}

pub fn tool_color(name: &str) -> u32 {
    match name {
        "Read" | "Grep" | "Glob" | "LS" | "read_file" | "list_dir" | "view_image" | "grep_files" | "WebSearch"
        | "WebFetch" | "web_search_call" | "web_search" => REFERENCE,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "apply_patch" => EDIT,
        "Bash" | "BashOutput" | "exec" | "exec_command" | "shell" | "local_shell_call" | "write_stdin" | "wait"
        | "Monitor" => SHELL,
        "TodoWrite" | "update_plan" | "update_goal" | "TaskCreate" | "TaskUpdate" | "TaskList" | "ExitPlanMode" => {
            AGENT
        }
        _ => OTHER,
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

/// 統計のグラフで読み込み元を表す色。暗い面（PANEL）での明度・彩度・色覚の区別・コントラストを検査済み
pub const SERIES_CLAUDE: u32 = 0xd95926;
pub const SERIES_CODEX: u32 = 0x199e70;
/// 量を表す青の段階（少 → 多）。読み込み元の色と紛れないよう、量は青だけで表す
pub const HEAT: [u32; 5] = [0x184f95, 0x256abf, 0x3987e5, 0x6da7ec, 0x9ec5f4];

/// プロジェクト（作業ディレクトリ）の目印の色。名前を必ず並べるので、同じ色になっても構わない
const PROJECT: [u32; 8] = [0x5d7fb0, 0x5e9a82, 0xb08f55, 0xa66f97, 0xb8714f, 0x4f97a6, 0x949a52, 0xb35f70];

pub fn project_color(cwd: &str) -> u32 {
    PROJECT[(crate::jsmock::hash(cwd) as u64 % PROJECT.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 色と寸法の正は design/src/tokens.ts。手作業の同期漏れをここで検出する
    const TOKENS: &str = include_str!("../design/src/tokens.ts");

    fn section(name: &str) -> &'static str {
        let start = TOKENS.find(&format!("export const {name} ")).unwrap_or_else(|| panic!("{name} がない"));
        let rest = &TOKENS[start..];
        &rest[..rest.find(" as const;").unwrap_or_else(|| panic!("{name} の終わりがない"))]
    }

    /// `key: ...'#rrggbb'` の最初の色を読む
    fn hex(section: &str, key: &str) -> u32 {
        let line = section
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{key}:")))
            .unwrap_or_else(|| panic!("{key} がない"));
        let at = line.find("'#").unwrap_or_else(|| panic!("{key} に色がない")) + 2;
        u32::from_str_radix(&line[at..at + 6], 16).unwrap()
    }

    fn hexes(section: &str) -> Vec<u32> {
        section.split("'#").skip(1).map(|s| u32::from_str_radix(&s[..6], 16).unwrap()).collect()
    }

    fn number(section: &str, key: &str) -> f32 {
        let at = section.find(&format!("{key}:")).unwrap_or_else(|| panic!("{key} がない")) + key.len() + 1;
        let tail = section[at..].trim_start();
        let end = tail.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap();
        tail[..end].parse().unwrap()
    }

    #[test]
    fn colors_match_tokens() {
        let colors = section("colors");
        let expected = [
            ("backdrop", BACKDROP),
            ("bg", BG),
            ("panel", PANEL),
            ("raised", RAISED),
            ("hover", HOVER),
            ("line", LINE),
            ("lineSoft", LINE_SOFT),
            ("text", TEXT),
            ("muted", MUTED),
            ("subtle", SUBTLE),
            ("accent", ACCENT),
            ("accentBg", ACCENT_BG),
            ("select", SELECT),
            ("live", LIVE),
            ("now", NOW),
            ("err", ERR),
            ("warn", WARN),
            ("diffAdd", DIFF_ADD),
            ("diffDel", DIFF_DEL),
            ("claude", SERIES_CLAUDE),
            ("codex", SERIES_CODEX),
        ];
        for (key, value) in expected {
            assert_eq!(hex(colors, key), value, "colors.{key}");
        }
    }

    #[test]
    fn event_groups_match_tokens() {
        let groups = section("eventGroups");
        for category in &CATEGORIES {
            assert_eq!(hex(groups, category.key), category.color, "eventGroups.{}", category.key);
        }
    }

    #[test]
    fn palettes_match_tokens() {
        assert_eq!(hexes(section("heat")), HEAT.to_vec());
        assert_eq!(hexes(section("projectColors")), PROJECT.to_vec());
    }

    #[test]
    fn scales_match_tokens() {
        let radius = section("radius");
        assert_eq!(number(radius, "sm"), radius::SM);
        assert_eq!(number(radius, "md"), radius::MD);
        assert_eq!(number(radius, "panel"), radius::PANEL);
        assert_eq!(number(radius, "inner"), radius::INNER);
        let font = section("fontSize");
        assert_eq!(number(font, "axis"), size::AXIS);
        assert_eq!(number(font, "small"), size::SMALL);
        assert_eq!(number(font, "body"), size::BODY);
        assert_eq!(number(font, "heading"), size::HEADING);
        assert_eq!(number(font, "value"), size::VALUE);
        let alpha = section("alpha");
        assert_eq!(number(alpha, "disabled"), alpha::DISABLED);
        assert_eq!(number(alpha, "stale"), alpha::STALE);
        assert_eq!(number(alpha, "range"), alpha::RANGE);
        assert_eq!(number(alpha, "diff"), alpha::DIFF);
    }
}
