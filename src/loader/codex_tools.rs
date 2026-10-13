//! Codex のツール入力・出力と、コードモードの JS 呼び出しを表示用に整える。

use std::borrow::Cow;

use serde_json::Value;

use crate::model::{Status, one_line, truncate};

use super::MAX_TEXT;
use super::format::{checklist, plan_summary, pretty, summarize_input, text_of};

/// ツール出力の本文。新しい形式では `[{type: input_text, text}, ...]` に分かれている
pub(super) fn codex_output_text(o: Option<&Value>) -> Cow<'_, str> {
    match o {
        Some(Value::String(s)) => Cow::Borrowed(s),
        Some(Value::Array(items)) => {
            let mut parts = items
                .iter()
                .filter_map(|i| i.get("text").and_then(|t| t.as_str()));
            let Some(first) = parts.next() else {
                return Cow::Borrowed("");
            };
            match parts.next() {
                None => Cow::Borrowed(first),
                Some(second) => Cow::Owned([first, second].into_iter().chain(parts).collect()),
            }
        }
        Some(o) => o
            .get("content")
            .map(text_of)
            .unwrap_or_else(|| Cow::Owned(o.to_string())),
        None => Cow::Borrowed(""),
    }
}

/// exec の入力として保存する JS の目印。詳細表示時に `exec_detail` で整形する
pub const JS_MARK: &str = "\u{1}exec\n";
/// 保存する JS の上限（切れると構文エラーになり、静的解析にフォールバックする）
const MAX_JS: usize = 50_000;

/// 詳細表示用の exec の整形。モック実行で呼び出しが取れればそれを、取れなければ静的解析の結果を使う
pub fn exec_detail(js: &str) -> String {
    if let Some(calls) = crate::jsmock::run(js) {
        let args: Vec<(&str, Value)> = calls
            .iter()
            .map(|c| {
                let v = if c.args.is_object() {
                    c.args.clone()
                } else {
                    serde_json::json!({ "input": c.args })
                };
                (c.name.as_str(), v)
            })
            .collect();
        let items: Vec<Item> = args
            .iter()
            .zip(&calls)
            .map(|((n, v), c)| Item {
                name: n,
                label: c
                    .label
                    .clone()
                    .or_else(|| c.at.and_then(|at| label_near(js, at))),
                args: Box::new(JsonArgs(v)),
                raw: pretty(v),
            })
            .collect();
        let body = format_calls(&items);
        return truncate(
            &format!("（JS をモック実行して推定: {} 件）\n\n{body}", items.len()),
            MAX_TEXT,
        );
    }
    let calls = js_tool_calls(js);
    if calls.is_empty() {
        return truncate(js, MAX_TEXT);
    }
    let items: Vec<Item> = calls
        .iter()
        .map(|(n, a)| Item {
            name: n,
            label: call_label(js, n),
            args: Box::new(JsArgs(a)),
            raw: a.trim_matches(|c| c == '(' || c == ')').to_string(),
        })
        .collect();
    if items.len() == 1 && codex_format(items[0].name, &*items[0].args).1.is_empty() {
        return truncate(js, MAX_TEXT);
    }
    truncate(&format_calls(&items), MAX_TEXT)
}

/// 詳細表示用の 1 呼び出し
struct Item<'a> {
    name: &'a str,
    /// 呼び出しに付いた名前（代入先の変数・オブジェクトのキーなど）
    label: Option<String>,
    args: Box<dyn Args + 'a>,
    /// 整形できないときの表示
    raw: String,
}

fn format_calls(items: &[Item]) -> String {
    let body = |it: &Item| {
        let d = codex_format(it.name, &*it.args).1;
        if d.is_empty() { it.raw.clone() } else { d }
    };
    if let [it] = items {
        return match &it.label {
            Some(l) => format!("{l} ← {}\n{}", it.name, body(it)),
            None => body(it),
        };
    }
    let mut detail = String::new();
    for (i, it) in items.iter().enumerate() {
        let head = match &it.label {
            Some(l) => format!("#{i} {l} ← {}", it.name),
            None => format!("#{i} {}", it.name),
        };
        detail.push_str(&format!("{head}\n{}\n\n", body(it)));
    }
    detail.trim_end().to_string()
}

