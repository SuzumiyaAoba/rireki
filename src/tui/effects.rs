//! ユーザーが選んだコピー・保存・ファイル表示。ログの文字列をシェルとして実行しない。

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn copy(text: &str) -> io::Result<String> {
    #[cfg(target_os = "macos")]
    if clipboard_command("pbcopy", &[], text) {
        return Ok("クリップボードにコピーしました".into());
    }
    #[cfg(target_os = "windows")]
    if clipboard_command(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); Set-Clipboard -Value ([Console]::In.ReadToEnd())",
        ],
        text,
    ) {
        return Ok("クリップボードにコピーしました".into());
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    for (program, args) in [
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
    ] {
        if clipboard_command(program, args, text) {
            return Ok("クリップボードにコピーしました".into());
        }
    }
    if text.len() > 1_048_576 {
        return Err(io::Error::other(
            "端末経由のコピー上限（1 MiB）を超えます。JSON 保存を使ってください",
        ));
    }
    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    stdout.flush()?;
    Ok("端末へコピー要求を送信しました（OSC 52 対応が必要）".into())
}

fn clipboard_command(program: &str, args: &[&str], text: &str) -> bool {
    clipboard_command_timeout(program, args, text, Duration::from_secs(3))
}

fn clipboard_command_timeout(program: &str, args: &[&str], text: &str, timeout: Duration) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let Some(mut input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };
    let bytes = text.as_bytes().to_vec();
    let (sent, received) = mpsc::sync_channel(1);
    // パイプへ上限以上を書いた時点で OS が待たせるため、書込み自体も期限の対象にする。
    let Ok(writer) = std::thread::Builder::new()
        .name("rireki-tui-clipboard".into())
        .spawn(move || {
            let written = input.write_all(&bytes).is_ok();
            drop(input);
            let _ = sent.send(written);
        })
    else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };
    let deadline = Instant::now() + timeout;
    let mut written = None;
    loop {
        if written.is_none() {
            written = received.try_recv().ok();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // 子が正常終了しても、入力の送信完了を確認してから成功を返す。
                let written = written
                    .or_else(|| {
                        received
                            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            .ok()
                    })
                    .unwrap_or(false);
                if writer.is_finished() {
                    let _ = writer.join();
                }
                return written && status.success();
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // 通常はパイプが閉じて直ちに終了する。孫プロセスが継承した場合もUIを待たせない。
                if writer.is_finished() {
                    let _ = writer.join();
                }
                return false;
            }
        }
    }
}

pub fn save(path: &Path, content: &str) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()
}

pub fn reveal(path: &str) -> io::Result<()> {
    let path = Path::new(path).canonicalize()?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg("-R").arg(&path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("explorer.exe");
        command.arg(format!("/select,{}", path.display()));
        command
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(&path));
        command
    };
    // ファイルを実行せず、ファイルマネージャーで所在地を表示する。
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        out.push(DIGITS[(a >> 2) as usize] as char);
        out.push(DIGITS[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            DIGITS[(((b & 15) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            DIGITS[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temporary(name: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "rireki-tui-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    #[test]
    fn terminal_clipboard_encodes_controls_and_unicode_as_data() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(
            base64("日本語\x1b]52;bad\x07".as_bytes()),
            "5pel5pys6KqeG101MjtiYWQH"
        );
    }
    #[test]
    fn export_never_overwrites_an_existing_file() {
        let root = temporary("export");
        save(&root, "original").unwrap();
        assert_eq!(
            save(&root, "replacement").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(&root).unwrap(), "original");
        std::fs::remove_file(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn clipboard_timeout_covers_a_full_stdin_pipe_and_input_is_not_shell_code() {
        let started = Instant::now();
        assert!(!clipboard_command_timeout(
            "/bin/sh",
            &["-c", "exec sleep 10"],
            &"x".repeat(1_048_576),
            Duration::from_millis(100)
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(clipboard_command_timeout(
            "/bin/sh",
            &["-c", "cat >/dev/null"],
            "日本語\n'$(exit 1)'\n\x1b[2J",
            Duration::from_secs(2)
        ));
        assert!(!clipboard_command_timeout(
            "/bin/sh",
            &["-c", "exit 7"],
            "copy",
            Duration::from_secs(2)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn export_refuses_symlinks_and_new_history_files_are_private() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let original = temporary("original");
        let link = temporary("symlink");
        save(&original, "日本語\n記録").unwrap();
        assert_eq!(
            std::fs::metadata(&original).unwrap().permissions().mode() & 0o077,
            0
        );
        symlink(&original, &link).unwrap();
        assert_eq!(
            save(&link, "replacement").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "日本語\n記録");
        std::fs::remove_file(link).unwrap();
        std::fs::remove_file(original).unwrap();
    }
}
