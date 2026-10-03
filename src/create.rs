use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::storage::{
    assert_track_name_available, find_workspace_root_from, resolve_active_track, resolve_gts_home,
    track_git_dir, validate_track_name, HomeOptions, StorageError,
};
use crate::switch::{switch_track, SwitchTrackInput, SwitchTrackResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTrackResult {
    pub id: String,
    pub track: String,
    pub worktree: PathBuf,
    pub git_dir: PathBuf,
}

fn path_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok() || fs::symlink_metadata(path).is_ok()
}

fn run_git_init(track_dir: &Path) -> Result<(), StorageError> {
    let output = Command::new("git")
        .args(["init"])
        .current_dir(track_dir)
        .output()
        .map_err(StorageError::io)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StorageError::message(format!(
            "git init failed (exit {:?}): {}",
            output.status.code(),
            stderr.trim()
        )));
    }
    Ok(())
}

fn assert_no_alternates(source_git: &Path) -> Result<(), StorageError> {
    let alternates = source_git.join("objects").join("info").join("alternates");
    if path_exists(&alternates) {
        return Err(StorageError::message(format!(
            "Cannot clone track: source has objects/info/alternates ({})",
            alternates.display()
        )));
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(entry.path())?;
            symlink(target, to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

pub fn create_track(
    worktree: &Path,
    track: &str,
    clone: bool,
    opts: &HomeOptions,
) -> Result<CreateTrackResult, StorageError> {
    validate_track_name(track)?;
    let home = resolve_gts_home(opts);
    let found = find_workspace_root_from(worktree, opts)?;
    let id = found.id;
    let worktree = found.root;
    assert_track_name_available(&id, track, opts)?;

    // Require a gts-managed .git symlink (also yields the active track for --clone).
    let active = resolve_active_track(&worktree, opts)?;

    let dest_git = track_git_dir(&home, &id, track);
    let track_dir = dest_git.parent().ok_or_else(|| {
        StorageError::message(format!("Invalid track git path: {}", dest_git.display()))
    })?;

    let mut created = false;
    let finish = (|| -> Result<CreateTrackResult, StorageError> {
        if clone {
            let source_git = track_git_dir(&home, &id, &active);
            if !path_exists(&source_git) {
                return Err(StorageError::message(format!(
                    "Current track Git directory missing: {}",
                    source_git.display()
                )));
            }
            assert_no_alternates(&source_git)?;
            fs::create_dir_all(track_dir)?;
            created = true;
            if path_exists(&dest_git) {
                return Err(StorageError::message(format!(
                    "Track git directory already exists: {}",
                    dest_git.display()
                )));
            }
            copy_dir_all(&source_git, &dest_git).map_err(StorageError::io)?;
        } else {
            fs::create_dir_all(track_dir)?;
            created = true;
            run_git_init(track_dir)?;
            if !path_exists(&dest_git) {
                return Err(StorageError::message(format!(
                    "git init did not create {}",
                    dest_git.display()
                )));
            }
        }

        let abs_dest = fs::canonicalize(&dest_git).map_err(StorageError::io)?;
        Ok(CreateTrackResult {
            id: id.clone(),
            track: track.to_string(),
            worktree: worktree.clone(),
            git_dir: abs_dest,
        })
    })();

    match finish {
        Ok(result) => Ok(result),
        Err(err) => {
            if created {
                let _ = fs::remove_dir_all(track_dir);
            }
            Err(err)
        }
    }
}

pub fn switch_to_new_track(
    worktree: &Path,
    track: &str,
    opts: &HomeOptions,
    force: bool,
    stash: bool,
    no_interactive: bool,
) -> Result<SwitchTrackResult, StorageError> {
    switch_track(SwitchTrackInput {
        worktree,
        track,
        force,
        stash,
        no_interactive,
        interactive: None,
        home: opts,
        resolve_dirty: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::{init_workspace, MoveGitDirOptions};
    use crate::storage::{read_index, track_git_dir};
    use crate::switch::SwitchTrackResult;

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
        let home = temp_dir("gts-create-home");
        let real_gts = dirs::home_dir().unwrap().join(".gts");
        assert!(!home.starts_with(&real_gts));
        home
    }

    fn init_plain(worktree: &Path, home: &Path) -> String {
        fs::create_dir_all(worktree.join(".git")).unwrap();
        fs::write(worktree.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(worktree.join(".git").join("marker"), "from-main\n").unwrap();
        let opts = HomeOptions::with_home(home);
        let result =
            init_workspace(worktree, "main", &opts, &MoveGitDirOptions::default()).unwrap();
        result.id
    }

    #[test]
    fn empty_create_inits_under_storage_without_changing_worktree() {
        let home = temp_home();
        let worktree = temp_dir("gts-create-wt");
        fs::write(worktree.join("keep.txt"), "unchanged\n").unwrap();
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        let before_link = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

        let result = create_track(&worktree, "feature", false, &opts).unwrap();
        assert_eq!(result.track, "feature");
        assert_eq!(
            fs::canonicalize(&result.git_dir).unwrap(),
            fs::canonicalize(track_git_dir(&home, &id, "feature")).unwrap()
        );
        assert!(path_exists(&result.git_dir.join("HEAD")));
        assert_eq!(
            fs::read_to_string(worktree.join("keep.txt")).unwrap(),
            "unchanged\n"
        );
        assert_eq!(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
            before_link
        );
        assert_eq!(resolve_active_track(&worktree, &opts).unwrap(), "main");
    }

    #[test]
    fn clone_copies_current_track_independently() {
        let home = temp_home();
        let worktree = temp_dir("gts-create-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let result = create_track(&worktree, "other", true, &opts).unwrap();
        let dest_marker = result.git_dir.join("marker");
        assert_eq!(fs::read_to_string(&dest_marker).unwrap(), "from-main\n");
        fs::write(&dest_marker, "cloned-edit\n").unwrap();
        assert_eq!(
            fs::read_to_string(track_git_dir(&home, &id, "main").join("marker")).unwrap(),
            "from-main\n"
        );
        assert_eq!(resolve_active_track(&worktree, &opts).unwrap(), "main");
    }

    #[test]
    fn clone_fails_on_alternates_without_half_write() {
        let home = temp_home();
        let worktree = temp_dir("gts-create-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        let alt_dir = track_git_dir(&home, &id, "main")
            .join("objects")
            .join("info");
        fs::create_dir_all(&alt_dir).unwrap();
        fs::write(alt_dir.join("alternates"), "/tmp/other-objects\n").unwrap();

        let err = create_track(&worktree, "other", true, &opts).unwrap_err();
        assert!(err.to_string().contains("alternates"));
        assert!(!path_exists(&home.join("storage").join(&id).join("other")));
    }

    #[test]
    fn rejects_duplicate_track_name() {
        let home = temp_home();
        let worktree = temp_dir("gts-create-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        let err = create_track(&worktree, "feature", false, &opts).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn rejects_invalid_track_name() {
        let home = temp_home();
        let worktree = temp_dir("gts-create-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        for name in ["", ".", "..", "a/b", "a\\b"] {
            let err = create_track(&worktree, name, false, &opts).unwrap_err();
            assert!(err.to_string().contains("Invalid track name"));
        }
    }

    #[test]
    fn fails_when_workspace_unregistered() {
        let home = temp_home();
        let worktree = temp_dir("gts-create-wt");
        let opts = HomeOptions::with_home(&home);
        let err = create_track(&worktree, "feature", false, &opts).unwrap_err();
        assert!(err.to_string().contains("No registered workspace"));
    }

    #[test]
    fn switch_to_new_track_retargets_symlink_and_active() {
        let home = temp_home();
        let worktree = temp_dir("gts-create-wt");
        // Real git repo required: switch runs `git status --porcelain`.
        let output = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&worktree)
            .output()
            .unwrap();
        assert!(output.status.success());
        fs::write(worktree.join("tracked.txt"), "base\n").unwrap();
        let git_cmds: &[&[&str]] = &[&["add", "tracked.txt"], &["commit", "-m", "init"]];
        for args in git_cmds {
            let output = std::process::Command::new("git")
                .args(*args)
                .current_dir(&worktree)
                .env("GIT_AUTHOR_NAME", "gts-test")
                .env("GIT_AUTHOR_EMAIL", "gts-test@example.com")
                .env("GIT_COMMITTER_NAME", "gts-test")
                .env("GIT_COMMITTER_EMAIL", "gts-test@example.com")
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        fs::write(worktree.join(".git").join("marker"), "from-main\n").unwrap();
        let opts = HomeOptions::with_home(&home);
        let id = init_workspace(&worktree, "main", &opts, &MoveGitDirOptions::default())
            .unwrap()
            .id;
        create_track(&worktree, "feature", false, &opts).unwrap();

        let result = switch_to_new_track(&worktree, "feature", &opts, false, false, true).unwrap();
        assert!(matches!(
            result,
            SwitchTrackResult::Switched {
                ref from,
                ref to
            } if from == "main" && to == "feature"
        ));

        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        let meta = fs::symlink_metadata(&git_path).unwrap();
        assert!(meta.file_type().is_symlink());
        let target = fs::read_link(&git_path).unwrap();
        assert!(target.is_absolute());
        assert_eq!(
            fs::canonicalize(&target).unwrap(),
            fs::canonicalize(track_git_dir(&home, &id, "feature")).unwrap()
        );
        assert_eq!(read_index(&opts).unwrap().get(&id).unwrap().active, "feature");
    }
}
