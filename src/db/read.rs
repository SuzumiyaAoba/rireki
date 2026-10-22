//! 索引の復元と、遅延読み込みするイベント本文の取得。

use super::events::{EventRows, read_events};
use super::{Db, unpack};
use crate::model::{Lane, Loaded, Session, Source};
use std::collections::HashMap;

impl Db {
    pub fn fingerprints(&self) -> rusqlite::Result<HashMap<String, String>> {
        let mut st = self.conn.prepare("SELECT path, fingerprint FROM files")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// 保存済みの全ファイル分を、描画用の軽量なイベント（input / output なし）として読む
    pub fn load_all(&self) -> rusqlite::Result<Vec<(String, Vec<Loaded>)>> {
        struct Item {
            path: String,
            child: bool,
            sid: String,
            source: Source,
            cwd: String,
            title: String,
            headless: bool,
            branch: String,
            version: String,
            lanes: Vec<Lane>,
        }
        let mut items: Vec<Item> = Vec::new();
        let mut item_ix: HashMap<i64, usize> = HashMap::new();
        {
            let mut st = self
                .conn
                .prepare("SELECT id, path, kind, session_id, source, cwd, title, headless, branch, version FROM items ORDER BY id")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                item_ix.insert(r.get(0)?, items.len());
                items.push(Item {
                    path: r.get(1)?,
                    child: r.get::<_, i64>(2)? == 1,
                    sid: r.get(3)?,
                    source: if r.get::<_, i64>(4)? == 1 {
                        Source::Codex
                    } else {
                        Source::Claude
                    },
                    cwd: r.get(5)?,
                    title: r.get(6)?,
                    headless: r.get(7)?,
                    branch: r.get(8)?,
                    version: r.get(9)?,
                    lanes: Vec::new(),
                });
            }
        }
        // lane_id → (item index, lane index)
        let mut lane_ix: HashMap<i64, (usize, usize)> = HashMap::new();
        {
            let mut st = self.conn.prepare("SELECT id, item_id, name, spawned_by, model, effort FROM lanes ORDER BY item_id, ord")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                let Some(&ii) = item_ix.get(&r.get::<_, i64>(1)?) else {
                    continue;
                };
                let mut lane = Lane::new(r.get::<_, String>(2)?);
                lane.spawned_by = r.get(3)?;
                lane.model = r.get(4)?;
                lane.effort = r.get(5)?;
                lane_ix.insert(r.get(0)?, (ii, items[ii].lanes.len()));
                items[ii].lanes.push(lane);
            }
        }
        drop(item_ix);
        // events は行 ID の範囲で分割し、別々の接続から並列に読む（ID 順に連結すれば元の順序になる）
        let parts = read_events(&self.conn)?;
        // 既に読み取った行からレーンごとの必要数を数える。配列の倍増による空き容量と
        // 再確保時の大きなイベント配列のコピーを避け、追加のSQL走査は行わない。
        let group_bound = parts
            .iter()
            .map(|part| match part {
                EventRows::Grouped(groups) => groups.len(),
                EventRows::Flat(rows) => rows.len(),
            })
            .sum::<usize>();
        let mut counts = HashMap::<i64, usize>::with_capacity(lane_ix.len().min(group_bound));
        for part in &parts {
            match part {
                EventRows::Grouped(groups) => {
                    for group in groups {
                        *counts.entry(group.lane_id).or_default() += group.events.len();
                    }
                }
                EventRows::Flat(rows) => {
                    for group in rows.chunk_by(|a, b| a.0 == b.0) {
                        *counts.entry(group[0].0).or_default() += group.len();
                    }
                }
            }
        }
        for part in parts {
            match part {
                EventRows::Grouped(groups) => {
                    for mut group in groups {
                        let Some(&(ii, li)) = lane_ix.get(&group.lane_id) else {
                            continue;
                        };
                        let events = &mut items[ii].lanes[li].events;
                        if let Some(count) = counts.remove(&group.lane_id) {
                            // 最初の配列をそのまま渡し、区間を跨ぐ場合だけ残りの容量を確保する。
                            group.events.reserve_exact(count - group.events.len());
                            *events = group.events;
                        } else {
                            events.append(&mut group.events);
                        }
                    }
                }
                EventRows::Flat(rows) => {
                    let mut last = None;
                    for (lane_id, event) in rows {
                        let (ii, li) = match last {
                            Some((id, ii, li)) if id == lane_id => (ii, li),
                            _ => {
                                let Some(&(ii, li)) = lane_ix.get(&lane_id) else {
                                    continue;
                                };
                                last = Some((lane_id, ii, li));
                                (ii, li)
                            }
                        };
                        let events = &mut items[ii].lanes[li].events;
                        if events.is_empty()
                            && let Some(count) = counts.remove(&lane_id)
                        {
                            events.reserve_exact(count);
                        }
                        events.push(event);
                    }
                }
            }
        }
        drop(counts);
        drop(lane_ix);

