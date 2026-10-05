use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    #[inline]
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

/// ツール呼び出しの結果。メッセージは常に Ok
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Status {
    #[default]
    Ok,
    /// 失敗（エラーの結果、0 以外の終了コード）
    Error,
    /// ユーザーが実行を拒否した
    Rejected,
    /// ユーザーが中断した
    Interrupted,
    /// 結果がない（実行中、または結果を記録せずに終わった）
    Pending,
}

impl Status {
    /// 失敗の扱い（エラー・拒否・中断）か
    #[inline]
    pub fn failed(self) -> bool {
        matches!(self, Status::Error | Status::Rejected | Status::Interrupted)
    }
}

/// トークン使用量。input はキャッシュを除いた入力、reasoning は output のうち推論の分
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Usage {
    pub input: u32,
    pub cache_read: u32,
    pub cache_write: u32,
    /// cache_write のうち 1 時間キャッシュの分
    pub cache_write_1h: u32,
    pub output: u32,
    pub reasoning: u32,
}

impl Usage {
    #[inline]
    pub fn is_empty(&self) -> bool {
        *self == Usage::default()
    }

    /// 入力（キャッシュ含む）と出力の合計
    #[inline]
    pub fn total(&self) -> u64 {
        self.input as u64 + self.cache_read as u64 + self.cache_write as u64 + self.output as u64
    }
}

/// 集計用。u32 の和はあふれうるので u64 で持つ
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct UsageSum {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
    /// 使用量の付いたイベント（API 呼び出し）の数
    pub calls: u64,
    /// 料金（USD）。料金の分からないモデルの分は含まない
    pub cost: f64,
    /// 料金の分からないモデルの呼び出し数
    pub unpriced: u64,
}

impl UsageSum {
    // ライブラリの外で走査する場合も、イベントごとの集計をインライン化できるようにする。
    #[inline]
    pub fn add(&mut self, u: &Usage, model: Option<&str>) {
        if u.is_empty() {
            return;
        }
        self.add_priced(u, model.and_then(crate::pricing::price));
    }

    /// 同じモデルの料金を使い回す集計用。空の使用量は呼び出し数に含めない。
    #[inline]
    pub fn add_priced(&mut self, u: &Usage, price: Option<crate::pricing::Price>) {
        if u.is_empty() {
            return;
        }
        match price {
            Some(p) => self.cost += p.cost(u),
            None => self.unpriced += 1,
        }
        self.input += u.input as u64;
        self.cache_read += u.cache_read as u64;
        self.cache_write += u.cache_write as u64;
        self.output += u.output as u64;
        self.reasoning += u.reasoning as u64;
        self.calls += 1;
    }

    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.calls += other.calls;
        self.cost += other.cost;
        self.unpriced += other.unpriced;
    }

    pub fn of<'a>(events: impl IntoIterator<Item = &'a Event>) -> Self {
        let mut s = Self::default();
        for e in events {
            s.add(&e.usage, e.model.as_deref());
        }
        s
    }

    /// モデルごとの集計（合計の多い順）
    pub fn by_model<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<(String, Self)> {
        let mut out: Vec<(String, Self)> = Vec::new();
        for e in events {
            if e.usage.is_empty() {
                continue;
            }
            let m = e.model.as_deref().unwrap_or("");
            let i = match out.iter().position(|(k, _)| k == m) {
                Some(i) => i,
                None => {
                    out.push((m.to_string(), Self::default()));
                    out.len() - 1
                }
            };
            out[i].1.add(&e.usage, e.model.as_deref());
        }
        out.sort_by_key(|(_, u)| std::cmp::Reverse(u.total()));
        out
    }

    #[inline]
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output
    }
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
    /// この時点の API 呼び出しのトークン使用量（なければ空）
    pub usage: Usage,
    /// 使用量を計上したモデル（使用量のあるイベントだけ）
    pub model: Option<Arc<str>>,
    pub status: Status,
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
            usage: self.usage,
            model: self.model.clone(),
            status: self.status,
        }
    }
}

/// イベント列の集計（詳細パネルの概要に出す）。トークン数は `UsageSum` で数える
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Stats {
    /// ユーザーの発話（ターン）の数
    pub turns: usize,
    /// ツール呼び出し（サブエージェントの起動を含む）の数
    pub tools: usize,
    pub errors: usize,
    pub rejected: usize,
    pub interrupted: usize,
    /// 1 回の API 呼び出しの入力（キャッシュ含む）の最大。コンテキストの大きさの最大
    pub peak: u64,
}

