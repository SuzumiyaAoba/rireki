//! 詳細パネル。選択したセッションの概要と、1 イベントなら入力・結果を、複数ならイベントの一覧を出す
//!
//! - 見出し: イベント名と結果の状態（失敗・拒否・中断など）、前後のイベントへの移動、選択の履歴
//! - セッション: 1 行の要約。開くと期間・モデル・ブランチと集計（ターン・ツール・失敗・トークン）、操作
//! - 1 イベント: 時刻・トークン数・きっかけになった指示と、入力・結果（差分・Markdown・ANSI の色・行番号）
//! - 複数: 集計と、名前・失敗・編集したファイルでの絞り込み、ユーザーの指示で区切ったイベントの一覧

use std::collections::HashMap;

use gpui::{
    AnyElement, ClickEvent, ClipboardItem, Context, Div, ElementId, FontWeight, SharedString, Stateful, WeakEntity,
    Window, div, prelude::*, px,
};

use super::rich;
use super::widgets::{dot, project_tile, tip};
use super::{DETAIL_MAX, HEADER_H, INNER_RADIUS, Rireki, Selection, fmt_dur, fmt_n, is_live, local, tilde};
use crate::db::JsCache;
use crate::icons::{self, Icon};
use crate::loader;
use crate::model::{Event, Kind, Session, Source, Stats, Status, Usage, UsageSum, one_line, truncate};
use crate::theme::{self, c};

/// 詳細パネル。選択が変わったときだけ描き直すよう、別ビューとしてキャッシュする
pub struct DetailView {
    pub(super) app: WeakEntity<Rireki>,
}

impl Render for DetailView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.app.upgrade() {
            Some(app) => app.update(cx, |app, cx| app.detail(cx)),
            None => div().into_any_element(),
        }
    }
}

/// 選択中ブロックの詳細（選択時に一度だけ作る）
pub(super) struct DetailCache {
    pub(super) sel: Selection,
    /// 先頭 DETAIL_MAX 件の (input, output)。表示用に切り詰め済み
    texts: Vec<(String, String)>,
    /// exec の JS 本文（生データ表示用）。texts と同じ並び
    raws: Vec<Option<String>>,
    /// 各イベントで編集したファイル (パス, 追加行数, 削除行数)。texts と同じ並び
    touched: Vec<Vec<(String, usize, usize)>>,
    /// 結果の行数（切り詰める前）。texts と同じ並び
    out_lines: Vec<usize>,
    /// 選択全体で編集したファイル（編集の多い順）
    files: Vec<FileStat>,
}

/// 選択の中で編集されたファイル
struct FileStat {
    path: String,
    edits: usize,
    added: usize,
    removed: usize,
}

/// 詳細パネルのイベント一覧の絞り込み
#[derive(Clone, PartialEq, Debug)]
pub(super) enum RowFilter {
    /// ツール名（または User / Assistant など）
    Name(String),
    /// 失敗・拒否・中断
    Failed,
    /// 編集したファイル
    File(String),
}

/// ファイルを編集するツール
const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"];
/// 結果を Markdown として表示するツール（サブエージェントの報告や取得したページ）
const MARKDOWN_OUTPUT: &[&str] = &["Agent", "Task", "WebFetch", "ExitPlanMode", "Skill"];
/// 長い本文を最初に出す行数（差分は行数で数える）
const FOLD_LINES: usize = 60;
const FOLD_DIFF: usize = 160;

fn is_edit(e: &Event) -> bool {
    EDIT_TOOLS.contains(&e.name.as_str())
}

/// セッションを再開するコマンド（作業ディレクトリに移ってから再開する）
fn resume_command(s: &Session) -> String {
    let cwd = format!("'{}'", s.cwd.replace('\'', "'\\''"));
    match s.source {
        Source::Claude => format!("cd {cwd} && claude --resume {}", s.id),
        Source::Codex => format!("cd {cwd} && codex resume {}", s.id),
    }
}

/// 結果の先頭に出る終了コード（Claude の `Exit code 1`、Codex の `[exit 1 · 0.2s]`）
fn exit_code(output: &str) -> Option<&str> {
    let first = output.lines().next()?;
    let rest = first.strip_prefix("Exit code ").or_else(|| first.strip_prefix("[exit "))?;
    let n = rest.bytes().take_while(|b| b.is_ascii_digit() || *b == b'-').count();
    (n > 0).then(|| &rest[..n])
}

/// 結果の状態の表示 (文言, 色)。成功なら None
fn status_label(e: &Event, live: bool, output: &str) -> Option<(String, u32)> {
    match e.status {
        Status::Ok => None,
        Status::Error => Some((exit_code(output).map_or("失敗".into(), |c| format!("失敗 · exit {c}")), theme::ERR)),
        Status::Rejected => Some(("拒否".into(), theme::WARN)),
        Status::Interrupted => Some(("中断".into(), theme::WARN)),
        Status::Pending if live => Some(("実行中".into(), theme::LIVE)),
        Status::Pending => Some(("結果なし".into(), theme::SUBTLE)),
    }
}

fn badge(label: impl Into<SharedString>, color: u32) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_sm()
        .bg(c(color).opacity(0.15))
        .text_xs()
        .text_color(c(color))
        .whitespace_nowrap()
        .child(label.into())
}

/// 小見出し（「入力」「結果」など）
fn label(text: impl Into<SharedString>) -> Div {
    div().flex_none().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::MUTED)).child(text.into())
}

/// 集計の 1 項目（小さい見出しと値）
fn stat(name: &'static str, value: String, color: Option<u32>) -> Div {
    div().flex().flex_col().min_w(px(64.0)).child(div().text_xs().text_color(c(theme::SUBTLE)).child(name)).child(
        div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(c(color.unwrap_or(theme::TEXT))).child(value),
    )
}

