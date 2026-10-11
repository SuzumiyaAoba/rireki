//! 詳細パネルの本文の表示。Markdown・差分・ANSI の色付きの出力・行番号付きのファイル内容を描く
//!
//! 文字列を構造に分ける解析と、GPUI の要素にする描画を分けてある（解析はテストできる）。
//! 1 行ごとに要素を作るのは差分と行番号付きの表示だけで、ほかは 1 つの `StyledText` に
//! 色や太字の範囲 (`Spans`) を付けて描く。

use std::borrow::Cow;
use std::ops::Range;

use gpui::{
    Div, ElementId, FontStyle, FontWeight, HighlightStyle, Stateful, StrikethroughStyle, StyledText, UnderlineStyle,
    div, prelude::*, px,
};

use crate::theme::{self, c};
pub use crate::patch::{FilePatch, is_patch, parse_patch};

mod syntax;
mod diff_rows;

/// 等幅フォント
pub const MONO: &str = "Menlo";
/// コードや出力の箱の地の色と文字の色
pub const CODE_BG: u32 = 0x15171c;
pub const CODE_TEXT: u32 = 0xc5c9d1;
const KEY: u32 = 0x8ab4f8;
const ADD: u32 = 0x7fd18b;
const DEL: u32 = 0xf07c7c;
const HUNK: u32 = 0x6fa8dc;
const FILE: u32 = 0xe0b341;
const INLINE_CODE: u32 = 0xe6c07b;

/// 文字列中の範囲ごとの装飾。範囲は昇順で重ならない
pub type Spans = Vec<(Range<usize>, HighlightStyle)>;

fn color(hex: u32) -> HighlightStyle {
    HighlightStyle { color: Some(c(hex)), ..Default::default() }
}

fn styled(text: String, spans: Spans) -> StyledText {
    StyledText::new(text).with_highlights(spans)
}

/// 先頭 `max` 行。切り詰めたら残りの行数も返す
pub fn head_lines(s: &str, max: usize) -> (&str, usize) {
    let mut ends = s.match_indices('\n').map(|(i, _)| i);
    match ends.nth(max.saturating_sub(1)) {
        Some(i) if max > 0 => {
            let rest = s[i + 1..].lines().count();
            if rest == 0 { (s, 0) } else { (&s[..i], rest) }
        }
        _ => (s, 0),
    }
}

// ---------------------------------------------------------------- ANSI

/// 暗い背景で読めるよう明るめにした 16 色（通常 8 色 + 明るい 8 色）
const ANSI16: [u32; 16] = [
    0x6b7280, 0xf07c7c, 0x7fd18b, 0xe0b341, 0x6fa8dc, 0xc678dd, 0x56b6c2, 0xd7dae0, 0x8b93a1, 0xff9b9b, 0x9be3a6,
    0xf2cf6b, 0x8cbcf0, 0xd99cf0, 0x7dd6e0, 0xffffff,
];

fn ansi256(n: u16) -> u32 {
    match n {
        0..=15 => ANSI16[n as usize],
        16..=231 => {
            let n = (n - 16) as u32;
            let v = |x: u32| if x == 0 { 0 } else { 55 + x * 40 };
            (v(n / 36) << 16) | (v(n / 6 % 6) << 8) | v(n % 6)
        }
        _ => {
            let g = 8 + (n.min(255) - 232) as u32 * 10;
            (g << 16) | (g << 8) | g
        }
    }
}

/// SGR（文字の装飾）の状態
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Sgr {
    fg: Option<u32>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

impl Sgr {
    fn style(self) -> Option<HighlightStyle> {
        (self != Sgr::default()).then(|| HighlightStyle {
            color: self.fg.map(c),
            font_weight: self.bold.then_some(FontWeight::BOLD),
            font_style: self.italic.then_some(FontStyle::Italic),
            underline: self.underline.then_some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false }),
            fade_out: self.dim.then_some(0.4),
            ..Default::default()
        })
    }

    fn apply(&mut self, params: &[u16]) {
        if params.is_empty() {
            *self = Sgr::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => *self = Sgr::default(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                22 => (self.bold, self.dim) = (false, false),
                23 => self.italic = false,
                24 => self.underline = false,
                n @ 30..=37 => self.fg = Some(ANSI16[(n - 30) as usize]),
                n @ 90..=97 => self.fg = Some(ANSI16[(n - 90 + 8) as usize]),
                39 => self.fg = None,
                // 拡張色 (38 = 文字, 48 = 背景)。背景色は使わない
                n @ (38 | 48) => match params.get(i + 1) {
                    Some(5) => {
                        if n == 38 {
                            self.fg = params.get(i + 2).map(|&x| ansi256(x));
                        }
                        i += 2;
                    }
                    Some(2) => {
                        if n == 38
                            && let [r, g, b] = params.get(i + 2..i + 5).unwrap_or(&[])
                        {
                            self.fg = Some(((*r as u32 & 255) << 16) | ((*g as u32 & 255) << 8) | (*b as u32 & 255));
                        }
                        i += 4;
                    }
                    _ => {}
                },
                _ => {}
            }
            i += 1;
        }
    }
}

