//! 容量制限付きの保存待ち行列と、専用スレッドの開始・完了通知・終了。

use std::sync::mpsc;
use std::thread::JoinHandle;

use crate::db::{Db, Packed};

pub(super) struct Completion {
    pub(super) files: Vec<(String, String)>,
    pub(super) saved: bool,
}

pub(super) struct Writer {
    pub(super) sender: mpsc::SyncSender<Vec<Packed>>,
    pub(super) completed: mpsc::Receiver<Completion>,
    pub(super) handle: JoinHandle<()>,
}

impl Writer {
    pub(super) fn start(mut db: Db) -> Self {
        // 保存がパースに追いつかない場合も、待ち行列に大量の本文を溜めない。
        let (sender, receiver) = mpsc::sync_channel::<Vec<Packed>>(1);
        let (done, completed) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            for batch in receiver {
                let result = db.put(&batch);
                if let Err(e) = &result {
                    eprintln!("rireki: 索引 DB への保存に失敗: {e}");
                }
                let _ = done.send(Completion {
                    files: batch
                        .iter()
                        .map(|p| (p.path.clone(), p.fingerprint.clone()))
                        .collect(),
                    saved: result.is_ok(),
                });
            }
        });
        Self {
            sender,
            completed,
            handle,
        }
    }

    /// 送信側を閉じ、受信済みの保存をすべて終えてからスレッドを回収する。
    pub(super) fn finish(self) {
        drop(self.sender);
        let _ = self.handle.join();
    }
}
