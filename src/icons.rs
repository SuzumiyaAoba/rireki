//! UI アイコン。Lucide (ISC License, https://lucide.dev) の SVG をバイナリに埋め込み、
//! Claude / Codex のロゴは Iconify の simple-icons から取得した `assets/icons/*.svg` を使う。
//! GPUI の `AssetSource` として `icons/<name>.svg` のパスで提供する。

use gpui::{AssetSource, Result, SharedString, Svg, prelude::*, svg};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    CalendarDay,
    CalendarWeek,
    Calendar,
    Timeline,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    GitBranch,
    Today,
    ZoomIn,
    ZoomOut,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Bot,
    Layers,
    Folder,
    FolderOpen,
    Copy,
    Check,
    Search,
    X,
    PanelLeftClose,
    PanelLeftOpen,
    Keyboard,
    Terminal,
    Filter,
    Activity,
    Clock,
    Locate,
    Refresh,
    Claude,
    Codex,
}

impl Icon {
    const ALL: [Icon; 34] = [
        Icon::CalendarDay,
        Icon::CalendarWeek,
        Icon::Calendar,
        Icon::Timeline,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::ChevronUp,
        Icon::GitBranch,
        Icon::Today,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::ArrowLeft,
        Icon::ArrowRight,
        Icon::ArrowUp,
        Icon::Bot,
        Icon::Layers,
        Icon::Folder,
        Icon::FolderOpen,
        Icon::Copy,
        Icon::Check,
        Icon::Search,
        Icon::X,
        Icon::PanelLeftClose,
        Icon::PanelLeftOpen,
        Icon::Keyboard,
        Icon::Terminal,
        Icon::Filter,
        Icon::Activity,
        Icon::Clock,
        Icon::Locate,
        Icon::Refresh,
        Icon::Claude,
        Icon::Codex,
    ];

