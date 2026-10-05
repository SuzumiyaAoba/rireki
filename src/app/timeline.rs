//! タイムライン（横軸が時刻、縦にセッションのレーンを並べる）と、ブロックの描画

use chrono::Timelike;
use gpui::{
    AnyElement, App, Bounds, ClickEvent, ContentMask, Context, ElementId, FontWeight, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, SharedString, TextRun, Window, canvas, div, fill, point, prelude::*, px, size,
};

use super::widgets::{project_tile, tip};
use super::{AXIS_H, BlockPaint, MIN_BLOCK, ROW_H, Rireki, Row, Selection, fmt_dur, local, now};
use crate::icons::{self, Icon};
use crate::layout::Block;
use crate::model::one_line;
use crate::theme::{self, c};

/// ブロックに描くアイコンの大きさと、アイコンを出すブロックの最小幅
const ICON: f32 = 11.0;
const ICON_MIN_W: f32 = 16.0;

/// 右端の縦のミニマップの幅
const LANE_MAP_W: f32 = 12.0;

impl Rireki {
    pub(super) fn block_paint(
        &self,
        sel: Selection,
        rect: (f32, f32, f32, f32),
        vertical: bool,
    ) -> BlockPaint {
        let (_, _, w, h) = rect;
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let b = Block {
            start: 0.0,
            end: 0.0,
            first: sel.first,
            last: sel.last,
        };
        let show_text = if vertical {
            h >= 14.0 && w >= 24.0
        } else {
            w >= 28.0
        };
        let (dominant, text) =
            self.layout_cache
                .borrow_mut()
                .describe((sel.session, sel.lane), lane, &b, show_text);
        let dom = &lane.events[dominant];
        let (color, icon) = (theme::event_color(dom), theme::event_icon(dom));
        let selected = self.selected.is_some_and(|s| {
            s.session == sel.session
                && s.lane == sel.lane
                && s.first <= sel.last
                && sel.first <= s.last
        });
        // 幅に収まる程度の文字数だけ整形する
        let label = text.map(|s| one_line(&s, (w / 5.0) as usize + 1).into());
        BlockPaint {
            rect,
            sel,
            color,
            icon,
            multi: sel.last > sel.first,
            selected,
            hovered: self.hovered == Some(sel),
            label,
        }
    }

