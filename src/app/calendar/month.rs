//! 月表示の日付グリッドと、その日に動いたセッションの描画。

use super::*;

impl Rireki {
    pub(super) fn month(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let first = month_grid_start(self.day0);
        let month = self.day0.month();
        let total_w = self.main_width(window);
        let ph = self.plot_height(window) + DAY_HEAD_H - MONTH_HEAD_H;
        let layout = self.calendar_layout(first, 42);
        self.calendar_cache.seek = false;
        self.hits.clear();
        self.counts = layout.counts.clone();
        let cell_w = total_w / 7.0;
        let cell_h = ph / 6.0;
        let max = layout
            .days
            .iter()
            .map(|d| d.active)
            .fold(0.0, f64::max)
            .max(1.0);
        let today = Local::now().date_naive();
        let rows_fit = ((cell_h - 26.0) / 17.0).floor().max(0.0) as usize;
        let head = div()
            .flex()
            .h(px(MONTH_HEAD_H))
            .flex_none()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children((0..7).map(|i| {
                let wd = (first + Duration::days(i)).weekday();
                div()
                    .w(px(cell_w))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(c(if i >= 5 { theme::MUTED } else { theme::TEXT }))
                    .child(weekday_ja(wd))
            }));
        let mut grid = div().flex().flex_col().h(px(ph)).overflow_hidden();
        for r in 0..6 {
            let mut row = div().flex().w(px(total_w)).h(px(cell_h)).flex_none();
            for k in 0..7 {
                let i = r * 7 + k;
                let date = first + Duration::days(i as i64);
                let day = &layout.days[i];
                let in_month = date.month() == month;
                let shown = if day.sessions > rows_fit {
                    rows_fit.saturating_sub(1)
                } else {
                    day.sessions
                };
                let heat = (day.active / max).sqrt() as f32;
                let mut cell = div()
                    .id(ElementId::NamedInteger("month-day".into(), i as u64))
                    .w(px(cell_w))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .p_1()
                    .overflow_hidden()
                    .border_r_1()
                    .border_b_1()
                    .border_color(c(theme::GRID))
                    .when(day.active > 0.0, |d| {
                        d.bg(c(theme::HEAT[2]).opacity(0.05 + 0.2 * heat))
                    })
                    .when(!in_month, |d| d.opacity(0.45))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::HOVER)))
                    .tooltip(tip("この日を日表示で開く", None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        t.day0 = date;
                        t.set_mode(Mode::Day, w);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(20.0))
                            .text_xs()
                            .child(
                                div()
                                    .min_w(px(20.0))
                                    .h(px(20.0))
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .when(date == today, |d| {
                                        d.bg(c(theme::ACCENT)).text_color(gpui::white())
                                    })
                                    .child(if date.day() == 1 {
                                        format!("{}/{}", date.month(), date.day())
                                    } else {
                                        date.day().to_string()
                                    }),
                            )
                            .child(div().flex_1())
                            .when(day.active > 0.0, |d| {
                                d.child(div().text_color(c(theme::MUTED)).child(fmt_hm(day.active)))
                            }),
                    );
                for g in day.month_sessions(shown) {
                    let si = g.session;
                    let start = g.start;
                    let info = self.block_info(&day.tracks, g);
                    let sel = info.sel;
                    let s = &self.store.sessions[si];
                    let selected = self.selected.is_some_and(|x| x.session == si);
                    cell = cell.child(
                        div()
                            .id(ElementId::NamedInteger(
                                SharedString::from(format!("month-chip-{i}")),
                                si as u64,
                            ))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .h(px(16.0))
                            .px_1()
                            .rounded_sm()
                            .text_xs()
                            .when(selected, |d| d.bg(c(theme::SELECT)))
                            .hover(|d| d.bg(gpui::white().opacity(0.1)))
                            .child(dot(theme::project_color(&s.cwd), 7.0))
                            .when(cell_w >= 120.0, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_color(c(theme::SUBTLE))
                                        .child(local(start).format("%H:%M").to_string()),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(info.title.clone()),
                            )
                            .when(info.live, |d| d.child(dot(theme::LIVE, 6.0)))
                            .tooltip(tip(
                                format!("{} · {}", self.project_name(&s.cwd), info.title),
                                None,
                            ))
                            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                cx.stop_propagation();
                                if let Some(sel) = sel {
                                    t.navigate(sel, w, cx);
                                }
                            })),
                    );
                }
                if shown < day.sessions {
                    cell = cell.child(
                        div()
                            .px_1()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(format!("ほか {} 件", day.sessions - shown)),
                    );
                }
                row = row.child(cell);
            }
            grid = grid.child(row);
        }
        let mut view = div()
            .relative()
            .flex()
            .flex_col()
            .w(px(total_w))
            .child(head)
            .child(grid);
        if layout.empty {
            view = view.child(self.empty_state(cx));
        }
        view.into_any_element()
    }
}
