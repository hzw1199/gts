use std::process::Command;
use std::sync::OnceLock;

fn gts_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        option_env!("CARGO_BIN_EXE_gts")
            .map(str::to_string)
            .unwrap_or_else(|| {
                let mut path = std::env::current_exe().expect("current exe");
                path.pop(); // deps
                if path.ends_with("deps") {
                    path.pop();
                }
                path.push("gts");
                path.to_string_lossy().into_owned()
            })
    })
    .as_str()
}

struct RunResult {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_cli(args: &[&str], env: &[(&str, &str)]) -> RunResult {
    let tmp = tempfile_dir();
    let mut cmd = Command::new(gts_bin());
    cmd.args(args).current_dir(&tmp).env_clear();
    // Keep PATH so anything unexpected still resolves; strip CI/GTS_* by default.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
    cmd.env("GTS_HOME", tmp.join("gts-home"));
    for (key, value) in env {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("spawn gts");
    RunResult {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn tempfile_dir() -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let name = format!(
        "gts-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    dir.push(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn is_english(text: &str) -> bool {
    !text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

#[test]
fn prints_english_help_with_no_interactive() {
    let result = run_cli(&["--help"], &[("CI", "")]);
    assert_eq!(result.code, 0);
    assert!(result.stdout.contains("--no-interactive"));
    assert!(result.stdout.contains("Usage:"));
    assert!(is_english(&result.stdout));
}

#[test]
fn exits_0_when_cancel_is_simulated() {
    let result = run_cli(&["demo"], &[("GTS_SIMULATE_CANCEL", "1"), ("CI", "")]);
    assert_eq!(result.code, 0);
}

#[test]
fn exits_nonzero_when_required_name_missing_and_non_tty() {
    let result = run_cli(&["demo"], &[("CI", "")]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts demo --name <name>"));
    assert!(is_english(&result.stderr));
}

#[test]
fn exits_nonzero_when_ci_set_and_name_missing() {
    let result = run_cli(&["demo"], &[("CI", "true")]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts demo --name <name>"));
}

#[test]
fn exits_nonzero_with_no_interactive_when_name_missing() {
    let result = run_cli(&["demo", "--no-interactive"], &[("CI", "")]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts demo --name <name>"));
}

#[test]
fn succeeds_with_name_without_prompting() {
    let result = run_cli(&["demo", "--name", "alpha"], &[("CI", "")]);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Demo name: alpha"));
    assert!(is_english(&result.stdout));
}

#[test]
fn force_interactive_with_script_answers() {
    let result = run_cli(
        &["demo"],
        &[
            ("CI", ""),
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", r#"["beta"]"#),
        ],
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(
        result.stdout.contains("Demo name: beta") || result.stderr.contains("Demo name: beta"),
        "stdout={} stderr={}",
        result.stdout,
        result.stderr
    );
}
