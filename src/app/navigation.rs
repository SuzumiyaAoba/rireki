//! 表示モード、期間、読み込み元とプロジェクトの切り替え。

use super::{
    Context, Duration, Local, Mode, NaiveDate, Rireki, SourceFilter, Window, calendar, local,
    midnight, now,
};
use chrono::Datelike;

impl Rireki {
    // ------------------------------------------------------------ 表示の切り替え

    /// 表示を切り替える。タイムラインとカレンダーの間では、見ていた時刻が画面に残るようにする
    pub(super) fn set_mode(&mut self, mode: Mode, window: &Window) {
        self.detail_fullscreen = false;
        self.statistics_open = false;
        let from = self.mode;
        if from == Mode::Timeline && mode != Mode::Timeline {
            let span = self.spp * self.plot_width(window) as f64;
            let tc = (self.t0 + span / 2.0).min(now());
            self.day0 = local(tc).date_naive();
            self.mode = mode;
            self.scroll_calendar_to(tc, 0.5, window);
        } else if from != Mode::Timeline && mode == Mode::Timeline {
            let ph = self.plot_height(window);
            let tc = midnight(self.day0)
                + ((self.cal_scroll + ph / 2.0) / self.px_per_hour) as f64 * 3600.0;
            self.mode = mode;
            let span = self.spp * self.plot_width(window) as f64;
            // 現在時刻より先は 15% までしか見せない
            self.t0 = (tc - span / 2.0).min(now() - span * 0.85);
        }
        self.mode = mode;
        self.clamp_timeline(window);
        if mode != Mode::Timeline {
            self.calendar = mode;
        }
        self.popover = None;
        self.persist();
    }

    /// カレンダーを、時刻 `t` が画面の上から `at`（0〜1）の位置に来るようにスクロールする
    pub(super) fn scroll_calendar_to(&mut self, t: f64, at: f32, window: &Window) {
        let ph = self.plot_height(window);
        let y = ((t - midnight(local(t).date_naive())) / 3600.0) as f32 * self.px_per_hour;
        self.cal_scroll = calendar::clamp_scroll(y - ph * at, self.px_per_hour, ph);
    }

    pub(super) fn set_source(&mut self, s: SourceFilter, cx: &mut Context<Self>) {
        self.source = s;
        self.rebuild_order();
        if !self.search_state.query.is_empty() {
            self.refresh_results(cx);
        }
        self.row_scroll = 0.0;
        self.persist();
        self.sidebar_changed(cx);
    }

    pub(super) fn set_focus(&mut self, f: Option<String>, cx: &mut Context<Self>) {
        self.focus = f;
        self.rebuild_order();
        if !self.search_state.query.is_empty() {
            self.refresh_results(cx);
        }
        self.row_scroll = 0.0;
        self.session_limit = 50;
        self.persist();
        self.sidebar_changed(cx);
    }

    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.persist();
        cx.notify();
    }

    /// タイムラインで表示範囲内のセッションだけを並べるか、範囲外もグレーアウトして並べるかを切り替える
    pub(super) fn toggle_in_view_only(&mut self, cx: &mut Context<Self>) {
        self.in_view_only = !self.in_view_only;
        self.row_scroll = 0.0;
        self.persist();
        cx.notify();
    }

    pub(super) fn toggle_directory_timeline(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.directory_timeline = !self.directory_timeline;
        self.row_scroll = 0.0;
        self.hovered = None;
        if let Some(sel) = self.selected {
            self.scroll_to_selection(sel, window);
        }
        self.persist();
        cx.notify();
    }

    pub(super) fn step(&mut self, dir: f64, window: &Window) {
        if self.statistics_open {
            return;
        }
        match self.mode {
            Mode::Day => self.day0 += Duration::days(dir as i64),
            Mode::Week => self.day0 += Duration::days(7 * dir as i64),
            Mode::Month => {
                let d = self.day0.with_day(1).unwrap_or(self.day0);
                self.day0 = if dir < 0.0 {
                    d - chrono::Months::new(1)
                } else {
                    d + chrono::Months::new(1)
                };
            }
            Mode::Timeline => self.t0 += dir * self.spp * self.plot_width(window) as f64,
        }
        self.clamp_timeline(window);
    }

    pub(super) fn go_today(&mut self, window: &Window) {
        self.day0 = Local::now().date_naive();
        match self.mode {
            Mode::Timeline => {
                let width = self.spp * self.plot_width(window) as f64;
                self.t0 = now() - width * 0.85;
            }
            Mode::Day | Mode::Week | Mode::Month => self.scroll_calendar_to(now(), 0.6, window),
        }
        self.clamp_timeline(window);
    }

    /// 直近の作業が見えるようにする（表示範囲に何もないとき用）
    pub(super) fn go_latest(&mut self, window: &Window) {
        let Some(end) = self
            .order
            .iter()
            .map(|&si| self.store.sessions[si].end)
            .reduce(f64::max)
        else {
            return;
        };
        match self.mode {
            Mode::Timeline => {
                let span = (self.spp * self.plot_width(window) as f64).min(86400.0);
                self.set_range(end - span * 0.85, end + span * 0.15, window);
            }
            Mode::Day | Mode::Week | Mode::Month => {
                self.day0 = local(end).date_naive();
                self.scroll_calendar_to(end, 0.6, window);
            }
        }
    }

    pub(super) fn zoom(&mut self, f: f64, window: &Window) {
        if self.statistics_open || self.mode == Mode::Month {
            return;
        }
        match self.mode {
            Mode::Timeline => {
                let w = self.plot_width(window) as f64;
                let center = self.t0 + w * self.spp / 2.0;
                self.spp = (self.spp * f).clamp(0.005, 86400.0 * 3.0);
                self.t0 = center - w * self.spp / 2.0;
            }
            _ => {
                let h = self.plot_height(window);
                let center = (self.cal_scroll + h / 2.0) / self.px_per_hour;
                self.px_per_hour = (self.px_per_hour / f as f32).clamp(20.0, 20000.0);
                self.cal_scroll = calendar::clamp_scroll(
                    center * self.px_per_hour - h / 2.0,
                    self.px_per_hour,
                    h,
                );
            }
        }
        self.clamp_timeline(window);
    }

    pub(super) fn week_start(&self) -> NaiveDate {
        self.day0 - Duration::days(self.day0.weekday().num_days_from_monday() as i64)
    }
}
