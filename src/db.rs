//! 安定した（更新の止まった）履歴を保存する索引 DB (SQLite)
//!
//! JSONL をパースした結果をファイル単位で保存し、起動時はここから読む。
//! 描画に必要な列だけを一括で読み、input / output は詳細表示のときに行 ID で取り出す。

use std::collections::HashMap;
use std::path::PathBuf;

use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags, params};

use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Status, Usage};

/// スキーマやパース結果の形式を変えたら上げる（古い DB は作り直す）
const VERSION: i64 = 15;

pub struct Db {
    conn: Connection,
}

fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RIREKI_CACHE_DIR") { return PathBuf::from(dir); }
    let base = dirs::cache_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"));
    base.join("rireki")
}

pub fn path() -> PathBuf {
    dir().join("index.db")
}

/// input / output の本文は別ファイルにする。同じファイルだと events のページが
/// 巨大な本文の間に散らばり、起動時の全件読み込みがディスク上を飛び回って遅くなる
fn texts_path() -> PathBuf {
    dir().join("texts.db")
}

/// DB ファイルを WAL / 共有メモリごと消す
fn remove_db(p: &std::path::Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut s = p.as_os_str().to_owned();
        s.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(s));
    }
}

fn read_conn(p: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.pragma_update(None, "mmap_size", 1i64 << 30)?;
    Ok(conn)
}

fn kind_to_i(k: Kind) -> i64 {
    match k {
        Kind::User => 0,
        Kind::Assistant => 1,
        Kind::Thinking => 2,
        Kind::Tool => 3,
        Kind::Agent => 4,
    }
}

fn kind_of(i: i64) -> Kind {
    match i {
        0 => Kind::User,
        1 => Kind::Assistant,
        2 => Kind::Thinking,
        4 => Kind::Agent,
        _ => Kind::Tool,
    }
}

fn status_to_i(s: Status) -> i64 {
    match s {
        Status::Ok => 0,
        Status::Error => 1,
        Status::Rejected => 2,
        Status::Interrupted => 3,
        Status::Pending => 4,
    }
}

fn status_of(i: i64) -> Status {
    match i {
        1 => Status::Error,
        2 => Status::Rejected,
        3 => Status::Interrupted,
        4 => Status::Pending,
        _ => Status::Ok,
    }
}

fn source_to_i(s: Source) -> i64 {
    match s {
        Source::Claude => 0,
        Source::Codex => 1,
    }
}

fn pack(s: &str) -> Vec<u8> {
    if s.is_empty() { Vec::new() } else { lz4_flex::compress_prepend_size(s.as_bytes()) }
}

fn unpack(b: &[u8]) -> String {
    if b.is_empty() {
        return String::new();
    }
    lz4_flex::decompress_size_prepended(b).ok().and_then(|v| String::from_utf8(v).ok()).unwrap_or_default()
}

/// 書き込み用に用意した 1 ファイル分のデータ。本文は圧縮済みで、items 側の input / output は空
pub struct Packed {
    pub path: String,
    pub fingerprint: String,
    pub items: Vec<Loaded>,
    /// イベントの出現順の (input, output)
    pub texts: Vec<(Vec<u8>, Vec<u8>)>,
}

/// 本文の圧縮は書き込みより重いので、書き込みスレッドに渡す前に並列で済ませておく
pub fn pack_files(files: &[(String, String, &[Loaded])]) -> Vec<Packed> {
    let lane = |l: &Lane, texts: &mut Vec<(Vec<u8>, Vec<u8>)>| Lane {
        name: l.name.clone(),
        spawned_by: l.spawned_by.clone(),
        model: l.model.clone(),
        effort: l.effort.clone(),
        events: l
            .events
            .iter()
            .map(|e| {
                texts.push((pack(&e.input), pack(&e.output)));
                Event { input: String::new(), output: String::new(), ..e.clone_meta() }
            })
            .collect(),
        ..Lane::new("")
    };
    files
        .par_iter()
        .map(|(path, fp, items)| {
            let mut texts = Vec::new();
            let items = items
                .iter()
                .map(|item| match item {
                    Loaded::Session(s) => Loaded::Session(Session {
                        id: s.id.clone(),
                        source: s.source,
                        cwd: s.cwd.clone(),
                        headless: s.headless,
                        title: s.title.clone(),
                        branch: s.branch.clone(),
                        version: s.version.clone(),
                        lanes: s.lanes.iter().map(|l| lane(l, &mut texts)).collect(),
                        start: 0.0,
                        end: 0.0,
                        parents: Vec::new(),
                        tree: Vec::new(),
                    }),
                    Loaded::Child(parent, l) => Loaded::Child(parent.clone(), lane(l, &mut texts)),
                })
                .collect();
            Packed { path: path.clone(), fingerprint: fp.clone(), items, texts }
        })
        .collect()
}

