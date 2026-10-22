//! DB の接続、保存先の初期化とスキーマの作成。

use super::{Db, path, texts_path};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;

/// スキーマやパース結果の形式を変えたら上げる（古い DB は作り直す）
const VERSION: i64 = 16;

/// DB ファイルを WAL / 共有メモリごと消す
pub(super) fn remove_db(p: &std::path::Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut s = p.as_os_str().to_owned();
        s.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(s));
    }
}

pub(super) fn read_conn(p: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        p,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "mmap_size", 1i64 << 30)?;
    Ok(conn)
}

impl Db {
    /// 計測用の索引を読み取り専用で開く。スキーマや実データは変更しない
    pub fn open_index_read(p: &std::path::Path) -> rusqlite::Result<Self> {
        Ok(Self {
            conn: read_conn(p)?,
            saved: HashMap::new(),
            last_event_id: 0,
        })
    }

    /// 書き込み用に開く。必要ならスキーマを作り直す
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(&path(), &texts_path())
    }

    pub(super) fn open_at(p: &std::path::Path, texts: &std::path::Path) -> rusqlite::Result<Self> {
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
        Ok(Self {
            conn,
            saved: HashMap::new(),
            last_event_id: 0,
        })
    }

    /// 詳細本文を引くための読み取り専用接続
    pub fn open_read() -> rusqlite::Result<Self> {
        Ok(Self {
            conn: read_conn(&texts_path())?,
            saved: HashMap::new(),
            last_event_id: 0,
        })
    }
}