/// 静的解析で見つけた呼び出し（`js_tool_calls` が返したツール名のスライス）の名前
fn call_label(js: &str, name: &str) -> Option<String> {
    let at = (name.as_ptr() as usize)
        .checked_sub(js.as_ptr() as usize)?
        .checked_sub(6)?;
    label_before(js, at)
}

/// モック実行で得た位置（`tools.xxx(` の周辺）から呼び出しの名前を探す
fn label_near(js: &str, at: usize) -> Option<String> {
    let mut end = (at + 6).min(js.len());
    while !js.is_char_boundary(end) {
        end -= 1;
    }
    let line_start = js[..at.min(end)].rfind('\n').map_or(0, |i| i + 1);
    let i = js[line_start..end].rfind("tools.")? + line_start;
    label_before(js, i)
}

/// `tools.` の直前を見て、呼び出しに付いた名前を返す
///
/// - `const result = await tools.x(...)` → `result`
/// - `{ build: tools.x(...) }` → `build`
/// - `["lint", tools.x(...)]` → `lint`
fn label_before(js: &str, at: usize) -> Option<String> {
    let mut p = js[..at].trim_end();
    if let Some(q) = p.strip_suffix("await").filter(|q| !q.ends_with(is_ident)) {
        p = q.trim_end();
    }
    let ident = |s: &str| {
        let s = s.trim_end();
        let n = s.chars().rev().take_while(|&c| is_ident(c)).count();
        let id = &s[s.len() - s.chars().rev().take(n).map(char::len_utf8).sum::<usize>()..];
        (!id.is_empty() && !id.starts_with(|c: char| c.is_ascii_digit())).then(|| id.to_string())
    };
    if let Some(q) = p.strip_suffix('=') {
        if q.ends_with(['=', '!', '<', '>']) {
            return None;
        }
        return ident(q.trim_end_matches(['+', '-', '*', '/', '|', '&', '?']));
    }
    if let Some(q) = p.strip_suffix(':') {
        let q = q.trim_end();
        if let Some(&qc @ (b'"' | b'\'' | b'`')) = q.as_bytes().last() {
            let open = q[..q.len() - 1].rfind(qc as char)?;
            return Some(q[open + 1..q.len() - 1].to_string()).filter(|s| !s.is_empty());
        }
        return ident(q);
    }
    if let Some(q) = p.strip_suffix(',') {
        let q = q.trim_end();
        let &qc @ (b'"' | b'\'' | b'`') = q.as_bytes().last()? else {
            return None;
        };
        let open = q[..q.len() - 1].rfind(qc as char)?;
        if q[..open].trim_end().ends_with('[') {
            return Some(q[open + 1..q.len() - 1].to_string())
                .filter(|s| !s.is_empty() && s.len() <= 60);
        }
    }
    None
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Codex のツール呼び出しを表示用に整える: (名前, 要約, 入力表示)
///
/// コードモードの `exec` は JS で `tools.xxx({...})` を呼ぶので、中身のツールを取り出す。
pub(super) fn codex_tool(name: &str, input: &Value) -> (String, String, String) {
    if let ("exec", Value::String(js)) = (name, input) {
        // 名前と要約は静的解析で決め、詳細は表示時にモック実行で作る（exec_detail）
        let calls = js_tool_calls(js);
        let stored = format!("{JS_MARK}{}", truncate(js, MAX_JS));
        return match calls.as_slice() {
            [] => (name.into(), summarize_input(name, input), stored),
            [(inner, args)] => (
                inner.to_string(),
                codex_format(inner, &JsArgs(args)).0,
                stored,
            ),
            many => {
                let sums: Vec<String> = many
                    .iter()
                    .map(|(inner, args)| {
                        let s = codex_format(inner, &JsArgs(args)).0;
                        let s = if s.is_empty() { inner.to_string() } else { s };
                        match call_label(js, inner) {
                            Some(l) => format!("{l}: {s}"),
                            None => s,
                        }
                    })
                    .collect();
                let first = many[0].0;
                let n = if many.iter().all(|(x, _)| *x == first) {
                    first
                } else {
                    "exec"
                };
                let s = format!("[{}] {}", many.len(), sums.join(" ; "));
                (n.into(), one_line(&s, 160), stored)
            }
        };
    }
    if let ("apply_patch", Value::String(p)) = (name, input) {
        return (name.into(), patch_summary(p), truncate(p, MAX_TEXT));
    }
    let (s, d) = match input {
        Value::Object(_) => codex_format(name, &JsonArgs(input)),
        _ => (String::new(), String::new()),
    };
    let s = if s.is_empty() {
        summarize_input(name, input)
    } else {
        s
    };
    let d = if d.is_empty() { pretty(input) } else { d };
    (name.into(), s, d)
}

/// JSON / JS オブジェクトリテラルの両方から引数を取り出す
trait Args {
    fn str(&self, key: &str) -> Option<String>;
    /// 位置引数の先頭の文字列 (`tools.apply_patch("...")`)
    fn first(&self) -> Option<String> {
        None
    }
    /// update_plan の (step, status) 列
    fn plan(&self) -> Vec<(String, String)>;
}

struct JsonArgs<'a>(&'a Value);

