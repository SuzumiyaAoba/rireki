//! 外側のスクロールを保ち、20px の差分行のうち表示付近だけを構築・配置する。
use super::{MONO, Spans, diff_row};
use crate::patch::DiffRow;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Div, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Style, TextStyle, Window, div, point, prelude::*, px, size,
};
use std::rc::{Rc, Weak};

const ROW_HEIGHT: f32 = 20.0;

pub(super) fn rows(rows: &[DiffRow], spans: Rc<Vec<Spans>>, gutter: f32, numbers: bool) -> AnyElement {
    let retained = Rc::strong_count(&spans) > 1;
    if !retained || rows.len() <= 64 || rows.iter().any(|row| row.text.contains('\n')) {
        return column()
            .children(
                rows.iter()
                    .zip(spans.iter())
                    .map(|(row, spans)| diff_row(row, spans, gutter, numbers).0),
            )
            .into_any_element();
    }
    Rows {
        rows: Rc::new(rows.to_vec()),
        spans,
        gutter,
        numbers,
        inner: None,
    }
    .into_any_element()
}

pub(super) struct Rows {
    rows: Rc<Vec<DiffRow>>,
    spans: Rc<Vec<Spans>>,
    gutter: f32,
    numbers: bool,
    inner: Option<Div>,
}

fn column() -> Div {
    div()
        .flex_none()
        .min_w_full()
        .py_0p5()
        .flex()
        .flex_col()
        .text_xs()
        .line_height(px(ROW_HEIGHT))
        .font_family(MONO)
}

impl IntoElement for Rows {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Rows {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = <Div as Element>::PrepaintState;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 各行を独立して配置しても丸めの格子が一致する倍率だけで間引く。
        let virtualize =
            (ROW_HEIGHT * window.scale_factor()).fract() == 0.0 && window.text_style().text_overflow.is_none();
        let mut inner = if virtualize {
            column().child(VisibleRows {
                rows: self.rows.clone(),
                spans: self.spans.clone(),
                gutter: self.gutter,
                numbers: self.numbers,
            })
        } else {
            column().children(
                self.rows
                    .iter()
                    .zip(self.spans.iter())
                    .map(|(row, spans)| diff_row(row, spans, self.gutter, self.numbers).0),
            )
        };
        let result = inner.request_layout(global_id, inspector_id, window, cx);
        self.inner = Some(inner);
        result
    }
    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.inner
            .as_mut()
            .unwrap()
            .prepaint(global_id, inspector_id, bounds, state, window, cx)
    }
    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .as_mut()
            .unwrap()
            .paint(global_id, inspector_id, bounds, state, prepaint, window, cx);
    }
}

struct VisibleRows {
    rows: Rc<Vec<DiffRow>>,
    spans: Rc<Vec<Spans>>,
    gutter: f32,
    numbers: bool,
}
// 強参照で装飾キャッシュの容量制限を延長しない。寸法は同じ装飾・フォント・倍率だけで再利用する。
#[derive(Clone)]
struct Geometry {
    spans: Weak<Vec<Spans>>,
    text_style: TextStyle,
    rem: Pixels,
    scale: f32,
    gutter: f32,
    numbers: bool,
    count: usize,
    natural: gpui::Size<Pixels>,
    bleed: Pixels,
}
impl Geometry {
    fn matches(&self, rows: &VisibleRows, window: &Window) -> bool {
        self.spans.ptr_eq(&Rc::downgrade(&rows.spans))
            && self.spans.strong_count() > 0
            && self.text_style == window.text_style()
            && self.rem == window.rem_size()
            && self.scale == window.scale_factor()
            && self.gutter == rows.gutter
            && self.numbers == rows.numbers
            && self.count == rows.rows.len()
    }
}
impl VisibleRows {
    fn measure_original(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::Size<Pixels>, Vec<std::sync::Arc<gpui::LineLayout>>) {
        let mut layouts = Vec::with_capacity(self.rows.len());
        let mut original = div()
            .flex_none()
            .min_w_full()
            .flex()
            .flex_col()
            .children(self.rows.iter().zip(self.spans.iter()).map(|(row, spans)| {
                let (row, layout) = diff_row(row, spans, self.gutter, self.numbers);
                layouts.push(layout);
                row
            }))
            .into_any_element();
        let natural = original.layout_as_root(size(AvailableSpace::MaxContent, AvailableSpace::MaxContent), window, cx);
        original.prepaint_at(point(px(0.0), px(0.0)), window, cx);
        let layouts = layouts
            .into_iter()
            .filter_map(|layout| {
                layout
                    .line_layout_for_index(0)
                    .map(|line| line.unwrapped_layout.clone())
            })
            .collect();
        (natural, layouts)
    }

