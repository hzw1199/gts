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
    let home = temp_dir("gts-init-func-home");
    let real_gts = dirs::home_dir().unwrap().join(".gts");
    assert!(!home.starts_with(&real_gts));
    home
}

fn run_cli(args: &[&str], cwd: &PathBuf, home: &PathBuf, extra_env: &[(&str, &str)]) -> RunResult {
    let mut cmd = Command::new(gts_bin());
    cmd.args(args).current_dir(cwd).env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(user_home) = std::env::var("HOME") {
        cmd.env("HOME", user_home);
    }
    cmd.env("GTS_HOME", home);
    for (key, value) in extra_env {
        if key == &"CI" && value.is_empty() {
            continue;
        }
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
fn moves_plain_git_into_storage_and_links() {
    let home = isolated_home();
    let worktree = temp_dir("gts-init-func-wt");
    fs::create_dir_all(worktree.join(".git")).unwrap();
    fs::write(worktree.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let result = run_cli(&["init", "main"], &worktree, &home, &[("CI", "")]);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Initialized track \"main\""));
    assert!(is_english(&result.stdout));

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
    assert!(fs::read_link(&git_path).unwrap().is_absolute());

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    let obj = index.as_object().unwrap();
    assert_eq!(obj.len(), 1);
    let (id, entry) = obj.iter().next().unwrap();
    assert_eq!(entry["active"], "main");
    assert_eq!(
        fs::canonicalize(entry["path"].as_str().unwrap()).unwrap(),
        fs::canonicalize(&worktree).unwrap()
    );
    let expected = home.join("storage").join(id).join("main").join(".git");
    assert_eq!(
        fs::canonicalize(fs::read_link(&git_path).unwrap()).unwrap(),
        fs::canonicalize(&expected).unwrap()
    );
    let real_gts = dirs::home_dir().unwrap().join(".gts");
    assert!(!home.starts_with(&real_gts));
}

#[test]
fn creates_storage_git_when_git_missing() {
    let home = isolated_home();
    let worktree = temp_dir("gts-init-func-wt");
    let result = run_cli(&["init", "main"], &worktree, &home, &[("CI", "")]);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
    let target = fs::canonicalize(fs::read_link(&git_path).unwrap()).unwrap();
    let head = fs::read_to_string(target.join("HEAD")).unwrap();
    assert!(head.contains("ref:"));
}

#[test]
fn rejects_gitfile() {
    let home = isolated_home();
    let worktree = temp_dir("gts-init-func-wt");
    fs::write(worktree.join(".git"), "gitdir: /tmp/elsewhere\n").unwrap();
    let result = run_cli(&["init", "main"], &worktree, &home, &[("CI", "")]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.to_lowercase().contains("gitfile"));
    assert!(is_english(&result.stderr));
}

#[test]
fn rejects_reinit_of_registered_workspace() {
    let home = isolated_home();
    let worktree = temp_dir("gts-init-func-wt");
    let first = run_cli(&["init", "main"], &worktree, &home, &[("CI", "")]);
    assert_eq!(first.code, 0, "stderr={}", first.stderr);
    let second = run_cli(&["init", "other"], &worktree, &home, &[("CI", "")]);
    assert_ne!(second.code, 0);
    assert!(second.stderr.to_lowercase().contains("already registered"));
}

#[test]
fn rejects_invalid_track_name() {
    let home = isolated_home();
    let worktree = temp_dir("gts-init-func-wt");
    let result = run_cli(&["init", ".."], &worktree, &home, &[("CI", "")]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("Invalid track name"));
}

#[test]
fn fails_non_interactively_when_name_missing() {
    let home = isolated_home();
    let worktree = temp_dir("gts-init-func-wt");
    let result = run_cli(&["init"], &worktree, &home, &[("CI", "")]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts init <name>"));
    assert!(is_english(&result.stderr));
}

#[test]
fn exits_0_when_cancel_is_simulated() {
    let home = isolated_home();
    let worktree = temp_dir("gts-init-func-wt");
    let result = run_cli(
        &["init"],
        &worktree,
        &home,
        &[("GTS_SIMULATE_CANCEL", "1"), ("CI", "")],
    );
    assert_eq!(result.code, 0);
}
