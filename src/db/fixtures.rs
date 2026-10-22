//! DB の保存・読み取りテストで共有する隔離された保存先。

use super::*;
use crate::model::{Event, Kind, Lane, Loaded, Session, Source, Status, Usage};

pub(crate) struct Fixture {
    pub index: PathBuf,
    pub texts: PathBuf,
    root: PathBuf,
}
impl Fixture {
    pub(crate) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rireki-write-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self {
            index: root.join("index.db"),
            texts: root.join("texts.db"),
            root,
        }
    }
    pub(crate) fn open(&self) -> Db {
        Db::open_at(&self.index, &self.texts).unwrap()
    }
    pub(crate) fn reject_writes(&self, reject: bool) {
        self.open().conn.execute_batch(if reject {
            "CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
        } else { "DROP TRIGGER reject_event;" }).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn items(values: &[(&str, &str, &str)]) -> Vec<Loaded> {
    let mut lane = Lane::new("main");
    lane.events = values
        .iter()
        .enumerate()
        .map(|(i, &(name, input, output))| Event {
            start: i as f64,
            end: i as f64 + 0.5,
            kind: Kind::Tool,
            name: name.into(),
            summary: "summary".into(),
            input: input.into(),
            output: output.into(),
            call_id: None,
            text_ref: Default::default(),
            usage: Usage::default(),
            model: None,
            status: Status::Ok,
        })
        .collect();
    lane.finish();
    vec![Loaded::Session(Session {
        id: "session".into(),
        source: Source::Codex,
        cwd: "/test".into(),
        headless: false,
        title: "fixture".into(),
        branch: String::new(),
        version: String::new(),
        lanes: vec![lane],
        start: 0.0,
        end: values.len() as f64,
        parents: Vec::new(),
        tree: Vec::new(),
    })]
}
