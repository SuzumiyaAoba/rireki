//! UI アイコン。Lucide (ISC License, https://lucide.dev) の SVG をバイナリに埋め込み、
//! イベントの種類のアイコンは Iconify の lucide セット、Claude / Codex のロゴは Iconify の simple-icons から取得した `assets/icons/*.svg` を使う。
//! GPUI の `AssetSource` として `icons/<name>.svg` のパスで提供する。

use gpui::{AssetSource, Result, SharedString, Svg, prelude::*, svg};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    CalendarDay,
    CalendarWeek,
    Calendar,
    Timeline,
    Chart,
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
    ArrowDown,
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
    // イベントの種類（Iconify の lucide セットから取得）
    User,
    MessageSquare,
    Brain,
    FileText,
    TextSearch,
    FolderSearch,
    Image,
    FilePen,
    FilePlus,
    NotebookPen,
    Hourglass,
    Globe,
    CloudDownload,
    ListChecks,
    Map,
    Target,
    Plug,
    Sparkles,
    Wrench,
}

impl Icon {
    const ALL: [Icon; 55] = [
        Icon::CalendarDay,
        Icon::CalendarWeek,
        Icon::Calendar,
        Icon::Timeline,
        Icon::Chart,
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
        Icon::ArrowDown,
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
        Icon::User,
        Icon::MessageSquare,
        Icon::Brain,
        Icon::FileText,
        Icon::TextSearch,
        Icon::FolderSearch,
        Icon::Image,
        Icon::FilePen,
        Icon::FilePlus,
        Icon::NotebookPen,
        Icon::Hourglass,
        Icon::Globe,
        Icon::CloudDownload,
        Icon::ListChecks,
        Icon::Map,
        Icon::Target,
        Icon::Plug,
        Icon::Sparkles,
        Icon::Wrench,
    ];

