use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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
    let home = temp_dir("gts-ignore-func-home");
    let real_gts = dirs::home_dir().unwrap().join(".gts");
    assert!(!home.starts_with(&real_gts));
    home
}

fn run_cli_env(args: &[&str], cwd: &Path, home: &Path, extra: &[(&str, &str)]) -> RunResult {
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

fn run_cli(args: &[&str], cwd: &Path, home: &Path) -> RunResult {
    run_cli_env(args, cwd, home, &[("CI", "1")])
}

fn run_git(args: &[&str], cwd: &Path) -> (i32, String, String) {
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

fn track_exclude(home: &Path, id: &str, track: &str) -> PathBuf {
    home.join("storage")
        .join(id)
        .join(track)
        .join(".git")
        .join("info")
        .join("exclude")
}

fn init_workspace(worktree: &Path, home: &Path) -> String {
    let (code, _, stderr) = run_git(&["init"], worktree);
    assert_eq!(code, 0, "git init: {stderr}");
    fs::write(worktree.join("tracked.txt"), "base\n").unwrap();
    let (code, _, stderr) = run_git(&["add", "tracked.txt"], worktree);
    assert_eq!(code, 0, "git add: {stderr}");
    let (code, _, stderr) = run_git(&["commit", "-m", "init"], worktree);
    assert_eq!(code, 0, "git commit: {stderr}");
    let init = run_cli(&["init", "main", "--no-interactive"], worktree, home);
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

fn fake_editor_script(log_path: &Path) -> PathBuf {
    let dir = temp_dir("gts-ignore-func-ed");
    let file = dir.join("editor.sh");
    let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n", log_path.display());
    fs::write(&file, script).unwrap();
    let mut perms = fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&file, perms).unwrap();
    file
}

#[test]
fn appends_pattern_and_preserves_existing_exclude() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let exclude_path = track_exclude(&home, &id, "main");
    fs::write(&exclude_path, "# keep\n*.tmp\n").unwrap();

    let result = run_cli(
        &["ignore", "*.log", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_eq!(result.code, 0, "stderr={} stdout={}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Appended \"*.log\""));
    assert_eq!(
        fs::read_to_string(&exclude_path).unwrap(),
        "# keep\n*.tmp\n*.log\n"
    );
}

#[test]
fn does_not_modify_workspace_gitignore() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    init_workspace(&worktree, &home);

    let gitignore = worktree.join(".gitignore");
    fs::write(&gitignore, "node_modules/\n").unwrap();

    let result = run_cli(
        &["ignore", "dist/", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert_eq!(fs::read_to_string(&gitignore).unwrap(), "node_modules/\n");
}

#[test]
fn opens_exclude_with_visual_preferred_over_editor() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let visual_log = temp_dir("gts-vis").join("log.txt");
    let editor_log = temp_dir("gts-ed").join("log.txt");
    let visual = fake_editor_script(&visual_log);
    let editor = fake_editor_script(&editor_log);

    let result = run_cli_env(
        &["ignore", "--no-interactive"],
        &worktree,
        &home,
        &[
            ("CI", "1"),
            ("VISUAL", visual.to_str().unwrap()),
            ("EDITOR", editor.to_str().unwrap()),
        ],
    );
    assert_eq!(result.code, 0, "stderr={} stdout={}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Opened info/exclude"));
    assert_eq!(
        fs::read_to_string(&visual_log).unwrap().trim(),
        track_exclude(&home, &id, "main").to_string_lossy()
    );
    assert!(!editor_log.exists());
}

#[test]
fn fails_when_neither_visual_nor_editor_set() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    init_workspace(&worktree, &home);

    let result = run_cli_env(
        &["ignore", "--no-interactive"],
        &worktree,
        &home,
        &[("CI", "1"), ("VISUAL", ""), ("EDITOR", "")],
    );
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("No editor configured"));
}

