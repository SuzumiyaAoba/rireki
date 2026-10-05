//! 表示スケールに応じてイベントをブロックにまとめる

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::{Event, Kind, Lane};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    /// スケールに応じて自動でまとめる
    Auto,
    /// 同じツールの連続呼び出しをまとめる
    SameTool,
    /// ユーザー発話ごと（ターン単位）にまとめる
    Turn,
    /// 1 コール = 1 ブロック
    Call,
}

impl Granularity {
    pub const ALL: [Granularity; 4] = [Granularity::Auto, Granularity::SameTool, Granularity::Turn, Granularity::Call];

    pub fn label(self) -> &'static str {
        match self {
            Granularity::Auto => "自動",
            Granularity::SameTool => "同一ツール",
            Granularity::Turn => "ターン",
            Granularity::Call => "コール",
        }
    }

    /// メニューに添える説明
    pub fn description(self) -> &'static str {
        match self {
            Granularity::Auto => "拡大率に応じて、近いイベントをまとめる",
            Granularity::SameTool => "同じツールの連続した呼び出しをまとめる",
            Granularity::Turn => "ユーザーの発話から次の発話までをまとめる",
            Granularity::Call => "まとめずに 1 呼び出しずつ表示する",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub start: f64,
    pub end: f64,
    /// lane.events の範囲 (first..=last)
    pub first: usize,
    pub last: usize,
}

#[derive(Default)]
pub struct Cache {
    lanes: HashMap<(usize, usize), CachedLane>,
}

#[derive(Default)]
struct CachedLane {
    grouping: Option<Grouping>,
    names: Option<NameIndex>,
}

struct Grouping {
    spp: f64,
    gran: Granularity,
    blocks: Vec<Block>,
    /// ブロックの終了時刻の累積最大（二分探索用）
    ends: Vec<f64>,
    /// 各イベントまでの、所属ブロック内での終了時刻の最大
    prefix_end: Vec<f64>,
}

struct NameIndex {
    names: Vec<(String, Vec<usize>)>,
}

impl NameIndex {
    fn new(lane: &Lane) -> Self {
        let mut index = HashMap::new();
        let mut names: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, e) in lane.events.iter().enumerate() {
            let k = *index.entry(e.name.as_str()).or_insert_with(|| {
                names.push((e.name.clone(), Vec::new()));
                names.len() - 1
            });
            names[k].1.push(i);
        }
        Self { names }
    }

    fn describe(&self, b: &Block, show_label: bool) -> (usize, Option<String>) {
        let mut counts: Vec<_> = self
            .names
            .iter()
            .filter_map(|(name, indices)| {
                let lo = indices.partition_point(|&i| i < b.first);
                let hi = indices.partition_point(|&i| i <= b.last);
                (hi > lo).then(|| (name.as_str(), hi - lo, indices[lo]))
            })
            .collect();
        // 同数の名前はブロック内で初めて現れた順に並べる
        counts.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
        let text = show_label.then(|| {
            if counts.len() == 1 {
                format!("{} ×{}", counts[0].0, counts[0].1)
            } else {
                let parts: Vec<_> = counts
                    .iter()
                    .take(4)
                    .map(|(n, c, _)| format!("{n}×{c}"))
                    .collect();
                format!("{}件 · {}", b.last - b.first + 1, parts.join(" "))
            }
        });
        (counts[0].2, text)
    }
}

impl Cache {
    /// セッションが差し替えられたら、集約と名前の索引を一緒に捨てる
    pub fn invalidate(&mut self, touched: &[usize]) {
        let touched: HashSet<_> = touched.iter().copied().collect();
        self.lanes.retain(|&(si, _), _| !touched.contains(&si));
    }

