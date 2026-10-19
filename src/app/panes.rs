//! ペイン寸法、開閉アニメーション、境界のリサイズ。

use super::{
    AXIS_H, Context, FOOTER_H, GAP, HANDLE_W, HEADER_H, Instant, MAIN_MIN_W, MAX_SIDEBAR_W,
    MIN_DETAIL_W, MIN_PANE_W, MIN_SIDEBAR_W, MINIMAP_H, Mode, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PANE_ANIM_SECS, RAIL_W, Resize, Rireki, STRIP_H, Window, c,
    calendar, div, ease, now, px, theme,
};
use gpui::prelude::*;

impl Rireki {
    // ------------------------------------------------------------ 寸法

    /// サイドバーと詳細パネルの実際の幅。ウィンドウが狭いときはメイン列が残るよう、詳細パネル → サイドバーの順に縮める
    pub(super) fn pane_widths(&self, window: &Window) -> (f32, f32) {
        if self.statistics_open {
            return (0.0, 0.0);
        }
        let (side, detail) = self.pane_full_widths(window);
        (side * ease(self.side_t), detail * ease(self.detail_t))
    }

    /// 開ききったときのサイドバーと詳細パネルの幅（アニメーション中の中身はこの幅で描き、外側で切り取る）
    pub(super) fn pane_full_widths(&self, window: &Window) -> (f32, f32) {
        let w: f32 = window.viewport_size().width.into();
        let room = w - RAIL_W - GAP - 2.0 - MAIN_MIN_W;
        let mut side = if self.side_t > 0.0 {
            self.sidebar_w
        } else {
            0.0
        };
        let mut detail = if self.detail_t > 0.0 {
            self.detail_w
        } else {
            0.0
        };
        if side + detail > room && detail > 0.0 {
            detail = (room - side).clamp(MIN_DETAIL_W, detail);
        }
        if side + detail > room && side > 0.0 {
            side = (room - detail).clamp(MIN_SIDEBAR_W, side);
        }
        (side, detail)
    }

    /// 開閉のアニメーションを 1 フレーム進める。途中なら次のフレームを要求する
    pub(super) fn step_panes(&mut self, window: &mut Window) {
        let side_goal = if self.sidebar_open { 1.0 } else { 0.0 };
        let detail_goal = if self.selected.is_some() { 1.0 } else { 0.0 };
        if self.side_t == side_goal && self.detail_t == detail_goal {
            self.anim_at = None;
            return;
        }
        let now = Instant::now();
        // 最初のフレームは 1 フレーム分だけ進める
        let dt = self.anim_at.map_or(1.0 / 60.0, |t| {
            now.duration_since(t).as_secs_f32().min(0.05)
        });
        self.anim_at = Some(now);
        let step = dt / PANE_ANIM_SECS;
        let approach = |t: f32, goal: f32| {
            if t < goal {
                (t + step).min(goal)
            } else {
                (t - step).max(goal)
            }
        };
        self.side_t = approach(self.side_t, side_goal);
        self.detail_t = approach(self.detail_t, detail_goal);
        window.request_animation_frame();
    }

    /// メイン列（見出し・本体・凡例）の幅
    pub(super) fn main_width(&self, window: &Window) -> f32 {
        let w: f32 = window.viewport_size().width.into();
        let (side, detail) = self.pane_widths(window);
        (w - RAIL_W - GAP - 2.0 - side - detail).max(200.0)
    }

    /// メイン列の本体（見出しと凡例を除く）の高さ
    pub(super) fn body_height(&self, window: &Window) -> f32 {
        let h: f32 = window.viewport_size().height.into();
        let minimap = if self.mode == Mode::Timeline {
            MINIMAP_H
        } else {
            0.0
        };
        (h - STRIP_H - GAP - 2.0 - HEADER_H - minimap - FOOTER_H).max(100.0)
    }

    /// タイムラインのブロックを描く領域の幅
    pub(super) fn plot_width(&self, window: &Window) -> f32 {
        (self.main_width(window) - self.label_w).max(100.0)
    }

    pub(super) fn plot_height(&self, window: &Window) -> f32 {
        let head = if self.mode == Mode::Timeline {
            AXIS_H
        } else {
            calendar::DAY_HEAD_H
        };
        (self.body_height(window) - head).max(100.0)
    }

    /// タイムラインの右端を現在時刻から 1 時間先までに制限する
    pub(super) fn clamp_timeline(&mut self, window: &Window) {
        if self.mode == Mode::Timeline && !self.statistics_open {
            let span = self.spp * self.plot_width(window) as f64;
            self.t0 = self.t0.min(now() + 3600.0 - span);
        }
    }

    pub(super) fn set_range(&mut self, start: f64, end: f64, window: &Window) {
        let w = self.plot_width(window) as f64;
        self.spp = ((end - start) / w).max(0.01);
        self.t0 = start;
        self.clamp_timeline(window);
        self.row_scroll = 0.0;
    }

    pub(super) fn start_resize(&mut self, r: Resize, cx: &mut Context<Self>) {
        self.resize = Some(r);
        self.drag = None;
        cx.stop_propagation();
    }

    pub(super) fn on_resize_move(
        &mut self,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.resize else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.resize = None;
            self.persist();
            return;
        }
        let x: f32 = e.position.x.into();
        let win: f32 = window.viewport_size().width.into();
        // メイン列が残るようにする
        let room = |others: f32| win - RAIL_W - GAP - 2.0 - others - MAIN_MIN_W;
        let (side, detail) = self.pane_widths(window);
        let widths = (self.sidebar_w, self.label_w, self.detail_w);
        match r {
            Resize::Sidebar { x: x0, w } => {
                let max = room(detail).clamp(MIN_SIDEBAR_W, MAX_SIDEBAR_W);
                self.sidebar_w = (w + x - x0).clamp(MIN_SIDEBAR_W, max);
            }
            Resize::Label { x: x0, w } => {
                let max = (self.main_width(window) - 100.0).max(MIN_PANE_W);
                self.label_w = (w + x - x0).clamp(MIN_PANE_W, max);
            }
            Resize::Detail { x: x0, w } => {
                self.detail_w = (w - (x - x0)).clamp(MIN_DETAIL_W, room(side).max(MIN_DETAIL_W));
            }
        }
        // 上下だけの移動や、幅の上限・下限を越えた移動では画面を作り直さない。
        if widths == (self.sidebar_w, self.label_w, self.detail_w) {
            cx.stop_propagation();
            return;
        }
        self.detail_changed(cx);
        self.sidebar_changed(cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn end_resize(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if self.resize.take().is_some() {
            self.persist();
        }
    }

    /// 境界のつまみ。ドラッグで幅を変える
    pub(super) fn resize_handle(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
        f: impl Fn(&Self, f32) -> Resize + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(HANDLE_W))
            .cursor_col_resize()
            .hover(|s| s.bg(c(theme::ACCENT).opacity(0.5)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    let r = f(t, e.position.x.into());
                    t.start_resize(r, cx);
                }),
            )
    }
}
