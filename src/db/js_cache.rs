//! JS の整形結果の永続キャッシュ。

use super::packing::pack;
use super::{dir, unpack};
use rusqlite::{Connection, params};

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
        let mut st = self
            .conn
            .prepare_cached("SELECT detail FROM calls WHERE hash = ?1")
            .ok()?;
        st.query_row([hash], |r| Ok(unpack(r.get_ref(0)?.as_blob()?)))
            .ok()
    }

    pub fn put(&self, hash: i64, detail: &str) {
        if let Ok(mut st) = self
            .conn
            .prepare_cached("INSERT OR REPLACE INTO calls (hash, detail) VALUES (?1, ?2)")
        {
            let _ = st.execute(params![hash, pack(detail)]);
        }
    }
}
