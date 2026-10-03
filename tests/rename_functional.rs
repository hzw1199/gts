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
    let home = temp_dir("gts-rename-func-home");
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

fn track_git(home: &PathBuf, id: &str, track: &str) -> PathBuf {
    home.join("storage").join(id).join(track).join(".git")
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
fn renames_inactive_track_via_cli() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let before_link = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();
    let result = run_cli(&["rename", "feature", "feature-v2"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Renamed track \"feature\" to \"feature-v2\""));

    assert!(path_exists(&storage_track(&home, &id, "feature-v2")));
    assert!(!path_exists(&storage_track(&home, &id, "feature")));
    assert_eq!(
        fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
        before_link
    );
}

#[test]
fn renames_active_track_and_updates_link_and_active() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    let id = init_workspace(&worktree, &home);

    let result = run_cli(&["rename", "main", "trunk"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
    assert_eq!(
        fs::canonicalize(fs::read_link(&git_path).unwrap()).unwrap(),
        fs::canonicalize(track_git(&home, &id, "trunk")).unwrap()
    );

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    assert_eq!(index[&id]["active"], "trunk");
}

#[test]
fn fails_on_name_conflict_without_half_write() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);
    assert_eq!(run_cli(&["create", "other"], &worktree, &home).code, 0);

    let failed = run_cli(&["rename", "feature", "other"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("already exists"));
    assert!(path_exists(&storage_track(&home, &id, "feature")));
}

#[test]
fn rejects_invalid_names() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    init_workspace(&worktree, &home);

    let failed = run_cli(&["rename", "main", "bad/name"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("Invalid track name"));
}

#[test]
fn fails_when_git_is_not_gts_link() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    fs::remove_file(&git_path).unwrap();
    fs::write(&git_path, "gitdir: /tmp/not-gts\n").unwrap();

    let failed = run_cli(&["rename", "feature", "feature-v2"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(path_exists(&storage_track(&home, &id, "feature")));
    assert!(!path_exists(&storage_track(&home, &id, "feature-v2")));
}

#[test]
fn fails_non_interactive_when_args_missing_with_copy_pasteable_hint() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    init_workspace(&worktree, &home);

    let failed = run_cli_env(
        &["rename", "--no-interactive"],
        &worktree,
        &home,
        &[("CI", "1")],
    );
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("gts rename <from> <to>"));
}

#[test]
fn interactive_omit_args_selects_source_and_inputs_new_name() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let script = r#"["feature","feature-v2"]"#;
    let result = run_cli_env(
        &["rename"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(result.code, 0, "stderr={} stdout={}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Renamed track \"feature\" to \"feature-v2\""));
    assert!(path_exists(&storage_track(&home, &id, "feature-v2")));
    assert!(!path_exists(&storage_track(&home, &id, "feature")));
}

#[test]
fn fails_unregistered_workspace() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    fs::create_dir_all(&worktree).unwrap();

    let failed = run_cli(&["rename", "main", "trunk"], &worktree, &home);
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
fn noop_when_from_equals_to() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    init_workspace(&worktree, &home);

    let result = run_cli(&["rename", "main", "main"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Track already named \"main\""));
}

#[test]
fn fails_on_missing_source() {
    let home = isolated_home();
    let worktree = temp_dir("gts-rename-func-wt");
    let id = init_workspace(&worktree, &home);

    let failed = run_cli(&["rename", "missing", "other"], &worktree, &home);
    assert_ne!(failed.code, 0);
    assert!(failed.stderr.contains("Track not found"));
    assert!(!path_exists(&storage_track(&home, &id, "other")));
}
