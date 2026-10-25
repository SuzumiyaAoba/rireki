//! 統計画面の数値・期間の表示。指標の値は共通コアから取得する。

use super::{Metric, NaiveDate, UsageSum, fmt_n, fmt_tokens, fmt_usd};

pub(super) trait MetricFormat {
    fn format(self, value: f64) -> String;
    fn headline(self, value: f64) -> String;
    fn tick(self, value: f64) -> String;
}

impl MetricFormat for Metric {
    fn format(self, v: f64) -> String {
        match self {
            Self::Tokens => fmt_tokens(v.round() as u64),
            Self::Cost => fmt_usd(v),
            _ => fmt_n(v.round() as usize),
        }
    }

    /// タイルの大きな数字。料金は 100 ドル以上ならセントを省く
    fn headline(self, v: f64) -> String {
        match self {
            Self::Cost if v >= 100.0 => fmt_usd(v.round()).trim_end_matches(".00").to_string(),
            _ => self.format(v),
        }
    }

    /// 目盛りの短い表示
    fn tick(self, v: f64) -> String {
        match self {
            Self::Cost if v >= 1000.0 => format!("${}", short(v)),
            Self::Cost if v.fract() == 0.0 => format!("${v:.0}"),
            Self::Cost => format!("${v:.2}").trim_end_matches('0').to_string(),
            _ => short(v),
        }
    }
}

/// 前の期間との差（▲ 12% / ▼ 3.4% / ±0%）。増減の良し悪しは決めつけない
pub(super) fn delta(now: f64, before: f64) -> String {
    if before <= 0.0 {
        return if now > 0.0 {
            "新規".into()
        } else {
            "—".into()
        };
    }
    let r = (now - before) / before * 100.0;
    let digits = if r.abs() < 10.0 { 1 } else { 0 };
    match r {
        r if r.abs() < 0.05 => "±0%".into(),
        r if r > 0.0 => format!("▲ {r:.digits$}%"),
        r => format!("▼ {:.digits$}%", -r),
    }
}

/// 割合（12% / 2.4% / <0.1%）
pub(super) fn percent(r: f64) -> String {
    let p = r * 100.0;
    match p {
        p if p <= 0.0 => "0%".into(),
        p if p < 0.1 => "<0.1%".into(),
        p if p < 10.0 => format!("{p:.1}%"),
        p => format!("{p:.0}%"),
    }
}

/// 目盛り用の短い数（950 / 1.2k / 35k / 1.5M / 2B）
pub(super) fn short(v: f64) -> String {
    let (n, unit) = match v.abs() {
        a if a >= 1e9 => (v / 1e9, "B"),
        a if a >= 1e6 => (v / 1e6, "M"),
        a if a >= 1e3 => (v / 1e3, "k"),
        _ => return format!("{}", (v * 100.0).round() / 100.0),
    };
    let s = if n.abs() >= 10.0 || n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        format!("{n:.1}")
    };
    format!("{}{unit}", s.trim_end_matches(".0"))
}

/// 期間の表示（2026/9/6 – 10/5）
pub(super) fn date_range(from: NaiveDate, to: NaiveDate) -> String {
    use chrono::Datelike;
    if from == to {
        to.format("%Y/%-m/%-d").to_string()
    } else if from.year() == to.year() {
        format!("{} – {}", from.format("%Y/%-m/%-d"), to.format("%-m/%-d"))
    } else {
        format!(
            "{} – {}",
            from.format("%Y/%-m/%-d"),
            to.format("%Y/%-m/%-d")
        )
    }
}

/// 料金。料金の分かる呼び出しがなければ「—」
pub(super) fn cost_label(u: &UsageSum) -> String {
    if u.calls > u.unpriced {
        fmt_usd(u.cost)
    } else {
        "—".into()
    }
}