/// 行の中の `\r`（プログレス表示の上書き）は、最後に書かれた部分だけを残す
fn overwrite_cr(s: &str) -> Cow<'_, str> {
    if !s.contains('\r') {
        return Cow::Borrowed(s);
    }
    let lines: Vec<&str> = s.split('\n').map(|l| l.trim_end_matches('\r').rsplit('\r').next().unwrap_or("")).collect();
    Cow::Owned(lines.join("\n"))
}

/// ANSI のエスケープシーケンスを取り除いた文字列と、色・太字などの範囲
pub fn ansi(s: &str) -> (String, Spans) {
    let s = overwrite_cr(s);
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut spans = Spans::new();
    let mut cur = Sgr::default();
    let mut seg = 0;
    let (mut i, mut text) = (0, 0);
    while i < b.len() {
        if b[i] != 0x1b {
            i += 1;
            continue;
        }
        out.push_str(&s[text..i]);
        let mut j = i + 1;
        match b.get(j) {
            // CSI: パラメータ (0x20〜0x3f) の後に終端 (0x40〜0x7e)。終端がなければそこで打ち切る
            Some(b'[') => {
                j += 1;
                let ps = j;
                while j < b.len() && (0x20..=0x3f).contains(&b[j]) {
                    j += 1;
                }
                match b.get(j) {
                    Some(b'm') => {
                        let params: Vec<u16> = if ps == j {
                            Vec::new()
                        } else {
                            s[ps..j].split([';', ':']).map(|p| p.parse().unwrap_or(0)).collect()
                        };
                        let mut next = cur;
                        next.apply(&params);
                        if next != cur {
                            if let Some(st) = cur.style().filter(|_| out.len() > seg) {
                                spans.push((seg..out.len(), st));
                            }
                            cur = next;
                            seg = out.len();
                        }
                        j += 1;
                    }
                    Some(0x40..=0x7e) => j += 1,
                    _ => {}
                }
            }
            // OSC（ハイパーリンクやタイトル）は BEL か ESC \ まで
            Some(b']') => {
                while j < b.len() && b[j] != 0x07 && !(b[j] == 0x1b && b.get(j + 1) == Some(&b'\\')) {
                    j += 1;
                }
                j = (j + if b.get(j) == Some(&0x1b) { 2 } else { 1 }).min(b.len());
            }
            // 文字集合の指定（tput sgr0 が出す `ESC ( B` など）は 2 文字
            Some(b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/') => {
                j = (j + 1 + s.get(j + 1..).and_then(|r| r.chars().next()).map_or(0, char::len_utf8)).min(b.len())
            }
            Some(_) => j += s[j..].chars().next().map_or(1, char::len_utf8),
            None => {}
        }
        i = j;
        text = j;
    }
    out.push_str(&s[text..]);
    if let Some(st) = cur.style().filter(|_| out.len() > seg) {
        spans.push((seg..out.len(), st));
    }
    (out, spans)
}

// ---------------------------------------------------------------- 等幅のテキスト

/// `  - key: value` 形式の行なら、キー（コロンまで）の終端位置を返す
fn yaml_key(l: &str) -> Option<usize> {
    let body = l.trim_start_matches(' ');
    let body = body.strip_prefix("- ").unwrap_or(body);
    let start = l.len() - body.len();
    let n = body.bytes().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')).count();
    if n == 0 || !body.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    match &body.as_bytes()[n..] {
        [b':'] | [b':', b' ', ..] => Some(start + n + 1),
        _ => None,
    }
}

