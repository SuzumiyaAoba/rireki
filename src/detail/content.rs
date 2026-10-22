//! 保存状態をまたぐ本文の復元と、表示用の JS・シェルコマンド整形。

use std::collections::HashMap;

use crate::db::JsCache;
use crate::loader;
use crate::model::{Event, EventText, TextRef};

pub(super) struct DetailEvent {
    pub(super) index: usize,
    pub(super) row: Option<i64>,
    pub(super) text_ref: TextRef,
    pub(super) input: String,
    pub(super) output: String,
    pub(super) edit: bool,
}

impl DetailEvent {
    pub(super) fn capture(index: usize, event: &Event, edit: bool) -> Self {
        let (row, input, output) = match event.text() {
            EventText::Row(row) => (Some(row), event.input.clone(), event.output.clone()),
            EventText::Inline(input, output) => (None, input.to_string(), output.to_string()),
            EventText::Pending(input, output) => (None, input, output),
        };
        Self {
            index,
            row,
            text_ref: event.text_ref.clone(),
            input,
            output,
            edit,
        }
    }

    /// 捕捉後に DB が更新されて行が消えた場合も、旧版の退避本文を復元する。
    pub(super) fn text(self, fetched: &mut HashMap<i64, (String, String)>) -> (String, String) {
        match self.row.and_then(|row| fetched.remove(&row)) {
            Some(text) => text,
            None => match self.text_ref.text() {
                EventText::Pending(input, output) => (input, output),
                _ => (self.input, self.output),
            },
        }
    }
}

pub(super) fn format_input(input: String, cache: Option<&JsCache>) -> (String, Option<String>) {
    match input.strip_prefix(loader::JS_MARK) {
        Some(js) => (exec_detail_cached(cache, js), Some(js.to_string())),
        None => (input, None),
    }
}

/// 整形した入力から、実行したシェルコマンドを取り出す。
pub(super) fn shell_commands(input: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for para in input.split("\n\n") {
        let at = if para.starts_with("$ ") {
            Some(0)
        } else {
            para.find("\n$ ").map(|i| i + 1)
        };
        if let Some(at) = at {
            let cmd = para[at + 2..].trim_end();
            if !cmd.is_empty() {
                out.push(cmd);
            }
        }
    }
    out
}

/// exec の JS を整形する。初回はモック実行してキャッシュに保存する。
fn exec_detail_cached(cache: Option<&JsCache>, js: &str) -> String {
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
    fn commands_in_input() {
        assert_eq!(shell_commands("$ ls -la\n\ncwd: /w"), vec!["ls -la"]);
        let exec = "（JS をモック実行して推定: 2 件）\n\n#0 exec_command\n$ cat a\n  | wc\n\ncwd: /w\n\n#1 a ← exec_command\n$ rg x";
        assert_eq!(shell_commands(exec), vec!["cat a\n  | wc", "rg x"]);
        assert!(shell_commands("{\"a\": 1}").is_empty());
    }
}
