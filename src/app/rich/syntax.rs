//! 履歴のファイル名から構文を選び、差分の変更前・変更後をそれぞれ解析する。

use std::path::Path;
use std::sync::OnceLock;

use gpui::{FontStyle, FontWeight, UnderlineStyle, px};
use two_face::re_exports::syntect::{
    easy::HighlightLines,
    highlighting::{Color, FontStyle as SyntaxFontStyle, Theme},
    parsing::{SyntaxReference, SyntaxSet},
};

use super::{CODE_TEXT, FilePatch, Spans, color};

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let mut theme = two_face::theme::extra()[two_face::theme::EmbeddedThemeName::TwoDark].clone();
        theme.settings.foreground =
            Some(Color { r: (CODE_TEXT >> 16) as u8, g: (CODE_TEXT >> 8) as u8, b: CODE_TEXT as u8, a: 255 });
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

fn line_spans(highlighter: &mut HighlightLines<'_>, text: &str) -> Spans {
    // 改行で終わる構文定義を使うが、表示する本文の範囲には改行を含めない。
    let line = format!("{text}\n");
    let Ok(tokens) = highlighter.highlight_line(&line, syntaxes()) else { return Spans::new() };
    let mut spans: Spans = Vec::new();
    let mut at = 0;
    for (style, token) in tokens {
        let end = (at + token.len()).min(text.len());
        if at >= end {
            break;
        }
        let fg = style.foreground;
        let mut highlight = color(((fg.r as u32) << 16) | ((fg.g as u32) << 8) | fg.b as u32);
        highlight.font_weight = style.font_style.contains(SyntaxFontStyle::BOLD).then_some(FontWeight::BOLD);
        highlight.font_style = style.font_style.contains(SyntaxFontStyle::ITALIC).then_some(FontStyle::Italic);
        highlight.underline = style.font_style.contains(SyntaxFontStyle::UNDERLINE).then_some(UnderlineStyle {
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

/// 表示する行だけを解析する。ハンク間の省略されたコードには状態を引き継がない。
pub(super) fn diff_spans(patch: &FilePatch, limit: usize) -> Vec<Spans> {
    let rows = &patch.rows[..patch.rows.len().min(limit)];
    if rows.is_empty() {
        return Vec::new();
    }
    let (old_path, new_path) = patch.path.split_once(" → ").unwrap_or((&patch.path, &patch.path));
    let (old_syntax, new_syntax) = (syntax(old_path), syntax(new_path));
    let mut old = HighlightLines::new(old_syntax, theme());
    let mut new = HighlightLines::new(new_syntax, theme());
    rows.iter()
        .map(|row| match row.kind {
            '@' => {
                old = HighlightLines::new(old_syntax, theme());
                new = HighlightLines::new(new_syntax, theme());
                Spans::new()
            }
            '-' => line_spans(&mut old, &row.text),
            '+' => line_spans(&mut new, &row.text),
            _ => {
                line_spans(&mut old, &row.text);
                line_spans(&mut new, &row.text)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
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
    fn diff_highlights_code_and_preserves_utf8_ranges() {
        let (patch, spans) =
            highlighted("src/main.rs", "@@\n-let message = \"旧\";\n+let message = \"日本語\"; // コメント\n message");
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
                assert!(spans.iter().any(|(_, style)| style.color != Some(c(CODE_TEXT))), "{path}");
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
