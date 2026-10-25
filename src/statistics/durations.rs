//! ツール呼び出しの所要時間の分布。セッションごとの区間の件数を足し合わせ、中央値・P95 を近似する。
//!
//! 区間は 1ms 未満を 0 とし、以降は 2 倍ごとの範囲を仮数の上位 3 ビットで 8 つに分ける。
//! 浮動小数点のビットから区間を求めるので対数を計算しない。区間の中央を代表値とし、誤差は ±6.25% 以内。

/// 区間の数。約 24 日を超える所要時間は最後の区間に入れる
pub const BUCKETS: usize = 256;

/// 1 つの詰めた値に収める件数の上限。超えたら同じ区間の値を追加する
const PACKED_COUNT: u32 = (1 << 24) - 1;

/// 所要時間（秒）の区間。負・NaN・1ms 未満は 0
#[inline]
pub(crate) fn bucket(seconds: f64) -> u8 {
    let ms = seconds * 1000.0;
    if ms.is_nan() || ms < 1.0 {
        return 0;
    }
    let bits = ms.to_bits();
    // 1ms 以上なので指数は 0 以上。無限大も最後の区間にまとめる
    let exponent = ((bits >> 52) & 0x7ff) - 1023;
    let fraction = (bits >> 49) & 7;
    (1 + exponent * 8 + fraction).min(BUCKETS as u64 - 1) as u8
}

/// 区間の代表値（秒）。区間の中央。1ms 未満は 0
fn value(bucket: usize) -> f64 {
    if bucket == 0 {
        return 0.0;
    }
    let exponent = (bucket - 1) / 8;
    let fraction = (bucket - 1) % 8;
    (1.0 + (fraction as f64 + 0.5) / 8.0) * 2f64.powi(exponent as i32) / 1000.0
}

/// 区間の件数を、区間（上位 8 ビット）と件数（下位 24 ビット）を詰めた値で追加する
pub(crate) fn pack(out: &mut Vec<u32>, bucket: u8, mut count: u32) {
    let key = u32::from(bucket) << 24;
    while count > PACKED_COUNT {
        out.push(key | PACKED_COUNT);
        count -= PACKED_COUNT;
    }
    if count > 0 {
        out.push(key | count);
    }
}

/// 詰めた値を区間ごとの件数に足し、足した件数を返す
#[inline]
pub(crate) fn unpack_into(counts: &mut [u32; BUCKETS], packed: &[u32]) -> u64 {
    let mut total = 0;
    for &p in packed {
        let n = p & PACKED_COUNT;
        let c = &mut counts[(p >> 24) as usize];
        *c = c.saturating_add(n);
        total += u64::from(n);
    }
    total
}

/// 期間内のツールごとの所要時間。結果のない呼び出しは含めない
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Durations {
    /// 件数のある区間とその件数（区間の順）
    buckets: Box<[(u8, u32)]>,
    count: u64,
    max: f64,
}

impl Durations {
    /// `first` 番目からの区間の件数と最長から作る。件数のある区間だけを保持する
    pub(crate) fn from_counts(counts: &[u32], first: u8, max: f64) -> Self {
        let n = counts.iter().filter(|&&c| c > 0).count();
        let mut buckets = Vec::with_capacity(n);
        buckets.extend(
            counts.iter().enumerate().filter(|(_, c)| **c > 0).map(|(i, &c)| (first + i as u8, c)),
        );
        let count = buckets.iter().map(|&(_, c)| u64::from(c)).sum();
        Self { buckets: buckets.into_boxed_slice(), count, max: if count > 0 { max } else { 0.0 } }
    }

    /// 所要時間（秒）の列から作る。照合や表示の検証に使う
    pub fn from_seconds(seconds: impl IntoIterator<Item = f64>) -> Self {
        let mut counts = [0; BUCKETS];
        let mut max = 0.0_f64;
        for s in seconds {
            counts[usize::from(bucket(s))] += 1;
            max = max.max(s);
        }
        Self::from_counts(&counts, 0, max)
    }

    /// 所要時間を数えた呼び出しの数
    pub fn count(&self) -> u64 {
        self.count
    }

