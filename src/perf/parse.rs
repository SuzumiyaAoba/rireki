//! 固定したログで、更新のたびに行う全ファイル解析と本文の破棄を測る。

use std::path::Path;

use crate::loader::JobKind;

pub(super) fn benchmark(manifest: &Path) {
    #[cfg(target_os = "macos")]
    let measure_memory = std::env::args().any(|a| a == "--measure-memory");
    let tasks = super::ingest_tasks(manifest);
    let mut samples: Vec<_> = tasks.iter().filter_map(|task| {
        std::fs::metadata(&task.job.path).ok().map(|m| (m.len(), &task.job))
    }).collect();
    samples.sort_by_key(|&(bytes, _)| std::cmp::Reverse(bytes));
    let mut selected: Vec<_> = samples.iter().filter(|(_, job)| job.kind == JobKind::Codex)
        .take(3).copied().collect();
    for kind in [JobKind::Claude, JobKind::ClaudeLegacyAgent] {
        if let Some(sample) = samples.iter().find(|(_, job)| job.kind == kind) {
            selected.push(*sample);
        }
    }
    for (sample, (bytes, job)) in selected.into_iter().enumerate() {
        #[cfg(target_os = "macos")]
        let memory = measure_memory.then(super::memory::Monitor::start);
        let items = crate::loader::parse(job);
        #[cfg(target_os = "macos")]
        let memory = memory.map(super::memory::Monitor::finish);
        let events: usize = items.iter().map(|item| match item {
            crate::model::Loaded::Session(s) => s.lanes.iter().map(|l| l.events.len()).sum(),
            crate::model::Loaded::Child(_, lane) => lane.events.len(),
        }).sum();
        println!("{}", serde_json::json!({"sample":sample, "kind":format!("{:?}",job.kind),
            "bytes":bytes, "items":items.len(), "events":events,
            "content_hash":format!("{:016x}", super::items_hash(&items))}));
        #[cfg(target_os = "macos")]
        if let Some(memory) = memory {
            println!("{}", serde_json::json!({"sample":sample,"memory":memory}));
        }
        drop(items);
        #[cfg(target_os = "macos")]
        let repeated_memory = measure_memory.then(super::memory::Monitor::start);
        super::measure(&format!("parse_sample_{sample}"), 7, || crate::loader::parse(job));
        #[cfg(target_os = "macos")]
        if measure_memory {
            println!("{}", serde_json::json!({"sample":sample,"repetitions_memory":repeated_memory.unwrap().finish()}));
            let (live, reserved) = super::memory::current();
            println!("{}", serde_json::json!({"sample":sample,"live_after_bytes":live,"reserved_after_bytes":reserved}));
        }
    }
}
