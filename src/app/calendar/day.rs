//! 日・週表示の時刻軸、セッションブロックと指示の描画。

use super::*;

impl Rireki {
    pub(in crate::app) fn calendar(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.mode == Mode::Month {
            return self.month(window, cx);
        }
        let week = self.mode == Mode::Week;
        let days = if week { 7 } else { 1 };
        let first = if week { self.week_start() } else { self.day0 };
        let total_w = self.main_width(window);
        let ph = self.plot_height(window);
        let col_w = (total_w - HOUR_LABEL_W) / days as f32;
        let pph = self.px_per_hour;
        let spp = 3600.0 / pph as f64;
        let layout = self.calendar_layout(first, days);
        // 期間を移ったとき、見えている時刻に作業がなければ最初の作業の少し前まで動かす
        if std::mem::take(&mut self.calendar_cache.seek) {
            let (v0, v1) = (
                self.cal_scroll as f64 * spp,
                (self.cal_scroll + ph) as f64 * spp,
            );
            let mut visible = false;
            let mut earliest = f64::INFINITY;
            for (d, day) in layout.days.iter().enumerate() {
                let ds = midnight(first + Duration::days(d as i64));
                for g in &day.groups {
                    visible |= g.end - ds >= v0 && g.start - ds <= v1;
                    earliest = earliest.min(g.start - ds);
                }
            }
            if !visible && earliest.is_finite() {
                self.cal_scroll = (earliest / spp) as f32 - pph * 0.5;
            }
        }
        self.cal_scroll = clamp_scroll(self.cal_scroll, pph, ph);
        let scroll = self.cal_scroll;
        let today = Local::now().date_naive();
        let columns = self.calendar_cache.columns(spp);
        let y_of = |ds: f64, t: f64| ((t - ds) / spp) as f32 - scroll;

        let mut header = div()
            .flex()
            .h(px(DAY_HEAD_H))
            .flex_none()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(div().w(px(HOUR_LABEL_W)).flex_none());
        let origin = self.hit_origin.clone();
        let mut body = div()
            .relative()
            .h(px(ph))
            .w(px(total_w))
            .overflow_hidden()
            .child(
                canvas(move |b, _, _| origin.set(b.origin), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            );

        // 時刻ラベルとグリッド
        let step_h = [0.25f32, 0.5, 1.0, 2.0, 3.0]
            .into_iter()
            .find(|s| s * pph >= 40.0)
            .unwrap_or(3.0);
        let mut h = 0.0f32;
        while h <= 24.0 {
            let y = h * pph - scroll;
            if y > -20.0 && y < ph + 20.0 {
                let mins = (h * 60.0).round() as u32;
                body = body
                    .child(
                        div()
                            .absolute()
                            .left(px(HOUR_LABEL_W))
                            .right_0()
                            .top(px(y))
                            .h(px(1.0))
                            .bg(c(theme::GRID)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(4.0))
                            .top(px(y - 7.0))
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(format!("{:02}:{:02}", mins / 60, mins % 60)),
                    );
            }
            h += step_h;
        }

        let mut hits = Vec::new();
        let mut blocks = Vec::new();
        let mut now_lines = Vec::new();
        for d in 0..days {
            let date = first + Duration::days(d as i64);
            let ds = midnight(date);
            let x0 = HOUR_LABEL_W + d as f32 * col_w;
            let is_today = date == today;
            let day = &layout.days[d];
            header = header.child(self.day_head(d, date, day, col_w, week, cx));
            body = body.child(
                div()
                    .absolute()
                    .left(px(x0))
                    .top_0()
                    .bottom_0()
                    .w(px(1.0))
                    .bg(c(theme::BORDER)),
            );
            if is_today {
                body = body.child(
                    div()
                        .absolute()
                        .left(px(x0))
                        .w(px(col_w))
                        .top_0()
                        .bottom_0()
                        .bg(c(theme::ACCENT).opacity(0.05)),
                );
            }
            let cols = &columns[d];
            for (k, g) in day.groups.iter().enumerate() {
                let gw = (col_w - 4.0) / cols[k].2 as f32;
                let x = x0 + 2.0 + cols[k].0 as f32 * gw;
                let gw = gw * cols[k].1 as f32;
                let y = y_of(ds, g.start);
                let h = (y_of(ds, g.end) - y).max(MIN_H);
                if y + h < 0.0 || y > ph {
                    continue;
                }
                let w = (gw - 2.0).max(3.0);
                let info = self.block_info(&day.tracks, g);
                if let Some(sel) = info.sel {
                    hits.push(((x, y, w, h), sel));
                }
                // 前日から続く・翌日へ続く辺は角を丸めない
                let cont = (g.start <= ds, g.end >= ds + 86_399.0);
                let prompts = if w >= PROMPT_MIN_W {
                    self.prompt_rows(&info, (x, y, w, h), &|t| y_of(ds, t), &mut hits)
                } else {
                    Vec::new()
                };
                blocks.push(self.session_block(&info, (x, y, w, h), cont, prompts, (d, k)));
            }
            if is_today {
                let y = y_of(ds, now());
                now_lines.push(
                    div()
                        .absolute()
                        .left(px(x0))
                        .w(px(col_w))
                        .top(px(y))
                        .h(px(2.0))
                        .bg(c(theme::NOW)),
                );
                now_lines.push(
                    div()
                        .absolute()
                        .left(px(x0 - 4.0))
                        .top(px(y - 3.0))
                        .size(px(8.0))
                        .rounded_full()
                        .bg(c(theme::NOW)),
                );
            }
        }
        self.hits = hits;
        self.counts = layout.counts.clone();
        body = body.children(blocks).children(now_lines);
        if layout.empty {
            body = body.child(self.empty_state(cx));
        }
        div()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// 日の見出し。曜日・日付と、その日の稼働時間・セッション数
    fn day_head(
        &self,
        d: usize,
        date: NaiveDate,
        day: &DayLayout,
        w: f32,
        week: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_today = date == Local::now().date_naive();
        let weekend = matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
        let summary = if day.groups.is_empty() {
            "—".to_string()
        } else if w >= 110.0 {
            format!("{} · {} 件", fmt_hm(day.active), day.sessions)
        } else {
            fmt_hm(day.active)
        };
        div()
            .id(ElementId::NamedInteger("day".into(), d as u64))
            .w(px(w))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_sm()
                    .text_color(c(if is_today {
                        theme::ACCENT
                    } else if weekend {
                        theme::MUTED
                    } else {
                        theme::TEXT
                    }))
                    .child(div().text_xs().child(weekday_ja(date.weekday())))
                    .child(
                        div()
                            .min_w(px(20.0))
                            .h(px(20.0))
                            .px_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .when(is_today, |d| {
                                d.bg(c(theme::ACCENT)).text_color(gpui::white())
                            })
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if week {
                                date.day().to_string()
                            } else {
                                date.format("%-m/%-d").to_string()
                            }),
                    ),
            )
            .child(div().text_xs().text_color(c(theme::SUBTLE)).child(summary))
            .when(week, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(c(theme::HOVER)))
                    .tooltip(tip("この日を日表示で開く", Some("D")))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        t.day0 = date;
                        t.set_mode(Mode::Day, w);
                        cx.notify();
                    }))
            })
    }

    /// セッションのブロック。上端が開始時刻。名前・時刻・プロジェクトを高さに応じて出す
    fn session_block(
        &self,
        info: &BlockInfo,
        (x, y, w, h): (f32, f32, f32, f32),
        (from_prev, to_next): (bool, bool),
        prompts: Vec<AnyElement>,
        (d, k): (usize, usize),
    ) -> AnyElement {
        let color = theme::project_color(&info.cwd);
        let selected = info
            .sel
            .is_some_and(|s| self.selected.is_some_and(|x| x.session == s.session));
        let hovered = info.sel.is_some_and(|s| self.hovered == Some(s));
        let (logo, logo_color) = crate::icons::source_icon(info.source);
        let radius = px(4.0);
        // ブロックの上端が画面より上にあるときも、名前は見える位置に留める
        let sticky = (-y).clamp(0.0, (h - MIN_H).max(0.0));
        let text = w >= 28.0;
        let mut inner = div()
            .absolute()
            .top(px(sticky))
            .left(px(3.0))
            .right_0()
            .flex()
            .flex_col()
            .px_1()
            .pt(px(1.0))
            .text_xs()
            .overflow_hidden()
            .when(text, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(16.0))
                        .when(w >= 60.0, |d| {
                            d.child(
                                crate::icons::icon(logo, logo_color)
                                    .size(px(11.0))
                                    .flex_none(),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(theme::TEXT))
                                .child(info.title.clone()),
                        )
                        .when(info.live, |d| d.child(dot(theme::LIVE, 6.0))),
                )
            });
        if text && h - sticky >= 32.0 {
            let meta = match info.sel {
                Some(_) if info.subs > 0 && w >= 120.0 => {
                    format!("{} · +{} サブ", self.project_name(&info.cwd), info.subs)
                }
                _ => self.project_name(&info.cwd),
            };
            inner = inner.child(
                div()
                    .h(px(14.0))
                    .truncate()
                    .text_color(c(theme::MUTED))
                    .child(meta),
            );
        }
        div()
            .id(ElementId::NamedInteger(
                SharedString::from(format!("cal-block-{d}")),
                k as u64,
            ))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .when(!from_prev, |d| d.rounded_t(radius))
            .when(!to_next, |d| d.rounded_b(radius))
            .overflow_hidden()
            .bg(c(color).opacity(if selected || hovered { 0.42 } else { 0.24 }))
            .border_1()
            .border_color(if selected {
                gpui::white()
            } else if hovered {
                gpui::white().opacity(0.6)
            } else {
                c(color).opacity(0.55)
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(3.0))
                    .bg(c(color)),
            )
            .children(prompts)
            .child(inner)
            .into_any_element()
    }

    /// 幅のあるブロックの中に、ユーザーの指示をその時刻の位置に並べる。重なるものは省く
    fn prompt_rows(
        &self,
        info: &BlockInfo,
        (bx, by, w, h): (f32, f32, f32, f32),
        y_of: &dyn Fn(f64) -> f32,
        hits: &mut Vec<Hit>,
    ) -> Vec<AnyElement> {
        let Some(sel) = info.sel else {
            return Vec::new();
        };
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let users: Vec<usize> = (sel.first..=sel.last)
            .filter(|&i| lane.events[i].kind == Kind::User)
            .collect();
        // 名前と（あれば）プロジェクトの行の下から並べる
        let top = by.max(0.0) + if h >= 32.0 { 32.0 } else { 17.0 };
        let mut last = f32::NEG_INFINITY;
        let mut out = Vec::new();
        for (n, &i) in users.iter().enumerate() {
            let y = y_of(lane.events[i].start).max(top);
            if y < last + 15.0 || y + 14.0 > by + h {
                continue;
            }
            last = y;
            let end = users.get(n + 1).map_or(sel.last, |&j| j - 1);
            let turn = Selection {
                session: sel.session,
                lane: sel.lane,
                first: i,
                last: end,
            };
            hits.push(((bx + 4.0, y, w - 6.0, 14.0), turn));
            let hovered = self.hovered == Some(turn);
            let selected = self.selected == Some(turn);
            out.push(
                div()
                    .absolute()
                    .top(px(y - by))
                    .left(px(4.0))
                    .right(px(2.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .when(hovered || selected, |d| {
                        d.bg(gpui::white().opacity(if selected { 0.16 } else { 0.08 }))
                    })
                    .text_xs()
                    .child(dot(USER, 5.0))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(local(lane.events[i].start).format("%H:%M").to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c(theme::TEXT))
                            .child(one_line(&lane.events[i].summary, 120)),
                    )
                    .into_any_element(),
            );
        }
        out
    }
}
