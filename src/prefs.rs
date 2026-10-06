//! 画面の状態（表示モード・パネルの開閉と幅・絞り込みなど）を保存し、次回の起動時に復元する

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::layout::Granularity;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Day,
    Week,
    Timeline,
}

/// 表示するセッションの読み込み元
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFilter {
    All,
    Claude,
    Codex,
}

/// 詳細パネルのタブ
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailTab {
    /// 選んだイベントの入力・結果、または一覧
    #[default]
    Events,
    /// 編集したファイル
    Files,
    /// セッションの情報と集計
    Session,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub mode: Mode,
    /// 統計画面を開いているか（履歴の表示モードは別に保持する）
    pub statistics: bool,
    /// カレンダーで最後に使った表示（日 / 週）
    pub calendar: Mode,
    pub sidebar: bool,
    pub sidebar_w: f32,
    pub label_w: f32,
    pub detail_w: f32,
    pub gran: Granularity,
    /// タイムラインで表示範囲内のセッションだけを並べるか
    pub in_view_only: bool,
    /// タイムラインでディレクトリごとにメインイベントを詰めて表示するか
    pub directory_timeline: bool,
    pub source: SourceFilter,
    /// 絞り込み中のプロジェクト（作業ディレクトリ）
    pub focus: Option<String>,
    /// タイムラインで折りたたんだプロジェクト
    pub collapsed: Vec<String>,
    /// サイドバーで折りたたんだ区分
    pub folded: Vec<String>,
    /// 詳細パネルで開いているタブ
    pub detail_tab: DetailTab,
    /// 詳細パネルのイベントを新しい順に表示するか
    pub detail_reversed: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: Mode::Timeline,
            statistics: false,
            calendar: Mode::Week,
            sidebar: true,
            sidebar_w: 260.0,
            label_w: 260.0,
            detail_w: 440.0,
            gran: Granularity::Auto,
            in_view_only: false,
            directory_timeline: false,
            source: SourceFilter::All,
            focus: None,
            collapsed: Vec::new(),
            folded: Vec::new(),
            detail_tab: DetailTab::Events,
            detail_reversed: false,
        }
    }
}

fn path() -> PathBuf {
    if let Some(dir) = std::env::var_os("RIREKI_CONFIG_DIR") { return PathBuf::from(dir).join("prefs.json"); }
    let base = dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    base.join("rireki").join("prefs.json")
}

pub fn load() -> Prefs {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(p: &Prefs) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec_pretty(p) {
        Ok(b) => {
            if let Err(e) = std::fs::write(&path, b) {
                eprintln!("rireki: 画面の状態を保存できません: {e}");
            }
        }
        Err(e) => eprintln!("rireki: 画面の状態を保存できません: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial() {
        let p = Prefs {
            focus: Some("/w".into()), gran: Granularity::Turn, statistics: true,
            detail_reversed: true, directory_timeline: true, ..Prefs::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Prefs>(&s).unwrap(), p);
        // 足りない項目は既定値で補う（項目を増やしても古いファイルを読める）
        let q: Prefs = serde_json::from_str(r#"{"mode":"day"}"#).unwrap();
        assert_eq!((q.mode, q.sidebar), (Mode::Day, true));
        assert!(!q.statistics);
        assert!(!q.detail_reversed);
        assert!(!q.directory_timeline);
    }
}
