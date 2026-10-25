//! 時間軸・カレンダーの描画。集計済みの行と可視範囲だけを参照する。

use super::{Kind, Mode, Model, Snapshot, clean, duration, local, midnight, tokens, weekday};
use chrono::Datelike;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

impl Model {
    pub fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        snapshot: &Snapshot,
        order: &[usize],
    ) {
        if area.is_empty() {
            return;
        }
        let label_width = (area.width / 3)
            .clamp(12, 42)
            .min(area.width.saturating_sub(1));
        let plot_width = area.width.saturating_sub(label_width + 3).max(1);
        if self.plot_width != plot_width {
            self.plot_width = plot_width;
            self.dirty = true;
        }
        self.ensure_rows(snapshot, order);
        let [heading, overview, body, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(if area.height >= 12 {
                if self.settings.mode == Mode::Timeline {
                    2
                } else if self.settings.mode == Mode::Month {
                    8
                } else {
                    10
                }
            } else {
                0
            }),
            Constraint::Min(1),
            Constraint::Length(if area.height >= 7 { 3 } else { 1 }),
        ])
        .areas(area);
        self.draw_heading(frame, heading);
        if self.settings.mode == Mode::Timeline {
            self.draw_minimap(frame, overview);
        } else if self.settings.mode == Mode::Month {
            self.draw_month(frame, overview);
        } else {
            self.draw_calendar(frame, overview);
        }
        if self.settings.mode != Mode::Timeline && self.show_projects {
            self.draw_projects(frame, body);
        } else {
            self.draw_rows(frame, body, label_width);
        }
        self.draw_footer(frame, footer, snapshot);
        if self.input.is_some() {
            self.draw_input(frame, area);
        }
    }

    fn draw_heading(&self, frame: &mut Frame<'_>, area: Rect) {
        let (a, b) = self.range();
        let period = if self.settings.mode == Mode::Month {
            self.day.format("%Y年%m月").to_string()
        } else {
            format!(
                "{} — {}",
                local(a).format("%Y-%m-%d %H:%M"),
                local(b).format("%Y-%m-%d %H:%M")
            )
        };
        let options = format!(
            "{} · {} · {} · {}",
            if self.settings.cost {
                "API換算料金"
            } else {
                "イベント"
            },
            if self.settings.directory {
                "ディレクトリ"
            } else {
                "レーン"
            },
            self.settings.granularity.label(),
            if self.settings.in_view_only {
                "範囲内のみ"
            } else {
                "すべての行"
            }
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{}  ", self.settings.mode.label()),
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(period),
                ]),
                Line::styled(options, Style::default().fg(MUTED)),
            ]),
            area,
        );
    }

    fn draw_minimap(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        let Some((a, b)) = self.bounds else {
            return;
        };
        let width = area.width.saturating_sub(2) as usize;
        if width == 0 {
            return;
        }
        let max = self.minimap.iter().copied().max().unwrap_or(1).max(1) as f64;
        let bars = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let mut spans = vec![Span::raw("│")];
        for i in 0..width {
            let bin = i * 64 / width;
            let value = self.minimap.get(bin).copied().unwrap_or(0);
            let time = a + (b - a) * i as f64 / width as f64;
            let active = time >= self.range.0 && time <= self.range.1;
            let style = Style::default().fg(if active { ACCENT } else { MUTED });
            spans.push(Span::styled(
                bars[if value == 0 {
                    0
                } else {
                    (value as f64 / max * 8.0).ceil() as usize
                }]
                .to_string(),
                if active {
                    style.bg(Color::DarkGray)
                } else {
                    style
                },
            ));
        }
        spans.push(Span::raw("│"));
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(spans),
                Line::styled(
                    format!(
                        "{}  時間分布  {{/}} 前/次の活動 · Ctrl+Home/End 最初/最新  {}",
                        local(a).format("%Y-%m-%d"),
                        local(b).format("%Y-%m-%d")
                    ),
                    Style::default().fg(MUTED),
                ),
            ]),
            area,
        );
    }

    fn draw_month(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        let width = (area.width / 7).max(1);
        let max_active = self
            .days
            .iter()
            .map(|day| day.active)
            .fold(0.0, f64::max)
            .max(1.0);
        for (i, name) in ["月", "火", "水", "木", "金", "土", "日"]
            .iter()
            .enumerate()
        {
            let x = area.x + i as u16 * width;
            if x >= area.right() {
                continue;
            }
            frame.render_widget(
                Paragraph::new(*name).style(Style::default().fg(ACCENT)),
                Rect::new(x, area.y, width.min(area.right() - x), 1),
            );
        }
        for (i, day) in self.days.iter().enumerate() {
            let x = area.x + i as u16 % 7 * width;
            let y = area.y + 1 + i as u16 / 7;
            if x >= area.right() || y >= area.bottom() {
                continue;
            }
            let text = if width >= 13 {
                format!(
                    "{:>2} {} {}",
                    day.date.day(),
                    day.sessions,
                    duration(day.active)
                )
            } else if width >= 6 {
                format!("{:>2} {:>2}", day.date.day(), day.sessions)
            } else {
                format!("{:>2}", day.date.day())
            };
            let color = if day.active > max_active * 0.66 {
                Color::Green
            } else if day.sessions > 0 {
                Color::Cyan
            } else {
                MUTED
            };
            let mut style = Style::default().fg(color);
            if day.date.month() != self.day.month() {
                style = style.add_modifier(Modifier::DIM);
            }
            if day.date == self.day {
                style = style.bg(Color::Blue);
            }
            frame.render_widget(
                Paragraph::new(text).style(style),
                Rect::new(x, y, width.min(area.right() - x), 1),
            );
        }
        if area.height >= 8 {
            frame.render_widget(
                Paragraph::new(
                    "日 / セッション数 / 稼働時間 · Alt+←/→ 日を選択 · g 日付指定 · z 時間軸で開く",
                )
                .style(Style::default().fg(MUTED)),
                Rect::new(area.x, area.bottom() - 1, area.width, 1),
            );
        }
    }

    fn draw_calendar(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() || self.days.is_empty() {
            return;
        }
        let hour_width = 6u16.min(area.width);
        let width = ((area.width - hour_width) / self.days.len() as u16).max(1);
        let rows = area.height.saturating_sub(2);
        if rows == 0 {
            return;
        }
        for (i, day) in self.days.iter().enumerate() {
            let x = area.x + hour_width + i as u16 * width;
            if x >= area.right() {
                break;
            }
            let title = if width > 17 {
                format!(
                    "{} {} · {}",
                    day.date.format("%m/%d"),
                    weekday(day.date),
                    duration(day.active)
                )
            } else {
                format!("{} {}", day.date.format("%m/%d"), weekday(day.date))
            };
            frame.render_widget(
                Paragraph::new(title).style(Style::default().fg(ACCENT)),
                Rect::new(x, area.y, width.min(area.right() - x), 1),
            );
            let start = midnight(day.date);
            for row in 0..rows {
                let lo =
                    start + (self.hour_start + self.hour_span * row as f64 / rows as f64) * 3600.0;
                let hi = start
                    + (self.hour_start + self.hour_span * (row + 1) as f64 / rows as f64) * 3600.0;
                let count = day
                    .intervals
                    .iter()
                    .filter(|&&(a, b)| a < hi && b >= lo)
                    .count();
                let cell = if count == 0 {
                    "·".into()
                } else {
                    format!("{} {}件", "█".repeat((width as usize / 3).max(1)), count)
                };
                frame.render_widget(
                    Paragraph::new(cell).style(Style::default().fg(if count > 0 {
                        Color::Green
                    } else {
                        MUTED
                    })),
                    Rect::new(x, area.y + row + 1, width.min(area.right() - x), 1),
                );
            }
        }
        for row in 0..rows {
            let hour = self.hour_start + self.hour_span * row as f64 / rows as f64;
            frame.render_widget(
                Paragraph::new(format!(
                    "{:02}:{:02}",
                    hour as u32,
                    (hour.fract() * 60.0) as u32
                ))
                .style(Style::default().fg(MUTED)),
                Rect::new(area.x, area.y + row + 1, hour_width, 1),
            );
        }
        frame.render_widget(
            Paragraph::new("稼働区間（15分以上の空白で分割） · +/− 時間目盛 · Ctrl+↑/↓ 時刻移動")
                .style(Style::default().fg(MUTED)),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }

    pub(super) fn draw_projects(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if area.is_empty() {
            return;
        }
        self.page = usize::from(area.height).saturating_sub(1).max(1);
        frame.render_widget(
            Paragraph::new("プロジェクト別の期間内稼働時間 · 長い順 · 同時実行セッションは加算")
                .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if body.is_empty() {
            return;
        }
        if self.projects.is_empty() {
            frame.render_widget(
                Paragraph::new("この期間に記録はありません").style(Style::default().fg(MUTED)),
                body,
            );
            return;
        }
        if self.project_selected < self.project_offset {
            self.project_offset = self.project_selected;
        }
        if self.project_selected >= self.project_offset + self.page {
            self.project_offset = self.project_selected + 1 - self.page;
        }
        self.project_offset = self
            .project_offset
            .min(self.projects.len().saturating_sub(self.page));
        let peak = self
            .projects
            .first()
            .map_or(1.0, |(_, seconds)| *seconds)
            .max(1.0);
        let bar_width = usize::from(area.width).saturating_sub(62).min(20);
        for (i, (cwd, seconds)) in self
            .projects
            .iter()
            .enumerate()
            .skip(self.project_offset)
            .take(self.page)
        {
            let selected = i == self.project_selected;
            let style = if selected {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                Style::default()
            };
            let bar = "█".repeat((seconds / peak * bar_width as f64).round() as usize);
            let text = format!(
                "{} {:>7}  {:<bar_width$} {}",
                if selected { "›" } else { " " },
                duration(*seconds),
                bar,
                clean(cwd)
            );
            frame.render_widget(
                Paragraph::new(text).style(style),
                Rect::new(
                    body.x,
                    body.y + (i - self.project_offset) as u16,
                    body.width,
                    1,
                ),
            );
        }
    }

    fn draw_rows(&mut self, frame: &mut Frame<'_>, area: Rect, label_width: u16) {
        if area.is_empty() {
            return;
        }
        self.page = area.height as usize;
        if self.selected < self.offset {
            self.offset = self.selected;
        }
        if self.selected >= self.offset + self.page {
            self.offset = self.selected + 1 - self.page;
        }
        self.offset = self.offset.min(self.rows.len().saturating_sub(self.page));
        if self.rows.is_empty() {
            let message = if self.settings.mode == Mode::Timeline
                && self.settings.cost
                && self.cost.is_none()
            {
                "料金を集計中…"
            } else {
                "この期間に表示する履歴はありません。L:最新の作業  0:全期間  g:期間指定"
            };
            frame.render_widget(
                Paragraph::new(message).style(Style::default().fg(MUTED)),
                area,
            );
            return;
        }
        for (row_index, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(self.page)
        {
            let selected = row_index == self.selected;
            let y = area.y + (row_index - self.offset) as u16;
            let rect = Rect::new(area.x, y, area.width, 1);
            let base = Style::default().bg(if selected { Color::Blue } else { Color::Reset });
            if row.header {
                let icon = if self.settings.collapsed.contains(&row.cwd) {
                    "▶"
                } else {
                    "▼"
                };
                frame.render_widget(
                    Paragraph::new(format!("{icon} {}", row.label))
                        .style(base.fg(ACCENT).add_modifier(Modifier::BOLD)),
                    rect,
                );
                continue;
            }
            let [label, plot] =
                Layout::horizontal([Constraint::Length(label_width), Constraint::Min(0)])
                    .areas(rect);
            frame.render_widget(
                Paragraph::new(row.label.clone()).style(base.fg(if row.items.is_empty() {
                    MUTED
                } else {
                    Color::White
                })),
                label,
            );
            let width = plot.width as usize;
            let mut cells: Vec<Span<'static>> = vec![Span::styled("·", base.fg(MUTED)); width];
            let max_cost = row
                .items
                .iter()
                .filter_map(|item| item.usage.map(|u| u.cost))
                .fold(0.0, f64::max)
                .max(f64::MIN_POSITIVE);
            for (ii, item) in row.items.iter().enumerate() {
                let span = (row.range.1 - row.range.0).max(1.0);
                let left = (((item.start - row.range.0) / span * width as f64)
                    .floor()
                    .max(0.0) as usize)
                    .min(width);
                let right = (((item.end - row.range.0) / span * width as f64)
                    .ceil()
                    .max(0.0) as usize)
                    .max(left + 1)
                    .min(width);
                let color = if item.usage.is_some() {
                    Color::Green
                } else {
                    kind_color(item.kind)
                };
                let chosen = selected && ii == self.item;
                let symbol = item.usage.map_or("━", |usage| {
                    let levels = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
                    levels[((usage.cost / max_cost * 7.0) as usize).min(7)]
                });
                for cell in &mut cells[left..right] {
                    *cell = Span::styled(
                        if chosen { "█" } else { symbol },
                        base.fg(color).add_modifier(if chosen {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    );
                }
            }
            frame.render_widget(Paragraph::new(Line::from(cells)), plot);
        }
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect, snapshot: &Snapshot) {
        if self.settings.mode != Mode::Timeline && self.show_projects {
            let selected = self
                .projects
                .get(self.project_selected)
                .map_or_else(String::new, |(cwd, seconds)| {
                    format!("{seconds:.1} 秒 · {}", clean(cwd))
                });
            frame.render_widget(Paragraph::new(vec![
                Line::raw(selected),
                Line::styled(format!("プロジェクト {}/{} · ↑↓/PgUp/PgDn/Home/End 選択 · Enter 活動一覧へ · u 一覧を切替", self.project_selected + usize::from(!self.projects.is_empty()), self.projects.len()), Style::default().fg(ACCENT)),
                Line::styled("←/→ 期間移動 · d/w/m 日/週/月 · s/p/P 読み込み元/プロジェクトの絞り込み", Style::default().fg(MUTED)),
            ]), area);
            return;
        }
        let selected = if self.date_focus && self.settings.mode != Mode::Timeline {
            format!(
                "選択日 {} · Enter 日表示 · z 選択日を時間軸で開く · ↑/↓ セッション一覧へ",
                self.day
            )
        } else {
            self.current_item()
                .map(|item| {
                    let session = &snapshot.sessions[item.selection.session];
                    let lane = &session.lanes[item.selection.lane];
                    format!(
                        "{} — {} · {} [{}] · {}",
                        local(item.start).format("%m/%d %H:%M:%S"),
                        local(item.end).format("%H:%M:%S"),
                        clean(&lane.name),
                        session.source.label(),
                        item.label
                    )
                })
                .unwrap_or_else(|| {
                    "↑/↓ 行  </> ブロック  Enter 詳細  Space プロジェクト折畳".into()
                })
        };
        let counts = if let Some(item) = self.current_item()
            && let Some(usage) = item.usage
        {
            format!(
                "入力 {} · キャッシュ読取 {} / 書込 {} · 出力 {} (推論 {}) · {} calls · USD {:.6} · 未換算 {} calls",
                usage.input,
                usage.cache_read,
                usage.cache_write,
                usage.output,
                usage.reasoning,
                usage.calls,
                usage.cost,
                usage.unpriced
            )
        } else if let Some(cost) = &self.cost {
            format!(
                "合計 USD {:.4} · {} tokens · {} calls · 未換算 {} calls （契約の請求額ではありません）",
                cost.usage.cost,
                tokens(&cost.usage),
                cost.usage.calls,
                cost.usage.unpriced
            )
        } else {
            format!(
                "行 {}/{} · ←/→ 前後  +/− 拡縮  t 今日  L 最新  g 期間  z 選択を拡大  [/] 親/子",
                self.selected + usize::from(!self.rows.is_empty()),
                self.rows.len()
            )
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw(selected),
                Line::styled(counts, Style::default().fg(MUTED)),
                Line::styled(
                    if self.settings.mode != Mode::Timeline { "u プロジェクト別稼働時間 · Alt+←/→ 日を選択 · Ctrl+↑/↓ 時刻移動 · 0 全期間" }
                    else { "c 値  o 行  a 集約  v 範囲内のみ · Alt+1..6 1h/6h/24h/7d/30d/90d · 0 全期間" },
                    Style::default().fg(MUTED),
                ),
            ]),
            area,
        );
    }

    fn draw_input(&self, frame: &mut Frame<'_>, area: Rect) {
        let width = area.width.min(76);
        let height = area.height.min(7);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(vec![
                Line::raw("YYYY-MM-DD または YYYY-MM-DD..YYYY-MM-DD"),
                Line::raw("直近: 1h / 6h / 24h / 7d / 30d / 90d / all"),
                Line::styled(
                    format!("> {}", self.input.as_deref().unwrap_or_default()),
                    Style::default().fg(ACCENT),
                ),
                Line::styled(self.error.clone(), Style::default().fg(Color::Red)),
                Line::styled("Enter 適用 · Esc 取消", Style::default().fg(MUTED)),
            ])
            .block(Block::default().borders(Borders::ALL).title("期間・日付")),
            popup,
        );
    }
}

fn kind_color(kind: Kind) -> Color {
    match kind {
        Kind::User => Color::Yellow,
        Kind::Assistant => Color::Cyan,
        Kind::Thinking => Color::DarkGray,
        Kind::Tool => Color::Green,
        Kind::Agent => Color::Magenta,
    }
}
