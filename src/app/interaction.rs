//! ホイール、ピンチ、ドラッグ、ポップオーバーの入力処理。

use super::{
    Context, Drag, Mode, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Rireki, ScrollWheelEvent, Selection, Window, calendar, px,
};

impl Rireki {
    // ------------------------------------------------------------ 入力

    pub(super) fn on_wheel(
        &mut self,
        e: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = (
            self.t0,
            self.spp,
            self.row_scroll,
            self.cal_scroll,
            self.px_per_hour,
            self.hovered,
        );
        let d = e.delta.pixel_delta(px(16.0));
        let (dx, dy): (f32, f32) = (d.x.into(), d.y.into());
        let zoom = e.modifiers.platform || e.modifiers.control || e.modifiers.alt;
        match self.mode {
            Mode::Timeline => {
                if zoom {
                    self.zoom_at(e.position, (-(dy as f64) * 0.01).exp(), window);
                } else if e.modifiers.shift {
                    self.t0 -= (dy + dx) as f64 * self.spp;
                } else {
                    self.t0 -= dx as f64 * self.spp;
                    self.row_scroll = (self.row_scroll - dy).max(0.0);
                }
            }
            Mode::Month => {}
            Mode::Day | Mode::Week => {
                if zoom {
                    self.zoom_at(e.position, (-(dy as f64) * 0.01).exp(), window);
                } else {
                    self.cal_scroll = calendar::clamp_scroll(
                        self.cal_scroll - dy,
                        self.px_per_hour,
                        self.plot_height(window),
                    );
                }
            }
        }
        self.clamp_timeline(window);
        self.hovered = None;
        if (
            self.t0,
            self.spp,
            self.row_scroll,
            self.cal_scroll,
            self.px_per_hour,
            self.hovered,
        ) != before
        {
            cx.notify();
        }
    }

    /// トラックパッドのピンチ。`m` は拡大率の差分（正で拡大）
    pub(super) fn on_pinch(&mut self, m: f64, window: &mut Window, cx: &mut Context<Self>) {
        if self.statistics_open || self.mode == Mode::Month {
            return;
        }
        let before = (
            self.t0,
            self.spp,
            self.row_scroll,
            self.cal_scroll,
            self.px_per_hour,
            self.hovered,
        );
        let pos = window.mouse_position();
        match self.mode {
            Mode::Timeline => self.zoom_at(pos, 1.0 / (1.0 + m).max(0.1), window),
            _ => self.zoom_at(pos, (1.0 + m).max(0.1), window),
        }
        if (
            self.t0,
            self.spp,
            self.row_scroll,
            self.cal_scroll,
            self.px_per_hour,
            self.hovered,
        ) != before
        {
            cx.notify();
        }
    }

    /// `pos` を基準に拡大・縮小する。`f` はタイムラインでは spp、カレンダーでは px_per_hour に掛ける倍率
    pub(super) fn zoom_at(&mut self, pos: Point<Pixels>, f: f64, window: &Window) {
        let o = self.hit_origin.get();
        let mx: f32 = (pos.x - o.x).into();
        let my: f32 = (pos.y - o.y).into();
        match self.mode {
            Mode::Timeline => {
                let x = mx.clamp(0.0, self.plot_width(window)) as f64;
                let anchor = self.t0 + x * self.spp;
                self.spp = (self.spp * f).clamp(0.005, 86400.0 * 3.0);
                self.t0 = anchor - x * self.spp;
            }
            _ => {
                let anchor_h = (my + self.cal_scroll) / self.px_per_hour;
                self.px_per_hour = (self.px_per_hour * f as f32).clamp(20.0, 20000.0);
                self.cal_scroll = calendar::clamp_scroll(
                    anchor_h * self.px_per_hour - my,
                    self.px_per_hour,
                    self.plot_height(window),
                );
            }
        }
        self.clamp_timeline(window);
    }

