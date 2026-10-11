use gpui::{
    App, Bounds, Edges, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    Hsla, LayoutId, Length, Pixels, Position, Style, Window, fill, px, relative, size,
};

use crate::theme::c;

/// 時間軸の背景線と行背景。丸めを含む配置は、従来と同じGPUIのレイアウトに任せる。
#[derive(Default)]
pub(super) struct Grid {
    pub(super) lines: Vec<(f32, u32)>,
    pub(super) rows: Vec<RowBackground>,
}

pub(super) struct RowBackground {
    y: f32,
    height: f32,
    background: Hsla,
    border: Option<Hsla>,
}

impl RowBackground {
    pub(super) fn new(y: f32, height: f32, background: Hsla) -> Self {
        Self { y, height, background, border: None }
    }

    pub(super) fn border_top(mut self, color: Hsla) -> Self {
        self.border = Some(color);
        self
    }

    fn style(&self) -> Style {
        Style {
            position: Position::Absolute,
            inset: Edges {
                left: px(0.0).into(),
                right: px(0.0).into(),
                top: px(self.y).into(),
                ..Edges::auto()
            },
            size: size(Length::Auto, px(self.height).into()),
            background: Some(self.background.into()),
            border_color: self.border,
            border_widths: Edges {
                top: px(if self.border.is_some() { 1.0 } else { 0.0 }).into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl IntoElement for Grid {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Grid {
    type RequestLayoutState = Vec<LayoutId>;
    type PrepaintState = Vec<Bounds<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut children = Vec::with_capacity(self.lines.len() + self.rows.len());
        children.extend(self.lines.iter().map(|&(x, _)| {
            window.request_layout(Style {
                position: Position::Absolute,
                inset: Edges {
                    left: px(x).into(),
                    top: px(0.0).into(),
                    bottom: px(0.0).into(),
                    ..Edges::auto()
                },
                size: size(px(1.0).into(), Length::Auto),
                ..Default::default()
            }, [], cx)
        }));
        children.extend(self.rows.iter().map(|row| window.request_layout(row.style(), [], cx)));
        let layout = window.request_layout(Style {
            position: Position::Absolute,
            inset: Edges {
                top: px(0.0).into(),
                left: px(0.0).into(),
                ..Edges::auto()
            },
            size: size(relative(1.0).into(), relative(1.0).into()),
            ..Default::default()
        }, children.iter().copied(), cx);
        (layout, children)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        children: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        children.iter().map(|&id| window.layout_bounds(id)).collect()
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        lines: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.lines.is_empty() && self.rows.is_empty() { return; }
        window.paint_layer(bounds, |window| {
            for (&(_, color), &bounds) in self.lines.iter().zip(lines.iter()) {
                let color = c(color);
                window.paint_quad(fill(bounds, color).border_color(color.opacity(0.0)));
            }
            for (row, &bounds) in self.rows.iter().zip(lines.iter().skip(self.lines.len())) {
                if row.border.is_some() {
                    row.style().paint(bounds, window, cx, |_, _| {});
                } else if !row.background.is_transparent() {
                    let color = row.background;
                    window.paint_quad(fill(bounds, color).border_color(color.opacity(0.0)));
                }
            }
        });
    }
}
