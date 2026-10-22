//! DB に保存する列挙値の変換。既存の数値と未知値の扱いを維持する。

use crate::model::{Kind, Source, Status};

pub(super) fn kind_to_i(k: Kind) -> i64 {
    match k {
        Kind::User => 0,
        Kind::Assistant => 1,
        Kind::Thinking => 2,
        Kind::Tool => 3,
        Kind::Agent => 4,
    }
}

pub(super) fn kind_of(i: i64) -> Kind {
    match i {
        0 => Kind::User,
        1 => Kind::Assistant,
        2 => Kind::Thinking,
        4 => Kind::Agent,
        _ => Kind::Tool,
    }
}

pub(super) fn status_to_i(s: Status) -> i64 {
    match s {
        Status::Ok => 0,
        Status::Error => 1,
        Status::Rejected => 2,
        Status::Interrupted => 3,
        Status::Pending => 4,
    }
}

pub(super) fn status_of(i: i64) -> Status {
    match i {
        1 => Status::Error,
        2 => Status::Rejected,
        3 => Status::Interrupted,
        4 => Status::Pending,
        _ => Status::Ok,
    }
}

pub(super) fn source_to_i(s: Source) -> i64 {
    match s {
        Source::Claude => 0,
        Source::Codex => 1,
    }
}
