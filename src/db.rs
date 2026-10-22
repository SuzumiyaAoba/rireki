//! 安定した（更新の止まった）履歴を保存する索引 DB (SQLite)
//!
//! JSONL をパースした結果をファイル単位で保存し、起動時はここから読む。
//! 描画に必要な列だけを一括で読み、input / output は詳細表示のときに行 ID で取り出す。

use rusqlite::Connection;
use std::collections::HashMap;
use std::path::PathBuf;

mod codec;
mod connection;
mod events;
#[cfg(test)]
pub(crate) mod fixtures;
mod js_cache;
mod packing;
mod read;
#[cfg(test)]
mod tests;
mod write;

pub use js_cache::JsCache;
pub use packing::{Packed, pack_files};
pub(crate) use packing::{pack_file, unpack};

pub struct Db {
    conn: Connection,
    saved: HashMap<String, std::sync::Weak<crate::text::Texts>>,
    last_event_id: i64,
}

fn dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RIREKI_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let base =
        dirs::cache_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"));
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
