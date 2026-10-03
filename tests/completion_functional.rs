use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

fn gts_bin() -> &'static str {
    static BIN: OnceLock<String> = OnceLock::new();
    BIN.get_or_init(|| {
        option_env!("CARGO_BIN_EXE_gts")
            .map(str::to_string)
            .unwrap_or_else(|| {
                let mut path = std::env::current_exe().expect("current exe");
                path.pop();
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

fn temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn isolated_home() -> PathBuf {
    let home = temp_dir("gts-comp-func-home");
    let real_gts = dirs::home_dir().unwrap().join(".gts");
    assert!(!home.starts_with(&real_gts));
    home
}

fn run_cli(args: &[&str], cwd: &PathBuf, home: &PathBuf, extra: &[(&str, &str)]) -> RunResult {
    let mut cmd = Command::new(gts_bin());
    cmd.args(args).current_dir(cwd).env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(user_home) = std::env::var("HOME") {
        cmd.env("HOME", user_home);
    }
    cmd.env("GTS_HOME", home);
    for (key, value) in extra {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("spawn gts");
    RunResult {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn is_english(text: &str) -> bool {
    !text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

#[test]
fn lists_tracks_matching_storage_in_registered_workspace() {
    let home = isolated_home();
    let worktree = temp_dir("gts-comp-func-wt");
    fs::create_dir_all(worktree.join(".git")).unwrap();
    fs::write(worktree.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let init = run_cli(&["init", "main"], &worktree, &home, &[]);
    assert_eq!(init.code, 0, "stderr={}", init.stderr);

    let create = run_cli(&["create", "feature"], &worktree, &home, &[]);
    assert_eq!(create.code, 0, "stderr={}", create.stderr);

    let result = run_cli(&["completion", "--tracks"], &worktree, &home, &[]);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    let names: std::collections::HashSet<_> = result
        .stdout
        .trim()
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(
        names,
        ["main".into(), "feature".into()].into_iter().collect()
    );
    assert_eq!(result.stderr, "");
}

#[test]
fn fails_without_guessing_when_unregistered() {
    let home = isolated_home();
    let worktree = temp_dir("gts-comp-func-wt");
    let result = run_cli(&["completion", "--tracks"], &worktree, &home, &[]);
    assert_ne!(result.code, 0);
    assert_eq!(result.stdout, "");
    assert!(result.stderr.contains("No registered workspace found"));
    assert!(is_english(&result.stderr));
}

#[test]
fn emits_bash_and_zsh_scripts_without_writing_shell_rc() {
    let home = isolated_home();
    let worktree = temp_dir("gts-comp-func-wt");
    let fake_home = temp_dir("gts-comp-fake-home");
    let zshrc = fake_home.join(".zshrc");
    let bashrc = fake_home.join(".bashrc");

    for shell in ["bash", "zsh"] {
        let result = run_cli(
            &["completion", shell],
            &worktree,
            &home,
            &[("HOME", fake_home.to_str().unwrap())],
        );
        assert_eq!(result.code, 0, "stderr={}", result.stderr);
        assert!(result.stdout.contains("completion --tracks"));
        assert!(result.stdout.contains("switch|to|remove"));
        assert!(result.stdout.contains("rename"));
        assert!(!result.stdout.contains("create)"));
        assert!(is_english(&format!("{}{}", result.stdout, result.stderr)));
    }

    assert!(!zshrc.exists());
    assert!(!bashrc.exists());
}

#[test]
fn rejects_unknown_shell_with_english_error() {
    let home = isolated_home();
    let worktree = temp_dir("gts-comp-func-wt");
    let result = run_cli(&["completion", "fish"], &worktree, &home, &[]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("Unsupported shell"));
    assert!(is_english(&result.stderr));
}

#[test]
fn shows_completion_in_help() {
    let home = isolated_home();
    let worktree = temp_dir("gts-comp-func-wt");
    let result = run_cli(&["--help"], &worktree, &home, &[]);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("completion"));
}
