use std::fs;
use std::path::Path;

use crate::storage::{
    find_workspace_root_from, list_tracks, resolve_active_track, resolve_gts_home,
    validate_track_name, workspace_storage_dir, HomeOptions, StorageError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveTrackResult {
    pub track: String,
}

pub fn remove_track(
    worktree: &Path,
    track: &str,
    opts: &HomeOptions,
) -> Result<RemoveTrackResult, StorageError> {
    validate_track_name(track)?;

    let home = resolve_gts_home(opts);
    let found = find_workspace_root_from(worktree, opts)?;
    let id = found.id;
    let worktree = found.root;

    let current = resolve_active_track(&worktree, opts)?;
    let tracks = list_tracks(&id, opts)?;

    if !tracks.iter().any(|t| t == track) {
        return Err(StorageError::message(format!("Track not found: {track}")));
    }

    if current == track {
        return Err(StorageError::message(format!(
            "Cannot remove the active track: {track}"
        )));
    }

    let track_dir = workspace_storage_dir(&home, &id).join(track);
    fs::remove_dir_all(&track_dir).map_err(StorageError::io)?;

    Ok(RemoveTrackResult {
        track: track.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create::create_track;
    use crate::init::{init_workspace, MoveGitDirOptions};
    use crate::storage::{list_tracks, read_index, track_git_dir};
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
        let home = temp_dir("gts-remove-home");
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
    fn removes_inactive_track_without_touching_git_or_active() {
        let home = temp_home();
        let worktree = temp_dir("gts-remove-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();

        let exclude = track_git_dir(&home, &id, "feature").join("info").join("exclude");
        fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        fs::write(&exclude, "local-ignore\n").unwrap();

        let before_link = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();
        let before_active = read_index(&opts).unwrap().get(&id).unwrap().active.clone();

        let result = remove_track(&worktree, "feature", &opts).unwrap();
        assert_eq!(result.track, "feature");

        let tracks = list_tracks(&id, &opts).unwrap();
        assert!(!tracks.contains(&"feature".to_string()));
        assert!(tracks.contains(&"main".to_string()));
        assert!(!path_exists(&workspace_storage_dir(&home, &id).join("feature")));
        assert!(!path_exists(&exclude));
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
    fn refuses_removing_active_track() {
        let home = temp_home();
        let worktree = temp_dir("gts-remove-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let err = remove_track(&worktree, "main", &opts).unwrap_err();
        assert!(err.to_string().contains("Cannot remove the active track"));
        assert!(path_exists(&workspace_storage_dir(&home, &id).join("main")));
    }

    #[test]
    fn fails_when_target_missing() {
        let home = temp_home();
        let worktree = temp_dir("gts-remove-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let err = remove_track(&worktree, "missing", &opts).unwrap_err();
        assert!(err.to_string().contains("Track not found"));
        assert!(!path_exists(&workspace_storage_dir(&home, &id).join("missing")));
    }

    #[test]
    fn rejects_invalid_track_names() {
        let home = temp_home();
        let worktree = temp_dir("gts-remove-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        for bad in ["", ".", "..", "bad/name", "bad\\name"] {
            let err = remove_track(&worktree, bad, &opts).unwrap_err();
            assert!(
                err.to_string().contains("Invalid track name"),
                "bad={bad} err={err}"
            );
        }
    }

    #[test]
    fn fails_when_git_is_not_gts_managed_before_delete() {
        let home = temp_home();
        let worktree = temp_dir("gts-remove-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();

        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        fs::remove_file(&git_path).unwrap();
        fs::write(&git_path, "gitdir: /tmp/not-gts\n").unwrap();

        let err = remove_track(&worktree, "feature", &opts).unwrap_err();
        assert!(!err.to_string().is_empty());
        assert!(path_exists(&workspace_storage_dir(&home, &id).join("feature")));
    }

    #[test]
    fn temp_home_does_not_touch_user_gts() {
        let home = temp_home();
        assert!(!home.starts_with(dirs::home_dir().unwrap().join(".gts")));
        let worktree = temp_dir("gts-remove-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        remove_track(&worktree, "feature", &opts).unwrap();
        assert!(!path_exists(&workspace_storage_dir(&home, &id).join("feature")));
    }
}