impl Rireki {
    /// 選択の詳細テキストを DB から引いて整える。編集したファイルは選択全体から数える
    /// （先頭 DETAIL_MAX 件より後ろは、編集系のツールの本文だけを引く）
    pub(super) fn load_detail(&self, sel: Selection) -> DetailCache {
        let evs = &self.store.sessions[sel.session].lanes[sel.lane].events[sel.first..=sel.last];
        let limit = if evs.len() == 1 { 6000 } else { 400 };
        let shown = evs.len().min(DETAIL_MAX);
        let wanted = |i: usize, e: &Event| i < shown || is_edit(e);
        let rows: Vec<i64> = evs.iter().enumerate().filter(|&(i, e)| wanted(i, e)).filter_map(|(_, e)| e.row).collect();
        let mut fetched = self.reader.as_ref().map(|db| db.details(&rows)).unwrap_or_default();
        let mut d = DetailCache {
            sel,
            texts: Vec::new(),
            raws: Vec::new(),
            touched: Vec::new(),
            out_lines: Vec::new(),
            files: Vec::new(),
        };
        let mut at: HashMap<String, usize> = HashMap::new();
        for (i, e) in evs.iter().enumerate().filter(|&(i, e)| wanted(i, e)) {
            let (input, output) = match e.row.and_then(|r| fetched.remove(&r)) {
                Some(v) => v,
                None => (e.input.clone(), e.output.clone()),
            };
            let (input, raw) = match input.strip_prefix(loader::JS_MARK) {
                Some(js) => (exec_detail_cached(self.js_cache.as_ref(), js), Some(js.to_string())),
                None => (input, None),
            };
            let patches = if is_edit(e) || raw.is_some() { rich::parse_patch(&input) } else { Vec::new() };
            let touched: Vec<(String, usize, usize)> =
                patches.into_iter().map(|p| (p.path, p.added, p.removed)).filter(|p| !p.0.is_empty()).collect();
            for (path, added, removed) in &touched {
                let k = *at.entry(path.clone()).or_insert_with(|| {
                    d.files.push(FileStat { path: path.clone(), edits: 0, added: 0, removed: 0 });
                    d.files.len() - 1
                });
                let f = &mut d.files[k];
                f.edits += 1;
                f.added += added;
                f.removed += removed;
            }
            if i < shown {
                d.out_lines.push(output.lines().count());
                d.raws.push(raw);
                d.touched.push(touched);
                d.texts.push((truncate(&input, limit), truncate(&output, limit)));
            }
        }
        d.files.sort_by(|a, b| b.edits.cmp(&a.edits).then((b.added + b.removed).cmp(&(a.added + a.removed))));
        d
    }

