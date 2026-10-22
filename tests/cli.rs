use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rireki-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("claude")).unwrap();
        std::fs::create_dir_all(root.join("codex")).unwrap();
        Self(root)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rireki"));
        command
            .env("RIREKI_CACHE_DIR", self.0.join("cache"))
            .env("RIREKI_CONFIG_DIR", self.0.join("config"));
        command
    }

    #[cfg(feature = "tui")]
    fn tui_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rireki-tui"));
        command
            .env("RIREKI_CACHE_DIR", self.0.join("cache"))
            .env("RIREKI_CONFIG_DIR", self.0.join("config"));
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn help_runs_without_opening_a_window_or_database() {
    let fixture = Fixture::new();
    for flag in ["--help", "-h"] {
        let help = stdout(fixture.command().arg(flag).output().unwrap());
        assert!(help.contains("--tui"));
        assert!(help.contains("--no-default-features --features tui"));
        assert!(help.contains("--stats"));
        assert!(help.contains("--bench-discover"));
    }
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}

#[test]
fn discovery_benchmark_runs_on_explicit_empty_log_roots() {
    let fixture = Fixture::new();
    let output = stdout(
        fixture
            .command()
            .arg("--bench-discover")
            .arg(fixture.0.join("claude"))
            .arg(fixture.0.join("codex"))
            .output()
            .unwrap(),
    );
    let rows: Vec<serde_json::Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["claude"], 0);
    assert_eq!(rows[0]["legacy"], 0);
    assert_eq!(rows[0]["codex"], 0);
    assert_eq!(rows[1]["name"], "discover");
    assert_eq!(rows[1]["repetitions"], 31);
}

#[cfg(not(any(feature = "gui", feature = "tui")))]
#[test]
fn build_without_gui_explains_how_to_run_when_no_command_is_given() {
    let fixture = Fixture::new();
    let output = fixture.command().output().unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("GUI が含まれていません"));
    assert!(error.contains("--help"));
    assert!(!fixture.0.join("cache").exists());
}

#[cfg(feature = "tui")]
#[test]
fn tui_help_runs_without_a_terminal_window_or_database() {
    let fixture = Fixture::new();
    for flag in ["--help", "-h"] {
        let help = stdout(fixture.command().args(["--tui", flag]).output().unwrap());
        assert!(help.contains("--claude-root"));
        assert!(help.contains("--codex-root"));
        assert!(!help.contains('\u{1b}'));
    }
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}

#[cfg(feature = "tui")]
#[test]
fn tui_requires_a_terminal_before_loading_history_or_emitting_terminal_controls() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--tui", "--claude-root"])
        .arg(fixture.0.join("claude"))
        .arg("--codex-root")
        .arg(fixture.0.join("codex"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("端末"), "{error}");
    assert!(!error.contains("panicked"));
    assert!(!error.contains('\u{1b}'));
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}

#[cfg(feature = "tui")]
#[test]
fn console_binary_help_runs_without_a_terminal_or_database() {
    let fixture = Fixture::new();
    for flag in ["--help", "-h"] {
        let help = stdout(fixture.tui_command().arg(flag).output().unwrap());
        assert!(help.contains("--claude-root"));
        assert!(help.contains("--codex-root"));
        assert!(!help.contains('\u{1b}'));
    }
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}

#[cfg(feature = "tui")]
#[test]
fn console_binary_requires_a_terminal_before_loading_history() {
    let fixture = Fixture::new();
    let output = fixture
        .tui_command()
        .arg("--claude-root")
        .arg(fixture.0.join("claude"))
        .arg("--codex-root")
        .arg(fixture.0.join("codex"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("端末"), "{error}");
    assert!(!error.contains("panicked"));
    assert!(!error.contains('\u{1b}'));
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}

#[cfg(all(
    feature = "gui",
    feature = "tui",
    target_os = "windows",
    not(debug_assertions)
))]
#[test]
fn windows_gui_release_directs_terminal_users_to_the_console_binary() {
    let fixture = Fixture::new();
    let output = fixture.command().arg("--tui").output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("rireki-tui.exe"), "{error}");
    assert!(!error.contains('\u{1b}'));
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}

#[cfg(all(not(feature = "gui"), feature = "tui"))]
#[test]
fn build_with_only_tui_uses_the_terminal_frontend_by_default() {
    let fixture = Fixture::new();
    let output = fixture.command().output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("端末"), "{error}");
    assert!(!error.contains('\u{1b}'));
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}

#[cfg(not(feature = "tui"))]
#[test]
fn build_without_tui_explains_the_required_feature() {
    let fixture = Fixture::new();
    let output = fixture.command().arg("--tui").output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("TUI が含まれていません"));
    assert!(error.contains("--no-default-features --features tui"));
    assert!(!error.contains('\u{1b}'));
    assert!(!fixture.0.join("cache").exists());
    assert!(!fixture.0.join("config").exists());
}
