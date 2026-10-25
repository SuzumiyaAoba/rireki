//! 詳細の選択、閲覧履歴、選択イベントへの移動。

use super::{
    Context, DetailTab, Duration, Mode, ROW_H, Rireki, Row, RowFilter, Selection, Source,
    SourceFilter, Window, calendar, local, midnight, point, px,
};
use chrono::Datelike;

impl Rireki {
    /// 表示順に応じて最新のイベント側へ移る。途中でユーザーがスクロールしたら止める
    pub(super) fn animate_to_latest(&mut self, cx: &mut Context<Self>) {
        if self.detail_uses_list() {
            self.animate_list_to_latest(cx);
            return;
        }
        const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
        const MAX_FRAMES: usize = 40;
        let h = self.detail_scroll.clone();
        self.detail_following = true;
        self.follow_anim = Some(cx.spawn(async move |this, cx| {
            // 増えたイベントがレイアウトされ、max_offset が更新されるのを待つ
            cx.background_executor().timer(FRAME).await;
            let mut last = h.offset().y;
            for i in 0..MAX_FRAMES {
                let cur = h.offset().y;
                if (cur - last).abs() > px(1.0) {
                    break;
                }
                let target = -h.max_offset().height;
                let gap = target - cur;
                let next = if gap.abs() < px(1.0) || i + 1 == MAX_FRAMES {
                    target
                } else {
                    cur + gap * 0.2
                };
                h.set_offset(gpui::point(h.offset().x, next));
                last = next;
                if this.update(cx, |t, cx| t.detail_changed(cx)).is_err() || next == target {
                    break;
                }
                cx.background_executor().timer(FRAME).await;
            }
            let _ = this.update(cx, |app, _| app.detail_following = false);
        }));
    }

    // ------------------------------------------------------------ 選択

    pub(super) fn valid(&self, sel: Selection) -> bool {
        self.store
            .sessions
            .get(sel.session)
            .and_then(|s| s.lanes.get(sel.lane))
            .is_some_and(|l| sel.first <= sel.last && sel.last < l.events.len())
    }

    pub(super) fn select(&mut self, sel: Selection, cx: &mut Context<Self>) {
        if self.selected == Some(sel) && self.detail_cache.is_some() {
            return;
        }
        self.detail_changed(cx);
        self.sidebar_changed(cx);
        if self.selected != Some(sel) {
            self.reset_analysis();
            self.analysis_content_view.update(cx, |_, cx| cx.notify());
            self.row_filter = None;
            self.expanded.clear();
            self.history_expanded.clear();
            self.file_open = None;
            self.detail_scroll.set_offset(point(px(0.0), px(0.0)));
            self.detail_scroll_offsets = [point(px(0.0), px(0.0)); 4];
            self.overview_list.reset();
            self.detail_list.reset();
            self.file_list.reset();
        }
        self.selected = Some(sel);
        self.request_detail(sel, cx);
    }

    /// 現在の選択を履歴に積んでから選択する。詳細パネルが開いて表示幅が縮むときは、選択が隠れないようにする
    pub(super) fn navigate(&mut self, sel: Selection, window: &Window, cx: &mut Context<Self>) {
        let opening = self.selected.is_none();
        if let Some(cur) = self
            .selected
            .filter(|c| *c != sel || self.detail_tab != DetailTab::Events)
        {
            self.push_back(cur);
            self.forward.clear();
        }
        self.select(sel, cx);
        self.set_detail_tab(DetailTab::Events, cx);
        if opening {
            self.reveal(sel, false, window);
        }
        cx.notify();
    }

    pub(super) fn push_back(&mut self, sel: Selection) {
        self.back.push((sel, self.detail_tab));
        if self.back.len() > 100 {
            self.back.remove(0);
        }
    }

    /// 履歴を遡って直前の選択に戻る（更新で無効になった項目は飛ばす）
    pub(super) fn go_back(&mut self, window: &Window, cx: &mut Context<Self>) {
        while let Some((sel, tab)) = self.back.pop() {
            if self.valid(sel) {
                if let Some(cur) = self.selected {
                    self.forward.push((cur, self.detail_tab));
                }
                self.select(sel, cx);
                self.set_detail_tab(tab, cx);
                self.reveal(sel, false, window);
                break;
            }
        }
        cx.notify();
    }