impl Args for JsonArgs<'_> {
    fn str(&self, key: &str) -> Option<String> {
        match self.0.get(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => Some(
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            Value::Null => None,
            v => Some(v.to_string()),
        }
    }
    fn plan(&self) -> Vec<(String, String)> {
        let Some(Value::Array(a)) = self.0.get("plan") else {
            return Vec::new();
        };
        a.iter()
            .map(|p| {
                let f = |k| {
                    p.get(k)
                        .and_then(|x: &Value| x.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                (f("step"), f("status"))
            })
            .collect()
    }
}

struct JsArgs<'a>(&'a str);

impl Args for JsArgs<'_> {
    fn str(&self, key: &str) -> Option<String> {
        js_fields(self.0, key).into_iter().next()
    }
    fn first(&self) -> Option<String> {
        let s = self.0.strip_prefix('(')?.trim_start();
        let q = *s.as_bytes().first()?;
        matches!(q, b'"' | b'\'' | b'`').then(|| js_unquote(js_balanced_str(s, q)))
    }
    fn plan(&self) -> Vec<(String, String)> {
        let steps = js_fields(self.0, "step");
        let mut st = js_fields(self.0, "status").into_iter();
        steps
            .into_iter()
            .map(|s| (s, st.next().unwrap_or_default()))
            .collect()
    }
}

/// JS ソース中の `tools.name(args)` 呼び出しを列挙する
fn js_tool_calls(js: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = js;
    while let Some(i) = rest.find("tools.") {
        rest = &rest[i + 6..];
        let n = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if n == 0 || rest.as_bytes().get(n) != Some(&b'(') {
            continue;
        }
        let name = &rest[..n];
        let args = js_balanced(&rest[n..]);
        out.push((name, args));
        rest = &rest[n + args.len()..];
    }
    out
}

/// 先頭の括弧に対応する閉じ括弧までを返す（文字列リテラル内は無視）
fn js_balanced(s: &str) -> &str {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut esc = false;
    for (i, b) in s.bytes().enumerate() {
        if let Some(q) = quote {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'"' | b'\'' | b'`' => quote = Some(b),
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return &s[..=i];
                }
            }
            _ => {}
        }
    }
    s
}

/// `key: "..."` / `"key": "..."` / `key: 123` の値をすべて取り出す
fn js_fields(src: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(i) = src[from..].find(key).map(|i| i + from) {
        from = i + key.len();
        let prev = if i > 0 { b[i - 1] } else { b'{' };
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            continue;
        }
        let mut j = from;
        if b.get(j) == Some(&b'"') && prev == b'"' {
            j += 1;
        } else if prev == b'"' {
            continue;
        }
        while b.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
            j += 1;
        }
        if b.get(j) != Some(&b':') {
            continue;
        }
        j += 1;
        while b.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
            j += 1;
        }
        match b.get(j) {
            Some(&q @ (b'"' | b'\'' | b'`')) => {
                let lit = js_balanced_str(&src[j..], q);
                out.push(js_unquote(lit));
                from = j + lit.len();
            }
            Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b't' || *c == b'f' => {
                let n = src[j..]
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'-')
                    .count();
                out.push(src[j..j + n].to_string());
            }
            _ => {}
        }
    }
    out
}

fn js_balanced_str(s: &str, q: u8) -> &str {
    let mut esc = false;
    for (i, b) in s.bytes().enumerate().skip(1) {
        if esc {
            esc = false;
        } else if b == b'\\' {
            esc = true;
        } else if b == q {
            return &s[..=i];
        }
    }
    s
}

