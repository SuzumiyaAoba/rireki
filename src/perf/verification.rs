//! 既存ログの読み取り専用サンプルと解析結果の照合。

use crate::model::{Lane, Loaded};
use std::hash::{DefaultHasher, Hash, Hasher};

pub(super) fn log_sample() -> Vec<(u64, crate::loader::Job)> {
    let mut sample: Vec<_> = crate::loader::discover()
        .into_iter()
        .filter_map(|job| std::fs::metadata(&job.path).ok().map(|m| (m.len(), job)))
        .collect();
    sample.sort_by_key(|&(bytes, _)| std::cmp::Reverse(bytes));
    sample.truncate(3);
    sample
}

/// 解析結果の全内容を比較できるよう、本文やパスを出さずに決定的なハッシュを返す
pub fn verify_logs() {
    let mut sample = log_sample();
    let jobs = crate::loader::discover();
    for kind in [
        crate::loader::JobKind::Claude,
        crate::loader::JobKind::ClaudeLegacyAgent,
    ] {
        if let Some(job) = jobs
            .iter()
            .filter(|j| j.kind == kind)
            .max_by_key(|j| std::fs::metadata(&j.path).map_or(0, |m| m.len()))
        {
            sample.push((
                std::fs::metadata(&job.path).map_or(0, |m| m.len()),
                job.clone(),
            ));
        }
    }
    for (i, (bytes, job)) in sample.into_iter().enumerate() {
        let items = crate::loader::parse(&job);
        let mut hash = DefaultHasher::new();
        let mut events = 0;
        let mut hash_lane = |lane: &Lane| {
            (&lane.name, &lane.spawned_by, &lane.model, &lane.effort).hash(&mut hash);
            lane.events.len().hash(&mut hash);
            for e in &lane.events {
                (
                    e.start.to_bits(),
                    e.end.to_bits(),
                    e.kind as u8,
                    e.status as u8,
                )
                    .hash(&mut hash);
                (
                    &e.name,
                    &e.summary,
                    &e.input,
                    &e.output,
                    &e.call_id,
                    &e.row(),
                    &e.model,
                )
                    .hash(&mut hash);
                (
                    e.usage.input,
                    e.usage.cache_read,
                    e.usage.cache_write,
                    e.usage.cache_write_1h,
                    e.usage.output,
                    e.usage.reasoning,
                )
                    .hash(&mut hash);
            }
            events += lane.events.len();
        };
        // レーンのハッシュとメタデータのハッシュを別々に作り、借用と順序を固定する
        let mut meta = DefaultHasher::new();
        items.len().hash(&mut meta);
        for item in &items {
            match item {
                Loaded::Session(s) => {
                    (
                        0u8,
                        &s.id,
                        s.source as u8,
                        &s.cwd,
                        s.headless,
                        &s.title,
                        &s.branch,
                        &s.version,
                    )
                        .hash(&mut meta);
                    s.lanes.len().hash(&mut meta);
                    for lane in &s.lanes {
                        hash_lane(lane);
                    }
                }
                Loaded::Child(parent, lane) => {
                    (1u8, parent).hash(&mut meta);
                    hash_lane(lane);
                }
            }
        }
        println!(
            "{}",
            serde_json::json!({"sample":i,"kind":format!("{:?}",job.kind),"bytes":bytes,"items":items.len(),"events":events,
            "metadata_hash":format!("{:016x}",meta.finish()),"event_hash":format!("{:016x}",hash.finish())})
        );
    }
}
