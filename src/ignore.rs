use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::storage::{
    find_workspace_root_from, list_tracks, resolve_active_track, resolve_gts_home, track_git_dir,
    validate_track_name, HomeOptions, StorageError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendExcludeResult {
    pub track: String,
    pub exclude_path: PathBuf,
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenExcludeResult {
    pub track: String,
    pub exclude_path: PathBuf,
    pub editor: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintExcludeResult {
    pub track: String,
    pub exclude_path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetExcludeResult {
    pub track: String,
    pub exclude_path: PathBuf,
}

fn resolve_exclude_path(
    worktree: &Path,
    opts: &HomeOptions,
    track_name: Option<&str>,
) -> Result<(String, PathBuf), StorageError> {
    let home = resolve_gts_home(opts);
    let found = find_workspace_root_from(worktree, opts)?;
    // Preflight: .git must be a gts-managed link (reconciles active).
    let _ = resolve_active_track(&found.root, opts)?;

    let track = if let Some(name) = track_name {
        validate_track_name(name)?;
        let tracks = list_tracks(&found.id, opts)?;
        if !tracks.iter().any(|t| t == name) {
            return Err(StorageError::message(format!("Track not found: {name}")));
        }
        name.to_string()
    } else {
        resolve_active_track(&found.root, opts)?
    };

    let exclude_path = track_git_dir(&home, &found.id, &track)
        .join("info")
        .join("exclude");
    Ok((track, exclude_path))
}

fn ensure_exclude_file(exclude_path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = exclude_path.parent() {
        fs::create_dir_all(parent).map_err(StorageError::io)?;
    }
    if !exclude_path.exists() {
        fs::write(exclude_path, "").map_err(StorageError::io)?;
    }
    Ok(())
}

pub fn append_exclude_pattern(
    worktree: &Path,
    pattern: &str,
    opts: &HomeOptions,
) -> Result<AppendExcludeResult, StorageError> {
    if pattern.is_empty() {
        return Err(StorageError::message("Pattern must not be empty"));
    }

    let (track, exclude_path) = resolve_exclude_path(worktree, opts, None)?;
    ensure_exclude_file(&exclude_path)?;

    let mut existing = fs::read_to_string(&exclude_path).map_err(StorageError::io)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(pattern);
    existing.push('\n');
    fs::write(&exclude_path, existing).map_err(StorageError::io)?;

    Ok(AppendExcludeResult {
        track,
        exclude_path,
        pattern: pattern.to_string(),
    })
}

pub fn resolve_editor_command(env: &HashMap<String, String>) -> Result<String, StorageError> {
    for key in ["VISUAL", "EDITOR"] {
        if let Some(raw) = env.get(key) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    Err(StorageError::message(
        "No editor configured. Set $VISUAL or $EDITOR to open info/exclude.",
    ))
}

fn env_map_from_process() -> HashMap<String, String> {
    std::env::vars().collect()
}

pub fn open_exclude_in_editor(
    worktree: &Path,
    opts: &HomeOptions,
    env: Option<&HashMap<String, String>>,
) -> Result<OpenExcludeResult, StorageError> {
    let owned = env.map(|e| e.clone()).unwrap_or_else(env_map_from_process);
    let editor = resolve_editor_command(&owned)?;
    let (track, exclude_path) = resolve_exclude_path(worktree, opts, None)?;
    ensure_exclude_file(&exclude_path)?;

    let status = Command::new(&editor)
        .arg(&exclude_path)
        .envs(&owned)
        .status()
        .map_err(StorageError::io)?;

    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(StorageError::message(format!(
            "Editor exited with code {code}"
        )));
    }

    Ok(OpenExcludeResult {
        track,
        exclude_path,
        editor,
    })
}

pub fn print_exclude(
    worktree: &Path,
    track: &str,
    opts: &HomeOptions,
) -> Result<PrintExcludeResult, StorageError> {
    let (track, exclude_path) = resolve_exclude_path(worktree, opts, Some(track))?;
    ensure_exclude_file(&exclude_path)?;
    let content = fs::read_to_string(&exclude_path).map_err(StorageError::io)?;
    Ok(PrintExcludeResult {
        track,
        exclude_path,
        content,
    })
}

pub fn set_exclude(
    worktree: &Path,
    track: &str,
    content: &str,
    opts: &HomeOptions,
) -> Result<SetExcludeResult, StorageError> {
    let (track, exclude_path) = resolve_exclude_path(worktree, opts, Some(track))?;
    if let Some(parent) = exclude_path.parent() {
        fs::create_dir_all(parent).map_err(StorageError::io)?;
    }
    fs::write(&exclude_path, content).map_err(StorageError::io)?;
    Ok(SetExcludeResult {
        track,
        exclude_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::{init_workspace, MoveGitDirOptions};
    use crate::storage::track_git_dir;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_home() -> PathBuf {
        let home = temp_dir("gts-ignore-home");
        let real_gts = dirs::home_dir().unwrap().join(".gts");
        assert!(!home.starts_with(&real_gts));
        home
    }

    fn run_git(args: &[&str], cwd: &Path) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "gts-test")
            .env("GIT_AUTHOR_EMAIL", "gts-test@example.com")
            .env("GIT_COMMITTER_NAME", "gts-test")
            .env("GIT_COMMITTER_EMAIL", "gts-test@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_plain(worktree: &Path, home: &Path) -> String {
        run_git(&["init"], worktree);
        fs::write(worktree.join("tracked.txt"), "base\n").unwrap();
        run_git(&["add", "tracked.txt"], worktree);
        run_git(&["commit", "-m", "init"], worktree);
        let result = init_workspace(
            worktree,
            "main",
            &HomeOptions::with_home(home),
            &MoveGitDirOptions::default(),
        )
        .unwrap();
        result.id
    }

    fn fake_editor_script(log_path: &Path) -> PathBuf {
        let dir = temp_dir("gts-fake-editor");
        let file = dir.join("editor.sh");
        let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n", log_path.display());
        fs::write(&file, script).unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&file, perms).unwrap();
        file
    }

    #[test]
    fn appends_pattern_and_preserves_existing_lines() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let exclude_path = track_git_dir(&home, &id, "main")
            .join("info")
            .join("exclude");
        fs::write(&exclude_path, "# keep\n*.tmp\n").unwrap();

        let result = append_exclude_pattern(&worktree, "*.log", &opts).unwrap();
        assert_eq!(result.track, "main");
        assert_eq!(
            fs::read_to_string(&exclude_path).unwrap(),
            "# keep\n*.tmp\n*.log\n"
        );
    }

    #[test]
    fn does_not_modify_workspace_gitignore() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let gitignore = worktree.join(".gitignore");
        fs::write(&gitignore, "node_modules/\n").unwrap();
        let before = fs::read_to_string(&gitignore).unwrap();

        append_exclude_pattern(&worktree, "dist/", &opts).unwrap();
        assert_eq!(fs::read_to_string(&gitignore).unwrap(), before);
    }

    #[test]
    fn prefers_visual_over_editor() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let visual_log = temp_dir("gts-vis").join("log.txt");
        let editor_log = temp_dir("gts-ed").join("log.txt");
        let visual = fake_editor_script(&visual_log);
        let editor = fake_editor_script(&editor_log);

        let mut env = env_map_from_process();
        env.insert("VISUAL".into(), visual.to_string_lossy().into_owned());
        env.insert("EDITOR".into(), editor.to_string_lossy().into_owned());

        let result = open_exclude_in_editor(&worktree, &opts, Some(&env)).unwrap();
        assert_eq!(result.editor, visual.to_string_lossy());
        let expected = track_git_dir(&home, &id, "main")
            .join("info")
            .join("exclude");
        assert_eq!(
            fs::read_to_string(&visual_log).unwrap().trim(),
            expected.to_string_lossy()
        );
        assert!(!editor_log.exists());
    }

    #[test]
    fn falls_back_to_editor_when_visual_empty() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let editor_log = temp_dir("gts-ed2").join("log.txt");
        let editor = fake_editor_script(&editor_log);

        let mut env = env_map_from_process();
        env.insert("VISUAL".into(), String::new());
        env.insert("EDITOR".into(), editor.to_string_lossy().into_owned());

        let result = open_exclude_in_editor(&worktree, &opts, Some(&env)).unwrap();
        assert_eq!(result.editor, editor.to_string_lossy());
        let expected = track_git_dir(&home, &id, "main")
            .join("info")
            .join("exclude");
        assert_eq!(
            fs::read_to_string(&editor_log).unwrap().trim(),
            expected.to_string_lossy()
        );
    }

    #[test]
    fn fails_when_neither_visual_nor_editor_set() {
        let mut env = HashMap::new();
        env.insert("VISUAL".into(), String::new());
        let err = resolve_editor_command(&env).unwrap_err();
        assert!(err.to_string().contains("No editor configured"));

        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let mut env = env_map_from_process();
        env.remove("VISUAL");
        env.remove("EDITOR");
        let err = open_exclude_in_editor(&worktree, &opts, Some(&env)).unwrap_err();
        assert!(err.to_string().contains("No editor configured"));
    }

    #[test]
    fn fails_when_git_not_gts_managed_without_writing() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let exclude_path = track_git_dir(&home, &id, "main")
            .join("info")
            .join("exclude");
        let before = fs::read_to_string(&exclude_path).unwrap();

        let root = fs::canonicalize(&worktree).unwrap();
        let git_path = root.join(".git");
        fs::remove_file(&git_path).unwrap();
        fs::write(&git_path, "gitdir: /tmp/not-gts\n").unwrap();

        let err = append_exclude_pattern(&worktree, "*.log", &opts).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a symbolic link")
                || msg.contains("Broken")
                || msg.contains(".git"),
            "{msg}"
        );
        assert_eq!(fs::read_to_string(&exclude_path).unwrap(), before);
    }

    #[test]
    fn fails_unregistered_workspace() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-unreg");
        fs::create_dir_all(&worktree).unwrap();
        let opts = HomeOptions::with_home(&home);
        let err = append_exclude_pattern(&worktree, "*.log", &opts).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn prints_existing_exclude_for_named_track() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        crate::create::create_track(&worktree, "feature", false, &opts).unwrap();

        let exclude_path = track_git_dir(&home, &id, "feature")
            .join("info")
            .join("exclude");
        fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        fs::write(&exclude_path, "# feature\n*.bak\n").unwrap();

        let result = print_exclude(&worktree, "feature", &opts).unwrap();
        assert_eq!(result.track, "feature");
        assert_eq!(result.content, "# feature\n*.bak\n");
        assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "# feature\n*.bak\n");
    }

    #[test]
    fn print_creates_missing_exclude_and_returns_empty() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let exclude_path = track_git_dir(&home, &id, "main")
            .join("info")
            .join("exclude");
        if exclude_path.exists() {
            fs::remove_file(&exclude_path).unwrap();
        }

        let result = print_exclude(&worktree, "main", &opts).unwrap();
        assert_eq!(result.content, "");
        assert!(exclude_path.exists());
        assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "");
    }

    #[test]
    fn print_unknown_track_fails_without_writing() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let missing = track_git_dir(&home, &id, "missing")
            .join("info")
            .join("exclude");
        let err = print_exclude(&worktree, "missing", &opts).unwrap_err();
        assert!(err.to_string().contains("Track not found"));
        assert!(!missing.exists());
    }

    #[test]
    fn set_replaces_whole_file() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        crate::create::create_track(&worktree, "feature", false, &opts).unwrap();

        let exclude_path = track_git_dir(&home, &id, "feature")
            .join("info")
            .join("exclude");
        fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        fs::write(&exclude_path, "old\n").unwrap();

        set_exclude(&worktree, "feature", "# new\n*.log\n", &opts).unwrap();
        assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "# new\n*.log\n");
    }

    #[test]
    fn set_empty_stdin_clears_exclude() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let exclude_path = track_git_dir(&home, &id, "main")
            .join("info")
            .join("exclude");
        fs::write(&exclude_path, "keep\n").unwrap();

        set_exclude(&worktree, "main", "", &opts).unwrap();
        assert!(exclude_path.exists());
        assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "");
    }

    #[test]
    fn set_refuses_non_gts_git_without_writing() {
        let home = temp_home();
        let worktree = temp_dir("gts-ignore-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let exclude_path = track_git_dir(&home, &id, "main")
            .join("info")
            .join("exclude");
        fs::write(&exclude_path, "keep\n").unwrap();

        let root = fs::canonicalize(&worktree).unwrap();
        let git_path = root.join(".git");
        fs::remove_file(&git_path).unwrap();
        fs::write(&git_path, "gitdir: /tmp/not-gts\n").unwrap();

        let err = set_exclude(&worktree, "main", "new\n", &opts).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a symbolic link")
                || msg.contains("Broken")
                || msg.contains(".git"),
            "{msg}"
        );
        assert_eq!(fs::read_to_string(&exclude_path).unwrap(), "keep\n");
    }
}
