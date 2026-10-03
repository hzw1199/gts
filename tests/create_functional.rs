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
    let home = temp_dir("gts-create-func-home");
    let real_gts = dirs::home_dir().unwrap().join(".gts");
    assert!(!home.starts_with(&real_gts));
    home
}

fn run_cli(args: &[&str], cwd: &PathBuf, home: &PathBuf) -> RunResult {
    let mut cmd = Command::new(gts_bin());
    cmd.args(args).current_dir(cwd).env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(user_home) = std::env::var("HOME") {
        cmd.env("HOME", user_home);
    }
    cmd.env("GTS_HOME", home);
    cmd.env("CI", "1");
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

fn path_exists(path: &std::path::Path) -> bool {
    fs::metadata(path).is_ok() || fs::symlink_metadata(path).is_ok()
}

fn init_workspace(worktree: &PathBuf, home: &PathBuf) -> String {
    let gi = Command::new("git")
        .args(["init"])
        .current_dir(worktree)
        .output()
        .unwrap();
    assert!(gi.status.success(), "git init: {}", String::from_utf8_lossy(&gi.stderr));
    fs::write(worktree.join("keep.txt"), "worktree-file\n").unwrap();
    fs::write(worktree.join("tracked.txt"), "base\n").unwrap();
    for args in [
        vec!["add", "keep.txt", "tracked.txt"],
        vec!["commit", "-m", "init"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(worktree)
            .env("GIT_AUTHOR_NAME", "gts-test")
            .env("GIT_AUTHOR_EMAIL", "gts-test@example.com")
            .env("GIT_COMMITTER_NAME", "gts-test")
            .env("GIT_COMMITTER_EMAIL", "gts-test@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fs::write(worktree.join(".git").join("marker"), "from-main\n").unwrap();
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

fn track_git(home: &PathBuf, id: &str, track: &str) -> PathBuf {
    home.join("storage").join(id).join(track).join(".git")
}

#[test]
fn empty_create_inits_in_storage_and_leaves_worktree() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    let id = init_workspace(&worktree, &home);
    let before_link = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

    let result = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Created track \"feature\""));
    assert!(is_english(&format!("{}{}", result.stdout, result.stderr)));

    assert!(path_exists(&track_git(&home, &id, "feature").join("HEAD")));
    assert_eq!(
        fs::read_to_string(worktree.join("keep.txt")).unwrap(),
        "worktree-file\n"
    );
    assert_eq!(
        fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
        before_link
    );
    assert!(!home.starts_with(dirs::home_dir().unwrap().join(".gts")));
}

#[test]
fn clone_copies_current_track() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    let id = init_workspace(&worktree, &home);

    let result = run_cli(&["create", "other", "--clone"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert_eq!(
        fs::read_to_string(track_git(&home, &id, "other").join("marker")).unwrap(),
        "from-main\n"
    );
}

#[test]
fn clone_with_alternates_fails_without_half_write() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    let id = init_workspace(&worktree, &home);
    let alt_dir = track_git(&home, &id, "main")
        .join("objects")
        .join("info");
    fs::create_dir_all(&alt_dir).unwrap();
    fs::write(alt_dir.join("alternates"), "/tmp/other-objects\n").unwrap();

    let result = run_cli(&["create", "other", "--clone"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("alternates"));
    assert!(!path_exists(&home.join("storage").join(&id).join("other")));
}

#[test]
fn switch_retargets_symlink_only() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    let id = init_workspace(&worktree, &home);

    let result = run_cli(&["create", "feature", "--switch"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result
        .stdout
        .contains("Created and switched to track \"feature\""));

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    let meta = fs::symlink_metadata(&git_path).unwrap();
    assert!(meta.file_type().is_symlink());
    let target = fs::read_link(&git_path).unwrap();
    assert_eq!(
        fs::canonicalize(&target).unwrap(),
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    assert_eq!(index[&id]["active"], "feature");
    assert_eq!(
        fs::read_to_string(worktree.join("keep.txt")).unwrap(),
        "worktree-file\n"
    );
}

#[test]
fn non_interactive_without_switch_does_not_switch() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    let id = init_workspace(&worktree, &home);

    let result = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Created track \"feature\""));
    assert!(!result.stdout.contains("switched"));

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    let target = fs::read_link(&git_path).unwrap();
    assert_eq!(
        fs::canonicalize(&target).unwrap(),
        fs::canonicalize(track_git(&home, &id, "main")).unwrap()
    );
}

#[test]
fn non_interactive_missing_name_fails_with_hint() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    init_workspace(&worktree, &home);

    let result = run_cli(&["create"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts create <track>"));
}

#[test]
fn unregistered_workspace_fails() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    let result = run_cli(&["create", "feature"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(
        result.stderr.contains("No registered workspace")
            || result.stderr.to_lowercase().contains("not registered")
    );
}

#[test]
fn invalid_and_duplicate_names_fail() {
    let home = isolated_home();
    let worktree = temp_dir("gts-create-func-wt");
    init_workspace(&worktree, &home);

    let invalid = run_cli(&["create", "a/b"], &worktree, &home);
    assert_ne!(invalid.code, 0);
    assert!(invalid.stderr.contains("Invalid track name"));

    let first = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(first.code, 0, "stderr={}", first.stderr);
    let dup = run_cli(&["create", "feature"], &worktree, &home);
    assert_ne!(dup.code, 0);
    assert!(dup.stderr.contains("already exists"));
}