/// 等幅で出すテキストの装飾。ANSI の色があればそれを使い、なければ行の形から決める
///
/// - YAML 風の引数: キーを色分け
/// - `$ コマンド`: プロンプトを強調
/// - Codex の exec の見出し（`#0 名前 ← ツール`、`── #0 [exit 0 · 0.1s]`）や `[exit 1 · 0.2s]`: 太字・控えめな色
/// - 計画の `✓ ▶ ○`: 状態の色
/// - パッチ（`*** Begin Patch` 〜 `*** End Patch`）: 追加・削除の色
pub fn mono_spans(text: &str) -> (String, Spans) {
    if text.contains('\x1b') {
        return ansi(text);
    }
    let text = overwrite_cr(text).into_owned();
    let yaml = text.lines().take(3).any(|l| yaml_key(l).is_some());
    let mut spans = Spans::new();
    let mut patch = false;
    let mut at = 0;
    for (n, l) in text.split('\n').enumerate() {
        let r = at..at + l.len();
        at = r.end + 1;
        if l.is_empty() {
            continue;
        }
        if l.starts_with("*** Begin Patch") || l.starts_with("*** Update File: ") || l.starts_with("*** Add File: ") {
            patch = true;
        }
        if patch {
            let col = match l.as_bytes()[0] {
                _ if l.starts_with("***") => Some(FILE),
                b'+' => Some(ADD),
                b'-' => Some(DEL),
                b'@' => Some(HUNK),
                _ => None,
            };
            if let Some(col) = col {
                spans.push((r, color(col)));
            }
            if l.starts_with("*** End Patch") {
                patch = false;
            }
            continue;
        }
        let bold = |hex| HighlightStyle { font_weight: Some(FontWeight::BOLD), ..color(hex) };
        // Codex の出力の先頭の `[exit 1 · 0.2s]` と、exec の呼び出しごとの `── #0 [exit 0 · 0.1s]`
        let meta = l.starts_with("── ") || (n == 0 && l.starts_with('[') && l.ends_with(']'));
        // exec の呼び出しの見出し `#0 名前 ← ツール`
        let call = l.starts_with('#') && (l.contains(" ← ") || l[1..].starts_with(|c: char| c.is_ascii_digit()));
        if l.starts_with("$ ") {
            spans.push((r.start..r.start + 1, bold(theme::ACCENT)));
            spans.push((r.start + 2..r.end, color(theme::TEXT)));
        } else if meta {
            spans.push((r, bold(theme::MUTED)));
        } else if call {
            spans.push((r, bold(theme::TEXT)));
        } else if let Some(mark) = ["✓ ", "▶ ", "○ "].iter().find(|m| l.starts_with(**m)) {
            let col = match *mark {
                "✓ " => theme::OK,
                "▶ " => theme::ACCENT,
                _ => theme::SUBTLE,
            };
            spans.push((r.start..r.start + mark.len(), color(col)));
        } else if yaml && let Some(k) = yaml_key(l) {
            spans.push((r.start..r.start + k, color(KEY)));
        }
    }
    (text, spans)
}

/// 本文の折り返し。文章は幅に合わせ、コードは元の行を保つ
pub enum TextFlow {
    Wrap,
    Scroll,
}

/// 元の行を保ったまま横にスクロールする領域。子は内容の幅で配置する
pub fn horizontal_scroll(id: impl Into<ElementId>) -> Stateful<Div> {
    let mut scroll = div().id(id).min_w_0().overflow_x_scroll().whitespace_nowrap().flex();
    // 通常の縦スクロールは外側のイベント一覧へ渡す
    scroll.style().restrict_scroll_to_axis = Some(true);
    scroll
}

/// 等幅のテキストの箱（コード・ツールの入力・結果）
pub fn mono_box(id: impl Into<ElementId>, text: &str, flow: TextFlow) -> Div {
    let (t, spans) = mono_spans(text);
    let text = styled(if t.is_empty() { " ".into() } else { t }, spans);
    let body = match flow {
        TextFlow::Wrap => div().min_w_0().whitespace_normal().child(text).into_any_element(),
        TextFlow::Scroll => horizontal_scroll(id)
            .child(div().flex_none().min_w_full().child(text)).into_any_element(),
    };
    code_frame().child(body)
}

/// コードの箱の外枠
pub fn code_frame() -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .px_2()
        .py_1p5()
        .rounded_md()
        .bg(c(CODE_BG))
        .text_xs()
        .text_color(c(CODE_TEXT))
        .font_family(MONO)
}

// ---------------------------------------------------------------- 行番号付きのファイル内容

/// `cat -n` 形式（行番号 + タブ / →）の行。大半の行がこの形なら (行番号, 本文) に分ける
pub fn numbered(s: &str) -> Option<Vec<(Option<u32>, &str)>> {
    fn parse(l: &str) -> Option<(u32, &str)> {
        let t = l.trim_start_matches(' ');
        let n = t.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > 7 {
            return None;
        }
        let rest = &t[n..];
        let body = rest.strip_prefix('\t').or_else(|| rest.strip_prefix('→'))?;
        Some((t[..n].parse().ok()?, body))
    }
    let lines: Vec<&str> = s.lines().collect();
    let non_empty = lines.iter().filter(|l| !l.trim().is_empty()).count();
    let hits = lines.iter().filter(|l| parse(l).is_some()).count();
    if hits < 3 || hits * 10 < non_empty * 7 {
        return None;
    }
    Some(lines.into_iter().map(|l| parse(l).map_or((None, l), |(n, b)| (Some(n), b))).collect())
}