    pub fn aggregate(
        &mut self,
        key: (usize, usize),
        lane: &Lane,
        view: (f64, f64),
        spp: f64,
        gran: Granularity,
    ) -> Vec<Block> {
        if lane.events.is_empty() {
            return Vec::new();
        }
        // コールとターンは表示範囲の近傍だけで求められる。初回に全体を集約する負荷を避ける
        if matches!(gran, Granularity::Call | Granularity::Turn) {
            return aggregate(lane, view, spp, gran);
        }
        let cached = self.lanes.entry(key).or_default();
        if cached
            .grouping
            .as_ref()
            .is_none_or(|g| g.spp != spp || g.gran != gran)
        {
            let blocks = aggregate(lane, (f64::NEG_INFINITY, f64::INFINITY), spp, gran);
            let mut ends = Vec::with_capacity(blocks.len());
            let mut prefix_end = vec![0.0; lane.events.len()];
            let mut end = f64::NEG_INFINITY;
            for b in &blocks {
                end = end.max(b.end);
                ends.push(end);
                let mut max = f64::NEG_INFINITY;
                for (e, value) in lane.events[b.first..=b.last]
                    .iter()
                    .zip(&mut prefix_end[b.first..=b.last])
                {
                    max = max.max(e.end);
                    *value = max;
                }
            }
            cached.grouping = Some(Grouping {
                spp,
                gran,
                blocks,
                ends,
                prefix_end,
            });
        }
        let g = cached.grouping.as_ref().unwrap();
        let hi = g.blocks.partition_point(|b| b.start <= view.1);
        let lo = g.ends.partition_point(|&end| end < view.0).min(hi);
        let event_hi = lane.events.partition_point(|e| e.start <= view.1);
        g.blocks[lo..hi]
            .iter()
            .filter_map(|b| {
                let last = b.last.min(event_hi - 1);
                let end = g.prefix_end[last];
                (end >= view.0).then_some(Block {
                    start: b.start,
                    end,
                    first: b.first,
                    last,
                })
            })
            .collect()
    }

    pub fn describe(
        &mut self,
        key: (usize, usize),
        lane: &Lane,
        b: &Block,
        show_label: bool,
    ) -> (usize, Option<String>) {
        if b.first == b.last {
            return describe(&lane.events, b, show_label);
        }
        self.lanes
            .entry(key)
            .or_default()
            .names
            .get_or_insert_with(|| NameIndex::new(lane))
            .describe(b, show_label)
    }
}

/// `spp` は 1px あたりの秒数
pub fn aggregate(lane: &Lane, view: (f64, f64), spp: f64, g: Granularity) -> Vec<Block> {
    let events = &lane.events;
    let hi = events.partition_point(|e| e.start <= view.1);
    // 表示範囲にかかる最初のイベントから、どのまとめ方でも結合されない切れ目まで遡る。
    // 先頭から走査しなくても、ブロックの区切りは全体を走査した場合と変わらない
    let mut lo = lane.cum_end.partition_point(|&t| t < view.0).min(hi);
    let thr = (40.0 * spp).max(120.0);
    let joins = |i: usize| match g {
        Granularity::Call => false,
        Granularity::Turn => events[i].kind != Kind::User,
        _ => events[i].start - lane.cum_end[i - 1] < thr,
    };
    while lo > 0 && lo < events.len() && joins(lo) {
        lo -= 1;
    }
    let mut out: Vec<Block> = Vec::new();
    let mut prev_name: Option<&str> = None;

    for (i, e) in events.iter().enumerate().take(hi).skip(lo) {
        let merge = match out.last() {
            None => false,
            Some(b) => {
                let gap = e.start - b.end;
                let same = prev_name == Some(e.name.as_str());
                match g {
                    Granularity::Call => false,
                    Granularity::SameTool => same && e.kind == Kind::Tool && gap < (120.0f64).max(30.0 * spp),
                    Granularity::Turn => e.kind != Kind::User,
                    Granularity::Auto => gap < 8.0 * spp || (same && gap < 40.0 * spp),
                }
            }
        };
        if merge {
            let b = out.last_mut().unwrap();
            b.end = b.end.max(e.end);
            b.last = i;
        } else {
            // 範囲外で確定したブロックは捨てる
            if out.last().is_some_and(|b| b.end < view.0) {
                out.pop();
            }
            out.push(Block { start: e.start, end: e.end, first: i, last: i });
        }
        prev_name = Some(e.name.as_str());
    }
    if out.last().is_some_and(|b| b.end < view.0) {
        out.pop();
    }
    out
}

/// ブロックのラベル
pub fn label(events: &[Event], b: &Block) -> String {
    let evs = &events[b.first..=b.last];
    if evs.len() == 1 {
        let e = &evs[0];
        return if e.summary.is_empty() { e.name.clone() } else { format!("{}: {}", e.name, e.summary) };
    }
    let counts = name_counts(evs);
    if counts.len() == 1 {
        return format!("{} ×{}", counts[0].0, counts[0].1);
    }
    let parts: Vec<String> = counts.iter().take(4).map(|(n, c)| format!("{n}×{c}")).collect();
    format!("{}件 · {}", evs.len(), parts.join(" "))
}

/// 最も多いイベント（色決めに使う）
pub fn dominant(events: &[Event], b: &Block) -> usize {
    if b.first == b.last {
        return b.first;
    }
    let evs = &events[b.first..=b.last];
    let counts = name_counts(evs);
    let top = counts[0].0;
    b.first + evs.iter().position(|e| e.name == top).unwrap_or(0)
}

