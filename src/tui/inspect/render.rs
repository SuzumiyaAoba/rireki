use super::*;
use rireki::detail::analysis::{duration, kind_name, status_name};
use rireki::detail::summary::Totals;
use rireki::model::UsageSum;
use rireki::time::local;

const EVENTS_PER_PAGE: usize = 500;
const FOLDED_LINES: usize = 60;
const FOLDED_DIFF: usize = 160;

fn heading(text: impl Into<String>) -> Entry {
    Entry::styled(
        text,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

fn muted(text: impl Into<String>) -> Entry {
    Entry::styled(text, Style::default().fg(Color::DarkGray))
}

pub(super) fn document(inspect: &mut Inspector, context: Context<'_>) -> Vec<Entry> {
    match inspect.tab {
        Tab::Overview => overview(inspect, context),
        Tab::History => history(inspect, context, false),
        Tab::Files => files(inspect, context),
        Tab::Analysis => analysis(inspect, context),
        Tab::Replay => history(inspect, context, true),
        Tab::Body => body(inspect, context),
    }
}

fn overview(inspect: &mut Inspector, context: Context<'_>) -> Vec<Entry> {
    let session = context.session;
    let mut out = vec![
        heading(&session.title),
        Entry::text(format!("ID: {}", session.id)),
        Entry::text(format!(
            "ソース: {:?}  バージョン: {}  起動: {}",
            session.source,
            session.version,
            if session.headless {
                "非対話"
            } else {
                "対話"
            }
        )),
        Entry::text(format!("ディレクトリ: {}", session.cwd)),
        Entry::text(format!("ブランチ: {}", session.branch)),
        Entry::text(format!(
            "期間: {} → {}  ({:.1}秒)",
            date(session.start),
            date(session.end),
            (session.end - session.start).max(0.0)
        )),
        Entry::text(format!("再開: {}", resume_command(session))),
        muted("c:再開コマンドをコピー  C:IDをコピー  g:ログの場所  a:レーン全体  [ ]:レーン移動"),
    ];
    if let Some(cache) = context.cache() {
        out.push(heading("セッション全体の使用量"));
        totals(&mut out, &cache.session_summary.total);
        models(&mut out, &cache.session_summary.models);
        out.push(heading("指示と応答（Enter で本文）"));
        let lane_index = context.selection.lane;
        if let (Some(turns), Some(lane)) = (
            cache.session_summary.turns.get(lane_index),
            session.lanes.get(lane_index),
        ) {
            let range = pages(inspect, turns.len(), false);
            for turn in turns.get(range).unwrap_or_default() {
                if let Some(event) = lane.events.get(turn.index) {
                    out.push(
                        Entry::text(format!(
                            "{} #{} 指示: {}",
                            clock(event.start),
                            turn.index + 1,
                            event.summary
                        ))
                        .target(Target::Event {
                            lane: lane_index,
                            event: turn.index,
                        }),
                    );
                    out.push(muted(format!(
                        "  {}イベント / 失敗{} / {:.1}秒",
                        turn.events,
                        turn.failures,
                        (turn.end - event.start).max(0.0)
                    )));
                }
                if let Some(index) = turn.response
                    && let Some(event) = lane.events.get(index)
                {
                    out.push(Entry::text(format!("  ↳ 応答: {}", event.summary)).target(
                        Target::Event {
                            lane: lane_index,
                            event: index,
                        },
                    ));
                }
            }
        }
    } else {
        out.push(muted("集計・本文を読み込んでいます…"));
    }
    out.push(heading(
        "エージェント構成（Enter でレーン全体、b で起動元）",
    ));
    let tree: Vec<_> = if session.tree.is_empty() {
        (0..session.lanes.len()).map(|lane| (lane, 0)).collect()
    } else {
        session.tree.clone()
    };
    for (index, depth) in tree {
        let Some(lane) = session.lanes.get(index) else {
            continue;
        };
        let target = Target::Lane(index);
        out.push(
            Entry::text(format!(
                "{}{}  {}イベント  {} {}",
                "  ".repeat(depth.min(16)),
                lane.name,
                lane.events.len(),
                lane.model,
                lane.effort
            ))
            .target(target),
        );
        if let Some(cache) = context.cache()
            && let Some(total) = cache.session_summary.lanes.get(index)
        {
            out.push(
                muted(format!(
                    "  {} tokens / {} / 失敗{}",
                    total.usage.total(),
                    cost(&total.usage),
                    total.stats.failures()
                ))
                .target(target),
            );
        }
        if !lane.origin.is_empty() {
            out.push(muted(format!("  ログ: {}", lane.origin)).target(target));
        }
        if let Some(parent) = session
            .parents
            .get(index)
            .copied()
            .flatten()
            .and_then(|parent| session.lanes.get(parent))
        {
            out.push(muted(format!("  起動元: {}", parent.name)).target(target));
        }
    }
    out
}

fn totals(out: &mut Vec<Entry>, total: &Totals) {
    out.push(Entry::text(format!(
        "指示{} / ツール{} / 失敗{} (エラー{}・拒否{}・中断{}) / 最大コンテキスト{}",
        total.stats.turns,
        total.stats.tools,
        total.stats.failures(),
        total.stats.errors,
        total.stats.rejected,
        total.stats.interrupted,
        total.stats.peak
    )));
    usage(out, &total.usage);
}

fn usage(out: &mut Vec<Entry>, usage: &UsageSum) {
    out.push(Entry::text(format!(
        "合計 {} tokens / API呼出 {} / 推定API料金 {}",
        usage.total(),
        usage.calls,
        cost(usage)
    )));
    out.push(Entry::text(format!(
        "入力 {} / キャッシュ読込 {} / 書込 {} / 出力 {} / 推論 {}（出力に含む）",
        usage.input, usage.cache_read, usage.cache_write, usage.output, usage.reasoning
    )));
    out.push(Entry::text(format!(
        "キャッシュヒット率 {}",
        usage
            .cache_hit_rate()
            .map_or_else(|| "—".into(), |rate| format!("{:.1}%", rate * 100.0))
    )));
}

fn cost(usage: &UsageSum) -> String {
    if usage.calls == 0 {
        "—（使用量未記録）".to_owned()
    } else if usage.unpriced > 0 {
        format!("${:.6} + 料金不明{}呼出", usage.cost, usage.unpriced)
    } else {
        format!("${:.6}", usage.cost)
    }
}

fn models(out: &mut Vec<Entry>, models: &[(String, UsageSum)]) {
    if models.is_empty() {
        return;
    }
    out.push(heading("モデル別"));
    for (name, usage) in models {
        out.push(Entry::text(format!(
            "{}: {} tokens / {} / 入力{} 読込{} 書込{} 出力{} 推論{}",
            if name.is_empty() { "不明" } else { name },
            usage.total(),
            cost(usage),
            usage.input,
            usage.cache_read,
            usage.cache_write,
            usage.output,
            usage.reasoning
        )));
    }
}

fn pages(inspect: &mut Inspector, count: usize, follow: bool) -> std::ops::Range<usize> {
    inspect.event_pages = count.div_ceil(EVENTS_PER_PAGE).max(1);
    inspect.event_page = if follow {
        inspect.event_pages - 1
    } else {
        inspect.event_page.min(inspect.event_pages - 1)
    };
    let start = inspect.event_page * EVENTS_PER_PAGE;
    start..(start + EVENTS_PER_PAGE).min(count)
}

fn history(inspect: &mut Inspector, context: Context<'_>, replay: bool) -> Vec<Entry> {
    let events = context.events();
    let mut out = Vec::new();
    if let Some(cache) = context.cache() {
        out.push(heading(format!(
            "選択範囲 #{}–{} ({}イベント)",
            context.selection.first + 1,
            context.selection.last + 1,
            events.len()
        )));
        totals(&mut out, &cache.selection_summary.total);
        out.push(muted(
            cache
                .selection_summary
                .names
                .iter()
                .map(|(name, n, _)| format!("{name}:{n}"))
                .collect::<Vec<_>>()
                .join("  "),
        ));
    }
    if replay {
        inspect.ensure_playback(context);
    }
    let visible = if replay {
        inspect.playback.as_ref().map_or(events.len(), |playback| {
            events.partition_point(|event| event.start <= playback.timestamp())
        })
    } else {
        events.len()
    };
    let analysis = inspect.analysis.clone();
    if analysis.is_none() && (inspect.filter != Filter::default() || inspect.order != Order::Oldest)
    {
        out.push(muted("絞り込みを計算しています…"));
        return out;
    }
    let count = analysis.as_ref().map_or(visible, |analysis| {
        if replay {
            analysis
                .indices
                .iter()
                .filter(|&&index| index < visible)
                .count()
        } else {
            analysis.indices.len()
        }
    });
    let range = pages(inspect, count, replay && inspect.follow_replay);
    out.push(muted(format!(
        "{}件 / ページ{}/{}（, . で前後500件）",
        count,
        inspect.event_page + 1,
        inspect.event_pages
    )));
    let indices: Box<dyn Iterator<Item = usize> + '_> = if let Some(analysis) = &analysis {
        Box::new(
            analysis
                .indices
                .iter()
                .copied()
                .filter(|&index| index < visible),
        )
    } else {
        Box::new(0..visible)
    };
    for index in indices.skip(range.start).take(range.len()) {
        let Some(event) = events.get(index) else {
            continue;
        };
        let target = Target::Event {
            lane: context.selection.lane,
            event: context.selection.first + index,
        };
        out.push(event_row(event, context.selection.first + index).target(target));
        if let Some((input, output)) = context.cache().and_then(|cache| cache.texts.get(index)) {
            let value = if matches!(event.kind, Kind::User | Kind::Assistant | Kind::Thinking)
                || output.is_empty()
            {
                input
            } else {
                output
            };
            for line in rich::body(value, !event.kind.is_tool(), false, inspect.raw)
                .into_iter()
                .take(if inspect.expanded_body { 60 } else { 2 })
            {
                out.push(Entry { line, target });
            }
        }
    }
    if count == 0 {
        out.push(muted(if replay {
            "この時点で表示できるイベントはありません"
        } else {
            "条件に一致するイベントはありません"
        }));
    }
    out
}

fn event_row(event: &Event, index: usize) -> Entry {
    let style = Style::default().fg(if event.status.failed() {
        Color::Red
    } else {
        match event.kind {
            Kind::User => Color::Cyan,
            Kind::Assistant => Color::Green,
            Kind::Thinking => Color::DarkGray,
            Kind::Agent => Color::Magenta,
            Kind::Tool => Color::Yellow,
        }
    });
    Entry::styled(
        format!(
            "#{} {} {} {} [{}] {:.2}秒 {} tokens · {}",
            index + 1,
            clock(event.start),
            kind_name(event.kind),
            event.name,
            status_name(event.status),
            duration(event),
            event.usage.total(),
            event.summary
        ),
        style,
    )
}

fn files(inspect: &mut Inspector, context: Context<'_>) -> Vec<Entry> {
    let Some(cache) = context.cache() else {
        return vec![muted("変更履歴を読み込んでいます…")];
    };
    let mut out = vec![heading(format!(
        "変更ファイル {}件（選択全体、本文300件の上限に依存しません）",
        cache.files.len()
    ))];
    let range = pages(inspect, cache.files.len(), false);
    for index in range {
        let file = &cache.files[index];
        let open = inspect.open_files.contains(&file.path);
        out.push(
            Entry::text(format!(
                "{} {}  {}編集 +{} -{}",
                if open { "▼" } else { "▶" },
                file.path,
                file.edits,
                file.added,
                file.removed
            ))
            .target(Target::File(index)),
        );
        if !open {
            continue;
        }
        for &event in &file.events {
            let target = Target::Event {
                lane: context.selection.lane,
                event: context.selection.first + event,
            };
            let label = context.events().get(event).map_or_else(
                || format!("  #{}", context.selection.first + event + 1),
                |e| {
                    format!(
                        "  #{} {} {} [{}]（Enter でイベント本文）",
                        context.selection.first + event + 1,
                        clock(e.start),
                        e.name,
                        status_name(e.status)
                    )
                },
            );
            out.push(heading(label).target(target));
            for patch in cache
                .patches
                .get(&event)
                .into_iter()
                .flatten()
                .filter(|patch| patch.path == file.path)
            {
                out.push(
                    muted(format!(
                        "  {} +{} -{}",
                        patch.op, patch.added, patch.removed
                    ))
                    .target(target),
                );
                let limit = if inspect.all_diffs {
                    patch.rows.len()
                } else {
                    FOLDED_DIFF
                };
                for row in patch.rows.iter().take(limit) {
                    let color = match row.kind {
                        '+' => Color::Green,
                        '-' => Color::Red,
                        '@' => Color::Cyan,
                        _ => Color::Gray,
                    };
                    let numbers = if inspect.line_numbers {
                        format!(
                            "{:>5} {:>5} ",
                            row.old.map_or(String::new(), |n| n.to_string()),
                            row.new.map_or(String::new(), |n| n.to_string())
                        )
                    } else {
                        String::new()
                    };
                    out.push(
                        Entry::styled(
                            format!("{numbers}{} {}", row.kind, row.text),
                            Style::default().fg(color),
                        )
                        .target(target),
                    );
                }
                if patch.rows.len() > limit {
                    out.push(
                        muted(format!(
                            "… 残り{}行（x で全行表示）",
                            patch.rows.len() - limit
                        ))
                        .target(target),
                    );
                }
            }
        }
    }
    if cache.files.is_empty() {
        out.push(muted("この範囲にファイルの変更はありません"));
    }
    out
}

fn analysis(inspect: &mut Inspector, context: Context<'_>) -> Vec<Entry> {
    let Some(analysis) = inspect.analysis.clone() else {
        return vec![muted("分析しています…")];
    };
    let mut out = vec![heading(format!(
        "絞り込み後 {}イベント / 失敗{} / 結果なし{}",
        analysis.indices.len(),
        analysis.failures,
        analysis.pending
    ))];
    usage(&mut out, &analysis.usage);
    out.push(Entry::text(format!(
        "経過 {:.2}秒 / ツール実行合計 {:.2}秒 / 平均 {:.2}秒 / p95 {:.2}秒 / 最大 {:.2}秒",
        analysis.span,
        analysis.tool_seconds,
        analysis.average(),
        analysis.p95_seconds,
        analysis.max_seconds
    )));
    out.push(Entry::text(format!(
        "失敗率 {}（未完了を除く） / 最大コンテキスト {}",
        analysis
            .failure_rate()
            .map_or_else(|| "—".to_owned(), |rate| format!("{:.1}%", rate * 100.0)),
        analysis.peak_context
    )));
    out.push(muted(
        "ツール実行合計は並行実行の重なりを含みます。e:JSON保存 / y:JSONコピー",
    ));
    models(&mut out, &analysis.models);
    out.push(heading("ツール別 所要時間・失敗"));
    for tool in &analysis.tools {
        out.push(Entry::text(format!(
            "{}: {}回 / 完了{} / 失敗{} / 結果なし{} / 合計{:.2}秒 / 平均{:.2}秒 / 最大{:.2}秒",
            tool.name,
            tool.calls,
            tool.completed,
            tool.failures,
            tool.pending,
            tool.seconds,
            tool.average(),
            tool.max_seconds
        )));
    }
    out.push(heading(
        "イベント（Enter で本文、並び順と絞り込みを JSON に反映）",
    ));
    let range = pages(inspect, analysis.indices.len(), false);
    let events = context.events();
    for &index in analysis.indices.get(range).unwrap_or_default() {
        if let Some(event) = events.get(index) {
            out.push(
                event_row(event, context.selection.first + index).target(Target::Event {
                    lane: context.selection.lane,
                    event: context.selection.first + index,
                }),
            );
        }
    }
    out
}

fn body(inspect: &mut Inspector, context: Context<'_>) -> Vec<Entry> {
    let Some(event) = context.events().first() else {
        return vec![muted("イベントがありません")];
    };
    let mut out = vec![
        event_row(event, context.selection.first),
        Entry::text(format!(
            "開始 {} / 終了 {} / モデル {}",
            date(event.start),
            date(event.end),
            event.model.as_deref().unwrap_or("—")
        )),
    ];
    let mut sum = UsageSum::default();
    sum.add(&event.usage, event.model.as_deref());
    usage(&mut out, &sum);
    let Some(cache) = context.cache() else {
        out.push(muted("本文を読み込んでいます…"));
        return out;
    };
    let Some((input, output)) = cache.texts.first() else {
        out.push(muted("記録された本文はありません"));
        return out;
    };
    if context.selection.first != context.selection.last {
        out.push(muted(
            "範囲先頭の本文です。履歴・分析で Enter を押すと各イベントの本文を開きます。",
        ));
        out.push(
            Entry::text("▶ 先頭イベントの本文を個別に読み込む（Enter）").target(Target::Event {
                lane: context.selection.lane,
                event: context.selection.first,
            }),
        );
    }
    out.push(heading(if event.kind.is_tool() {
        "入力"
    } else {
        "メッセージ"
    }));
    let raw = cache.raws.first().and_then(|raw| raw.as_deref());
    append_body(
        &mut out,
        if inspect.raw {
            raw.unwrap_or(input)
        } else {
            input
        },
        !event.kind.is_tool(),
        inspect,
    );
    if let Some(commands) = cache
        .commands
        .first()
        .filter(|commands| !commands.is_empty())
    {
        out.push(heading("実行コマンド（全文）"));
        for command in commands {
            out.extend(
                rich::body(command, false, false, true)
                    .into_iter()
                    .map(|line| Entry {
                        line,
                        target: Target::None,
                    }),
            );
        }
    }
    if !output.is_empty() {
        out.push(heading(format!(
            "結果（記録 {}行）",
            cache.out_lines.first().copied().unwrap_or_default()
        )));
        append_body(
            &mut out,
            output,
            matches!(
                event.name.as_ref(),
                "Agent" | "Task" | "WebFetch" | "ExitPlanMode" | "Skill"
            ),
            inspect,
        );
    }
    if let Some(touched) = cache.touched.first().filter(|touched| !touched.is_empty()) {
        out.push(heading("編集したファイル"));
        for (path, added, removed) in touched {
            out.push(Entry::text(format!("{path} +{added} -{removed}")));
        }
    }
    if let Some(lane) = event
        .call_id
        .as_deref()
        .and_then(|call| context.session.lane_spawned_by(call))
    {
        out.push(
            Entry::text("▶ 子エージェントを開く（Enter または d）").target(Target::Lane(lane)),
        );
    }
    out
}

fn append_body(out: &mut Vec<Entry>, text: &str, markdown: bool, inspect: &Inspector) {
    let lines = rich::body(text, markdown, inspect.line_numbers, inspect.raw);
    let total = lines.len();
    let limit = if inspect.expanded_body {
        total
    } else {
        FOLDED_LINES
    };
    out.extend(lines.into_iter().take(limit).map(|line| Entry {
        line,
        target: Target::None,
    }));
    if total > limit {
        out.push(muted(format!("… 残り{}行（x で展開）", total - limit)));
    }
}

fn date(time: f64) -> String {
    if time.is_finite() {
        local(time).format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        "—".to_owned()
    }
}
fn clock(time: f64) -> String {
    if time.is_finite() {
        local(time).format("%H:%M:%S").to_string()
    } else {
        "—".to_owned()
    }
}
