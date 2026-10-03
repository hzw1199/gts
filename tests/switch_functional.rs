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
    let home = temp_dir("gts-switch-func-home");
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

fn init_workspace(worktree: &PathBuf, home: &PathBuf) -> String {
    let (code, _, stderr) = run_git(&["init"], worktree);
    assert_eq!(code, 0, "git init: {stderr}");
    fs::write(worktree.join("keep.txt"), "worktree-file\n").unwrap();
    fs::write(worktree.join("tracked.txt"), "base\n").unwrap();
    let (code, _, stderr) = run_git(&["add", "keep.txt", "tracked.txt"], worktree);
    assert_eq!(code, 0, "git add: {stderr}");
    let (code, _, stderr) = run_git(&["commit", "-m", "init"], worktree);
    assert_eq!(code, 0, "git commit: {stderr}");
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

#[test]
fn switch_to_current_succeeds_as_noop() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    init_workspace(&worktree, &home);
    let before = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

    let result = run_cli(&["switch", "main"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Already on track \"main\""));
    assert_eq!(
        fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
        before
    );
}

#[test]
fn to_alias_retargets_link_without_changing_worktree_files() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let result = run_cli(&["to", "feature"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Switched to track \"feature\""));

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
    assert_eq!(
        fs::canonicalize(fs::read_link(&git_path).unwrap()).unwrap(),
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(worktree.join("keep.txt")).unwrap(),
        "worktree-file\n"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
        "base\n"
    );
    assert!(!home.starts_with(dirs::home_dir().unwrap().join(".gts")));
}

#[test]
fn non_interactive_dirty_without_flags_fails() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let result = run_cli(&["switch", "feature"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(
        result.stderr.contains("uncommitted changes")
            || result.stderr.contains("--force")
            || result.stderr.contains("--stash")
    );
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "main")).unwrap()
    );
}

#[test]
fn force_switches_with_dirty_worktree() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let result = run_cli(&["switch", "feature", "--force"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert_eq!(
        fs::read_to_string(worktree.join("dirty.txt")).unwrap(),
        "dirty\n"
    );
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
}

#[test]
fn stash_flag_stashes_then_switches() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let result = run_cli(&["switch", "feature", "--stash"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(!path_exists(&worktree.join("dirty.txt")));
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );

    assert_eq!(
        run_cli(&["switch", "main", "--force"], &worktree, &home).code,
        0
    );
    let (code, stdout, _) = run_git(&["stash", "list"], &worktree);
    assert_eq!(code, 0);
    assert!(stdout.contains("stash before switch from main"));
}

#[test]
fn force_and_stash_together_fail() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let result = run_cli(&["switch", "feature", "--force", "--stash"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("--force and --stash"));
}

#[test]
fn missing_track_name_fails_with_hint() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    init_workspace(&worktree, &home);

    let result = run_cli(&["switch"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts switch <track>"));
}

#[test]
fn missing_target_track_fails_without_changing_link() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);

    let result = run_cli(&["switch", "nope"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("Track not found"));
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "main")).unwrap()
    );
}

#[test]
fn interactive_dirty_abort_leaves_link() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let script = serde_json::json!(["abort"]).to_string();
    let result = run_cli_env(
        &["switch", "feature"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", &script),
        ],
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "main")).unwrap()
    );
}

#[test]
fn interactive_dirty_stash_then_switches() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let script = serde_json::json!(["stash"]).to_string();
    let result = run_cli_env(
        &["switch", "feature"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", &script),
        ],
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(!path_exists(&worktree.join("dirty.txt")));
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
}

#[test]
fn omit_track_interactive_notes_and_selects() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);

    let script = serde_json::json!(["feature"]).to_string();
    let result = run_cli_env(
        &["switch"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", &script),
        ],
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(
        result.stdout.contains("main") && result.stdout.contains("current"),
        "stdout={}",
        result.stdout
    );
    assert!(result.stdout.contains("Switched to track \"feature\""));
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
}

#[test]
fn omit_track_fails_when_no_other_tracks() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    init_workspace(&worktree, &home);

    let result = run_cli_env(
        &["switch"],
        &worktree,
        &home,
        &[("GTS_FORCE_INTERACTIVE", "1")],
    );
    assert_ne!(result.code, 0);
    assert!(
        result.stderr.contains("No other tracks") || result.stdout.contains("No other tracks"),
        "stderr={} stdout={}",
        result.stderr,
        result.stdout
    );
}

#[test]
fn create_switch_dirty_without_flags_fails_and_leaves_track() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let result = run_cli(&["create", "feature", "--switch"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(
        result.stderr.contains("uncommitted changes")
            || result.stderr.contains("--force")
            || result.stderr.contains("--stash")
    );
    assert!(path_exists(&track_git(&home, &id, "feature").join("HEAD")));
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "main")).unwrap()
    );
}

#[test]
fn create_switch_force_switches_when_dirty() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let result = run_cli(
        &["create", "feature", "--switch", "--force"],
        &worktree,
        &home,
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result
        .stdout
        .contains("Created and switched to track \"feature\""));
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(worktree.join("dirty.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn stash_failure_does_not_switch() {
    let home = isolated_home();
    let worktree = temp_dir("gts-switch-func-wt");
    let id = init_workspace(&worktree, &home);
    assert_eq!(run_cli(&["create", "feature"], &worktree, &home).code, 0);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    let git_real = fs::canonicalize(
        fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
    )
    .unwrap();
    // Keep status readable; block stash from writing refs.
    let mut perms = fs::metadata(&git_real).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&git_real, perms.clone()).unwrap();
    // Also clear write on nested dirs git needs for stash.
    for entry in walkdir_readonly(&git_real) {
        let mut p = fs::metadata(&entry).unwrap().permissions();
        p.set_readonly(true);
        let _ = fs::set_permissions(&entry, p);
    }

    let result = run_cli(&["switch", "feature", "--stash"], &worktree, &home);

    // Restore writability for cleanup.
    clear_readonly_tree(&git_real);

    assert_ne!(result.code, 0, "stdout={} stderr={}", result.stdout, result.stderr);
    assert!(
        result.stderr.to_lowercase().contains("stash")
            || result.stderr.contains("Permission denied")
            || result.stderr.contains("Read-only"),
        "stderr={}",
        result.stderr
    );
    assert_eq!(
        fs::canonicalize(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
        )
        .unwrap(),
        fs::canonicalize(track_git(&home, &id, "main")).unwrap()
    );
}

fn walkdir_readonly(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(path.clone());
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            }
        }
    }
    out
}

fn clear_readonly_tree(root: &std::path::Path) {
    for path in walkdir_readonly(root) {
        let mut p = match fs::metadata(&path) {
            Ok(m) => m.permissions(),
            Err(_) => continue,
        };
        p.set_readonly(false);
        let _ = fs::set_permissions(&path, p);
    }
}
