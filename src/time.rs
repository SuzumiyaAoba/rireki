//! ローカル時刻と日付境界の変換。GUI と集計で同じ規則を使う。

use chrono::{DateTime, Local, NaiveDate, TimeZone};

pub fn local(t: f64) -> DateTime<Local> {
    Local
        .timestamp_millis_opt((t * 1000.0) as i64)
        .single()
        .unwrap_or_else(Local::now)
}

pub fn midnight(d: NaiveDate) -> f64 {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .map(|t| t.timestamp() as f64)
        .unwrap_or(0.0)
}