/// 色とラベルに必要な名前の集計を一度だけ行う
pub fn describe(events: &[Event], b: &Block, show_label: bool) -> (usize, Option<String>) {
    if b.first == b.last {
        return (b.first, show_label.then(|| label(events, b)));
    }
    let evs = &events[b.first..=b.last];
    let counts = name_counts(evs);
    let dominant = b.first + evs.iter().position(|e| e.name == counts[0].0).unwrap_or(0);
    let text = show_label.then(|| {
        if counts.len() == 1 {
            format!("{} ×{}", counts[0].0, counts[0].1)
        } else {
            let parts: Vec<_> = counts
                .iter()
                .take(4)
                .map(|(n, c)| format!("{n}×{c}"))
                .collect();
            format!("{}件 · {}", evs.len(), parts.join(" "))
        }
    });
    (dominant, text)
}

fn name_counts(evs: &[Event]) -> Vec<(&str, usize)> {
    let mut m: HashMap<&str, usize> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for e in evs {
        let c = m.entry(e.name.as_str()).or_insert(0);
        if *c == 0 {
            order.push(e.name.as_str());
        }
        *c += 1;
    }
    let mut v: Vec<_> = order.into_iter().map(|n| (n, m[n])).collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.1));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> Lane {
        let mut lane = Lane::new("main");
        let mut t = 0.0;
        for i in 0..1500 {
            t += if i % 71 == 0 { 800.0 } else { (i % 17) as f64 };
            lane.events.push(Event {
                start: t,
                end: t + if i % 101 == 0 {
                    1500.0
                } else {
                    (i % 19) as f64
                },
                kind: if i % 83 == 0 { Kind::User } else { Kind::Tool },
                name: format!("tool-{}", i % 5),
                summary: "日本語の要約".into(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                row: None,
                usage: Default::default(),
                model: None,
                status: Default::default(),
            });
        }
        let mut end = f64::NEG_INFINITY;
        lane.cum_end = lane
            .events
            .iter()
            .map(|e| {
                end = end.max(e.end);
                end
            })
            .collect();
        lane
    }

    #[test]
    fn cached_groups_match_uncached_with_overlaps_and_view_clipping() {
        let lane = lane();
        let mut cache = Cache::default();
        for gran in Granularity::ALL {
            for spp in [0.01, 1.0, 15.0, 1000.0] {
                for lo in [-10.0, 0.0, 900.0, 2000.0, 7000.0, 16000.0, 40000.0] {
                    for width in [0.0, 10.0, 900.0, 10000.0] {
                        let view = (lo, lo + width);
                        assert_eq!(
                            cache.aggregate((0, 0), &lane, view, spp, gran),
                            aggregate(&lane, view, spp, gran),
                            "{gran:?}, spp={spp}, view={view:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cached_descriptions_keep_dominant_and_tie_order() {
        let lane = lane();
        let mut cache = Cache::default();
        for first in [0, 1, 31, 800, 1499] {
            for last in [first, (first + 3).min(1499), (first + 101).min(1499), 1499] {
                let b = Block {
                    start: 0.0,
                    end: 0.0,
                    first,
                    last,
                };
                for label_visible in [true, false] {
                    let expected = (
                        dominant(&lane.events, &b),
                        label_visible.then(|| label(&lane.events, &b)),
                    );
                    assert_eq!(describe(&lane.events, &b, label_visible), expected);
                    assert_eq!(cache.describe((0, 0), &lane, &b, label_visible), expected);
                }
            }
        }
    }

    #[test]
    fn replacing_data_invalidates_groups_and_names() {
        let mut lane = lane();
        let mut cache = Cache::default();
        let view = (0.0, 40000.0);
        let b = Block {
            start: 0.0,
            end: 0.0,
            first: 0,
            last: 100,
        };
        cache.aggregate((3, 0), &lane, view, 1.0, Granularity::Auto);
        cache.describe((3, 0), &lane, &b, true);
        for e in &mut lane.events {
            e.name = "changed".into();
        }
        cache.invalidate(&[9, 3, 1]);
        assert_eq!(
            cache.aggregate((3, 0), &lane, view, 1.0, Granularity::Auto),
            aggregate(&lane, view, 1.0, Granularity::Auto)
        );
        assert_eq!(
            cache.describe((3, 0), &lane, &b, true),
            describe(&lane.events, &b, true)
        );
    }
}
