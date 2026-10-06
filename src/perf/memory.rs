//! macOSのmalloc統計を使い、RSSとは別に取り込み中の使用ヒープを観測する。

use std::ffi::c_void;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

#[repr(C)]
#[derive(Default)]
struct Usage {
    blocks: u32,
    live: usize,
    high_water: usize,
    reserved: usize,
}

unsafe extern "C" {
    fn malloc_zone_statistics(zone: *mut c_void, stats: *mut Usage);
}

fn usage() -> Usage {
    let mut stats = Usage::default();
    // malloc/malloc.hのABI。nullのzoneは全zoneの統計を集める。
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut stats); }
    stats
}

pub(crate) fn current() -> (usize, usize) {
    let stats = usage();
    (stats.live, stats.reserved)
}

#[derive(Default)]
struct Peaks {
    live: AtomicUsize,
    reserved: AtomicUsize,
}

impl Peaks {
    fn sample(&self) {
        let stats = usage();
        self.live.fetch_max(stats.live, Ordering::Relaxed);
        self.reserved.fetch_max(stats.reserved, Ordering::Relaxed);
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Measurement {
    start_live_bytes: usize,
    peak_live_bytes: usize,
    end_live_bytes: usize,
    peak_reserved_bytes: usize,
}

pub(crate) struct Monitor {
    running: Arc<AtomicBool>,
    peaks: Arc<Peaks>,
    handle: Option<JoinHandle<()>>,
    start_live: usize,
}

impl Monitor {
    pub(crate) fn start() -> Self {
        let start_live = usage().live;
        let peaks = Arc::new(Peaks::default());
        let running = Arc::new(AtomicBool::new(true));
        let worker_peaks = peaks.clone();
        let worker_running = running.clone();
        let handle = std::thread::spawn(move || {
            while worker_running.load(Ordering::Relaxed) {
                worker_peaks.sample();
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Self { running, peaks, handle: Some(handle), start_live }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() { let _ = handle.join(); }
    }

    pub(crate) fn finish(mut self) -> Measurement {
        self.peaks.sample();
        self.stop();
        Measurement {
            start_live_bytes: self.start_live,
            peak_live_bytes: self.peaks.live.load(Ordering::Relaxed),
            end_live_bytes: usage().live,
            peak_reserved_bytes: self.peaks.reserved.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Monitor { fn drop(&mut self) { self.stop(); } }

pub fn db(index: &Path) {
    let db = crate::db::Db::open_index_read(index).expect("計測対象の索引DBを開けません");
    let start = usage();
    println!("{}", serde_json::json!({"event_bytes":std::mem::size_of::<crate::model::Event>(),
        "start_live_bytes":start.live, "start_reserved_bytes":start.reserved}));
    for step in 0..15 {
        let files = db.load_all().unwrap();
        let live = usage();
        let count = files.len();
        drop(files);
        let freed = usage();
        println!("{}", serde_json::json!({"step":step,"files":count,
            "live_bytes":live.live,"live_blocks":live.blocks,"reserved_bytes":live.reserved,
            "freed_live_bytes":freed.live,"freed_blocks":freed.blocks,"freed_reserved_bytes":freed.reserved}));
    }
}
