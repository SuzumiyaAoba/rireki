//! apply_patch と unified diff の解析。表示とは独立したファイル・行のデータを返す。

/// 差分の 1 行
#[derive(Clone, PartialEq, Debug)]
pub struct DiffRow {
    /// ' ' = 文脈, '+' = 追加, '-' = 削除, '@' = ハンクの見出し
    pub kind: char,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// 1 ファイル分の差分
#[derive(Clone, PartialEq, Debug)]
pub struct FilePatch {
    /// 「変更」「追加」「削除」
    pub op: &'static str,
    pub path: String,
    pub rows: Vec<DiffRow>,
    pub added: usize,
    pub removed: usize,
}

/// `@@ -a,b +c,d @@` の (a, c)
fn hunk_start(h: &str) -> Option<(u32, u32)> {
    let mut it = h.split_whitespace().skip(1);
    let old = it.next()?.strip_prefix('-')?;
    let new = it.next()?.strip_prefix('+')?;
    let n = |s: &str| s.split(',').next()?.parse::<u32>().ok();
    Some((n(old)?, n(new)?))
}

/// 差分として読めるテキストか（先頭の行で判定する）
pub fn is_patch(s: &str) -> bool {
    let first = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    [
        "*** Begin Patch",
        "*** Update File: ",
        "*** Add File: ",
        "*** Delete File: ",
        "diff --git ",
    ]
    .iter()
    .any(|p| first.starts_with(p))
}

/// apply_patch / Edit の `*** Update File: パス` 形式と、git の unified diff を読む。
/// ファイルの見出しより前の行は無視する（Codex の exec の整形結果に混ざったパッチも拾える）
pub fn parse_patch(s: &str) -> Vec<FilePatch> {
    let mut files: Vec<FilePatch> = Vec::new();
    let (mut old, mut new): (Option<u32>, Option<u32>) = (None, None);
    // git diff のファイル見出し（最初のハンクより前）
    let mut header = false;
    let start = |files: &mut Vec<FilePatch>, op, path: &str| {
        files.push(FilePatch {
            op,
            path: path.trim().to_string(),
            rows: Vec::new(),
            added: 0,
            removed: 0,
        });
    };
    for l in s.lines() {
        if let Some(p) = l.strip_prefix("*** Update File: ") {
            start(&mut files, "変更", p);
            (old, new, header) = (None, None, false);
            continue;
        }
        if let Some(p) = l.strip_prefix("*** Add File: ") {
            start(&mut files, "追加", p);
            (old, new, header) = (None, Some(1), false);
            continue;
        }
        if let Some(p) = l.strip_prefix("*** Delete File: ") {
            start(&mut files, "削除", p);
            continue;
        }
        if let Some(rest) = l.strip_prefix("diff --git ") {
            let path = rest.split_once(" b/").map_or(rest, |(_, b)| b);
            start(&mut files, "変更", path);
            header = true;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if let Some(p) = l.strip_prefix("*** Move to: ") {
            f.path = format!("{} → {}", f.path, p.trim());
            continue;
        }
        if l.starts_with("*** ") {
            continue;
        }
        if header {
            if l.starts_with("--- /dev/null") || l.starts_with("new file mode") {
                f.op = "追加";
            } else if l.starts_with("+++ /dev/null") || l.starts_with("deleted file mode") {
                f.op = "削除";
            }
            if !l.starts_with("@@") {
                continue;
            }
            header = false;
        }
        if l.starts_with("@@") {
            if let Some((a, b)) = hunk_start(l) {
                (old, new) = (Some(a), Some(b));
            }
            // 行番号も文脈（関数名など）もない先頭の `@@` は区切りの意味がないので出さない
            if l.trim() != "@@" || !f.rows.is_empty() {
                f.rows.push(DiffRow {
                    kind: '@',
                    old: None,
                    new: None,
                    text: l.trim().to_string(),
                });
            }
            continue;
        }
        let (kind, text) = match l.as_bytes().first() {
            Some(b'+') => ('+', &l[1..]),
            Some(b'-') => ('-', &l[1..]),
            Some(b' ') => (' ', &l[1..]),
            Some(b'\\') => continue, // "\ No newline at end of file"
            _ => (' ', l),
        };
        let row = DiffRow {
            kind,
            old: if kind == '+' { None } else { old },
            new: if kind == '-' { None } else { new },
            text: text.to_string(),
        };
        match kind {
            '+' => f.added += 1,
            '-' => f.removed += 1,
            _ => {}
        }
        if kind != '+' {
            old = old.map(|n| n + 1);
        }
        if kind != '-' {
            new = new.map(|n| n + 1);
        }
        f.rows.push(row);
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches() {
        let p = "*** Update File: /w/src/a.rs\n@@ -10,3 +10,4 @@ fn f()\n ctx\n-old\n+new\n+more\n ctx2";
        let f = &parse_patch(p)[0];
        assert_eq!(
            (f.op, f.path.as_str(), f.added, f.removed),
            ("変更", "/w/src/a.rs", 2, 1)
        );
        let nums: Vec<_> = f.rows.iter().map(|r| (r.kind, r.old, r.new)).collect();
        assert_eq!(
            nums,
            [
                ('@', None, None),
                (' ', Some(10), Some(10)),
                ('-', Some(11), None),
                ('+', None, Some(11)),
                ('+', None, Some(12)),
                (' ', Some(12), Some(13)),
            ]
        );
        assert_eq!(f.rows[0].text, "@@ -10,3 +10,4 @@ fn f()");
        // apply_patch（行番号なし）と追加・削除
        let p = "*** Begin Patch\n*** Add File: b.rs\n+x\n*** Delete File: c.rs\n*** Update File: d.rs\n*** Move to: e.rs\n@@\n-a\n+b\n*** End Patch";
        let fs = parse_patch(p);
        assert_eq!(
            fs.iter().map(|f| (f.op, f.path.as_str())).collect::<Vec<_>>(),
            [("追加", "b.rs"), ("削除", "c.rs"), ("変更", "d.rs → e.rs")]
        );
        assert_eq!(fs[0].rows[0].new, Some(1));
        // 先頭の `@@` だけの見出しは省く
        assert_eq!(fs[2].rows.iter().map(|r| r.kind).collect::<String>(), "-+");
        assert!(is_patch(p) && !is_patch("#0 apply_patch\n*** Begin Patch"));
        // git diff
        let g = "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b";
        let f = &parse_patch(g)[0];
        assert_eq!(
            (f.path.as_str(), f.added, f.removed, f.rows.len()),
            ("x.rs", 1, 1, 3)
        );
        // 見出しより前の行は無視する（exec の整形結果）
        assert_eq!(
            parse_patch("#0 apply_patch\n*** Begin Patch\n*** Update File: a\n@@\n+x").len(),
            1
        );
    }
}
