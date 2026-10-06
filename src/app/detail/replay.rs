//! イベントタブの再生操作と、記録時刻に到達した行の表示。

use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui::{Context, Div, ElementId, Window, div, prelude::*, px, relative};

use super::{DetailRow, Rireki, RowFilter, Selection, fmt_dur, fmt_n};
use crate::detail::replay::{Playback, SPEEDS};
use crate::icons::Icon;
use crate::model::Event;
use crate::prefs::DetailTab;
use crate::theme::{self, c};

pub(in crate::app) struct State {
    sel: Selection,
    clock: Playback,
    pub(super) following: bool,
    pub(super) check_follow: bool,
    pub(super) scroll_pending: Cell<bool>,
}

impl State {
    pub(super) fn is_finished(&self) -> bool {
        self.clock.is_finished()
    }

    pub(super) fn visible(&self, events: &[Event]) -> usize {
        events.partition_point(|event| event.start <= self.clock.timestamp())
    }
}

pub(super) fn rows(events: &[Event], visible: usize, filter: Option<&RowFilter>, reversed: bool) -> Vec<DetailRow> {
    let mut rows = matching_rows(events, 0, visible, filter);
    if reversed {
        rows.reverse();
    }
    if rows.is_empty() {
        rows.push(DetailRow::Empty);
    }
    rows
}

pub(super) fn matching_rows(events: &[Event], first: usize, last: usize, filter: Option<&RowFilter>) -> Vec<DetailRow> {
    events
        .iter()
        .enumerate()
        .take(last)
        .skip(first)
        .filter_map(|(index, event)| {
            let keep = match filter {
                None => true,
                Some(RowFilter::Name(name)) => event.name.as_ref() == name,
                Some(RowFilter::Failed) => event.status.failed(),
            };
            keep.then_some(DetailRow::Event(index))
        })
        .collect()
}

impl Rireki {
    pub(super) fn reset_replay(&mut self) {
        self.replay_task = None;
        self.replay = None;
        self.replay_autostart = None;
    }

    fn replay_visible(&self) -> usize {
        self.replay.as_ref().map_or(0, |replay| {
            let sel = replay.sel;
            replay.visible(&self.detail_session(sel).lanes[sel.lane].events[sel.first..=sel.last])
        })
    }

    fn tick_replay(&mut self, now: Instant, cx: &mut Context<Self>) {
        if self.replay.as_ref().is_some_and(|replay| replay.check_follow) {
            self.check_replay_follow(cx);
            if let Some(replay) = &mut self.replay {
                replay.check_follow = false;
            }
        }
        let before = self.replay_visible();
        if let Some(replay) = &mut self.replay {
            replay.clock.tick(now);
        }
        let changed = before != self.replay_visible();
        if let Some(replay) = &self.replay {
            if changed && replay.following {
                replay.scroll_pending.set(true);
            }
            self.detail_changed(cx);
        }
    }

    pub(super) fn check_replay_follow(&mut self, cx: &mut Context<Self>) {
        // スクロール処理と描画が終わった位置で、表示順に応じた最新側の端を調べる。
        let following = self.detail_list.at_latest(self.detail_reversed);
        if let Some(replay) = &mut self.replay {
            replay.following = following;
            if following {
                replay.scroll_pending.set(true);
            }
            self.detail_changed(cx);
        }
    }

    pub(super) fn pause_replay(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.tick_replay(now, cx);
        if let Some(replay) = &mut self.replay {
            replay.clock.pause(now);
        }
        self.replay_task = None;
    }

