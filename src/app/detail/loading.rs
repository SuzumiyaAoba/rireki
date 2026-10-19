//! 詳細本文の読み込みタスクとキャッシュの更新。

use super::{Context, DetailTab, Rireki, Selection};
use std::sync::atomic::Ordering;

impl Rireki {
    pub(in crate::app) fn cancel_detail_load(&mut self) {
        self.reset_replay();
        self.detail_gen.fetch_add(1, Ordering::Relaxed);
        self.detail_task = None;
        self.cancel_detail_follow();
    }

    /// 大きな編集履歴と旧版セッションの解放で、選択や閉じる操作を待たせない。
    pub(in crate::app) fn clear_detail_cache(&mut self, cx: &mut Context<Self>) {
        let old = (self.detail_cache.take(), self.detail_snapshot.take());
        if old.0.is_some() || old.1.is_some() {
            cx.background_executor()
                .spawn(async move {
                    drop(old);
                })
                .detach();
        }
    }

    /// DB 読み込みと JS のモック実行を背景で行い、最新の選択だけを反映する
    pub(in crate::app) fn request_detail(&mut self, sel: Selection, cx: &mut Context<Self>) {
        self.clear_detail_cache(cx);
        self.refresh_detail(sel, cx);
    }

    /// 同じ選択の更新では表示中の詳細を保持し、セッションの版ごと差し替える
    pub(in crate::app) fn refresh_detail(&mut self, sel: Selection, cx: &mut Context<Self>) {
        self.cancel_detail_load();
        self.detail_changed(cx);
        let generation = self.detail_gen.load(Ordering::Relaxed);
        let gen_counter = self.detail_gen.clone();
        let loader = self.detail_loader.clone();
        let session = self.store.sessions[sel.session].clone();
        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    let is_current = || gen_counter.load(Ordering::Relaxed) == generation;
                    if !is_current() {
                        return None;
                    }
                    let mut loader = loader.lock().unwrap();
                    if !is_current() {
                        return None;
                    }
                    loader.prepare();
                    loader
                        .load_session(sel, &session, is_current)
                        .map(|cache| (cache, session))
                })
                .await;
            let Some((cache, session)) = loaded else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                if app.detail_gen.load(Ordering::Relaxed) == generation
                    && app.selected == Some(sel)
                    && app.valid(sel)
                {
                    let old_cache = app.detail_cache.replace(cache);
                    let old_snapshot = app.detail_snapshot.replace((session, generation));
                    cx.background_executor()
                        .spawn(async move {
                            drop((old_cache, old_snapshot));
                        })
                        .detach();
                    app.request_analysis(cx);
                    if app.replay_autostart.take() == Some(sel)
                        && app.detail_tab == DetailTab::Events
                        && !app.statistics_open
                    {
                        app.begin_replay(sel, cx);
                    }
                    app.detail_changed(cx);
                    if std::mem::take(&mut app.detail_follow) && app.detail_tab == DetailTab::Events
                    {
                        app.animate_to_latest(cx);
                    }
                }
            });
        }));
    }
}
