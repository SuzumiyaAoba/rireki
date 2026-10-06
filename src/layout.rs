//! 表示スケールに応じてイベントをブロックにまとめる

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::{Event, Kind, Lane};

pub mod directory;

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
    full_history: bool,
    exact_scales: bool,
}

#[derive(Default)]
struct CachedLane {
    grouping: Option<Grouping>,
    /// サブエージェント呼び出しを除いた、元のイベント列の連続範囲
    main_ranges: Option<Vec<std::ops::Range<usize>>>,
    main_grouping: Option<Grouping>,
    names: Option<NameIndex>,
}

struct Grouping {
    spp: f64,
    gran: Granularity,
    view: (f64, f64),
    blocks: Vec<Block>,
    /// ブロックの終了時刻の累積最大（二分探索用）
    ends: Vec<f64>,
    /// 各イベントまでの、所属ブロック内での終了時刻の最大
    prefix_end: Vec<f64>,
    prefix_start: usize,
}

impl Grouping {
    fn new(lane: &Lane, blocks: Vec<Block>, spp: f64, gran: Granularity, view: (f64, f64)) -> Self {
        let mut ends = Vec::with_capacity(blocks.len());
        let prefix_start = blocks.first().map_or(0, |b| b.first);
        let count = blocks.last().map_or(0, |b| b.last + 1 - prefix_start);
        let mut prefix_end = vec![0.0; count];
        let mut end = f64::NEG_INFINITY;
        for b in &blocks {
            end = end.max(b.end);
            ends.push(end);
            let mut max = f64::NEG_INFINITY;
            for (e, value) in lane.events[b.first..=b.last]
                .iter()
                .zip(&mut prefix_end[b.first - prefix_start..=b.last - prefix_start])
            {
                max = max.max(e.end);
                *value = max;
            }
        }
        Self { spp, gran, view, blocks, ends, prefix_end, prefix_start }
    }

    fn covers(&self, view: (f64, f64)) -> bool {
        self.view.0 <= view.0 && view.1 <= self.view.1
    }

    fn matches_scale(&self, spp: f64, exact: bool) -> bool {
        match self.gran {
            Granularity::Call | Granularity::Turn => true,
            Granularity::SameTool if !exact => 120.0f64.max(30.0 * self.spp) == 120.0f64.max(30.0 * spp),
            _ => self.spp == spp,
        }
    }

    fn range(view: (f64, f64)) -> (f64, f64) {
        let width = view.1 - view.0;
        if view.0.is_finite() && view.1.is_finite() && width.is_finite() && width >= 0.0 {
            // 横移動の前後一画面分も保持し、小さな移動では集約を作り直さない。
            (view.0 - width, view.1 + width)
        } else {
            view
        }
    }

    fn visible(&self, lane: &Lane, view: (f64, f64)) -> Vec<Block> {
        let hi = self.blocks.partition_point(|b| b.start <= view.1);
        let lo = self.ends.partition_point(|&end| end < view.0).min(hi);
        let event_hi = lane.events.partition_point(|e| e.start <= view.1);
        self.blocks[lo..hi]
            .iter()
            .filter_map(|b| {
                let last = b.last.min(event_hi - 1);
                let end = self.prefix_end[last - self.prefix_start];
                (end >= view.0).then_some(Block { start: b.start, end, first: b.first, last })
            })
            .collect()
    }
}

struct NameIndex {
    names: Vec<(String, Vec<usize>)>,
}