    fn run_replay(&mut self, cx: &mut Context<Self>) {
        if self.replay.as_ref().is_none_or(|replay| !replay.clock.is_playing()) {
            return;
        }
        self.replay_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let Ok(playing) = this.update(cx, |app, cx| {
                    if app.detail_tab != DetailTab::Events || app.statistics_open {
                        let now = Instant::now();
                        app.tick_replay(now, cx);
                        if let Some(replay) = &mut app.replay {
                            replay.clock.pause(now);
                        }
                        return false;
                    }
                    app.tick_replay(Instant::now(), cx);
                    app.replay.as_ref().is_some_and(|replay| replay.clock.is_playing())
                }) else {
                    return;
                };
                if !playing {
                    return;
                }
            }
        }));
    }

    pub(super) fn begin_replay(&mut self, sel: Selection, cx: &mut Context<Self>) {
        let Some(cache) = self.cache(sel) else {
            return;
        };
        let start = self.detail_session(sel).lanes[sel.lane].events[sel.first].start;
        let mut clock = Playback::new(start, cache.end(), self.replay_speed);
        clock.play(Instant::now());
        self.cancel_detail_follow();
        self.replay = Some(State { sel, clock, following: true, check_follow: false, scroll_pending: Cell::new(true) });
        self.detail_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.run_replay(cx);
        self.detail_changed(cx);
    }

    fn toggle_replay(&mut self, sel: Selection, window: &mut Window, cx: &mut Context<Self>) {
        // 編集のないファイルタブがイベント表示へ代替されている場合も再生できる。
        if self.detail_tab != DetailTab::Events {
            self.set_detail_tab(DetailTab::Events, cx);
        }
        if let Some(replay) = &mut self.replay {
            if replay.clock.is_playing() {
                self.pause_replay(cx);
            } else {
                replay.clock.play(Instant::now());
                replay.scroll_pending.set(replay.following);
                self.run_replay(cx);
                self.detail_changed(cx);
            }
        } else if self.cache(sel).is_some() {
            // 単一イベントから始める場合は、同じレーンの残りを再生する。
            let last = self.detail_session(sel).lanes[sel.lane].events.len() - 1;
            if sel.first == sel.last && sel.last < last {
                let range = Selection { last, ..sel };
                self.navigate(range, window, cx);
                self.replay_autostart = Some(range);
            } else {
                self.begin_replay(sel, cx);
            }
        }
    }

    fn set_replay_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.tick_replay(now, cx);
        self.replay_speed = speed;
        if let Some(replay) = &mut self.replay {
            replay.clock.set_speed(speed, now);
        }
        self.detail_changed(cx);
    }

    pub(super) fn replay_controls(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        if self.cache(sel).is_none() {
            return div();
        }
        let playing = self.replay.as_ref().is_some_and(|replay| replay.clock.is_playing());
        let finished = self.replay.as_ref().is_some_and(|replay| replay.clock.is_finished());
        let text = if playing {
            "一時停止"
        } else if finished {
            "もう一度再生"
        } else if self.replay.is_some() {
            "再開"
        } else {
            "リプレイ"
        };
        let mut controls = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .text_xs();
        let mut actions = div().flex().flex_wrap().items_center().gap_1().child(self.icon_button(
            "replay-toggle",
            Some(if playing { Icon::Pause } else { Icon::Play }),
            text,
            playing,
            cx,
            move |app, window, cx| app.toggle_replay(sel, window, cx),
        ));
        if let Some(replay) = &self.replay {
            actions = actions
                .child(self.ghost_button(
                    "replay-rewind",
                    Icon::Refresh,
                    "先頭に戻る",
                    None,
                    true,
                    cx,
                    |app, _, cx| {
                        if let Some(replay) = &mut app.replay {
                            replay.clock.rewind(Instant::now());
                            replay.following = true;
                            replay.check_follow = false;
                            replay.scroll_pending.set(true);
                        }
                        app.detail_changed(cx);
                    },
                ))
                .child(self.icon_button(
                    "replay-follow",
                    Some(Icon::Locate),
                    "追従",
                    replay.following,
                    cx,
                    |app, _, cx| {
                        if let Some(replay) = &mut app.replay {
                            replay.following = !replay.following;
                            replay.check_follow = false;
                            replay.scroll_pending.set(replay.following);
                        }
                        app.detail_changed(cx);
                    },
                ))
                .child(self.ghost_button(
                    "replay-stop",
                    Icon::X,
                    "リプレイを終了して一覧に戻る",
                    None,
                    true,
                    cx,
                    |app, _, cx| {
                        app.reset_replay();
                        app.detail_list.reset();
                        if let Some(sel) = app.selected.filter(|&sel| app.valid(sel))
                            && app.detail_snapshot.as_ref().is_some_and(|(session, _)| {
                                !std::sync::Arc::ptr_eq(session, &app.store.sessions[sel.session])
                            })
                        {
                            app.refresh_detail(sel, cx);
                        }
                        app.detail_changed(cx);
                    },
                ));
            let visible = self.replay_visible();
            let total = sel.last - sel.first + 1;
            let position = replay.clock.position();
            let duration = replay.clock.duration();
            let progress = if duration > 0.0 { position / duration } else { 1.0 };
            controls = controls
                .child(actions)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .text_color(c(theme::MUTED))
                        .child(format!("{} / {}", fmt_dur(position), fmt_dur(duration)))
                        .child(format!(
                            "{} / {} 件{}",
                            fmt_n(visible),
                            fmt_n(total),
                            if finished { " · 再生完了" } else { "" }
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(3.0))
                        .rounded_full()
                        .bg(c(theme::BORDER))
                        .child(div().w(relative(progress as f32)).h_full().rounded_full().bg(c(theme::ACCENT))),
                );
        } else {
            controls = controls.child(actions);
        }
        controls
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(div().text_color(c(theme::MUTED)).child("速度"))
                    .children(SPEEDS.iter().enumerate().map(|(index, &speed)| {
                        self.icon_button(
                            ElementId::NamedInteger("replay-speed".into(), index as u64),
                            None,
                            if speed == 1.0 { "1×（実時間）".to_string() } else { format!("{speed}×") },
                            self.replay_speed == speed,
                            cx,
                            move |app, _, cx| app.set_replay_speed(speed, cx),
                        )
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Status};

    #[test]
    fn replay_reveals_tied_timestamps_in_order_and_can_pass_normal_display_limit() {
        let events: Vec<_> = (0..350)
            .map(|i| Event {
                start: 100.0 + (i / 2) as f64,
                end: 101.0 + (i / 2) as f64,
                kind: Kind::Tool,
                name: if i % 2 == 0 { "Read" } else { "Edit" }.into(),
                summary: Default::default(),
                input: String::new(),
                output: String::new(),
                call_id: None,
                text_ref: Default::default(),
                usage: Default::default(),
                model: None,
                status: if i % 2 == 0 { Status::Ok } else { Status::Error },
            })
            .collect();
        let now = Instant::now();
        let mut state = State {
            sel: Selection { session: 0, lane: 0, first: 0, last: 349 },
            clock: Playback::new(100.0, 275.0, 2.0),
            following: true,
            check_follow: false,
            scroll_pending: Cell::new(false),
        };
        assert_eq!(state.visible(&events), 2);
        state.clock.play(now);
        state.clock.tick(now + Duration::from_millis(500));
        assert_eq!(state.visible(&events), 4);
        assert_eq!(rows(&events, 4, Some(&RowFilter::Failed), false), [DetailRow::Event(1), DetailRow::Event(3)]);
        assert_eq!(rows(&events, 4, Some(&RowFilter::Failed), true), [DetailRow::Event(3), DetailRow::Event(1)]);
        assert_eq!(
            rows(&events, 4, Some(&RowFilter::Name("Read".into())), false),
            [DetailRow::Event(0), DetailRow::Event(2)]
        );
        assert_eq!(rows(&events, 0, None, false), [DetailRow::Empty]);
        assert_eq!(rows(&events, 0, None, true), [DetailRow::Empty]);
        state.clock.tick(now + Duration::from_secs(100));
        assert_eq!(state.visible(&events), 350);
        let shown = rows(&events, state.visible(&events), None, false);
        assert_eq!(shown.len(), 350);
        assert_eq!(shown.last(), Some(&DetailRow::Event(349)));
        assert!(state.clock.is_finished());
    }
}