    /// 同じレーンの次（`dir` = 1）/ 前（-1）のイベントを選ぶ。複数を選んでいるときは、その先頭 / 末尾を選ぶ。
    /// 1 イベントずつ進むときは履歴に積まない（「戻る」で一覧に戻れるようにする）
    pub(super) fn step_event(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cur) = self.selected.filter(|&s| self.valid(s)) else { return };
        let n = self.store.sessions[cur.session].lanes[cur.lane].events.len();
        let i = if cur.first != cur.last {
            if dir > 0 { cur.first } else { cur.last }
        } else {
            match cur.first.checked_add_signed(dir).filter(|&i| i < n) {
                Some(i) => i,
                None => return,
            }
        };
        let sel = Selection { session: cur.session, lane: cur.lane, first: i, last: i };
        if cur.first != cur.last {
            self.navigate(sel, window, cx);
        } else {
            self.select(sel, cx);
            cx.notify();
        }
        self.reveal(sel, false, window);
    }

    pub(super) fn detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let panel = div()
            .size_full()
            .flex()
            .flex_col()
            .rounded_r(px(INNER_RADIUS))
            .bg(c(theme::PANEL))
            .border_l_1()
            .border_color(c(theme::BORDER))
            .text_sm()
            .text_color(c(theme::TEXT));
        let Some(sel) = self.selected.filter(|&s| self.valid(s)) else {
            return panel.into_any_element();
        };
        let mut body = div()
            .id("detail-scroll")
            .track_scroll(&self.detail_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.session_card(sel, cx));
        if sel.lane != 0 {
            body = body.child(self.subagent_row(sel, cx));
        }
        if sel.first == sel.last {
            body = body.child(self.event_view(sel, cx));
        } else {
            body = body.child(self.selection_summary(sel, cx)).child(self.event_list(sel, cx));
        }
        panel.child(self.detail_header(sel, cx)).child(body).into_any_element()
    }

    fn cache(&self, sel: Selection) -> Option<&DetailCache> {
        self.detail_cache.as_ref().filter(|d| d.sel == sel)
    }

    /// 見出し。イベント名と状態、前後のイベント・選択の履歴の移動、閉じる
    fn detail_header(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = &self.store.sessions[sel.session];
        let lane = &s.lanes[sel.lane];
        let evs = &lane.events[sel.first..=sel.last];
        let single = evs.len() == 1;
        let (title, color) = if single {
            (evs[0].name.clone(), theme::event_color(&evs[0]))
        } else {
            (format!("{} 件のイベント", fmt_n(evs.len())), theme::MUTED)
        };
        let output = self.cache(sel).and_then(|d| d.texts.first()).map_or("", |t| t.1.as_str());
        let status = if single { status_label(&evs[0], is_live(s.end), output) } else { None };
        let n = lane.events.len();
        let (prev, next) = if single { (sel.first > 0, sel.first + 1 < n) } else { (true, true) };
        div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .pl_3()
            .pr_2()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(dot(color, 8.0))
                    .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when_some(status, |d, (l, col)| d.child(badge(l, col))),
            )
            .child(self.ghost_button(
                "d-prev",
                Icon::ChevronUp,
                "前のイベント",
                Some("K"),
                prev,
                cx,
                |t, w, cx| t.step_event(-1, w, cx),
            ))
            .child(self.ghost_button(
                "d-next",
                Icon::ChevronDown,
                "次のイベント",
                Some("J"),
                next,
                cx,
                |t, w, cx| t.step_event(1, w, cx),
            ))
            .child(div().w(px(1.0)).h(px(16.0)).mx_0p5().bg(c(theme::BORDER)))
            .child(self.ghost_button(
                "d-back",
                Icon::ArrowLeft,
                "戻る",
                Some("⌘["),
                !self.back.is_empty(),
                cx,
                |t, w, cx| t.go_back(w, cx),
            ))
            .child(self.ghost_button(
                "d-forward",
                Icon::ArrowRight,
                "進む",
                Some("⌘]"),
                !self.forward.is_empty(),
                cx,
                |t, w, cx| t.go_forward(w, cx),
            ))
            .child(self.ghost_button("d-locate", Icon::Locate, "選択位置を表示", None, true, cx, |t, w, _| {
                if let Some(sel) = t.selected {
                    t.reveal(sel, true, w);
                }
            }))
            .child(
                self.ghost_button("d-close", Icon::X, "閉じる", Some("esc"), true, cx, |t, _, cx| {
                    t.close_detail(cx)
                }),
            )
    }

    /// セッションの概要。閉じているときは 1 行の要約とタイトルだけを出し、開くと詳細・集計・操作を出す
    fn session_card(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = &self.store.sessions[sel.session];
        let lane = &s.lanes[sel.lane];
        let open = self.session_open;
        let usage = UsageSum::of(s.lanes.iter().flat_map(|l| &l.events));
        let (logo, logo_color) = icons::source_icon(s.source);
        let head = div()
            .id("ses-toggle")
            .flex()
            .items_center()
            .gap_1p5()
            .text_xs()
            .text_color(c(theme::MUTED))
            .cursor_pointer()
            .child(icons::icon(logo, logo_color))
            .child(div().flex_none().child(s.source.label()))
            .child("·")
            .child(project_tile(&s.cwd, 14.0))
            .child(div().min_w_0().truncate().child(self.project_name(&s.cwd)))
            .when(!s.branch.is_empty(), |d| {
                d.child(icons::icon(Icon::GitBranch, theme::SUBTLE).size_3())
                    .child(div().min_w_0().max_w(px(140.0)).truncate().child(s.branch.clone()))
            })
            .when(s.headless, |d| {
                d.child(div().flex_none().px_1().rounded_sm().border_1().border_color(c(theme::BORDER)).child("非対話"))
            })
            .when(is_live(s.end), |d| d.child(dot(theme::LIVE, 7.0)))
            .child(div().flex_1())
            .when(usage.calls > usage.unpriced, |d| {
                d.child(div().flex_none().text_color(c(theme::OK)).child(fmt_usd(usage.cost)))
            })
            .child(div().flex_none().child(fmt_dur(s.end - s.start)))
            .child(icons::icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::SUBTLE).size_3())
            .tooltip(tip(if open { "セッションの詳細を隠す" } else { "セッションの詳細を表示" }, None))
            .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.session_open = !t.session_open;
                t.persist();
                t.detail_changed(cx);
            }));
        let title = if s.title.is_empty() { "(無題)".to_string() } else { s.title.clone() };
        let card = div()
            .flex()
            .flex_col()
            .gap_1p5()
            .px_3()
            .py_2p5()
            .rounded_lg()
            .bg(c(theme::BG))
            .border_1()
            .border_color(c(theme::BORDER))
            .child(head)
            .child(div().font_weight(FontWeight::SEMIBOLD).when(!open, |d| d.truncate()).child(title));
        if !open {
            return card;
        }
        let meta = |k: &'static str, v: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(div().w(px(76.0)).flex_none().text_color(c(theme::SUBTLE)).child(k))
                .child(div().flex_1().min_w_0().child(v))
        };
        let model = match (lane.model.is_empty(), lane.effort.is_empty()) {
            (true, true) => None,
            (false, true) => Some(lane.model.clone()),
            (true, false) => Some(format!("エフォート {}", lane.effort)),
            (false, false) => Some(format!("{} · エフォート {}", lane.model, lane.effort)),
        };
        let version = (!s.version.is_empty()).then(|| match s.source {
            Source::Claude => format!("Claude Code {}", s.version),
            Source::Codex => format!("Codex CLI {}", s.version),
        });
        let st = Stats::of(s.lanes.iter().flat_map(|l| &l.events));
        let mut tiles = div()
            .flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_2()
            .pt_1()
            .child(stat("ターン", fmt_n(st.turns), None))
            .child(stat("ツール", fmt_n(st.tools), None))
            .child(stat("失敗", fmt_n(st.failures()), (st.failures() > 0).then_some(theme::ERR)));
        if s.lanes.len() > 1 {
            tiles = tiles.child(stat("サブエージェント", fmt_n(s.lanes.len() - 1), Some(theme::SUB)));
        }
        if st.peak > 0 {
            tiles = tiles.child(stat("最大コンテキスト", fmt_tokens(st.peak), None));
        }
        let models = UsageSum::by_model(s.lanes.iter().flat_map(|l| &l.events));
        let lane_usage = (s.lanes.len() > 1).then(|| UsageSum::of(&lane.events)).filter(|u| u.calls > 0);
        let resume = resume_command(s);
        let resumed = self.copied == Some("copy-resume");
        let origin = s.lanes.first().map(|l| l.origin.clone()).unwrap_or_default();
        card.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .child(meta(
                    "期間",
                    div()
                        .child(format!(
                            "{} – {}（{}）",
                            local(s.start).format("%Y/%m/%d %H:%M"),
                            local(s.end).format(if local(s.start).date_naive() == local(s.end).date_naive() {
                                "%H:%M"
                            } else {
                                "%m/%d %H:%M"
                            }),
                            fmt_dur(s.end - s.start)
                        ))
                        .into_any_element(),
                ))
                .when_some(model, |d, m| d.child(meta("モデル", div().child(m).into_any_element())))
                .when(!s.branch.is_empty(), |d| {
                    d.child(meta("ブランチ", div().child(s.branch.clone()).into_any_element()))
                })
                .child(meta("ディレクトリ", div().child(tilde(&s.cwd)).into_any_element()))
                .child(meta(
                    "セッション",
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().min_w_0().truncate().font_family(rich::MONO).child(s.id.clone()))
                        .child(self.copy_button("copy-id", s.id.clone(), cx).flex_none())
                        .into_any_element(),
                ))
                .when_some(version, |d, v| d.child(meta("バージョン", div().child(v).into_any_element())))
                .when(usage.calls > 0, |d| d.child(meta("トークン", usage_view(&usage).into_any_element())))
                .when_some(lane_usage, |d, u| d.child(meta("このレーン", usage_view(&u).into_any_element())))
                .when(models.len() > 1 || models.first().is_some_and(|m| m.1.unpriced > 0), |d| {
                    d.child(meta("モデル別", model_table(&models).into_any_element()))
                }),
        )
        .child(tiles)
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1p5()
                .child(
                    self.action_button(
                        "resume",
                        if resumed { Icon::Check } else { Icon::Terminal },
                        if resumed { "コピーしました" } else { "再開コマンドをコピー" },
                    )
                    .tooltip(tip(resume.clone(), None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(resume.clone()));
                        t.flash_copied("copy-resume", cx);
                    })),
                )
                .when(!origin.is_empty(), |d| {
                    d.child(
                        self.action_button("reveal-log", Icon::FolderOpen, "ログを Finder で表示")
                            .tooltip(tip(origin.clone(), None))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                cx.reveal_path(std::path::Path::new(&origin));
                            })),
                    )
                }),
        )
    }

    fn action_button(&self, id: &'static str, icon: Icon, label: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(c(theme::BORDER))
            .bg(c(theme::PANEL))
            .text_xs()
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(icon, if icon == Icon::Check { theme::OK } else { theme::MUTED }))
            .child(label)
    }

    /// サブエージェントのレーンなら、その名前と起動元への移動
    fn subagent_row(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let (session, parent_id) = (sel.session, lane.spawned_by.clone());
        div()
            .flex()
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(c(0xa77bf3).opacity(0.08))
            .border_1()
            .border_color(c(0xa77bf3).opacity(0.3))
            .text_xs()
            .child(icons::icon(Icon::Bot, theme::SUB))
            .child(div().flex_1().min_w_0().text_color(c(theme::SUB)).child(format!("サブエージェント: {}", lane.name)))
            .when_some(parent_id, |d, id| {
                d.child(self.icon_button("to-parent", Some(Icon::ArrowUp), "起動元", false, cx, move |t, w, cx| {
                    let s = &t.store.sessions[session];
                    if let Some(i) = s.lanes[0].events.iter().position(|e| e.call_id.as_deref() == Some(id.as_str())) {
                        let sel = Selection { session, lane: 0, first: i, last: i };
                        t.navigate(sel, w, cx);
                        t.reveal(sel, false, w);
                    }
                }))
            })
    }

    // ------------------------------------------------------------ 1 イベント

    /// 1 イベントの詳細。時刻・トークン数・きっかけの指示と、入力・結果
    fn event_view(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = &self.store.sessions[sel.session];
        let lane = &s.lanes[sel.lane];
        let e = &lane.events[sel.first];
        let cache = self.cache(sel);
        let (input, output) = cache.and_then(|d| d.texts.first()).map_or(("", ""), |t| (t.0.as_str(), t.1.as_str()));
        let raw = cache.and_then(|d| d.raws.first()).and_then(|r| r.as_deref());
        let mut col = div().flex().flex_col().gap_3().min_w_0();

        // 時刻・位置・トークン数
        let when = if e.end > e.start {
            format!(
                "{} → {} · {}",
                local(e.start).format("%m/%d %H:%M:%S"),
                local(e.end).format("%H:%M:%S"),
                fmt_dur(e.end - e.start)
            )
        } else {
            local(e.start).format("%m/%d %H:%M:%S").to_string()
        };
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .text_xs()
                .text_color(c(theme::MUTED))
                .child(div().flex().gap_2().child(div().flex_1().min_w_0().child(when)).child(
                    div().flex_none().text_color(c(theme::SUBTLE)).child(format!(
                        "{} / {}",
                        fmt_n(sel.first + 1),
                        fmt_n(lane.events.len())
                    )),
                ))
                .when(!e.usage.is_empty(), |d| {
                    d.child(div().flex().child(usage_chip("usage-ev", &e.usage, e.model.as_deref())))
                }),
        );

        // きっかけになったユーザーの指示
        if e.kind != Kind::User
            && let Some(ui) = lane.events[..sel.first].iter().rposition(|x| x.kind == Kind::User)
        {
            let u = &lane.events[ui];
            let (session, lane_i) = (sel.session, sel.lane);
            col = col.child(
                div()
                    .id("turn-prompt")
                    .flex()
                    .items_start()
                    .gap_1p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(c(theme::event_color(u)).opacity(0.07))
                    .border_l_2()
                    .border_color(c(theme::event_color(u)).opacity(0.7))
                    .text_xs()
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::event_color(u)).opacity(0.12)))
                    .child(div().flex_none().text_color(c(theme::MUTED)).child("指示"))
                    .child(div().flex_1().min_w_0().line_clamp(2).child(one_line(&u.summary, 200)))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(local(u.start).format("%H:%M").to_string()),
                    )
                    .tooltip(tip("このイベントのきっかけになった指示を開く", None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                        let sel = Selection { session, lane: lane_i, first: ui, last: ui };
                        t.navigate(sel, w, cx);
                        t.reveal(sel, false, w);
                    })),
            );
        }

        match e.kind {
            Kind::User | Kind::Assistant | Kind::Thinking => {
                let color = if e.kind == Kind::Thinking { theme::MUTED } else { theme::TEXT };
                col = col.child(self.titled(
                    None,
                    "copy-in",
                    input,
                    rich::markdown(input, color).text_size(px(13.0)).into_any_element(),
                    cx,
                ));
            }
            Kind::Tool | Kind::Agent => {
                col = col.child(self.tool_input(e, input, raw, &s.cwd, cx));
                if e.kind == Kind::Agent
                    && let Some(li) = e.call_id.as_deref().and_then(|id| s.lane_spawned_by(id))
                {
                    col = col.child(self.subagent_button(sel.session, li, cx));
                }
                let lines = cache.and_then(|d| d.out_lines.first()).copied().unwrap_or(0);
                col = col.child(self.tool_output(e, output, lines, is_live(s.end), &s.cwd, cx));
            }
        }
        col
    }

    /// 見出しとコピーボタンを付けた本文
    fn titled(
        &self,
        head: Option<Div>,
        copy_id: &'static str,
        copy: &str,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .min_h(px(22.0))
                    .child(head.unwrap_or_else(div))
                    .child(div().flex_1())
                    .when(!copy.is_empty(), |d| d.child(self.copy_button(copy_id, copy.to_string(), cx).flex_none())),
            )
            .child(body)
    }

    /// 「残り N 行を表示」。押すと `id` の本文を全部出す
    fn expand_button(&self, id: &'static str, hidden: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("more-{id}")))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_md()
            .text_xs()
            .text_color(c(theme::ACCENT))
            .cursor_pointer()
            .hover(|s| s.bg(c(theme::HOVER)))
            .child(icons::icon(Icon::ChevronDown, theme::ACCENT).size_3())
            .child(format!("残り {} 行を表示", fmt_n(hidden)))
            .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                t.expanded.insert(id);
                t.detail_changed(cx);
            }))
    }

    /// 長い本文を `FOLD_LINES` 行で畳んだ等幅の箱
    fn folded_mono(&self, id: &'static str, text: &str, cx: &mut Context<Self>) -> Div {
        let (shown, hidden) = if self.expanded.contains(id) { (text, 0) } else { rich::head_lines(text, FOLD_LINES) };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(rich::mono_box(shown))
            .when(hidden > 0, |d| d.child(self.expand_button(id, hidden, cx)))
    }

    /// ツールの入力。差分・コマンド・Markdown（計画やサブエージェントへの指示）・引数で見せ方を変える
    fn tool_input(&self, e: &Event, input: &str, raw: Option<&str>, cwd: &str, cx: &mut Context<Self>) -> Div {
        // 要約が入力に含まれていなければ（コマンドの説明など）見出しの下に添える。
        // exec の要約は呼び出しを並べたものなので、整形した入力と重なる
        let key = e.summary.trim_end_matches('…');
        let caption =
            (raw.is_none() && !key.is_empty() && !one_line(input, usize::MAX).contains(key)).then(|| e.summary.clone());
        let markdown = matches!(e.name.as_str(), "ExitPlanMode" | "Agent" | "Task");
        let (title, body): (&str, AnyElement) = if let Some(raw) = raw {
            // exec は整形した呼び出しと JS 本文を切り替える
            let show = self.show_raw;
            let toggle =
                self.segmented("rawmode", &[(false, "整形"), (true, "JS")], Some(show), cx, |t, v, _, cx| {
                    t.show_raw = v;
                    t.detail_changed(cx);
                });
            let text = if show { raw } else { input };
            let body = div().flex().flex_col().gap_1p5().child(toggle).child(self.folded_mono("in", text, cx));
            ("入力", body.into_any_element())
        } else if rich::is_patch(input) {
            let files = rich::parse_patch(input);
            let limit = if self.expanded.contains("in") { usize::MAX } else { FOLD_DIFF };
            let (view, hidden) = rich::diff_view(&files, cwd, limit);
            let body = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(view)
                .when(hidden > 0, |d| d.child(self.expand_button("in", hidden, cx)));
            ("変更", body.into_any_element())
        } else if markdown {
            ("内容", rich::markdown(input, theme::TEXT).text_size(px(13.0)).into_any_element())
        } else if input.starts_with("$ ") {
            ("コマンド", self.folded_mono("in", input, cx).into_any_element())
        } else {
            ("入力", self.folded_mono("in", input, cx).into_any_element())
        };
        let copy = if self.show_raw { raw.unwrap_or(input) } else { input };
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .min_w_0()
            .when_some(caption, |d, cap| d.child(div().text_sm().child(cap)))
            .child(self.titled(Some(label(title)), "copy-in", copy, body, cx))
    }

    /// ツールの結果。失敗なら枠を赤くし、内容に合わせて Markdown・行番号付き・差分・ANSI の色で出す
    fn tool_output(&self, e: &Event, output: &str, lines: usize, live: bool, cwd: &str, cx: &mut Context<Self>) -> Div {
        let status = status_label(e, live, output);
        let head = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(label("結果"))
            .when(lines > 1, |d| {
                d.child(div().text_xs().text_color(c(theme::SUBTLE)).child(format!("{} 行", fmt_n(lines))))
            })
            .when_some(status.clone(), |d, (l, col)| d.child(badge(l, col)));
        let body: AnyElement = if output.is_empty() {
            let note = match e.status {
                Status::Pending if live => "結果はまだありません",
                Status::Pending => "結果は記録されていません",
                _ => "出力なし",
            };
            div().text_xs().text_color(c(theme::SUBTLE)).child(note).into_any_element()
        } else if MARKDOWN_OUTPUT.contains(&e.name.as_str()) {
            rich::markdown(output, theme::TEXT).text_size(px(13.0)).into_any_element()
        } else if rich::is_patch(output) {
            let files = rich::parse_patch(output);
            let limit = if self.expanded.contains("out") { usize::MAX } else { FOLD_DIFF };
            let (view, hidden) = rich::diff_view(&files, cwd, limit);
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(view)
                .when(hidden > 0, |d| d.child(self.expand_button("out", hidden, cx)))
                .into_any_element()
        } else if let Some(rows) = rich::numbered(output) {
            let all = self.expanded.contains("out");
            let hidden = if all { 0 } else { rows.len().saturating_sub(FOLD_LINES) };
            let shown = if all { &rows[..] } else { &rows[..rows.len().min(FOLD_LINES)] };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(rich::numbered_view(shown))
                .when(hidden > 0, |d| d.child(self.expand_button("out", hidden, cx)))
                .into_any_element()
        } else {
            self.folded_mono("out", output, cx).into_any_element()
        };
        let failed = e.status.failed();
        self.titled(Some(head), "copy-out", output, body, cx).when(failed && !output.is_empty(), |d| {
            d.pl_2().border_l_2().border_color(c(status.map_or(theme::ERR, |s| s.1)).opacity(0.6))
        })
    }

    /// サブエージェントのレーンを開くボタン（件数・期間・失敗を添える）
    fn subagent_button(&self, session: usize, li: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let l = &self.store.sessions[session].lanes[li];
        let n = l.events.len();
        let (a, b) = l.range().unwrap_or((0.0, 0.0));
        let st = Stats::of(&l.events);
        let mut text = format!("サブエージェントを開く · {} 件 · {}", fmt_n(n), fmt_dur(b - a));
        if st.failures() > 0 {
            text.push_str(&format!(" · 失敗 {}", st.failures()));
        }
        self.icon_button(
            SharedString::from(format!("to-sub-{li}")),
            Some(Icon::Bot),
            text,
            false,
            cx,
            move |t, w, cx| {
                let sel = Selection { session, lane: li, first: 0, last: n - 1 };
                t.navigate(sel, w, cx);
                t.reveal(sel, true, w);
            },
        )
    }

    // ------------------------------------------------------------ 複数

    /// 選択範囲の期間・件数・集計と、絞り込み（名前・失敗）、編集したファイル
    fn selection_summary(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = &self.store.sessions[sel.session];
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        let (start, end) = self.sel_range(sel);
        let st = Stats::of(evs);
        let mut counts: Vec<(&str, usize, u32)> = Vec::new();
        for e in evs {
            match counts.iter_mut().find(|(n, _, _)| *n == e.name) {
                Some(x) => x.1 += 1,
                None => counts.push((&e.name, 1, theme::event_color(e))),
            }
        }
        counts.sort_by_key(|a| std::cmp::Reverse(a.1));
        let filter = self.row_filter.clone();
        let chip = |id: SharedString, text: String, col: Option<u32>, on: bool, f: Option<RowFilter>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .py(px(1.0))
                .rounded_md()
                .border_1()
                .border_color(c(if on { theme::ACCENT } else { theme::BORDER }))
                .bg(c(if on { theme::ACCENT_BG } else { theme::BG }))
                .text_xs()
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::HOVER)))
                .when_some(col, |d, col| d.child(dot(col, 7.0)))
                .child(text)
                .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    t.row_filter = if t.row_filter == f { None } else { f.clone() };
                    t.detail_changed(cx);
                }))
        };
        let mut chips = div().flex().flex_wrap().gap_1().child(chip(
            "chip-all".into(),
            format!("すべて {}", fmt_n(evs.len())),
            None,
            filter.is_none(),
            None,
        ));
        if st.failures() > 0 {
            let on = filter == Some(RowFilter::Failed);
            chips = chips.child(chip(
                "chip-failed".into(),
                format!("失敗 ×{}", st.failures()),
                Some(theme::ERR),
                on,
                Some(RowFilter::Failed),
            ));
        }
        for (i, &(name, n, col)) in counts.iter().take(12).enumerate() {
            let f = RowFilter::Name(name.to_string());
            let on = filter.as_ref() == Some(&f);
            chips = chips.child(chip(
                SharedString::from(format!("chip-{i}")),
                format!("{name} ×{n}"),
                Some(col),
                on,
                Some(f),
            ));
        }

        let mut facts = vec![format!("ツール {}", fmt_n(st.tools))];
        if st.turns > 0 {
            facts.insert(0, format!("指示 {}", fmt_n(st.turns)));
        }
        let usage = UsageSum::of(evs);
        let mut col = div()
            .flex()
            .flex_col()
            .gap_2()
            .text_xs()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_color(c(theme::TEXT)).child(format!(
                        "{} – {}  ·  {}  ·  {} 件",
                        local(start).format("%m/%d %H:%M:%S"),
                        local(end).format(if local(start).date_naive() == local(end).date_naive() {
                            "%H:%M:%S"
                        } else {
                            "%m/%d %H:%M:%S"
                        }),
                        fmt_dur(end - start),
                        fmt_n(evs.len())
                    )))
                    .child(div().text_color(c(theme::MUTED)).child(facts.join("  ·  "))),
            )
            .when(usage.calls > 0, |d| d.child(usage_view(&usage)))
            .child(chips);
        if let Some(d) = self.cache(sel).filter(|d| !d.files.is_empty()) {
            col = col.child(self.file_list(d, &s.cwd, cx));
        }
        col.pb_1().border_b_1().border_color(c(theme::BORDER))
    }

    /// 選択の中で編集したファイル。押すとそのファイルの編集だけに絞り込む
    fn file_list(&self, d: &DetailCache, cwd: &str, cx: &mut Context<Self>) -> Div {
        const MAX: usize = 8;
        let all = self.expanded.contains("files");
        let (added, removed) = d.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        let mut list = div().flex().flex_col().gap_0p5().child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(label(format!("編集したファイル {}", d.files.len())))
                .child(div().text_color(c(theme::OK)).child(format!("+{}", fmt_n(added))))
                .child(div().text_color(c(theme::ERR)).child(format!("−{}", fmt_n(removed)))),
        );
        for (i, f) in d.files.iter().enumerate().take(if all { usize::MAX } else { MAX }) {
            let filter = RowFilter::File(f.path.clone());
            let on = self.row_filter.as_ref() == Some(&filter);
            let path = rich::rel_path(&f.path, cwd);
            let (dir, base) = match path.rsplit_once('/') {
                Some((d, b)) => (format!("{d}/"), b.to_string()),
                None => (String::new(), path.to_string()),
            };
            list = list.child(
                div()
                    .id(ElementId::NamedInteger("file".into(), i as u64))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1p5()
                    .py(px(2.0))
                    .rounded_md()
                    .cursor_pointer()
                    .when(on, |d| d.bg(c(theme::ACCENT_BG)))
                    .hover(|s| s.bg(c(theme::HOVER)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .child(div().min_w_0().truncate().text_color(c(theme::SUBTLE)).child(dir))
                            .child(div().flex_none().max_w_full().truncate().text_color(c(theme::TEXT)).child(base)),
                    )
                    .child(div().flex_none().text_color(c(theme::SUBTLE)).child(format!("×{}", f.edits)))
                    .child(div().flex_none().text_color(c(theme::OK)).child(format!("+{}", f.added)))
                    .child(div().flex_none().text_color(c(theme::ERR)).child(format!("−{}", f.removed)))
                    .tooltip(tip(f.path.clone(), None))
                    .on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                        t.row_filter = if t.row_filter.as_ref() == Some(&filter) { None } else { Some(filter.clone()) };
                        t.detail_changed(cx);
                    })),
            );
        }
        if !all && d.files.len() > MAX {
            list = list.child(
                div()
                    .id("more-files")
                    .px_1p5()
                    .text_color(c(theme::ACCENT))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(format!("ほか {} ファイル", d.files.len() - MAX))
                    .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                        t.expanded.insert("files");
                        t.detail_changed(cx);
                    })),
            );
        }
        list
    }

    /// イベントの一覧。絞り込みがなければユーザーの指示ごとに区切って並べる
    fn event_list(&self, sel: Selection, cx: &mut Context<Self>) -> Div {
        let s = &self.store.sessions[sel.session];
        let evs = &s.lanes[sel.lane].events[sel.first..=sel.last];
        let cache = self.cache(sel);
        let filter = self.row_filter.as_ref();
        let live = is_live(s.end);
        let mut list = div().flex().flex_col().gap_0p5().min_w_0();
        let mut shown = 0;
        for (i, e) in evs.iter().enumerate().take(DETAIL_MAX) {
            let touched = cache.and_then(|d| d.touched.get(i)).map_or(&[][..], Vec::as_slice);
            let keep = match filter {
                None => true,
                Some(RowFilter::Name(n)) => e.name == *n,
                Some(RowFilter::Failed) => e.status.failed(),
                Some(RowFilter::File(p)) => touched.iter().any(|t| t.0 == *p),
            };
            if !keep {
                continue;
            }
            shown += 1;
            let (preview, output) =
                cache.and_then(|d| d.texts.get(i)).map_or(("", ""), |t| (t.0.as_str(), t.1.as_str()));
            let row = if e.kind == Kind::User && filter.is_none() {
                self.turn_row(sel, i, preview, cx)
            } else {
                self.event_row(sel, i, (preview, output), touched, live, cx)
            };
            list = list.child(row);
        }
        if shown == 0 {
            list =
                list.child(div().py_2().text_xs().text_color(c(theme::SUBTLE)).child("該当するイベントはありません"));
        }
        if evs.len() > DETAIL_MAX {
            list = list.child(div().pt_1().text_xs().text_color(c(theme::MUTED)).child(format!(
                "先頭 {} 件を表示しています（全 {} 件）",
                fmt_n(DETAIL_MAX),
                fmt_n(evs.len())
            )));
        }
        list
    }

    /// ユーザーの指示の行。次の指示までの件数・時間・失敗を添える
    fn turn_row(&self, sel: Selection, i: usize, text: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let lane = &self.store.sessions[sel.session].lanes[sel.lane];
        let idx = sel.first + i;
        let e = &lane.events[idx];
        let end =
            lane.events[idx + 1..].iter().position(|x| x.kind == Kind::User).map_or(lane.events.len(), |p| idx + 1 + p);
        let turn = &lane.events[idx + 1..end];
        let st = Stats::of(turn);
        let until = turn.iter().map(|x| x.end).fold(e.start, f64::max);
        let mut facts = vec![format!("{} 件", fmt_n(turn.len()))];
        if until > e.start {
            facts.push(fmt_dur(until - e.start));
        }
        let color = theme::event_color(e);
        let text = if text.is_empty() { e.summary.as_str() } else { text };
        self.row_frame(sel, idx, cx)
            .mt_1()
            .px_2()
            .py_1p5()
            .bg(c(color).opacity(0.07))
            .border_l_2()
            .border_color(c(color).opacity(0.8))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(local(e.start).format("%H:%M:%S").to_string()),
                    )
                    .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(c(color)).child("指示"))
                    .child(div().flex_1())
                    .child(div().flex_none().text_color(c(theme::SUBTLE)).child(facts.join(" · ")))
                    .when(st.failures() > 0, |d| d.child(badge(format!("失敗 {}", st.failures()), theme::ERR))),
            )
            .child(div().text_sm().line_clamp(3).child(one_line(text, 400)))
    }

    /// 一覧の 1 行。ツールは要約を 1 行で、メッセージは本文の先頭を数行で出す
    fn event_row(
        &self,
        sel: Selection,
        i: usize,
        (text, output): (&str, &str),
        touched: &[(String, usize, usize)],
        live: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let idx = sel.first + i;
        let s = &self.store.sessions[sel.session];
        let e = &s.lanes[sel.lane].events[idx];
        let color = theme::event_color(e);
        let message = matches!(e.kind, Kind::User | Kind::Assistant | Kind::Thinking);
        let status = status_label(e, live, output);
        let (added, removed) = touched.iter().fold((0, 0), |(a, r), t| (a + t.1, r + t.2));
        // ファイルのパスは作業ディレクトリからの相対にして短くする
        let summary = rich::rel_path(&e.summary, &s.cwd).to_string();
        self.row_frame(sel, idx, cx)
            .px_1p5()
            .py_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .child(div().w(px(3.0)).h(px(12.0)).flex_none().rounded_full().bg(c(color)))
                    .child(
                        div()
                            .flex_none()
                            .text_color(c(theme::SUBTLE))
                            .child(local(e.start).format("%H:%M:%S").to_string()),
                    )
                    .child(
                        div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(c(color)).child(e.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c(theme::MUTED))
                            .when(!message, |d| d.child(summary)),
                    )
                    .when(added + removed > 0, |d| {
                        d.child(div().flex_none().text_color(c(theme::OK)).child(format!("+{added}")))
                            .child(div().flex_none().text_color(c(theme::ERR)).child(format!("−{removed}")))
                    })
                    .when_some(status, |d, (l, col)| d.child(badge(l, col)))
                    .when(e.end > e.start, |d| {
                        d.child(div().flex_none().text_color(c(theme::SUBTLE)).child(fmt_dur(e.end - e.start)))
                    })
                    .when(!e.usage.is_empty(), |d| {
                        d.child(usage_chip(
                            ElementId::NamedInteger("usage".into(), idx as u64),
                            &e.usage,
                            e.model.as_deref(),
                        ))
                    }),
            )
            .when(message, |d| {
                let text = if text.is_empty() { e.summary.as_str() } else { text };
                d.child(
                    div()
                        .pl(px(9.0))
                        .text_xs()
                        .line_clamp(if e.kind == Kind::Thinking { 2 } else { 3 })
                        .text_color(c(if e.kind == Kind::Thinking { theme::MUTED } else { theme::TEXT }))
                        .child(one_line(text, 400)),
                )
            })
    }

    /// 一覧の行の外枠。押すとそのイベントだけを選ぶ
    fn row_frame(&self, sel: Selection, idx: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let (session, lane) = (sel.session, sel.lane);
        div()
            .id(ElementId::NamedInteger("ev".into(), idx as u64))
            .min_w_0()
            .flex()
            .flex_col()
            .gap_0p5()
            .rounded_md()
            .cursor_pointer()
            .hover(|d| d.bg(c(theme::HOVER)))
            .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                t.navigate(Selection { session, lane, first: idx, last: idx }, w, cx);
            }))
    }
}