/// JS の文字列リテラルを復号する（JSON 互換のエスケープを想定）
fn js_unquote(lit: &str) -> String {
    let inner = lit.get(1..lit.len().saturating_sub(1)).unwrap_or("");
    let quoted = if lit.starts_with('"') {
        lit.to_string()
    } else {
        format!(
            "\"{}\"",
            inner
                .replace("\\'", "'")
                .replace("\\`", "`")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        )
    };
    serde_json::from_str::<String>(&quoted).unwrap_or_else(|_| inner.to_string())
}

/// ツールごとの (要約, 入力表示)。空文字は既定の表示に任せる
fn codex_format(name: &str, a: &dyn Args) -> (String, String) {
    match name {
        "exec_command" | "shell" | "local_shell" => {
            let Some(cmd) = a.str("cmd").or_else(|| a.str("command")) else {
                return Default::default();
            };
            let mut d = format!("$ {cmd}");
            if let Some(w) = a.str("workdir") {
                d.push_str(&format!("\n\ncwd: {w}"));
            }
            (one_line(&cmd, 160), truncate(&d, MAX_TEXT))
        }
        "write_stdin" => {
            let id = a.str("session_id").unwrap_or_default();
            let chars = a.str("chars").unwrap_or_default();
            let s = if chars.is_empty() {
                format!("session {id} の出力を待機")
            } else {
                format!("session {id} へ入力")
            };
            (
                s.clone(),
                if chars.is_empty() {
                    s
                } else {
                    format!("{s}\n\n{chars}")
                },
            )
        }
        "apply_patch" => {
            let Some(p) = a.first().or_else(|| a.str("input")) else {
                return Default::default();
            };
            (patch_summary(&p), truncate(&p, MAX_TEXT))
        }
        "wait" => {
            let s = format!("cell {} の完了を待機", a.str("cell_id").unwrap_or_default());
            (s.clone(), s)
        }
        "update_plan" => {
            let plan = a.plan();
            let mut d = a
                .str("explanation")
                .map(|e| format!("{e}\n\n"))
                .unwrap_or_default();
            d.push_str(&checklist(&plan));
            (plan_summary(&plan), d.trim_end().to_string())
        }
        _ => Default::default(),
    }
}

/// パッチの対象ファイルを「Update a.rs, Add b.rs」の形で並べる
fn patch_summary(p: &str) -> String {
    let files: Vec<String> = p
        .lines()
        .filter_map(|l| {
            let r = l.strip_prefix("*** ")?;
            let (op, path) = r.split_once(" File: ")?;
            let base = crate::paths::basename(path.trim());
            Some(format!("{op} {base}"))
        })
        .collect();
    one_line(&files.join(", "), 160)
}

/// コードモードの `text()` が出力した JSON 1 行を読みやすくする。2 つ目は失敗（reject・0 以外の終了コード）か
///
/// `{"i":0,"status":"fulfilled","value":{exit_code, output, ...}}`（Promise.allSettled の結果）や
/// `{"exit_code":0,"output":"..."}`（ツールの戻り値そのまま）を見出し + 本文にする。
fn codex_js_line(line: &str) -> Option<(String, bool)> {
    // text() を続けて呼ぶと、改行なしで JSON が連なる（`{...}{"i":1,...}`）
    let line = line.trim();
    let mut it = serde_json::Deserializer::from_str(line).into_iter::<Value>();
    let (mut parts, mut failed, mut end) = (Vec::new(), false, 0);
    while let Some(Ok(v)) = it.next() {
        let (text, f) = codex_js_value(&v)?;
        parts.push(text);
        failed |= f;
        end = it.byte_offset();
    }
    if parts.is_empty() {
        return None;
    }
    if !line[end..].trim().is_empty() {
        parts.push(line[end..].trim().to_string());
    }
    Some((parts.join("\n"), failed))
}