impl Db {
    /// 計測用の索引を読み取り専用で開く。スキーマや実データは変更しない
    pub fn open_index_read(p: &std::path::Path) -> rusqlite::Result<Self> {
        Ok(Self { conn: read_conn(p)? })
    }

    /// 書き込み用に開く。必要ならスキーマを作り直す
    pub fn open() -> rusqlite::Result<Self> {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut conn = Connection::open(&p)?;
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v != VERSION {
            // 古い DB は表を消してもファイルが縮まないので、ファイルごと作り直す
            drop(conn);
            remove_db(&p);
            remove_db(&texts_path());
            conn = Connection::open(&p)?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute("ATTACH DATABASE ?1 AS t", [texts_path().to_string_lossy()])?;
        conn.execute_batch("PRAGMA t.journal_mode = WAL; PRAGMA t.synchronous = NORMAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                 path TEXT PRIMARY KEY,
                 fingerprint TEXT NOT NULL
             );
             -- kind: 0 = セッション, 1 = 親セッションに合流する子レーン
             CREATE TABLE IF NOT EXISTS items (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL,
                 kind INTEGER NOT NULL,
                 session_id TEXT NOT NULL,
                 source INTEGER NOT NULL,
                 cwd TEXT NOT NULL,
                 title TEXT NOT NULL,
                 headless INTEGER NOT NULL DEFAULT 0,
                 branch TEXT NOT NULL DEFAULT '',
                 version TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS items_path ON items(path);
             CREATE TABLE IF NOT EXISTS lanes (
                 id INTEGER PRIMARY KEY,
                 item_id INTEGER NOT NULL,
                 ord INTEGER NOT NULL,
                 name TEXT NOT NULL,
                 spawned_by TEXT,
                 model TEXT NOT NULL DEFAULT '',
                 effort TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS lanes_item ON lanes(item_id);
             CREATE TABLE IF NOT EXISTS events (
                 id INTEGER PRIMARY KEY,
                 lane_id INTEGER NOT NULL,
                 start REAL NOT NULL,
                 end REAL NOT NULL,
                 kind INTEGER NOT NULL,
                 name TEXT NOT NULL,
                 summary TEXT NOT NULL,
                 call_id TEXT,
                 -- トークン使用量（なければ 0）
                 u_in INTEGER NOT NULL DEFAULT 0,
                 u_cr INTEGER NOT NULL DEFAULT 0,
                 u_cw INTEGER NOT NULL DEFAULT 0,
                 u_out INTEGER NOT NULL DEFAULT 0,
                 u_rs INTEGER NOT NULL DEFAULT 0,
                 u_cw1h INTEGER NOT NULL DEFAULT 0,
                 u_model TEXT,
                 -- 0 = 成功, 1 = 失敗, 2 = 拒否, 3 = 中断, 4 = 結果なし
                 status INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS events_lane ON events(lane_id);
             -- 大きな本文は別表にして、起動時の events 全件読み込みを軽くする
             CREATE TABLE IF NOT EXISTS t.texts (
                 id INTEGER PRIMARY KEY,
                 -- lz4 圧縮した本文
                 input BLOB NOT NULL,
                 output BLOB NOT NULL
             );",
        )?;
        conn.pragma_update(None, "user_version", VERSION)?;
        Ok(Self { conn })
    }

    /// 詳細本文を引くための読み取り専用接続
    pub fn open_read() -> rusqlite::Result<Self> {
        Ok(Self { conn: read_conn(&texts_path())? })
    }

    pub fn fingerprints(&self) -> rusqlite::Result<HashMap<String, String>> {
        let mut st = self.conn.prepare("SELECT path, fingerprint FROM files")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// 複数ファイルのパース結果をまとめて保存する（既存の行は置き換え）
    pub fn put(&mut self, files: &[Packed]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut del_text = tx.prepare_cached(
                "DELETE FROM t.texts WHERE id IN (SELECT events.id FROM events WHERE lane_id IN
                 (SELECT lanes.id FROM lanes JOIN items ON lanes.item_id = items.id WHERE items.path = ?1))",
            )?;
            let mut del_ev = tx.prepare_cached(
                "DELETE FROM events WHERE lane_id IN
                 (SELECT lanes.id FROM lanes JOIN items ON lanes.item_id = items.id WHERE items.path = ?1)",
            )?;
            let mut del_lane =
                tx.prepare_cached("DELETE FROM lanes WHERE item_id IN (SELECT id FROM items WHERE path = ?1)")?;
            let mut del_item = tx.prepare_cached("DELETE FROM items WHERE path = ?1")?;
            let mut ins_file = tx.prepare_cached("INSERT OR REPLACE INTO files (path, fingerprint) VALUES (?1, ?2)")?;
            let mut ins_item = tx.prepare_cached(
                "INSERT INTO items (path, kind, session_id, source, cwd, title, headless, branch, version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            let mut ins_lane =
                tx.prepare_cached("INSERT INTO lanes (item_id, ord, name, spawned_by, model, effort) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
            let mut ins_ev = tx.prepare_cached(
                "INSERT INTO events (lane_id, start, end, kind, name, summary, call_id, u_in, u_cr, u_cw, u_out, u_rs, u_cw1h, u_model, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;
            let mut ins_text = tx.prepare_cached("INSERT INTO t.texts (id, input, output) VALUES (?1, ?2, ?3)")?;
            for Packed { path, fingerprint: fp, items, texts } in files {
                let mut texts = texts.iter();
                del_text.execute([path])?;
                del_ev.execute([path])?;
                del_lane.execute([path])?;
                del_item.execute([path])?;
                ins_file.execute([path, fp])?;
                for item in items {
                    let (kind, sid, source, lanes, s): (i64, &str, i64, Vec<&Lane>, Option<&Session>) = match item {
                        Loaded::Session(s) => (0, &s.id, source_to_i(s.source), s.lanes.iter().collect(), Some(s)),
                        Loaded::Child(parent, lane) => (1, parent, 0, vec![lane], None),
                    };
                    let (cwd, title, headless, branch, version) = match s {
                        Some(s) => {
                            (s.cwd.as_str(), s.title.as_str(), s.headless, s.branch.as_str(), s.version.as_str())
                        }
                        None => ("", "", false, "", ""),
                    };
                    ins_item.execute(params![path, kind, sid, source, cwd, title, headless, branch, version])?;
                    let item_id = tx.last_insert_rowid();
                    for (ord, lane) in lanes.iter().enumerate() {
                        ins_lane.execute(params![item_id, ord as i64, lane.name, lane.spawned_by, lane.model, lane.effort])?;
                        let lane_id = tx.last_insert_rowid();
                        for e in &lane.events {
                            ins_ev.execute(params![
                                lane_id,
                                e.start,
                                e.end,
                                kind_to_i(e.kind),
                                e.name,
                                e.summary,
                                e.call_id,
                                e.usage.input,
                                e.usage.cache_read,
                                e.usage.cache_write,
                                e.usage.output,
                                e.usage.reasoning,
                                e.usage.cache_write_1h,
                                e.model.as_deref(),
                                status_to_i(e.status)
                            ])?;
                            let (input, output) =
                                texts.next().map(|(i, o)| (i.as_slice(), o.as_slice())).unwrap_or_default();
                            ins_text.execute(params![tx.last_insert_rowid(), input, output])?;
                        }
                    }
                }
            }
        }
        tx.commit()
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
                    source: if r.get::<_, i64>(4)? == 1 { Source::Codex } else { Source::Claude },
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
        // events は行 ID の範囲で分割し、別々の接続から並列に読む（ID 順に連結すれば元の順序になる）
        let mut last: Option<(i64, usize, usize)> = None;
        for part in read_events(&self.conn)? {
            for (lane_id, e) in part {
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
                items[ii].lanes[li].events.push(e);
            }
        }

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
        let mut out = HashMap::new();
        let Ok(mut st) = self.conn.prepare_cached("SELECT input, output FROM texts WHERE id = ?1") else {
            return out;
        };
        for &id in rows {
            let v = st.query_row([id], |r| Ok((unpack(r.get_ref(0)?.as_blob()?), unpack(r.get_ref(1)?.as_blob()?))));
            if let Ok(v) = v {
                out.insert(id, v);
            }
        }
        out
    }
}

/// Codex の exec を JS のモック実行で整形した結果のキャッシュ（詳細表示時に作る）
///
/// 索引 DB とは別ファイルにし、キーは JS 本文のハッシュにする。リアルタイムファイルの
/// イベントは索引 DB に行を持たないため、行 ID ではなく内容で引く。
pub struct JsCache {
    conn: Connection,
}

impl JsCache {
    pub fn open() -> rusqlite::Result<Self> {
        let _ = std::fs::create_dir_all(dir());
        let conn = Connection::open(dir().join("jscache.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v != crate::jsmock::ENGINE_VERSION {
            conn.execute_batch("DROP TABLE IF EXISTS calls; VACUUM;")?;
            conn.pragma_update(None, "user_version", crate::jsmock::ENGINE_VERSION)?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS calls (
                 hash INTEGER PRIMARY KEY,
                 -- lz4 圧縮した整形済みテキスト
                 detail BLOB NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    pub fn get(&self, hash: i64) -> Option<String> {
        let mut st = self.conn.prepare_cached("SELECT detail FROM calls WHERE hash = ?1").ok()?;
        st.query_row([hash], |r| Ok(unpack(r.get_ref(0)?.as_blob()?))).ok()
    }

    pub fn put(&self, hash: i64, detail: &str) {
        if let Ok(mut st) = self.conn.prepare_cached("INSERT OR REPLACE INTO calls (hash, detail) VALUES (?1, ?2)") {
            let _ = st.execute(params![hash, pack(detail)]);
        }
    }
}

/// events を ID 範囲ごとに並列で読む。結果は ID 順に並んだ範囲の列
fn read_events(conn: &Connection) -> rusqlite::Result<Vec<Vec<(i64, Event)>>> {
    let (lo, hi, n): (i64, i64, i64) =
        conn.query_row("SELECT IFNULL(MIN(id), 0), IFNULL(MAX(id), -1), COUNT(*) FROM events", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    if n == 0 {
        return Ok(Vec::new());
    }
    let parts = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16) as i64;
    let step = (hi - lo) / parts + 1;
    let cap = (n / parts) as usize + 16;
    let p = PathBuf::from(conn.path().ok_or(rusqlite::Error::InvalidPath(PathBuf::new()))?);
    std::thread::scope(|sc| {
        let handles: Vec<_> = (0..parts)
            .map(|k| {
                let p = &p;
                sc.spawn(move || -> rusqlite::Result<Vec<(i64, Event)>> {
                    let conn = read_conn(p)?;
                    let mut st = conn.prepare(
                        "SELECT id, lane_id, start, end, kind, name, summary, call_id, u_in, u_cr, u_cw, u_out, u_rs, u_cw1h, u_model, status
                         FROM events WHERE id >= ?1 AND id < ?2 ORDER BY id",
                    )?;
                    let a = lo + k * step;
                    let mut rows = st.query([a, a + step])?;
                    let mut out = Vec::with_capacity(cap);
                    let mut models = crate::loader::Interner::default();
                    while let Some(r) = rows.next()? {
                        out.push((
                            r.get(1)?,
                            Event {
                                start: r.get(2)?,
                                end: r.get(3)?,
                                kind: kind_of(r.get(4)?),
                                name: r.get(5)?,
                                summary: r.get(6)?,
                                input: String::new(),
                                output: String::new(),
                                call_id: r.get(7)?,
                                row: Some(r.get(0)?),
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
                            },
                        ));
                    }
                    Ok(out)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("events 読み込みスレッドが異常終了")).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        for s in ["", "abc", &"日本語テキスト".repeat(500)] {
            assert_eq!(unpack(&pack(s)), s);
        }
    }
}
