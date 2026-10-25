//! 統計と料金の背景集計。入力・描画とは独立してキャンセルを確認する。

use chrono::{Duration, NaiveDate};
use rireki::cost_timeline;
use rireki::model::Snapshot;
use rireki::statistics::{self, Period, Summary};
use rireki::time::midnight;

use super::BINS;

#[derive(Clone, Copy)]
pub(in crate::tui) struct Params {
    pub period: Period,
    pub today: NaiveDate,
    pub cost_view: Option<cost_timeline::View>,
    pub in_view_only: bool,
}

pub(in crate::tui) struct Computed {
    pub summary: Summary,
    pub costs: cost_timeline::Summary,
}

#[derive(Default)]
pub(in crate::tui) struct Cache {
    statistics: statistics::Cache,
    costs: cost_timeline::Cache,
}

impl Cache {
    pub(in crate::tui) fn build(
        &mut self,
        snapshot: &Snapshot,
        scope: &[usize],
        params: Params,
        keep_going: impl Fn() -> bool,
    ) -> Option<Computed> {
        let summary = self.statistics.build_while(
            snapshot,
            scope,
            params.period,
            params.today,
            &keep_going,
        )?;
        let view = params.cost_view.or_else(|| {
            cost_timeline::View::new(
                midnight(summary.from),
                midnight(summary.to + Duration::days(1)),
                BINS,
            )
        })?;
        let costs =
            self.costs
                .build_while(snapshot, scope, view, params.in_view_only, keep_going)?;
        Some(Computed { summary, costs })
    }
}