#[test]
fn fails_on_broken_or_non_gts_git_without_writing() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let exclude_path = track_exclude(&home, &id, "main");
    let before = fs::read_to_string(&exclude_path).unwrap();

    let wt = fs::canonicalize(&worktree).unwrap();
    fs::remove_file(wt.join(".git")).unwrap();
    fs::write(wt.join(".git"), "gitdir: /tmp/not-gts\n").unwrap();

    let result = run_cli(
        &["ignore", "*.log", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_ne!(result.code, 0);
    let msg = format!("{}{}", result.stderr, result.stdout);
    assert!(
        msg.contains("not a symbolic link") || msg.contains("Broken") || msg.contains(".git"),
        "{msg}"
    );
    assert_eq!(fs::read_to_string(&exclude_path).unwrap(), before);
}

#[test]
fn fails_unregistered_workspace() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-unreg");
    fs::create_dir_all(&worktree).unwrap();

    let result = run_cli(
        &["ignore", "*.log", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_ne!(result.code, 0);
    assert!(!result.stderr.is_empty() || !result.stdout.is_empty());
    assert!(!home.join("index.json").exists());
}

fn run_cli_stdin(args: &[&str], cwd: &Path, home: &Path, stdin: &str) -> RunResult {
    use std::io::Write;
    use std::process::Stdio;

    let mut cmd = Command::new(gts_bin());
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(user_home) = std::env::var("HOME") {
        cmd.env("HOME", user_home);
    }
    cmd.env("GTS_HOME", home);
    cmd.env("CI", "1");
    let mut child = cmd.spawn().expect("spawn gts");
    {
        let mut input = child.stdin.take().expect("stdin");
        input.write_all(stdin.as_bytes()).unwrap();
    }
    let output = child.wait_with_output().expect("wait gts");
    RunResult {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn prints_existing_exclude_for_inactive_track() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let create = run_cli(
        &["create", "feature", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_eq!(create.code, 0, "stderr={}", create.stderr);

    let exclude_path = track_exclude(&home, &id, "feature");
    fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
    fs::write(&exclude_path, "# feature\n*.bak\n").unwrap();

    let gitignore = worktree.join(".gitignore");
    fs::write(&gitignore, "node_modules/\n").unwrap();

    let result = run_cli(
        &["ignore", "--track", "feature", "--print", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert_eq!(result.stdout, "# feature\n*.bak\n");
    assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "# feature\n*.bak\n");
    assert_eq!(fs::read_to_string(&gitignore).unwrap(), "node_modules/\n");
}

#[test]
fn print_creates_missing_exclude_empty() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let exclude_path = track_exclude(&home, &id, "main");
    if exclude_path.exists() {
        fs::remove_file(&exclude_path).unwrap();
    }

    let result = run_cli(
        &["ignore", "--track", "main", "--print", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert_eq!(result.stdout, "");
    assert!(exclude_path.exists());
    assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "");
}

#[test]
fn print_unknown_track_fails_without_writing() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let missing = track_exclude(&home, &id, "missing");
    let result = run_cli(
        &["ignore", "--track", "missing", "--print", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("Track not found") || result.stdout.contains("Track not found"));
    assert!(!missing.exists());
}

#[test]
fn set_replaces_whole_file_for_named_track() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let create = run_cli(
        &["create", "feature", "--no-interactive"],
        &worktree,
        &home,
    );
    assert_eq!(create.code, 0, "stderr={}", create.stderr);

    let exclude_path = track_exclude(&home, &id, "feature");
    fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
    fs::write(&exclude_path, "old\n").unwrap();

    let gitignore = worktree.join(".gitignore");
    fs::write(&gitignore, "node_modules/\n").unwrap();

    let result = run_cli_stdin(
        &["ignore", "--track", "feature", "--set", "--no-interactive"],
        &worktree,
        &home,
        "# new\n*.log\n",
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Updated info/exclude for track \"feature\""));
    assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "# new\n*.log\n");
    assert_eq!(fs::read_to_string(&gitignore).unwrap(), "node_modules/\n");
}

#[test]
fn set_empty_stdin_clears_exclude() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let exclude_path = track_exclude(&home, &id, "main");
    fs::write(&exclude_path, "keep\n").unwrap();

    let result = run_cli_stdin(
        &["ignore", "--track", "main", "--set", "--no-interactive"],
        &worktree,
        &home,
        "",
    );
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(exclude_path.exists());
    assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "");
}

#[test]
fn set_refuses_non_gts_git_without_writing() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    let id = init_workspace(&worktree, &home);

    let exclude_path = track_exclude(&home, &id, "main");
    fs::write(&exclude_path, "keep\n").unwrap();

    let wt = fs::canonicalize(&worktree).unwrap();
    fs::remove_file(wt.join(".git")).unwrap();
    fs::write(wt.join(".git"), "gitdir: /tmp/not-gts\n").unwrap();

    let result = run_cli_stdin(
        &["ignore", "--track", "main", "--set", "--no-interactive"],
        &worktree,
        &home,
        "new\n",
    );
    assert_ne!(result.code, 0);
    let msg = format!("{}{}", result.stderr, result.stdout);
    assert!(
        msg.contains("not a symbolic link") || msg.contains("Broken") || msg.contains(".git"),
        "{msg}"
    );
    assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "keep\n");
}

#[test]
fn print_and_set_are_mutually_exclusive_with_pattern() {
    let home = isolated_home();
    let worktree = temp_dir("gts-ignore-func-wt");
    init_workspace(&worktree, &home);

    let both = run_cli(
        &[
            "ignore",
            "--track",
            "main",
            "--print",
            "--set",
            "--no-interactive",
        ],
        &worktree,
        &home,
    );
    assert_ne!(both.code, 0);
    assert!(
        both.stderr.contains("Use only one of --print or --set")
            || both.stdout.contains("Use only one of --print or --set")
    );

    let with_pattern = run_cli(
        &[
            "ignore",
            "*.log",
            "--track",
            "main",
            "--print",
            "--no-interactive",
        ],
        &worktree,
        &home,
    );
    assert_ne!(with_pattern.code, 0);
    assert!(
        with_pattern
            .stderr
            .contains("Do not pass a pattern with --print or --set")
            || with_pattern
                .stdout
                .contains("Do not pass a pattern with --print or --set")
    );
}
