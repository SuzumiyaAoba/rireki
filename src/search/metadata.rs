//! メタデータの語検索。索引検索の ASCII 小文字化とは別に Unicode を扱う。

/// 各語を名前・要約・モデルなどの三つのフィールドにまたがって照合する。
/// 後続フィールドの小文字化は必要になったときだけ行い、イベント内で再利用する。
pub(crate) fn matches_words(words: &[&str], fields: [&str; 3]) -> bool {
    if words.is_empty() {
        return true;
    }
    let first = fields[0].to_lowercase();
    let mut second = None;
    let mut third = None;
    words.iter().all(|word| {
        first.contains(word)
            || second
                .get_or_insert_with(|| fields[1].to_lowercase())
                .contains(word)
            || third
                .get_or_insert_with(|| fields[2].to_lowercase())
                .contains(word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_search_matches_joined_reference_across_unicode_fields() {
        let cases = [
            ["", "", ""],
            ["EXEC_command", "cargo test 日本語", "MODEL-X"],
            ["ΟΣ", "İΣ 日本語\u{3000}入力", "gpt-6-sol"],
            ["Ο", "Σ", "\u{301}Σ"],
            ["Σ\u{301}", "Ο\u{301}Σ", "ΣΟ"],
            ["line\nName", "\tSummary\r\n", "模型"],
        ];
        for fields in cases {
            let reference = fields.join("\n").to_lowercase();
            for query in [
                "",
                " \t\u{3000}\n",
                "exec CARGO",
                "日本語 MODEL-x",
                "ος",
                "οσ",
                "σ",
                "i\u{307}",
                "sol 入力",
                "name summary 模型",
                "missing",
            ] {
                let query = query.to_lowercase();
                let words: Vec<_> = query.split_whitespace().collect();
                assert_eq!(
                    matches_words(&words, fields),
                    words.iter().all(|w| reference.contains(w)),
                    "fields={fields:?}, query={query:?}"
                );
            }
        }
    }

    #[test]
    fn words_can_match_different_fields_but_cannot_join_adjacent_fields() {
        let fields = ["exec", "日本語", "model"];
        assert!(matches_words(&["exec", "日本語", "model"], fields));
        assert!(!matches_words(&["exec日本語"], fields));
        assert!(!matches_words(&["日本語model"], fields));
    }
}
