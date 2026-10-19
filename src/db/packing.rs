//! 保存前のファイル単位の本文圧縮と保存状態の共有。

use crate::model::{Event, Lane, Loaded, Session};
use rayon::prelude::*;
use std::sync::Arc;

pub(super) fn pack(s: &str) -> Vec<u8> {
    if s.is_empty() {
        Vec::new()
    } else {
        lz4_flex::compress_prepend_size(s.as_bytes())
    }
}

pub(crate) fn unpack(b: &[u8]) -> String {
    if b.is_empty() {
        return String::new();
    }
    lz4_flex::decompress_size_prepended(b)
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
        .unwrap_or_default()
}

/// 書き込み用に用意した 1 ファイル分のデータ。本文は圧縮済みで、items 側の input / output は空
pub struct Packed {
    pub path: String,
    pub fingerprint: String,
    pub items: Vec<Loaded>,
    /// イベントの出現順の (input, output)
    pub texts: Arc<crate::text::Compressed>,
    pub(super) source: Arc<crate::text::Texts>,
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
    files
        .par_iter()
        .map(|(path, fp, items)| pack_file(path, fp, items))
        .collect()
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
                texts.push(
                    e.text_ref
                        .compressed()
                        .unwrap_or_else(|| (pack(&e.input), pack(&e.output))),
                );
                Event {
                    input: String::new(),
                    output: String::new(),
                    ..e.clone_meta()
                }
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
    Packed {
        path: path.to_owned(),
        fingerprint: fp.to_owned(),
        items,
        texts,
        source,
    }
}
