//! ツール入力・出力の共通整形と、Claude の編集差分・計画の表示。

use std::borrow::Cow;

use serde_json::Value;

use crate::model::{one_line, truncate};

use super::MAX_TEXT;

pub(super) fn pretty(v: &Value) -> String {
    match v {
        Value::String(s) => truncate(s, MAX_TEXT),
        other => bounded_yaml(other, MAX_TEXT),
    }
}

/// JSON を YAML 風の読みやすい行に整形する（文字列は引用符を外し、複数行はそのまま字下げする）
#[cfg(test)]
fn yaml(v: &Value) -> Vec<String> {
    yaml_lines(v, usize::MAX).collect()
}

fn yaml_lines(v: &Value, line_limit: usize) -> Box<dyn Iterator<Item = String> + '_> {
    match v {
        Value::Object(m) if !m.is_empty() => Box::new(
            m.iter()
                .flat_map(move |(k, v)| entry_lines(k, v, line_limit)),
        ),
        Value::Array(a) if !a.is_empty() => {
            if let Some(s) = inline_list(a) {
                return Box::new(std::iter::once(s));
            }
            Box::new(a.iter().flat_map(move |x| {
                let mut ls = yaml_lines(x, line_limit);
                let first = ls.next().unwrap_or_default();
                std::iter::once(format!("- {first}")).chain(ls.map(|l| format!("  {l}")))
            }))
        }
        other => Box::new(std::iter::once(scalar_prefix(other, line_limit))),
    }
}

fn entry(k: &str, v: &Value) -> Vec<String> {
    entry_lines(k, v, usize::MAX).collect()
}

fn entry_lines<'a>(
    k: &'a str,
    v: &'a Value,
    line_limit: usize,
) -> Box<dyn Iterator<Item = String> + 'a> {
    let k = text_prefix(k, line_limit);
    let nested =
        |ls: Box<dyn Iterator<Item = String> + 'a>| -> Box<dyn Iterator<Item = String> + 'a> {
            Box::new(std::iter::once(format!("{k}:")).chain(ls.map(|l| format!("  {l}"))))
        };
    match v {
        Value::String(s) if s.contains('\n') => nested(Box::new(
            s.lines()
                .map(move |line| text_prefix(line, line_limit).to_owned()),
        )),
        Value::Object(m) if !m.is_empty() => nested(yaml_lines(v, line_limit)),
        Value::Array(a) if !a.is_empty() => match inline_list(a) {
            Some(s) => Box::new(std::iter::once(format!("{k}: {s}"))),
            None => nested(yaml_lines(v, line_limit)),
        },
        other => Box::new(std::iter::once(format!(
            "{k}: {}",
            scalar_prefix(other, line_limit)
        ))),
    }
}

fn text_prefix(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        text
    } else {
        let end = text
            .char_indices()
            .nth(limit)
            .map_or(text.len(), |(i, _)| i);
        &text[..end]
    }
}

fn scalar_prefix(value: &Value, limit: usize) -> String {
    match value {
        Value::String(text) if !text.is_empty() => text_prefix(text, limit).to_owned(),
        _ => scalar(value),
    }
}

/// 表示上限の一文字先まで生成し、元の切り詰めと同じ位置に省略記号を付ける。
fn bounded_yaml(v: &Value, limit: usize) -> String {
    let mut out = String::new();
    let mut chars = 0;
    for (i, line) in yaml_lines(v, limit.saturating_add(1)).enumerate() {
        if i != 0 {
            out.push('\n');
            chars += 1;
            if chars > limit {
                return truncate(&out, limit);
            }
        }
        for c in line.chars() {
            out.push(c);
            chars += 1;
            if chars > limit {
                return truncate(&out, limit);
            }
        }
    }
    out
}

/// 短いスカラーだけの配列は 1 行にまとめる
fn inline_list(a: &[Value]) -> Option<String> {
    let mut out = String::from("[");
    let mut chars = 1;
    for (i, value) in a.iter().enumerate() {
        if value.is_object() || value.is_array() {
            return None;
        }
        if i != 0 {
            out.push_str(", ");
            chars += 2;
        }
        // 閉じ括弧を含めて80文字。長い配列や文字列を全て複製・連結せずに判定する。
        let remaining = 80usize.checked_sub(chars + 1)?;
        if let Some(text) = value.as_str()
            && (text.chars().take(remaining + 1).count() > remaining || text.contains('\n'))
        {
            return None;
        }
        let text = scalar(value);
        let count = text.chars().count();
        if count > remaining {
            return None;
        }
        chars += count;
        out.push_str(&text);
    }
    out.push(']');
    Some(out)
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) if s.is_empty() => "\"\"".into(),
        Value::String(s) => s.clone(),
        Value::Object(_) => "{}".into(),
        Value::Array(_) => "[]".into(),
        other => other.to_string(),
    }
}

