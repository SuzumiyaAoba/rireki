//! セッションとレーンの構造、時刻索引、エージェントの親子関係。

use super::{Event, Kind, Source};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct Lane {
    pub name: String,
    /// サブエージェントを起動した親側のツール呼び出し ID
    pub spawned_by: Option<String>,
    /// 使用モデル（ログの assistant 行の message.model）
    pub model: String,
    /// 推論エフォート（ログの assistant 行の effort）
    pub effort: String,
    pub events: Vec<Event>,
    /// 読み込み元ファイル（差し替え単位）
    pub origin: String,
    /// events[..=i] の end の最大値。表示範囲の先頭を二分探索するのに使う
    pub cum_end: Vec<f64>,
    /// events[..=i] の Tool の end の最大値。Agent の長時間実行を起動候補に含めない
    pub cum_tool_end: Vec<f64>,
    /// 起動元ツール呼び出しの時刻（Session::finalize で確定）
    pub spawn_t: Option<f64>,
}

impl Lane {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            spawned_by: None,
            model: String::new(),
            effort: String::new(),
            events: Vec::new(),
            origin: String::new(),
            cum_end: Vec::new(),
            cum_tool_end: Vec::new(),
            spawn_t: None,
        }
    }

    #[inline]
    pub fn range(&self) -> Option<(f64, f64)> {
        Some((self.events.first()?.start, *self.cum_end.last()?))
    }

    pub fn finish(&mut self) {
        self.events.sort_by(|a, b| a.start.total_cmp(&b.start));
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub source: Source,
    pub cwd: String,
    /// `codex exec` / `claude -p` のように非対話で起動されたセッション
    pub headless: bool,
    pub title: String,
    /// Git のブランチ（最後に記録されたもの）
    pub branch: String,
    /// 記録した CLI のバージョン
    pub version: String,
    pub lanes: Vec<Lane>,
    pub start: f64,
    pub end: f64,
    /// 各レーンの親レーン（finalize で確定）
    pub parents: Vec<Option<usize>>,
    /// 親 → 子の深さ優先順に並べた (レーン, 深さ)。兄弟は開始時刻順（finalize で確定）
    pub tree: Vec<(usize, usize)>,
}

impl Session {
    /// レーン構成が変わった後に、描画で使う派生データを作り直す
    pub fn finalize(&mut self) {
        let (mut references, mut events) = (0, 0);
        for l in &mut self.lanes {
            references += usize::from(l.spawned_by.is_some());
            events += l.events.len();
            let (mut m, mut tool) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            l.cum_end.clear();
            l.cum_tool_end.clear();
            l.cum_end.reserve(l.events.len());
            l.cum_tool_end.reserve(l.events.len());
            for e in &l.events {
                m = m.max(e.end);
                if e.kind == Kind::Tool {
                    tool = tool.max(e.end);
                }
                l.cum_end.push(m);
                l.cum_tool_end.push(tool);
            }
        }
        (self.start, self.end) = self.lanes.iter().filter_map(Lane::range).fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(a, b), (start, end)| (a.min(start), b.max(end)),
        );
        // 通常は少数のレーンが参照する起動元だけを探す。参照が密な入力は
        // 一度の辞書挿入で済む全件索引を使い、二つの表の照合費用を避ける。
        let mut owner = HashMap::new();
        if references > events / 4 {
            for (li, lane) in self.lanes.iter().enumerate() {
                for event in &lane.events {
                    if let Some(id) = event.call_id.as_deref() {
                        owner.entry(id).or_insert((li, event.start));
                    }
                }
            }
        } else if references != 0 {
            let mut needed: HashSet<&str> = self
                .lanes
                .iter()
                .filter_map(|lane| lane.spawned_by.as_deref())
                .collect();
            owner.reserve(needed.len());
            // レーン順・イベント順で最初に見つかった ID だけを採用する。
            'lanes: for (li, lane) in self.lanes.iter().enumerate() {
                for event in &lane.events {
                    if let Some(id) = event.call_id.as_deref()
                        && needed.remove(id)
                    {
                        owner.insert(id, (li, event.start));
                        if needed.is_empty() {
                            break 'lanes;
                        }
                    }
                }
            }
        }
        self.parents = self.lane_parents(&owner);
        self.tree = self.lane_tree();
        let spawn: Vec<Option<f64>> = self
            .lanes
            .iter()
            .enumerate()
            .map(|(li, l)| {
                let id = l.spawned_by.as_deref()?;
                let p = self.parents.get(li).copied().flatten().unwrap_or(0);
                owner.get(id).filter(|&&(li, _)| li == p).map(|&(_, t)| t)
            })
            .collect();
        for (l, t) in self.lanes.iter_mut().zip(spawn) {
            l.spawn_t = t;
        }
    }

    fn lane_tree(&self) -> Vec<(usize, usize)> {
        let n = self.lanes.len();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (li, p) in self.parents.iter().enumerate() {
            if let Some(p) = *p {
                children[p].push(li);
            }
        }
        let t = |li: usize| {
            self.lanes[li]
                .events
                .first()
                .map_or(f64::INFINITY, |e| e.start)
        };
        for ch in &mut children {
            ch.sort_by(|&a, &b| t(a).total_cmp(&t(b)));
        }
        let mut out = Vec::with_capacity(n);
        let mut stack = vec![(0usize, 0usize)];
        let mut seen = vec![false; n];
        while let Some((li, depth)) = stack.pop() {
            if li >= n || std::mem::replace(&mut seen[li], true) {
                continue;
            }
            out.push((li, depth));
            stack.extend(children[li].iter().rev().map(|&c| (c, depth + 1)));
        }
        out
    }

    /// 各レーンの親レーン。起動元のツール呼び出しを含むレーンを親とし、見つからなければ main
    fn lane_parents(&self, owner: &HashMap<&str, (usize, f64)>) -> Vec<Option<usize>> {
        (0..self.lanes.len())
            .map(|li| {
                if li == 0 {
                    return None;
                }
                let p = self.lanes[li]
                    .spawned_by
                    .as_deref()
                    .and_then(|id| owner.get(id).map(|&(li, _)| li));
                Some(p.filter(|&p| p != li).unwrap_or(0))
            })
            .collect()
    }

    /// 指定したツール呼び出しで起動されたレーンを探す
    pub fn lane_spawned_by(&self, call_id: &str) -> Option<usize> {
        self.lanes
            .iter()
            .position(|l| l.spawned_by.as_deref() == Some(call_id))
    }
}
