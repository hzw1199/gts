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
    let home = temp_dir("gts-remove-func-home");
    let real_gts = dirs::home_dir().unwrap().join(".gts");
    assert!(!home.starts_with(&real_gts));
    home
}

fn run_cli_env(args: &[&str], cwd: &PathBuf, home: &PathBuf, extra: &[(&str, &str)]) -> RunResult {
    let mut cmd = Command::new(gts_bin());
    cmd.args(args).current_dir(cwd).env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(user_home) = std::env::var("HOME") {
        cmd.env("HOME", user_home);
    }
    cmd.env("GTS_HOME", home);
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("spawn gts");
    RunResult {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn run_cli(args: &[&str], cwd: &PathBuf, home: &PathBuf) -> RunResult {
    run_cli_env(args, cwd, home, &[("CI", "1")])
}

fn run_git(args: &[&str], cwd: &PathBuf) -> (i32, String, String) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "gts-test")
        .env("GIT_AUTHOR_EMAIL", "gts-test@example.com")
        .env("GIT_COMMITTER_NAME", "gts-test")
        .env("GIT_COMMITTER_EMAIL", "gts-test@example.com")
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn path_exists(path: &std::path::Path) -> bool {
    fs::metadata(path).is_ok() || fs::symlink_metadata(path).is_ok()
}

fn storage_track(home: &PathBuf, id: &str, track: &str) -> PathBuf {
    home.join("storage").join(id).join(track)
}

fn init_workspace(worktree: &PathBuf, home: &PathBuf) -> String {
    let (code, _, stderr) = run_git(&["init"], worktree);
    assert_eq!(code, 0, "git init: {stderr}");
    fs::write(worktree.join("tracked.txt"), "base\n").unwrap();
    let (code, _, stderr) = run_git(&["add", "tracked.txt"], worktree);
    assert_eq!(code, 0, "git add: {stderr}");
    let (code, _, stderr) = run_git(&["commit", "-m", "init"], worktree);
    assert_eq!(code, 0, "git commit: {stderr}");
    let init = run_cli(&["init", "main"], worktree, home);
    assert_eq!(init.code, 0, "stderr={}", init.stderr);
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    index
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone()
}

#[test]
fn removes_inactive_track_with_yes() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let exclude = storage_track(&home, &id, "feature")
        .join(".git")
        .join("info")
        .join("exclude");
    fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    fs::write(&exclude, "local-ignore\n").unwrap();

    let before_link = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();
    let before_active = {
        let index: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
        index[&id]["active"].as_str().unwrap().to_string()
    };

    let result = run_cli(&["remove", "feature", "--yes"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Removed track \"feature\""));

    assert!(!path_exists(&storage_track(&home, &id, "feature")));
    assert!(path_exists(&storage_track(&home, &id, "main")));
    assert!(!path_exists(&exclude));
    assert_eq!(
        fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
        before_link
    );
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    assert_eq!(index[&id]["active"], before_active);
}

#[test]
fn refuses_removing_active_track() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    let id = init_workspace(&worktree, &home);

    let failed = run_cli(&["remove", "main", "--yes"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("Cannot remove the active track"));
    assert!(path_exists(&storage_track(&home, &id, "main")));
}

#[test]
fn fails_on_missing_target() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    let id = init_workspace(&worktree, &home);

    let failed = run_cli(&["remove", "missing", "--yes"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("Track not found"));
    assert!(!path_exists(&storage_track(&home, &id, "missing")));
}

#[test]
fn fails_non_interactive_without_yes() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let failed = run_cli(&["remove", "feature"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("--yes"));
    assert!(failed.stderr.contains("gts remove feature --yes"));
    assert!(path_exists(&storage_track(&home, &id, "feature")));
}

#[test]
fn fails_when_track_name_missing_with_copy_pasteable_hint() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    init_workspace(&worktree, &home);

    let failed = run_cli_env(
        &["remove", "--no-interactive"],
        &worktree,
        &home,
        &[("CI", "1")],
    );
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("gts remove <track> --yes"));
}

#[test]
fn interactive_cancel_does_not_delete() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let result = run_cli_env(
        &["remove", "feature"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", "[false]"),
        ],
    );
    assert_eq!(result.code, 0, "stderr={} stdout={}", result.stderr, result.stdout);
    assert!(path_exists(&storage_track(&home, &id, "feature")));
}

#[test]
fn interactive_confirm_deletes() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let result = run_cli_env(
        &["remove", "feature"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", "[true]"),
        ],
    );
    assert_eq!(result.code, 0, "stderr={} stdout={}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Removed track \"feature\""));
    assert!(!path_exists(&storage_track(&home, &id, "feature")));
}

#[test]
fn fails_unregistered_workspace() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    fs::create_dir_all(&worktree).unwrap();

    let failed = run_cli(&["remove", "main", "--yes"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(
        failed.stderr.contains("No registered workspace")
            || failed.stderr.contains("not registered")
            || failed.stderr.contains("registered"),
        "stderr={}",
        failed.stderr
    );
    assert!(!path_exists(&home.join("index.json")));
}

#[test]
fn fails_when_git_is_not_gts_link() {
    let home = isolated_home();
    let worktree = temp_dir("gts-remove-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    fs::remove_file(&git_path).unwrap();
    fs::write(&git_path, "gitdir: /tmp/not-gts\n").unwrap();

    let failed = run_cli(&["remove", "feature", "--yes"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(path_exists(&storage_track(&home, &id, "feature")));
}
