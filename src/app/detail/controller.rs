//! 詳細一覧の絞り込み、並び順、イベント移動と末尾への追従。

use super::*;

impl Rireki {
    pub(in crate::app) fn detail_uses_list(&self) -> bool {
        self.shown_detail_selection()
            .filter(|&sel| sel.first < sel.last)
            .is_some_and(|_| self.detail_tab == DetailTab::Events)
    }

    pub(in crate::app) fn detail_at_bottom(&self) -> bool {
        if self.detail_tab == DetailTab::Files
            && self
                .detail_cache
                .as_ref()
                .is_some_and(|d| !d.files.is_empty())
        {
            self.file_list.at_bottom()
        } else if self.detail_uses_list() {
            self.detail_list.at_bottom()
        } else {
            -self.detail_scroll.offset().y >= self.detail_scroll.max_offset().height - px(4.0)
        }
    }

    pub(in crate::app) fn detail_at_latest(&self) -> bool {
        if self.detail_uses_list() {
            self.detail_list.at_latest(self.detail_reversed)
        } else {
            self.detail_at_bottom()
        }
    }

    /// 未計測の行も少しずつ描きながら追う。ユーザー操作・絞り込み・選択変更で止める
    pub(in crate::app) fn animate_list_to_latest(&mut self, cx: &mut Context<Self>) {
        const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
        const MAX_FRAMES: usize = 40;
        let reversed = self.detail_reversed;
        self.detail_following = true;
        self.follow_anim = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FRAME).await;
            for frame in 0..MAX_FRAMES {
                let Ok(keep_going) = this.update(cx, |app, cx| {
                    if !app.detail_uses_list() || app.detail_reversed != reversed {
                        return None;
                    }
                    let list = &app.detail_list.state;
                    let offset = list.logical_scroll_top();
                    if reversed && offset.item_ix == 0 && offset.offset_in_item <= px(1.0) {
                        app.detail_list.scroll_to_latest(true);
                        app.detail_changed(cx);
                        return None;
                    }
                    if !reversed && app.detail_list.at_bottom() {
                        return None;
                    }
                    if frame + 1 == MAX_FRAMES {
                        app.detail_list.scroll_to_latest(reversed);
                    } else if reversed {
                        // 上端へ近づくほど減速する。移動量を先読み範囲内に抑え、未計測の行を飛ばさない。
                        let distance = (-list.scroll_px_offset_for_scrollbar().y * 0.2)
                            .clamp(px(1.0), px(100.0));
                        list.scroll_by(-distance);
                    } else {
                        let bounds = list.viewport_bounds();
                        let distance = list
                            .item_count()
                            .checked_sub(1)
                            .and_then(|i| list.bounds_for_item(i))
                            .map(|last| ((last.bottom() - bounds.bottom()) * 0.2).max(px(1.0)))
                            .unwrap_or_else(|| {
                                (bounds.size.height * 0.15).clamp(px(40.0), px(100.0))
                            });
                        list.scroll_by(distance);
                    }
                    app.detail_changed(cx);
                    Some(())
                }) else {
                    return;
                };
                if keep_going.is_none() {
                    break;
                }
                cx.background_executor().timer(FRAME).await;
            }
            let _ = this.update(cx, |app, _| app.detail_following = false);
        }));
    }

    pub(in crate::app) fn cancel_detail_follow(&mut self) {
        self.detail_follow = false;
        self.detail_following = false;
        self.follow_anim = None;
    }

    pub(in crate::app) fn set_row_filter(
        &mut self,
        filter: Option<RowFilter>,
        cx: &mut Context<Self>,
    ) {
        self.row_filter = filter;
        self.cancel_detail_follow();
        self.detail_changed(cx);
    }

    pub(super) fn set_detail_order(&mut self, reversed: bool, cx: &mut Context<Self>) {
        if self.detail_reversed == reversed {
            return;
        }
        let following = self.replay.as_ref().is_some_and(|replay| replay.following)
            || self.detail_list.at_latest(self.detail_reversed);
        self.detail_reversed = reversed;
        if let Some(replay) = &mut self.replay {
            replay.following = following;
            replay.check_follow = false;
            replay.scroll_pending.set(following);
        }
        self.cancel_detail_follow();
        self.persist();
        self.detail_changed(cx);
    }

    /// 表示順で同じレーンの次（`dir` = 1）/ 前（-1）を選ぶ。複数を選んでいるときは、表示上の先頭 / 末尾を選ぶ。
    /// 1 イベントずつ進むときは履歴に積まない（「戻る」で一覧に戻れるようにする）
    pub(in crate::app) fn step_event(
        &mut self,
        dir: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cur) = self.selected.filter(|&s| self.valid(s)) else {
            return;
        };
        let n = self.store.sessions[cur.session].lanes[cur.lane]
            .events
            .len();
        let dir = if self.detail_reversed { -dir } else { dir };
        let i = if cur.first != cur.last {
            if dir > 0 { cur.first } else { cur.last }
        } else {
            match cur.first.checked_add_signed(dir).filter(|&i| i < n) {
                Some(i) => i,
                None => return,
            }
        };
        let sel = Selection {
            session: cur.session,
            lane: cur.lane,
            first: i,
            last: i,
        };
        if cur.first != cur.last {
            self.navigate(sel, window, cx);
        } else {
            self.select(sel, cx);
            self.set_detail_tab(DetailTab::Events, cx);
            cx.notify();
        }
        self.reveal(sel, false, window);
    }
}