    pub(super) fn on_down(&mut self, e: &MouseDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let scroll = if self.mode == Mode::Timeline {
            self.row_scroll
        } else {
            self.cal_scroll
        };
        self.drag = Some(Drag {
            origin: e.position,
            t0: self.t0,
            scroll,
            moved: false,
        });
    }

    /// 位置にあるブロック（上に描いたものを優先）
    pub(super) fn hit(&self, pos: Point<Pixels>) -> Option<Selection> {
        let o = self.hit_origin.get();
        let (x, y): (f32, f32) = ((pos.x - o.x).into(), (pos.y - o.y).into());
        self.hits
            .iter()
            .rev()
            .find(|((bx, by, bw, bh), _)| x >= *bx && x < bx + bw && y >= *by && y < by + bh)
            .map(|(_, s)| *s)
    }

    pub(super) fn on_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            let h = self.hit(e.position);
            if h != self.hovered {
                self.hovered = h;
                self.hover_at = e.position;
                cx.notify();
            }
            return;
        }
        let before = (
            self.t0,
            self.spp,
            self.row_scroll,
            self.cal_scroll,
            self.px_per_hour,
            self.hovered,
        );
        let ph = self.plot_height(window);
        let Some(d) = self.drag.as_mut() else { return };
        let dx: f32 = (e.position.x - d.origin.x).into();
        let dy: f32 = (e.position.y - d.origin.y).into();
        if dx.abs() + dy.abs() > 3.0 {
            d.moved = true;
        }
        if !d.moved {
            return;
        }
        self.hovered = None;
        match self.mode {
            Mode::Timeline => {
                self.t0 = d.t0 - dx as f64 * self.spp;
                self.row_scroll = (d.scroll - dy).max(0.0);
            }
            _ => {
                self.cal_scroll = calendar::clamp_scroll(d.scroll - dy, self.px_per_hour, ph);
            }
        }
        let t0 = self.t0;
        self.clamp_timeline(window);
        if self.t0 < t0 {
            // 上限を越えたドラッグ量を残さず、逆方向に動かすとすぐ過去へ戻れるようにする
            if let Some(d) = self.drag.as_mut() {
                d.t0 = self.t0;
                d.origin.x = e.position.x;
            }
        }
        if (
            self.t0,
            self.spp,
            self.row_scroll,
            self.cal_scroll,
            self.px_per_hour,
            self.hovered,
        ) != before
        {
            cx.notify();
        }
    }

    pub(super) fn on_up(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        if d.moved {
            return;
        }
        let Some(sel) = self.hit(e.position) else {
            return;
        };
        if e.click_count >= 2 && self.mode == Mode::Timeline {
            // ダブルクリックでブロックの範囲まで拡大する
            let (a, b) = self.sel_range(sel);
            let pad = ((b - a) * 0.1).max(5.0);
            let w = self.plot_width(window) as f64;
            self.spp = ((b - a + 2.0 * pad) / w).clamp(0.005, 86400.0 * 3.0);
            self.t0 = a - pad;
        } else if e.click_count >= 2 {
            // カレンダーではダブルクリックでその範囲をタイムラインで開く
            let (a, b) = self.sel_range(sel);
            let pad = ((b - a) * 0.1).max(60.0);
            self.set_mode(Mode::Timeline, window);
            self.set_range(a - pad, b + pad, window);
        }
        self.navigate(sel, window, cx);
    }

    // ------------------------------------------------------------ キー操作

    pub(super) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popover.take().is_some() {
            cx.notify();
        } else if self.detail_fullscreen {
            self.toggle_detail_fullscreen(window, cx);
        } else if self.statistics_open {
            self.set_mode(self.mode, window);
            cx.notify();
        } else if self.selected.is_some() {
            self.close_detail(cx);
        } else if !self.query.is_empty() {
            self.clear_search(cx);
        } else {
            window.focus(&self.focus_handle);
        }
    }

    pub(super) fn cancel_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popover.take().is_some() {
            cx.notify();
        } else if self.search_input.read(cx).text().is_empty() {
            window.focus(&self.focus_handle);
            self.sidebar_changed(cx);
        } else {
            self.clear_search(cx);
        }
    }
}