fn codex_js_value(v: &Value) -> Option<(String, bool)> {
    if !v.is_object() {
        return None;
    }
    let idx = v.get("i").map(|i| format!("#{i} "));
    let (val, rejected) = match v.get("status").and_then(|s| s.as_str()) {
        Some("rejected") => (v.get("reason")?, true),
        Some(_) => (v.get("value")?, false),
        None => (v, false),
    };
    let idx = idx.unwrap_or_default();
    if rejected {
        let r = val
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| val.to_string());
        return Some((format!("── {idx}rejected\n{r}"), true));
    }
    let out = val.get("output")?.as_str()?;
    let mut meta = Vec::new();
    let mut failed = false;
    if let Some(c) = val.get("exit_code").filter(|c| !c.is_null()) {
        failed = c.as_i64() != Some(0);
        meta.push(format!("exit {c}"));
    } else if let Some(id) = val.get("session_id").filter(|c| !c.is_null()) {
        meta.push(format!("running · session {id}"));
    }
    if let Some(w) = val.get("wall_time_seconds").and_then(|w| w.as_f64()) {
        meta.push(format!("{w:.1}s"));
    }
    Some((
        format!("── {idx}[{}]\n{}", meta.join(" · "), out.trim_end()),
        failed,
    ))
}

/// 出力の先頭に出る、失敗・中断を表す定型文
fn codex_failure(s: &str) -> Option<Status> {
    const ERRORS: &[&str] = &[
        "Script failed",
        "apply_patch verification failed",
        "failed to spawn",
        "execution error",
        "unsupported custom tool call",
    ];
    let mut end = s.len().min(200);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let head = &s[..end];
    if head.starts_with("aborted by user") || head.contains("\naborted by user") {
        Some(Status::Interrupted)
    } else if ERRORS.iter().any(|e| head.starts_with(e)) {
        Some(Status::Error)
    } else {
        None
    }
}

/// Codex のツール出力から定型ヘッダ (Wall time など) を取り除き、終了コードと所要時間を 1 行にまとめる。
/// あわせて、終了コードや定型文から呼び出しの成否を判定する
pub(super) fn codex_output(s: &str) -> (Cow<'_, str>, Status) {
    let mut status = codex_failure(s).unwrap_or(Status::Ok);
    let fail = |status: &mut Status| {
        if *status == Status::Ok {
            *status = Status::Error;
        }
    };
    // 旧形式の `{"output": "...", "metadata": {"exit_code": 0, "duration_seconds": 0.1}}`
    if s.starts_with("{\"output\"")
        && let Ok(v) = serde_json::from_str::<Value>(s)
        && let Some(out) = v.get("output").and_then(|o| o.as_str())
    {
        let mut meta = Vec::new();
        if let Some(c) = v.pointer("/metadata/exit_code").and_then(|c| c.as_i64()) {
            if c != 0 {
                fail(&mut status);
            }
            meta.push(format!("exit {c}"));
        }
        if let Some(d) = v
            .pointer("/metadata/duration_seconds")
            .and_then(|d| d.as_f64())
        {
            meta.push(format!("{d:.1}s"));
        }
        let text = if meta.is_empty() {
            out.to_string()
        } else {
            format!("[{}]\n{out}", meta.join(" · "))
        };
        return (Cow::Owned(text), status);
    }
    let Some(i) = s.find("Output:\n").filter(|&i| i < 400) else {
        return (Cow::Borrowed(s), status);
    };
    let (mut exit, mut wall) = (None, None);
    for l in s[..i].lines() {
        let l = l.trim();
        if let Some(c) = l
            .strip_prefix("Process exited with code ")
            .or_else(|| l.strip_prefix("Exit code: "))
        {
            exit = Some(c.to_string());
        } else if let Some(w) = l.strip_prefix("Wall time") {
            wall = Some(
                w.trim_start_matches(':')
                    .trim()
                    .replace(" seconds", "s")
                    .replace(" second", "s"),
            );
        } else if !(l.is_empty()
            || l.starts_with("Chunk ID")
            || l.starts_with("Original token count")
            || l.starts_with("Script ")
            || l.starts_with("Process running"))
        {
            return (Cow::Borrowed(s), status);
        }
    }
    if exit.as_deref().is_some_and(|c| c.trim() != "0") {
        fail(&mut status);
    }
    let body = &s[i + 8..];
    let lines: Vec<String> = body
        .lines()
        .map(|l| match codex_js_line(l) {
            Some((text, failed)) => {
                if failed {
                    fail(&mut status);
                }
                text
            }
            None => l.to_string(),
        })
        .collect();
    let body = &lines.join("\n");
    let meta: Vec<String> = exit
        .map(|c| format!("exit {c}"))
        .into_iter()
        .chain(wall)
        .collect();
    let text = if meta.is_empty() {
        body.to_string()
    } else {
        format!("[{}]\n{body}", meta.join(" · "))
    };
    (Cow::Owned(text), status)
}