    fn name(self) -> &'static str {
        match self {
            Icon::CalendarDay => "calendar-day",
            Icon::CalendarWeek => "calendar-week",
            Icon::Calendar => "calendar",
            Icon::Timeline => "timeline",
            Icon::ChevronLeft => "chevron-left",
            Icon::ChevronRight => "chevron-right",
            Icon::ChevronDown => "chevron-down",
            Icon::ChevronUp => "chevron-up",
            Icon::GitBranch => "git-branch",
            Icon::Today => "today",
            Icon::ZoomIn => "zoom-in",
            Icon::ZoomOut => "zoom-out",
            Icon::ArrowLeft => "arrow-left",
            Icon::ArrowRight => "arrow-right",
            Icon::ArrowUp => "arrow-up",
            Icon::Bot => "bot",
            Icon::Layers => "layers",
            Icon::Folder => "folder",
            Icon::FolderOpen => "folder-open",
            Icon::Copy => "copy",
            Icon::Check => "check",
            Icon::Search => "search",
            Icon::X => "x",
            Icon::PanelLeftClose => "panel-left-close",
            Icon::PanelLeftOpen => "panel-left-open",
            Icon::Keyboard => "keyboard",
            Icon::Terminal => "terminal",
            Icon::Filter => "filter",
            Icon::Activity => "activity",
            Icon::Clock => "clock",
            Icon::Locate => "locate",
            Icon::Refresh => "refresh",
            Icon::Claude => "claude",
            Icon::Codex => "codex",
        }
    }

    /// 塗りつぶしのロゴ。SVG ファイル全体をそのまま使う
    fn file(self) -> Option<&'static [u8]> {
        match self {
            Icon::Claude => Some(include_bytes!("../assets/icons/claude.svg")),
            Icon::Codex => Some(include_bytes!("../assets/icons/codex.svg")),
            _ => None,
        }
    }

    /// 線画アイコンの SVG の中身（`<svg>` 要素の子）
    fn body(self) -> &'static str {
        match self {
            Icon::Claude | Icon::Codex => "",
            Icon::CalendarDay => {
                r#"<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/><path d="M12 14h.01"/>"#
            }
            Icon::CalendarWeek => {
                r#"<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/><path d="M7 15h10"/>"#
            }
            Icon::Calendar => {
                r#"<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/><path d="M8 14h.01"/><path d="M12 14h.01"/><path d="M16 14h.01"/><path d="M8 18h.01"/><path d="M12 18h.01"/><path d="M16 18h.01"/>"#
            }
            Icon::Timeline => {
                r#"<path d="M3 3v16a2 2 0 0 0 2 2h16"/><path d="M7 7h8"/><path d="M11 12h8"/><path d="M9 17h5"/>"#
            }
            Icon::ChevronLeft => r#"<path d="m15 18-6-6 6-6"/>"#,
            Icon::ChevronRight => r#"<path d="m9 18 6-6-6-6"/>"#,
            Icon::ChevronDown => r#"<path d="m6 9 6 6 6-6"/>"#,
            Icon::ChevronUp => r#"<path d="m18 15-6-6-6 6"/>"#,
            Icon::GitBranch => {
                r#"<line x1="6" x2="6" y1="3" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"#
            }
            Icon::Today => {
                r#"<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="9"/><path d="M12 1v2"/><path d="M12 21v2"/><path d="M1 12h2"/><path d="M21 12h2"/>"#
            }
            Icon::ZoomIn => {
                r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/><path d="M11 8v6"/><path d="M8 11h6"/>"#
            }
            Icon::ZoomOut => r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/><path d="M8 11h6"/>"#,
            Icon::ArrowLeft => r#"<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>"#,
            Icon::ArrowRight => r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#,
            Icon::ArrowUp => r#"<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>"#,
            Icon::Bot => {
                r#"<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>"#
            }
            Icon::Copy => {
                r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#
            }
            Icon::Check => r#"<path d="M20 6 9 17l-5-5"/>"#,
            Icon::Layers => r#"<path d="m12 2 9 5-9 5-9-5z"/><path d="m3 12 9 5 9-5"/><path d="m3 17 9 5 9-5"/>"#,
            Icon::Folder => {
                r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
            }
            Icon::FolderOpen => {
                r#"<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>"#
            }
            Icon::Search => r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#,
            Icon::X => r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#,
            Icon::PanelLeftClose => {
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/><path d="m16 15-3-3 3-3"/>"#
            }
            Icon::PanelLeftOpen => {
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/><path d="m14 9 3 3-3 3"/>"#
            }
            Icon::Keyboard => {
                r#"<path d="M10 8h.01"/><path d="M12 12h.01"/><path d="M14 8h.01"/><path d="M16 12h.01"/><path d="M18 8h.01"/><path d="M6 8h.01"/><path d="M7 16h10"/><path d="M8 12h.01"/><rect width="20" height="16" x="2" y="4" rx="2"/>"#
            }
            Icon::Terminal => r#"<path d="M12 19h8"/><path d="m4 17 6-6-6-6"/>"#,
            Icon::Filter => {
                r#"<path d="M10 20a1 1 0 0 0 .553.895l2 1A1 1 0 0 0 14 21v-7a2 2 0 0 1 .517-1.341L21.74 4.67A1 1 0 0 0 21 3H3a1 1 0 0 0-.742 1.67l7.225 7.989A2 2 0 0 1 10 14z"/>"#
            }
            Icon::Activity => {
                r#"<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>"#
            }
            Icon::Clock => r#"<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>"#,
            Icon::Locate => {
                r#"<path d="M2 12h3"/><path d="M19 12h3"/><path d="M12 2v3"/><path d="M12 19v3"/><circle cx="12" cy="12" r="7"/>"#
            }
            Icon::Refresh => {
                r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#
            }
        }
    }

    fn path(self) -> String {
        format!("icons/{}.svg", self.name())
    }
}

/// アイコン要素。色は親の `text_color` ではなく引数で指定する
pub fn icon(i: Icon, color: u32) -> Svg {
    svg().path(i.path()).size_3p5().flex_none().text_color(crate::theme::c(color))
}

/// セッションの読み込み元を表すロゴとその色
pub fn source_icon(s: crate::model::Source) -> (Icon, u32) {
    match s {
        crate::model::Source::Claude => (Icon::Claude, 0xd97757),
        crate::model::Source::Codex => (Icon::Codex, 0xc8c8c8),
    }
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL.into_iter().find(|i| i.path() == path).map(|i| {
            if let Some(b) = i.file() {
                return Cow::Borrowed(b);
            }
            let s = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
                i.body()
            );
            Cow::Owned(s.into_bytes())
        }))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Icon::ALL.into_iter().map(Icon::path).filter(|p| p.starts_with(path)).map(Into::into).collect())
    }
}