    // 本文は GPUI の公開文字計測 API で測り、横幅・丸めは元の行へ委ねる。
    fn measure_text(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(gpui::Size<Pixels>, Vec<std::sync::Arc<gpui::LineLayout>>)> {
        let mut style = window.text_style();
        let height = px(self.rows.len() as f32 * ROW_HEIGHT);
        if style.line_clamp.is_some()
            || window.line_height() != px(ROW_HEIGHT)
            || f32::from(height) * window.scale_factor() > 16_777_216.0
        {
            return None;
        }
        style.color = crate::theme::c(super::CODE_TEXT);
        let mut hunk_style = style.clone();
        hunk_style.color = crate::theme::c(crate::theme::SUBTLE);
        let font_size = style.font_size.to_pixels(window.rem_size());
        let mut layouts = Vec::with_capacity(self.rows.len());
        for (row, spans) in self.rows.iter().zip(self.spans.iter()) {
            let text: gpui::SharedString = if row.text.is_empty() {
                " ".to_string()
            } else {
                row.text.clone()
            }
            .into();
            let default_style = if row.kind == '@' { &hunk_style } else { &style };
            // GPUI 0.2.2 StyledText::compute_runs と同じ順で、未装飾部分も含める。
            let mut runs = Vec::new();
            let mut ix = 0;
            for (range, highlight) in spans {
                if ix < range.start {
                    debug_assert!(text.is_char_boundary(range.start));
                    runs.push(default_style.clone().to_run(range.start - ix));
                }
                debug_assert!(text.is_char_boundary(range.end));
                runs.push(default_style.clone().highlight(*highlight).to_run(range.len()));
                ix = range.end;
            }
            if ix < text.len() {
                runs.push(default_style.to_run(text.len() - ix));
            }
            let lines = window
                .text_system()
                .shape_text(text, font_size, &runs, None, None)
                .ok()?;
            if lines.len() != 1 || lines[0].size(window.line_height()).height != px(ROW_HEIGHT) {
                return None;
            }
            layouts.push(lines[0].unwrapped_layout.clone());
        }
        let mut height_box = div().h(height).into_any_element();
        let mut natural =
            height_box.layout_as_root(size(AvailableSpace::MaxContent, AvailableSpace::MaxContent), window, cx);
        if natural.height != height {
            return None;
        }
        let widest = layouts
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                a.width
                    .ceil()
                    .partial_cmp(&b.width.ceil())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?
            .0;
        let mut row = diff_row(&self.rows[widest], &self.spans[widest], self.gutter, self.numbers)
            .0
            .into_any_element();
        natural.width = row
            .layout_as_root(size(AvailableSpace::MaxContent, AvailableSpace::MaxContent), window, cx)
            .width;
        Some((natural, layouts))
    }

    fn measure(&self, window: &mut Window, cx: &mut App) -> Geometry {
        let (natural, layouts) = self
            .measure_text(window, cx)
            .unwrap_or_else(|| self.measure_original(window, cx));
        let mut bleed = px(ROW_HEIGHT);
        for line in &layouts {
            for run in &line.runs {
                let b = cx.text_system().bounding_box(run.font_id, line.font_size);
                bleed = bleed.max(b.size.height + line.ascent + line.descent + px(ROW_HEIGHT));
            }
        }
        // 行番号とマーカーで使うフォールバックフォントも含める。
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let symbols: gpui::SharedString = "0123456789+−".into();
        if let Ok(lines) = window.text_system().shape_text(
            symbols.clone(),
            font_size,
            &[text_style.to_run(symbols.len())],
            None,
            None,
        ) {
            for line in lines {
                for run in &line.unwrapped_layout.runs {
                    let b = cx.text_system().bounding_box(run.font_id, font_size);
                    bleed = bleed.max(
                        b.size.height + line.unwrapped_layout.ascent + line.unwrapped_layout.descent + px(ROW_HEIGHT),
                    );
                }
            }
        }
        let mut thickness = text_style
            .underline
            .map_or(px(0.0), |style| style.thickness)
            .max(text_style.strikethrough.map_or(px(0.0), |style| style.thickness));
        for spans in self.spans.iter() {
            for (_, style) in spans {
                thickness = thickness
                    .max(style.underline.map_or(px(0.0), |style| style.thickness))
                    .max(style.strikethrough.map_or(px(0.0), |style| style.thickness));
            }
        }
        bleed += thickness * 2.0;
        Geometry {
            spans: Rc::downgrade(&self.spans),
            text_style,
            rem: window.rem_size(),
            scale: window.scale_factor(),
            gutter: self.gutter,
            numbers: self.numbers,
            count: self.rows.len(),
            natural,
            bleed,
        }
    }
}
impl IntoElement for VisibleRows {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for VisibleRows {
    type RequestLayoutState = Geometry;
    type PrepaintState = Vec<AnyElement>;
    fn id(&self) -> Option<ElementId> {
        Some("visible-diff-rows".into())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Geometry) {
        let geometry = window.with_element_state(global_id.unwrap(), |state: Option<Geometry>, window| {
            let geometry = state
                .filter(|state| state.matches(self, window))
                .unwrap_or_else(|| self.measure(window, cx));
            (geometry.clone(), geometry)
        });
        let natural = geometry.natural;
        let style = Style {
            flex_shrink: 0.0,
            ..Default::default()
        };
        let id = window.request_measured_layout(style, move |known, _, _, _| {
            size(
                known.width.unwrap_or(natural.width),
                known.height.unwrap_or(natural.height),
            )
        });
        (id, geometry)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Geometry,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let mask = window.content_mask().bounds;
        if mask.is_empty() {
            return Vec::new();
        }
        let start = (((mask.top() - state.bleed - bounds.top()) / px(ROW_HEIGHT))
            .floor()
            .max(0.0) as usize)
            .min(self.rows.len());
        let end = (((mask.bottom() + state.bleed - bounds.top()) / px(ROW_HEIGHT))
            .ceil()
            .max(0.0) as usize)
            .min(self.rows.len());
        let mut visible = Vec::with_capacity(end.saturating_sub(start));
        for index in start..end {
            let mut row = diff_row(&self.rows[index], &self.spans[index], self.gutter, self.numbers)
                .0
                .w(bounds.size.width)
                .h(px(ROW_HEIGHT))
                .into_any_element();
            row.layout_as_root(
                size(
                    AvailableSpace::Definite(bounds.size.width),
                    AvailableSpace::Definite(px(ROW_HEIGHT)),
                ),
                window,
                cx,
            );
            row.prepaint_at(
                bounds.origin + point(px(0.0), px(index as f32 * ROW_HEIGHT)),
                window,
                cx,
            );
            visible.push(row);
        }
        visible
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Geometry,
        rows: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for row in rows {
            row.paint(window, cx);
        }
    }
}
