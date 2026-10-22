//! フロントエンドで共有するセッション一覧とプロジェクトの集計。
//!
//! [`Catalog`] の添字は作成に使った [`Snapshot`] を参照する。
//! 読み込み元だけで絞った `scope` と、プロジェクトも絞った `order` を分けることで、
//! プロジェクト選択中でも切り替え先の一覧を同じ条件で表示できる。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::{Snapshot, Source};
use crate::paths::{basename, parent_name};

/// 表示するセッションの読み込み元。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFilter {
    #[default]
    All,
    Claude,
    Codex,
}

impl SourceFilter {
    pub fn matches(self, source: Source) -> bool {
        match self {
            Self::All => true,
            Self::Claude => source == Source::Claude,
            Self::Codex => source == Source::Codex,
        }
    }
}

/// 作業ディレクトリごとのセッション集計。
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub cwd: String,
    /// 同名ディレクトリが複数ある場合は親ディレクトリを添える。
    pub name: String,
    pub sessions: usize,
    /// 最後に作業した時刻。
    pub last: f64,
}

/// 同じスナップショットと絞り込みから作った一覧。
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    /// 表示可能で、読み込み元が一致するセッション。元の順序を保つ。
    pub scope: Vec<usize>,
    /// `scope` から作業ディレクトリで絞り、開始時刻の新しい順にした一覧。
    /// 開始時刻が同じセッションは元の順序を保つ。
    pub order: Vec<usize>,
    /// `scope` のプロジェクト。最終作業時刻の新しい順。
    pub projects: Vec<Project>,
}

impl Catalog {
    pub fn new(snapshot: &Snapshot, source: SourceFilter, cwd: Option<&str>) -> Self {
        let scope: Vec<_> = (0..snapshot.sessions.len())
            .filter(|&i| snapshot.visible(i))
            .filter(|&i| source.matches(snapshot.sessions[i].source))
            .collect();
        let mut order: Vec<_> = scope
            .iter()
            .copied()
            .filter(|&i| cwd.is_none_or(|cwd| cwd == snapshot.sessions[i].cwd))
            .collect();
        order.sort_by(|&a, &b| {
            snapshot.sessions[b]
                .start
                .total_cmp(&snapshot.sessions[a].start)
        });
        let projects = projects(snapshot, &scope);
        Self {
            scope,
            order,
            projects,
        }
    }
}

fn projects(snapshot: &Snapshot, scope: &[usize]) -> Vec<Project> {
    let mut by: HashMap<&str, (usize, f64)> = HashMap::new();
    for &si in scope {
        let s = &snapshot.sessions[si];
        let e = by.entry(s.cwd.as_str()).or_insert((0, f64::NEG_INFINITY));
        e.0 += 1;
        e.1 = e.1.max(s.end);
    }
    let mut names: HashMap<&str, usize> = HashMap::new();
    for cwd in by.keys() {
        *names.entry(basename(cwd)).or_default() += 1;
    }
    let mut out: Vec<_> = by
        .iter()
        .map(|(&cwd, &(sessions, last))| {
            let base = basename(cwd);
            let name = if names[base] > 1 {
                format!("{base} — {}", parent_name(cwd))
            } else {
                base.to_string()
            };
            Project {
                cwd: cwd.to_string(),
                name,
                sessions,
                last,
            }
        })
        .collect();
    out.sort_by(|a, b| b.last.total_cmp(&a.last));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Lane, Session};
    use std::sync::Arc;

    fn session(source: Source, cwd: &str, start: f64, end: f64) -> Arc<Session> {
        Arc::new(Session {
            id: String::new(),
            source,
            cwd: cwd.to_string(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: vec![Lane::new("main")],
            start,
            end,
            parents: Vec::new(),
            tree: Vec::new(),
        })
    }

    #[test]
    fn source_and_project_filters_keep_scope_and_stable_newest_order() {
        let mut empty = session(Source::Codex, "/one", 100.0, 101.0);
        Arc::make_mut(&mut empty).lanes.clear();
        let snapshot = Snapshot {
            sessions: vec![
                session(Source::Codex, "/one", 10.0, 11.0),
                session(Source::Claude, "/one", 30.0, 31.0),
                session(Source::Codex, "/two", 50.0, 51.0),
                session(Source::Codex, "/one", 20.0, 21.0),
                session(Source::Codex, "/one", 20.0, 22.0),
                empty,
                session(Source::Codex, "/one", f64::INFINITY, 200.0),
            ],
        };
        let catalog = Catalog::new(&snapshot, SourceFilter::Codex, Some("/one"));
        assert_eq!(catalog.scope, [0, 2, 3, 4]);
        assert_eq!(catalog.order, [3, 4, 0]);
        assert_eq!(catalog.projects.len(), 2);
        assert_eq!(catalog.projects[0].cwd, "/two");
        assert_eq!(catalog.projects[1].sessions, 3);
        assert_eq!(catalog.projects[1].last, 22.0);

        let all = Catalog::new(&snapshot, SourceFilter::All, None);
        assert_eq!(all.scope, [0, 1, 2, 3, 4]);
        assert_eq!(all.order, [2, 1, 3, 4, 0]);
        assert_eq!(
            Catalog::new(&snapshot, SourceFilter::Claude, None).order,
            [1]
        );
        let missing = Catalog::new(&snapshot, SourceFilter::All, Some("/missing"));
        assert!(missing.order.is_empty());
        assert_eq!(missing.scope, all.scope);
        assert_eq!(missing.projects.len(), all.projects.len());
        assert!(
            Catalog::new(&Snapshot::default(), SourceFilter::All, None)
                .scope
                .is_empty()
        );
    }

    #[test]
    fn project_names_disambiguate_unix_and_windows_paths_within_source_scope() {
        let snapshot = Snapshot {
            sessions: vec![
                session(Source::Codex, "/home/alice/work/", 1.0, 10.0),
                session(Source::Claude, r"C:\Users\bob\work\", 2.0, 20.0),
                session(Source::Codex, "/home/alice/unique", 3.0, 30.0),
            ],
        };
        let all = Catalog::new(&snapshot, SourceFilter::All, None);
        assert_eq!(
            all.projects
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["unique", "work — bob", "work — alice"]
        );
        let codex = Catalog::new(&snapshot, SourceFilter::Codex, None);
        assert_eq!(codex.projects[1].name, "work");
    }

    #[test]
    fn source_filter_preserves_saved_preferences() {
        for (filter, encoded) in [
            (SourceFilter::All, "\"all\""),
            (SourceFilter::Claude, "\"claude\""),
            (SourceFilter::Codex, "\"codex\""),
        ] {
            assert_eq!(serde_json::to_string(&filter).unwrap(), encoded);
            assert_eq!(
                serde_json::from_str::<SourceFilter>(encoded).unwrap(),
                filter
            );
        }
    }
}
