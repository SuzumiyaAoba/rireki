//! モデルごとの料金表とトークン使用量の料金換算
//!
//! 料金は 100 万トークンあたりの USD。Claude の標準料金は組み込みで持ち、
//! `~/Library/Application Support/rireki/prices.json` で追加・上書きできる（Codex の OpenAI モデルなど）。

use std::collections::{BTreeMap, HashMap};
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

/// 集計中の料金検索。同じモデルが続く間は文字列のハッシュを計算しない。
/// モデル名の表は256件まで保持し、直前の1モデルは別に持つ。上限後も料金表で正しく照会する。
#[derive(Default)]
pub(crate) struct Cache<'a> {
    prices: HashMap<Option<&'a str>, Option<&'static Price>>,
    // Vacant の分岐だけで上限を調べ、既存のモデルの照会には判定を足さない。
    inserted: u16,
    last: Option<(Option<&'a str>, Option<&'static Price>)>,
}

impl<'a> Cache<'a> {
    #[inline]
    pub(crate) fn get(&mut self, model: Option<&'a str>) -> Option<Price> {
        self.get_ref(model).copied()
    }

    #[inline]
    fn get_ref(&mut self, model: Option<&'a str>) -> Option<&'static Price> {
        model?;
        if let Some((last, price)) = self.last && last == model {
            return price;
        }
        self.get_changed(model)
    }

    #[inline(never)]
    fn get_changed(&mut self, model: Option<&'a str>) -> Option<&'static Price> {
        let price = match self.prices.entry(model) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let price = model.and_then(price_ref);
                if self.inserted < 256 { entry.insert(price); self.inserted += 1; }
                price
            }
        };
        self.last = Some((model, price));
        price
    }
}

impl Price {
    const fn new(input: f64, output: f64, cache_read: f64) -> Self {
        Self { input, output, cache_read: Some(cache_read), cache_write: None, cache_write_1h: None }
    }

    /// 使用量の料金（USD）
    #[inline]
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
    TABLE.iter().find(|(key, _)| matches(model, key)).map(|(_, price)| *price)
}

#[inline]
fn price_ref(model: &str) -> Option<&'static Price> {
    let table = &*TABLE;
    table.first().filter(|(key, _)| matches(model, key))
        .or_else(|| price_tail(table, model)).map(|(_, price)| price)
}

// 先頭候補はそのまま試し、残りは同じ先頭バイトのキーだけを長い順に試す。
struct Candidates {
    indices: Vec<usize>,
    offsets: [usize; 257],
    empty: Option<usize>,
}

impl Candidates {
    fn new(table: &[(String, Price)]) -> Self {
        let mut indices: Vec<_> = (1..table.len()).filter(|&i| !table[i].0.is_empty()).collect();
        // 安定ソートなので、各バイト内の長いキーからの照合順は変わらない。
        indices.sort_by_key(|&i| table[i].0.as_bytes()[0]);
        let mut offsets = [0; 257];
        for &i in &indices {
            offsets[table[i].0.as_bytes()[0] as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        Self { indices, offsets, empty: table.iter().position(|(key, _)| key.is_empty()) }
    }

    fn get<'a>(&self, table: &'a [(String, Price)], model: &str) -> Option<&'a (String, Price)> {
        let price = model.as_bytes().first().and_then(|&first| {
            let first = first as usize;
            self.indices[self.offsets[first]..self.offsets[first + 1]].iter()
                .find_map(|&i| matches(model, &table[i].0).then_some(&table[i]))
        });
        price.or_else(|| self.empty.and_then(|i| matches(model, &table[i].0).then_some(&table[i])))
    }
}

static CANDIDATES: LazyLock<Candidates> = LazyLock::new(|| Candidates::new(&TABLE));

#[inline(never)]
fn price_tail<'a>(table: &'a [(String, Price)], model: &str) -> Option<&'a (String, Price)> {
    CANDIDATES.get(table, model)
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
    fn candidate_index_preserves_longest_prefix_unicode_and_empty_keys() {
        let keys = ["", "x", "x-1", "x-1-2", "x-1-2-日本語", "x-1-２", "x-", "x--1",
            "日本語", "日本語-1", "💡-10", "-", "-1", "a", "a-1", "\0", "\0-1"];
        let mut table: Vec<_> = keys.iter().enumerate()
            .map(|(i, key)| ((*key).to_owned(), Price::new(i as f64, 1.0, 0.1))).collect();
        table.sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
        let candidates = Candidates::new(&table);
        let check = |model: &str| {
            let expected = table.iter().find(|(key, _)| matches(model, key));
            let actual = table.first().filter(|(key, _)| matches(model, key))
                .or_else(|| candidates.get(&table, model));
            assert_eq!(actual, expected, "{model:?}");
        };
        let alphabet = ["x", "-", "1", "a", "日", "１", "💡"];
        for n in 0..=6 {
            for mut code in 0..alphabet.len().pow(n) {
                let mut model = String::new();
                for _ in 0..n { model.push_str(alphabet[code % alphabet.len()]); code /= alphabet.len(); }
                check(&model);
            }
        }
        let suffixes = ["", "-1", "-12abc", "-0-xyz", "-01-3", "-name", "--1", "-１", "-😀", "\0-1"];
        for key in keys { for suffix in suffixes { check(&format!("{key}{suffix}")); } }
        // 公開関数も実際の組み込み・ユーザー料金表の全候補で前版の走査と比較する。
        for (key, _) in &*TABLE {
            for suffix in suffixes {
                let model = format!("{key}{suffix}");
                let expected = TABLE.iter().find(|(key, _)| matches(&model, key)).map(|(_, price)| *price);
                assert_eq!(price_ref(&model).copied(), expected, "{model:?}");
            }
        }
    }

    #[test]
    fn bounded_cache_keeps_exact_prices_after_many_models_and_none() {
        let models: Vec<_> = (0..600).map(|i| if i % 2 == 0 {
            format!("claude-sonnet-4-5-{:08}", i + 1)
        } else { format!("unpriced-{i}") }).collect();
        let mut cache = Cache::default();
        for model in models.iter().chain(models.iter().rev()).chain(models.iter().cycle().take(1200)) {
            assert_eq!(cache.get(Some(model)), price(model));
            assert_eq!(cache.get(None), None);
            assert_eq!(cache.get(Some(model)), price(model));
            assert!(cache.prices.len() <= 256);
        }
        assert_eq!(cache.prices.len(), 256);
        assert!(!cache.prices.contains_key(&None));
        assert_eq!(cache.get(Some("")), price(""));
        assert_eq!(cache.get(Some("日本語-1")), price("日本語-1"));
    }

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