/// 行番号付きの表示
pub fn numbered_view(lines: &[(Option<u32>, &str)]) -> Div {
    let width = lines.iter().filter_map(|l| l.0).max().unwrap_or(0).to_string().len().max(2) as f32 * 7.5 + 6.0;
    let rows = div().flex_none().min_w_full().flex().flex_col().children(lines.iter().map(|&(n, body)| {
        div()
            .flex()
            .min_w_0()
            .child(
                div()
                    .w(px(width))
                    .flex_none()
                    .pr_2()
                    .flex()
                    .justify_end()
                    .text_color(c(theme::SUBTLE))
                    .child(n.map(|n| n.to_string()).unwrap_or_default()),
            )
            .child(div().flex_1().min_w_0().pr_2().child(if body.is_empty() {
                " ".to_string()
            } else {
                body.to_string()
            }))
    }));
    code_frame().px_0().child(horizontal_scroll("numbered").child(rows))
}

// ---------------------------------------------------------------- 差分

/// パスを作業ディレクトリからの相対にする
pub fn rel_path<'a>(path: &'a str, cwd: &str) -> &'a str {
    path.strip_prefix(cwd).and_then(|p| p.strip_prefix('/')).filter(|p| !p.is_empty()).unwrap_or(path)
}

/// 差分の表示。`limit` 行を超える分は省き、省いた行数を返す
pub fn diff_view<'a>(files: impl IntoIterator<Item = &'a FilePatch>, cwd: &str, limit: usize) -> (Div, usize) {
    let mut col = div().w_full().min_w_0().flex().flex_col().gap_2();
    let mut budget = limit;
    let mut hidden = 0;
    for (i, f) in files.into_iter().enumerate() {
        let numbers = f.rows.iter().any(|r| r.old.is_some() || r.new.is_some());
        let digits = f.rows.iter().filter_map(|r| r.old.max(r.new)).max().unwrap_or(0).to_string().len().max(2);
        let gutter = digits as f32 * 7.5 + 8.0;
        let path = rel_path(&f.path, cwd);
        let (dir, base) = match path.rsplit_once('/') {
            Some((d, b)) => (format!("{d}/"), b.to_string()),
            None => (String::new(), path.to_string()),
        };
        let op_color = match f.op {
            "追加" => ADD,
            "削除" => DEL,
            _ => FILE,
        };
        let head = div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .bg(c(0x1f232b))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_xs()
            .child(
                div().flex_none().px_1().rounded_sm().bg(c(op_color).opacity(0.15)).text_color(c(op_color)).child(f.op),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(StyledText::new(format!("{dir}{base}")).with_highlights((!base.is_empty()).then(|| {
                        let st = HighlightStyle { font_weight: Some(FontWeight::SEMIBOLD), ..color(theme::TEXT) };
                        (dir.len()..dir.len() + base.len(), st)
                    })))
                    .text_color(c(theme::MUTED)),
            )
            .when(f.added > 0, |d| d.child(div().flex_none().text_color(c(ADD)).child(format!("+{}", f.added))))
            .when(f.removed > 0, |d| d.child(div().flex_none().text_color(c(DEL)).child(format!("−{}", f.removed))));
        let shown = f.rows.len().min(budget);
        hidden += f.rows.len() - shown;
        budget -= shown;
        let spans = syntax::cached_diff_spans(f, shown);
        let rows = diff_rows::rows(&f.rows[..shown], spans, gutter, numbers);
        col = col.child(
            div()
                .w_full()
                .min_w_0()
                .rounded_md()
                .overflow_hidden()
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(CODE_BG))
                .child(head)
                .child(horizontal_scroll(ElementId::NamedInteger("diff".into(), i as u64)).w_full().child(
                    rows,
                )),
        );
    }
    (col, hidden)
}

// ---------------------------------------------------------------- Markdown

/// インラインの装飾の状態
#[derive(Clone, Copy, Default, PartialEq)]
struct Inline {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

impl Inline {
    fn style(self) -> Option<HighlightStyle> {
        (self != Inline::default()).then(|| HighlightStyle {
            color: if self.code {
                Some(c(INLINE_CODE))
            } else if self.link {
                Some(c(theme::ACCENT))
            } else {
                None
            },
            background_color: self.code.then(|| c(0x2b303b)),
            font_weight: self.bold.then_some(FontWeight::BOLD),
            font_style: self.italic.then_some(FontStyle::Italic),
            underline: self.link.then_some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false }),
            strikethrough: self.strike.then_some(StrikethroughStyle { thickness: px(1.0), color: None }),
            fade_out: None,
        })
    }
}

