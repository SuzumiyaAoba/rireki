//! 背景計算の種類と世代。MCP 制御とスナップショット交換は計算のキャンセル対象に含めない。

use std::ops::{Index, IndexMut};

/// 宣言順は背景計算と結果配送の優先順でもある。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Job {
    Inspect,
    Analysis,
    Metrics,
    Cost,
}

#[derive(Default)]
pub struct Generations<T> {
    inspect: T,
    analysis: T,
    metrics: T,
    cost: T,
}

impl<T> Index<Job> for Generations<T> {
    type Output = T;

    fn index(&self, job: Job) -> &T {
        match job {
            Job::Inspect => &self.inspect,
            Job::Analysis => &self.analysis,
            Job::Metrics => &self.metrics,
            Job::Cost => &self.cost,
        }
    }
}

impl<T> IndexMut<Job> for Generations<T> {
    fn index_mut(&mut self, job: Job) -> &mut T {
        match job {
            Job::Inspect => &mut self.inspect,
            Job::Analysis => &mut self.analysis,
            Job::Metrics => &mut self.metrics,
            Job::Cost => &mut self.cost,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RequestKey {
    Job(Job),
    Mcp,
    Snapshot,
}

impl RequestKey {
    pub(super) fn is_control(self) -> bool {
        !matches!(self, Self::Job(_))
    }

    pub(super) fn result(self) -> Option<UpdateKey> {
        match self {
            Self::Job(job) => Some(UpdateKey::Job(job)),
            Self::Mcp => Some(UpdateKey::Mcp),
            Self::Snapshot => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum UpdateKey {
    Job(Job),
    Mcp,
    Error,
}
