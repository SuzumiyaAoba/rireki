//! アプリの保存先とログ内のパス。ログの区切り文字は実行 OS に依存しない。

use std::path::PathBuf;

/// OS 標準の設定保存先。料金表は既存の設定を参照し、画面計測用の上書きには従わない。
pub fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rireki")
}

/// GUI の設定保存先。環境変数が空でも明示された値をそのまま使う。
pub fn config_dir() -> PathBuf {
    std::env::var_os("RIREKI_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_config_dir)
}

/// 索引・本文・JS 整形キャッシュの共通保存先。
pub fn cache_dir() -> PathBuf {
    std::env::var_os("RIREKI_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
                .join("rireki")
        })
}

pub const SEPARATORS: [char; 2] = ['/', '\\'];

pub fn basename(path: &str) -> &str {
    path.rsplit(SEPARATORS).find(|s| !s.is_empty()).unwrap_or(path)
}

pub fn parent_name(path: &str) -> &str {
    path.trim_end_matches(SEPARATORS).rsplit(SEPARATORS).nth(1).unwrap_or("")
}

/// ディレクトリ部分には末尾の区切り文字も含める。
pub fn split_file(path: &str) -> (&str, &str) {
    match path.rfind(SEPARATORS) {
        Some(i) => (&path[..i + 1], &path[i + 1..]),
        None => ("", path),
    }
}

fn windows_path(path: &str) -> bool {
    path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':')
}

/// ディレクトリの境界を確認してから接頭辞を取り除く。Windows は ASCII の大小文字と区切り文字を同一視する。
pub fn strip_directory_prefix<'a>(path: &'a str, directory: &str) -> Option<&'a str> {
    if directory.is_empty() { return None; }
    let trimmed = directory.trim_end_matches(SEPARATORS);
    let directory = if trimmed.is_empty() { directory } else { trimmed };
    let prefix = path.get(..directory.len())?;
    let windows = windows_path(path) && windows_path(directory);
    if !prefix.bytes().zip(directory.bytes()).all(|(a, b)| {
        a == b || (matches!(a, b'/' | b'\\') && matches!(b, b'/' | b'\\'))
            || (windows && a.eq_ignore_ascii_case(&b))
    }) {
        return None;
    }
    let rest = &path[directory.len()..];
    (rest.is_empty() || rest.starts_with(SEPARATORS) || directory.ends_with(SEPARATORS)).then_some(rest)
}

pub fn relative<'a>(path: &'a str, directory: &str) -> &'a str {
    strip_directory_prefix(path, directory)
        .map(|p| p.trim_start_matches(SEPARATORS))
        .filter(|p| !p.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_paths_work_independently_of_host_os() {
        for (path, name, parent) in [
            ("/home/user/project/", "project", "user"),
            (r"C:\Users\user\project\", "project", "user"),
            (r"C:/Users/user/project/", "project", "user"),
            (r"\\server\share\project", "project", "share"),
            ("project", "project", ""),
        ] {
            assert_eq!(basename(path), name);
            assert_eq!(parent_name(path), parent);
        }
        assert_eq!(split_file(r"src\main.rs"), (r"src\", "main.rs"));
        assert_eq!(split_file("src/main.rs"), ("src/", "main.rs"));
        assert_eq!(split_file("main.rs"), ("", "main.rs"));
    }

    #[test]
    fn relative_paths_require_a_directory_boundary() {
        assert_eq!(relative("/home/user/project/src/main.rs", "/home/user/project/"), "src/main.rs");
        assert_eq!(relative(r"C:\Users\user\project\src\main.rs", "c:/users/User/project/"), r"src\main.rs");
        assert_eq!(relative(r"\\server\share\src\main.rs", r"\\SERVER\share"), r"src\main.rs");
        assert_eq!(relative("/project-other/main.rs", "/project"), "/project-other/main.rs");
        assert_eq!(relative(r"C:\project-other\main.rs", r"C:\project"), r"C:\project-other\main.rs");
        assert_eq!(relative("/Project/main.rs", "/project"), "/Project/main.rs");
        assert_eq!(relative("/project", "/project"), "/project");
        assert_eq!(relative("/project/main.rs", ""), "/project/main.rs");
        assert_eq!(relative("/main.rs", "/"), "main.rs");
        assert_eq!(relative("日本語/main.rs", "日本"), "日本語/main.rs");
    }
}