    /// ブロックを 1 つの canvas にまとめて描き、当たり判定用に位置を覚えておく
    pub(super) fn blocks_canvas(&mut self, blocks: Vec<BlockPaint>) -> impl IntoElement {
        self.hits = blocks.iter().map(|b| (b.rect, b.sel)).collect();
        let origin = self.hit_origin.clone();
        canvas(
            move |bounds, _, _| origin.set(bounds.origin),
            move |bounds, _, window: &mut Window, cx: &mut App| {
                let font = window.text_style().font();
                let font_size = window.rem_size() * 0.75;
                let line_h = px(16.0);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let rect = |b: &BlockPaint| {
                        let (x, y, w, h) = b.rect;
                        Bounds::new(bounds.origin + point(px(x), px(y)), size(px(w), px(h)))
                    };
                    let plain = |b: &BlockPaint| {
                        b.label.is_none() && (b.rect.2 < ICON_MIN_W || b.rect.3 < ICON + 2.0)
                    };
                    let mut first = 0;
                    while first < blocks.len() {
                        let mut end = first + 1;
                        let mut layer = rect(&blocks[first]);
                        if plain(&blocks[first]) {
                            // 装飾のない連続した矩形は同じ種類の描画命令だけになる。
                            // 重なりと挿入順を保ち、GPUI の境界木への登録をまとめる。
                            while end < blocks.len() && plain(&blocks[end]) {
                                layer = layer.union(&rect(&blocks[end]));
                                end += 1;
                            }
                        }
                        let mut paint = |window: &mut Window| {
                            for b in &blocks[first..end] {
                                let (x, y, w, h) = b.rect;
                                let r = Bounds::new(
                                    bounds.origin + point(px(x), px(y)),
                                    size(px(w), px(h)),
                                );
                                let col = c(b.color);
                                let alpha = if b.hovered {
                                    1.0
                                } else if b.multi {
                                    0.75
                                } else {
                                    0.9
                                };
                                let mut q = fill(r, col.opacity(alpha)).corner_radii(px(2.0));
                                if b.selected {
                                    q = q.border_widths(px(1.5)).border_color(gpui::white());
                                } else if b.hovered {
                                    q = q
                                        .border_widths(px(1.0))
                                        .border_color(gpui::white().opacity(0.6));
                                } else if b.multi {
                                    q = q
                                        .border_widths(gpui::Edges {
                                            left: px(2.0),
                                            ..Default::default()
                                        })
                                        .border_color(col);
                                }
                                window.paint_quad(q);
                                // 収まるならアイコンを左端に出し、ラベルはその右に続ける
                                let mut text_x = 4.0;
                                if w >= ICON_MIN_W && h >= ICON + 2.0 {
                                    let top = (h - ICON) / 2.0;
                                    let ib = Bounds::new(
                                        r.origin + point(px(3.0), px(top)),
                                        size(px(ICON), px(ICON)),
                                    );
                                    let _ = window.paint_svg(
                                        ib,
                                        b.icon.path().into(),
                                        gpui::TransformationMatrix::unit(),
                                        gpui::black().opacity(0.8),
                                        cx,
                                    );
                                    text_x = 3.0 + ICON + 3.0;
                                }
                                let Some(label) = &b.label else { continue };
                                let run = TextRun {
                                    len: label.len(),
                                    font: font.clone(),
                                    color: gpui::black(),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                let line = window.text_system().shape_line(
                                    label.clone(),
                                    font_size,
                                    &[run],
                                    None,
                                );
                                let top = if h > line_h.into() {
                                    (h - f32::from(line_h)) / 2.0
                                } else {
                                    0.0
                                };
                                window.with_content_mask(
                                    Some(ContentMask { bounds: r }),
                                    |window| {
                                        let _ = line.paint(
                                            r.origin + point(px(text_x), px(top)),
                                            line_h,
                                            window,
                                            cx,
                                        );
                                    },
                                );
                            }
                        };
                        if plain(&blocks[first]) {
                            window.paint_layer(layer, paint);
                        } else {
                            paint(window);
                        }
                        first = end;
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    pub(super) fn timeline(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let pw = self.plot_width(window);
        let ph = self.plot_height(window);
        let v0 = self.t0;
        let v1 = self.t0 + self.spp * pw as f64;
        let tracks = self.rows_in(v0, v1);
        let max_scroll = (tracks.len() as f32 * ROW_H - ph + ROW_H).max(0.0);
        self.row_scroll = self.row_scroll.min(max_scroll);
        let first_row = (self.row_scroll / ROW_H) as usize;
        let rows_visible = (ph / ROW_H) as usize + 2;
        let spp = self.spp;
        let x_of = move |t: f64| ((t - v0) / spp) as f32;
        let n_groups = tracks.iter().filter(|r| matches!(r, Row::Header { .. })).count();
        let n_lanes = tracks.iter().filter(|r| matches!(r, Row::Lane(_))).count();
        self.counts = format!("{n_groups} プロジェクト · {n_lanes} レーン");

        // 時間軸
        let mut axis = div().relative().h(px(AXIS_H)).w(px(pw)).overflow_hidden();
        let mut grid = div().absolute().top_0().left_0().size_full();
        for (t, label, major) in ticks(v0, v1, self.spp) {
            let x = x_of(t);
            if x < 0.0 {
                continue;
            }
            axis = axis.child(
                div()
                    .absolute()
                    .left(px(x + 3.0))
                    .top(px(if major { 2.0 } else { 14.0 }))
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(c(if major { theme::TEXT } else { theme::MUTED }))
                    .child(label),
            );
            grid = grid.child(div().absolute().left(px(x)).top_0().bottom_0().w(px(1.0)).bg(c(if major {
                0x353b47
            } else {
                theme::GRID
            })));
        }
        let nx = x_of(now());
        if (0.0..pw).contains(&nx) {
            grid = grid.child(div().absolute().left(px(nx)).top_0().bottom_0().w(px(1.0)).bg(c(theme::NOW)));
            axis = axis
                .child(div().absolute().left(px(nx - 3.0)).bottom_0().size(px(7.0)).rounded_full().bg(c(theme::NOW)));
        }

        let mut labels = div().relative().w(px(self.label_w)).h(px(ph)).overflow_hidden();
        let mut plot = div().relative().w(px(pw)).h(px(ph)).overflow_hidden().child(grid);
        let mut blocks: Vec<BlockPaint> = Vec::new();
        let mut row_y_of_lane = std::collections::HashMap::new();
        let selected = self.selected;

        for (row, r) in tracks.iter().enumerate().skip(first_row).take(rows_visible) {
            let y = row as f32 * ROW_H - self.row_scroll;
            let tr = match r {
                Row::Header { cwd, sessions, collapsed, active } => {
                    let collapsed = *collapsed;
                    let dim = !*active;
                    let (cwd_c, cwd_f) = (cwd.clone(), cwd.clone());
                    let focused = self.focus.as_ref() == Some(cwd);
                    let group = SharedString::from(format!("grp-{row}"));
                    labels = labels.child(
                        div()
                            .id(ElementId::NamedInteger("grp".into(), row as u64))
                            .group(group.clone())
                            .absolute()
                            .top(px(y))
                            .left_0()
                            .w(px(self.label_w))
                            .h(px(ROW_H))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .bg(c(0x1f232b))
                            .border_t_1()
                            .border_color(c(0x3a4150))
                            .when(dim, |d| d.opacity(0.45))
                            .text_xs()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .cursor_pointer()
                            .hover(|d| d.bg(c(0x262b35)))
                            .child(icons::icon(
                                if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                                theme::MUTED,
                            ))
                            .child(project_tile(cwd, 14.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c(theme::TEXT))
                                    .child(self.project_name(cwd)),
                            )
                            .child(div().flex_none().text_color(c(theme::SUBTLE)).child(format!("{}", sessions.len())))
                            // 右寄せは ml_auto ではなく詰め物で行う（auto マージンがあると gap が効かない）
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(ElementId::NamedInteger("grp-focus".into(), row as u64))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .gap_1()
                                    .px_1()
                                    .rounded_sm()
                                    .when(!focused, |d| d.invisible().group_hover(group.clone(), |s| s.visible()))
                                    .text_color(c(if focused { theme::ACCENT } else { theme::MUTED }))
                                    .hover(|d| d.bg(c(0x3a4150)).text_color(c(theme::TEXT)))
                                    .child(
                                        icons::icon(Icon::Filter, if focused { theme::ACCENT } else { theme::MUTED })
                                            .size_3(),
                                    )
                                    .child(if focused { "解除" } else { "絞り込む" })
                                    .tooltip(tip(
                                        if focused {
                                            "プロジェクトの絞り込みを解除"
                                        } else {
                                            "このプロジェクトだけを表示"
                                        },
                                        None,
                                    ))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        let f = if this.focus.as_ref() == Some(&cwd_f) {
                                            None
                                        } else {
                                            Some(cwd_f.clone())
                                        };
                                        this.set_focus(f, cx);
                                        cx.notify();
                                    })),
                            )
                            .tooltip(tip(cwd.clone(), None))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if !this.collapsed.remove(&cwd_c) {
                                    this.collapsed.insert(cwd_c.clone());
                                }
                                this.persist();
                                cx.notify();
                            })),
                    );
                    plot = plot.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(y))
                            .h(px(ROW_H))
                            .bg(c(0x1b1e25))
                            .border_t_1()
                            .border_color(c(0x3a4150)),
                    );
                    // 折りたたみ中はグループ内のメインレーンを 1 行に重ねて描く
                    if collapsed {
                        for &si in sessions {
                            let lane = &self.store.sessions[si].lanes[0];
                            let grouped = self.layout_cache.borrow_mut().aggregate((si, 0), lane, (v0, v1), self.spp, self.gran);
                            for b in grouped {
                                let x = x_of(b.start).max(-2.0);
                                let w = (x_of(b.end).min(pw + 2.0) - x).max(MIN_BLOCK);
                                let sel = Selection { session: si, lane: 0, first: b.first, last: b.last };
                                blocks.push(self.block_paint(sel, (x, y + 3.0, w, ROW_H - 6.0), false));
                            }
                        }
                    }
                    continue;
                }
                Row::Lane(t) => t,
            };
            row_y_of_lane.insert((tr.session, tr.lane), y);
            let s = &self.store.sessions[tr.session];
            let lane = &s.lanes[tr.lane];
            let new_session = row > 0 && matches!(&tracks[row - 1], Row::Lane(p) if p.session != tr.session);
            let lane_selected = selected.is_some_and(|sel| sel.session == tr.session && sel.lane == tr.lane);

            let (title, sub) = if tr.lane == 0 {
                let t = if s.title.is_empty() { "(無題)" } else { s.title.as_str() };
                (one_line(t, 80), false)
            } else {
                (format!("↳ {}", one_line(&lane.name, 80)), true)
            };
            let (badge, badge_color) = icons::source_icon(s.source);
            let (session, lane_i) = (tr.session, tr.lane);
            let indent = 16.0 + 16.0 * tr.depth as f32;
            let n = lane.events.len();
            let detail = {
                let (a, b) = lane.range().unwrap_or((s.start, s.end));
                let mut d = format!(
                    "{title}\n{} · {} – {}（{}）· {n} 件",
                    self.project_name(&s.cwd),
                    local(a).format("%m/%d %H:%M"),
                    local(b).format("%H:%M"),
                    fmt_dur(b - a)
                );
                if !lane.model.is_empty() {
                    d.push_str(&format!("\n{}", lane.model));
                }
                d
            };
            labels = labels.child(
                div()
                    .id(ElementId::NamedInteger("lbl".into(), row as u64))
                    .absolute()
                    .top(px(y))
                    .left_0()
                    .w(px(self.label_w))
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .pl(px(indent))
                    .when(new_session, |d| d.border_t_1().border_color(c(theme::BORDER)))
                    .when(lane_selected, |d| d.bg(c(theme::ACCENT_BG)))
                    .when(!tr.active, |d| d.opacity(0.4))
                    .text_xs()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|d| d.bg(c(0x232733)))
                    .when(!sub, |d| d.child(icons::icon(badge, badge_color)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(c(if sub { theme::SUB } else { theme::TEXT }))
                            .child(title),
                    )
                    .tooltip(tip(detail, None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.navigate(Selection { session, lane: lane_i, first: 0, last: n - 1 }, window, cx);
                    })),
            );
            if new_session {
                plot = plot.child(div().absolute().left_0().right_0().top(px(y)).h(px(1.0)).bg(c(theme::BORDER)));
            }
            if !tr.active {
                // 表示範囲外のレーンはグレーアウトする
                plot = plot.child(div().absolute().left_0().right_0().top(px(y)).h(px(ROW_H)).bg(c(0x000000).opacity(0.18)));
            }
            if lane_selected {
                plot = plot.child(
                    div().absolute().left_0().right_0().top(px(y)).h(px(ROW_H)).bg(c(theme::ACCENT).opacity(0.07)),
                );
            }
            let grouped = self.layout_cache.borrow_mut().aggregate((tr.session, tr.lane), lane, (v0, v1), self.spp, self.gran);
            for b in grouped {
                let x = x_of(b.start).max(-2.0);
                let x2 = x_of(b.end).min(pw + 2.0);
                let w = (x2 - x).max(MIN_BLOCK);
                let sel = Selection { session: tr.session, lane: tr.lane, first: b.first, last: b.last };
                blocks.push(self.block_paint(sel, (x, y + 3.0, w, ROW_H - 6.0), false));
            }
        }
        plot = plot.child(self.blocks_canvas(blocks));

        // サブエージェントの起動位置から子レーンへの接続線
        let lane_rows = tracks.iter().skip(first_row).take(rows_visible).filter_map(|r| match r {
            Row::Lane(t) => Some(t),
            Row::Header { .. } => None,
        });
        for tr in lane_rows.filter(|t| t.lane != 0 || t.link.is_some()) {
            let s = &self.store.sessions[tr.session];
            let lane = &s.lanes[tr.lane];
            let Some(&cy) = row_y_of_lane.get(&(tr.session, tr.lane)) else {
                continue;
            };
            let t0 = lane.events.first().map_or(s.start, |e| e.start);
            let (pkey, t) = match tr.link {
                Some((ps, pl, t)) => ((ps, pl), t),
                None => {
                    ((tr.session, s.parents.get(tr.lane).copied().flatten().unwrap_or(0)), lane.spawn_t.unwrap_or(t0))
                }
            };
            let py = row_y_of_lane.get(&pkey).copied().unwrap_or(-ROW_H);
            let line = c(0xa77bf3).opacity(0.7);
            // 起動位置で親レーンから下ろし、子レーンの先頭まで横に伸ばす
            let x = x_of(t).clamp(0.0, pw - 1.0);
            let top = py + ROW_H / 2.0;
            let mid = cy + ROW_H / 2.0;
            plot =
                plot.child(div().absolute().left(px(x)).top(px(top)).w(px(1.0)).h(px((mid - top).max(0.0))).bg(line));
            plot =
                plot.child(div().absolute().left(px(x - 2.5)).top(px(top - 2.5)).size(px(6.0)).rounded_full().bg(line));
            let x2 = x_of(t0).clamp(0.0, pw);
            if x2 > x {
                plot = plot.child(div().absolute().left(px(x)).top(px(mid)).w(px(x2 - x)).h(px(1.0)).bg(line));
            }
        }

        if tracks.is_empty() {
            plot = plot.child(self.empty_state(cx));
        } else {
            plot = plot.child(self.lane_map(&tracks, ph, cx));
        }

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .child(
                        div()
                            .w(px(self.label_w))
                            .h(px(AXIS_H))
                            .flex_none()
                            .px_2()
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(c(theme::SUBTLE))
                            .border_r_1()
                            .border_color(c(theme::BORDER))
                            .child("セッション"),
                    )
                    .child(axis)
                    .border_b_1()
                    .border_color(c(theme::BORDER)),
            )
            .child(div().flex().child(labels.border_r_1().border_color(c(theme::BORDER))).child(plot))
            .into_any_element()
    }

    /// 右端の縦のミニマップ。表示範囲内にイベントがあるレーンの位置と、いま見えている行の範囲を示す
    fn lane_map(&self, tracks: &[Row], ph: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let n = tracks.len().max(1) as f32;
        let marks: Vec<(f32, u32)> = tracks
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Row::Lane(t) if t.active => {
                    let selected = self.selected.is_some_and(|x| x.session == t.session && x.lane == t.lane);
                    let col = if selected {
                        0xffffff
                    } else if t.lane == 0 {
                        theme::ACCENT
                    } else {
                        theme::SUB
                    };
                    Some((i as f32 / n, col))
                }
                Row::Header { active: true, collapsed: true, .. } => Some((i as f32 / n, theme::ACCENT)),
                _ => None,
            })
            .collect();
        let total = n * ROW_H;
        let view = ((self.row_scroll / total).clamp(0.0, 1.0), ((self.row_scroll + ph) / total).clamp(0.0, 1.0));
        let bounds_cell = self.lane_map_bounds.clone();
        let map = canvas(
            move |b, _, _| bounds_cell.set(b),
            move |b, _, window: &mut Window, _| {
                let h: f32 = b.size.height.into();
                let w = b.size.width;
                let (v0, v1) = view;
                window.paint_quad(
                    fill(
                        Bounds::new(b.origin + point(px(0.0), px(v0 * h)), size(w, px(((v1 - v0) * h).max(4.0)))),
                        gpui::white().opacity(0.10),
                    )
                    .corner_radii(px(2.0)),
                );
                let mh = (h / n).clamp(2.0, 4.0);
                for &(f, col) in &marks {
                    window.paint_quad(fill(
                        Bounds::new(b.origin + point(px(2.0), px(f * h)), size(w - px(4.0), px(mh))),
                        c(col).opacity(0.9),
                    ));
                }
            },
        )
        .size_full();
        div()
            .id("lane-map")
            .absolute()
            .top_0()
            .right_0()
            .w(px(LANE_MAP_W))
            .h(px(ph))
            .bg(c(0x15181e).opacity(0.85))
            .border_l_1()
            .border_color(c(theme::BORDER))
            .cursor_pointer()
            .child(map)
            .tooltip(tip("表示範囲内のレーンの位置（クリック・ドラッグで移動）", None))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                    t.lane_map_drag = true;
                    t.lane_map_seek(e.position.y, total, ph);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |t, e: &MouseMoveEvent, _, cx| {
                if !t.lane_map_drag {
                    return;
                }
                if e.pressed_button != Some(MouseButton::Left) {
                    t.lane_map_drag = false;
                    return;
                }
                cx.stop_propagation();
                t.lane_map_seek(e.position.y, total, ph);
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|t, _, _, _| t.lane_map_drag = false))
    }

    /// 縦のミニマップ上の位置 y が表示の中央に来るようにスクロールする
    fn lane_map_seek(&mut self, y: Pixels, total: f32, ph: f32) {
        let b = self.lane_map_bounds.get();
        let h: f32 = b.size.height.into();
        if h <= 0.0 {
            return;
        }
        let f = (f32::from(y - b.origin.y) / h).clamp(0.0, 1.0);
        let max = (total - ph + ROW_H).max(0.0);
        self.row_scroll = (f * total - ph / 2.0).clamp(0.0, max);
    }

    /// 表示範囲に何もないときの案内
    pub(super) fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.loading.is_some();
        let has_any = !self.order.is_empty();
        div()
            .absolute()
            .top(px(48.0))
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .text_sm()
            .text_color(c(theme::MUTED))
            .child(if loading {
                "読み込み中…"
            } else if has_any {
                "この期間の作業履歴はありません"
            } else {
                "表示できる履歴がありません"
            })
            .when(!loading && has_any, |d| {
                d.child(
                    div()
                        .id("go-latest")
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .bg(c(theme::PANEL))
                        .text_xs()
                        .text_color(c(theme::TEXT))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::HOVER)))
                        .child(icons::icon(Icon::Clock, theme::MUTED))
                        .child("最新の作業へ移動")
                        .on_click(cx.listener(|t, _: &ClickEvent, window, cx| {
                            t.go_latest(window);
                            cx.notify();
                        })),
                )
            })
            .when(
                !loading && !has_any && (self.focus.is_some() || self.source != crate::prefs::SourceFilter::All),
                |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(c(theme::SUBTLE))
                            .child("読み込み元やプロジェクトの絞り込みを確認してください"),
                    )
                },
            )
            .into_any_element()
    }
}

/// 目盛り (時刻, ラベル, 主目盛りか)
fn ticks(v0: f64, v1: f64, spp: f64) -> Vec<(f64, String, bool)> {
    const STEPS: &[f64] = &[
        1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0, 10800.0, 21600.0, 43200.0, 86400.0, 172800.0,
        604800.0, 2592000.0,
    ];
    let step = STEPS.iter().copied().find(|s| s / spp >= 90.0).unwrap_or(2592000.0);
    let offset = local(v0).offset().local_minus_utc() as f64;
    let mut t = ((v0 + offset) / step).floor() * step - offset;
    let mut out = Vec::new();
    while t <= v1 && out.len() < 400 {
        let lt = local(t);
        let is_day = lt.hour() == 0 && lt.minute() == 0 && lt.second() == 0;
        let label = if step >= 86400.0 || is_day {
            format!("{}（{}）", lt.format("%-m/%-d"), super::header::weekday_ja(chrono::Datelike::weekday(&lt)))
        } else if step < 60.0 {
            lt.format("%H:%M:%S").to_string()
        } else {
            lt.format("%H:%M").to_string()
        };
        out.push((t, label, is_day));
        t += step;
    }
    out
}