/// 結果が JSON なら整形する
pub(super) fn pretty_output(s: &str) -> Cow<'_, str> {
    let t = s.trim();
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(v) = serde_json::from_str::<Value>(t)
    {
        return Cow::Owned(bounded_yaml(&v, MAX_TEXT));
    }
    // Codex の "[exit 0 · 0.1s]" 見出しの後ろが JSON の場合
    if let Some((head, body)) = s
        .split_once('\n')
        .filter(|(h, _)| h.starts_with('[') && h.ends_with(']'))
    {
        let b = pretty_output(body);
        if let Cow::Owned(b) = b
            && b != body
        {
            return Cow::Owned(format!("{head}\n{b}"));
        }
    }
    Cow::Borrowed(s)
}

/// ツール入力から一行の要約を作る
pub(super) fn summarize_input(name: &str, input: &Value) -> String {
    const KEYS: &[&str] = &[
        "description",
        "file_path",
        "path",
        "command",
        "cmd",
        "pattern",
        "url",
        "query",
        "prompt",
        "skill",
        "plan",
    ];
    if name == "TodoWrite" {
        let todos = todo_items(input);
        if !todos.is_empty() {
            return plan_summary(&todos);
        }
    }
    if let Value::Object(map) = input {
        for k in KEYS {
            if let Some(v) = map.get(*k) {
                let s = match v {
                    Value::String(s) => s.clone(),
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    other => other.to_string(),
                };
                if !s.is_empty() {
                    return one_line(&s, 160);
                }
            }
        }
        return one_line(&input.to_string(), 160);
    }
    if let Value::String(s) = input {
        // apply_patch などはファイル名だけ拾う
        if name == "apply_patch" {
            let files: Vec<_> = s
                .lines()
                .filter_map(|l| {
                    l.strip_prefix("*** ")
                        .and_then(|l| l.split_once(": "))
                        .map(|x| x.1)
                })
                .collect();
            if !files.is_empty() {
                return one_line(&files.join(", "), 160);
            }
        }
        return one_line(s, 160);
    }
    String::new()
}

pub(super) fn text_of(content: &Value) -> Cow<'_, str> {
    match content {
        Value::String(s) => Cow::Borrowed(s),
        Value::Array(items) => {
            let mut parts = items
                .iter()
                .filter_map(|i| i.get("text").and_then(|t| t.as_str()));
            let Some(first) = parts.next() else {
                return Cow::Borrowed("");
            };
            match parts.next() {
                None => Cow::Borrowed(first),
                Some(second) => Cow::Owned(
                    [first, second]
                        .into_iter()
                        .chain(parts)
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            }
        }
        _ => Cow::Borrowed(""),
    }
}

/// シェルのコマンドの表示。`$ コマンド` の後に、その他の引数を YAML 風に添える
fn shell_text(cmd: &str, rest: &str) -> String {
    let mut d = format!("$ {cmd}");
    if !rest.is_empty() {
        d.push_str("\n\n");
        d.push_str(rest);
    }
    d
}

