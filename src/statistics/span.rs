//! 集計対象の日付と日・週・月の列境界。加算方法によらず同じ規則で決める。

use crate::model::Snapshot;
use crate::time::local;
use chrono::{Datelike, Duration, NaiveDate};

/// 集計する期間。全期間以外は今日までの直近の日数
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Period {
    Week,
    Month,
    Quarter,
    All,
}

impl Period {
    /// 期間の日数。全期間は履歴の最初の日から数えるので決まっていない
    pub fn days(self) -> Option<i64> {
        match self {
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::Quarter => Some(90),
            Self::All => None,
        }
    }
}

/// グラフの 1 列の長さ。長い全期間は週・月ごとにまとめる
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Day,
    Week,
    Month,
}

/// 日ごとの列にする全期間の最長の日数
const DAY_UNIT_MAX: i64 = 92;
/// 週ごとの列にする全期間の最長の日数。超えると月ごと
const WEEK_UNIT_MAX: i64 = 371;

/// 期間の日付と列の区切り。集計の方法によらず同じ規則で決める
#[derive(Clone, PartialEq, Debug)]
pub(super) struct Span {
    pub(super) from: NaiveDate,
    pub(super) to: NaiveDate,
    pub(super) unit: Unit,
    /// 列ごとの (最初の日, 最後の日)
    pub(super) buckets: Vec<(NaiveDate, NaiveDate)>,
    /// 直前の同じ長さの期間の最初の日
    pub(super) previous: Option<NaiveDate>,
}

impl Span {
    pub(super) fn from_date(
        store: &Snapshot,
        order: &[usize],
        period: Period,
        today: NaiveDate,
    ) -> NaiveDate {
        match period.days() {
            Some(n) => today - Duration::days(n - 1),
            None => {
                // 最も早い開始時刻だけを日付に変換する。
                let first = order
                    .iter()
                    .map(|&si| store.sessions[si].start)
                    .filter(|t| t.is_finite())
                    .fold(f64::INFINITY, f64::min);
                if first.is_finite() {
                    local(first).date_naive().min(today)
                } else {
                    today
                }
            }
        }
    }

    pub(super) fn new(from: NaiveDate, period: Period, today: NaiveDate) -> Self {
        let days = (today - from).num_days() + 1;
        let unit = match period {
            Period::All if days > WEEK_UNIT_MAX => Unit::Month,
            Period::All if days > DAY_UNIT_MAX => Unit::Week,
            _ => Unit::Day,
        };
        let mut buckets = Vec::new();
        let mut day = from;
        while day <= today {
            // 週は月曜から、月は 1 日から。期間の端の列は途中で切れる。
            let last = match unit {
                Unit::Day => day,
                Unit::Week => {
                    day + Duration::days(6 - i64::from(day.weekday().num_days_from_monday()))
                }
                Unit::Month => month_end(day),
            }
            .min(today);
            buckets.push((day, last));
            day = last + Duration::days(1);
        }
        Self {
            from,
            to: today,
            unit,
            buckets,
            previous: period.days().map(|n| from - Duration::days(n)),
        }
    }

    pub(super) fn days(&self) -> usize {
        ((self.to - self.from).num_days() + 1) as usize
    }
}

fn month_end(d: NaiveDate) -> NaiveDate {
    let (y, m) = if d.month() == 12 {
        (d.year() + 1, 1)
    } else {
        (d.year(), d.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1).map_or(d, |next| next - Duration::days(1))
}
