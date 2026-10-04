//! トラックパッドのピンチ（拡大・縮小）ジェスチャ。
//! GPUI は NSEventTypeMagnify を扱わないため、AppKit のローカルイベントモニタで拾ってチャネルへ流す。

use std::sync::OnceLock;

use async_channel::{Receiver, Sender};

static TX: OnceLock<Sender<f64>> = OnceLock::new();

/// モニタを登録し、拡大率の差分（正で拡大）を受け取るチャネルを返す
pub fn listen() -> Receiver<f64> {
    let (tx, rx) = async_channel::unbounded();
    if TX.set(tx).is_ok() {
        install();
    }
    rx
}

#[cfg(target_os = "macos")]
fn install() {
    use block::ConcreteBlock;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    const NS_EVENT_MASK_MAGNIFY: u64 = 1 << 30;
    let handler = ConcreteBlock::new(|event: *mut Object| -> *mut Object {
        let m: f64 = unsafe { msg_send![event, magnification] };
        if let Some(tx) = TX.get() {
            let _ = tx.try_send(m);
        }
        event
    })
    .copy();
    unsafe {
        let _: *mut Object = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: NS_EVENT_MASK_MAGNIFY handler: &*handler];
    }
    // モニタはアプリ終了まで生かす
    std::mem::forget(handler);
}

#[cfg(not(target_os = "macos"))]
fn install() {}
