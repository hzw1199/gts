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
    let home = temp_dir("gts-menu-func-home");
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
    let result = run_cli(&["init", "main"], worktree, home);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    let obj = index.as_object().unwrap();
    obj.keys().next().unwrap().clone()
}

fn is_english(text: &str) -> bool {
    !text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

#[test]
fn menu_status_prints_human_readable_report() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    init_workspace(&worktree, &home);
    let create = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(create.code, 0, "{}", create.stderr);

    let script = r#"["status"]"#;
    let result = run_cli_env(
        &[],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(result.code, 0, "{}{}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Active: main"));
    assert!(result.stdout.contains("Link: "));
    assert!(result.stdout.contains("main") && result.stdout.contains("ACTIVE"));
    assert!(result.stdout.contains("feature"));
    assert!(!result.stdout.contains('{'));
    assert!(!result.stderr.contains("Ran out of scripted prompt answers"));
    assert!(is_english(&result.stdout));
    assert!(!home.starts_with(dirs::home_dir().unwrap().join(".gts")));
}

#[test]
fn menu_status_failure_keeps_symlink() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    let id = init_workspace(&worktree, &home);
    let git_link = worktree.join(".git");
    let before = fs::read_link(&git_link).unwrap();
    let main_git = track_git(&home, &id, "main");
    fs::rename(&main_git, format!("{}.bak", main_git.display())).unwrap();

    let script = r#"["status"]"#;
    let result = run_cli_env(
        &[],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_ne!(result.code, 0);
    assert!(is_english(&result.stderr));
    assert_eq!(fs::read_link(&git_link).unwrap(), before);
}

#[test]
fn non_tty_bare_gts_fails_with_copy_pasteable_command() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    init_workspace(&worktree, &home);

    let result = run_cli_env(&[], &worktree, &home, &[]);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts status"), "{}", result.stderr);
    let combined = format!("{}{}", result.stdout, result.stderr);
    assert!(!combined.lines().any(|l| l.starts_with("Usage:")));
    assert!(is_english(&result.stderr));
}

#[test]
fn tty_bare_gts_switch_path_not_help_first() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    init_workspace(&worktree, &home);
    let create = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(create.code, 0, "{}", create.stderr);

    let script = r#"["switch","feature"]"#;
    let result = run_cli_env(
        &[],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(result.code, 0, "{}{}", result.stderr, result.stdout);
    assert!(
        result.stdout.contains("Switched to track \"feature\"") || result.stdout.contains("gts ·")
    );
    assert!(!result.stdout.lines().any(|l| l.starts_with("Usage:")));
    assert!(!result.stderr.lines().any(|l| l.starts_with("Usage:")));
}

#[test]
fn switch_omit_name_notes_current_selects_others() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    let id = init_workspace(&worktree, &home);
    let create = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(create.code, 0, "{}", create.stderr);

    let script = r#"["feature"]"#;
    let result = run_cli_env(
        &["switch"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert!(result.stdout.contains("Switched to track \"feature\""));
    assert!(
        result.stdout.contains("current") || result.stdout.contains("Current track"),
        "expected note of current track: {}",
        result.stdout
    );
    let link = fs::canonicalize(
        fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        link,
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
}

#[test]
fn menu_create_confirm_switch_hits_dirty_fuse() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    let id = init_workspace(&worktree, &home);
    fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

    // action=create, name=feature, clone=no, switch=yes, dirty=keep
    let script = r#"["create","feature",false,true,"keep"]"#;
    let result = run_cli_env(
        &[],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(
        result.code, 0,
        "code={} stderr=[{}] stdout=[{}]",
        result.code, result.stderr, result.stdout
    );
    assert!(
        path_exists(&track_git(&home, &id, "feature").join("HEAD")),
        "feature track missing"
    );
    let link = fs::canonicalize(
        fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        link,
        fs::canonicalize(track_git(&home, &id, "feature")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(worktree.join("dirty.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn unregistered_bare_gts_enters_init() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");

    let script = r#"["main"]"#;
    let result = run_cli_env(
        &[],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(result.code, 0, "{}{}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Initialized track \"main\""));
    assert!(!result.stdout.contains("Switch track (current:"));
}

#[test]
fn menu_remove_excludes_current_and_deletes() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    let id = init_workspace(&worktree, &home);
    let create = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(create.code, 0, "{}", create.stderr);

    let script = r#"["remove","feature",true]"#;
    let result = run_cli_env(
        &[],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(result.code, 0, "{}{}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Removed track \"feature\""));
    assert!(!path_exists(&track_git(&home, &id, "feature").join("HEAD")));
}

#[test]
fn interactive_remove_omit_selects_non_active() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    let id = init_workspace(&worktree, &home);
    let create = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(create.code, 0, "{}", create.stderr);

    let script = r#"["feature",true]"#;
    let result = run_cli_env(
        &["remove"],
        &worktree,
        &home,
        &[
            ("GTS_FORCE_INTERACTIVE", "1"),
            ("GTS_PROMPT_SCRIPT", script),
        ],
    );
    assert_eq!(result.code, 0, "{}{}", result.stderr, result.stdout);
    assert!(result.stdout.contains("Removed track \"feature\""));
    assert!(!path_exists(&track_git(&home, &id, "feature")));
}

#[test]
fn non_interactive_remove_omit_fails_with_hint() {
    let home = isolated_home();
    let worktree = temp_dir("gts-menu-func-wt");
    init_workspace(&worktree, &home);
    let create = run_cli(&["create", "feature"], &worktree, &home);
    assert_eq!(create.code, 0, "{}", create.stderr);

    let result = run_cli(&["remove"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("gts remove"), "{}", result.stderr);
    assert!(path_exists(&home.join("storage")));
}
