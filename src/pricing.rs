//! モデルごとの料金表とトークン使用量の料金換算
//!
//! 料金は 100 万トークンあたりの USD。Claude の標準料金は組み込みで持ち、
//! `~/Library/Application Support/rireki/prices.json` で追加・上書きできる（Codex の OpenAI モデルなど）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::model::Usage;

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// 省略時は input の 0.1 倍
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// 5 分キャッシュの書き込み。省略時は input の 1.25 倍
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    /// 1 時間キャッシュの書き込み。省略時は input の 2 倍
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<f64>,
}

impl Price {
    const fn new(input: f64, output: f64, cache_read: f64) -> Self {
        Self { input, output, cache_read: Some(cache_read), cache_write: None, cache_write_1h: None }
    }

    /// 使用量の料金（USD）
    pub fn cost(&self, u: &Usage) -> f64 {
        let w1h = u.cache_write_1h.min(u.cache_write) as f64;
        let w5m = u.cache_write as f64 - w1h;
        (u.input as f64 * self.input
            + u.output as f64 * self.output
            + u.cache_read as f64 * self.cache_read.unwrap_or(self.input * 0.1)
            + w5m * self.cache_write.unwrap_or(self.input * 1.25)
            + w1h * self.cache_write_1h.unwrap_or(self.input * 2.0))
            / 1e6
    }
}

/// OpenAI の料金。キャッシュ書き込みの割増はないので入力と同額にする
const fn openai(input: f64, cached: f64, output: f64) -> Price {
    Price { input, output, cache_read: Some(cached), cache_write: Some(input), cache_write_1h: Some(input) }
}

/// 組み込みの料金（2026-10 時点の各社 API の標準料金）
///
/// - Claude: <https://platform.claude.com/docs/en/about-claude/pricing>
/// - OpenAI: <https://developers.openai.com/api/docs/pricing> と各モデルのページ
///
/// gpt-5.3-codex-spark は API の料金が公開されていないので載せない
const BUILTIN: &[(&str, Price)] = &[
    ("claude-fable-5-1", Price::new(10.0, 50.0, 0.25)),
    ("claude-mythos-5-1", Price::new(10.0, 50.0, 0.25)),
    ("claude-fable-5", Price::new(10.0, 50.0, 1.0)),
    ("claude-mythos-5", Price::new(10.0, 50.0, 1.0)),
    ("claude-opus-5-5", Price::new(4.0, 20.0, 0.2)),
    ("claude-opus-5", Price::new(5.0, 25.0, 0.5)),
    ("claude-opus-4-8", Price::new(5.0, 25.0, 0.5)),
    ("claude-opus-4-7", Price::new(5.0, 25.0, 0.5)),
    ("claude-opus-4-6", Price::new(5.0, 25.0, 0.5)),
    ("claude-opus-4-5", Price::new(5.0, 25.0, 0.5)),
    ("claude-opus-4-1", Price::new(15.0, 75.0, 1.5)),
    ("claude-opus-4", Price::new(15.0, 75.0, 1.5)),
    ("claude-sonnet-5-5", Price::new(2.0, 10.0, 0.2)),
    ("claude-sonnet-5", Price::new(2.0, 10.0, 0.2)),
    ("claude-sonnet-4-6", Price::new(3.0, 15.0, 0.3)),
    ("claude-sonnet-4-5", Price::new(3.0, 15.0, 0.3)),
    ("claude-sonnet-4", Price::new(3.0, 15.0, 0.3)),
    ("claude-haiku-4-5", Price::new(1.0, 5.0, 0.1)),
    ("claude-3-5-haiku", Price::new(0.8, 4.0, 0.08)),
    ("gpt-6-astra", openai(10.0, 1.0, 50.0)),
    ("gpt-6.1-sol", openai(2.0, 0.1, 10.0)),
    ("gpt-6-sol", openai(2.0, 0.2, 10.0)),
    ("gpt-6-luna", openai(0.1, 0.01, 0.5)),
    ("gpt-5.6-sol", openai(4.0, 0.4, 20.0)),
    ("gpt-5.6-terra", openai(2.0, 0.2, 12.0)),
    ("gpt-5.6-luna", openai(0.2, 0.02, 1.2)),
    (
        "gpt-5.5-pro",
        Price { input: 30.0, output: 180.0, cache_read: Some(30.0), cache_write: Some(30.0), cache_write_1h: Some(30.0) },
    ),
    ("gpt-5.5", openai(5.0, 0.5, 30.0)),
    ("gpt-5.4-mini", openai(0.75, 0.075, 4.5)),
    ("gpt-5.4", openai(2.5, 0.25, 15.0)),
    ("gpt-5.3-codex", openai(1.75, 0.175, 14.0)),
    ("gpt-5.2-codex", openai(1.75, 0.175, 14.0)),
    ("gpt-5.2", openai(1.75, 0.175, 14.0)),
    ("gpt-5.1-codex-max", openai(1.25, 0.125, 10.0)),
    ("gpt-5.1-codex", openai(1.25, 0.125, 10.0)),
    ("gpt-5.1", openai(1.25, 0.125, 10.0)),
    ("gpt-5-codex", openai(1.25, 0.125, 10.0)),
    ("gpt-5-mini", openai(0.25, 0.025, 2.0)),
    ("gpt-5-nano", openai(0.05, 0.005, 0.4)),
    ("gpt-5", openai(1.25, 0.125, 10.0)),
];

