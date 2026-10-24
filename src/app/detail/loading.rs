//! 詳細本文の読み込みタスクとキャッシュの更新。

use super::{Context, DetailTab, Rireki, Selection, invalidation, is_live};
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
        self.detail_live.set(None);
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
        // 更新中は旧版を表示し続ける。読み込み開始だけでは本文を無効化しない。
        if self.detail_cache.is_none() {
            self.detail_changed(cx);
        }
        let generation = self.detail_gen.load(Ordering::Relaxed);
        let gen_counter = self.detail_gen.clone();
        let loader = self.detail_loader.clone();
        let session = self.store.sessions[sel.session].clone();
        let previous = self.detail_cache.as_ref().filter(|cache| cache.sel == sel)
            .and(self.detail_snapshot.as_ref()).map(|(session, _)| session.clone());
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
                        .map(|cache| {
                            let same_selection = previous.as_ref()
                                .is_some_and(|before| invalidation::same_selection(before, &session, sel));
                            (cache, session, same_selection)
                        })
                })
                .await;
            let Some((cache, session, same_selection)) = loaded else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                if app.detail_gen.load(Ordering::Relaxed) == generation
                    && app.selected == Some(sel)
                    && app.valid(sel)
                {
                    let content_changed = !same_selection || app.detail_cache.as_ref()
                        .is_none_or(|before| !invalidation::same_content(before, &cache));
                    let context_changed = app.detail_snapshot.as_ref().is_none_or(|(before, _)| {
                        before.cwd != session.cwd || before.id != session.id
                            || before.source != session.source || app.detail_live.get() != Some(is_live(session.end))
                    });
                    if content_changed {
                        app.detail_selection_gen = generation;
                    }
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
                    if content_changed || context_changed || app.detail_tab == DetailTab::Session {
                        app.detail_changed(cx);
                    } else {
                        app.detail_header_changed(cx);
                    }
                    if std::mem::take(&mut app.detail_follow) && app.detail_tab == DetailTab::Events
                    {
                        app.animate_to_latest(cx);
                    }
                }
            });
        }));
    }
}
