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
    let home = temp_dir("gts-status-func-home");
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
fn shows_active_track_and_list_after_init() {
    let home = isolated_home();
    let worktree = temp_dir("gts-status-func-wt");
    fs::create_dir_all(worktree.join(".git")).unwrap();
    fs::write(worktree.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let init = run_cli(&["init", "main"], &worktree, &home);
    assert_eq!(init.code, 0, "stderr={}", init.stderr);

    let result = run_cli(&["status"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    assert!(result.stdout.contains("Active: main"));
    assert!(result.stdout.contains("main  ACTIVE"));
    assert!(result.stdout.contains("Link: "));
    assert!(is_english(&format!("{}{}", result.stdout, result.stderr)));
}

#[test]
fn outputs_json_shape() {
    let home = isolated_home();
    let worktree = temp_dir("gts-status-func-wt");
    fs::create_dir_all(worktree.join(".git")).unwrap();
    fs::write(worktree.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let init = run_cli(&["init", "main"], &worktree, &home);
    assert_eq!(init.code, 0, "stderr={}", init.stderr);

    let result = run_cli(&["status", "--json"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    let payload: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
    let obj = payload.as_object().unwrap();
    assert_eq!(obj.len(), 5);
    assert!(obj["id"].is_string());
    assert_eq!(
        obj["path"].as_str().unwrap(),
        fs::canonicalize(&worktree).unwrap().to_string_lossy()
    );
    assert_eq!(obj["active"], "main");
    assert!(PathBuf::from(obj["link"].as_str().unwrap()).is_absolute());
    let tracks = obj["tracks"].as_array().unwrap();
    assert!(tracks.iter().any(|t| t == "main"));
}

#[test]
fn fails_when_unregistered() {
    let home = isolated_home();
    let worktree = temp_dir("gts-status-func-wt");
    let result = run_cli(&["status"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("No registered workspace found"));
    assert!(is_english(&result.stderr));
}

#[test]
fn corrects_drifted_active() {
    let home = isolated_home();
    let worktree = temp_dir("gts-status-func-wt");
    fs::create_dir_all(worktree.join(".git")).unwrap();
    fs::write(worktree.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let init = run_cli(&["init", "main"], &worktree, &home);
    assert_eq!(init.code, 0, "stderr={}", init.stderr);

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    let id = index.as_object().unwrap().keys().next().unwrap().clone();
    let feature_git = home
        .join("storage")
        .join(&id)
        .join("feature-v1")
        .join(".git");
    fs::create_dir_all(&feature_git).unwrap();

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    let abs_feature = fs::canonicalize(&feature_git).unwrap();
    let temp = git_path.with_file_name(format!(".git.gts-tmp-status-{}", uuid::Uuid::new_v4()));
    std::os::unix::fs::symlink(&abs_feature, &temp).unwrap();
    fs::rename(&temp, &git_path).unwrap();

    let before: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    assert_eq!(before[&id]["active"], "main");

    let result = run_cli(&["status", "--json"], &worktree, &home);
    assert_eq!(result.code, 0, "stderr={}", result.stderr);
    let payload: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
    assert_eq!(payload["active"], "feature-v1");

    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join("index.json")).unwrap()).unwrap();
    assert_eq!(after[&id]["active"], "feature-v1");
}

#[test]
fn fails_on_broken_link() {
    let home = isolated_home();
    let worktree = temp_dir("gts-status-func-wt");
    fs::create_dir_all(worktree.join(".git")).unwrap();
    fs::write(worktree.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let init = run_cli(&["init", "main"], &worktree, &home);
    assert_eq!(init.code, 0, "stderr={}", init.stderr);

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    fs::remove_file(&git_path).unwrap();
    std::os::unix::fs::symlink(worktree.join("missing-target"), &git_path).unwrap();

    let result = run_cli(&["status"], &worktree, &home);
    assert_ne!(result.code, 0);
    assert!(result.stderr.contains("Broken .git symlink"));
    assert!(is_english(&result.stderr));
}