/// `keys` 以外の引数を YAML 風に並べる
fn yaml_except(input: &Value, keys: &[&str]) -> String {
    match input {
        Value::Object(m) => m
            .iter()
            .filter(|(k, _)| !keys.contains(&k.as_str()))
            .flat_map(|(k, v)| entry(k, v))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Claude のツール入力を表示用に整える。専用の形がないものは YAML 風にする
///
/// 編集系は差分（`*** Update File: パス` + `@@` 区切りの -/+ 行）にする。結果に行番号付きの差分が
/// あれば、結果を受け取ったときに置き換える（`claude_patch`）
pub(super) fn claude_input(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str());
    let text = match name {
        "Bash" => s("command")
            .map(|cmd| shell_text(cmd, &yaml_except(input, &["command", "description"]))),
        "Edit" => match (s("file_path"), s("old_string"), s("new_string")) {
            (Some(p), Some(a), Some(b)) => {
                Some(format!("*** Update File: {p}\n{}", line_diff(a, b)))
            }
            _ => None,
        },
        "MultiEdit" => s("file_path")
            .zip(input.get("edits").and_then(|e| e.as_array()))
            .map(|(p, edits)| {
                let hunks: Vec<String> = edits
                    .iter()
                    .map(|e| {
                        let f = |k| e.get(k).and_then(|v: &Value| v.as_str()).unwrap_or("");
                        line_diff(f("old_string"), f("new_string"))
                    })
                    .collect();
                format!("*** Update File: {p}\n{}", hunks.join("\n"))
            }),
        "Write" => s("file_path")
            .zip(s("content"))
            .map(|(p, c)| added_file(p, c)),
        "TodoWrite" => {
            let todos = todo_items(input);
            (!todos.is_empty()).then(|| checklist(&todos))
        }
        "ExitPlanMode" => s("plan").map(String::from),
        // サブエージェントへの指示は Markdown で読めるよう、その他の引数を前に置いて本文をそのまま残す
        "Agent" | "Task" => s("prompt").map(|p| {
            let rest = yaml_except(input, &["prompt"]);
            if rest.is_empty() {
                p.to_string()
            } else {
                format!("{rest}\n\n{p}")
            }
        }),
        _ => None,
    };
    match text {
        Some(t) => truncate(&t, MAX_TEXT),
        None => pretty(input),
    }
}

/// 新しく作るファイルの差分（全行を追加として並べる）
fn added_file(path: &str, content: &str) -> String {
    let mut d = format!("*** Add File: {path}");
    for l in content.lines() {
        d.push_str("\n+");
        d.push_str(l);
    }
    d
}

/// Claude の Edit / Write の結果（toolUseResult）から、行番号付きの差分を作る
pub(super) fn claude_patch(r: &Value) -> Option<String> {
    let path = r.get("filePath")?.as_str()?;
    let hunks = r
        .get("structuredPatch")?
        .as_array()
        .filter(|h| !h.is_empty())?;
    let mut d = format!("*** Update File: {path}");
    for h in hunks {
        let n = |k| h.get(k).and_then(|v: &Value| v.as_u64()).unwrap_or(0);
        d.push_str(&format!(
            "\n@@ -{},{} +{},{} @@",
            n("oldStart"),
            n("oldLines"),
            n("newStart"),
            n("newLines")
        ));
        for l in h.get("lines")?.as_array()? {
            d.push('\n');
            d.push_str(l.as_str().unwrap_or(""));
        }
    }
    Some(truncate(&d, MAX_TEXT))
}

/// 行単位の差分。`old` と `new` の全行を文脈として含め、`@@` で始める
fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut d = String::from("@@");
    for (mark, l) in diff_lines(&a, &b) {
        d.push('\n');
        d.push(mark);
        d.push_str(l);
    }
    d
}

/// 2 つの行列の差分を (' ' | '-' | '+', 行) の列で返す（最長共通部分列による）
fn diff_lines<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, &'a str)> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a2, b2) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut out: Vec<(char, &str)> = a[..pre].iter().map(|l| (' ', *l)).collect();
    let (n, m) = (a2.len(), b2.len());
    if n * m > 1_000_000 {
        // 大きすぎるときは比べずに、消した行と足した行を並べる
        out.extend(a2.iter().map(|l| ('-', *l)));
        out.extend(b2.iter().map(|l| ('+', *l)));
    } else {
        // lcs[i * (m + 1) + j] = a2[i..] と b2[j..] の最長共通部分列の長さ
        let w = m + 1;
        let mut lcs = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * w + j] = if a2[i] == b2[j] {
                    lcs[(i + 1) * w + j + 1] + 1
                } else {
                    lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a2[i] == b2[j] {
                out.push((' ', a2[i]));
                i += 1;
                j += 1;
            } else if lcs[(i + 1) * w + j] >= lcs[i * w + j + 1] {
                out.push(('-', a2[i]));
                i += 1;
            } else {
                out.push(('+', b2[j]));
                j += 1;
            }
        }
        out.extend(a2[i..].iter().map(|l| ('-', *l)));
        out.extend(b2[j..].iter().map(|l| ('+', *l)));
    }
    out.extend(a[a.len() - suf..].iter().map(|l| (' ', *l)));
    out
}