/// トークン数を短く（1,234 / 12.3k / 1.23M）
fn fmt_tokens(n: u64) -> String {
    match n {
        0..10_000 => fmt_n(n as usize),
        10_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// 使用量の内訳の色: 入力・キャッシュ読込・キャッシュ書込・出力
const USAGE_COLORS: [u32; 4] = [0x5b9cf5, 0x3f6a9e, 0xd29a4a, 0x7fd18b];

/// 使用量の合計・内訳の帯・凡例
fn usage_view(u: &UsageSum) -> Div {
    let total = u.total().max(1) as f32;
    let parts = [
        ("入力", u.input, USAGE_COLORS[0]),
        ("キャッシュ読込", u.cache_read, USAGE_COLORS[1]),
        ("キャッシュ書込", u.cache_write, USAGE_COLORS[2]),
        ("出力", u.output, USAGE_COLORS[3]),
    ];
    div()
        .flex()
        .flex_col()
        .gap_1()
        .text_xs()
        .child(
            div()
                .flex()
                .gap_1p5()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(fmt_tokens(u.total())))
                .when(u.calls > u.unpriced, |d| {
                    d.child(div().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::OK)).child(fmt_usd(u.cost)))
                })
                .child(div().text_color(c(theme::SUBTLE)).child(format!("· {} 回", fmt_n(u.calls as usize))))
                .when(u.unpriced > 0, |d| {
                    d.child(
                        div()
                            .id("unpriced")
                            .text_color(c(theme::SUBTLE))
                            .child(format!("（料金不明 {} 回）", fmt_n(u.unpriced as usize)))
                            .tooltip(tip(
                                format!(
                                    "料金表にないモデル。{} に追加できます",
                                    tilde(&crate::pricing::path().to_string_lossy())
                                ),
                                None,
                            )),
                    )
                }),
        )
        .child(div().flex().h(px(6.0)).w_full().rounded_sm().overflow_hidden().bg(c(theme::GRID)).children(
            parts.iter().filter(|p| p.1 > 0).map(|&(_, n, col)| {
                div().h_full().flex_basis(gpui::relative(n as f32 / total)).flex_shrink_0().bg(c(col))
            }),
        ))
        .child(div().flex().flex_wrap().gap_x_2().gap_y_0p5().text_color(c(theme::MUTED)).children(
            parts.iter().filter(|p| p.1 > 0).map(|&(label, n, col)| {
                div().flex().items_center().gap_1().child(dot(col, 7.0)).child(format!("{label} {}", fmt_tokens(n)))
            }),
        ))
        .when(u.reasoning > 0, |d| {
            d.child(div().text_color(c(theme::SUBTLE)).child(format!("出力のうち推論 {}", fmt_tokens(u.reasoning))))
        })
}

