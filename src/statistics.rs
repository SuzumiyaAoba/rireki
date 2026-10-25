//! 期間・読み込み元・モデル・プロジェクト・セッション・ツール・時間帯ごとの統計。GPUI に依存しない。

mod cache;
mod durations;
mod metric;
#[cfg(test)]
mod reference;
mod span;

pub use cache::Cache;
pub use durations::Durations;
pub use metric::Metric;
use span::Span;
pub use span::{Period, Unit};

use crate::model::{Source, UsageSum};
use chrono::NaiveDate;

/// 時間帯の表のマスの数（曜日 × 時）
pub const HOURS: usize = 7 * 24;

/// 読み込み元の並び（Claude, Codex）での位置
#[inline]
pub fn source_index(s: Source) -> usize {
    usize::from(s == Source::Codex)
}

#[derive(Clone, Default, Debug)]
pub struct Totals {
    /// イベントのあるセッションの数。複数のレーンは 1 件として数える
    pub sessions: usize,
    pub turns: usize,
    /// ツール呼び出し（サブエージェントの起動を含む）
    pub tools: usize,
    /// 失敗・拒否・中断
    pub failures: usize,
    pub usage: UsageSum,
}

impl Totals {
    pub fn merge(&mut self, other: &Self) {
        self.sessions += other.sessions;
        self.turns += other.turns;
        self.tools += other.tools;
        self.failures += other.failures;
        self.usage.merge(&other.usage);
    }
}

/// グラフの列や時間帯のマスに積む量
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Amount {
    pub tokens: u64,
    /// 推定料金（USD）。料金の分からないモデルの分は含まない
    pub cost: f64,
    pub turns: u64,
    pub tools: u64,
    /// 失敗・拒否・中断
    pub failures: u64,
    /// その区間にイベントのあるセッションの数
    pub sessions: u64,
}

impl Amount {
    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.tokens += other.tokens;
        self.cost += other.cost;
        self.turns += other.turns;
        self.tools += other.tools;
        self.failures += other.failures;
        self.sessions += other.sessions;
    }
}

/// グラフの 1 列。from〜to の両日を含む
#[derive(Clone, Debug)]
pub struct Bucket {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// 読み込み元ごと（Claude, Codex）
    pub sources: [Amount; 2],
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub struct Calls {
    pub calls: usize,
    /// 失敗・拒否・中断
    pub failures: usize,
    /// failures のうち、ユーザーが拒否・中断した呼び出し
    pub rejected: usize,
    pub interrupted: usize,
    /// 結果のない呼び出し。所要時間と失敗率の分母には含めない
    pub pending: usize,
}

impl Calls {
    #[inline]
    pub fn merge(&mut self, other: &Self) {
        self.calls += other.calls;
        self.failures += other.failures;
        self.rejected += other.rejected;
        self.interrupted += other.interrupted;
        self.pending += other.pending;
    }

    /// 失敗（エラーの結果）。拒否・中断を除く
    pub fn errors(&self) -> usize {
        self.failures.saturating_sub(self.rejected + self.interrupted)
    }
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub name: String,
    /// サブエージェントの起動
    pub agent: bool,
    /// 読み込み元ごと（Claude, Codex）
    pub sources: [Calls; 2],
    /// 結果のある呼び出しの所要時間
    pub durations: Durations,
}

impl Tool {
    /// 読み込み元を合わせた件数
    pub fn total(&self) -> Calls {
        let mut calls = self.sources[0];
        calls.merge(&self.sources[1]);
        calls
    }
}

/// 期間内にイベントのあるセッションの集計
#[derive(Clone, Debug)]
pub struct SessionRow {
    /// Snapshot のセッション番号
    pub index: usize,
    /// セッションの開始時刻。順位の同値や新しい順の並べ替えに使う
    pub start: f64,
    pub total: Totals,
}

pub struct Summary {
    pub period: Period,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub unit: Unit,
    pub total: Totals,
    /// 直前の同じ長さの期間。全期間にはない
    pub previous: Option<Totals>,
    /// 読み込み元ごと（Claude, Codex）
    pub sources: [Totals; 2],
    /// モデルごとの使用量（読み込み元ごと）。トークンの多い順
    pub models: Vec<(String, [UsageSum; 2])>,
    /// 作業ディレクトリごとの集計（読み込み元ごと）。トークンの多い順
    pub projects: Vec<(String, [Totals; 2])>,
    /// 呼び出しの多い順
    pub tools: Vec<Tool>,
    /// 期間内にイベントのあるセッション。集計に渡した表示順のまま
    pub sessions: Vec<SessionRow>,
    pub activity: Vec<Bucket>,
    /// 曜日（月曜 = 0）× 時（0〜23）の `HOURS` マス。期間内の日をローカル時間で重ねる
    pub hours: Vec<Amount>,
    /// 曜日ごと・時ごとの合計。セッションは重ねずに数える
    pub by_weekday: [Amount; 7],
    pub by_hour: [Amount; 24],
    /// イベントのある日の数
    pub active_days: usize,
    /// 使用量の記録があるセッションの数
    pub recorded_sessions: usize,
}

impl Summary {
    /// 期間の日数（今日を含む）
    pub fn days(&self) -> i64 {
        (self.to - self.from).num_days() + 1
    }

