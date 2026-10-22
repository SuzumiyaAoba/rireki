//! トークン使用量と料金の集計。

use super::Event;

/// トークン使用量。input はキャッシュを除いた入力、reasoning は output のうち推論の分
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Usage {
    pub input: u32,
    pub cache_read: u32,
    pub cache_write: u32,
    /// cache_write のうち 1 時間キャッシュの分
    pub cache_write_1h: u32,
    pub output: u32,
    pub reasoning: u32,
}

impl Usage {
    #[inline]
    pub fn is_empty(&self) -> bool {
        *self == Usage::default()
    }

    /// キャッシュを含む入力全体。推論トークンは出力に含まれる。
    #[inline]
    pub fn input_total(&self) -> u64 {
        self.input as u64 + self.cache_read as u64 + self.cache_write as u64
    }

    #[inline]
    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
            cache_write_1h: self.cache_write_1h.saturating_add(other.cache_write_1h),
            output: self.output.saturating_add(other.output),
            reasoning: self.reasoning.saturating_add(other.reasoning),
        }
    }

    #[inline]
    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_sub(other.input),
            cache_read: self.cache_read.saturating_sub(other.cache_read),
            cache_write: self.cache_write.saturating_sub(other.cache_write),
            cache_write_1h: self.cache_write_1h.saturating_sub(other.cache_write_1h),
            output: self.output.saturating_sub(other.output),
            reasoning: self.reasoning.saturating_sub(other.reasoning),
        }
    }

    /// 入力（キャッシュ含む）と出力の合計
    #[inline]
    pub fn total(&self) -> u64 {
        self.input_total() + self.output as u64
    }

    /// 入力全体のうちキャッシュから読んだ割合。入力がなければ算出できない。
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let input = self.input_total();
        (input > 0).then(|| self.cache_read as f64 / input as f64)
    }
}

/// 集計用。u32 の和はあふれうるので u64 で持つ
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct UsageSum {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
    /// 使用量の付いたイベント（API 呼び出し）の数
    pub calls: u64,
    /// 料金（USD）。料金の分からないモデルの分は含まない
    pub cost: f64,
    /// 料金の分からないモデルの呼び出し数
    pub unpriced: u64,
}

impl UsageSum {
    // ライブラリの外で走査する場合も、イベントごとの集計をインライン化できるようにする。
    #[inline]
    pub fn add(&mut self, u: &Usage, model: Option<&str>) {
        if u.is_empty() {
            return;
        }
        self.add_priced(u, model.and_then(crate::pricing::price));
    }

    /// 同じモデルの料金を使い回す集計用。空の使用量は呼び出し数に含めない。
    #[inline]
    pub fn add_priced(&mut self, u: &Usage, price: Option<crate::pricing::Price>) {
        if u.is_empty() {
            return;
        }
        match price {
            Some(p) => self.cost += p.cost(u),
            None => self.unpriced += 1,
        }
        self.input += u.input as u64;
        self.cache_read += u.cache_read as u64;
        self.cache_write += u.cache_write as u64;
        self.output += u.output as u64;
        self.reasoning += u.reasoning as u64;
        self.calls += 1;
    }

    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.input += other.input;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.calls += other.calls;
        self.cost += other.cost;
        self.unpriced += other.unpriced;
    }

    pub fn of<'a>(events: impl IntoIterator<Item = &'a Event>) -> Self {
        let mut s = Self::default();
        for e in events {
            s.add(&e.usage, e.model.as_deref());
        }
        s
    }

    /// モデルごとの集計（合計の多い順）
    pub fn by_model<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<(String, Self)> {
        let mut out: Vec<(String, Self)> = Vec::new();
        for e in events {
            if e.usage.is_empty() {
                continue;
            }
            let m = e.model.as_deref().unwrap_or("");
            let i = match out.iter().position(|(k, _)| k == m) {
                Some(i) => i,
                None => {
                    out.push((m.to_string(), Self::default()));
                    out.len() - 1
                }
            };
            out[i].1.add(&e.usage, e.model.as_deref());
        }
        out.sort_by_key(|(_, u)| std::cmp::Reverse(u.total()));
        out
    }

    /// キャッシュを含む入力全体。
    #[inline]
    pub fn input_total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    #[inline]
    pub fn total(&self) -> u64 {
        self.input_total() + self.output
    }

    /// 入力トークン数で重み付けしたキャッシュヒット率。
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let input = self.input_total();
        (input > 0).then(|| self.cache_read as f64 / input as f64)
    }
}
