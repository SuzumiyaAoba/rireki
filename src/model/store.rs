//! ファイル単位の履歴差し替えと共有スナップショット。

use super::{Lane, Session};
use std::collections::HashMap;
use std::sync::Arc;

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
        Snapshot {
            sessions: self.sessions.clone(),
        }
    }

    /// ファイル (origin) ごとに、そのファイル由来のデータを items で置き換える。
    /// 派生データの再計算はセッションごとに最後に一度だけ行う。変更のあったセッションの index を返す
    pub fn replace(
        &mut self,
        updates: impl IntoIterator<Item = (String, Vec<Loaded>)>,
    ) -> Vec<usize> {
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
                            id: s.id.clone(),
                            source: s.source,
                            cwd: s.cwd.clone(),
                            headless: s.headless,
                            title: s.title.clone(),
                            branch: s.branch.clone(),
                            version: s.version.clone(),
                            lanes: s
                                .lanes
                                .iter()
                                .filter(|l| l.origin != origin)
                                .cloned()
                                .collect(),
                            start: s.start,
                            end: s.end,
                            parents: Vec::new(),
                            tree: Vec::new(),
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
            let mut mine: Vec<usize> = items
                .into_iter()
                .filter_map(|item| self.add(&origin, item))
                .collect();
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
                        self.origins
                            .entry(l.origin.clone())
                            .or_default()
                            .push(self.sessions.len());
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