/// 料金（USD）
fn fmt_usd(v: f64) -> String {
    if v < 0.01 && v > 0.0 { format!("${v:.4}") } else { format!("${v:.2}") }
}

/// モデルごとのトークン数と料金
fn model_table(models: &[(String, UsageSum)]) -> Div {
    div().flex().flex_col().gap_0p5().children(models.iter().map(|(m, u)| {
        div()
            .flex()
            .gap_2()
            .child(div().flex_1().min_w_0().truncate().child(if m.is_empty() {
                "(不明)".to_string()
            } else {
                m.clone()
            }))
            .child(div().flex_none().text_color(c(theme::MUTED)).child(fmt_tokens(u.total())))
            .child(div().flex_none().w(px(64.0)).flex().justify_end().child(if u.unpriced > 0 && u.cost == 0.0 {
                div().text_color(c(theme::SUBTLE)).child("—")
            } else {
                div().text_color(c(theme::OK)).child(fmt_usd(u.cost))
            }))
    }))
}

/// イベントに添える使用量（入力はキャッシュ込み）。内訳・モデル・料金はツールチップに出す
fn usage_chip(id: impl Into<ElementId>, u: &Usage, model: Option<&str>) -> Stateful<Div> {
    let input = u.input as u64 + u.cache_read as u64 + u.cache_write as u64;
    let mut detail = format!(
        "入力 {} / キャッシュ読込 {} / キャッシュ書込 {} / 出力 {}",
        fmt_n(u.input as usize),
        fmt_n(u.cache_read as usize),
        fmt_n(u.cache_write as usize),
        fmt_n(u.output as usize)
    );
    if u.reasoning > 0 {
        detail.push_str(&format!("（うち推論 {}）", fmt_n(u.reasoning as usize)));
    }
    if let Some(m) = model {
        detail.push_str(&format!("\n{m}"));
        if let Some(p) = crate::pricing::price(m) {
            detail.push_str(&format!(" · {}", fmt_usd(p.cost(u))));
        }
    }
    div()
        .id(id)
        .flex_none()
        .text_xs()
        .text_color(c(theme::SUBTLE))
        .child(format!("↑{} ↓{}", fmt_tokens(input), fmt_tokens(u.output as u64)))
        .tooltip(tip(detail, None))
}