    fn name(self) -> &'static str {
        match self {
            Icon::CalendarDay => "calendar-day",
            Icon::CalendarWeek => "calendar-week",
            Icon::Calendar => "calendar",
            Icon::Timeline => "timeline",
            Icon::Chart => "chart",
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
            Icon::ArrowDown => "arrow-down",
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
            Icon::User => "user",
            Icon::MessageSquare => "message-square",
            Icon::Brain => "brain",
            Icon::FileText => "file-text",
            Icon::TextSearch => "text-search",
            Icon::FolderSearch => "folder-search",
            Icon::Image => "image",
            Icon::FilePen => "file-pen",
            Icon::FilePlus => "file-plus",
            Icon::NotebookPen => "notebook-pen",
            Icon::Hourglass => "hourglass",
            Icon::Globe => "globe",
            Icon::CloudDownload => "cloud-download",
            Icon::ListChecks => "list-checks",
            Icon::Map => "map",
            Icon::Target => "target",
            Icon::Plug => "plug",
            Icon::Sparkles => "sparkles",
            Icon::Wrench => "wrench",
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
            Icon::Chart => r#"<path d="M3 3v16a2 2 0 0 0 2 2h16"/><path d="M8 16v-3"/><path d="M13 16V8"/><path d="M18 16v-5"/>"#,
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
            Icon::ArrowDown => r#"<path d="M12 5v14"/><path d="m19 12-7 7-7-7"/>"#,
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
            Icon::User => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></g>"#,
            Icon::MessageSquare => r#"<path fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"/>"#,
            Icon::Brain => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M12 18V5m3 8a4.17 4.17 0 0 1-3-4a4.17 4.17 0 0 1-3 4m8.598-6.5A3 3 0 1 0 12 5a3 3 0 1 0-5.598 1.5"/><path d="M17.997 5.125a4 4 0 0 1 2.526 5.77"/><path d="M18 18a4 4 0 0 0 2-7.464"/><path d="M19.967 17.483A4 4 0 1 1 12 18a4 4 0 1 1-7.967-.517"/><path d="M6 18a4 4 0 0 1-2-7.464"/><path d="M6.003 5.125a4 4 0 0 0-2.526 5.77"/></g>"#,
            Icon::FileText => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5M10 9H8m8 4H8m8 4H8"/></g>"#,
            Icon::TextSearch => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M21 5H3m7 7H3m7 7H3"/><circle cx="17" cy="15" r="3"/><path d="m21 19l-1.9-1.9"/></g>"#,
            Icon::FolderSearch => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M10.7 20H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H20a2 2 0 0 1 2 2v4.1M21 21l-1.9-1.9"/><circle cx="17" cy="17" r="3"/></g>"#,
            Icon::Image => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15l-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/></g>"#,
            Icon::FilePen => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M12.659 22H18a2 2 0 0 0 2-2V8a2.4 2.4 0 0 0-.706-1.706l-3.588-3.588A2.4 2.4 0 0 0 14 2H6a2 2 0 0 0-2 2v9.34"/><path d="M14 2v5a1 1 0 0 0 1 1h5m-9.622 4.622a1 1 0 0 1 3 3.003L8.36 20.637a2 2 0 0 1-.854.506l-2.867.837a.5.5 0 0 1-.62-.62l.836-2.869a2 2 0 0 1 .506-.853z"/></g>"#,
            Icon::FilePlus => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5M9 15h6m-3 3v-6"/></g>"#,
            Icon::NotebookPen => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M13.4 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-7.4M2 6h4m-4 4h4m-4 4h4m-4 4h4"/><path d="M21.378 5.626a1 1 0 1 0-3.004-3.004l-5.01 5.012a2 2 0 0 0-.506.854l-.837 2.87a.5.5 0 0 0 .62.62l2.87-.837a2 2 0 0 0 .854-.506z"/></g>"#,
            Icon::Hourglass => r#"<path fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 22h14M5 2h14m-2 20v-4.172a2 2 0 0 0-.586-1.414L12 12l-4.414 4.414A2 2 0 0 0 7 17.828V22M7 2v4.172a2 2 0 0 0 .586 1.414L12 12l4.414-4.414A2 2 0 0 0 17 6.172V2"/>"#,
            Icon::Globe => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20a14.5 14.5 0 0 0 0-20M2 12h20"/></g>"#,
            Icon::CloudDownload => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M12 13v8l-4-4m4 4l4-4"/><path d="M4.393 15.269A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.436 8.284"/></g>"#,
            Icon::ListChecks => r#"<path fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 5h8m-8 7h8m-8 7h8M3 17l2 2l4-4M3 7l2 2l4-4"/>"#,
            Icon::Map => r#"<path fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M14.106 5.553a2 2 0 0 0 1.788 0l3.659-1.83A1 1 0 0 1 21 4.619v12.764a1 1 0 0 1-.553.894l-4.553 2.277a2 2 0 0 1-1.788 0l-4.212-2.106a2 2 0 0 0-1.788 0l-3.659 1.83A1 1 0 0 1 3 19.381V6.618a1 1 0 0 1 .553-.894l4.553-2.277a2 2 0 0 1 1.788 0zm.894.211v15M9 3.236v15"/>"#,
            Icon::Target => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="6"/><circle cx="12" cy="12" r="2"/></g>"#,
            Icon::Plug => r#"<path fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 22v-5m3-9V2m2 6a1 1 0 0 1 1 1v4a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V9a1 1 0 0 1 1-1zM9 8V2"/>"#,
            Icon::Sparkles => r#"<g fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"><path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594zM20 2v4m2-2h-4"/><circle cx="4" cy="20" r="2"/></g>"#,
            Icon::Wrench => r#"<path fill="none" stroke="black" stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.106-3.105c.32-.322.863-.22.983.218a6 6 0 0 1-8.259 7.057l-7.91 7.91a1 1 0 0 1-2.999-3l7.91-7.91a6 6 0 0 1 7.057-8.259c.438.12.54.662.219.984z"/>"#,
            Icon::Refresh => {
                r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#
            }
        }
    }

    pub fn path(self) -> String {
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