#[cfg(test)]
mod tests {
    use super::super::format::pretty_output;
    use super::*;

    /// 保存された JS を詳細表示用に整形し、モック実行の見出しを除く
    fn detail(d: &str) -> String {
        let d = exec_detail(
            d.strip_prefix(JS_MARK)
                .expect("exec の入力は JS のまま保存する"),
        );
        d.split_once("\n\n")
            .filter(|(h, _)| h.starts_with("（JS をモック"))
            .map_or(d.clone(), |(_, b)| b.to_string())
    }

    #[test]
    fn codex_labels() {
        let js = Value::String(
            "const build = await tools.exec_command({cmd:\"make\"});\nconst r = await Promise.allSettled([\n [\"lint\", tools.exec_command({cmd:\"ls\"})],\n tools.exec_command({cmd:\"pwd\"})]);\nconst o = { test: tools.exec_command({cmd:\"t\"}) };".into(),
        );
        let (_, s, d) = codex_tool("exec", &js);
        assert_eq!(s, "[4] build: make ; lint: ls ; pwd ; test: t");
        assert_eq!(
            detail(&d),
            "#0 build ← exec_command\n$ make\n\n#1 lint ← exec_command\n$ ls\n\n#2 exec_command\n$ pwd\n\n#3 test ← exec_command\n$ t"
        );
        let js = Value::String(
            "for (const f of [\"a\"]) { const out = await tools.exec_command({cmd: `cat ${f}`}); }"
                .into(),
        );
        let (_, _, d) = codex_tool("exec", &js);
        assert_eq!(detail(&d), "out ← exec_command\n$ cat a");
        // 配列の要素の 0 番目が名前
        let js = Value::String(
            "const commands=[[\"cpu\",\"sysctl\"],[\"os\",\"sw_vers\"]];\nconst rs=await Promise.allSettled(commands.map(async ([name,cmd])=>({name,...await tools.exec_command({cmd})})));\nfor (const [n, c] of [[\"x\", \"ls\"]]) await tools.exec_command({cmd: c});\nawait tools.exec_command({cmd: \"pwd\"});".into(),
        );
        let (_, _, d) = codex_tool("exec", &js);
        assert_eq!(
            detail(&d),
            "#0 cpu ← exec_command\n$ sysctl\n\n#1 os ← exec_command\n$ sw_vers\n\n#2 x ← exec_command\n$ ls\n\n#3 exec_command\n$ pwd"
        );
    }

    #[test]
    fn codex_mock_loop() {
        let js = Value::String(
            "for (const f of [\"a\", \"b\"]) await tools.exec_command({cmd: `cat ${f}`});".into(),
        );
        let (_, _, d) = codex_tool("exec", &js);
        assert_eq!(
            detail(&d),
            "#0 exec_command\n$ cat a\n\n#1 exec_command\n$ cat b"
        );
    }

    #[test]
    fn codex_exec_js() {
        let js = Value::String(
            "const r=await tools.exec_command({cmd:\"sed -n '1,2p' a.py\",workdir:\"/w\",yield_time_ms:10000}); text(r.output);"
                .into(),
        );
        let (n, s, d) = codex_tool("exec", &js);
        assert_eq!(n, "exec_command");
        assert_eq!(s, "sed -n '1,2p' a.py");
        assert_eq!(
            detail(&d),
            "r ← exec_command\n$ sed -n '1,2p' a.py\n\ncwd: /w"
        );
    }

    #[test]
    fn codex_plan_js() {
        let js = Value::String(
            "await tools.update_plan({plan:[\n {step:\"A\",status:\"completed\"},\n {step:\"B\",status:\"in_progress\"}]})".into(),
        );
        let (n, s, d) = codex_tool("exec", &js);
        assert_eq!(n, "update_plan");
        assert_eq!(s, "1/2 B");
        assert_eq!(detail(&d), "✓ A\n▶ B");
    }