impl Stats {
    pub fn of<'a>(events: impl IntoIterator<Item = &'a Event>) -> Self {
        let mut s = Stats::default();
        for e in events {
            s.add(e);
        }
        s
    }

    #[inline]
    pub fn add(&mut self, e: &Event) {
        match e.kind {
            Kind::User => self.turns += 1,
            Kind::Tool | Kind::Agent => self.tools += 1,
            _ => {}
        }
        match e.status {
            Status::Error => self.errors += 1,
            Status::Rejected => self.rejected += 1,
            Status::Interrupted => self.interrupted += 1,
            _ => {}
        }
        let u = &e.usage;
        self.peak = self.peak.max(u.input as u64 + u.cache_read as u64 + u.cache_write as u64);
    }

    /// 失敗（エラー・拒否・中断）の数
    #[inline]
    pub fn failures(&self) -> usize {
        self.errors + self.rejected + self.interrupted
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
        for l in &mut self.lanes {
            let (mut m, mut tool) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            l.cum_end.clear();
            l.cum_tool_end.clear();
            l.cum_end.reserve(l.events.len());
            l.cum_tool_end.reserve(l.events.len());
            for e in &l.events {
                m = m.max(e.end);
                if e.kind == Kind::Tool { tool = tool.max(e.end); }
                l.cum_end.push(m);
                l.cum_tool_end.push(tool);
            }
        }
        (self.start, self.end) = self.lanes.iter().filter_map(Lane::range).fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(a, b), (start, end)| (a.min(start), b.max(end)),
        );
        // 起動元のレーンと時刻を一度の走査で索引化する。子ごとに親の全イベントを探し直さない
        let mut owner = HashMap::new();
        for (li, lane) in self.lanes.iter().enumerate() {
            for e in &lane.events {
                if let Some(id) = e.call_id.as_deref() {
                    owner.entry(id).or_insert((li, e.start));
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
        self.lanes.iter().position(|l| l.spawned_by.as_deref() == Some(call_id))
    }
}

/// 読み込み結果。サブエージェントのセッションは親が見つかった時点で親に合流する。
/// レーンは読み込み元ファイル (origin) 単位で差し替えられる
#[derive(Default)]
pub struct Store {
    pub sessions: Vec<Arc<Session>>,
    index: HashMap<String, usize>,
    orphans: HashMap<String, Vec<Lane>>,
    /// origin → そのファイル由来のレーンを持つセッション
    origins: HashMap<String, Vec<usize>>,
}

/// 描画と検索が共有する読み取り専用の版。イベント本文を複製せずに背景へ渡せる
#[derive(Clone, Default)]
pub struct Snapshot {
    pub sessions: Vec<Arc<Session>>,
}

impl Snapshot {
    pub fn visible(&self, i: usize) -> bool {
        let s = &self.sessions[i];
        !s.lanes.is_empty() && s.start.is_finite()
    }
}

pub enum Loaded {
    Session(Session),
    /// 親セッション ID と、そこに追加するレーン
    Child(String, Lane),
}

impl Store {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot { sessions: self.sessions.clone() }
    }

    /// ファイル (origin) ごとに、そのファイル由来のデータを items で置き換える。
    /// 派生データの再計算はセッションごとに最後に一度だけ行う。変更のあったセッションの index を返す
    pub fn replace(&mut self, updates: impl IntoIterator<Item = (String, Vec<Loaded>)>) -> Vec<usize> {
        let mut touched = Vec::new();
        for (origin, items) in updates {
            if let Some(old) = self.origins.remove(&origin) {
                for &i in &old {
                    if let Some(s) = Arc::get_mut(&mut self.sessions[i]) {
                        s.lanes.retain(|l| l.origin != origin);
                    } else {
                        // 差し替える本文は複製しない。旧版は描画・検索が読み終わるまで残す
                        let s = &self.sessions[i];
                        self.sessions[i] = Arc::new(Session {
                            id: s.id.clone(), source: s.source, cwd: s.cwd.clone(), headless: s.headless,
                            title: s.title.clone(), branch: s.branch.clone(), version: s.version.clone(),
                            lanes: s.lanes.iter().filter(|l| l.origin != origin).cloned().collect(),
                            start: s.start, end: s.end, parents: Vec::new(), tree: Vec::new(),
                        });
                    }
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
            Arc::make_mut(&mut self.sessions[i]).finalize();
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
                    let ex = Arc::make_mut(&mut self.sessions[i]);
                    s.lanes.append(&mut ex.lanes);
                    ex.lanes = s.lanes;
                    ex.title = s.title;
                    ex.cwd = s.cwd;
                    ex.source = s.source;
                    ex.headless = s.headless;
                    ex.branch = s.branch;
                    ex.version = s.version;
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
                self.sessions.push(Arc::new(s));
                Some(i)
            }
            Loaded::Child(parent, mut lane) => {
                if lane.events.is_empty() {
                    return None;
                }
                lane.origin = origin.to_string();
                if let Some(&i) = self.index.get(&parent) {
                    Arc::make_mut(&mut self.sessions[i]).lanes.push(lane);
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
    let Some((end, _)) = s.char_indices().nth(max) else {
        return s.to_string();
    };
    let mut out = String::with_capacity(end + '…'.len_utf8());
    out.push_str(&s[..end]);
    out.push('…');
    out
}

pub fn one_line(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max.saturating_mul(4).saturating_add(3)));
    let mut count = 0;
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space |= !out.is_empty();
            continue;
        }
        if space {
            if count == max {
                out.push('…');
                return out;
            }
            out.push(' ');
            count += 1;
            space = false;
        }
        if count == max {
            out.push('…');
            return out;
        }
        out.push(c);
        count += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_end_prefix_ignores_other_kinds_and_rebuilds_after_changes() {
        let mut session = test_session();
        let template = session.lanes[0].events[0].clone();
        session.lanes[0].events = vec![
            Event { start: 0.0, end: 1000.0, kind: Kind::Agent, ..template.clone() },
            Event { start: 1.0, end: 5.0, ..template.clone() },
            Event { start: 2.0, end: 2000.0, kind: Kind::Assistant, ..template.clone() },
            Event { start: 3.0, end: 4.0, ..template.clone() },
        ];
        session.finalize();
        assert_eq!(session.lanes[0].cum_end, [1000.0, 1000.0, 2000.0, 2000.0]);
        assert_eq!(session.lanes[0].cum_tool_end, [f64::NEG_INFINITY, 5.0, 5.0, 5.0]);
        session.lanes[0].events[1].kind = Kind::Thinking;
        session.finalize();
        assert_eq!(session.lanes[0].cum_tool_end, [f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY, 4.0]);
        session.lanes[0].events.clear();
        session.finalize();
        assert!(session.lanes[0].cum_end.is_empty() && session.lanes[0].cum_tool_end.is_empty());
        session.lanes[0].events.push(template);
        session.finalize();
        assert_eq!(session.lanes[0].cum_tool_end, [2.0]);
    }

    fn test_lane(name: &str, start: f64) -> Lane {
        let mut lane = Lane::new(name);
        lane.events.push(Event {
            start, end: start + 1.0, kind: Kind::Tool, name: name.into(), summary: name.into(),
            input: format!("input-{name}"), output: format!("output-{name}"), call_id: None,
            row: None, usage: Default::default(), model: None, status: Default::default(),
        });
        lane
    }

    fn test_session() -> Session {
        Session {
            id: "s".into(), source: Source::Claude, cwd: "project".into(), headless: false,
            title: "initial".into(), branch: "main".into(), version: "1".into(),
            lanes: vec![test_lane("main", 1.0)], start: 0.0, end: 0.0, parents: Vec::new(), tree: Vec::new(),
        }
    }

    #[test]
    fn store_updates_leave_shared_snapshots_and_other_origins_intact() {
        let mut store = Store::default();
        assert_eq!(store.replace([("parent".into(), vec![Loaded::Session(test_session())])]), [0]);
        let initial = store.snapshot();
        assert_eq!(store.replace([("child".into(), vec![Loaded::Child("s".into(), test_lane("child", 3.0))])]), [0]);
        let with_child = store.snapshot();
        assert_eq!(initial.sessions[0].lanes.len(), 1);
        assert_eq!(with_child.sessions[0].lanes.len(), 2);
        let mut updated = test_session();
        updated.headless = true;
        updated.title = "updated".into();
        updated.lanes = vec![test_lane("new-main", 5.0)];
        store.replace([("parent".into(), vec![Loaded::Session(updated)])]);
        assert_eq!(with_child.sessions[0].title, "initial");
        assert!(!with_child.sessions[0].headless);
        assert_eq!(with_child.sessions[0].lanes[0].events[0].input, "input-main");
        let current = &store.sessions[0];
        assert_eq!(current.title, "updated");
        assert!(current.headless);
        assert_eq!(current.lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["new-main", "child"]);
        assert_eq!((current.start, current.end), (3.0, 6.0));
        store.replace([("child".into(), Vec::new())]);
        assert_eq!(store.sessions[0].lanes.len(), 1);
        assert_eq!(with_child.sessions[0].lanes.len(), 2);
        assert_eq!((store.sessions[0].start, store.sessions[0].end), (5.0, 6.0));
        store.replace([("parent".into(), Vec::new())]);
        assert!(!store.visible(0));
        assert!(initial.visible(0));
    }

    #[test]
    fn orphan_replacement_is_attached_once_and_keeps_its_origin() {
        let mut store = Store::default();
        assert!(store.replace([("child".into(), vec![Loaded::Child("s".into(), test_lane("old", 0.0))])]).is_empty());
        store.replace([("child".into(), vec![Loaded::Child("s".into(), test_lane("child", 3.0))])]);
        store.replace([("parent".into(), vec![Loaded::Session(test_session())])]);
        let snapshot = store.snapshot();
        assert_eq!(store.sessions[0].lanes.len(), 2);
        assert_eq!(store.sessions[0].lanes[1].origin, "child");
        store.replace([("child".into(), Vec::new())]);
        assert_eq!(store.sessions[0].lanes.len(), 1);
        assert_eq!(snapshot.sessions[0].lanes[1].events[0].input, "input-child");
    }

    #[test]
    fn finalize_preserves_nested_parents_spawn_times_and_duplicate_ids() {
        let event = |start, id: &str| Event {
            start, end: start + 1.0, kind: Kind::Agent, name: "Agent".into(), summary: String::new(),
            input: String::new(), output: String::new(), call_id: Some(id.into()), row: None,
            usage: Default::default(), model: None, status: Default::default(),
        };
        let mut main = Lane::new("main");
        main.events = vec![event(1.0, "child"), event(2.0, "child")];
        let mut child = Lane::new("child");
        child.spawned_by = Some("child".into());
        child.events = vec![event(3.0, "grandchild")];
        let mut grandchild = Lane::new("grandchild");
        grandchild.spawned_by = Some("grandchild".into());
        grandchild.events = vec![event(4.0, "self")];
        let mut detached = Lane::new("detached");
        detached.spawned_by = Some("missing".into());
        detached.events = vec![event(5.0, "detached")];
        let mut session = Session {
            id: "s".into(), source: Source::Claude, cwd: String::new(), headless: false,
            title: String::new(), branch: String::new(), version: String::new(),
            lanes: vec![main, child, grandchild, detached], start: 0.0, end: 0.0, parents: Vec::new(), tree: Vec::new(),
        };
        session.finalize();
        assert_eq!(session.parents, [None, Some(0), Some(1), Some(0)]);
        assert_eq!(session.lanes.iter().map(|l| l.spawn_t).collect::<Vec<_>>(), [None, Some(1.0), Some(3.0), None]);
        assert_eq!(session.tree, [(0, 0), (1, 1), (2, 2), (3, 1)]);
        assert_eq!((session.start, session.end), (1.0, 6.0));
        session.lanes[0].events[0].start = 0.5;
        session.finalize();
        assert_eq!(session.lanes[1].spawn_t, Some(0.5));
        assert_eq!(session.start, 0.5);
    }

    #[test]
    fn bounded_text_matches_full_normalization() {
        for text in [
            "",
            " \t\n",
            "a",
            " a  b \n",
            "日本語\u{3000}abc\u{a0}終わり",
            "🦀é e\u{301}\t\r\n次",
            "abcd     ",
        ] {
            for max in 0..20 {
                let reference = |s: &str| {
                    let mut out: String = s.chars().take(max).collect();
                    if s.chars().count() > max {
                        out.push('…');
                    }
                    out
                };
                assert_eq!(truncate(text, max), reference(text));
                let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
                assert_eq!(
                    one_line(text, max),
                    reference(&joined),
                    "text={text:?}, max={max}"
                );
            }
        }
    }
}