/// exec の JS を整形する。初回はモック実行してキャッシュに保存する
pub(super) fn exec_detail_cached(cache: Option<&JsCache>, js: &str) -> String {
    let h = crate::jsmock::hash(js);
    if let Some(d) = cache.and_then(|c| c.get(h)) {
        return d;
    }
    let d = loader::exec_detail(js);
    if let Some(c) = cache {
        c.put(h, &d);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_quotes_cwd() {
        let s = Session {
            id: "abc".into(),
            source: Source::Claude,
            cwd: "/w/it's here".into(),
            headless: false,
            title: String::new(),
            branch: String::new(),
            version: String::new(),
            lanes: Vec::new(),
            start: 0.0,
            end: 0.0,
            parents: Vec::new(),
            tree: Vec::new(),
        };
        assert_eq!(resume_command(&s), r#"cd '/w/it'\''s here' && claude --resume abc"#);
        let s = Session { source: Source::Codex, cwd: "/w".into(), ..s };
        assert_eq!(resume_command(&s), "cd '/w' && codex resume abc");
    }

    #[test]
    fn short_numbers() {
        assert_eq!(fmt_tokens(9_999), "9,999");
        assert_eq!(fmt_tokens(85_234), "85.2k");
        assert_eq!(fmt_tokens(12_345_678), "12.35M");
        assert_eq!(exit_code("Exit code 127\nnot found"), Some("127"));
        assert_eq!(exit_code("[exit 1 · 0.2s]\nerr"), Some("1"));
        assert_eq!(exit_code("[0.2s]\nok"), None);
    }
}