/// インラインの Markdown（`code`・**太字**・*斜体*・~~取り消し~~・[リンク](URL)）を、
/// 記号を除いた文字列と装飾の範囲にする。閉じていない記号はそのまま残す
pub fn inline(s: &str) -> (String, Spans) {
    let mut out = String::with_capacity(s.len());
    let mut spans = Spans::new();
    let mut cur = Inline::default();
    let mut seg = 0;
    let mut i = 0;
    // 装飾を切り替える（直前までの範囲を確定する）
    let switch = |next: Inline, cur: &mut Inline, seg: &mut usize, out: &String, spans: &mut Spans| {
        if next != *cur {
            if let Some(st) = cur.style().filter(|_| out.len() > *seg) {
                spans.push((*seg..out.len(), st));
            }
            *cur = next;
            *seg = out.len();
        }
    };
    while i < s.len() {
        let rest = &s[i..];
        // 装飾記号までの通常文はまとめてコピーする。ASCII記号の位置はUTF-8境界になる。
        if !matches!(rest.as_bytes()[0], b'\\' | b'`' | b'*' | b'~' | b'[') {
            let end = rest.bytes().position(|b| matches!(b, b'\\' | b'`' | b'*' | b'~' | b'['))
                .unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            i += end;
            continue;
        }
        let prev = s[..i].chars().next_back();
        if let Some(ch) = rest.strip_prefix('\\').and_then(|r| r.chars().next()).filter(char::is_ascii_punctuation) {
            out.push(ch);
            i += 2;
            continue;
        }
        if rest.starts_with('`') {
            let n = rest.bytes().take_while(|&b| b == b'`').count();
            let fence = &rest[..n];
            if let Some(k) = rest[n..].find(fence) {
                let code = &rest[n..n + k];
                let code = if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') {
                    &code[1..code.len() - 1]
                } else {
                    code
                };
                switch(Inline { code: true, ..cur }, &mut cur, &mut seg, &out, &mut spans);
                out.push_str(code);
                switch(Inline { code: false, ..cur }, &mut cur, &mut seg, &out, &mut spans);
                i += n + k + n;
                continue;
            }
        }
        // 閉じる記号が後ろにあるときだけ開く
        let opens = |after: &str, marker: &str| after.contains(marker) && !after.starts_with(char::is_whitespace);
        if let Some(after) = rest.strip_prefix("**")
            && (cur.bold || opens(after, "**"))
        {
            switch(Inline { bold: !cur.bold, ..cur }, &mut cur, &mut seg, &out, &mut spans);
            i += 2;
            continue;
        }
        if let Some(after) = rest.strip_prefix("~~")
            && (cur.strike || opens(after, "~~"))
        {
            switch(Inline { strike: !cur.strike, ..cur }, &mut cur, &mut seg, &out, &mut spans);
            i += 2;
            continue;
        }
        if let Some(after) = rest.strip_prefix('*') {
            let closes = |a: &str| a.find('*').is_some_and(|k| k > 0 && !a[..k].ends_with(char::is_whitespace));
            let open =
                !cur.italic && !after.starts_with(char::is_whitespace) && !after.starts_with('*') && closes(after);
            let close = cur.italic && prev.is_some_and(|p| !p.is_whitespace());
            if open || close {
                switch(Inline { italic: !cur.italic, ..cur }, &mut cur, &mut seg, &out, &mut spans);
                i += 1;
                continue;
            }
        }
        if rest.starts_with('[')
            && let Some(mid) = rest.find("](")
            && !rest[1..mid].contains(['\n', ']'])
            && let Some(end) = rest[mid + 2..].find(')')
            && !rest[mid + 2..mid + 2 + end].contains(char::is_whitespace)
        {
            switch(Inline { link: true, ..cur }, &mut cur, &mut seg, &out, &mut spans);
            out.push_str(&rest[1..mid]);
            switch(Inline { link: false, ..cur }, &mut cur, &mut seg, &out, &mut spans);
            i += mid + 2 + end + 1;
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    if let Some(st) = cur.style().filter(|_| out.len() > seg) {
        spans.push((seg..out.len(), st));
    }
    (out, spans)
}

/// リストの印
#[derive(Clone, PartialEq, Debug)]
pub enum Marker {
    Bullet,
    Number(String),
    /// チェックボックス（済みなら true）
    Task(bool),
}

/// Markdown のブロック
#[derive(Clone, PartialEq, Debug)]
pub enum Block {
    Heading(usize, String),
    Para(String),
    Code(String, String),
    Item {
        depth: usize,
        marker: Marker,
        text: String,
    },
    Quote(String),
    Rule,
    /// 行ごとのセル。先頭の行が見出し
    Table(Vec<Vec<String>>),
}

/// リストの行なら (印, 本文)
fn list_item(t: &str) -> Option<(Marker, &str)> {
    let (marker, rest) = if let Some(r) = t.strip_prefix(['-', '*', '+']) {
        (Marker::Bullet, r)
    } else {
        let n = t.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > 3 {
            return None;
        }
        let r = t[n..].strip_prefix(['.', ')'])?;
        (Marker::Number(t[..n + 1].to_string()), r)
    };
    let body = rest.strip_prefix(' ')?;
    if marker == Marker::Bullet {
        for (m, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(b) = body.strip_prefix(m) {
                return Some((Marker::Task(done), b));
            }
        }
    }
    Some((marker, body.trim_start()))
}

fn is_rule(t: &str) -> bool {
    let mut chars = t.chars().filter(|c| !c.is_whitespace());
    let Some(first @ ('-' | '*' | '_')) = chars.next() else { return false };
    let mut count = 1;
    for ch in chars {
        if ch != first { return false; }
        count += 1;
    }
    count >= 3
}

/// 行頭の ASCII の空白を除く（全角の空白は本文として残す）
fn trim_indent(l: &str) -> &str {
    l.trim_start_matches([' ', '\t'])
}

fn is_table_sep(t: &str) -> bool {
    t.contains('-') && t.contains('|') && t.chars().all(|c| matches!(c, '|' | ':' | '-' | ' '))
}

fn table_cells(t: &str) -> Vec<String> {
    let t = t.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Markdown をブロックに分ける（見出し・段落・コード・リスト・引用・区切り線・表）
pub fn blocks(s: &str) -> Vec<Block> {
    let lines: Vec<&str> = s.lines().collect();
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(para.join("\n")));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = trim_indent(line);
        let indent = line.len() - t.len();
        i += 1;
        if let Some(fence) = ["```", "~~~"].into_iter().find(|f| t.starts_with(f)) {
            flush(&mut para, &mut out);
            let lang = t.trim_start_matches(fence.chars().next().unwrap_or('`')).trim().to_string();
            let mut code = Vec::new();
            while i < lines.len() && !trim_indent(lines[i]).starts_with(fence) {
                let l = lines[i];
                // 囲みの字下げの分だけ戻す
                let strip = l.len() - trim_indent(l).len();
                code.push(&l[strip.min(indent)..]);
                i += 1;
            }
            i += 1;
            out.push(Block::Code(lang, code.join("\n")));
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut out);
            continue;
        }
        let hashes = t.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && (t.len() == hashes || t.as_bytes()[hashes] == b' ') {
            flush(&mut para, &mut out);
            out.push(Block::Heading(hashes, t[hashes..].trim().trim_end_matches('#').trim().to_string()));
            continue;
        }
        if indent < 4 && is_rule(t) {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
            continue;
        }
        if t.starts_with('|') && lines.get(i).is_some_and(|n| is_table_sep(n.trim())) {
            flush(&mut para, &mut out);
            let mut rows = vec![table_cells(t)];
            i += 1;
            while i < lines.len() && trim_indent(lines[i]).starts_with('|') {
                rows.push(table_cells(lines[i]));
                i += 1;
            }
            out.push(Block::Table(rows));
            continue;
        }
        if let Some(q) = t.strip_prefix('>') {
            flush(&mut para, &mut out);
            let mut text = vec![q.strip_prefix(' ').unwrap_or(q)];
            while let Some(q) = lines.get(i).and_then(|l| trim_indent(l).strip_prefix('>')) {
                text.push(q.strip_prefix(' ').unwrap_or(q));
                i += 1;
            }
            out.push(Block::Quote(text.join("\n")));
            continue;
        }
        if let Some((marker, body)) = list_item(t) {
            flush(&mut para, &mut out);
            let mut text = body.to_string();
            // 字下げした続きの行
            while let Some(l) = lines.get(i) {
                let lt = trim_indent(l);
                if lt.is_empty() || l.len() - lt.len() <= indent || list_item(lt).is_some() || lt.starts_with("```") {
                    break;
                }
                text.push('\n');
                text.push_str(lt);
                i += 1;
            }
            out.push(Block::Item { depth: indent / 2, marker, text });
            continue;
        }
        para.push(t);
    }
    flush(&mut para, &mut out);
    out
}

