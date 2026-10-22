//! ログ中の制御シーケンスを実行せず、Markdown・ANSI・差分を端末の表示要素へ変換する。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

pub(super) fn clean(text: &str) -> String {
    ansi(text)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn plain(text: impl Into<String>) -> Line<'static> {
    Line::raw(clean(&text.into()).replace('\n', " "))
}

pub(super) fn ansi(text: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    let mut style = Style::default();
    let mut segment = String::new();
    let mut chars = text.chars().peekable();
    let flush = |segment: &mut String, lines: &mut Vec<Line<'static>>, style| {
        if !segment.is_empty() {
            lines
                .last_mut()
                .unwrap()
                .spans
                .push(Span::styled(std::mem::take(segment), style));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                flush(&mut segment, &mut lines, style);
                match chars.next() {
                    Some('[') => {
                        let mut args = String::new();
                        for next in chars.by_ref() {
                            if ('\u{40}'..='\u{7e}').contains(&next) {
                                if next == 'm' {
                                    sgr(&args, &mut style);
                                }
                                break;
                            }
                            if args.len() < 128 {
                                args.push(next);
                            }
                        }
                    }
                    Some(']' | 'P' | '^' | '_') => {
                        while let Some(next) = chars.next() {
                            if next == '\u{7}' {
                                break;
                            }
                            if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            '\n' => {
                flush(&mut segment, &mut lines, style);
                lines.push(Line::default());
            }
            '\r' => {
                // CRLF は改行、プログレス出力の CR は現在の行を上書きする。
                if chars.peek() != Some(&'\n') {
                    segment.clear();
                    lines.last_mut().unwrap().spans.clear();
                }
            }
            '\t' => segment.push_str("    "),
            c if c.is_control() || ('\u{80}'..='\u{9f}').contains(&c) => {}
            c => segment.push(c),
        }
    }
    flush(&mut segment, &mut lines, style);
    lines
}

fn sgr(args: &str, style: &mut Style) {
    let values: Vec<u16> = if args.is_empty() {
        vec![0]
    } else {
        args.split([';', ':'])
            .map(|s| s.parse().unwrap_or(0))
            .collect()
    };
    let mut at = 0;
    while at < values.len() {
        match values[at] {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 => *style = style.fg(Color::Indexed((values[at] - 30) as u8)),
            90..=97 => *style = style.fg(Color::Indexed((values[at] - 90 + 8) as u8)),
            39 => style.fg = None,
            38 => match values.get(at + 1) {
                Some(5) if values.get(at + 2).is_some() => {
                    *style = style.fg(Color::Indexed(values[at + 2].min(255) as u8));
                    at += 2;
                }
                Some(2) if values.get(at + 4).is_some() => {
                    *style = style.fg(Color::Rgb(
                        values[at + 2].min(255) as u8,
                        values[at + 3].min(255) as u8,
                        values[at + 4].min(255) as u8,
                    ));
                    at += 4;
                }
                _ => {}
            },
            _ => {}
        }
        at += 1;
    }
}

pub(super) fn body(text: &str, markdown: bool, numbers: bool, raw: bool) -> Vec<Line<'static>> {
    let mut lines = ansi(text);
    let mut fence = false;
    for (i, line) in lines.iter_mut().enumerate() {
        let value = line.to_string();
        if markdown && !raw {
            let trimmed = value.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fence = !fence;
                *line = Line::styled(value, Style::default().fg(Color::DarkGray));
            } else if fence {
                *line = code(&value);
            } else if trimmed.starts_with('#') {
                *line = Line::styled(
                    value,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            } else if trimmed.starts_with('>') {
                *line = Line::styled(
                    value,
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::ITALIC),
                );
            } else if trimmed.starts_with("---") || trimmed.starts_with("***") {
                *line = Line::styled(value, Style::default().fg(Color::DarkGray));
            } else {
                *line = inline(&value);
            }
        } else if !raw
            && (value.starts_with('+') || value.starts_with('-') || value.starts_with("@@"))
        {
            let color = if value.starts_with('+') {
                Color::Green
            } else if value.starts_with('-') {
                Color::Red
            } else {
                Color::Cyan
            };
            *line = Line::styled(value, Style::default().fg(color));
        }
        if numbers {
            line.spans.insert(
                0,
                Span::styled(
                    format!("{:>4} │ ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
            );
        }
    }
    lines
}

fn inline(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut part = String::new();
    let mut code = false;
    let mut bold = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let is_bold = c == '*' && chars.peek() == Some(&'*') && !code;
        if c == '`' || is_bold {
            let mut style = Style::default();
            if code {
                style = style.fg(Color::Yellow);
            }
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if !part.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut part), style));
            }
            if is_bold {
                chars.next();
                bold = !bold;
            } else {
                code = !code;
            }
        } else {
            part.push(c);
        }
    }
    let mut style = Style::default();
    if code {
        style = style.fg(Color::Yellow);
    }
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    spans.push(Span::styled(part, style));
    Line::from(spans)
}

