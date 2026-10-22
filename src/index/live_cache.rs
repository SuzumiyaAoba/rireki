//! ライブ解析状態の保持量を管理する。キャッシュ外へ出した状態は保持量から除く。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::loader::{LiveLog, LiveStats};

/// 生の解析状態を保持する上限。古いものから破棄し、次回は全体解析に戻す。
pub(super) const LIVE_CACHE_BYTES: usize = 64 * 1024 * 1024;

struct CachedLive {
    log: LiveLog,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
pub(super) struct LiveCache {
    parsers: HashMap<String, CachedLive>,
    bytes: usize,
    tick: u64,
}

impl LiveCache {
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(super) fn contains(&self, origin: &str) -> bool {
        self.parsers.contains_key(origin)
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.parsers.is_empty()
    }

    pub(super) fn stats(&self, origin: &str) -> Option<LiveStats> {
        self.parsers
            .get(origin)
            .map(|cache| cache.log.last_update())
    }

    pub(super) fn take(&mut self, origin: &str) -> Option<LiveLog> {
        self.parsers.remove(origin).map(|cache| {
            self.bytes -= cache.bytes;
            cache.log
        })
    }

    /// 探索結果から消えたファイルの解析状態を解放する。
    pub(super) fn retain(&mut self, origins: &HashSet<String>) {
        self.parsers.retain(|key, cache| {
            if origins.contains(key) {
                true
            } else {
                self.bytes -= cache.bytes;
                false
            }
        });
    }

    pub(super) fn insert(&mut self, origin: String, log: LiveLog, bytes: usize) {
        if let Some(old) = self.parsers.remove(&origin) {
            self.bytes -= old.bytes;
        }
        if bytes > LIVE_CACHE_BYTES {
            return;
        }
        while self.bytes + bytes > LIVE_CACHE_BYTES {
            let Some(oldest) = self
                .parsers
                .iter()
                .min_by_key(|(_, c)| c.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.bytes -= self.parsers.remove(&oldest).unwrap().bytes;
        }
        self.tick = self.tick.wrapping_add(1);
        self.parsers.insert(
            origin,
            CachedLive {
                log,
                bytes,
                used: self.tick,
            },
        );
        self.bytes += bytes;
    }
}

/// 並列ワーカーから回収する途中の解析状態にも、キャッシュとは別の上限を設ける。
#[derive(Default)]
pub(super) struct BatchBudget(AtomicUsize);

impl BatchBudget {
    pub(super) fn retain(&self, log: LiveLog) -> Option<(LiveLog, usize)> {
        let bytes = log.retained_bytes();
        self.0
            .try_update(Relaxed, Relaxed, |held| {
                held.checked_add(bytes)
                    .filter(|total| *total <= LIVE_CACHE_BYTES)
            })
            .ok()
            .map(|_| (log, bytes))
    }
}