    /// 最長の所要時間（秒）
    pub fn max(&self) -> Option<f64> {
        (self.count > 0).then_some(self.max)
    }

    /// `percent`（0〜100）パーセンタイルの所要時間（秒）。順位の区間の代表値で、最長を超えない
    pub fn percentile(&self, percent: u64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        // 詳細の分析と同じ nearest rank。最後の順位は最長そのものを返す。
        let rank = (self.count * percent.min(100)).div_ceil(100).max(1);
        if rank == self.count {
            return Some(self.max);
        }
        let mut seen = 0;
        for &(b, n) in &self.buckets {
            seen += u64::from(n);
            if seen >= rank {
                return Some(value(usize::from(b)).min(self.max));
            }
        }
        Some(self.max)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_grow_monotonically_and_keep_the_representative_within_the_bucket() {
        let mut last = 0;
        let mut seconds = 0.0005;
        while seconds < 3.0e6 {
            let b = bucket(seconds);
            assert!(b >= last, "{seconds}");
            last = b;
            if b > 0 && usize::from(b) < BUCKETS - 1 {
                let v = value(usize::from(b));
                assert!((v - seconds).abs() <= seconds * 0.0625 + 1e-12, "{seconds} -> {v}");
            }
            seconds *= 1.013;
        }
        for invalid in [f64::NAN, -1.0, 0.0, 0.000_999] {
            assert_eq!(bucket(invalid), 0);
        }
        assert_eq!(bucket(f64::INFINITY), (BUCKETS - 1) as u8);
        assert_eq!(bucket(1.0e9), (BUCKETS - 1) as u8);
    }

    #[test]
    fn percentiles_use_nearest_rank_and_never_exceed_the_longest_call() {
        assert_eq!(Durations::default().percentile(50), None);
        let d = Durations::from_seconds((1..=20).map(f64::from));
        let near = |actual: Option<f64>, expected: f64| {
            let actual = actual.unwrap();
            assert!((actual - expected).abs() <= expected * 0.0625, "{actual} / {expected}");
        };
        // 20 件の nearest rank: 中央値は 10 番目、P95 は 19 番目
        near(d.percentile(50), 10.0);
        near(d.percentile(95), 19.0);
        assert_eq!((d.percentile(100), d.max(), d.count()), (Some(20.0), Some(20.0), 20));
        // 区間の代表値が最長を超える場合は最長にそろえる
        let one = Durations::from_seconds([1.0, 1.0001, 1.0002]);
        assert!(one.percentile(50).unwrap() <= 1.0002);
        let zero = Durations::from_seconds([0.0]);
        assert_eq!((zero.percentile(95), zero.max()), (Some(0.0), Some(0.0)));
    }

    #[test]
    fn packed_session_counts_merge_like_individual_calls() {
        let values = [0.0, 0.0004, 0.002, 0.25, 0.26, 1.5, 1.5, 61.0, 3600.0, 4.0e6];
        let mut counts = [0; BUCKETS];
        for &v in &values {
            counts[usize::from(bucket(v))] += 1;
        }
        let mut packed = Vec::new();
        for (b, &n) in counts.iter().enumerate() {
            pack(&mut packed, b as u8, n);
        }
        let mut merged = [0; BUCKETS];
        assert_eq!(unpack_into(&mut merged, &packed), values.len() as u64);
        assert_eq!(Durations::from_counts(&merged, 0, 4.0e6), Durations::from_seconds(values));
        // 範囲の外の区間は数えない（1ms 未満の 2 件を除いた分布）
        assert_eq!(Durations::from_counts(&merged[1..], 1, 4.0e6), Durations::from_seconds(values[2..].iter().copied()));
        // 1 つの値の件数の上限を超えたら、同じ区間の値を追加する
        let mut full = Vec::new();
        pack(&mut full, 7, PACKED_COUNT + 2);
        assert_eq!(full, [(7 << 24) | PACKED_COUNT, (7 << 24) | 2]);
        let mut big = [0; BUCKETS];
        assert_eq!(unpack_into(&mut big, &full), u64::from(PACKED_COUNT) + 2);
        assert_eq!(big[7], PACKED_COUNT + 2);
    }
}