fn inline_text(s: &str) -> StyledText {
    let (t, spans) = inline(s);
    styled(t, spans)
}

/// Markdown の表示。`color` は本文の文字の色
pub fn markdown(s: &str, color: u32) -> Div {
    let mut col = div().flex().flex_col().gap_1p5().min_w_0().whitespace_normal().text_color(c(color));
    for (i, b) in blocks(s).into_iter().enumerate() {
        col = col.child(match b {
            Block::Heading(level, text) => div()
                .pt(px(if level <= 2 { 6.0 } else { 2.0 }))
                .font_weight(if level <= 3 { FontWeight::BOLD } else { FontWeight::SEMIBOLD })
                .text_color(c(theme::TEXT))
                .text_size(px(match level {
                    1 => 16.0,
                    2 => 15.0,
                    _ => 14.0,
                }))
                .child(inline_text(&text)),
            Block::Para(text) => div().min_w_0().child(inline_text(&text)),
            Block::Code(lang, code) => code_frame()
                .relative()
                .border_1()
                .border_color(c(theme::BORDER))
                .child(horizontal_scroll(ElementId::NamedInteger("code".into(), i as u64)).child(
                    div().flex_none().min_w_full()
                        .child(styled(if code.is_empty() { " ".into() } else { code }, Spans::new())),
                ))
                .when(!lang.is_empty(), |d| {
                    d.child(div().absolute().top(px(2.0)).right(px(6.0)).text_color(c(theme::SUBTLE)).child(lang))
                }),
            Block::Item { depth, marker, text } => {
                let (mark, mark_color) = match &marker {
                    Marker::Bullet => ("•".to_string(), theme::MUTED),
                    Marker::Number(n) => (n.clone(), theme::MUTED),
                    Marker::Task(true) => ("☑".to_string(), theme::OK),
                    Marker::Task(false) => ("☐".to_string(), theme::MUTED),
                };
                div()
                    .flex()
                    .gap_1p5()
                    .min_w_0()
                    .pl(px(depth as f32 * 16.0))
                    .child(div().flex_none().min_w(px(12.0)).text_color(c(mark_color)).child(mark))
                    .child(div().flex_1().min_w_0().child(inline_text(&text)))
            }
            Block::Quote(text) => div()
                .pl_2()
                .border_l_2()
                .border_color(c(theme::BORDER))
                .text_color(c(theme::MUTED))
                .child(inline_text(&text)),
            Block::Rule => div().my_1().h(px(1.0)).bg(c(theme::BORDER)),
            Block::Table(rows) => {
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1);
                div()
                    .min_w_0()
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .text_xs()
                    .children(rows.into_iter().enumerate().map(|(ri, row)| {
                        div()
                            .flex()
                            .min_w_0()
                            .when(ri > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
                            .when(ri == 0, |d| d.bg(c(0x1f232b)).font_weight(FontWeight::SEMIBOLD))
                            .children((0..cols).map(|ci| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .px_1p5()
                                    .py_1()
                                    .when(ci > 0, |d| d.border_l_1().border_color(c(theme::BORDER)))
                                    .child(inline_text(row.get(ci).map_or("", String::as_str)))
                            }))
                    }))
            }
        });
    }
    col
}


