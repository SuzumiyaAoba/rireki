//! 集約結果から表示行を構築する。選択位置と集計の加算順序を保持する。

use super::{
    Day, IDLE, Item, Kind, Mode, Model, Row, Selection, Snapshot, clean, cost_timeline, duration,
    layout, local, midnight, title, tokens, weekday,
};
use chrono::Duration;
use std::collections::HashMap;

impl Model {
    pub(super) fn ensure_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        if !self.dirty {
            return;
        }
        let selected = self.current_item().map(|item| item.selection);
        self.rows.clear();
        self.days.clear();
        self.projects.clear();
        if self.settings.mode != Mode::Timeline {
            self.calendar_rows(snapshot, order);
        } else if self.settings.cost {
            self.cost_rows(snapshot);
        } else if self.settings.directory {
            self.directory_rows(snapshot, order);
        } else {
            self.lane_rows(snapshot, order);
        }
        if let Some(selection) = selected
            && let Some((row, item)) = self.rows.iter().enumerate().find_map(|(row, r)| {
                r.items
                    .iter()
                    .position(|item| item.selection == selection)
                    .map(|item| (row, item))
            })
        {
            self.selected = row;
            self.item = item;
        }
        self.project_selected = self
            .project_selected
            .min(self.projects.len().saturating_sub(1));
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.item = self
            .rows
            .get(self.selected)
            .map_or(0, |row| self.item.min(row.items.len().saturating_sub(1)));
        self.dirty = false;
    }

    fn header_row(&mut self, cwd: &str, label: String) {
        self.rows.push(Row {
            cwd: cwd.into(),
            label,
            header: true,
            lane: None,
            range: self.range,
            items: Vec::new(),
        });
    }

    fn lane_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
        let mut by_cwd = HashMap::new();
        for &si in order {
            let Some(s) = snapshot.sessions.get(si) else {
                continue;
            };
            let i = *by_cwd.entry(s.cwd.as_str()).or_insert_with(|| {
                groups.push((&s.cwd, Vec::new()));
                groups.len() - 1
            });
            groups[i].1.push(si);
        }
        let spp = (self.range.1 - self.range.0) / self.plot_width.max(1) as f64;
        for (cwd, sessions) in groups {
            let header = self.rows.len();
            self.header_row(
                cwd,
                format!("{} ({} セッション)", clean(cwd), sessions.len()),
            );
            if self.settings.collapsed.contains(cwd) {
                continue;
            }
            for si in sessions {
                let session = &snapshot.sessions[si];
                for &(li, depth) in &session.tree {
                    let lane = &session.lanes[li];
                    let blocks = self.cache.aggregate(
                        (si, li),
                        lane,
                        self.range,
                        spp,
                        self.settings.granularity,
                    );
                    if blocks.is_empty() && self.settings.in_view_only {
                        continue;
                    }
                    let label = if li == 0 {
                        format!(
                            "{}{} {}",
                            if self.parents.contains_key(&si) {
                                "↳ "
                            } else {
                                ""
                            },
                            session.source.label(),
                            title(session)
                        )
                    } else {
                        format!("{}↳ {}", " ".repeat(depth.min(8)), clean(&lane.name))
                    };
                    let items = blocks
                        .into_iter()
                        .map(|block| {
                            let (dominant, label) =
                                self.cache.describe((si, li), lane, &block, true);
                            Item {
                                selection: Selection {
                                    session: si,
                                    lane: li,
                                    first: block.first,
                                    last: block.last,
                                },
                                start: block.start,
                                end: block.end,
                                label: clean(&label.unwrap_or_default()),
                                kind: lane.events[dominant].kind,
                                usage: None,
                            }
                        })
                        .collect();
                    self.rows.push(Row {
                        cwd: cwd.into(),
                        label,
                        header: false,
                        lane: Some((si, li)),
                        range: self.range,
                        items,
                    });
                }
            }
            if self.settings.in_view_only && self.rows.len() == header + 1 {
                self.rows.pop();
            }
        }
    }

    fn directory_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        let spp = (self.range.1 - self.range.0) / self.plot_width.max(1) as f64;
        let groups = layout::directory::groups(
            &mut self.cache,
            &snapshot.sessions,
            order,
            &self.parents,
            layout::directory::View {
                range: self.range,
                spp,
                gran: self.settings.granularity,
                min_width: 1.0,
                in_view_only: self.settings.in_view_only,
            },
        );
        for group in groups {
            self.header_row(
                &group.cwd,
                format!(
                    "{} ({} セッション)",
                    clean(&group.cwd),
                    group.sessions.len()
                ),
            );
            if self.settings.collapsed.contains(&group.cwd) {
                continue;
            }
            for (i, blocks) in group.rows.into_iter().enumerate() {
                let items = blocks
                    .into_iter()
                    .map(|b| {
                        let lane = &snapshot.sessions[b.session].lanes[0];
                        let (dominant, label) =
                            self.cache.describe((b.session, 0), lane, &b.block, true);
                        Item {
                            selection: Selection {
                                session: b.session,
                                lane: 0,
                                first: b.block.first,
                                last: b.block.last,
                            },
                            start: b.block.start,
                            end: b.block.end,
                            label: format!(
                                "{} · {}",
                                title(&snapshot.sessions[b.session]),
                                clean(&label.unwrap_or_default())
                            ),
                            kind: lane.events[dominant].kind,
                            usage: None,
                        }
                    })
                    .collect();
                self.rows.push(Row {
                    cwd: group.cwd.clone(),
                    label: format!("並列行 {}", i + 1),
                    header: false,
                    lane: None,
                    range: self.range,
                    items,
                });
            }
        }
    }

    fn cost_rows(&mut self, snapshot: &Snapshot) {
        let Some(cost) = self.cost.clone() else {
            return;
        };
        for directory in &cost.directories {
            self.header_row(
                &directory.cwd,
                format!(
                    "{} · USD {:.4} · {} tokens",
                    clean(&directory.cwd),
                    directory.usage.cost,
                    tokens(&directory.usage)
                ),
            );
            if self.settings.collapsed.contains(&directory.cwd) {
                continue;
            }
            if self.settings.directory {
                self.rows.push(Row {
                    cwd: directory.cwd.clone(),
                    label: "API 換算料金".into(),
                    header: false,
                    lane: None,
                    range: self.range,
                    items: cost_items(&directory.buckets, &cost.edges),
                });
            } else {
                for session in &directory.sessions {
                    let Some(s) = snapshot.sessions.get(session.session) else {
                        continue;
                    };
                    self.rows.push(Row {
                        cwd: directory.cwd.clone(),
                        label: title(s),
                        header: false,
                        lane: Some((session.session, 0)),
                        range: self.range,
                        items: cost_items(&session.buckets, &cost.edges),
                    });
                }
            }
        }
    }

    fn calendar_rows(&mut self, snapshot: &Snapshot, order: &[usize]) {
        let (first, count) = self.calendar_span();
        let mut projects: HashMap<String, f64> = HashMap::new();
        for day in 0..count {
            let date = first + Duration::days(day as i64);
            let range = (midnight(date), midnight(date + Duration::days(1)));
            let mut by_root: HashMap<usize, Vec<Item>> = HashMap::new();
            for &si in order {
                let Some(session) = snapshot.sessions.get(si) else {
                    continue;
                };
                if session.start >= range.1 || session.end < range.0 {
                    continue;
                }
                let mut root = si;
                while let Some(&(parent, _, _)) = self.parents.get(&root) {
                    root = parent;
                }
                for (li, lane) in session.lanes.iter().enumerate() {
                    let hi = lane.events.partition_point(|e| e.start < range.1);
                    let lo = lane.cum_end.partition_point(|&end| end < range.0).min(hi);
                    let mut current: Option<Item> = None;
                    for (ei, event) in lane.events.iter().enumerate().take(hi).skip(lo) {
                        if event.end < range.0 {
                            continue;
                        }
                        let (start, end) = (event.start.max(range.0), event.end.min(range.1));
                        if let Some(item) = &mut current
                            && start <= item.end + IDLE
                        {
                            item.end = item.end.max(end);
                            item.selection.last = ei;
                        } else {
                            if let Some(item) = current.take() {
                                by_root.entry(root).or_default().push(item);
                            }
                            current = Some(Item {
                                selection: Selection {
                                    session: si,
                                    lane: li,
                                    first: ei,
                                    last: ei,
                                },
                                start,
                                end,
                                label: title(&snapshot.sessions[root]),
                                kind: event.kind,
                                usage: None,
                            });
                        }
                    }
                    if let Some(item) = current {
                        by_root.entry(root).or_default().push(item);
                    }
                }
            }
            let mut groups = Vec::new();
            let sessions = by_root.len();
            for (root, mut items) in by_root {
                items.sort_by(|a, b| {
                    a.start
                        .total_cmp(&b.start)
                        .then(a.selection.lane.cmp(&b.selection.lane))
                });
                let mut merged: Option<Item> = None;
                for item in items {
                    if let Some(previous) = &mut merged
                        && item.start <= previous.end + IDLE
                    {
                        previous.end = previous.end.max(item.end);
                        // 詳細は一つのレーン内の選択。重なるメインレーンを代表にする。
                        if item.selection.session == root && item.selection.lane == 0 {
                            if previous.selection.session == root && previous.selection.lane == 0 {
                                previous.selection.first =
                                    previous.selection.first.min(item.selection.first);
                                previous.selection.last =
                                    previous.selection.last.max(item.selection.last);
                            } else {
                                previous.selection = item.selection;
                            }
                        }
                    } else {
                        if let Some(item) = merged.take() {
                            groups.push((root, item));
                        }
                        merged = Some(item);
                    }
                }
                if let Some(item) = merged {
                    groups.push((root, item));
                }
            }
            groups.sort_by(|a, b| a.1.start.total_cmp(&b.1.start).then(a.0.cmp(&b.0)));
            let intervals: Vec<_> = groups
                .iter()
                .map(|(_, item)| (item.start, item.end))
                .collect();
            let active = union_duration(&intervals);
            self.days.push(Day {
                date,
                active,
                sessions,
                intervals,
            });
            if groups.is_empty() {
                continue;
            }
            self.rows.push(Row {
                cwd: String::new(),
                label: format!(
                    "{} ({}) · {} セッション · 稼働 {}",
                    date,
                    weekday(date),
                    sessions,
                    duration(active)
                ),
                header: true,
                lane: None,
                range,
                items: Vec::new(),
            });
            for (root, item) in groups {
                let session = &snapshot.sessions[root];
                *projects.entry(session.cwd.clone()).or_default() += item.end - item.start;
                let label = format!(
                    "{}–{} {} {}",
                    local(item.start).format("%H:%M"),
                    local(item.end).format("%H:%M"),
                    session.source.label(),
                    title(session)
                );
                self.rows.push(Row {
                    cwd: session.cwd.clone(),
                    label,
                    header: false,
                    lane: Some((item.selection.session, item.selection.lane)),
                    range,
                    items: vec![item],
                });
            }
        }
        self.projects = projects.into_iter().collect();
        self.projects
            .sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    }
}

fn cost_items(buckets: &[cost_timeline::Bucket], edges: &[f64]) -> Vec<Item> {
    buckets
        .iter()
        .filter_map(|bucket| {
            Some(Item {
                selection: bucket.sample,
                start: *edges.get(bucket.index)?,
                end: *edges.get(bucket.index + 1)?,
                label: format!(
                    "USD {:.6} · {} tokens · {} calls",
                    bucket.usage.cost,
                    tokens(&bucket.usage),
                    bucket.usage.calls
                ),
                kind: Kind::Assistant,
                usage: Some(bucket.usage),
            })
        })
        .collect()
}
pub(super) fn union_duration(intervals: &[(f64, f64)]) -> f64 {
    let mut current: Option<(f64, f64)> = None;
    let mut total = 0.0;
    for &(start, end) in intervals {
        current = match current {
            Some((a, b)) if start <= b => Some((a, b.max(end))),
            Some((a, b)) => {
                total += (b - a).max(0.0);
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    total + current.map_or(0.0, |(a, b)| (b - a).max(0.0))
}
