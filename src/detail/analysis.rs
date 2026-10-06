//! 選択範囲全体のイベント分析。本文の表示上限に依存せず、メタデータだけを使う。

use std::collections::HashMap;

use crate::model::{Event, Kind, Status, UsageSum};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KindFilter {
    #[default]
    All,
    Tools,
    Messages,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    Failed,
    Pending,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Oldest,
    Newest,
    Duration,
    Tokens,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub query: String,
    pub kind: KindFilter,
    pub status: StatusFilter,
    pub name: Option<String>,
}

pub fn is_tool(e: &Event) -> bool {
    matches!(e.kind, Kind::Tool | Kind::Agent)
}

/// 記録された経過時間。欠損や不正な値は集計に混ぜない。
pub fn duration(e: &Event) -> f64 {
    let d = e.end - e.start;
    if d.is_finite() && d > 0.0 { d } else { 0.0 }
}

pub fn status_name(status: Status) -> &'static str {
    match status {
        Status::Ok => "成功",
        Status::Error => "失敗",
        Status::Rejected => "拒否",
        Status::Interrupted => "中断",
        Status::Pending => "結果なし",
    }
}

pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::User => "ユーザー",
        Kind::Assistant => "アシスタント",
        Kind::Thinking => "思考",
        Kind::Tool => "ツール",
        Kind::Agent => "エージェント",
    }
}

#[derive(Debug, Default)]
pub struct Tool {
    pub name: String,
    pub calls: usize,
    pub completed: usize,
    pub failures: usize,
    pub pending: usize,
    pub seconds: f64,
    pub max_seconds: f64,
}

impl Tool {
    pub fn average(&self) -> f64 {
        if self.completed == 0 {
            0.0
        } else {
            self.seconds / self.completed as f64
        }
    }
}

#[derive(Debug, Default)]
pub struct Analysis {
    /// 選択範囲の先頭からの位置。絞り込みと並べ替えを反映し、全件を保持する。
    pub indices: Vec<usize>,
    pub tools: Vec<Tool>,
    pub usage: UsageSum,
    /// 絞り込んだ範囲のモデル別の使用量。本文の読み込みは不要。
    pub models: Vec<(String, UsageSum)>,
    pub peak_context: u64,
    pub failures: usize,
    pub pending: usize,
    pub completed_tools: usize,
    pub failed_tools: usize,
    /// 完了したツールの時間の和。同時実行の重なりも含む。
    pub tool_seconds: f64,
    pub p95_seconds: f64,
    pub max_seconds: f64,
    pub span: f64,
}

impl Analysis {
    pub fn average(&self) -> f64 {
        if self.completed_tools == 0 {
            0.0
        } else {
            self.tool_seconds / self.completed_tools as f64
        }
    }

    /// 結果未記録のツールは分母に含めない。
    pub fn failure_rate(&self) -> Option<f64> {
        (self.completed_tools > 0).then(|| self.failed_tools as f64 / self.completed_tools as f64)
    }

