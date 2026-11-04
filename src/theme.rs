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
pub const DIFF_DEL: u32 = 0xf07c7c;
/// ロゴの地色。ブランドの色なので他には使わない
pub const LOGO_CLAUDE: u32 = 0xd97757;
pub const LOGO_CODEX: u32 = 0xd0d0d0;

/// モーダルの背後に敷く幕の透明度。黒に重ねる
pub const SCRIM: f32 = 0.45;

/// 塗りの上に載せる文字・アイコンの色。BACKDROP と TEXT のうち、対比の高い方を選ぶ
pub fn ink_on(fill: u32) -> u32 {
    fn luminance(hex: u32) -> f32 {
        let channel = |shift: u32| {
            let v = ((hex >> shift) & 0xff) as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    let contrast = |ink: u32| {
        let (a, b) = (luminance(fill), luminance(ink));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    };
    if contrast(BACKDROP) >= contrast(TEXT) { BACKDROP } else { TEXT }
}

/// 角丸は 4 段。正円は点と目印（今日の日付・履歴のノード・ドーナツの穴）だけに使う。棒・帯・チップ・トグルの地には使わない
pub mod radius {
    pub const SM: f32 = 3.0;
    pub const MD: f32 = 6.0;
    pub const PANEL: f32 = 10.0;
    pub const INNER: f32 = 9.0;
}

/// 画面の骨格の寸法。正は design/src/tokens.ts の dimensions
pub mod dim {
    pub const STRIP: f32 = 34.0;
    pub const RAIL: f32 = 68.0;
    pub const GAP: f32 = 8.0;
    pub const HEADER: f32 = 46.0;
    pub const FOOTER: f32 = 26.0;
    pub const MINIMAP: f32 = 30.0;
    pub const AXIS: f32 = 40.0;
    pub const ROW: f32 = 32.0;
    pub const CONTROL: f32 = 28.0;
    pub const SIDEBAR: f32 = 260.0;
    pub const LABEL: f32 = 260.0;
    pub const DETAIL: f32 = 440.0;
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
    fn ink_stays_readable_on_every_category_color() {
        let contrast = |a: u32, b: u32| {
            let l = |hex: u32| {
                let ch = |s: u32| {
                    let v = ((hex >> s) & 0xff) as f32 / 255.0;
                    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
                };
                0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
            };
            let (x, y) = (l(a), l(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        for category in &CATEGORIES {
            let ink = ink_on(category.color);
            assert!(contrast(ink, category.color) >= 3.0, "{} の上の文字", category.key);
        }
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
        // 正円は 9999px（tokens.ts と DESIGN.md の値）。点と目印にだけ使う
        assert_eq!(number(radius, "full"), 9999.0);
    }

    #[test]
    fn dimensions_match_tokens() {
        let dimensions = section("dimensions");
        let expected = [
            ("strip", dim::STRIP),
            ("rail", dim::RAIL),
            ("gap", dim::GAP),
            ("header", dim::HEADER),
            ("footer", dim::FOOTER),
            ("minimap", dim::MINIMAP),
            ("axis", dim::AXIS),
            ("row", dim::ROW),
            ("control", dim::CONTROL),
            ("sidebar", dim::SIDEBAR),
            ("label", dim::LABEL),
            ("detail", dim::DETAIL),
        ];
        for (key, value) in expected {
            assert_eq!(number(dimensions, key), value, "dimensions.{key}");
        }
        // 余白と同じく、骨格の隙間も 4px の倍数
        assert_eq!(dim::GAP % 4.0, 0.0);
    }

    #[test]
    fn scrim_matches_tokens() {
        let overlay = section("overlay");
        let at = overlay.find("scrim: '#").expect("scrim がない") + "scrim: '#".len();
        let alpha = u32::from_str_radix(&overlay[at + 6..at + 8], 16).unwrap();
        assert_eq!((SCRIM * 255.0).round() as u32, alpha, "overlay.scrim の透明度");
    }

    /// ツール名ごとのアイコンは tokens.ts の toolIcons と同じ。lucide の名前（FolderSearch）は SVG のファイル名（folder-search）に対応する
    #[test]
    fn tool_icons_match_tokens() {
        let kebab = |name: &str| {
            let mut out = String::new();
            for (i, ch) in name.chars().enumerate() {
                if ch.is_uppercase() {
                    if i > 0 {
                        out.push('-');
                    }
                    out.extend(ch.to_lowercase());
                } else {
                    out.push(ch);
                }
            }
            out
        };
        let mut count = 0;
        for line in section("toolIcons").lines() {
            let Some((tool, icon)) = line.trim().trim_end_matches(',').split_once(": '") else { continue };
            let expected = format!("icons/{}.svg", kebab(icon.trim_end_matches('\'')));
            assert_eq!(tool_icon(tool).path(), expected, "toolIcons.{tool}");
            count += 1;
        }
        assert!(count >= 30, "toolIcons を読めていない: {count}");
        // 名前のないツールの既定は工具、mcp__ で始まるものは差し込み口
        assert_eq!(tool_icon("mcp__x__y").path(), "icons/plug.svg");
        assert_eq!(tool_icon("unknown_tool").path(), "icons/wrench.svg");
    }

    /// DESIGN.md の frontmatter から、最上位キー `name:` の直下の `key: value` を読む（コメントと入れ子は除く）
    fn design(name: &str) -> Vec<(String, String)> {
        let front = include_str!("../DESIGN.md").split("---").nth(1).expect("DESIGN.md に frontmatter がない");
        let mut out = Vec::new();
        let mut inside = false;
        for line in front.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if !line.starts_with(' ') {
                inside = line.trim_end() == format!("{name}:");
            } else if inside && !line.starts_with("    ") {
                let (key, value) = line.trim().split_once(':').expect("key: value の形");
                out.push((key.trim_matches('"').to_string(), value.trim().trim_matches('"').to_string()));
            }
        }
        out
    }

    /// DESIGN.md は tokens.ts の要約。色・角丸・余白・骨格の寸法・透明度が食い違わないようにする
    #[test]
    fn design_md_matches_tokens() {
        fn find(list: &[(String, String)], key: &str) -> String {
            list.iter().find(|(name, _)| name == key).unwrap_or_else(|| panic!("DESIGN.md に {key} がない")).1.clone()
        }
        let rgb = |v: &str| u32::from_str_radix(v.trim_start_matches('#'), 16).unwrap_or_else(|_| panic!("色でない: {v}"));
        let num = |v: &str| v.trim_end_matches("px").parse::<f32>().unwrap_or_else(|_| panic!("数値でない: {v}"));

        let colors = design("colors");
        let tokens = section("colors");
        // DESIGN.md の名前（primary など）と tokens.ts の名前が違うものを含む
        for (name, key) in [
            ("backdrop", "backdrop"),
            ("bg", "bg"),
            ("panel", "panel"),
            ("raised", "raised"),
            ("hover", "hover"),
            ("line", "line"),
            ("line-soft", "lineSoft"),
            ("text", "text"),
            ("muted", "muted"),
            ("subtle", "subtle"),
            ("primary", "accent"),
            ("primary-bg", "accentBg"),
            ("select", "select"),
            ("live", "live"),
            ("now", "now"),
            ("err", "err"),
            ("warn", "warn"),
            ("diff-add", "diffAdd"),
            ("diff-del", "diffDel"),
            ("claude", "claude"),
            ("codex", "codex"),
        ] {
            assert_eq!(rgb(&find(&colors, name)), hex(tokens, key), "DESIGN.md colors.{name}");
        }
        // brand は 1 行に claude, codex の順で並ぶ
        let brand = hexes(section("brand"));
        assert_eq!(rgb(&find(&colors, "logo-claude")), brand[0], "DESIGN.md colors.logo-claude");
        assert_eq!(rgb(&find(&colors, "logo-codex")), brand[1], "DESIGN.md colors.logo-codex");
        let groups = section("eventGroups");
        for key in ["user", "assistant", "thinking", "reference", "edit", "shell", "agent", "other"] {
            assert_eq!(rgb(&find(&colors, &format!("event-{key}"))), hex(groups, key), "DESIGN.md colors.event-{key}");
        }
        for (i, value) in hexes(section("heat")).into_iter().enumerate() {
            assert_eq!(rgb(&find(&colors, &format!("heat-{}", i + 1))), value, "DESIGN.md colors.heat-{}", i + 1);
        }
        for (i, value) in hexes(section("projectColors")).into_iter().enumerate() {
            assert_eq!(rgb(&find(&colors, &format!("project-{}", i + 1))), value, "DESIGN.md colors.project-{}", i + 1);
        }
        // 幕と影は透明度を含む色なので、文字列のまま比べる
        let overlay = section("overlay");
        for key in ["scrim", "shadow"] {
            let at = overlay.find(&format!("{key}: '")).unwrap_or_else(|| panic!("overlay.{key} がない")) + key.len() + 3;
            let raw = &overlay[at..at + overlay[at..].find('\'').unwrap()];
            assert_eq!(find(&colors, key), raw, "DESIGN.md colors.{key}");
        }

        let rounded = design("rounded");
        let radius = section("radius");
        assert_eq!(rounded.len(), 5, "DESIGN.md の rounded の数");
        for (key, value) in &rounded {
            assert_eq!(num(value), number(radius, key), "DESIGN.md rounded.{key}");
        }

        // spacing は余白の段階（数字のキー）と骨格の寸法（名前のキー）を一つにまとめている
        let spacing = design("spacing");
        let (steps, named): (Vec<_>, Vec<_>) = spacing.iter().partition(|(key, _)| key.chars().all(|c| c.is_ascii_digit()));
        let (space, dimensions) = (section("space"), section("dimensions"));
        for (key, value) in &steps {
            assert_eq!(num(value), number(space, key), "DESIGN.md spacing.{key}");
        }
        for (key, value) in &named {
            assert_eq!(num(value), number(dimensions, key), "DESIGN.md spacing.{key}");
        }
        let count = |section: &str| section.lines().filter(|l| l.contains(": ") && !l.starts_with("export")).count();
        assert_eq!(named.len(), count(dimensions), "DESIGN.md の骨格の寸法の数");
        assert_eq!(steps.len(), 6, "DESIGN.md の余白の段階の数");

        let alpha = design("alpha");
        assert_eq!(alpha.len(), 4, "DESIGN.md の alpha の数");
        for (key, value) in &alpha {
            assert_eq!(value.parse::<f32>().unwrap(), number(section("alpha"), key), "DESIGN.md alpha.{key}");
        }
    }

    /// 画面のソースに、段階を通さない文字サイズ・角丸・色・透明度や、廃止した呼び名が戻らないようにする
    #[test]
    fn app_sources_use_the_scale() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().unwrap() != "tests.rs" {
                    out.push(path);
                }
            }
        }
        // 構文強調と ANSI の表は、意味の色ではなく内容の色なので直書きを許す
        const RAW_COLORS: [&str; 2] = ["rich.rs", "rich/syntax.rs"];
        // 影はポップオーバーとモーダルだけが持つ
        const SHADOW_FILES: [&str; 3] = ["range.rs", "overlay.rs", "mcp.rs"];
        const BANNED: &[&str] = &[
            ".text_xs(",
            ".text_sm(",
            ".text_lg(",
            ".text_base(",
            ".text_size(",
            ".rounded_sm(",
            ".rounded_md(",
            ".rounded_lg(",
            ".rounded_xl(",
            "gpui::white()",
            "theme::SUB",
            "theme::OK",
            "API コスト",
            "API 換算料金",
            "推定料金",
            // 太さは通常と 600 の 2 つだけ
            "FontWeight::THIN",
            "FontWeight::EXTRA_LIGHT",
            "FontWeight::LIGHT",
            "FontWeight::MEDIUM",
            "FontWeight::BOLD",
            "FontWeight::EXTRA_BOLD",
            "FontWeight::BLACK",
            // 色付きの左帯は付けない（区切り線の border_l_1 は許す）
            ".border_l_2(",
            ".border_l_4(",
            ".border_l_8(",
            ".shadow_2xs(",
            ".shadow_xs(",
            ".shadow_sm(",
            ".shadow_md(",
            ".shadow_xl(",
            ".shadow_2xl(",
        ];
        let mut files = Vec::new();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        walk(&root.join("app"), &mut files);
        files.push(root.join("app.rs"));
        files.push(root.join("main.rs"));
        assert!(files.len() > 20, "ソースを見つけられない");
        let mut found = Vec::new();
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            // テストモジュールは検査しない
            let code = source.split("#[cfg(test)]").next().unwrap();
            let rel = path.strip_prefix(&root).unwrap().to_str().unwrap().replace('\\', "/");
            let name = rel.strip_prefix("app/").unwrap_or(&rel);
            for pattern in BANNED {
                // `theme::SUB` が `theme::SUBTLE` に当たらないよう、識別子の途中で終わるものは除く
                let hit = code.match_indices(pattern).any(|(at, _)| {
                    pattern.ends_with('(')
                        || !code[at + pattern.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                });
                if hit {
                    found.push(format!("{name}: {pattern}"));
                }
            }
            if code.contains(".shadow_lg(") && !SHADOW_FILES.contains(&name) {
                found.push(format!("{name}: 影はポップオーバーとモーダルだけ"));
            }
            for (at, _) in code.match_indices("gpui::black()") {
                if !code[at..].starts_with("gpui::black().opacity(theme::SCRIM)") {
                    found.push(format!("{name}: gpui::black()"));
                }
            }
            for (at, _) in code.match_indices(".opacity(") {
                let next = &code[at + ".opacity(".len()..];
                if next.starts_with(|c: char| c.is_ascii_digit()) && !next.starts_with("0.0)") {
                    found.push(format!("{name}: 透明度の直書き {}", &next[..next.find(')').unwrap_or(8)]));
                }
            }
            // 余白は 4px の倍数（1〜2px の区切りのすき間を除く）。rem 指定の 0.5・1.5・2.5・3.5 は使わない
            for m in [
                "p", "px", "py", "pt", "pb", "pl", "pr", "m", "mx", "my", "mt", "mb", "ml", "mr", "gap", "gap_x",
                "gap_y",
            ] {
                for odd in ["0p5", "1p5", "2p5", "3p5"] {
                    if code.contains(&format!(".{m}_{odd}(")) {
                        found.push(format!("{name}: 余白 .{m}_{odd}()"));
                    }
                }
                let call = format!(".{m}(px(");
                for (at, _) in code.match_indices(&call) {
                    let rest = &code[at + call.len()..];
                    let Ok(value) = rest[..rest.find(')').unwrap_or(0)].parse::<f32>() else { continue };
                    let line_start = code[..at].rfind('\n').map_or(0, |n| n + 1);
                    let line_end = code[at..].find('\n').map_or(code.len(), |n| at + n);
                    if code[line_start..line_end].contains("余白の例外") {
                        continue;
                    }
                    if value < 0.0 || (value > 2.0 && value % 4.0 != 0.0) {
                        found.push(format!("{name}: 余白 {call}{value})"));
                    }
                }
            }
            if !RAW_COLORS.contains(&name) {
                for (at, _) in code.match_indices("0x") {
                    let digits = code[at + 2..].chars().take_while(char::is_ascii_hexdigit).count();
                    if digits == 6 {
                        found.push(format!("{name}: 色の直書き {}", &code[at..at + 8]));
                    }
                }
            }
        }
        assert!(found.is_empty(), "theme の段階を使う:\n{}", found.join("\n"));
    }
}
