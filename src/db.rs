//! 安定した（更新の止まった）履歴を保存する索引 DB (SQLite)
//!
//! JSONL をパースした結果をファイル単位で保存し、起動時はここから読む。
//! 描画に必要な列だけを一括で読み、input / output は詳細表示のときに行 ID で取り出す。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags, params};

use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Status, Usage};

/// スキーマやパース結果の形式を変えたら上げる（古い DB は作り直す）
const VERSION: i64 = 15;

pub struct Db {
    conn: Connection,
    saved: HashMap<String, std::sync::Weak<crate::text::Texts>>,
    last_event_id: i64,
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

pub(crate) fn unpack(b: &[u8]) -> String {
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
    pub texts: Arc<crate::text::Compressed>,
    source: Arc<crate::text::Texts>,
}

impl Packed {
    /// 本文は保存待ちの圧縮データからも読めるため、解析した大きなStringを解放できる。
    pub(crate) fn release_texts(&self, items: &mut [Loaded]) {
        let mut index = 0;
        for item in items {
            let lanes = match item {
                Loaded::Session(s) => s.lanes.as_mut_slice(),
                Loaded::Child(_, lane) => std::slice::from_mut(lane),
            };
            for event in lanes.iter_mut().flat_map(|l| &mut l.events) {
                event.text_ref = self.source.reference(index);
                event.input = String::new();
                event.output = String::new();
                index += 1;
            }
        }
        assert_eq!(index, self.texts.len());
    }
}

/// 本文の圧縮は書き込みより重いので、書き込みスレッドに渡す前に並列で済ませておく
pub fn pack_files(files: &[(String, String, &[Loaded])]) -> Vec<Packed> {
    files.par_iter().map(|(path, fp, items)| pack_file(path, fp, items)).collect()
}

/// 1ファイルのパース直後に圧縮し、次のファイルを読む前に本文を解放できる。
pub(crate) fn pack_file(path: &str, fp: &str, items: &[Loaded]) -> Packed {
    let lane = |l: &Lane, texts: &mut Vec<(Vec<u8>, Vec<u8>)>| Lane {
        name: l.name.clone(),
        spawned_by: l.spawned_by.clone(),
        model: l.model.clone(),
        effort: l.effort.clone(),
        events: l
            .events
            .iter()
            .map(|e| {
                texts.push(e.text_ref.compressed().unwrap_or_else(|| (pack(&e.input), pack(&e.output))));
                Event { input: String::new(), output: String::new(), ..e.clone_meta() }
            })
            .collect(),
        ..Lane::new("")
    };
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
    let texts = Arc::new(texts);
    let source = crate::text::Texts::pending(texts.clone());
    Packed { path: path.to_owned(), fingerprint: fp.to_owned(), items, texts, source }
}

impl Db {
    /// 計測用の索引を読み取り専用で開く。スキーマや実データは変更しない
    pub fn open_index_read(p: &std::path::Path) -> rusqlite::Result<Self> {
        Ok(Self { conn: read_conn(p)?, saved: HashMap::new(), last_event_id: 0 })
    }

    /// 書き込み用に開く。必要ならスキーマを作り直す
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(&path(), &texts_path())
    }