    pub fn build(
        events: &[Event],
        filter: &Filter,
        order: Order,
        is_current: impl Fn() -> bool,
    ) -> Option<Self> {
        let mut out = Self::default();
        let query = filter.query.to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        let mut names = HashMap::<&str, usize>::new();
        let mut models = HashMap::<&str, usize>::new();
        let mut last_price = None;
        let mut durations = Vec::new();
        let mut start = f64::INFINITY;
        let mut end = f64::NEG_INFINITY;
        for (i, e) in events.iter().enumerate() {
            if i.is_multiple_of(256) && !is_current() {
                return None;
            }
            if !match filter.kind {
                KindFilter::All => true,
                KindFilter::Tools => is_tool(e),
                KindFilter::Messages => !is_tool(e),
            } {
                continue;
            }
            if !match filter.status {
                StatusFilter::All => true,
                StatusFilter::Failed => e.status.failed(),
                StatusFilter::Pending => e.status == Status::Pending,
            } {
                continue;
            }
            if filter
                .name
                .as_deref()
                .is_some_and(|name| name != e.name.as_ref())
            {
                continue;
            }
            if !words.is_empty() {
                let name = e.name.to_lowercase();
                let mut summary = None;
                let mut model = None;
                if !words.iter().all(|word| name.contains(word)
                    || summary.get_or_insert_with(|| e.summary.to_lowercase()).contains(word)
                    || model.get_or_insert_with(|| e.model.as_deref().unwrap_or("").to_lowercase()).contains(word)) {
                    continue;
                }
            }
            out.indices.push(i);
            if !e.usage.is_empty() {
                let model = e.model.as_deref();
                let price = match last_price {
                    Some((last, price)) if last == model => price,
                    _ => {
                        let price = model.and_then(crate::pricing::price);
                        last_price = Some((model, price));
                        price
                    }
                };
                out.usage.add_priced(&e.usage, price);
                out.peak_context = out.peak_context.max(e.usage.input as u64
                    + e.usage.cache_read as u64 + e.usage.cache_write as u64);
                let name = model.unwrap_or("");
                let index = *models.entry(name).or_insert_with(|| {
                    out.models.push((name.to_owned(), UsageSum::default()));
                    out.models.len() - 1
                });
                out.models[index].1.add_priced(&e.usage, price);
            }
            out.failures += usize::from(e.status.failed());
            out.pending += usize::from(e.status == Status::Pending);
            if e.start.is_finite() {
                start = start.min(e.start);
            }
            if e.end.is_finite() {
                end = end.max(e.end);
            }
            if !is_tool(e) {
                continue;
            }
            let at = *names.entry(e.name.as_ref()).or_insert_with(|| {
                out.tools.push(Tool {
                    name: e.name.to_string(),
                    ..Tool::default()
                });
                out.tools.len() - 1
            });
            let tool = &mut out.tools[at];
            tool.calls += 1;
            if e.status == Status::Pending {
                tool.pending += 1;
                continue;
            }
            let seconds = duration(e);
            tool.completed += 1;
            tool.failures += usize::from(e.status.failed());
            tool.seconds += seconds;
            tool.max_seconds = tool.max_seconds.max(seconds);
            out.completed_tools += 1;
            out.failed_tools += usize::from(e.status.failed());
            out.tool_seconds += seconds;
            out.max_seconds = out.max_seconds.max(seconds);
            durations.push(seconds);
        }
        if !is_current() {
            return None;
        }
        out.models.sort_by_key(|(_, usage)| std::cmp::Reverse(usage.total()));
        if start.is_finite() && end.is_finite() {
            out.span = (end - start).max(0.0);
        }
        if !durations.is_empty() {
            // nearest rank。小さい標本でも最大値を越えない。
            let rank = (durations.len() * 95).div_ceil(100) - 1;
            out.p95_seconds = *durations.select_nth_unstable_by(rank, f64::total_cmp).1;
        }
        out.tools.sort_by(|a, b| {
            b.seconds
                .total_cmp(&a.seconds)
                .then(b.calls.cmp(&a.calls))
                .then(a.name.cmp(&b.name))
        });
        out.indices.sort_unstable_by(|&a, &b| {
            let a_event = &events[a];
            let b_event = &events[b];
            let ordering = match order {
                Order::Oldest => a_event.start.total_cmp(&b_event.start),
                Order::Newest => b_event.start.total_cmp(&a_event.start),
                Order::Duration => duration(b_event).total_cmp(&duration(a_event)),
                Order::Tokens => b_event.usage.total().cmp(&a_event.usage.total()),
            };
            ordering.then(a.cmp(&b))
        });
        is_current().then_some(out)
    }

