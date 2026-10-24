//! 変更ファイルと編集履歴の描画。

use super::{
    AnyElement, ClickEvent, Context, DetailCache, DetailTab, Div, ElementId, FILE_MAX, FileListKey,
    FileRow, FileStat, FontWeight, Icon, PER_EDIT, Rireki, Selection, SharedString, Window, c,
    diff_bar, div, event_icon, fmt_n, icons, label, local, px, rich, theme, tip,
};
use gpui::prelude::*;

impl Rireki {
    /// 選択の中で編集したファイルと、その編集履歴を表示する。
    pub(super) fn files_list(
        &self,
        sel: Selection,
        cache: &DetailCache,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.file_list.prepare(
            FileListKey {
                sel,
                generation: self.shown_selection_generation(),
                width: self.detail_content_width(window).to_bits(),
                open: self.file_open.clone(),
                all_files: self.expanded.contains("files"),
                all_diffs: self.expanded.contains("file-diff"),
            },
            cache,
        );
        self.file_list.rendered.set(0);
        if self.profile_eager_files() {
            return div()
                .id("profile-eager-files")
                .track_scroll(&self.detail_scroll)
                .size_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children((0..self.file_list.state.item_count()).map(|i| {
                    div()
                        .flex_none()
                        .min_w_0()
                        .child(self.file_list_item(sel, i, cx))
                }))
                .into_any_element();
        }
        let app = cx.entity().downgrade();
        gpui::list(self.file_list.state.clone(), move |index, _, cx| {
            app.update(cx, |app, cx| app.file_list_item(sel, index, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .size_full()
        .into_any_element()
    }

    pub(super) fn file_list_item(
        &self,
        sel: Selection,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.file_list
            .rendered
            .set(self.file_list.rendered.get() + 1);
        let Some(row) = self.file_list.row(index) else {
            return div().into_any_element();
        };
        let Some(cache) = self.cache(sel) else {
            return div().into_any_element();
        };
        let cwd = &self.detail_session(sel).cwd;
        // List は各行を独立して計測するため、編集履歴も表示領域の幅で配置する。
        div()
            .w_full()
            .min_w_0()
            .child(match row {
                FileRow::Summary | FileRow::File(_) | FileRow::More => {
                    self.file_row_view(cache, cwd, row, cx)
                }
                FileRow::History(file)
                | FileRow::Edit { file, .. }
                | FileRow::Empty(file)
                | FileRow::Expand(file) => {
                    let Some(file) = cache.files.get(file) else {
                        return div().into_any_element();
                    };
                    self.file_history(sel, cache, file, cwd, row, cx)
                }
            })
            .into_any_element()
    }

    pub(super) fn file_row_view(
        &self,
        d: &DetailCache,
        cwd: &str,
        row: FileRow,
        cx: &mut Context<Self>,
    ) -> Div {
        let all = self.expanded.contains("files");
        let (added, removed) = d
            .files
            .iter()
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        let mut list = div()
            .flex()
            .flex_col()
            .gap_1()
            .when(row == FileRow::Summary, |list| {
                list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .pb_1()
                        .child(label(format!(
                            "記録された編集 · {} ファイル",
                            d.files.len()
                        )))
                        .child(
                            div()
                                .text_xs()
                                .text_color(c(theme::OK))
                                .child(format!("+{}", fmt_n(added))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(c(theme::ERR))
                                .child(format!("−{}", fmt_n(removed))),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .text_color(c(theme::SUBTLE))
                                .child("編集履歴の差分"),
                        ),
                )
            });
        for (i, f) in d
            .files
            .iter()
            .enumerate()
            .take(if all { usize::MAX } else { FILE_MAX })
        {
            if row != FileRow::File(i) {
                continue;
            }
            let open = self.file_open.as_deref() == Some(f.path.as_str());
            let path = rich::rel_path(&f.path, cwd);
            let (dir, base) = rireki::paths::split_file(path);
            let (dir, base) = (dir.to_string(), base.to_string());
            let key = f.path.clone();
            let row = div()
                .id(ElementId::NamedInteger("file".into(), i as u64))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(open, |d| d.bg(c(theme::ACCENT_BG)))
                .when(!open, |d| d.hover(|s| s.bg(c(theme::HOVER))))
                .child(
                    icons::icon(
                        if open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        theme::SUBTLE,
                    )
                    .size_3()
                    .flex_none(),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(c(theme::SUBTLE))
                                .child(dir),
                        )
                        .child(
                            div()
                                .flex_none()
                                .max_w_full()
                                .truncate()
                                .text_color(c(theme::TEXT))
                                .when(open, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(base),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(c(theme::SUBTLE))
                        .child(format!("{}回", f.edits)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(c(theme::OK))
                        .child(format!("+{}", f.added)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(c(theme::ERR))
                        .child(format!("−{}", f.removed)),
                )
                .child(diff_bar(f.added, f.removed))
                .tooltip(tip(f.path.clone(), None))
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    t.file_open = if t.file_open.as_deref() == Some(key.as_str()) {
                        None
                    } else {
                        Some(key.clone())
                    };
                    t.expanded.remove("file-diff");
                    t.detail_changed(cx);
                }));
            list = list.child(row);
        }
        if row == FileRow::More && !all && d.files.len() > FILE_MAX {
            list = list.child(
                div()
                    .id("more-files")
                    .px_1p5()
                    .text_xs()
                    .text_color(c(theme::ACCENT))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(format!("ほか {} ファイル", d.files.len() - FILE_MAX))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        t.expanded.insert("files");
                        t.detail_changed(cx);
                    })),
            );
        }
        list.pb_1()
    }

    /// 1 イベントを選んでイベントのタブを開く
    pub(super) fn open_edit(
        &mut self,
        session: usize,
        lane: usize,
        idx: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = Selection {
            session,
            lane,
            first: idx,
            last: idx,
        };
        self.navigate(sel, w, cx);
        self.set_detail_tab(DetailTab::Events, cx);
        self.reveal(sel, false, w);
    }

    /// 開いたファイルの編集の履歴。選択の期間の上に編集を印で置いた帯と、編集ごとの差分
    pub(super) fn file_history(
        &self,
        sel: Selection,
        d: &DetailCache,
        f: &FileStat,
        cwd: &str,
        row: FileRow,
        cx: &mut Context<Self>,
    ) -> Div {
        let s = self.detail_session(sel);
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        let (start, end) = (evs[0].start, d.end());
        let span = (end - start).max(1.0);
        // 各編集でこのファイルに加えた変更
        let edits: Vec<(usize, Vec<&rich::FilePatch>)> = f
            .events
            .iter()
            .filter_map(|&i| {
                let ps: Vec<&rich::FilePatch> = d
                    .patches
                    .get(&i)?
                    .iter()
                    .filter(|p| p.path == f.path)
                    .collect();
                (!ps.is_empty()).then_some((i, ps))
            })
            .collect();
        let max_lines = edits
            .iter()
            .map(|(_, ps)| ps.iter().map(|p| p.added + p.removed).sum::<usize>())
            .max()
            .unwrap_or(1)
            .max(1);
        let (session, lane) = (sel.session, sel.lane);

        // 編集の帯: 横は時刻、印の高さは変更行数、色は追加と削除の割合
        let fmt = if local(start).date_naive() == local(end).date_naive() {
            "%H:%M"
        } else {
            "%m/%d %H:%M"
        };
        let mut strip = div()
            .relative()
            .h(px(34.0))
            .w_full()
            .rounded_md()
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(6.0))
                    .h(px(1.0))
                    .bg(c(theme::GRID)),
            );
        for (n, (i, ps)) in edits
            .iter()
            .enumerate()
            .filter(|_| matches!(row, FileRow::History(_)))
        {
            let e = &evs[*i];
            let (a, r) = ps
                .iter()
                .fold((0, 0), |(a, r), p| (a + p.added, r + p.removed));
            let frac = (((e.start - start) / span) as f32).clamp(0.0, 1.0);
            let h = 6.0 + 18.0 * ((a + r) as f32 / max_lines as f32).sqrt();
            let col = if a >= r { theme::OK } else { theme::ERR };
            strip = strip.child(
                div()
                    .id(ElementId::NamedInteger("edit-mark".into(), *i as u64))
                    .absolute()
                    .bottom(px(6.0))
                    .left(gpui::relative(frac * 0.96 + 0.01))
                    .w(px(5.0))
                    .h(px(h))
                    .rounded(px(2.0))
                    .bg(c(col).opacity(0.85))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::ACCENT)))
                    .tooltip(tip(
                        format!(
                            "#{} {} · {} · +{a} −{r}",
                            n + 1,
                            local(e.start).format("%H:%M:%S"),
                            e.name
                        ),
                        None,
                    ))
                    .on_click({
                        let idx = sel.first + i;
                        cx.listener(move |t, _: &ClickEvent, w, cx| {
                            t.open_edit(session, lane, idx, w, cx)
                        })
                    }),
            );
        }
        let axis = div()
            .flex()
            .text_xs()
            .text_color(c(theme::SUBTLE))
            .child(local(start).format(fmt).to_string())
            .child(div().flex_1())
            .child(local(end).format(fmt).to_string());

        // 編集ごとの差分
        let all = self.expanded.contains("file-diff");
        let mut diffs = div().flex().flex_col().gap_3();
        let mut hidden_total = if matches!(row, FileRow::Expand(_)) {
            edits
                .iter()
                .map(|(_, ps)| {
                    ps.iter()
                        .map(|p| p.rows.len())
                        .sum::<usize>()
                        .saturating_sub(PER_EDIT)
                })
                .sum()
        } else {
            0
        };
        for (n, (i, ps)) in edits.iter().enumerate() {
            if !matches!(row, FileRow::Edit { event, .. } if event == *i) {
                continue;
            }
            let e = &evs[*i];
            let (a, r) = ps
                .iter()
                .fold((0, 0), |(a, r), p| (a + p.added, r + p.removed));
            let (view, hidden) = rich::diff_view(
                ps.iter().copied(),
                cwd,
                if all { usize::MAX } else { PER_EDIT },
            );
            hidden_total += hidden;
            let color = theme::event_color(e);
            diffs = diffs.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .id(ElementId::NamedInteger("edit-head".into(), *i as u64))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_1()
                            .py_0p5()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .hover(|d| d.bg(c(theme::HOVER)))
                            .child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .rounded_full()
                                    .bg(c(theme::GRID))
                                    .text_color(c(theme::MUTED))
                                    .child(format!("#{}", n + 1)),
                            )
                            .child(event_icon(theme::event_icon(e), color, 12.0))
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(SharedString::from(e.name.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(c(theme::SUBTLE))
                                    .child(local(e.start).format("%m/%d %H:%M:%S").to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(c(theme::OK))
                                    .child(format!("+{a}")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(c(theme::ERR))
                                    .child(format!("−{r}")),
                            )
                            .child(icons::icon(Icon::ArrowRight, theme::SUBTLE).size_3())
                            .tooltip(tip("このイベントを開く", None))
                            .on_click({
                                let idx = sel.first + i;
                                cx.listener(move |t, _: &ClickEvent, w, cx| {
                                    t.open_edit(session, lane, idx, w, cx)
                                })
                            }),
                    )
                    .child(view)
                    .when(hidden > 0, |d| {
                        d.child(
                            div()
                                .px_1()
                                .text_xs()
                                .text_color(c(theme::SUBTLE))
                                .child(format!("… {} 行を省略", fmt_n(hidden))),
                        )
                    }),
            );
        }
        if matches!(row, FileRow::Expand(_)) && hidden_total > 0 {
            diffs = diffs.child(
                div()
                    .id("file-diff-all")
                    .text_xs()
                    .text_color(c(theme::ACCENT))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(format!("省略した {} 行をすべて表示", fmt_n(hidden_total)))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        t.expanded.insert("file-diff");
                        t.detail_changed(cx);
                    })),
            );
        }
        if matches!(row, FileRow::Empty(_)) {
            diffs = diffs.child(
                div()
                    .text_xs()
                    .text_color(c(theme::SUBTLE))
                    .child("差分は記録されていません"),
            );
        }
        div()
            .ml(px(10.0))
            .pb_3()
            .pl_3()
            .when(matches!(row, FileRow::History(_)), |d| d.pt_1())
            .border_l_2()
            .border_color(c(theme::ACCENT).opacity(0.5))
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .when(matches!(row, FileRow::History(_)), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(label(format!("編集の時系列 · {} 回", edits.len())))
                        .child(strip)
                        .child(axis),
                )
            })
            .child(diffs)
    }
}