fn diff_row(r: &crate::patch::DiffRow, spans: &Spans, gutter: f32, numbers: bool) -> (Div, gpui::TextLayout) {
    let (bg, fg, mark) = match r.kind {
        '+' => (Some(c(ADD).opacity(0.10)), ADD, "+"),
        '-' => (Some(c(DEL).opacity(0.10)), DEL, "−"),
        '@' => (Some(c(HUNK).opacity(0.08)), HUNK, ""),
        _ => (None, CODE_TEXT, ""),
    };
    let num = |n: Option<u32>| {
        div()
            .w(px(gutter))
            .flex_none()
            .pr_1p5()
            .flex()
            .justify_end()
            .text_color(c(theme::SUBTLE))
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    let text = styled(
        if r.text.is_empty() {
            " ".to_string()
        } else {
            r.text.clone()
        },
        spans.clone(),
    );
    let layout = text.layout().clone();
    let row = div()
        .flex()
        .flex_none()
        .min_w_0()
        .when_some(bg, |d, bg| d.bg(bg))
        .when(numbers, |d| d.child(num(r.old)).child(num(r.new)))
        .child(
            div()
                .w(px(14.0))
                .flex_none()
                .flex()
                .justify_center()
                .text_color(c(fg))
                .child(mark),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pr_2()
                .text_color(c(CODE_TEXT))
                .when(r.kind == '@', |d| d.text_color(c(theme::SUBTLE)))
                .child(text),
        );
    (row, layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(spans: &Spans) -> Vec<Range<usize>> {
        spans.iter().map(|(r, _)| r.clone()).collect()
    }

    #[test]
    fn ansi_strips_and_colors() {
        let (t, s) = ansi("\x1b[31mred\x1b[0m plain \x1b[1;32mok\x1b[m\x1b[K");
        assert_eq!(t, "red plain ok");
        assert_eq!(ranges(&s), [0..3, 10..12]);
        assert_eq!(s[0].1.color, Some(c(ANSI16[1])));
        assert_eq!(s[1].1.font_weight, Some(FontWeight::BOLD));
        // OSC のハイパーリンクと 256 色
        let (t, s) = ansi("\x1b]8;;http://x\x07link\x1b]8;;\x07 \x1b[38;5;196mX\x1b[39m");
        assert_eq!(t, "link X");
        assert_eq!(ranges(&s).as_slice(), std::slice::from_ref(&(5..6)));
        // 進捗表示の上書き
        assert_eq!(ansi("10%\r50%\r100%\ndone").0, "100%\ndone");
        // 文字集合の指定（`ESC ( B`）
        assert_eq!(ansi("\x1b[31m-\x1b(B\x1b[m x").0, "- x");
        assert_eq!(ansi("a\x1b(").0, "a");
        // 終端のない CSI は後ろの行を飲み込まない
        assert_eq!(ansi("a\x1b[12\nb").0, "a\nb");
        // 途中で切れたシーケンスや非 ASCII の直前の ESC でも落ちない
        assert_eq!(ansi("日本\x1b語\x1b[3").0, "日本");
    }

    #[test]
    fn inline_markdown() {
        let (t, s) = inline("a **b** `c` *d* ~~e~~ [f](http://g)");
        assert_eq!(t, "a b c d e f");
        assert_eq!(ranges(&s), [2..3, 4..5, 6..7, 8..9, 10..11]);
        // 閉じていない記号や掛け算はそのまま
        assert_eq!(inline("2 * 3 = 6, **x").0, "2 * 3 = 6, **x");
        assert_eq!(inline("\\*lit\\*").0, "*lit*");
        assert_eq!(inline("``a ` b``").0, "a ` b");
        assert_eq!(inline("日本**語**").1.len(), 1);
    }

    #[test]
    fn markdown_plain_runs_preserve_utf8_escapes_and_style_boundaries() {
        let (text, spans) = inline("日本🙂**太字**の文\\*と`code`後 [リンク](url)!");
        assert_eq!(text, "日本🙂太字の文*とcode後 リンク!");
        assert_eq!(ranges(&spans), [10..16, 26..30, 34..43]);
        assert_eq!(spans[0].1.font_weight, Some(FontWeight::BOLD));
        assert_eq!(spans[1].1.color, Some(c(INLINE_CODE)));
        assert!(spans[2].1.underline.is_some());
        let plain = "日本🙂 ] ~ \\日本 `閉じない";
        assert_eq!(inline(plain), (plain.to_string(), Vec::new()));
        assert_eq!(inline("*日本🙂 文*後").0, "日本🙂 文後");
    }

    #[test]
    fn markdown_rules_keep_unicode_whitespace_and_reject_mixed_marks() {
        for line in ["- \t-　-", "*\u{2003}* *", "___", "　- - -　"] {
            assert!(is_rule(line), "{line:?}");
        }
        for line in ["", "  ", "--", "- * -", "日本語 ---", "--- x", "----\u{200b}"] {
            assert!(!is_rule(line), "{line:?}");
        }
        assert_eq!(blocks("本文\n- \t-　-\n続き"), [
            Block::Para("本文".into()), Block::Rule, Block::Para("続き".into()),
        ]);
    }

    #[test]
    fn markdown_blocks() {
        let md = "# 見出し\n本文\n続き\n\n- a\n  続く\n- [x] 済み\n1. 番号\n\n```rust\nfn x() {}\n```\n> 引用\n---\n| a | b |\n|---|---|\n| 1 | 2 |";
        assert_eq!(
            blocks(md),
            [
                Block::Heading(1, "見出し".into()),
                Block::Para("本文\n続き".into()),
                Block::Item { depth: 0, marker: Marker::Bullet, text: "a\n続く".into() },
                Block::Item { depth: 0, marker: Marker::Task(true), text: "済み".into() },
                Block::Item { depth: 0, marker: Marker::Number("1.".into()), text: "番号".into() },
                Block::Code("rust".into(), "fn x() {}".into()),
                Block::Quote("引用".into()),
                Block::Rule,
                Block::Table(vec![vec!["a".into(), "b".into()], vec!["1".into(), "2".into()]]),
            ]
        );
        // 段落の直後のリスト
        assert_eq!(blocks("以下:\n- x").len(), 2);
    }

    #[test]
    fn numbered_lines() {
        let n = numbered("     1\tfn a() {\n     2\t}\n     3\t\n").unwrap();
        assert_eq!(n, [(Some(1), "fn a() {"), (Some(2), "}"), (Some(3), "")]);
        assert!(numbered("a\nb\nc\n1\tx").is_none());
        assert_eq!(numbered("10→a\n11→b\n12→c").unwrap()[0], (Some(10), "a"));
    }

    #[test]
    fn mono_decorations() {
        let (t, s) = mono_spans("$ cargo test\n\ntimeout: 100");
        assert_eq!(t, "$ cargo test\n\ntimeout: 100");
        assert_eq!(ranges(&s), [0..1, 2..12, 14..22]);
        let (_, s) = mono_spans("#0 build ← exec_command\n$ make");
        assert_eq!(ranges(&s)[0], 0..25);
        assert_eq!(head_lines("a\nb\nc\nd", 2), ("a\nb", 2));
        assert_eq!(head_lines("a\nb", 5), ("a\nb", 0));
        assert_eq!(head_lines("a\nb\n", 2), ("a\nb\n", 0));
    }
}
