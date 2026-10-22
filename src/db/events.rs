//! イベント行の並列読み取りと、レーン・要約の共有。

use super::codec::{kind_of, status_of};
use super::connection::read_conn;
use crate::model::{Event, Usage};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;

/// 同じ要約だけ共有する。枠には既読イベントの位置を持ち、共有参照を追加しない。
pub(super) struct SummaryCache([(u64, u32, u32); 1024]);

impl SummaryCache {
    pub(super) fn new() -> Self {
        Self([(0, u32::MAX, u32::MAX); 1024])
    }

    pub(super) fn key(s: &str) -> u64 {
        let bytes = s.as_bytes();
        let n = bytes.len();
        let word = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
        let sampled = if n >= 16 {
            word(&bytes[..8])
                ^ word(&bytes[8..16]).rotate_left(21)
                ^ word(&bytes[n - 8..]).rotate_left(43)
        } else if n >= 8 {
            word(&bytes[..8]) ^ word(&bytes[n - 8..]).rotate_left(21)
        } else {
            let mut padded = [0; 8];
            padded[..n].copy_from_slice(bytes);
            u64::from_le_bytes(padded)
        };
        // 枠選びだけを安く済ませる。再利用には、下で必ず文字列全体を比較する。
        let mut mixed = sampled ^ (n as u64).wrapping_mul(0x9e3779b97f4a7c15);
        mixed ^= mixed >> 33;
        mixed = mixed.wrapping_mul(0xff51afd7ed558ccd);
        mixed ^= mixed >> 33;
        mixed
    }

    pub(super) fn get<'a>(
        &mut self,
        s: &str,
        next: (usize, usize),
        at: impl FnOnce(usize, usize) -> Option<&'a Event>,
    ) -> Arc<str> {
        if s.len() > 512 {
            return Arc::from(s);
        }
        let key = Self::key(s);
        let slot = key as usize & 1023;
        let (previous_key, group, index) = self.0[slot];
        if previous_key == key
            && let Some(event) = at(group as usize, index as usize)
            && event.summary.as_ref() == s
        {
            return event.summary.clone();
        }
        if let (Ok(group), Ok(index)) = (u32::try_from(next.0), u32::try_from(next.1)) {
            self.0[slot] = (key, group, index);
        }
        Arc::from(s)
    }
}

/// ID 順に連続する同じレーンの行。配列は表示用レーンへ直接渡せる。
pub(super) struct EventGroup {
    pub(super) lane_id: i64,
    pub(super) events: Vec<Event>,
}

impl EventGroup {
    pub(super) fn finish(&mut self) {
        if self.events.len() < self.events.capacity() {
            // realloc の縮小では元の大きな確保が残る場合があるため、実サイズへ移す。
            let mut exact = Vec::with_capacity(self.events.len());
            exact.append(&mut self.events);
            self.events = exact;
        }
    }
}

pub(super) enum EventRows {
    Grouped(Vec<EventGroup>),
    Flat(Vec<(i64, Event)>),
}

fn begin_group(
    groups: &mut Vec<EventGroup>,
    lane_id: i64,
    seen: usize,
    cap: usize,
    max_initial: usize,
) -> Option<Vec<(i64, Event)>> {
    if groups.last().is_some_and(|g| g.lane_id == lane_id) {
        return None;
    }
    if let Some(previous) = groups.last_mut() {
        previous.finish();
    }
    if groups.len() >= 64 && seen < groups.len() * 4 {
        // 小さい群が続く入力は、イベントごとの小さな確保を増やさない。
        let mut flat = Vec::with_capacity(cap.max(seen));
        for group in groups.drain(..) {
            flat.extend(group.events.into_iter().map(|e| (group.lane_id, e)));
        }
        return Some(flat);
    }
    // 既読の群の平均から初期容量を選ぶ。小さい群には小さいまま、大きくても64件まで。
    let average = if groups.is_empty() {
        16
    } else {
        seen / groups.len()
    };
    let initial = average.clamp(1, 64).next_power_of_two().min(max_initial);
    groups.push(EventGroup {
        lane_id,
        events: Vec::with_capacity(initial),
    });
    None
}

