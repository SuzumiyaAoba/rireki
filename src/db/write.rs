//! ファイル単位の置き換えと、コミットに連動した共有本文の更新。

use super::codec::{kind_to_i, source_to_i, status_to_i};
use super::{Db, Packed};
use crate::model::{Lane, Loaded, Session};
use rusqlite::params;
use std::sync::Arc;

impl Db {
    /// 複数ファイルのパース結果をまとめて保存する（既存の行は置き換え）
    pub fn put(&mut self, files: &[Packed]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        let mut saved = Vec::with_capacity(files.len());
        // 削除した行IDを新しい本文で再利用せず、旧版の読み取りとの取り違えを防ぐ。
        let mut row_id = self.last_event_id.max(tx.query_row(
            "SELECT IFNULL(MAX(id), 0) FROM events",
            [],
            |r| r.get::<_, i64>(0),
        )?);
        {
            let mut del_text = tx.prepare_cached(
                "DELETE FROM t.texts WHERE id IN (SELECT events.id FROM events WHERE lane_id IN
                 (SELECT lanes.id FROM lanes JOIN items ON lanes.item_id = items.id WHERE items.path = ?1))",
            )?;
            let mut del_ev = tx.prepare_cached(
                "DELETE FROM events WHERE lane_id IN
                 (SELECT lanes.id FROM lanes JOIN items ON lanes.item_id = items.id WHERE items.path = ?1)",
            )?;
            let mut del_lane = tx.prepare_cached(
                "DELETE FROM lanes WHERE item_id IN (SELECT id FROM items WHERE path = ?1)",
            )?;
            let mut del_item = tx.prepare_cached("DELETE FROM items WHERE path = ?1")?;
            let mut ins_file = tx.prepare_cached(
                "INSERT OR REPLACE INTO files (path, fingerprint) VALUES (?1, ?2)",
            )?;
            let mut ins_item = tx.prepare_cached(
                "INSERT INTO items (path, kind, session_id, source, cwd, title, headless, branch, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            let mut ins_lane =
                tx.prepare_cached("INSERT INTO lanes (item_id, ord, name, spawned_by, model, effort) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
            let mut ins_ev = tx.prepare_cached(
                "INSERT INTO events (lane_id, start, end, kind, name, summary, call_id, u_in, u_cr, u_cw, u_out, u_rs, u_cw1h, u_model, status, id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            )?;
            let mut ins_text =
                tx.prepare_cached("INSERT INTO t.texts (id, input, output) VALUES (?1, ?2, ?3)")?;
            let mut old_text =
                tx.prepare_cached("SELECT input, output FROM t.texts WHERE id = ?1")?;
            for Packed {
                path,
                fingerprint: fp,
                items,
                texts,
                source,
            } in files
            {
                let mut ids = Vec::with_capacity(texts.len());
                let mut texts = texts.iter();
                // 更新前の版がまだ読まれている場合だけ、その本文を削除前に退避する。
                if let Some(previous) = self.saved.get(path).and_then(std::sync::Weak::upgrade)
                    && !Arc::ptr_eq(&previous, source)
                    && let Some(rows) = previous.rows()
                {
                    let bodies: rusqlite::Result<_> = rows
                        .iter()
                        .map(|id| {
                            old_text.query_row([id], |r| {
                                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
                            })
                        })
                        .collect();
                    previous.preserve(bodies?);
                }
                del_text.execute([path])?;
                del_ev.execute([path])?;
                del_lane.execute([path])?;
                del_item.execute([path])?;
                ins_file.execute([path, fp])?;
                for item in items {
                    let (kind, sid, source, lanes, s): (
                        i64,
                        &str,
                        i64,
                        Vec<&Lane>,
                        Option<&Session>,
                    ) = match item {
                        Loaded::Session(s) => (
                            0,
                            &s.id,
                            source_to_i(s.source),
                            s.lanes.iter().collect(),
                            Some(s),
                        ),
                        Loaded::Child(parent, lane) => (1, parent, 0, vec![lane], None),
                    };
                    let (cwd, title, headless, branch, version) = match s {
                        Some(s) => (
                            s.cwd.as_str(),
                            s.title.as_str(),
                            s.headless,
                            s.branch.as_str(),
                            s.version.as_str(),
                        ),
                        None => ("", "", false, "", ""),
                    };
                    ins_item.execute(params![
                        path, kind, sid, source, cwd, title, headless, branch, version
                    ])?;
                    let item_id = tx.last_insert_rowid();
                    for (ord, lane) in lanes.iter().enumerate() {
                        ins_lane.execute(params![
                            item_id,
                            ord as i64,
                            lane.name,
                            lane.spawned_by,
                            lane.model,
                            lane.effort
                        ])?;
                        let lane_id = tx.last_insert_rowid();
                        for e in &lane.events {
                            row_id = row_id
                                .checked_add(1)
                                .ok_or(rusqlite::Error::IntegralValueOutOfRange(0, row_id))?;
                            ins_ev.execute(params![
                                lane_id,
                                e.start,
                                e.end,
                                kind_to_i(e.kind),
                                e.name.as_ref(),
                                e.summary.as_ref(),
                                e.call_id.as_deref(),
                                e.usage.input,
                                e.usage.cache_read,
                                e.usage.cache_write,
                                e.usage.output,
                                e.usage.reasoning,
                                e.usage.cache_write_1h,
                                e.model.as_deref(),
                                status_to_i(e.status),
                                row_id
                            ])?;
                            let (input, output) = texts
                                .next()
                                .map(|(i, o)| (i.as_slice(), o.as_slice()))
                                .unwrap_or_default();
                            let id = tx.last_insert_rowid();
                            ins_text.execute(params![id, input, output])?;
                            ids.push(id);
                        }
                    }
                }
                saved.push(ids);
            }
        }
        tx.commit()?;
        self.last_event_id = row_id;
        for (file, rows) in files.iter().zip(saved) {
            file.source.committed(rows);
            self.saved
                .insert(file.path.clone(), Arc::downgrade(&file.source));
        }
        Ok(())
    }
}
