//! UI アイコン。Lucide (ISC License, https://lucide.dev) の SVG をバイナリに埋め込み、
//! Claude / Codex のロゴは Iconify の simple-icons から取得した `assets/icons/*.svg` を使う。
//! GPUI の `AssetSource` として `icons/<name>.svg` のパスで提供する。

use gpui::{AssetSource, Result, SharedString, Svg, prelude::*, svg};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    CalendarDay,
    CalendarWeek,
    Timeline,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    Today,
    ZoomIn,
    ZoomOut,
    ArrowLeft,
    ArrowUp,
    Bot,
    Layers,
    Folder,
    Claude,
    Codex,
}

impl Icon {
    const ALL: [Icon; 16] = [
        Icon::CalendarDay,
        Icon::CalendarWeek,
        Icon::Timeline,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::Today,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::ArrowLeft,
        Icon::ArrowUp,
        Icon::Bot,
        Icon::Layers,
        Icon::Folder,
        Icon::Claude,
        Icon::Codex,
    ];

    fn name(self) -> &'static str {
        match self {
            Icon::CalendarDay => "calendar-day",
            Icon::CalendarWeek => "calendar-week",
            Icon::Timeline => "timeline",
            Icon::ChevronLeft => "chevron-left",
            Icon::ChevronRight => "chevron-right",
            Icon::ChevronDown => "chevron-down",
            Icon::Today => "today",
            Icon::ZoomIn => "zoom-in",
            Icon::ZoomOut => "zoom-out",
            Icon::ArrowLeft => "arrow-left",
            Icon::ArrowUp => "arrow-up",
            Icon::Bot => "bot",
            Icon::Layers => "layers",
            Icon::Folder => "folder",
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
            Icon::Timeline => {
                r#"<path d="M3 3v16a2 2 0 0 0 2 2h16"/><path d="M7 7h8"/><path d="M11 12h8"/><path d="M9 17h5"/>"#
            }
            Icon::ChevronLeft => r#"<path d="m15 18-6-6 6-6"/>"#,
            Icon::ChevronRight => r#"<path d="m9 18 6-6-6-6"/>"#,
            Icon::ChevronDown => r#"<path d="m6 9 6 6 6-6"/>"#,
            Icon::Today => {
                r#"<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="9"/><path d="M12 1v2"/><path d="M12 21v2"/><path d="M1 12h2"/><path d="M21 12h2"/>"#
            }
            Icon::ZoomIn => {
                r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/><path d="M11 8v6"/><path d="M8 11h6"/>"#
            }
            Icon::ZoomOut => r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/><path d="M8 11h6"/>"#,
            Icon::ArrowLeft => r#"<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>"#,
            Icon::ArrowUp => r#"<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>"#,
            Icon::Bot => {
                r#"<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2"/><path d="M20 14h2"/><path d="M15 13v2"/><path d="M9 13v2"/>"#
            }
            Icon::Layers => r#"<path d="m12 2 9 5-9 5-9-5z"/><path d="m3 12 9 5 9-5"/><path d="m3 17 9 5 9-5"/>"#,
            Icon::Folder => {
                r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
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
