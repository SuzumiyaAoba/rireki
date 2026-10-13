//! 履歴のファイル名から構文を選び、差分の変更前・変更後をそれぞれ解析する。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use gpui::{FontStyle, FontWeight, UnderlineStyle, px};
use two_face::re_exports::syntect::{
    highlighting::{Color, FontStyle as SyntaxFontStyle, HighlightIterator, HighlightState, Highlighter, Style, Theme},
    parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet},
};

use super::{CODE_TEXT, FilePatch, Spans, color};

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const CACHE_ENTRIES: usize = 64;

struct Cached {
    hash: u64,
    path: String,
    rows: Vec<(char, String)>,
    spans: Rc<Vec<Spans>>,
    bytes: usize,
}

struct Cache {
    entries: VecDeque<Cached>,
    empty: Rc<Vec<Spans>>,
    bytes: usize,
    limit: usize,
}

impl Cache {
    fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            empty: Rc::new(Vec::new()),
            bytes: 0,
            limit,
        }
    }

    fn get(&mut self, patch: &FilePatch, limit: usize) -> Rc<Vec<Spans>> {
        let rows = &patch.rows[..patch.rows.len().min(limit)];
        if rows.is_empty() {
            return self.empty.clone();
        }
        let mut hash = DefaultHasher::new();
        patch.path.hash(&mut hash);
        rows.len().hash(&mut hash);
        for row in rows {
            (row.kind, &row.text).hash(&mut hash);
        }
        let hash = hash.finish();
        if let Some(i) = self.entries.iter().position(|entry| {
            entry.hash == hash
                && entry.path == patch.path
                && entry.rows.len() == rows.len()
                && entry
                    .rows
                    .iter()
                    .zip(rows)
                    .all(|((kind, text), row)| *kind == row.kind && *text == row.text)
        }) {
            let entry = self.entries.remove(i).unwrap();
            let spans = entry.spans.clone();
            self.entries.push_back(entry);
            return spans;
        }
        let spans = Rc::new(diff_spans(patch, limit));
        let bytes = std::mem::size_of::<Cached>()
            + patch.path.len()
            + rows.len() * std::mem::size_of::<(char, String)>()
            + rows.iter().map(|row| row.text.len()).sum::<usize>()
            + std::mem::size_of::<Vec<Spans>>()
            + 2 * std::mem::size_of::<usize>()
            + spans.capacity() * std::mem::size_of::<Spans>()
            + spans
                .iter()
                .map(|line| line.capacity() * std::mem::size_of::<(std::ops::Range<usize>, gpui::HighlightStyle)>())
                .sum::<usize>();
        if bytes <= self.limit {
            while self.entries.len() >= CACHE_ENTRIES || self.bytes + bytes > self.limit {
                self.bytes -= self.entries.pop_front().unwrap().bytes;
            }
            self.entries.push_back(Cached {
                hash,
                path: patch.path.clone(),
                rows: rows.iter().map(|row| (row.kind, row.text.clone())).collect(),
                spans: spans.clone(),
                bytes,
            });
            self.bytes += bytes;
        }
        spans
    }
}

/// 表示中の差分だけを再利用し、本文と装飾の保持量を4 MiB・64項目に制限する。
pub(super) fn cached_diff_spans(patch: &FilePatch, limit: usize) -> Rc<Vec<Spans>> {
    thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::new(CACHE_BYTES)); }
    CACHE.with(|cache| cache.borrow_mut().get(patch, limit))
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let mut theme = two_face::theme::extra()[two_face::theme::EmbeddedThemeName::TwoDark].clone();
        theme.settings.foreground = Some(Color {
            r: (CODE_TEXT >> 16) as u8,
            g: (CODE_TEXT >> 8) as u8,
            b: CODE_TEXT as u8,
            a: 255,
        });
        theme
    })
}

fn syntax(path: &str) -> &'static SyntaxReference {
    let set = syntaxes();
    let path = Path::new(path);
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| set.find_syntax_by_extension(name))
        .or_else(|| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .and_then(|ext| set.find_syntax_by_extension(&ext.to_ascii_lowercase()))
        })
        .unwrap_or_else(|| set.find_syntax_plain_text())
}

