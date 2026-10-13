//! セッションログの探索と、関連ファイルを含む変更検知。

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Job, JobKind};

struct Stamp {
    mtime: f64,
    fp: String,
}

impl Stamp {
    fn new() -> Self {
        Self {
            mtime: 0.0,
            fp: String::new(),
        }
    }

    fn push(&mut self, p: &Path) {
        let Ok(m) = fs::metadata(p) else { return };
        let t = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok());
        let t = t.map(|d| d.as_secs_f64()).unwrap_or(0.0);
        self.mtime = self.mtime.max(t);
        write!(&mut self.fp, "{t:.3}:{};", m.len()).unwrap();
    }
}

/// 読み込み対象のファイルを新しい順に列挙する
pub fn discover() -> Vec<Job> {
    let home = dirs::home_dir().unwrap_or_default();
    let claude = home.join(".claude/projects");
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or(home.join(".codex"))
        .join("sessions");
    discover_in(&claude, &codex)
}

/// 計測とテストでは実際のホームを変更せず、同じ形式のルートを指定する
pub fn discover_in(claude: &Path, codex: &Path) -> Vec<Job> {
    let mut jobs: Vec<Job> = Vec::new();
    for proj in read_dir(claude) {
        for f in read_dir(&proj) {
            if f.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let legacy = f
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("agent-"));
            let mut st = Stamp::new();
            st.push(&f);
            let kind = if legacy {
                JobKind::ClaudeLegacyAgent
            } else {
                let mut subs = read_dir(&f.with_extension("").join("subagents"));
                subs.sort();
                for s in &subs {
                    st.push(s);
                }
                JobKind::Claude
            };
            jobs.push(Job {
                kind,
                path: f,
                mtime: st.mtime,
                fingerprint: st.fp,
            });
        }
    }

    let mut stack = vec![codex.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            // 列挙時の種類を使い、ログごとの is_dir の stat を省く。リンクは従来どおり辿る
            let directory = entry
                .file_type()
                .map(|kind| {
                    if kind.is_symlink() {
                        p.is_dir()
                    } else {
                        kind.is_dir()
                    }
                })
                .unwrap_or_else(|_| p.is_dir());
            if directory {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "jsonl") {
                let mut st = Stamp::new();
                st.push(&p);
                jobs.push(Job {
                    kind: JobKind::Codex,
                    path: p,
                    mtime: st.mtime,
                    fingerprint: st.fp,
                });
            }
        }
    }

    jobs.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
    jobs
}

pub(super) fn read_dir(p: &Path) -> Vec<PathBuf> {
    fs::read_dir(p)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}
