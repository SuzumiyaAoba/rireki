//! 読み取り専用の索引と詳細本文の計測。

use super::{items_hash, measure};
use crate::model::Store;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

/// 固定した索引DBの内容ハッシュと、再読み込み・結果の破棄を計測する。
pub fn db_load(index: &Path) {
    let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
    let files = db.load_all().unwrap();
    let mut contents = std::collections::BTreeMap::new();
    for (path, items) in &files {
        contents.insert(path, items_hash(items));
    }
    let mut hash = DefaultHasher::new();
    contents.hash(&mut hash);
    println!(
        "{}",
        serde_json::json!({"files":files.len(), "content_hash":format!("{:016x}",hash.finish())})
    );
    drop(files);
    measure("db_load", 15, || db.load_all().unwrap());
}

/// 詳細パネルと同じ行IDで本文を読み、内容を出さずにハッシュと所要時間を比較する。
pub fn details(index: &Path, texts: &Path) {
    let db = crate::db::Db::open_index_read(index).expect("計測対象の索引 DB を開けません");
    let mut store = Store::default();
    store.replace(db.load_all().unwrap());
    let session = store
        .sessions
        .iter()
        .max_by_key(|s| s.lanes.iter().map(|l| l.events.len()).sum::<usize>())
        .unwrap();
    let lane = session.lanes.iter().max_by_key(|l| l.events.len()).unwrap();
    let reader = crate::db::Db::open_index_read(texts).expect("計測対象の本文 DB を開けません");
    for (name, rows) in [
        (
            "detail_texts_single",
            lane.events
                .iter()
                .take(1)
                .filter_map(|e| e.row())
                .collect::<Vec<_>>(),
        ),
        (
            "detail_texts_300",
            lane.events
                .iter()
                .take(crate::detail::EVENT_LIMIT)
                .filter_map(|e| e.row())
                .collect::<Vec<_>>(),
        ),
        (
            "detail_texts_selection",
            lane.events
                .iter()
                .enumerate()
                .filter(|(i, e)| {
                    *i < crate::detail::EVENT_LIMIT
                        || ["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"]
                            .contains(&e.name.as_ref())
                })
                .filter_map(|(_, e)| e.row())
                .collect::<Vec<_>>(),
        ),
    ] {
        let fetched = reader.details(&rows);
        assert_eq!(fetched.len(), rows.len());
        let mut contents: Vec<_> = fetched.iter().collect();
        contents.sort_unstable_by_key(|&(id, _)| id);
        let mut hash = DefaultHasher::new();
        contents.hash(&mut hash);
        println!(
            "{}",
            serde_json::json!({"case":name, "rows":rows.len(),
            "bytes":fetched.values().map(|(i,o)| i.len()+o.len()).sum::<usize>(),
            "content_hash":format!("{:016x}",hash.finish())})
        );
        measure(name, 31, || reader.details(&rows));
    }
}
