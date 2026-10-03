use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use super::error::StorageError;
use super::home::{resolve_gts_home, track_git_dir, workspace_storage_dir, HomeOptions};
use super::index_file::{read_index, write_index};
use super::workspace::find_workspace_by_path;

fn is_outside(storage_real: &Path, target_real: &Path) -> bool {
    match target_real.strip_prefix(storage_real) {
        Ok(rel) => rel.as_os_str().is_empty(),
        Err(_) => true,
    }
}

pub fn replace_git_symlink(worktree: &Path, absolute_target: &Path) -> Result<(), StorageError> {
    let root = fs::canonicalize(worktree).map_err(StorageError::io)?;
    let git_path = root.join(".git");
    let abs_target = if absolute_target.is_absolute() {
        absolute_target.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(StorageError::io)?
            .join(absolute_target)
    };
    let _ = fs::canonicalize(&abs_target).map_err(StorageError::io)?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_name = format!(".git.gts-tmp-{}-{}", std::process::id(), nanos);
    let temp_path = root.join(&temp_name);
    symlink(&abs_target, &temp_path).map_err(StorageError::io)?;
    if let Err(err) = fs::rename(&temp_path, &git_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(StorageError::io(err));
    }
    Ok(())
}

pub fn replace_git_symlink_to_track(
    worktree: &Path,
    track: &str,
    opts: &HomeOptions,
) -> Result<(), StorageError> {
    let found = find_workspace_by_path(worktree, opts)?;
    let home = resolve_gts_home(opts);
    let target = track_git_dir(&home, &found.id, track);
    fs::create_dir_all(&target)?;
    let abs_target = fs::canonicalize(&target).map_err(StorageError::io)?;
    replace_git_symlink(worktree, &abs_target)?;

    let mut index = read_index(opts)?;
    let entry = index.get_mut(&found.id).ok_or_else(|| {
        StorageError::message(format!("Workspace missing from index: {}", found.id))
    })?;
    if entry.active != track {
        entry.active = track.to_string();
        write_index(&index, opts)?;
    }
    Ok(())
}

pub fn resolve_active_track(worktree: &Path, opts: &HomeOptions) -> Result<String, StorageError> {
    let found = find_workspace_by_path(worktree, opts)?;
    let home = resolve_gts_home(opts);
    let root = fs::canonicalize(worktree).map_err(StorageError::io)?;
    let git_path = root.join(".git");

    let meta = fs::symlink_metadata(&git_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            StorageError::message(format!("Missing .git symlink: {}", git_path.display()))
        } else {
            StorageError::io(err)
        }
    })?;
    if !meta.file_type().is_symlink() {
        return Err(StorageError::message(format!(
            ".git is not a symbolic link: {}",
            git_path.display()
        )));
    }

    let link_target = fs::read_link(&git_path).map_err(StorageError::io)?;
    let abs_target = if link_target.is_absolute() {
        link_target
    } else {
        git_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(link_target)
    };

    let target_real = fs::canonicalize(&abs_target).map_err(|_| {
        StorageError::message(format!("Broken .git symlink: {}", git_path.display()))
    })?;

    let storage_real = fs::canonicalize(workspace_storage_dir(&home, &found.id))
        .map_err(StorageError::io)?;
    if is_outside(&storage_real, &target_real) {
        return Err(StorageError::message(format!(
            ".git symlink points outside workspace storage: {}",
            target_real.display()
        )));
    }

    let rel = target_real
        .strip_prefix(&storage_real)
        .map_err(|_| {
            StorageError::message(format!(
                ".git symlink points outside workspace storage: {}",
                target_real.display()
            ))
        })?;
    let parts: Vec<_> = rel.components().collect();
    if parts.len() != 2
        || parts[1] != Component::Normal(".git".as_ref())
        || matches!(parts[0], Component::Normal(s) if s.is_empty())
    {
        return Err(StorageError::message(format!(
            ".git symlink does not point to a track .git directory: {}",
            target_real.display()
        )));
    }
    let track = match parts[0] {
        Component::Normal(s) => s.to_string_lossy().into_owned(),
        _ => {
            return Err(StorageError::message(format!(
                ".git symlink does not point to a track .git directory: {}",
                target_real.display()
            )));
        }
    };

    if found.entry.active != track {
        let mut index = read_index(opts)?;
        if let Some(entry) = index.get_mut(&found.id) {
            entry.active = track.clone();
            write_index(&index, opts)?;
        }
    }
    Ok(track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::home::index_path;
    use crate::storage::index_file::read_index;
    use crate::storage::workspace::register_workspace;
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

    fn setup_workspace(active: &str) -> (PathBuf, PathBuf, String, PathBuf, PathBuf) {
        let home = temp_dir("gts-home");
        let worktree = temp_dir("gts-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, active, &opts, None).unwrap();
        let main_git = track_git_dir(&home, &registered.id, "main");
        let feature_git = track_git_dir(&home, &registered.id, "feature-v1");
        fs::create_dir_all(&main_git).unwrap();
        fs::create_dir_all(&feature_git).unwrap();
        (home, worktree, registered.id, main_git, feature_git)
    }

    #[test]
    fn replaces_git_with_absolute_symlink() {
        let (home, worktree, id, main_git, feature_git) = setup_workspace("main");
        let opts = HomeOptions::with_home(&home);
        replace_git_symlink(&worktree, &fs::canonicalize(&main_git).unwrap()).unwrap();
        replace_git_symlink_to_track(&worktree, "feature-v1", &opts).unwrap();
        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        let meta = fs::symlink_metadata(&git_path).unwrap();
        assert!(meta.file_type().is_symlink());
        let target = fs::read_link(&git_path).unwrap();
        assert!(target.is_absolute());
        assert_eq!(
            fs::canonicalize(&target).unwrap(),
            fs::canonicalize(&feature_git).unwrap()
        );
        let index = read_index(&opts).unwrap();
        assert_eq!(index.get(&id).unwrap().active, "feature-v1");
    }

    #[test]
    fn rewrites_active_when_symlink_drifted() {
        let (home, worktree, id, _main_git, feature_git) = setup_workspace("main");
        let opts = HomeOptions::with_home(&home);
        replace_git_symlink(&worktree, &fs::canonicalize(&feature_git).unwrap()).unwrap();
        let before: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(index_path(&home)).unwrap()).unwrap();
        assert_eq!(before[&id]["active"], "main");
        let track = resolve_active_track(&worktree, &opts).unwrap();
        assert_eq!(track, "feature-v1");
        assert_eq!(read_index(&opts).unwrap().get(&id).unwrap().active, "feature-v1");
    }

    #[test]
    fn errors_on_broken_symlink() {
        let (home, worktree, _id, _main_git, _feature_git) = setup_workspace("main");
        let opts = HomeOptions::with_home(&home);
        let git_path = worktree.join(".git");
        symlink(worktree.join("missing-target"), &git_path).unwrap();
        let err = resolve_active_track(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("Broken .git symlink"));
    }

    #[test]
    fn errors_when_symlink_points_outside_storage() {
        let (home, worktree, _id, _main_git, _feature_git) = setup_workspace("main");
        let opts = HomeOptions::with_home(&home);
        let outside = temp_dir("gts-outside");
        let outside_git = outside.join(".git");
        fs::create_dir_all(&outside_git).unwrap();
        replace_git_symlink(&worktree, &outside_git).unwrap();
        let err = resolve_active_track(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("outside workspace storage"));
    }
}