/// TodoWrite の (内容, 状態) 列
fn todo_items(input: &Value) -> Vec<(String, String)> {
    let Some(Value::Array(a)) = input.get("todos") else {
        return Vec::new();
    };
    a.iter()
        .map(|t| {
            let f = |k| {
                t.get(k)
                    .and_then(|x: &Value| x.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            (f("content"), f("status"))
        })
        .collect()
}

/// 計画を「✓ 済み / ▶ 進行中 / ○ 未着手」の行で並べる
pub(super) fn checklist(items: &[(String, String)]) -> String {
    items
        .iter()
        .map(|(step, st)| {
            let mark = match st.as_str() {
                "completed" => "✓",
                "in_progress" => "▶",
                _ => "○",
            };
            format!("{mark} {step}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 計画の要約（「済み数/全体 進行中の項目」）
pub(super) fn plan_summary(items: &[(String, String)]) -> String {
    let done = items.iter().filter(|p| p.1 == "completed").count();
    let cur = items
        .iter()
        .find(|p| p.1 == "in_progress")
        .map(|p| p.0.as_str())
        .unwrap_or("");
    one_line(&format!("{done}/{} {cur}", items.len()), 160)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_yaml_preserves_unicode_indentation_and_truncation() {
        let value = serde_json::json!({"前":"あ\r\nい\n", "rows":[
            {"kind":"日本語", "values":[true, 1, null]},
            {"empty":{}, "text":"x\n\ny\n"}], "empty":[]});
        let expected = "前:\n  あ\n  い\nrows:\n  - kind: 日本語\n    values: [true, 1, null]\n  - empty: {}\n    text:\n      x\n      \n      y\nempty: []";
        assert_eq!(yaml(&value).join("\n"), expected);
        for limit in 0..=expected.chars().count() + 2 {
            assert_eq!(
                bounded_yaml(&value, limit),
                truncate(expected, limit),
                "limit={limit}"
            );
        }
        assert_eq!(
            bounded_yaml(&serde_json::json!(["a\nb", [2, 3]]), 100),
            "- a\nb\n- [2, 3]"
        );
        for value in [
            serde_json::json!({"rows":(0..10_000).collect::<Vec<_>>(), "tail":"省略される"}),
            serde_json::json!({"日本語":"あ".repeat(6001)}),
            serde_json::json!({"lines":"日本語\n".repeat(6001)}),
            Value::Object(std::iter::once(("あ".repeat(6001), Value::Bool(true))).collect()),
        ] {
            let full = yaml(&value).join("\n");
            assert_eq!(bounded_yaml(&value, MAX_TEXT), truncate(&full, MAX_TEXT));
        }
    }

    #[test]
    fn inline_arrays_preserve_character_limit_and_scalar_formatting() {
        let short = serde_json::json!(["", null, true, -42, 1.25, "日本語"]);
        assert_eq!(
            inline_list(short.as_array().unwrap()).as_deref(),
            Some("[\"\", null, true, -42, 1.25, 日本語]")
        );
        assert_eq!(inline_list(&[]).as_deref(), Some("[]"));
        let boundary = "あ".repeat(78);
        assert_eq!(
            inline_list(&[Value::String(boundary.clone())]),
            Some(format!("[{boundary}]"))
        );
        assert!(inline_list(&[Value::String("あ".repeat(79))]).is_none());
        assert_eq!(
            inline_list(&[Value::String("x".repeat(74)), Value::String("".into())]),
            Some(format!("[{}, \"\"]", "x".repeat(74)))
        );
        for value in [
            serde_json::json!(["x\ny"]),
            serde_json::json!([{}]),
            serde_json::json!([[]]),
            serde_json::json!(["x".repeat(100_000)]),
            serde_json::json!(vec![0; 100_000]),
        ] {
            assert!(inline_list(value.as_array().unwrap()).is_none());
        }
    }

    #[test]
    fn claude_edits_as_diff() {
        let edit = serde_json::json!({"file_path": "/w/a.rs", "old_string": "a\nb\nc", "new_string": "a\nB\nc\nd"});
        assert_eq!(
            claude_input("Edit", &edit),
            "*** Update File: /w/a.rs\n@@\n a\n-b\n+B\n c\n+d"
        );
        let write = serde_json::json!({"file_path": "/w/n.rs", "content": "x\ny\n"});
        assert_eq!(
            claude_input("Write", &write),
            "*** Add File: /w/n.rs\n+x\n+y"
        );
        let bash =
            serde_json::json!({"command": "cargo test", "description": "テスト", "timeout": 60000});
        assert_eq!(
            claude_input("Bash", &bash),
            "$ cargo test\n\ntimeout: 60000"
        );
        let todo = serde_json::json!({"todos": [{"content": "A", "status": "completed"}, {"content": "B", "status": "in_progress"}]});
        assert_eq!(claude_input("TodoWrite", &todo), "✓ A\n▶ B");
        assert_eq!(summarize_input("TodoWrite", &todo), "1/2 B");
        // 結果に行番号付きの差分があれば、それで置き換える
        let r = serde_json::json!({"filePath": "/w/a.rs", "structuredPatch": [{"oldStart": 3, "oldLines": 2, "newStart": 3, "newLines": 2, "lines": [" a", "-b", "+B"]}]});
        assert_eq!(
            claude_patch(&r).unwrap(),
            "*** Update File: /w/a.rs\n@@ -3,2 +3,2 @@\n a\n-b\n+B"
        );
        assert_eq!(
            claude_patch(&serde_json::json!({"filePath": "/w", "structuredPatch": []})),
            None
        );
    }

    #[test]
    fn diff_keeps_common_lines() {
        let a = ["x", "a", "b", "y"];
        let b = ["x", "b", "c", "y"];
        assert_eq!(
            diff_lines(&a, &b),
            [(' ', "x"), ('-', "a"), (' ', "b"), ('+', "c"), (' ', "y")]
        );
        assert_eq!(diff_lines(&["a"], &["a"]), [(' ', "a")]);
        assert_eq!(diff_lines(&[], &["n"]), [('+', "n")]);
    }
}
