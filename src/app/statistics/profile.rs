//! 統計画面の期間・絞り込み・連続更新を、隔離した DB とメモリ上の模擬更新で検証する。

use super::*;
use crate::model::Kind;
use crate::time::midnight;

impl Rireki {
    pub(in crate::app) fn statistics_profile_ready(&self) -> bool {
        let key = Some((
            self.data_gen,
            self.statistics_state.period,
            Local::now().date_naive(),
        ));
        self.statistics_open
            && self.statistics_state.summary.is_some()
            && self.statistics_state.key == key
            && self.statistics_state.shown == key
    }

    pub(in crate::app) fn statistics_profile_state(&self) -> serde_json::Value {
        let Some(summary) = &self.statistics_state.summary else {
            return serde_json::Value::Null;
        };
        serde_json::json!({"ready":self.statistics_profile_ready(),
            "period":match self.statistics_state.period { Period::Week=>"week", Period::Month=>"month", Period::Quarter=>"quarter", Period::All=>"all" },
            "metric":format!("{:?}", self.statistics_state.metric), "active_days":summary.active_days,
            "tools_named":summary.tools.len(), "previous_turns":summary.previous.as_ref().map(|p| p.turns),
            "from":summary.from.to_string(), "to":summary.to.to_string(),
            "scoped":self.focus.is_some(), "sessions":summary.total.sessions,
            "turns":summary.total.turns, "tokens":summary.total.usage.total(),
            "projects":summary.projects.len(), "bins":summary.activity.len(),
            "calls":summary.total.usage.calls, "unpriced":summary.total.usage.unpriced,
            "source_sessions":summary.sources.iter().map(|s| s.sessions).collect::<Vec<_>>()})
    }

    pub(in crate::app) fn statistics_profile_step(
        &mut self,
        stage: &str,
        step: usize,
        anchor: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if step == 0 {
            self.close_detail(cx);
            self.statistics_state.period = Period::All;
            self.show_statistics(window, cx);
            self.statistics_state
                .scroll
                .set_offset(point(px(0.0), px(0.0)));
        }
        match stage {
            "statistics_open" => {
                if step.is_multiple_of(20) {
                    self.statistics_state.metric = Metric::ALL[step / 20 % Metric::ALL.len()];
                    self.statistics_state.hover = Some(if step.is_multiple_of(40) {
                        Hover::Bucket(step / 20)
                    } else {
                        Hover::Hour(step * 7 % 168)
                    });
                    self.statistics_state
                        .scroll
                        .set_offset(point(px(0.0), px(-(step as f32) * 5.0)));
                }
            }
            "statistics_period" => {
                if step.is_multiple_of(30) {
                    self.set_statistics_period(
                        [Period::Week, Period::Month, Period::Quarter, Period::All][step / 30],
                        cx,
                    );
                }
                if step % 30 == 29 {
                    assert!(self.statistics_profile_ready());
                    println!(
                        "{}",
                        serde_json::json!({"ui_profile":"statistics_check", "stage":stage,
                        "step":step, "statistics":self.statistics_profile_state()})
                    );
                }
            }
            "statistics_scope" => {
                if step.is_multiple_of(40) {
                    if step == 40 {
                        self.set_focus(Some(self.store.sessions[anchor].cwd.clone()), cx);
                    } else {
                        self.set_focus(None, cx);
                    }
                }
                if step % 40 == 39 {
                    assert!(self.statistics_profile_ready());
                    assert!(
                        self.statistics_state
                            .summary
                            .as_ref()
                            .unwrap()
                            .projects
                            .len()
                            <= if self.focus.is_some() { 1 } else { usize::MAX }
                    );
                    println!(
                        "{}",
                        serde_json::json!({"ui_profile":"statistics_check", "stage":stage,
                        "step":step, "statistics":self.statistics_profile_state()})
                    );
                }
            }
            "statistics_update" if step.is_multiple_of(30) => {
                assert!(self.statistics_profile_ready());
                let before = self.statistics_state.summary.as_ref().unwrap();
                let expected = (before.total.turns + 1, before.total.usage.total() + 444);
                let mut session = (*self.store.sessions[anchor]).clone();
                session.lanes[0].events.push(crate::model::Event {
                    start: midnight(Local::now().date_naive()) + 3600.0 + step as f64,
                    end: midnight(Local::now().date_naive()) + 3600.0 + step as f64,
                    kind: Kind::User,
                    name: "User".into(),
                    summary: "statistics profile".into(),
                    input: String::new(),
                    output: String::new(),
                    call_id: None,
                    text_ref: Default::default(),
                    usage: crate::model::Usage {
                        input: 333,
                        output: 111,
                        ..Default::default()
                    },
                    model: Some("gpt-6-sol".into()),
                    status: Default::default(),
                });
                session.finalize();
                self.store.sessions[anchor] = Arc::new(session);
                self.rebuild_order();
                self.request_statistics(cx);
                assert!(!self.statistics_state.dimmed(self.source, self.focus.as_deref()));
                println!(
                    "{}",
                    serde_json::json!({"ui_profile":"statistics_expected", "stage":stage,
                    "step":step, "turns":expected.0,"tokens":expected.1})
                );
            }
            "statistics_source" => {
                if step.is_multiple_of(40) {
                    self.set_source(
                        [SourceFilter::Claude, SourceFilter::Codex, SourceFilter::All][step / 40],
                        cx,
                    );
                }
                if step % 40 == 39 {
                    assert!(self.statistics_profile_ready());
                    let summary = self.statistics_state.summary.as_ref().unwrap();
                    match self.source {
                        SourceFilter::Claude => assert_eq!(summary.sources[1].sessions, 0),
                        SourceFilter::Codex => assert_eq!(summary.sources[0].sessions, 0),
                        SourceFilter::All => {}
                    }
                    println!(
                        "{}",
                        serde_json::json!({"ui_profile":"statistics_check", "stage":stage,
                        "step":step, "statistics":self.statistics_profile_state()})
                    );
                }
            }
            "statistics_update" if step % 30 == 29 => {
                assert!(self.statistics_profile_ready());
                println!(
                    "{}",
                    serde_json::json!({"ui_profile":"statistics_check", "stage":stage,
                    "step":step, "statistics":self.statistics_profile_state()})
                );
            }
            "statistics_cancel" => {
                // 描画より速い要求の連続でも、最後の期間の結果だけを反映する。
                for i in 0..6 {
                    self.statistics_state.period = if i % 2 == 0 {
                        Period::Week
                    } else {
                        Period::All
                    };
                    self.request_statistics(cx);
                }
            }
            _ => {}
        }
    }
}
