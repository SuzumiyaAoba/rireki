//! 全文を保持し、ホバーから 1 秒後に左へ流す 1 行のテキスト。

use std::{
    cell::RefCell,
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, ContentMask, DispatchPhase, Element, ElementId, GlobalElementId, HighlightStyle,
    Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseMoveEvent, Pixels,
    ShapedLine, SharedString, Style, TextRun, Window, point, px, relative,
};

const DELAY: Duration = Duration::from_secs(1);
const SPEED: f32 = 36.0;
const GAP: f32 = 24.0;

pub(in crate::app) struct Marquee {
    id: ElementId,
    text: SharedString,
    on_show: Option<Instant>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
}

impl Marquee {
    pub(in crate::app) fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            on_show: None,
            highlights: Vec::new(),
        }
    }

    /// ホバーカードは、表示された時点をホバーの開始とする。
    pub(in crate::app) fn on_show(mut self) -> Self {
        self.on_show = Some(Instant::now());
        self
    }

    pub(in crate::app) fn with_highlights(
        mut self,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        self.highlights = highlights;
        self
    }
}

struct State {
    text: SharedString,
    since: Option<Instant>,
}

impl State {
    fn hover(&mut self, hovered: bool, now: Instant) -> bool {
        match (hovered, self.since.is_some()) {
            (true, false) => self.since = Some(now),
            (false, true) => self.since = None,
            _ => return false,
        }
        true
    }

    fn offset(
        &self,
        now: Instant,
        width: Pixels,
        viewport: Pixels,
        cycle: Pixels,
    ) -> Option<Pixels> {
        if width <= viewport || viewport <= px(0.0) {
            return None;
        }
        let elapsed = now
            .saturating_duration_since(self.since?)
            .checked_sub(DELAY)?;
        Some(px(
            (elapsed.as_secs_f64() * SPEED as f64 % f64::from(f32::from(cycle))) as f32,
        ))
    }
}

pub(in crate::app) struct LayoutState {
    state: Rc<RefCell<State>>,
    line: ShapedLine,
    separator: ShapedLine,
    ellipsis: ShapedLine,
}

pub(in crate::app) struct PrepaintState {
    hitbox: Option<Hitbox>,
}

impl IntoElement for Marquee {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Marquee {
    type RequestLayoutState = LayoutState;
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutState) {
        let state =
            window.with_element_state(id.unwrap(), |state: Option<Rc<RefCell<State>>>, _| {
                let state = state
                    .filter(|s| s.borrow().text == self.text)
                    .unwrap_or_else(|| {
                        Rc::new(RefCell::new(State {
                            text: self.text.clone(),
                            since: self.on_show,
                        }))
                    });
                (state.clone(), state)
            });
        let text_style = window.text_style();
        let run = |len, style: &gpui::TextStyle| TextRun {
            len,
            font: style.font(),
            color: style.color,
            background_color: style.background_color,
            underline: style.underline,
            strikethrough: style.strikethrough,
        };
        let mut runs = Vec::new();
        let mut end = 0;
        for (range, highlight) in &self.highlights {
            if range.start > end {
                runs.push(run(range.start - end, &text_style));
            }
            runs.push(run(range.len(), &text_style.clone().highlight(*highlight)));
            end = range.end;
        }
        if end < self.text.len() {
            runs.push(run(self.text.len() - end, &text_style));
        }
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(self.text.clone(), font_size, &runs, None);
        let separator = window.text_system().shape_line(
            "↻".into(),
            font_size,
            &[TextRun {
                color: text_style.color.opacity(0.55),
                ..run("↻".len(), &text_style)
            }],
            None,
        );
        let ellipsis = window.text_system().shape_line(
            "…".into(),
            font_size,
            &[run("…".len(), &text_style)],
            None,
        );
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (
            window.request_layout(style, [], cx),
            LayoutState {
                state,
                line,
                separator,
                ellipsis,
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut LayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> PrepaintState {
        PrepaintState {
            hitbox: self
                .on_show
                .is_none()
                .then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal)),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut LayoutState,
        prepaint: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(hitbox) = &prepaint.hitbox {
            layout.state.borrow_mut().hover(
                hitbox.is_hovered(window) && !cx.has_active_drag(),
                Instant::now(),
            );
            let state = layout.state.clone();
            let hitbox = hitbox.clone();
            let view = window.current_view();
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture
                    && state.borrow_mut().hover(
                        hitbox.is_hovered(window) && !cx.has_active_drag(),
                        Instant::now(),
                    )
                {
                    cx.notify(view);
                }
            });
        }
        let width = bounds.size.width;
        let cycle = layout.line.width + layout.separator.width + px(2.0 * GAP);
        let offset = layout
            .state
            .borrow()
            .offset(Instant::now(), layout.line.width, width, cycle);
        let line_height = window.line_height();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(offset) = offset {
                let origin = point(bounds.left() - offset, bounds.top());
                let _ = layout.line.paint(origin, line_height, window, cx);
                let _ = layout.separator.paint(
                    origin + point(layout.line.width + px(GAP), px(0.0)),
                    line_height,
                    window,
                    cx,
                );
                let _ = layout
                    .line
                    .paint(origin + point(cycle, px(0.0)), line_height, window, cx);
            } else if layout.line.width > width {
                let mut text_bounds = bounds;
                text_bounds.size.width = (width - layout.ellipsis.width).max(px(0.0));
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: text_bounds,
                    }),
                    |window| {
                        let _ = layout.line.paint(bounds.origin, line_height, window, cx);
                    },
                );
                let _ = layout.ellipsis.paint(
                    point(bounds.right() - layout.ellipsis.width, bounds.top()),
                    line_height,
                    window,
                    cx,
                );
            } else {
                let _ = layout.line.paint(bounds.origin, line_height, window, cx);
            }
        });
        // 画面内でホバー中の長いテキストだけが次のフレームを要求する。
        if layout.state.borrow().since.is_some() && layout.line.width > width && width > px(0.0) {
            window.request_animation_frame();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_waits_then_scrolls_left_and_loops() {
        let now = Instant::now();
        let mut state = State {
            text: "長いプロンプト".into(),
            since: None,
        };
        assert!(state.hover(true, now));
        let offset = |s: &State, millis| {
            s.offset(
                now + Duration::from_millis(millis),
                px(500.0),
                px(100.0),
                px(600.0),
            )
        };
        assert_eq!(offset(&state, 999), None);
        assert_eq!(offset(&state, 1000), Some(px(0.0)));
        assert_eq!(offset(&state, 2000), Some(px(36.0)));
        assert_eq!(offset(&state, 18000), Some(px(12.0)));
        assert!(!state.hover(true, now + Duration::from_secs(2)));
        assert!(state.hover(false, now + Duration::from_secs(3)));
        assert_eq!(offset(&state, 4000), None);
        state.hover(true, now + Duration::from_secs(5));
        assert_eq!(offset(&state, 5999), None);
        assert_eq!(offset(&state, 6000), Some(px(0.0)));
    }

    #[test]
    fn fitting_text_does_not_scroll() {
        let now = Instant::now();
        let state = State {
            text: "短い指示".into(),
            since: Some(now),
        };
        for (text, viewport) in [(99.0, 100.0), (100.0, 100.0), (100.0, 0.0)] {
            assert_eq!(
                state.offset(
                    now + Duration::from_secs(3),
                    px(text),
                    px(viewport),
                    px(600.0)
                ),
                None
            );
        }
    }
}