        let mut by_path: Vec<(String, Vec<Loaded>)> = Vec::new();
        let mut path_ix: HashMap<String, usize> = HashMap::new();
        for mut it in items {
            let loaded = if it.child {
                let Some(lane) = it.lanes.pop() else { continue };
                Loaded::Child(it.sid, lane)
            } else {
                Loaded::Session(Session {
                    id: it.sid,
                    source: it.source,
                    cwd: it.cwd,
                    headless: it.headless,
                    title: it.title,
                    branch: it.branch,
                    version: it.version,
                    lanes: it.lanes,
                    start: 0.0,
                    end: 0.0,
                    parents: Vec::new(),
                    tree: Vec::new(),
                })
            };
            let i = *path_ix.entry(it.path.clone()).or_insert_with(|| {
                by_path.push((it.path, Vec::new()));
                by_path.len() - 1
            });
            by_path[i].1.push(loaded);
        }
        Ok(by_path)
    }

    /// 行 ID からイベントの input / output を取り出す
    pub fn details(&self, rows: &[i64]) -> HashMap<i64, (String, String)> {
        let mut out = HashMap::with_capacity(rows.len());
        if rows.is_empty() {
            return out;
        }
        // 読み取りスナップショットを共有し、行ごとのロック・WAL の確認を省く。
        // 既存のトランザクション内なら、そのスナップショットをそのまま使う。
        let _transaction = (rows.len() > 1)
            .then(|| self.conn.unchecked_transaction().ok())
            .flatten();
        if rows.len() > 16 {
            const BATCH: usize = 64;
            static SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
                format!(
                    "SELECT id, input, output FROM texts WHERE id IN ({})",
                    std::iter::repeat_n("?", BATCH)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            });
            let Ok(mut st) = self.conn.prepare_cached(&SQL) else {
                return out;
            };
            for chunk in rows.chunks(BATCH) {
                let ids = chunk
                    .iter()
                    .copied()
                    .map(Some)
                    .chain(std::iter::repeat_n(None, BATCH - chunk.len()));
                let Ok(found) = st.query_map(rusqlite::params_from_iter(ids), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        (
                            unpack(row.get_ref(1)?.as_blob()?),
                            unpack(row.get_ref(2)?.as_blob()?),
                        ),
                    ))
                }) else {
                    continue;
                };
                out.extend(found.flatten());
            }
            return out;
        }
        let Ok(mut st) = self
            .conn
            .prepare_cached("SELECT input, output FROM texts WHERE id = ?1")
        else {
            return out;
        };
        for &id in rows {
            let v = st.query_row([id], |r| {
                Ok((
                    unpack(r.get_ref(0)?.as_blob()?),
                    unpack(r.get_ref(1)?.as_blob()?),
                ))
            });
            if let Ok(v) = v {
                out.insert(id, v);
            }
        }
        out
    }
}