    fn open_at(p: &std::path::Path, texts: &std::path::Path) -> rusqlite::Result<Self> {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut conn = Connection::open(p)?;
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v != VERSION {
            // 古い DB は表を消してもファイルが縮まないので、ファイルごと作り直す
            drop(conn);
            remove_db(p);
            remove_db(texts);
            conn = Connection::open(p)?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute("ATTACH DATABASE ?1 AS t", [texts.to_string_lossy()])?;
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
        Ok(Self { conn, saved: HashMap::new(), last_event_id: 0 })
    }

    /// 詳細本文を引くための読み取り専用接続
    pub fn open_read() -> rusqlite::Result<Self> {
        Ok(Self { conn: read_conn(&texts_path())?, saved: HashMap::new(), last_event_id: 0 })
    }

    pub fn fingerprints(&self) -> rusqlite::Result<HashMap<String, String>> {
        let mut st = self.conn.prepare("SELECT path, fingerprint FROM files")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// 複数ファイルのパース結果をまとめて保存する（既存の行は置き換え）
    pub fn put(&mut self, files: &[Packed]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        let mut saved = Vec::with_capacity(files.len());
        // 削除した行IDを新しい本文で再利用せず、旧版の読み取りとの取り違えを防ぐ。
        let mut row_id = self.last_event_id.max(tx.query_row("SELECT IFNULL(MAX(id), 0) FROM events", [], |r| r.get::<_, i64>(0))?);
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
                "INSERT INTO events (lane_id, start, end, kind, name, summary, call_id, u_in, u_cr, u_cw, u_out, u_rs, u_cw1h, u_model, status, id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            )?;
            let mut ins_text = tx.prepare_cached("INSERT INTO t.texts (id, input, output) VALUES (?1, ?2, ?3)")?;
            let mut old_text = tx.prepare_cached("SELECT input, output FROM t.texts WHERE id = ?1")?;
            for Packed { path, fingerprint: fp, items, texts, source } in files {
                let mut ids = Vec::with_capacity(texts.len());
                let mut texts = texts.iter();
                // 更新前の版がまだ読まれている場合だけ、その本文を削除前に退避する。
                if let Some(previous) = self.saved.get(path).and_then(std::sync::Weak::upgrade)
                    && !Arc::ptr_eq(&previous, source)
                    && let Some(rows) = previous.rows()
                {
                    let bodies: rusqlite::Result<_> = rows.iter().map(|id| {
                        old_text.query_row([id], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)))
                    }).collect();
                    previous.preserve(bodies?);
                }
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
                            row_id = row_id.checked_add(1).ok_or(rusqlite::Error::IntegralValueOutOfRange(0, row_id))?;
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
                            let (input, output) =
                                texts.next().map(|(i, o)| (i.as_slice(), o.as_slice())).unwrap_or_default();
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
            self.saved.insert(file.path.clone(), Arc::downgrade(&file.source));
        }
        Ok(())
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
        let parts = read_events(&self.conn)?;
        // 既に読み取った行からレーンごとの必要数を数える。配列の倍増による空き容量と
        // 再確保時の大きなイベント配列のコピーを避け、追加のSQL走査は行わない。
        let mut counts = HashMap::<i64, usize>::with_capacity(lane_ix.len());
        for part in &parts {
            match part {
                EventRows::Grouped(groups) => for group in groups {
                    *counts.entry(group.lane_id).or_default() += group.events.len();
                },
                EventRows::Flat(rows) => for group in rows.chunk_by(|a, b| a.0 == b.0) {
                    *counts.entry(group[0].0).or_default() += group.len();
                },
            }
        }
        for part in parts {
            match part {
            EventRows::Grouped(groups) => for mut group in groups {
                let Some(&(ii, li)) = lane_ix.get(&group.lane_id) else { continue };
                let events = &mut items[ii].lanes[li].events;
                if let Some(count) = counts.remove(&group.lane_id) {
                    // 最初の配列をそのまま渡し、区間を跨ぐ場合だけ残りの容量を確保する。
                    group.events.reserve_exact(count - group.events.len());
                    *events = group.events;
                } else {
                    events.append(&mut group.events);
                }
            },
            EventRows::Flat(rows) => {
                let mut last = None;
                for (lane_id, event) in rows {
                    let (ii, li) = match last {
                        Some((id, ii, li)) if id == lane_id => (ii, li),
                        _ => {
                            let Some(&(ii, li)) = lane_ix.get(&lane_id) else { continue };
                            last = Some((lane_id, ii, li));
                            (ii, li)
                        }
                    };
                    let events = &mut items[ii].lanes[li].events;
                    if events.is_empty()
                        && let Some(count) = counts.remove(&lane_id)
                    { events.reserve_exact(count); }
                    events.push(event);
                }
            },
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
        let mut out = HashMap::with_capacity(rows.len());
        if rows.is_empty() { return out; }
        // 読み取りスナップショットを共有し、行ごとのロック・WAL の確認を省く。
        // 既存のトランザクション内なら、そのスナップショットをそのまま使う。
        let _transaction = (rows.len() > 1).then(|| self.conn.unchecked_transaction().ok()).flatten();
        if rows.len() > 16 {
            const BATCH: usize = 64;
            static SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
                format!("SELECT id, input, output FROM texts WHERE id IN ({})",
                    std::iter::repeat_n("?", BATCH).collect::<Vec<_>>().join(","))
            });
            let Ok(mut st) = self.conn.prepare_cached(&SQL) else { return out; };
            for chunk in rows.chunks(BATCH) {
                let ids = chunk.iter().copied().map(Some).chain(std::iter::repeat_n(None, BATCH - chunk.len()));
                let Ok(found) = st.query_map(rusqlite::params_from_iter(ids), |row| {
                    Ok((row.get::<_, i64>(0)?, (unpack(row.get_ref(1)?.as_blob()?), unpack(row.get_ref(2)?.as_blob()?))))
                }) else { continue; };
                out.extend(found.flatten());
            }
            return out;
        }
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

/// 同じ要約だけ共有する。枠には既読イベントの位置を持ち、共有参照を追加しない。
struct SummaryCache([(u64, u32, u32); 1024]);

impl SummaryCache {
    fn new() -> Self { Self([(0, u32::MAX, u32::MAX); 1024]) }

    fn key(s: &str) -> u64 {
        let bytes = s.as_bytes();
        let n = bytes.len();
        let word = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
        let sampled = if n >= 16 {
            word(&bytes[..8]) ^ word(&bytes[8..16]).rotate_left(21) ^ word(&bytes[n - 8..]).rotate_left(43)
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

    fn get<'a>(&mut self, s: &str, next: (usize, usize), at: impl FnOnce(usize, usize) -> Option<&'a Event>) -> Arc<str> {
        if s.len() > 512 { return Arc::from(s); }
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
struct EventGroup { lane_id: i64, events: Vec<Event> }

impl EventGroup {
    fn finish(&mut self) {
        if self.events.len() < self.events.capacity() {
            // realloc の縮小では元の大きな確保が残る場合があるため、実サイズへ移す。
            let mut exact = Vec::with_capacity(self.events.len());
            exact.append(&mut self.events);
            self.events = exact;
        }
    }
}

enum EventRows { Grouped(Vec<EventGroup>), Flat(Vec<(i64, Event)>) }

fn begin_group(groups: &mut Vec<EventGroup>, lane_id: i64, seen: usize, cap: usize) -> Option<Vec<(i64, Event)>> {
    if groups.last().is_some_and(|g| g.lane_id == lane_id) { return None; }
    if let Some(previous) = groups.last_mut() { previous.finish(); }
    if groups.len() >= 64 && seen < groups.len() * 4 {
        // 小さい群が続く入力は、イベントごとの小さな確保を増やさない。
        let mut flat = Vec::with_capacity(cap.max(seen));
        for group in groups.drain(..) {
            flat.extend(group.events.into_iter().map(|e| (group.lane_id, e)));
        }
        return Some(flat);
    }
    groups.push(EventGroup { lane_id, events: Vec::with_capacity(1) });
    None
}

fn read_event(r: &rusqlite::Row<'_>, names: &mut crate::loader::Interner, models: &mut crate::loader::Interner,
              summary: impl FnOnce(&str) -> Arc<str>) -> rusqlite::Result<Event> {
    Ok(Event {
        start: r.get(2)?, end: r.get(3)?, kind: kind_of(r.get(4)?), name: names.get(r.get_ref(5)?.as_str()?),
        summary: summary(r.get_ref(6)?.as_str()?), input: String::new(), output: String::new(),
        call_id: r.get_ref(7)?.as_str_or_null()?.map(Into::into), text_ref: crate::model::TextRef::row(r.get(0)?),
        usage: Usage { input: r.get(8)?, cache_read: r.get(9)?, cache_write: r.get(10)?,
            output: r.get(11)?, reasoning: r.get(12)?, cache_write_1h: r.get(13)? },
        model: r.get_ref(14)?.as_str_or_null()?.map(|m| models.get(m)), status: status_of(r.get(15)?),
    })
}

/// events を ID 範囲ごとに並列で読む。範囲も各範囲内のレーン群も ID 順。
fn read_events(conn: &Connection) -> rusqlite::Result<Vec<EventRows>> {
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
    let parts = (std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16) as i64).min(n);
    let step = (hi - lo) / parts + 1;
    let cap = (n / parts) as usize + 16;
    let p = PathBuf::from(conn.path().ok_or(rusqlite::Error::InvalidPath(PathBuf::new()))?);
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
                        if let Some(mut flat) = begin_group(&mut out, lane_id, seen, cap) {
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
                            flat.shrink_to_fit();
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
        handles.into_iter().map(|h| h.join().expect("events 読み込みスレッドが異常終了")).collect()
    })
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub(crate) struct Fixture { pub index: PathBuf, pub texts: PathBuf, root: PathBuf }
    impl Fixture {
        pub(crate) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!("rireki-write-{}-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            std::fs::create_dir(&root).unwrap();
            Self { index: root.join("index.db"), texts: root.join("texts.db"), root }
        }
        pub(crate) fn open(&self) -> Db { Db::open_at(&self.index, &self.texts).unwrap() }
        pub(crate) fn reject_writes(&self, reject: bool) {
            self.open().conn.execute_batch(if reject {
                "CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
            } else { "DROP TRIGGER reject_event;" }).unwrap();
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); } }

    pub(crate) fn items(values: &[(&str, &str, &str)]) -> Vec<Loaded> {
        let mut lane = Lane::new("main");
        lane.events = values.iter().enumerate().map(|(i, &(name, input, output))| Event {
            start: i as f64, end: i as f64 + 0.5, kind: Kind::Tool, name: name.into(), summary: "summary".into(),
            input: input.into(), output: output.into(), call_id: None, text_ref: Default::default(),
            usage: Usage::default(), model: None, status: Status::Ok,
        }).collect();
        lane.finish();
        vec![Loaded::Session(Session { id: "session".into(), source: Source::Codex, cwd: "/test".into(),
            headless: false, title: "fixture".into(), branch: String::new(), version: String::new(), lanes: vec![lane],
            start: 0.0, end: values.len() as f64, parents: Vec::new(), tree: Vec::new() })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_text_references_preserve_order_and_release_compressed_snapshots() {
        let fixture = fixtures::Fixture::new();
        let mut db = fixture.open();
        let output = "日本語\n".repeat(4096);
        let mut items = fixtures::items(&[("first", "", ""), ("second", "入力\n", &output)]);
        let Loaded::Session(extra) = fixtures::items(&[("other", "other input", "other output")]).remove(0) else { unreachable!() };
        let mut child = extra.lanes[0].clone();
        child.name = "child".into();
        let Loaded::Session(session) = &mut items[0] else { unreachable!() };
        session.lanes.push(extra.lanes[0].clone());
        items.push(Loaded::Child("session".into(), child));
        let packed = pack_files(&[("file".into(), "fp".into(), &items)]);
        let weak = Arc::downgrade(&packed[0].texts);
        packed[0].release_texts(&mut items);
        let mut store = crate::model::Store::default();
        store.replace([("file".into(), items)]);
        let snapshot = store.snapshot();
        let events: Vec<_> = snapshot.sessions[0].lanes.iter().flat_map(|l| &l.events).collect();
        assert_eq!(events.len(), 4);
        let expected = [("", ""), ("入力\n", output.as_str()), ("other input", "other output"), ("other input", "other output")];
        for (e, (i, o)) in events.iter().zip(expected) {
            assert!(e.input.is_empty() && e.output.is_empty());
            let crate::model::EventText::Pending(input, output) = e.text() else { panic!("pending expected") };
            assert_eq!((input.as_str(), output.as_str()), (i, o));
        }
        let bytes = crate::model::pending_text_bytes(events.iter().copied());
        assert_eq!(bytes, packed[0].texts.iter().map(|(i, o)| i.len() + o.len()).sum::<usize>());
        db.put(&packed).unwrap();
        drop(packed);
        assert!(weak.upgrade().is_none(), "旧版のスナップショットも圧縮本文を保持しない");
        assert_eq!(crate::model::pending_text_bytes(events.iter().copied()), 0);
        let reader = Db::open_index_read(&fixture.texts).unwrap();
        let ids: Vec<_> = events.iter().map(|e| match e.text() { crate::model::EventText::Row(id) => id, _ => panic!("row expected") }).collect();
        let mut texts = reader.details(&ids);
        for (id, (i, o)) in ids.iter().zip(expected) {
            let (input, output) = texts.remove(id).unwrap();
            assert_eq!((input.as_str(), output.as_str()), (i, o));
        }
    }

    #[test]
    fn overwritten_snapshots_keep_bodies_and_empty_files_do_not_reuse_ids() {
        use crate::model::EventText;
        let fixture = fixtures::Fixture::new();
        let mut db = fixture.open();
        let reader = Db::open_index_read(&fixture.texts).unwrap();
        let mut versions = Vec::new();
        let mut ids = Vec::new();
        // 空の版を挟んでも、詳細読み込みが取得した旧版の行IDは再利用しない。
        for (fp, values) in [("1", vec![("old", "旧入力", "旧結果")]), ("empty", vec![]),
            ("2", vec![("new", "new input", "new output")])]
        {
            let mut items = fixtures::items(&values);
            let packed = pack_files(&[("file".into(), fp.into(), &items)]);
            let weak = Arc::downgrade(&packed[0].source);
            packed[0].release_texts(&mut items);
            db.put(&packed).unwrap();
            drop(packed);
            let Loaded::Session(session) = items.remove(0) else { unreachable!() };
            if let Some(e) = session.lanes[0].events.first() {
                let EventText::Row(id) = e.text() else { panic!("saved row expected") };
                ids.push(id);
            }
            versions.push((session, weak));
        }
        assert!(ids[1] > ids[0]);
        assert!(reader.details(&[ids[0]]).is_empty());
        let EventText::Pending(input, output) = versions[0].0.lanes[0].events[0].text() else { panic!("old body expected") };
        assert_eq!((input.as_str(), output.as_str()), ("旧入力", "旧結果"));
        assert_eq!(reader.details(&[ids[1]])[&ids[1]], ("new input".into(), "new output".into()));
        assert_eq!(crate::model::pending_text_bytes(&versions[2].0.lanes[0].events), 0);
        let weak = versions[0].1.clone();
        drop(versions);
        assert!(weak.upgrade().is_none(), "writer must not retain superseded snapshots");
    }

    #[test]
    fn failed_overwrite_preserves_both_versions_for_retry() {
        use crate::model::EventText;
        let fixture = fixtures::Fixture::new();
        let mut db = fixture.open();
        let mut old = fixtures::items(&[("old", "old input", "old output")]);
        let first = pack_files(&[("file".into(), "1".into(), &old)]);
        first[0].release_texts(&mut old);
        db.put(&first).unwrap();
        drop(first);
        let mut new = fixtures::items(&[("new", "new input", "new output")]);
        let second = pack_files(&[("file".into(), "2".into(), &new)]);
        second[0].release_texts(&mut new);
        fixture.reject_writes(true);
        assert!(db.put(&second).is_err());
        assert_eq!(db.fingerprints().unwrap()["file"], "1");
        for (items, expected) in [(&old, ("old input", "old output")), (&new, ("new input", "new output"))] {
            let Loaded::Session(s) = &items[0] else { unreachable!() };
            let EventText::Pending(i, o) = s.lanes[0].events[0].text() else { panic!("body expected") };
            assert_eq!((i.as_str(), o.as_str()), expected);
        }
        fixture.reject_writes(false);
        db.put(&second).unwrap();
        assert_eq!(db.fingerprints().unwrap()["file"], "2");
        let Loaded::Session(old) = &old[0] else { unreachable!() };
        assert!(matches!(old.lanes[0].events[0].text(), EventText::Pending(_, _)));
        let Loaded::Session(new) = &new[0] else { unreachable!() };
        assert!(matches!(new.lanes[0].events[0].text(), EventText::Row(_)));
    }

    #[test]
    fn failed_batch_keeps_all_pending_texts_until_retry_commits() {
        let fixture = fixtures::Fixture::new();
        let mut db = fixture.open();
        let mut first = fixtures::items(&[("good", "入力", "結果")]);
        let mut second = fixtures::items(&[("reject", "second", "body")]);
        let packed = pack_files(&[("first".into(), "1".into(), &first), ("second".into(), "2".into(), &second)]);
        packed[0].release_texts(&mut first);
        packed[1].release_texts(&mut second);
        db.conn.execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON events WHEN new.name = 'reject'
            BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
        assert!(db.put(&packed).is_err());
        assert!(db.fingerprints().unwrap().is_empty());
        for (items, expected) in [(&first, ("入力", "結果")), (&second, ("second", "body"))] {
            let Loaded::Session(s) = &items[0] else { unreachable!() };
            let crate::model::EventText::Pending(i, o) = s.lanes[0].events[0].text() else { panic!("rollback must retain pending body") };
            assert_eq!((i.as_str(), o.as_str()), expected);
        }
        db.conn.execute_batch("DROP TRIGGER reject_event;").unwrap();
        db.put(&packed).unwrap();
        assert_eq!(db.fingerprints().unwrap().len(), 2);
        for items in [&first, &second] {
            let Loaded::Session(s) = &items[0] else { unreachable!() };
            assert!(matches!(s.lanes[0].events[0].text(), crate::model::EventText::Row(_)));
        }
    }

    #[test]
    fn summary_cache_preserves_collisions_versions_and_utf8_boundaries() {
        fn add(cache: &mut SummaryCache, events: &mut Vec<EventGroup>, text: &str) -> Arc<str> {
            if events.last().is_none_or(|g| g.events.len() == 7) {
                events.push(EventGroup { lane_id: events.len() as i64, events: Vec::new() });
            }
            let summary = cache.get(text, (events.len() - 1, events.last().unwrap().events.len()),
                |g, i| events.get(g).and_then(|g| g.events.get(i)));
            let Loaded::Session(mut session) = fixtures::items(&[("User", "", "")]).remove(0) else { unreachable!() };
            let mut event = session.lanes[0].events.remove(0);
            event.summary = summary.clone();
            events.last_mut().unwrap().events.push(event);
            summary
        }
        let mut cache = SummaryCache::new();
        let mut events = Vec::new();
        let key = "日本語の要約";
        let slot = |s: &str| SummaryCache::key(s) as usize & 1023;
        let collision = (0..30_000).map(|n| format!("別の要約-{n}"))
            .find(|s| slot(s) == slot(key)).expect("衝突する要約が必要");
        let original = add(&mut cache, &mut events, key);
        assert!(Arc::ptr_eq(&original, &add(&mut cache, &mut events, key)));
        for lane_id in 10..18 { events.push(EventGroup { lane_id, events: Vec::new() }); }
        assert!(Arc::ptr_eq(&original, &add(&mut cache, &mut events, key)));
        let replaced = add(&mut cache, &mut events, &collision);
        assert_eq!(original.as_ref(), key);
        assert_eq!(replaced.as_ref(), collision);
        assert!(Arc::ptr_eq(&replaced, &add(&mut cache, &mut events, &collision)));
        assert_eq!(add(&mut cache, &mut events, key).as_ref(), key);
        assert_eq!(replaced.as_ref(), collision);

        let sampled_a = "0123456789abcdef中間甲abcdefgh";
        let sampled_b = "0123456789abcdef中間乙abcdefgh";
        assert_eq!(SummaryCache::key(sampled_a), SummaryCache::key(sampled_b));
        let a = add(&mut cache, &mut events, sampled_a);
        let b = add(&mut cache, &mut events, sampled_b);
        assert_eq!(a.as_ref(), sampled_a);
        assert_eq!(b.as_ref(), sampled_b);
        assert_eq!(add(&mut cache, &mut events, sampled_a).as_ref(), sampled_a);

        let boundary = "あ".repeat(170) + "ab";
        assert_eq!(boundary.len(), 512);
        let shared = add(&mut cache, &mut events, &boundary);
        assert!(Arc::ptr_eq(&shared, &add(&mut cache, &mut events, &boundary)));
        let longer = boundary + "é";
        let first = add(&mut cache, &mut events, &longer);
        let second = add(&mut cache, &mut events, &longer);
        assert_eq!(first.as_ref(), longer);
        assert_eq!(second.as_ref(), longer);
        assert!(!Arc::ptr_eq(&first, &second));
        for short in ["", "a", "1234567", "12345678", "123456789012345", "1234567890123456", "ééé", "日本"] {
            let value = add(&mut cache, &mut events, short);
            assert!(Arc::ptr_eq(&value, &add(&mut cache, &mut events, short)));
        }
        events.clear();
        assert_eq!(add(&mut cache, &mut events, key).as_ref(), key);
        assert_eq!(std::mem::size_of::<SummaryCache>(), 16 * 1024);
        if usize::BITS > 32 {
            let large = (u32::MAX as u64 + 1) as usize;
            assert_eq!(cache.get("群の位置が大きい", (large, 0), |_, _| None).as_ref(), "群の位置が大きい");
            assert_eq!(cache.get("行の位置が大きい", (0, large), |_, _| None).as_ref(), "行の位置が大きい");
        }
    }

    #[test]
    fn parallel_event_reads_handle_empty_and_sparse_ids() {
        let path = std::env::temp_dir().join(format!("rireki-events-{}-{}.db", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE events (
            id INTEGER PRIMARY KEY, lane_id INTEGER, start REAL, end REAL, kind INTEGER,
            name TEXT, summary TEXT, call_id TEXT, u_in INTEGER, u_cr INTEGER, u_cw INTEGER,
            u_out INTEGER, u_rs INTEGER, u_cw1h INTEGER, u_model TEXT, status INTEGER
        )").unwrap();
        assert!(read_events(&conn).unwrap().is_empty());
        let ids = [-7, 0, 11, 12, 13, 4000, 900000];
        for &id in &ids {
            let name = if id == 13 { "独自ツール" } else { "User" };
            let summary = if id == 12 { "" } else { "日本語" };
            let call_id = match id { 0 => Some(""), 13 => Some("呼び出し-α"), _ => None };
            conn.execute("INSERT INTO events VALUES (?1, 7, ?2, ?3, 0, ?4, ?5, ?6, 1, 2, 3, 4, 5, 6, 'gpt-6-sol', 0)",
                params![id, id as f64, id as f64 + 0.5, name, summary, call_id]).unwrap();
        }
        let parts = read_events(&conn).unwrap();
        assert!(parts.len() <= ids.len());
        let events: Vec<_> = parts.into_iter().flat_map(|part| match part {
            EventRows::Grouped(groups) => groups.into_iter().flat_map(|group|
                group.events.into_iter().map(move |event| (group.lane_id, event))
            ).collect::<Vec<_>>(),
            EventRows::Flat(rows) => rows,
        }).collect();
        assert_eq!(events.len(), ids.len());
        for ((lane, event), id) in events.iter().zip(ids) {
            assert_eq!(*lane, 7);
            assert_eq!(event.row(), Some(id));
            assert_eq!((event.start, event.end), (id as f64, id as f64 + 0.5));
            assert_eq!(event.kind, Kind::User);
            assert_eq!(event.name.as_ref(), if id == 13 { "独自ツール" } else { "User" });
            assert_eq!(event.summary.as_ref(), if id == 12 { "" } else { "日本語" });
            assert_eq!(event.call_id.as_deref(), match id { 0 => Some(""), 13 => Some("呼び出し-α"), _ => None });
            assert_eq!(event.usage, Usage { input: 1, cache_read: 2, cache_write: 3, output: 4, reasoning: 5, cache_write_1h: 6 });
            assert_eq!(event.model.as_deref(), Some("gpt-6-sol"));
        }
        assert!(std::sync::Arc::ptr_eq(&events[0].1.name, &events[1].1.name));
        assert!(Arc::ptr_eq(&events[0].1.summary, &events[1].1.summary));
        drop(conn);
        remove_db(&path);
    }

    #[test]
    fn interleaved_lane_rows_switch_to_flat_reads_and_keep_order() {
        let fixture = fixtures::Fixture::new();
        let mut db = fixture.open();
        let mut items = fixtures::items(&[]);
        let Loaded::Session(session) = &mut items[0] else { unreachable!() };
        let Loaded::Session(sample) = fixtures::items(&[("User", "入力", "結果")]).remove(0) else { unreachable!() };
        for lane_index in 0..2 {
            let mut lane = Lane::new(format!("lane-{lane_index}"));
            for i in 0..2048 {
                let mut event = sample.lanes[0].events[0].clone();
                event.start = i as f64;
                event.end = i as f64 + 0.5;
                event.summary = format!("{lane_index}:{i}").into();
                lane.events.push(event);
            }
            session.lanes.push(lane);
        }
        db.put(&pack_files(&[("file".into(), "fp".into(), &items)])).unwrap();
        db.conn.execute("UPDATE events SET id=-id", []).unwrap();
        db.conn.execute("UPDATE events SET id=CASE WHEN -id <= 2048 THEN (-id-1)*2+1 ELSE (-id-2049)*2+2 END", []).unwrap();
        assert!(read_events(&db.conn).unwrap().iter().all(|part| matches!(part, EventRows::Flat(_))));
        let files = db.load_all().unwrap();
        let Loaded::Session(session) = &files[0].1[0] else { unreachable!() };
        for (lane_index, lane) in session.lanes[1..].iter().enumerate() {
            assert_eq!(lane.events.len(), 2048);
            for (i, event) in lane.events.iter().enumerate() {
                assert_eq!(event.summary.as_ref(), format!("{lane_index}:{i}"));
                assert_eq!(event.row(), Some((i * 2 + lane_index + 1) as i64));
            }
        }
    }

    #[test]
    fn load_all_keeps_lanes_and_event_order_across_sparse_ranges() {
        let fixture = fixtures::Fixture::new();
        let mut db = fixture.open();
        let mut items = fixtures::items(&[]);
        let Loaded::Session(s) = &mut items[0] else { unreachable!() };
        for (lane_index, count) in [37, 19, 0].into_iter().enumerate() {
            let mut lane = Lane::new(format!("lane-{lane_index}"));
            for i in 0..count {
                let mut event = fixtures::items(&[("User", "入力", "")]);
                let Loaded::Session(event) = event.remove(0) else { unreachable!() };
                let mut event = event.lanes[0].events[0].clone();
                event.summary = format!("{lane_index}:{i}").into();
                lane.events.push(event);
            }
            s.lanes.push(lane);
        }
        db.put(&pack_files(&[("file".into(), "fp".into(), &items)])).unwrap();
        // レーンの行を混在させ、0・負値・離れたIDも含む範囲に配置する。
        db.conn.execute("UPDATE events SET id = id + 1000", []).unwrap();
        let mut expected = vec![Vec::new(); 3];
        let mut old = 1001;
        for (lane_index, count) in [37, 19, 0].into_iter().enumerate() {
            for i in 0..count {
                let id = if i == count - 1 { 900000 + lane_index as i64 }
                    else { i as i64 * 2 + lane_index as i64 - 7 };
                db.conn.execute("UPDATE events SET id = ?1 WHERE id = ?2", params![id, old]).unwrap();
                expected[lane_index].push((id, format!("{lane_index}:{i}")));
                old += 1;
            }
        }
        // 孤立したレーンへの参照は、従来と同じく表示の対象にしない。
        db.conn.execute("UPDATE events SET lane_id = -1 WHERE id = -7", []).unwrap();
        expected[0].remove(0);
        let files = db.load_all().unwrap();
        assert_eq!(files.len(), 1);
        let Loaded::Session(s) = &files[0].1[0] else { unreachable!() };
        assert_eq!(s.lanes.len(), 4); // 元の空のmainと3レーン
        assert!(s.lanes[0].events.is_empty());
        for (lane, expected) in s.lanes[1..].iter().zip(expected) {
            let actual: Vec<_> = lane.events.iter().map(|e| (e.row().unwrap(), e.summary.to_string())).collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn detail_reads_preserve_contents_and_release_read_transaction() {
        let db = Db { conn: Connection::open_in_memory().unwrap(), saved: HashMap::new(), last_event_id: 0 };
        db.conn.execute_batch("CREATE TABLE texts (id INTEGER PRIMARY KEY, input BLOB, output BLOB)").unwrap();
        let contents = [
            (-1, "日本語\n入力".to_string(), "結果\n".repeat(1000)),
            (5, String::new(), String::new()),
            (90123, "コマンド".into(), "出力".into()),
        ];
        for (id, input, output) in &contents {
            db.conn.execute("INSERT INTO texts VALUES (?1, ?2, ?3)", params![id, pack(input), pack(output)]).unwrap();
        }
        db.conn.pragma_update(None, "query_only", true).unwrap();
        let expected: HashMap<_, _> = contents.iter().map(|(id, i, o)| (*id, (i.clone(), o.clone()))).collect();
        assert!(db.details(&[]).is_empty());
        assert_eq!(db.details(&[90123, 0, -1, 5, -1]), expected);
        assert!(db.conn.is_autocommit());
        // 呼び出し元が読み取りトランザクションを持つ場合も、終了させない。
        let tx = db.conn.unchecked_transaction().unwrap();
        assert_eq!(db.details(&[5, -1, 90123]), expected);
        assert!(!db.conn.is_autocommit());
        drop(tx);
        assert!(db.conn.is_autocommit());
        assert_eq!(db.details(&[90123, -1, 5]), expected);
    }

    #[test]
    fn detail_batches_preserve_sparse_duplicate_and_missing_rows() {
        let db = Db { conn: Connection::open_in_memory().unwrap(), saved: HashMap::new(), last_event_id: 0 };
        db.conn.execute_batch("CREATE TABLE texts (id INTEGER PRIMARY KEY, input BLOB, output BLOB)").unwrap();
        let mut contents = HashMap::new();
        let mut ids = Vec::new();
        for i in 0..145 {
            let id = i * 97 - 7;
            let input = format!("入力 {i}\n{}", "あ".repeat(i as usize));
            let output = format!("出力 {i}");
            db.conn.execute("INSERT INTO texts VALUES (?1, ?2, ?3)", params![id, pack(&input), pack(&output)]).unwrap();
            contents.insert(id, (input, output));
            ids.push(id);
        }
        db.conn.execute("INSERT INTO texts VALUES (?1, ?2, ?3)", params![9999999, "invalid blob", pack("")]).unwrap();
        db.conn.pragma_update(None, "query_only", true).unwrap();
        for count in [1, 12, 13, 59, 60, 61, 124, 125, 145] {
            let mut selected: Vec<_> = ids[..count].iter().copied().rev().collect();
            selected.extend([9999999, 99999, ids[0], ids[count - 1]]);
            let expected: HashMap<_, _> = selected.iter().filter_map(|id| contents.get(id).map(|texts| (*id, texts.clone()))).collect();
            assert_eq!(db.details(&selected), expected);
            assert!(db.conn.is_autocommit());
        }
    }

    #[test]
    fn pack_roundtrip() {
        for s in ["", "abc", &"日本語テキスト".repeat(500)] {
            assert_eq!(unpack(&pack(s)), s);
        }
    }
}
