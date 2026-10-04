//! 表示スケールに応じてイベントをブロックにまとめる

use std::collections::HashMap;

use crate::model::{Event, Kind, Lane};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
}

#[derive(Clone, Debug)]
pub struct Block {
    pub start: f64,
    pub end: f64,
    /// lane.events の範囲 (first..=last)
    pub first: usize,
    pub last: usize,
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
    let evs = &events[b.first..=b.last];
    let counts = name_counts(evs);
    let top = counts[0].0;
    b.first + evs.iter().position(|e| e.name == top).unwrap_or(0)
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
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}