pub fn path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    base.join("rireki").join("prices.json")
}

/// 組み込み + prices.json。ファイルがなければ組み込みの内容で作る（編集用のひな形）
static TABLE: LazyLock<Vec<(String, Price)>> = LazyLock::new(|| {
    let mut map: BTreeMap<String, Price> = BUILTIN.iter().map(|(k, p)| (k.to_string(), *p)).collect();
    let p = path();
    match std::fs::read(&p) {
        Ok(b) => match serde_json::from_slice::<BTreeMap<String, Price>>(&b) {
            Ok(user) => map.extend(user),
            Err(e) => eprintln!("rireki: {} を読めません: {e}", p.display()),
        },
        Err(_) => {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(b) = serde_json::to_vec_pretty(&map) {
                let _ = std::fs::write(&p, b);
            }
        }
    }
    // 長いキーを先に試す（claude-opus-5-5 を claude-opus-5 より優先）
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    v
});

/// モデル名の料金。完全一致か、日付などの数字の付いた名前（claude-haiku-4-5-20251001）に当てる。
/// gpt-5.3-codex-spark のような別モデルを gpt-5.3-codex と取り違えないよう、続きが `-数字` のときだけ前方一致とする
pub fn price(model: &str) -> Option<Price> {
    TABLE.iter().find(|(k, _)| matches(model, k)).map(|(_, p)| *p)
}

fn matches(model: &str, key: &str) -> bool {
    match model.strip_prefix(key) {
        Some("") => true,
        Some(rest) => rest.strip_prefix('-').is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_and_cost() {
        let p = BUILTIN.iter().find(|(k, _)| *k == "claude-opus-5-5").unwrap().1;
        let u = Usage {
            input: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 2_000_000,
            cache_write_1h: 1_000_000,
            output: 1_000_000,
            reasoning: 0,
        };
        // 4 + 0.2 + 5 (5m) + 8 (1h) + 20
        assert!((p.cost(&u) - 37.2).abs() < 1e-9);
        let mut v: Vec<_> = BUILTIN.to_vec();
        v.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
        let hit = |m: &str| v.iter().find(|(k, _)| matches(m, k)).map(|(k, _)| *k);
        assert_eq!(hit("claude-opus-5-5"), Some("claude-opus-5-5"));
        assert_eq!(hit("claude-haiku-4-5-20251001"), Some("claude-haiku-4-5"));
        assert_eq!(hit("gpt-5.3-codex-spark"), None);
        assert_eq!(hit("gpt-5.6-luna"), Some("gpt-5.6-luna"));
        assert_eq!(hit("gpt-5.6-unknown"), None);
    }
}
