use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Claude => "Claude",
            Source::Codex => "Codex",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    User,
    Assistant,
    Thinking,
    Tool,
    Agent,
}

#[derive(Clone, Debug)]
pub struct Event {
    /// Unix 秒
    pub start: f64,
    pub end: f64,
    pub kind: Kind,
    /// ツール名、または "User" / "Assistant" など
    pub name: String,
    pub summary: String,
    pub input: String,
    pub output: String,
    /// ツール呼び出し ID（サブエージェントとの紐付けに使う）
    pub call_id: Option<String>,
    /// 索引 DB 上の行 ID。DB から読んだイベントは input / output が空で、詳細表示時に取り出す
    pub row: Option<i64>,
}

impl Event {
    /// input / output を除いた複製
    pub fn clone_meta(&self) -> Self {
        Self {
            start: self.start,
            end: self.end,
            kind: self.kind,
            name: self.name.clone(),
            summary: self.summary.clone(),
            input: String::new(),
            output: String::new(),
            call_id: self.call_id.clone(),
            row: self.row,
        }
    }
}

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
            spawn_t: None,
        }
    }

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
        for l in &mut self.lanes {
            let mut m = f64::NEG_INFINITY;
            l.cum_end = l
                .events
                .iter()
                .map(|e| {
                    m = m.max(e.end);
                    m
                })
                .collect();
        }
        let ranges: Vec<_> = self.lanes.iter().filter_map(|l| l.range()).collect();
        self.start = ranges.iter().map(|r| r.0).fold(f64::INFINITY, f64::min);
        self.end = ranges.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
        self.parents = self.lane_parents();
        self.tree = self.lane_tree();
        let spawn: Vec<Option<f64>> = self
            .lanes
            .iter()
            .enumerate()
            .map(|(li, l)| {
                let id = l.spawned_by.as_deref()?;
                let p = self.parents.get(li).copied().flatten().unwrap_or(0);
                self.lanes.get(p)?.events.iter().find(|e| e.call_id.as_deref() == Some(id)).map(|e| e.start)
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
        let t = |li: usize| self.lanes[li].events.first().map_or(f64::INFINITY, |e| e.start);
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
    fn lane_parents(&self) -> Vec<Option<usize>> {
        let mut owner: HashMap<&str, usize> = HashMap::new();
        for (li, l) in self.lanes.iter().enumerate() {
            for e in &l.events {
                if let Some(id) = e.call_id.as_deref() {
                    owner.entry(id).or_insert(li);
                }
            }
        }
        (0..self.lanes.len())
            .map(|li| {
                if li == 0 {
                    return None;
                }
                let p = self.lanes[li].spawned_by.as_deref().and_then(|id| owner.get(id).copied());
                Some(p.filter(|&p| p != li).unwrap_or(0))
            })
            .collect()
    }

    /// 指定したツール呼び出しで起動されたレーンを探す
    pub fn lane_spawned_by(&self, call_id: &str) -> Option<usize> {
        self.lanes.iter().position(|l| l.spawned_by.as_deref() == Some(call_id))
    }
}

/// 読み込み結果。サブエージェントのセッションは親が見つかった時点で親に合流する。
/// レーンは読み込み元ファイル (origin) 単位で差し替えられる
#[derive(Default)]
pub struct Store {
    pub sessions: Vec<Session>,
    index: HashMap<String, usize>,
    orphans: HashMap<String, Vec<Lane>>,
    /// origin → そのファイル由来のレーンを持つセッション
    origins: HashMap<String, Vec<usize>>,
}

pub enum Loaded {
    Session(Session),
    /// 親セッション ID と、そこに追加するレーン
    Child(String, Lane),
}

impl Store {
    /// ファイル (origin) ごとに、そのファイル由来のデータを items で置き換える。
    /// 派生データの再計算はセッションごとに最後に一度だけ行う。変更のあったセッションの index を返す
    pub fn replace(&mut self, updates: impl IntoIterator<Item = (String, Vec<Loaded>)>) -> Vec<usize> {
        let mut touched = Vec::new();
        for (origin, items) in updates {
            if let Some(old) = self.origins.remove(&origin) {
                for &i in &old {
                    self.sessions[i].lanes.retain(|l| l.origin != origin);
                }
                touched.extend(old);
            }
            if !self.orphans.is_empty() {
                for lanes in self.orphans.values_mut() {
                    lanes.retain(|l| l.origin != origin);
                }
            }
            let mut mine: Vec<usize> = items.into_iter().filter_map(|item| self.add(&origin, item)).collect();
            mine.sort_unstable();
            mine.dedup();
            touched.extend(&mine);
            if !mine.is_empty() {
                self.origins.insert(origin, mine);
            }
        }
        touched.sort_unstable();
        touched.dedup();
        for &i in &touched {
            self.sessions[i].finalize();
        }
        touched
    }

    fn add(&mut self, origin: &str, item: Loaded) -> Option<usize> {
        match item {
            Loaded::Session(mut s) => {
                for l in &mut s.lanes {
                    l.origin = origin.to_string();
                }
                if let Some(&i) = self.index.get(&s.id) {
                    // main とその配下を先頭に、他ファイル由来の子レーンを後ろに置く
                    let ex = &mut self.sessions[i];
                    s.lanes.append(&mut ex.lanes);
                    ex.lanes = s.lanes;
                    ex.title = s.title;
                    ex.cwd = s.cwd;
                    ex.source = s.source;
                    return Some(i);
                }
                if let Some(lanes) = self.orphans.remove(&s.id) {
                    for l in &lanes {
                        self.origins.entry(l.origin.clone()).or_default().push(self.sessions.len());
                    }
                    s.lanes.extend(lanes);
                }
                let i = self.sessions.len();
                self.index.insert(s.id.clone(), i);
                self.sessions.push(s);
                Some(i)
            }
            Loaded::Child(parent, mut lane) => {
                if lane.events.is_empty() {
                    return None;
                }
                lane.origin = origin.to_string();
                if let Some(&i) = self.index.get(&parent) {
                    self.sessions[i].lanes.push(lane);
                    Some(i)
                } else {
                    self.orphans.entry(parent).or_default().push(lane);
                    None
                }
            }
        }
    }

    /// 表示対象にできるセッションか
    pub fn visible(&self, i: usize) -> bool {
        let s = &self.sessions[i];
        !s.lanes.is_empty() && s.start.is_finite()
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

pub fn one_line(s: &str, max: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&joined, max)
}