fn read_event(
    r: &rusqlite::Row<'_>,
    names: &mut crate::loader::Interner,
    models: &mut crate::loader::Interner,
    summary: impl FnOnce(&str) -> Arc<str>,
) -> rusqlite::Result<Event> {
    Ok(Event {
        start: r.get(2)?,
        end: r.get(3)?,
        kind: kind_of(r.get(4)?),
        name: names.get(r.get_ref(5)?.as_str()?),
        summary: summary(r.get_ref(6)?.as_str()?),
        input: String::new(),
        output: String::new(),
        call_id: r.get_ref(7)?.as_str_or_null()?.map(Into::into),
        text_ref: crate::model::TextRef::row(r.get(0)?),
        usage: Usage {
            input: r.get(8)?,
            cache_read: r.get(9)?,
            cache_write: r.get(10)?,
            output: r.get(11)?,
            reasoning: r.get(12)?,
            cache_write_1h: r.get(13)?,
        },
        model: r.get_ref(14)?.as_str_or_null()?.map(|m| models.get(m)),
        status: status_of(r.get(15)?),
    })
}

/// events を ID 範囲ごとに並列で読む。範囲も各範囲内のレーン群も ID 順。
pub(super) fn read_events(conn: &Connection) -> rusqlite::Result<Vec<EventRows>> {
    let (lo, hi, n): (i64, i64, i64) =
        // MIN/MAX の単独検索と SQLite の COUNT 専用処理を使い、索引を全行走査しない。
        conn.query_row("SELECT (SELECT IFNULL(MIN(id), 0) FROM events),
                               (SELECT IFNULL(MAX(id), -1) FROM events),
                               (SELECT COUNT(*) FROM events)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    if n == 0 {
        return Ok(Vec::new());
    }
    let parts = (std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16) as i64)
        .min(n);
    let step = (hi - lo) / parts + 1;
    let max_initial = step.min(64) as usize;
    let cap = (n / parts) as usize + 16;
    let p = PathBuf::from(
        conn.path()
            .ok_or(rusqlite::Error::InvalidPath(PathBuf::new()))?,
    );
    std::thread::scope(|sc| {
        let handles: Vec<_> = (0..parts)
            .map(|k| {
                let p = &p;
                sc.spawn(move || -> rusqlite::Result<EventRows> {
                    let conn = read_conn(p)?;
                    let mut st = conn.prepare(
                        "SELECT id, lane_id, start, end, kind, name, summary, call_id, u_in, u_cr, u_cw, u_out, u_rs, u_cw1h, u_model, status
                         FROM events WHERE id >= ?1 AND id < ?2 ORDER BY id",
                    )?;
                    let a = lo + k * step;
                    let mut rows = st.query([a, a + step])?;
                    let mut out: Vec<EventGroup> = Vec::new();
                    let mut seen = 0;
                    let mut models = crate::loader::Interner::default();
                    let mut names = crate::loader::Interner::default();
                    let mut summaries = SummaryCache::new();
                    while let Some(r) = rows.next()? {
                        let lane_id = r.get(1)?;
                        if let Some(mut flat) = begin_group(&mut out, lane_id, seen, cap, max_initial) {
                            summaries = SummaryCache::new();
                            let event = read_event(r, &mut names, &mut models, |s|
                                summaries.get(s, (0, flat.len()), |_, i| flat.get(i).map(|(_, e)| e)))?;
                            flat.push((lane_id, event));
                            // 切り替えたら平坦な配列専用のループで読む。
                            while let Some(r) = rows.next()? {
                                let lane_id = r.get(1)?;
                                let event = read_event(r, &mut names, &mut models, |s|
                                    summaries.get(s, (0, flat.len()), |_, i| flat.get(i).map(|(_, e)| e)))?;
                                flat.push((lane_id, event));
                            }
                            // この配列はレーンへ移して解放する。余りが64行以内なら
                            // 大きな配列の縮小再確保を省く（最大16workerで1024行）。
                            if flat.capacity() - flat.len() > 64 { flat.shrink_to_fit(); }
                            return Ok(EventRows::Flat(flat));
                        }
                        let event = read_event(r, &mut names, &mut models, |s|
                            summaries.get(s, (out.len() - 1, out.last().unwrap().events.len()),
                                |g, i| out.get(g).and_then(|g| g.events.get(i))))?;
                        out.last_mut().unwrap().events.push(event);
                        seen += 1;
                    }
                    if let Some(last) = out.last_mut() { last.finish(); }
                    Ok(EventRows::Grouped(out))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("events 読み込みスレッドが異常終了"))
            .collect()
    })
}