impl NameIndex {
    fn new(lane: &Lane) -> Self {
        let mut index = HashMap::new();
        let mut names: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, e) in lane.events.iter().enumerate() {
            let k = *index.entry(e.name.as_ref()).or_insert_with(|| {
                names.push((e.name.to_string(), Vec::new()));
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
    /// 比較計測用。通常は Default で表示範囲の近傍だけを保持する。
    pub fn with_full_history() -> Self {
        Self { full_history: true, exact_scales: true, ..Self::default() }
    }

    /// 比較計測用。近傍の保持を維持し、同一ツールでも縮尺が変わるたびに集約を作り直す。
    pub fn with_exact_scales() -> Self {
        Self { exact_scales: true, ..Self::default() }
    }

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
            .is_none_or(|g| g.gran != gran || !g.matches_scale(spp, self.exact_scales) || !g.covers(view))
        {
            let range = if self.full_history { (f64::NEG_INFINITY, f64::INFINITY) } else { Grouping::range(view) };
            let blocks = aggregate(lane, range, spp, gran);
            cached.grouping = Some(Grouping::new(lane, blocks, spp, gran, range));
        }
        cached.grouping.as_ref().unwrap().visible(lane, view)
    }

    /// ディレクトリ表示用。Agent を集約前に除外し、選択範囲は元のイベント位置を保つ。
    pub fn aggregate_main(
        &mut self,
        session: usize,
        lane: &Lane,
        view: (f64, f64),
        spp: f64,
        gran: Granularity,
    ) -> Vec<Block> {
        let cached = self.lanes.entry((session, 0)).or_default();
        let ranges = cached.main_ranges.get_or_insert_with(|| {
            let mut ranges = Vec::new();
            let mut first = 0;
            for (i, e) in lane.events.iter().enumerate() {
                // コードモードの exec 経由では、表示名が spawn_agent でも Kind::Tool になる。
                if e.kind == Kind::Agent
                    || (e.kind == Kind::Tool && matches!(e.name.as_ref(), "Agent" | "Task" | "spawn_agent"))
                {
                    if first < i {
                        ranges.push(first..i);
                    }
                    first = i + 1;
                }
            }
            if first < lane.events.len() {
                ranges.push(first..lane.events.len());
            }
            ranges
        });
        if cached.main_grouping.as_ref().is_none_or(|g| {
            g.gran != gran || !g.matches_scale(spp, self.exact_scales)
                || !g.covers(view)
        }) {
            let range_view = if self.full_history { (f64::NEG_INFINITY, f64::INFINITY) } else { Grouping::range(view) };
            let mut blocks = Vec::new();
            for range in ranges {
                let grouped = aggregate_events(
                    &lane.events[range.clone()],
                    &lane.cum_end[range.clone()],
                    range_view,
                    spp,
                    gran,
                );
                blocks.extend(grouped.into_iter().map(|mut b| {
                    b.first += range.start;
                    b.last += range.start;
                    b
                }));
            }
            cached.main_grouping = Some(Grouping::new(lane, blocks, spp, gran, range_view));
        }
        cached.main_grouping.as_ref().unwrap().visible(lane, view)
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
    aggregate_events(&lane.events, &lane.cum_end, view, spp, g)
}

fn aggregate_events(events: &[Event], cum_end: &[f64], view: (f64, f64), spp: f64, g: Granularity) -> Vec<Block> {
    let hi = events.partition_point(|e| e.start <= view.1);
    // 表示範囲にかかる最初のイベントから、どのまとめ方でも結合されない切れ目まで遡る。
    // 先頭から走査しなくても、ブロックの区切りは全体を走査した場合と変わらない
    let mut lo = cum_end.partition_point(|&t| t < view.0).min(hi);
    let joins = |i: usize| match g {
        Granularity::Call => false,
        Granularity::Turn => events[i].kind != Kind::User,
        Granularity::SameTool => events[i].kind == Kind::Tool
            && events[i].name == events[i - 1].name
            && events[i].start - cum_end[i - 1] < 120.0f64.max(30.0 * spp),
        Granularity::Auto => events[i].start - cum_end[i - 1]
            < spp * if events[i].name == events[i - 1].name { 40.0 } else { 8.0 },
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
                let same = prev_name == Some(e.name.as_ref());
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
        prev_name = Some(e.name.as_ref());
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
        return if e.summary.is_empty() { e.name.to_string() } else { format!("{}: {}", e.name, e.summary) };
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
    b.first + evs.iter().position(|e| e.name.as_ref() == top).unwrap_or(0)
}

/// 色とラベルに必要な名前の集計を一度だけ行う
pub fn describe(events: &[Event], b: &Block, show_label: bool) -> (usize, Option<String>) {
    if b.first == b.last {
        return (b.first, show_label.then(|| label(events, b)));
    }
    let evs = &events[b.first..=b.last];
    let counts = name_counts(evs);
    let dominant = b.first + evs.iter().position(|e| e.name.as_ref() == counts[0].0).unwrap_or(0);
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
        let c = m.entry(e.name.as_ref()).or_insert(0);
        if *c == 0 {
            order.push(e.name.as_ref());
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

    // 全履歴の区切りから、表示上端より前に始まるブロックと右端の途中切り取りを求める。
    fn full_reference(lane: &Lane, view: (f64, f64), spp: f64, gran: Granularity) -> Vec<Block> {
        let hi = lane.events.partition_point(|e| e.start <= view.1);
        aggregate(lane, (f64::NEG_INFINITY, f64::INFINITY), spp, gran).into_iter().filter_map(|mut block| {
            if block.first >= hi { return None; }
            block.last = block.last.min(hi - 1);
            block.end = lane.events[block.first..=block.last].iter().map(|e| e.end).fold(f64::NEG_INFINITY, f64::max);
            (block.end >= view.0).then_some(block)
        }).collect()
    }

    fn sparse_lane(count: usize) -> Lane {
        let mut result = Lane::new("main");
        let prototype = lane().events[1].clone();
        let names: [std::sync::Arc<str>; 2] = ["even".into(), "odd".into()];
        result.events = (0..count).map(|i| {
            let mut event = prototype.clone();
            event.start = i as f64;
            event.end = event.start + 0.1;
            event.kind = Kind::Tool;
            event.name = names[i % 2].clone();
            event
        }).collect();
        result.cum_end = result.events.iter().map(|e| e.end).collect();
        result
    }

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
                name: format!("tool-{}", i % 5).into(),
                summary: "日本語の要約".into(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: Default::default(),
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
                            full_reference(&lane, view, spp, gran),
                            "{gran:?}, spp={spp}, view={view:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_small_view_retains_nearby_events_and_reuses_small_pans() {
        let lane = sparse_lane(20000);
        let mut cache = Cache::default();
        let key = (0, 0);
        let view = (10000.0, 10010.0);
        assert_eq!(cache.aggregate(key, &lane, view, 0.01, Granularity::Auto), full_reference(&lane, view, 0.01, Granularity::Auto));
        let first = cache.lanes[&key].grouping.as_ref().unwrap();
        assert!(first.prefix_start >= 9990 && first.prefix_end.len() <= 32);
        let storage = first.prefix_end.as_ptr();
        let pan = (10003.0, 10013.0);
        assert_eq!(cache.aggregate(key, &lane, pan, 0.01, Granularity::Auto), full_reference(&lane, pan, 0.01, Granularity::Auto));
        assert_eq!(storage, cache.lanes[&key].grouping.as_ref().unwrap().prefix_end.as_ptr());
        let far = (15000.0, 15010.0);
        assert_eq!(cache.aggregate(key, &lane, far, 0.01, Granularity::Auto), full_reference(&lane, far, 0.01, Granularity::Auto));
        let moved = cache.lanes[&key].grouping.as_ref().unwrap();
        assert!(moved.prefix_start >= 14990 && moved.prefix_end.len() <= 32);
        let all = (f64::NEG_INFINITY, f64::INFINITY);
        assert_eq!(cache.aggregate(key, &lane, all, 0.01, Granularity::Auto), full_reference(&lane, all, 0.01, Granularity::Auto));
        assert_eq!(cache.lanes[&key].grouping.as_ref().unwrap().prefix_end.len(), lane.events.len());
    }

    #[test]
    fn same_tool_zoom_reuses_a_constant_threshold_and_rebuilds_above_its_boundary() {
        let mut lane = sparse_lane(4);
        for (event, time) in lane.events.iter_mut().zip([0.0, 120.0, 241.0, 242.0]) {
            event.start = time; event.end = time; event.name = "tool".into();
        }
        lane.events[3].kind = Kind::User;
        lane.cum_end = lane.events.iter().map(|e| e.end).collect();
        let view = (-10.0, 400.0);
        let gran = Granularity::SameTool;
        for main in [false, true] {
            let mut cache = Cache::default();
            let aggregate = |cache: &mut Cache, spp| if main {
                cache.aggregate_main(0, &lane, view, spp, gran)
            } else { cache.aggregate((0, 0), &lane, view, spp, gran) };
            let storage = |cache: &Cache| {
                let lane = &cache.lanes[&(0, 0)];
                let group = if main { &lane.main_grouping } else { &lane.grouping };
                group.as_ref().unwrap().prefix_end.as_ptr()
            };
            assert_eq!(aggregate(&mut cache, 1.0), full_reference(&lane, view, 1.0, gran));
            let first = storage(&cache);
            for spp in [0.1, 2.0, 4.0] {
                assert_eq!(aggregate(&mut cache, spp), full_reference(&lane, view, spp, gran));
                assert_eq!(storage(&cache), first);
            }
            let above = f64::from_bits(4.0f64.to_bits() + 1);
            let changed = aggregate(&mut cache, above);
            assert_eq!(changed, full_reference(&lane, view, above, gran));
            assert_eq!((changed[0].first, changed[0].last), (0, 1));
            assert_ne!(storage(&cache), first);
            assert_eq!(aggregate(&mut cache, 4.0), full_reference(&lane, view, 4.0, gran));
        }
    }

    #[test]
    fn exact_scale_control_rebuilds_while_preserving_the_same_blocks() {
        let lane = sparse_lane(1000);
        let view = (100.0, 110.0);
        let mut cache = Cache::with_exact_scales();
        for gran in [Granularity::Auto, Granularity::SameTool] {
            let first = cache.aggregate((0, 0), &lane, view, 0.01, gran);
            let storage = cache.lanes[&(0, 0)].grouping.as_ref().unwrap().prefix_end.as_ptr();
            let next = cache.aggregate((0, 0), &lane, view, 0.02, gran);
            assert_eq!(next, first);
            assert_ne!(storage, cache.lanes[&(0, 0)].grouping.as_ref().unwrap().prefix_end.as_ptr());
        }
    }

    #[test]
    fn main_groups_match_full_history_with_agent_gaps_pan_and_zoom() {
        let mut lane = lane();
        for (i, event) in lane.events.iter_mut().enumerate() {
            if i.is_multiple_of(23) { event.kind = Kind::Agent; }
            if i.is_multiple_of(27) { event.kind = Kind::Tool; event.name = "spawn_agent".into(); }
        }
        let keep = |event: &Event| event.kind != Kind::Agent
            && !(event.kind == Kind::Tool && matches!(event.name.as_ref(), "Agent" | "Task" | "spawn_agent"));
        let mut cache = Cache::default();
        for gran in Granularity::ALL {
            for spp in [0.01, 0.1, 1.0, 20.0, 1000.0] {
                for lo in [-100.0, 0.0, 2000.0, 2020.0, 2050.0, 16000.0, 100000.0] {
                    for width in [0.0, 1.0, 20.0, 2000.0] {
                        let view = (lo, lo + width);
                        let mut expected = Vec::new();
                        let mut first = 0;
                        while first < lane.events.len() {
                            if !keep(&lane.events[first]) { first += 1; continue; }
                            let last = first + lane.events[first..].iter().take_while(|e| keep(e)).count();
                            let part = Lane { events: lane.events[first..last].to_vec(), cum_end: lane.cum_end[first..last].to_vec(), ..Lane::new("main") };
                            expected.extend(full_reference(&part, view, spp, gran).into_iter().map(|mut block| {
                                block.first += first; block.last += first; block
                            }));
                            first = last;
                        }
                        assert_eq!(cache.aggregate_main(0, &lane, view, spp, gran), expected, "{gran:?}, {spp}, {view:?}");
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
