//! 端末用設定の読み込み・保存。既存のファイル名と復元時の既定値を保つ。

use super::{Page, SourceFilter, State, Workspace, inspect, metrics, temporal};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub(super) page: Page,
    pub(super) source: SourceFilter,
    pub(super) project: Option<String>,
    pub(super) live_only: bool,
    pub(super) sidebar: bool,
    pub(super) sidebar_width: u16,
    pub(super) events_height: u16,
    pub(super) fullscreen: bool,
    pub(super) temporal: temporal::Settings,
    pub(super) metrics: metrics::Settings,
    pub(super) inspector: inspect::Settings,
    pub(super) mcp_port: u16,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            page: Page::History,
            source: SourceFilter::All,
            project: None,
            live_only: false,
            sidebar: true,
            sidebar_width: 32,
            events_height: 38,
            fullscreen: false,
            temporal: temporal::Settings::default(),
            metrics: metrics::Settings::default(),
            inspector: inspect::Settings::default(),
            mcp_port: rireki::mcp::DEFAULT_PORT,
        }
    }
}

fn preferences_path() -> PathBuf {
    // OS の設定ディレクトリがない場合は ./rireki を使う、TUI 固有の既存規則を保つ。
    std::env::var_os("RIREKI_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rireki")
        })
        .join("tui.json")
}

impl Preferences {
    pub(super) fn load(persistent: bool) -> Self {
        if persistent {
            std::fs::read(preferences_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }
}

impl Workspace {
    pub fn save(&self, state: &State) -> std::io::Result<()> {
        if !self.persistent {
            return Ok(());
        }
        let prefs = Preferences {
            page: self.page,
            source: state.source,
            project: state.project.clone(),
            live_only: state.live_only,
            sidebar: self.sidebar,
            sidebar_width: self.sidebar_width,
            events_height: self.events_height,
            fullscreen: self.fullscreen,
            temporal: self.temporal.settings(),
            metrics: self.metrics.settings(),
            inspector: self.inspector.settings(),
            mcp_port: self.mcp_port,
        };
        let path = preferences_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&prefs)?)
    }
}
