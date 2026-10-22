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

#[cfg(not(feature = "gui"))]
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