    fn empty(span: &Span, period: Period) -> Self {
        Self {
            period,
            from: span.from,
            to: span.to,
            unit: span.unit,
            total: Totals::default(),
            previous: span.previous.map(|_| Totals::default()),
            sources: Default::default(),
            models: Vec::new(),
            projects: Vec::new(),
            tools: Vec::new(),
            sessions: Vec::new(),
            activity: span
                .buckets
                .iter()
                .map(|&(from, to)| Bucket {
                    from,
                    to,
                    sources: Default::default(),
                })
                .collect(),
            hours: vec![Amount::default(); HOURS],
            by_weekday: Default::default(),
            by_hour: Default::default(),
            active_days: 0,
            recorded_sessions: 0,
        }
    }

    /// セッションが触れた時間帯のマスを、曜日ごと・時ごとのセッション数に数える
    fn count_cells(&mut self, cells: impl IntoIterator<Item = usize>) {
        let (mut weekdays, mut hours) = (0u8, 0u32);
        for cell in cells {
            weekdays |= 1 << (cell / 24);
            hours |= 1 << (cell % 24);
        }
        while weekdays != 0 {
            self.by_weekday[weekdays.trailing_zeros() as usize].sessions += 1;
            weekdays &= weekdays - 1;
        }
        while hours != 0 {
            self.by_hour[hours.trailing_zeros() as usize].sessions += 1;
            hours &= hours - 1;
        }
    }

    /// マスの量を曜日ごと・時ごとに足す（セッション数は `count_cells` で数える）
    fn sum_cells(&mut self) {
        for (i, cell) in self.hours.iter().enumerate() {
            for a in [&mut self.by_weekday[i / 24], &mut self.by_hour[i % 24]] {
                a.tokens += cell.tokens;
                a.cost += cell.cost;
                a.turns += cell.turns;
                a.tools += cell.tools;
                a.failures += cell.failures;
            }
        }
    }

    /// 集計した表を、件数の多い順・名前順の決まった並びにする
    fn finish<'a>(
        &mut self,
        models: impl IntoIterator<Item = (&'a str, [UsageSum; 2])>,
        projects: impl IntoIterator<Item = (&'a str, [Totals; 2])>,
        tools: impl IntoIterator<Item = ((&'a str, bool), ToolSum)>,
    ) {
        let tokens = |u: &[UsageSum; 2]| u[0].total() + u[1].total();
        self.models = models.into_iter().map(|(m, u)| (m.to_owned(), u)).collect();
        self.models
            .sort_by(|a, b| tokens(&b.1).cmp(&tokens(&a.1)).then_with(|| a.0.cmp(&b.0)));
        let key = |t: &[Totals; 2]| {
            (
                t[0].usage.total() + t[1].usage.total(),
                t[0].turns + t[1].turns,
            )
        };
        self.projects = projects
            .into_iter()
            .map(|(p, t)| (p.to_owned(), t))
            .collect();
        self.projects
            .sort_by(|a, b| key(&b.1).cmp(&key(&a.1)).then_with(|| a.0.cmp(&b.0)));
        self.tools = tools
            .into_iter()
            .map(|((name, agent), sum)| Tool {
                name: name.to_owned(),
                agent,
                sources: sum.sources,
                durations: sum.durations,
            })
            .collect();
        self.tools.sort_by(|a, b| {
            let calls = |t: &Tool| t.sources[0].calls + t.sources[1].calls;
            calls(b)
                .cmp(&calls(a))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.agent.cmp(&b.agent))
        });
    }
}

/// ツールごとの集計の途中経過
#[derive(Default)]
struct ToolSum {
    sources: [Calls; 2],
    durations: Durations,
}

#[cfg(test)]
mod tests;
