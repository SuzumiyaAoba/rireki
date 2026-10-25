//! GUI と TUI が同じ集計値を参照するための指標。数値の表示形式は各画面で決める。

use serde::{Deserialize, Serialize};

use super::{Amount, Totals};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    Tokens,
    Cost,
    Turns,
    Sessions,
    Tools,
    /// 失敗・拒否・中断したツール呼び出し
    Failures,
}

impl Metric {
    pub const ALL: [Self; 6] = [
        Self::Tokens,
        Self::Cost,
        Self::Turns,
        Self::Sessions,
        Self::Tools,
        Self::Failures,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tokens => "トークン",
            Self::Cost => "推定料金",
            Self::Turns => "ターン",
            Self::Sessions => "セッション",
            Self::Tools => "ツール呼び出し",
            Self::Failures => "失敗・拒否・中断",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Self::Tokens => "入力・キャッシュ・出力の合計",
            Self::Cost => "ログの使用量と料金表による推定（USD）。料金不明のモデルは含まない",
            Self::Turns => "ユーザーの発話の数",
            Self::Sessions => {
                "イベントのあるセッションの数。サブエージェントは起動元と合わせて 1 件"
            }
            Self::Tools => "ツール呼び出し（サブエージェントの起動を含む）の数",
            Self::Failures => {
                "失敗（エラーの結果）・拒否・中断したツール呼び出しの数。結果のない呼び出しは含めない"
            }
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Self::Tokens => " トークン",
            Self::Cost => "",
            Self::Turns => " ターン",
            Self::Sessions => " セッション",
            Self::Tools => " 回",
            Self::Failures => " 件",
        }
    }

    pub fn total(self, value: &Totals) -> f64 {
        match self {
            Self::Tokens => value.usage.total() as f64,
            Self::Cost => value.usage.cost,
            Self::Turns => value.turns as f64,
            Self::Sessions => value.sessions as f64,
            Self::Tools => value.tools as f64,
            Self::Failures => value.failures as f64,
        }
    }

    pub fn amount(self, value: &Amount) -> f64 {
        match self {
            Self::Tokens => value.tokens as f64,
            Self::Cost => value.cost,
            Self::Turns => value.turns as f64,
            Self::Sessions => value.sessions as f64,
            Self::Tools => value.tools as f64,
            Self::Failures => value.failures as f64,
        }
    }

    /// 件数を数える指標か（目盛りを整数にする）。
    pub fn count(self) -> bool {
        !matches!(self, Self::Cost)
    }
}