    #[test]
    fn codex_output_header() {
        assert_eq!(
            pretty(
                &serde_json::json!({"a": 1, "b": "x\ny", "c": [1, 2], "d": {"e": null}, "f": [{"g": true, "h": "i"}]})
            ),
            "a: 1\nb:\n  x\n  y\nc: [1, 2]\nd:\n  e: null\nf:\n  - g: true\n    h: i"
        );
        assert_eq!(
            pretty_output("[exit 0]\n{\"ok\":true}"),
            "[exit 0]\nok: true"
        );
        let ok = |s: &str| (Cow::<str>::Owned(s.to_string()), Status::Ok);
        let err = |s: &str| (Cow::<str>::Owned(s.to_string()), Status::Error);
        assert_eq!(
            codex_output("Script completed\nWall time 0.2 seconds\nOutput:\nhello"),
            ok("[0.2s]\nhello")
        );
        assert_eq!(
            codex_output(
                "Chunk ID: x\nWall time: 0.1 seconds\nProcess exited with code 1\nOriginal token count: 3\nOutput:\nerr"
            ),
            err("[exit 1 · 0.1s]\nerr")
        );
        assert_eq!(
            codex_output("Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess."),
            ok("[exit 0 · 0s]\nSuccess.")
        );
        assert_eq!(
            codex_output(
                "{\"output\":\"x\\n\",\"metadata\":{\"exit_code\":2,\"duration_seconds\":0.5}}"
            ),
            err("[exit 2 · 0.5s]\nx\n")
        );
        assert_eq!(
            codex_output("Script failed\nWall time 0.1 seconds\nOutput:\nboom").1,
            Status::Error
        );
        assert_eq!(
            codex_output("aborted by user after 4.9s").1,
            Status::Interrupted
        );
        assert_eq!(
            codex_output("Wall time: 573.7 seconds\naborted by user").1,
            Status::Interrupted
        );
        assert_eq!(
            codex_output("apply_patch verification failed: Failed to find expected lines").1,
            Status::Error
        );
        // ネストした呼び出しの 1 つが失敗していれば失敗
        assert_eq!(
            codex_output("Script completed\nWall time 0.2 seconds\nOutput:\n{\"i\":0,\"status\":\"fulfilled\",\"value\":{\"exit_code\":1,\"output\":\"\"}}\n{\"i\":1,\"status\":\"fulfilled\",\"value\":{\"exit_code\":0,\"output\":\"\"}}").1,
            Status::Error
        );
        assert_eq!(
            codex_output_text(Some(
                &serde_json::json!([{"type": "input_text", "text": "Script completed\nOutput:\n"}, {"type": "input_text", "text": "hi"}])
            )),
            "Script completed\nOutput:\nhi"
        );
        // 改行なしで連なった JSON
        assert_eq!(
            codex_output(
                "Script completed\nWall time 0.1 seconds\nOutput:\n{\"i\":0,\"status\":\"fulfilled\",\"value\":{\"exit_code\":0,\"output\":\"a\\n\"}}{\"i\":1,\"status\":\"fulfilled\",\"value\":{\"exit_code\":2,\"output\":\"b\"}}"
            ),
            err("[0.1s]\n── #0 [exit 0]\na\n── #1 [exit 2]\nb")
        );
    }

    #[test]
    fn codex_multi_js() {
        let js = Value::String(
            "const r=await Promise.allSettled([tools.exec_command({cmd:\"ls\"}),tools.exec_command({cmd:\"pwd\"})]);".into(),
        );
        let (n, s, d) = codex_tool("exec", &js);
        assert_eq!(n, "exec_command");
        assert_eq!(s, "[2] ls ; pwd");
        assert_eq!(
            detail(&d),
            "#0 exec_command\n$ ls\n\n#1 exec_command\n$ pwd"
        );
        let js = Value::String(
            "text(await tools.apply_patch(\"*** Begin Patch\\n*** Update File: /a/b.rs\\n@@\\n\"))"
                .into(),
        );
        let (n, s, _) = codex_tool("exec", &js);
        assert_eq!((n.as_str(), s.as_str()), ("apply_patch", "Update b.rs"));
        assert_eq!(
            codex_output(
                "Script completed\nWall time 0.2 seconds\nOutput:\n{\"i\":0,\"status\":\"fulfilled\",\"value\":{\"exit_code\":0,\"wall_time_seconds\":0.01,\"output\":\"hi\\n\"}}"
            ),
            (
                Cow::Borrowed("[0.2s]\n── #0 [exit 0 · 0.0s]\nhi"),
                Status::Ok
            )
        );
    }
}