/// 変更前・変更後の解析状態を照合し、一致する文脈行だけ処理を共有する。
struct DiffHighlighter<'a> {
    highlighter: Highlighter<'a>,
    parse_state: ParseState,
    highlight_state: HighlightState,
}

impl<'a> DiffHighlighter<'a> {
    fn new(syntax: &SyntaxReference, theme: &'a Theme) -> Self {
        let highlighter = Highlighter::new(theme);
        let highlight_state = HighlightState::new(&highlighter, ScopeStack::new());
        Self {
            highlighter,
            parse_state: ParseState::new(syntax),
            highlight_state,
        }
    }

    fn highlight_line<'b>(
        &mut self,
        line: &'b str,
    ) -> Result<Vec<(Style, &'b str)>, two_face::re_exports::syntect::Error> {
        let ops = self.parse_state.parse_line(line, syntaxes())?;
        Ok(HighlightIterator::new(&mut self.highlight_state, &ops, line, &self.highlighter).collect())
    }
}

fn line_spans(highlighter: &mut DiffHighlighter<'_>, text: &str) -> Spans {
    // 改行で終わる構文定義を使うが、表示する本文の範囲には改行を含めない。
    let line = format!("{text}\n");
    let Ok(tokens) = highlighter.highlight_line(&line) else {
        return Spans::new();
    };
    let mut spans: Spans = Vec::new();
    let mut at = 0;
    for (style, token) in tokens {
        let end = (at + token.len()).min(text.len());
        if at >= end {
            break;
        }
        let fg = style.foreground;
        let mut highlight = color(((fg.r as u32) << 16) | ((fg.g as u32) << 8) | fg.b as u32);
        highlight.font_weight = style
            .font_style
            .contains(SyntaxFontStyle::BOLD)
            .then_some(FontWeight::BOLD);
        highlight.font_style = style
            .font_style
            .contains(SyntaxFontStyle::ITALIC)
            .then_some(FontStyle::Italic);
        highlight.underline = style
            .font_style
            .contains(SyntaxFontStyle::UNDERLINE)
            .then_some(UnderlineStyle {
                thickness: px(1.0),
                color: None,
                wavy: false,
            });
        if let Some((range, previous)) = spans.last_mut()
            && *previous == highlight
        {
            range.end = end;
        } else {
            spans.push((at..end, highlight));
        }
        at = end;
    }
    spans
}

// 再利用はこの差分の解析中だけ。本文と全状態が一致する固定点の行を覚える。
struct StableSpans {
    parse_state: ParseState,
    highlight_state: HighlightState,
    spans: Spans,
}

struct LineMemo<'a> {
    text: &'a str,
    checked: bool,
    stable: Option<StableSpans>,
}

fn memo_line_spans<'a>(highlighter: &mut DiffHighlighter<'_>, text: &'a str, memo: &mut Option<LineMemo<'a>>) -> Spans {
    // 異なる本文では状態を複製しない。繰り返しを見つけた次の行で固定点を確かめる。
    if memo.as_ref().is_none_or(|last| last.text != text) {
        *memo = Some(LineMemo {
            text,
            checked: false,
            stable: None,
        });
        return line_spans(highlighter, text);
    }
    if let Some(stable) = memo.as_ref().and_then(|last| last.stable.as_ref())
        && stable.parse_state == highlighter.parse_state
        && stable.highlight_state == highlighter.highlight_state
    {
        #[cfg(test)]
        MEMO_TEST_HITS.with(|hits| hits.set(hits.get() + 1));
        return stable.spans.clone();
    }
    // 状態が変化した連続行では一度で確認を打ち切り、深い状態の複製を繰り返さない。
    if memo.as_ref().is_some_and(|last| last.checked) {
        return line_spans(highlighter, text);
    }
    #[cfg(test)]
    MEMO_TEST_PROBES.with(|probes| probes.set(probes.get() + 1));
    let parse_state = highlighter.parse_state.clone();
    let highlight_state = highlighter.highlight_state.clone();
    let spans = line_spans(highlighter, text);
    let stable =
        (parse_state == highlighter.parse_state && highlight_state == highlighter.highlight_state).then(|| {
            StableSpans {
                parse_state,
                highlight_state,
                spans: spans.clone(),
            }
        });
    *memo = Some(LineMemo {
        text,
        checked: true,
        stable,
    });
    spans
}

