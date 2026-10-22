//! Unicode の文字数を保つ省略表示と、空白をまとめた一行表示。

/// 文字の途中で分割せず、上限までの借用文字列と省略の有無を返す。
pub(crate) fn clip(s: &str, max: usize) -> (&str, bool) {
    match s.char_indices().nth(max) {
        Some((end, _)) => (&s[..end], true),
        None => (s, false),
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    let (prefix, truncated) = clip(s, max);
    if !truncated {
        return prefix.to_owned();
    }
    let mut out = String::with_capacity(prefix.len() + '…'.len_utf8());
    out.push_str(prefix);
    out.push('…');
    out
}

pub fn one_line(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max.saturating_mul(4).saturating_add(3)));
    let mut count = 0;
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space |= !out.is_empty();
            continue;
        }
        if space {
            if count == max {
                out.push('…');
                return out;
            }
            out.push(' ');
            count += 1;
            space = false;
        }
        if count == max {
            out.push('…');
            return out;
        }
        out.push(c);
        count += 1;
    }
    out
}
