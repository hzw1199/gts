use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::storage::{
    assert_destination_available, find_workspace_root_from, list_tracks, read_index,
    resolve_active_track, resolve_gts_home, track_git_dir, validate_track_name,
    workspace_storage_dir, write_index, HomeOptions, StorageError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTrackResult {
    Renamed {
        from: String,
        to: String,
        active: bool,
    },
    Noop {
        track: String,
    },
}

fn rename_track_dir(storage_dir: &Path, from: &str, to: &str) -> Result<(), StorageError> {
    if from == to {
        return Ok(());
    }

    let from_path = storage_dir.join(from);
    let to_path = storage_dir.join(to);

    if from.eq_ignore_ascii_case(to) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mid = storage_dir.join(format!(
            ".gts-rename-{}-{}-{}",
            std::process::id(),
            nanos,
            uuid::Uuid::new_v4().simple()
        ));
        fs::rename(&from_path, &mid).map_err(StorageError::io)?;
        if let Err(err) = fs::rename(&mid, &to_path) {
            let _ = fs::rename(&mid, &from_path);
            return Err(StorageError::io(err));
        }
        return Ok(());
    }

    fs::rename(&from_path, &to_path).map_err(StorageError::io)
}

pub fn rename_track(
    worktree: &Path,
    from: &str,
    to: &str,
    opts: &HomeOptions,
) -> Result<RenameTrackResult, StorageError> {
    validate_track_name(from)?;
    validate_track_name(to)?;

    let home = resolve_gts_home(opts);
    let found = find_workspace_root_from(worktree, opts)?;
    let id = found.id;
    let worktree = found.root;

    let current = resolve_active_track(&worktree, opts)?;
    let tracks = list_tracks(&id, opts)?;

    if !tracks.iter().any(|t| t == from) {
        return Err(StorageError::message(format!("Track not found: {from}")));
    }

    if from == to {
        return Ok(RenameTrackResult::Noop {
            track: from.to_string(),
        });
    }

    assert_destination_available(&id, from, to, opts)?;

    let storage_dir = workspace_storage_dir(&home, &id);
    let is_active = current == from;
    let new_git_abs = track_git_dir(&home, &id, to);
    // Absolute path string for the symlink target (directory may not exist yet).
    let new_git_abs = if new_git_abs.is_absolute() {
        new_git_abs
    } else {
        std::env::current_dir()
            .map_err(StorageError::io)?
            .join(new_git_abs)
    };

    if !is_active {
        rename_track_dir(&storage_dir, from, to)?;
        return Ok(RenameTrackResult::Renamed {
            from: from.to_string(),
            to: to.to_string(),
            active: false,
        });
    }

    let git_path = worktree.join(".git");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_name = format!(
        ".git.gts-tmp-{}-{}-{}",
        std::process::id(),
        nanos,
        uuid::Uuid::new_v4().simple()
    );
    let temp_path = worktree.join(&temp_name);

    symlink(&new_git_abs, &temp_path).map_err(StorageError::io)?;

    let finish = (|| -> Result<(), StorageError> {
        rename_track_dir(&storage_dir, from, to)?;
        if let Err(err) = fs::rename(&temp_path, &git_path) {
            let _ = rename_track_dir(&storage_dir, to, from);
            return Err(StorageError::io(err));
        }
        Ok(())
    })();

    if let Err(err) = finish {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }

    let mut index = read_index(opts)?;
    let entry = index.get_mut(&id).ok_or_else(|| {
        StorageError::message(format!("Workspace missing from index: {id}"))
    })?;
    if entry.active != to {
        entry.active = to.to_string();
        write_index(&index, opts)?;
    }

    Ok(RenameTrackResult::Renamed {
        from: from.to_string(),
        to: to.to_string(),
        active: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create::create_track;
    use crate::init::{init_workspace, MoveGitDirOptions};
    use crate::storage::{list_tracks, read_index, track_git_dir, workspace_storage_dir};
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

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
        let home = temp_dir("gts-rename-home");
        let real_gts = dirs::home_dir().unwrap().join(".gts");
        assert!(!home.starts_with(&real_gts));
        home
    }

    fn run_git(args: &[&str], cwd: &Path) {
        let output = std::process::Command::new("git")
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
        let opts = HomeOptions::with_home(home);
        init_workspace(worktree, "main", &opts, &MoveGitDirOptions::default())
            .unwrap()
            .id
    }

    fn path_exists(path: &Path) -> bool {
        fs::metadata(path).is_ok() || fs::symlink_metadata(path).is_ok()
    }

    #[test]
    fn renames_inactive_track_without_touching_git_or_active() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();

        let before_link = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();
        let before_active = read_index(&opts).unwrap().get(&id).unwrap().active.clone();

        let result = rename_track(&worktree, "feature", "feature-v2", &opts).unwrap();
        assert_eq!(
            result,
            RenameTrackResult::Renamed {
                from: "feature".into(),
                to: "feature-v2".into(),
                active: false,
            }
        );

        let tracks = list_tracks(&id, &opts).unwrap();
        assert!(tracks.contains(&"feature-v2".to_string()));
        assert!(!tracks.contains(&"feature".to_string()));
        assert_eq!(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
            before_link
        );
        assert_eq!(
            read_index(&opts).unwrap().get(&id).unwrap().active,
            before_active
        );
        assert_eq!(resolve_active_track(&worktree, &opts).unwrap(), "main");
    }

    #[test]
    fn renames_active_track_with_link_and_active_update() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let result = rename_track(&worktree, "main", "trunk", &opts).unwrap();
        assert_eq!(
            result,
            RenameTrackResult::Renamed {
                from: "main".into(),
                to: "trunk".into(),
                active: true,
            }
        );

        let tracks = list_tracks(&id, &opts).unwrap();
        assert_eq!(tracks, vec!["trunk".to_string()]);

        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
        assert_eq!(
            fs::canonicalize(fs::read_link(&git_path).unwrap()).unwrap(),
            fs::canonicalize(track_git_dir(&home, &id, "trunk")).unwrap()
        );
        assert_eq!(read_index(&opts).unwrap().get(&id).unwrap().active, "trunk");
        assert_eq!(resolve_active_track(&worktree, &opts).unwrap(), "trunk");
    }

    #[test]
    fn fails_on_destination_conflict_without_renaming_source() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        create_track(&worktree, "other", false, &opts).unwrap();

        let err = rename_track(&worktree, "feature", "other", &opts).unwrap_err();
        assert!(err.to_string().contains("already exists"));

        let tracks = list_tracks(&id, &opts).unwrap();
        assert!(tracks.contains(&"feature".to_string()));
        assert!(tracks.contains(&"other".to_string()));
        assert!(path_exists(
            &workspace_storage_dir(&home, &id).join("feature")
        ));
    }

    #[test]
    fn rejects_invalid_track_names() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        for bad in ["", ".", "..", "a/b", "a\\b"] {
            let err = rename_track(&worktree, "main", bad, &opts).unwrap_err();
            assert!(err.to_string().contains("Invalid track name"));
            let err = rename_track(&worktree, bad, "ok", &opts).unwrap_err();
            assert!(err.to_string().contains("Invalid track name"));
        }
    }

    #[test]
    fn fails_when_git_is_not_gts_managed_before_rename() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();

        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        fs::remove_file(&git_path).unwrap();
        fs::write(&git_path, "gitdir: /tmp/not-gts\n").unwrap();

        let err = rename_track(&worktree, "feature", "feature-v2", &opts).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not a symbolic link") || msg.contains("Broken") || msg.contains(".git"),
            "{msg}"
        );

        let tracks = list_tracks(&id, &opts).unwrap();
        assert!(tracks.contains(&"feature".to_string()));
        assert!(!tracks.contains(&"feature-v2".to_string()));
    }

    #[test]
    fn noop_when_from_equals_to() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let result = rename_track(&worktree, "main", "main", &opts).unwrap();
        assert_eq!(
            result,
            RenameTrackResult::Noop {
                track: "main".into()
            }
        );
    }

    #[test]
    fn fails_on_missing_source() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let err = rename_track(&worktree, "missing", "other", &opts).unwrap_err();
        assert!(err.to_string().contains("Track not found"));
        assert!(!path_exists(
            &workspace_storage_dir(&home, &id).join("other")
        ));
    }

    #[test]
    fn case_only_rename_of_same_track_when_fs_folds() {
        let home = temp_home();
        let worktree = temp_dir("gts-rename-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        let storage = workspace_storage_dir(&home, &id);

        let folds = match (
            fs::metadata(storage.join("main")),
            fs::metadata(storage.join("Main")),
        ) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        };
        if !folds {
            return;
        }

        let result = rename_track(&worktree, "main", "Main", &opts).unwrap();
        assert!(matches!(
            result,
            RenameTrackResult::Renamed {
                active: true,
                ..
            }
        ));
        assert_eq!(resolve_active_track(&worktree, &opts).unwrap(), "Main");
        assert_eq!(read_index(&opts).unwrap().get(&id).unwrap().active, "Main");
    }
}