    pub(super) fn go_forward(&mut self, window: &Window, cx: &mut Context<Self>) {
        while let Some((sel, tab)) = self.forward.pop() {
            if self.valid(sel) {
                if let Some(cur) = self.selected {
                    self.push_back(cur);
                }
                self.select(sel, cx);
                self.set_detail_tab(tab, cx);
                self.reveal(sel, false, window);
                break;
            }
        }
        cx.notify();
    }

    /// 詳細パネルを閉じる。閉じた選択は「戻る」で開き直せる
    pub(super) fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_fullscreen = false;
        self.reset_analysis();
        self.analysis_content_view.update(cx, |_, cx| cx.notify());
        self.cancel_detail_load();
        if let Some(cur) = self.selected.take() {
            self.push_back(cur);
            self.forward.clear();
        }
        self.clear_detail_cache(cx);
        self.sidebar_changed(cx);
        cx.notify();
    }

    /// 選択の時間範囲 (開始, 終了)
    pub(super) fn sel_range(&self, sel: Selection) -> (f64, f64) {
        let evs = &self.store.sessions[sel.session].lanes[sel.lane].events[sel.first..=sel.last];
        let a = evs.first().map_or(0.0, |e| e.start);
        let end = self
            .detail_cache
            .as_ref()
            .filter(|d| d.sel == sel)
            .map_or_else(|| evs.iter().map(|e| e.end).fold(a, f64::max), |d| d.end());
        (a, end)
    }

    /// 選択を表示中のビューに映す。`zoom` なら収まらないときや縮小しすぎのときに拡大率も合わせる。
    /// タイムラインではレーンの行までスクロールし、カレンダーではその日の該当時刻を出す
    pub(super) fn reveal(&mut self, sel: Selection, zoom: bool, window: &Window) {
        if self.statistics_open {
            self.statistics_open = false;
            self.persist();
        }
        let (a, b) = self.sel_range(sel);
        match self.mode {
            Mode::Timeline => {
                let w = self.plot_width(window) as f64;
                let span = self.spp * w;
                let dur = b - a;
                // 1 イベントなら前後が分かる程度（30 分）まで寄せ、範囲なら全体が収まるようにする
                let fit = if sel.first == sel.last {
                    (dur * 3.0).max(30.0 * 60.0)
                } else {
                    (dur * 1.3).max(20.0 * 60.0)
                };
                if zoom && (fit < span * 0.5 || fit > span) {
                    self.spp = (fit / w).clamp(0.005, 86400.0 * 3.0);
                    self.t0 = (a + b) / 2.0 - fit / 2.0;
                } else if b < self.t0 || a > self.t0 + span {
                    // 見えていなければ拡大率はそのままで中央に寄せる（収まらない長さなら先頭を見せる）
                    self.t0 = if dur > span * 0.9 {
                        a - span * 0.05
                    } else {
                        (a + b) / 2.0 - span / 2.0
                    };
                }
                self.clamp_timeline(window);
                self.scroll_to_selection(sel, window);
            }
            Mode::Day | Mode::Week | Mode::Month => {
                let day = local(a).date_naive();
                let (first, days) = match self.mode {
                    Mode::Day => (self.day0, 1),
                    Mode::Week => (self.week_start(), 7),
                    _ => (calendar::month_grid_start(self.day0), 42),
                };
                let other_month = self.mode == Mode::Month && day.month() != self.day0.month();
                if other_month || day < first || day >= first + Duration::days(days) {
                    self.day0 = day;
                }
                let ph = self.plot_height(window);
                let y = ((a - midnight(day)) / 3600.0) as f32 * self.px_per_hour;
                if y < self.cal_scroll || y > self.cal_scroll + ph - 20.0 {
                    self.cal_scroll = calendar::clamp_scroll(y - ph / 3.0, self.px_per_hour, ph);
                }
            }
        }
    }

    /// タイムラインで選択した枠の行へスクロールする。セッション表示では折りたたみも開く。
    pub(super) fn scroll_to_selection(&mut self, sel: Selection, window: &Window) {
        if self.cost_timeline {
            self.scroll_to_cost_selection(sel, window);
            return;
        }
        let v0 = self.t0;
        let v1 = self.t0 + self.spp * self.plot_width(window) as f64;
        let cwd = self.store.sessions[sel.session].cwd.clone();
        if !self.directory_timeline && self.collapsed.remove(&cwd) {
            self.persist();
        }
        let layout = self.rows_in(v0, v1);
        let rows = &layout.rows;
        let i = rows
            .iter()
            .position(|r| {
                if self.directory_timeline {
                    sel.lane == 0
                        && r.packed_blocks().iter().any(|b| {
                            b.session == sel.session
                                && b.block.first <= sel.last
                                && sel.first <= b.block.last
                        })
                } else {
                    matches!(r, Row::Lane(t) if t.session == sel.session && t.lane == sel.lane)
                }
            })
            .or_else(|| {
                if self.directory_timeline {
                    rows.iter()
                        .position(|r| matches!(r, Row::Header { cwd: c, .. } if *c == cwd))
                } else {
                    None
                }
            });
        let Some(i) = i else { return };
        let y = i as f32 * ROW_H;
        let ph = self.plot_height(window);
        if y < self.row_scroll || y + ROW_H > self.row_scroll + ph {
            self.row_scroll = (y - ph / 3.0).max(0.0);
        }
    }

    /// セッションが隠れる絞り込み（読み込み元・プロジェクト）を外す
    pub(super) fn unhide(&mut self, si: usize) {
        let s = &self.store.sessions[si];
        let mut changed = false;
        let hidden = match self.source {
            SourceFilter::All => false,
            SourceFilter::Claude => s.source != Source::Claude,
            SourceFilter::Codex => s.source != Source::Codex,
        };
        if hidden {
            self.source = SourceFilter::All;
            changed = true;
        }
        if self.focus.as_ref().is_some_and(|f| *f != s.cwd) {
            self.focus = None;
            changed = true;
        }
        if changed {
            self.rebuild_order();
            self.persist();
        }
    }

    /// セッション全体（main レーン）を選んで表示する
    pub(super) fn open_session(&mut self, si: usize, window: &Window, cx: &mut Context<Self>) {
        let n = self.store.sessions[si]
            .lanes
            .first()
            .map_or(0, |l| l.events.len());
        if n == 0 {
            return;
        }
        self.unhide(si);
        let sel = Selection {
            session: si,
            lane: 0,
            first: 0,
            last: n - 1,
        };
        self.navigate(sel, window, cx);
        self.set_detail_tab(DetailTab::Session, cx);
        self.reveal(sel, true, window);
        self.sidebar_changed(cx);
    }

    /// 失敗・拒否・中断の最も多いレーン（同数なら起動元に近いレーン）を、失敗だけに絞り込んで開く。
    /// 失敗がなければセッション全体を開く
    pub(super) fn open_failures(&mut self, si: usize, window: &mut Window, cx: &mut Context<Self>) {
        let best = self.store.sessions[si]
            .lanes
            .iter()
            .enumerate()
            .map(|(lane, l)| (l.events.iter().filter(|e| e.status.failed()).count(), lane))
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        match best {
            Some((n, lane)) if n > 0 => {
                self.unhide(si);
                self.open_lane_filtered(si, lane, Some(RowFilter::Failed), window, cx);
            }
            _ => self.open_session(si, window, cx),
        }
    }

    /// 1 つのイベントを選んで表示する
    pub(super) fn open_event(
        &mut self,
        session: usize,
        lane: usize,
        event: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.unhide(session);
        let sel = Selection {
            session,
            lane,
            first: event,
            last: event,
        };
        self.navigate(sel, window, cx);
        self.reveal(sel, true, window);
    }
}
