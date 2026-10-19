//! 画面で共有する時刻、件数、パスの表示。

use crate::index;
use crate::time::local;
use chrono::{Datelike, Duration, Local};

pub(super) fn now() -> f64 {
    Local::now().timestamp_millis() as f64 / 1000.0
}

pub(super) fn fmt_dur(s: f64) -> String {
    let s = s.max(0.0);
    if s < 1.0 {
        format!("{:.0}ms", s * 1000.0)
    } else if s < 60.0 {
        format!("{s:.1}s")
    } else if s < 3600.0 {
        format!("{}m{:02}s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!(
            "{}h{:02}m",
            (s / 3600.0) as u64,
            ((s % 3600.0) / 60.0) as u64
        )
    }
}

/// 一覧に添える短い時刻（「3分前」「14:05」「昨日」「10/02」など）
pub(super) fn rel_time(t: f64) -> String {
    let d = now() - t;
    if d < 60.0 {
        return "たった今".into();
    }
    if d < 3600.0 {
        return format!("{}分前", (d / 60.0) as u64);
    }
    let lt = local(t);
    let today = Local::now().date_naive();
    if lt.date_naive() == today {
        lt.format("%H:%M").to_string()
    } else if lt.date_naive() == today - Duration::days(1) {
        "昨日".into()
    } else if lt.year() == today.year() {
        lt.format("%-m/%-d").to_string()
    } else {
        lt.format("%Y/%-m/%-d").to_string()
    }
}

/// 検索結果などに添える時刻（今日は時刻だけ、それ以前は日付と時刻）
pub(super) fn when(t: f64) -> String {
    let lt = local(t);
    let today = Local::now().date_naive();
    if lt.date_naive() == today {
        lt.format("%H:%M").to_string()
    } else if lt.date_naive() == today - Duration::days(1) {
        format!("昨日 {}", lt.format("%H:%M"))
    } else if lt.year() == today.year() {
        lt.format("%-m/%-d %H:%M").to_string()
    } else {
        lt.format("%Y/%-m/%-d").to_string()
    }
}

/// 3 桁ごとに区切った数
pub(super) fn fmt_n(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// ホームディレクトリを `~` にしたパス
pub(super) fn tilde(p: &str) -> String {
    match dirs::home_dir()
        .and_then(|h| rireki::paths::strip_directory_prefix(p, h.to_str()?).map(str::to_string))
    {
        Some(rest) => format!("~{rest}"),
        _ => p.to_string(),
    }
}

/// パスの末尾のディレクトリ名
pub(super) fn base_name(p: &str) -> &str {
    rireki::paths::basename(p)
}

pub(super) fn is_live(end: f64) -> bool {
    now() - end < index::LIVE_SECS
}
