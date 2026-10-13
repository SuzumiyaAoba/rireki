//! Wayland でサーバー側の枠がない場合にウィンドウの端をドラッグしてリサイズする。

use gpui::{CursorStyle, Decorations, Div, MouseButton, ResizeEdge, Window, div, prelude::*, px};

pub(super) fn resize_border(window: &Window) -> Div {
    let Decorations::Client { tiling } = window.window_decorations() else { return div(); };
    let handle = |edge| {
        let cursor = match edge {
            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
            ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        };
        div().absolute().cursor(cursor).on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.start_window_resize(edge);
            cx.stop_propagation();
        })
    };
    div().absolute().top_0().left_0().size_full()
        .when(!tiling.top, |d| d.child(handle(ResizeEdge::Top).top_0().left(px(6.0)).right(px(6.0)).h(px(4.0))))
        .when(!tiling.bottom, |d| d.child(handle(ResizeEdge::Bottom).bottom_0().left(px(6.0)).right(px(6.0)).h(px(4.0))))
        .when(!tiling.left, |d| d.child(handle(ResizeEdge::Left).left_0().top(px(6.0)).bottom(px(6.0)).w(px(4.0))))
        .when(!tiling.right, |d| d.child(handle(ResizeEdge::Right).right_0().top(px(6.0)).bottom(px(6.0)).w(px(4.0))))
        .when(!tiling.top && !tiling.left, |d| d.child(handle(ResizeEdge::TopLeft).top_0().left_0().size(px(6.0))))
        .when(!tiling.top && !tiling.right, |d| d.child(handle(ResizeEdge::TopRight).top_0().right_0().size(px(6.0))))
        .when(!tiling.bottom && !tiling.left, |d| d.child(handle(ResizeEdge::BottomLeft).bottom_0().left_0().size(px(6.0))))
        .when(!tiling.bottom && !tiling.right, |d| d.child(handle(ResizeEdge::BottomRight).bottom_0().right_0().size(px(6.0))))
}