#[cfg(test)]
thread_local! {
    static MEMO_TEST_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MEMO_TEST_HITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 表示する行だけを解析する。ハンク間の省略されたコードには状態を引き継がない。
pub(super) fn diff_spans(patch: &FilePatch, limit: usize) -> Vec<Spans> {
    let rows = &patch.rows[..patch.rows.len().min(limit)];
    if rows.is_empty() {
        return Vec::new();
    }
    let (old_path, new_path) = patch.path.split_once(" → ").unwrap_or((&patch.path, &patch.path));
    let (old_syntax, new_syntax) = (syntax(old_path), syntax(new_path));
    let mut old = DiffHighlighter::new(old_syntax, theme());
    let mut new = DiffHighlighter::new(new_syntax, theme());
    let (mut old_memo, mut new_memo) = (None, None);
    rows.iter()
        .map(|row| match row.kind {
            '@' => {
                old = DiffHighlighter::new(old_syntax, theme());
                new = DiffHighlighter::new(new_syntax, theme());
                old_memo = None;
                new_memo = None;
                Spans::new()
            }
            '-' => memo_line_spans(&mut old, &row.text, &mut old_memo),
            '+' => memo_line_spans(&mut new, &row.text, &mut new_memo),
            _ => {
                if old.parse_state == new.parse_state && old.highlight_state == new.highlight_state {
                    let spans = memo_line_spans(&mut new, &row.text, &mut new_memo);
                    old.parse_state.clone_from(&new.parse_state);
                    old.highlight_state.clone_from(&new.highlight_state);
                    spans
                } else {
                    memo_line_spans(&mut old, &row.text, &mut old_memo);
                    memo_line_spans(&mut new, &row.text, &mut new_memo)
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    fn reference_diff_spans(patch: &FilePatch, limit: usize) -> Vec<Spans> {
        let rows = &patch.rows[..patch.rows.len().min(limit)];
        if rows.is_empty() {
            return Vec::new();
        }
        let (old_path, new_path) = patch.path.split_once(" → ").unwrap_or((&patch.path, &patch.path));
        let (old_syntax, new_syntax) = (syntax(old_path), syntax(new_path));
        let mut old = DiffHighlighter::new(old_syntax, theme());
        let mut new = DiffHighlighter::new(new_syntax, theme());
        rows.iter()
            .map(|row| match row.kind {
                '@' => {
                    old = DiffHighlighter::new(old_syntax, theme());
                    new = DiffHighlighter::new(new_syntax, theme());
                    Spans::new()
                }
                '-' => line_spans(&mut old, &row.text),
                '+' => line_spans(&mut new, &row.text),
                _ => {
                    if old.parse_state == new.parse_state && old.highlight_state == new.highlight_state {
                        let spans = line_spans(&mut new, &row.text);
                        old.parse_state.clone_from(&new.parse_state);
                        old.highlight_state.clone_from(&new.highlight_state);
                        spans
                    } else {
                        line_spans(&mut old, &row.text);
                        line_spans(&mut new, &row.text)
                    }
                }
            })
            .collect()
    }

    #[test]
    fn repeated_line_memo_matches_uncached_spans_and_all_parser_states() {
        let lines = [
            "",
            "",
            "let value = 1;",
            "let value = 1;",
            "let value = 1;",
            "/*",
            "/*",
            "日本語🙂 e\u{301}",
            "日本語🙂 e\u{301}",
            "*/",
            "*/",
            "value = \"\"\"",
            "continued",
            "continued",
            "\"\"\"",
            "\"\"\"",
            "{",
            "{",
            "name",
            "name",
            "}",
            "}",
            "# comment",
            "# comment",
            "\tconst view = <div>あ</div>;",
            "\tconst view = <div>あ</div>;",
        ];
        for path in [
            "a.rs",
            "a.py",
            "a.js",
            "a.ts",
            "a.tsx",
            "a.json",
            "Cargo.toml",
            "a.sh",
            "Dockerfile",
            "a.txt",
            "a.cpp",
            "a.go",
            "a.md",
        ] {
            let syntax = syntax(path);
            let mut control = DiffHighlighter::new(syntax, theme());
            let mut candidate = DiffHighlighter::new(syntax, theme());
            let mut memo = None;
            for (index, text) in lines.iter().cycle().take(lines.len() * 8).enumerate() {
                assert_eq!(
                    memo_line_spans(&mut candidate, text, &mut memo),
                    line_spans(&mut control, text),
                    "{path} {index}"
                );
                assert_eq!(candidate.parse_state, control.parse_state, "{path} parse state {index}");
                assert_eq!(
                    candidate.highlight_state, control.highlight_state,
                    "{path} highlight state {index}"
                );
            }
        }
        MEMO_TEST_HITS.with(|hits| hits.set(0));
        let mut control = DiffHighlighter::new(syntax("a.rs"), theme());
        let mut candidate = DiffHighlighter::new(syntax("a.rs"), theme());
        let mut memo = None;
        for index in 0..1200 {
            assert_eq!(
                memo_line_spans(&mut candidate, "value", &mut memo),
                line_spans(&mut control, "value")
            );
            assert_eq!(candidate.parse_state, control.parse_state, "parse state {index}");
            assert_eq!(
                candidate.highlight_state, control.highlight_state,
                "highlight state {index}"
            );
        }
        assert!(MEMO_TEST_HITS.with(|hits| hits.get()) >= 1190);
    }

    #[test]
    fn changing_repeated_states_probe_once_without_copying_growing_stacks_each_line() {
        MEMO_TEST_PROBES.with(|probes| probes.set(0));
        let mut control = DiffHighlighter::new(syntax("a.rs"), theme());
        let mut candidate = DiffHighlighter::new(syntax("a.rs"), theme());
        let mut memo = None;
        for index in 0..200 {
            assert_eq!(
                memo_line_spans(&mut candidate, "/*", &mut memo),
                line_spans(&mut control, "/*")
            );
            assert_eq!(candidate.parse_state, control.parse_state, "parse state {index}");
            assert_eq!(
                candidate.highlight_state, control.highlight_state,
                "highlight state {index}"
            );
        }
        assert_eq!(MEMO_TEST_PROBES.with(|probes| probes.get()), 1);
        assert!(memo.unwrap().stable.is_none());
    }

    #[test]
    fn repeated_diff_rows_match_uncached_before_after_hunks_and_renames() {
        let texts = [
            "b",
            "a",
            "/*",
            "*/",
            "日本語🙂",
            "value = \"\"\"",
            "continued",
            "\"\"\"",
            "# comment",
            "",
            "\tvalue",
            "let value = 1;",
        ];
        for path in [
            "a.rs",
            "a.py",
            "a.tsx",
            "a.json",
            "a.sh",
            "a.md",
            "old.rs → new.py",
            "old.txt → new.rs",
        ] {
            for pattern in 0..8 {
                let mut patch = FilePatch {
                    op: "変更",
                    path: path.into(),
                    rows: Vec::new(),
                    added: 0,
                    removed: 0,
                };
                for index in 0..160 {
                    let kind = if index % 31 == 0 {
                        '@'
                    } else {
                        ['-', '+', ' ', ' '][(index + pattern) % 4]
                    };
                    let text = if kind == '@' {
                        "@@ -1,10 +1,10 @@"
                    } else {
                        texts[((index / 7) + pattern) % texts.len()]
                    };
                    patch.rows.push(crate::patch::DiffRow {
                        kind,
                        old: None,
                        new: None,
                        text: text.into(),
                    });
                }
                for limit in [0, 1, 2, 31, 32, 64, 65, 159, usize::MAX] {
                    assert_eq!(
                        diff_spans(&patch, limit),
                        reference_diff_spans(&patch, limit),
                        "{path} pattern {pattern} limit {limit}"
                    );
                }
            }
        }
    }

    use super::*;
    use crate::patch::parse_patch;
    use crate::theme::c;
    use gpui::HighlightStyle;

    fn highlighted(path: &str, body: &str) -> (FilePatch, Vec<Spans>) {
        let patch = parse_patch(&format!("*** Update File: {path}\n{body}")).remove(0);
        let spans = diff_spans(&patch, usize::MAX);
        (patch, spans)
    }

    fn style_at(spans: &Spans, offset: usize) -> HighlightStyle {
        spans.iter().find(|(range, _)| range.contains(&offset)).unwrap().1
    }

    #[test]
    fn cached_diff_matches_direct_analysis_after_content_path_and_limit_changes() {
        let (mut patch, expected) = highlighted(
            "src/main.rs",
            "@@\n-let message = \"旧\";\n+let message = \"日本語\";\n message",
        );
        let mut cache = Cache::new(CACHE_BYTES);
        let first = cache.get(&patch, usize::MAX);
        assert_eq!(&*first, &expected);
        assert!(Rc::ptr_eq(&first, &cache.get(&patch.clone(), usize::MAX)));
        assert_eq!(&*cache.get(&patch, 2), &diff_spans(&patch, 2));
        assert!(Rc::ptr_eq(&first, &cache.get(&patch, usize::MAX)));
        patch.rows[1].text = "/* コメント開始".into();
        assert_eq!(&*cache.get(&patch, usize::MAX), &diff_spans(&patch, usize::MAX));
        patch.path = "renamed.rs → renamed.py".into();
        assert_eq!(&*cache.get(&patch, usize::MAX), &diff_spans(&patch, usize::MAX));
        patch.rows[1].kind = '+';
        assert_eq!(&*cache.get(&patch, usize::MAX), &diff_spans(&patch, usize::MAX));
    }

    #[test]
    fn empty_diffs_reuse_one_result_without_evicting_highlighted_rows() {
        let (patch, _) = highlighted("src/main.rs", "@@\n+let value = 1;");
        let mut cache = Cache::new(CACHE_BYTES);
        let highlighted = Rc::downgrade(&cache.get(&patch, usize::MAX));
        let bytes = cache.bytes;
        let empty = cache.get(&patch, 0);
        let mut header = patch.clone();
        header.rows.clear();
        for i in 0..CACHE_ENTRIES * 2 {
            header.path = format!("different-{i}.rs");
            assert!(Rc::ptr_eq(&empty, &cache.get(&header, usize::MAX)));
        }
        assert!(empty.is_empty());
        assert!(highlighted.upgrade().is_some());
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, bytes);
        let weak = Rc::downgrade(&empty);
        drop(empty);
        drop(cache);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn diff_cache_releases_evicted_and_oversized_results() {
        let (mut patch, _) = highlighted("src/main.rs", "@@\n+let value = 1;");
        let mut cache = Cache::new(CACHE_BYTES);
        let first = Rc::downgrade(&cache.get(&patch, usize::MAX));
        for i in 0..CACHE_ENTRIES {
            patch.rows[0].text = format!("let value = {};", i + 2);
            cache.get(&patch, usize::MAX);
        }
        assert!(first.upgrade().is_none());
        let mut cache = Cache::new(4096);
        let first = Rc::downgrade(&cache.get(&patch, usize::MAX));
        for i in 0..20 {
            patch.rows[0].text = format!("let other_value = {i};");
            cache.get(&patch, usize::MAX);
        }
        assert!(first.upgrade().is_none());
        assert!(cache.bytes <= 4096);
        patch.rows[0].text = "あ".repeat(4096);
        let oversized = cache.get(&patch, usize::MAX);
        assert_eq!(&*oversized, &diff_spans(&patch, usize::MAX));
        let weak = Rc::downgrade(&oversized);
        drop(oversized);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn diff_highlights_code_and_preserves_utf8_ranges() {
        let (patch, spans) = highlighted(
            "src/main.rs",
            "@@\n-let message = \"旧\";\n+let message = \"日本語\"; // コメント\n message",
        );
        for (row, spans) in patch.rows.iter().zip(&spans) {
            let mut end = 0;
            for (range, _) in spans {
                assert_eq!(range.start, end);
                assert!(row.text.is_char_boundary(range.start) && row.text.is_char_boundary(range.end));
                end = range.end;
            }
            assert_eq!(end, row.text.len());
        }
        for (row, spans) in patch.rows.iter().zip(&spans).take(2) {
            let keyword = style_at(spans, 0);
            let string = style_at(spans, row.text.find('"').unwrap() + 1);
            assert_ne!(keyword.color, Some(c(CODE_TEXT)));
            assert_ne!(keyword.color, string.color);
        }
        let comment = style_at(&spans[1], patch.rows[1].text.find("//").unwrap() + 3);
        assert_ne!(comment.color, style_at(&spans[1], 0).color);
    }

    #[test]
    fn diff_highlights_common_languages() {
        for (path, code) in [
            ("a.ts", "const message: string = \"日本語\";"),
            ("a.tsx", "const view = <div title=\"hello\">Hello</div>;"),
            ("a.js", "const message = \"hello\";"),
            ("a.json", "{\"value\": true}"),
            ("Cargo.toml", "version = \"0.1.0\""),
            ("a.sh", "echo \"hello\""),
            ("Dockerfile", "FROM alpine:latest"),
        ] {
            let (_, spans) = highlighted(path, &format!("@@\n-{code}\n+{code}"));
            for spans in spans {
                assert!(
                    spans.iter().any(|(_, style)| style.color != Some(c(CODE_TEXT))),
                    "{path}"
                );
            }
        }
    }

    #[test]
    fn diff_keeps_old_and_new_multiline_states_separate() {
        let (_, spans) = highlighted("a.rs", "@@\n-/*\n+let value = 1;\n let other = 2;\n-*/\n+let end = 3;");
        let (_, comment) = highlighted("a.rs", "@@\n /* comment */");
        let (_, code) = highlighted("a.rs", "@@\n let other = 2;");
        assert_eq!(style_at(&spans[2], 0).color, style_at(&code[0], 0).color);
        assert_eq!(style_at(&spans[3], 0).color, style_at(&comment[0], 3).color);

        let (_, spans) = highlighted("a.rs", "@@\n-let value = 1;\n+/*\n let other = 2;\n-let end = 3;\n+*/");
        assert_eq!(style_at(&spans[2], 0).color, style_at(&comment[0], 3).color);
        assert_eq!(style_at(&spans[3], 0).color, style_at(&code[0], 0).color);
    }

    #[test]
    fn diff_keeps_multiline_strings_and_resets_at_hunks() {
        let (_, spans) = highlighted(
            "a.py",
            "@@\n value = \"\"\"\n-old\n+new\n continued\n \"\"\"\n@@ -30 +30 @@\n+def next(): pass",
        );
        let string = style_at(&spans[1], 0).color;
        assert_eq!(style_at(&spans[2], 0).color, string);
        assert_eq!(style_at(&spans[3], 0).color, string);
        assert!(spans[5].is_empty());
        assert_ne!(style_at(&spans[6], 0).color, string);

        let (_, spans) = highlighted("a.rs", "@@\n+/* unfinished\n@@ -30 +30 @@\n+let next = 1;");
        assert!(spans[1].is_empty());
        assert_ne!(style_at(&spans[0], 3).color, style_at(&spans[2], 0).color);
    }

    #[test]
    fn diff_selects_syntax_without_reading_files() {
        for (path, expected) in [
            ("missing/a.rs", "Rust"),
            ("missing/a.RS", "Rust"),
            ("missing/a.ts", "TypeScript"),
            ("missing/a.tsx", "TypeScriptReact"),
            ("missing/a.py", "Python"),
            ("missing/Cargo.toml", "TOML"),
            ("missing/Dockerfile", "Dockerfile"),
            ("missing/a.unknown", "Plain Text"),
        ] {
            assert_eq!(syntax(path).name, expected, "{path}");
        }
        let (patch, spans) = highlighted("a.txt → a.rs", "@@\n-let value = 1;\n+let value = 1;");
        assert_eq!(style_at(&spans[0], 0).color, Some(c(CODE_TEXT)));
        assert_ne!(style_at(&spans[1], 0).color, Some(c(CODE_TEXT)));
        assert_eq!(diff_spans(&patch, 1).len(), 1);
        assert!(diff_spans(&patch, 0).is_empty());
    }
}