fn code(text: &str) -> Line<'static> {
    if ["//", "#", "--"]
        .iter()
        .any(|prefix| text.trim_start().starts_with(*prefix))
    {
        return Line::styled(text.to_owned(), Style::default().fg(Color::DarkGray));
    }
    let mut spans = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    for c in text.chars() {
        if let Some(q) = quote {
            token.push(c);
            if c == q {
                spans.push(Span::styled(
                    std::mem::take(&mut token),
                    Style::default().fg(Color::Green),
                ));
                quote = None;
            }
        } else if c == '\'' || c == '"' {
            spans.extend(code_tokens(&std::mem::take(&mut token)));
            token.push(c);
            quote = Some(c);
        } else {
            token.push(c);
        }
    }
    spans.extend(code_tokens(&token));
    Line::from(spans)
}

fn code_tokens(text: &str) -> Vec<Span<'static>> {
    const KEYWORDS: &[&str] = &[
        "async", "await", "break", "case", "catch", "class", "const", "continue", "def", "else",
        "enum", "export", "false", "fn", "for", "from", "function", "if", "impl", "import", "in",
        "let", "match", "mod", "mut", "new", "None", "null", "pub", "return", "self", "Some",
        "struct", "super", "throw", "trait", "true", "try", "type", "use", "var", "while",
    ];
    text.split_inclusive(|c: char| !c.is_alphanumeric() && c != '_')
        .flat_map(|part| {
            let end = part
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .len();
            let (word, punctuation) = part.split_at(end);
            let style = if KEYWORDS.contains(&word) {
                Style::default().fg(Color::Cyan)
            } else if !word.is_empty() && word.chars().all(|c| c.is_ascii_digit() || c == '_') {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            [
                Span::styled(word.to_owned(), style),
                Span::raw(punctuation.to_owned()),
            ]
        })
        .collect()
}

/// 書記素境界で折り返し、全角文字や結合文字の途中を切らない。
pub(super) fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut result = Vec::new();
    let mut current = Line::default().style(line.style);
    let mut used = 0;
    for span in line.spans {
        for grapheme in span.content.graphemes(true) {
            let size = Span::raw(grapheme).width();
            if used > 0 && used + size > width {
                result.push(current);
                current = Line::default().style(line.style);
                used = 0;
            }
            if size <= width {
                current
                    .spans
                    .push(Span::styled(grapheme.to_owned(), span.style));
                used += size;
            }
        }
    }
    result.push(current);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_are_not_forwarded_and_sgr_is_kept_as_style() {
        let lines = ansi("\x1b]52;c;ZXhmaWw=\x07\x1b[31m失敗\x1b[0m\x1b[2J\n10%\r100%\r\n");
        assert_eq!(lines[0].to_string(), "失敗");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Indexed(1)));
        assert_eq!(lines[1].to_string(), "100%");
        assert!(!clean("x\x1b]0;title\x1b\\y\x07").contains('\x1b'));
        assert_eq!(clean("x\x1b]0;title\x1b\\y\x07"), "xy");
    }

    #[test]
    fn markdown_code_diff_and_unicode_wrap_preserve_readable_content() {
        let lines = body(
            "# 見出し\n**強調** と `コード`\n```rust\nlet x = \"日本語\";\n```",
            true,
            true,
            false,
        );
        assert!(lines[1].to_string().contains("強調 と コード"));
        assert!(lines[0].to_string().starts_with("   1 │"));
        let lines = wrap(Line::raw("日本語e\u{301}abc"), 4);
        assert!(lines.iter().all(|line| line.width() <= 4));
        assert_eq!(
            lines.iter().map(Line::to_string).collect::<String>(),
            "日本語e\u{301}abc"
        );
    }
}