    /// 一覧と同じ絞り込み・順序のメタデータを出力する。本文を再読み込みしない。
    pub fn export_json(
        &self,
        events: &[Event],
        offset: usize,
        session_id: &str,
        lane: &str,
    ) -> String {
        let rows: Vec<_> = self.indices.iter().filter_map(|&i| events.get(i).map(|e| {
            serde_json::json!({
                "event": offset + i + 1, "name":e.name.as_ref(), "kind":kind_name(e.kind),
                "summary":e.summary.as_ref(), "status":status_name(e.status),
                "start":e.start, "end":e.end, "duration_seconds":duration(e),
                "model":e.model.as_deref(), "tokens":e.usage.total(), "usage":{
                    "input":e.usage.input, "cache_read":e.usage.cache_read, "cache_write":e.usage.cache_write,
                    "output":e.usage.output, "reasoning":e.usage.reasoning,
                },
            })
        })).collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "session_id":session_id, "lane":lane, "event_count":rows.len(), "events":rows,
        }))
        .expect("イベントのメタデータは JSON に変換できる")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TextRef, Usage};

    fn event(name: &str, start: f64, end: f64, status: Status) -> Event {
        Event {
            name: name.into(),
            summary: "日本語の解析".into(),
            start,
            end,
            status,
            kind: Kind::Tool,
            input: String::new(),
            output: String::new(),
            call_id: None,
            text_ref: TextRef::default(),
            usage: Usage::default(),
            model: None,
        }
    }

    #[test]
    fn analyzes_beyond_body_limit_and_excludes_pending_from_duration_and_failure_rate() {
        let mut events: Vec<_> = (0..400)
            .map(|i| event("Read", i as f64, i as f64 + 2.0, Status::Ok))
            .collect();
        events[350].status = Status::Error;
        events[390].status = Status::Pending;
        events[390].end = 10000.0;
        events[399].usage.output = 42;
        let a = Analysis::build(&events, &Filter::default(), Order::Oldest, || true).unwrap();
        assert_eq!(a.indices.len(), 400);
        assert_eq!((a.failures, a.pending, a.completed_tools), (1, 1, 399));
        assert_eq!(
            (a.tool_seconds, a.average(), a.p95_seconds, a.max_seconds),
            (798.0, 2.0, 2.0, 2.0)
        );
        assert_eq!(a.failure_rate(), Some(1.0 / 399.0));
        assert_eq!(a.usage.total(), 42);
        let failed = Filter {
            status: StatusFilter::Failed,
            ..Filter::default()
        };
        assert_eq!(
            Analysis::build(&events, &failed, Order::Oldest, || true)
                .unwrap()
                .indices,
            [350]
        );
    }

    #[test]
    fn filters_combine_unicode_search_name_kind_and_status() {
        let mut events = vec![
            event("Exec", 0.0, 2.0, Status::Error),
            event("Read", 1.0, 3.0, Status::Error),
        ];
        events[0].model = Some("MODEL-A".into());
        let mut f = Filter {
            query: "exec  日本語 model-a".into(),
            kind: KindFilter::Tools,
            status: StatusFilter::Failed,
            name: Some("Exec".into()),
        };
        let a = Analysis::build(&events, &f, Order::Oldest, || true).unwrap();
        assert_eq!(a.indices, [0]);
        assert_eq!(a.tools[0].name, "Exec");
        f.kind = KindFilter::Messages;
        assert!(
            Analysis::build(&events, &f, Order::Oldest, || true)
                .unwrap()
                .indices
                .is_empty()
        );
        f.kind = KindFilter::Tools;
        f.name = Some("Read".into());
        assert!(
            Analysis::build(&events, &f, Order::Oldest, || true)
                .unwrap()
                .indices
                .is_empty()
        );
    }

    #[test]
    fn time_sum_does_not_confuse_overlapping_calls_with_elapsed_span() {
        let events = [
            event("A", 0.0, 10.0, Status::Ok),
            event("B", 5.0, 15.0, Status::Rejected),
        ];
        let a = Analysis::build(&events, &Filter::default(), Order::Duration, || true).unwrap();
        assert_eq!((a.span, a.tool_seconds, a.average()), (15.0, 20.0, 10.0));
        assert_eq!(a.failure_rate(), Some(0.5));
    }

    #[test]
    fn orders_use_duration_tokens_and_time_with_stable_ties() {
        let mut events = vec![
            event("A", 5.0, 6.0, Status::Ok),
            event("B", 0.0, 8.0, Status::Ok),
            event("C", 3.0, 11.0, Status::Ok),
        ];
        events[0].usage.output = 100;
        events[1].usage.output = 50;
        for (order, expected) in [
            (Order::Oldest, vec![1, 2, 0]),
            (Order::Newest, vec![0, 2, 1]),
            (Order::Duration, vec![1, 2, 0]),
            (Order::Tokens, vec![0, 1, 2]),
        ] {
            assert_eq!(
                Analysis::build(&events, &Filter::default(), order, || true)
                    .unwrap()
                    .indices,
                expected
            );
        }
    }

    #[test]
    fn empty_and_missing_results_have_no_failure_rate_and_bad_durations_are_zero() {
        for events in [vec![], vec![event("A", 0.0, 4.0, Status::Pending)]] {
            let a = Analysis::build(&events, &Filter::default(), Order::Duration, || true).unwrap();
            assert_eq!(a.failure_rate(), None);
            assert_eq!((a.average(), a.p95_seconds), (0.0, 0.0));
        }
        for (start, end) in [(4.0, 1.0), (f64::NAN, 2.0), (0.0, f64::INFINITY)] {
            assert_eq!(duration(&event("A", start, end, Status::Ok)), 0.0);
        }
    }

    #[test]
    fn percentiles_use_nearest_rank_and_messages_do_not_dilute_tool_metrics() {
        let mut events: Vec<_> = (1..=20)
            .map(|i| event("Exec", 0.0, i as f64, Status::Ok))
            .collect();
        let mut message = event("User", 0.0, 10000.0, Status::Ok);
        message.kind = Kind::User;
        events.push(message);
        let a = Analysis::build(&events, &Filter::default(), Order::Oldest, || true).unwrap();
        assert_eq!(
            (a.completed_tools, a.average(), a.p95_seconds, a.max_seconds),
            (20, 10.5, 19.0, 20.0)
        );
    }

    #[test]
    fn percentile_selection_matches_sorted_values_for_shuffled_and_repeated_durations() {
        for n in [1, 2, 19, 20, 21, 64, 257, 4096] {
            let events: Vec<_> = (0..n).map(|i| {
                let mut e = event("Exec", 0.0, ((i * 104729 + 31) % 127) as f64, Status::Ok);
                if i % 11 == 0 { e.kind = Kind::User; }
                if i % 13 == 0 { e.status = Status::Pending; }
                e
            }).collect();
            let mut sorted: Vec<_> = events.iter().filter(|e| is_tool(e) && e.status != Status::Pending)
                .map(duration).collect();
            sorted.sort_by(f64::total_cmp);
            let expected = if sorted.is_empty() { 0.0 } else { sorted[(sorted.len() * 95).div_ceil(100) - 1] };
            let actual = Analysis::build(&events, &Filter::default(), Order::Duration, || true).unwrap();
            assert_eq!(actual.p95_seconds.to_bits(), expected.to_bits(), "n={n}");
        }
    }

    #[test]
    fn cached_prices_preserve_usage_and_exact_cost_across_model_and_filter_changes() {
        let events: Vec<_> = (0..513).map(|i| {
            let mut e = event("Exec", ((i * 371) % 513) as f64, i as f64 + 1024.0,
                if i % 3 == 0 { Status::Error } else { Status::Ok });
            e.model = match (i / 7) % 5 {
                0 => Some("claude-sonnet-4-5".into()),
                1 => Some("claude-opus-4-1".into()),
                2 => Some("unknown-モデル".into()),
                3 => None,
                _ => Some("claude-sonnet-4-5".into()),
            };
            if i % 11 != 0 {
                e.usage = Usage { input: i as u32 + 1, output: 17, cache_read: 13,
                    cache_write: 7, cache_write_1h: 3, reasoning: 2 };
            }
            e
        }).collect();
        for status in [StatusFilter::All, StatusFilter::Failed] {
            let mut expected = UsageSum::default();
            for e in &events {
                if status == StatusFilter::All || e.status.failed() { expected.add(&e.usage, e.model.as_deref()); }
            }
            for order in [Order::Oldest, Order::Newest, Order::Duration, Order::Tokens] {
                let actual = Analysis::build(&events, &Filter { status, ..Filter::default() }, order, || true).unwrap();
                assert_eq!(actual.usage, expected);
                assert_eq!(actual.usage.cost.to_bits(), expected.cost.to_bits());
                assert_eq!(actual.models, UsageSum::by_model(events.iter().filter(|e| {
                    status == StatusFilter::All || e.status.failed()
                })));
            }
        }
    }

    #[test]
    fn lazy_search_keeps_words_across_fields_and_contextual_unicode_lowercase() {
        let events: Vec<_> = (0..98).map(|i| {
            let mut e = event(&format!("EXEC{i}"), i as f64, i as f64 + 1.0, Status::Ok);
            e.summary = format!("message {i}").into();
            if i == 97 {
                e.summary = "ΟΣ İ".into();
                e.model = Some("MODEL-X".into());
            }
            e
        }).collect();
        let actual = Analysis::build(&events, &Filter {
            query: "exec97 ος i\u{307} model-x".into(), ..Filter::default()
        }, Order::Oldest, || true).unwrap();
        assert_eq!(actual.indices, [97]);
    }

    #[test]
    fn export_preserves_filtered_order_original_positions_and_escaped_text() {
        let mut events = vec![
            event("A", 1.0, 3.0, Status::Error),
            event("B", 2.0, 6.0, Status::Error),
            event("C", 3.0, 4.0, Status::Ok),
        ];
        events[1].summary = "日本語\n\"引用\"".into();
        events[1].usage.reasoning = 3;
        events[1].usage.output = 10;
        let a = Analysis::build(
            &events,
            &Filter {
                status: StatusFilter::Failed,
                ..Filter::default()
            },
            Order::Duration,
            || true,
        )
        .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&a.export_json(&events, 300, "session", "main")).unwrap();
        assert_eq!(json["event_count"], 2);
        assert_eq!(json["events"][0]["event"], 302);
        assert_eq!(json["events"][0]["summary"], events[1].summary.as_ref());
        assert_eq!(json["events"][0]["tokens"], 10);
        assert_eq!(json["events"][0]["usage"]["reasoning"], 3);
    }

    #[test]
    fn cancellation_discards_analysis() {
        let calls = std::cell::Cell::new(0);
        let events = vec![event("A", 0.0, 1.0, Status::Ok); 600];
        assert!(
            Analysis::build(&events, &Filter::default(), Order::Oldest, || {
                calls.set(calls.get() + 1);
                calls.get() < 2
            })
            .is_none()
        );
        assert!(Analysis::build(&[], &Filter::default(), Order::Oldest, || false).is_none());
    }
}
