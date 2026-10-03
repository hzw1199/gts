use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::storage::{
    find_workspace_root_from, read_index, register_workspace, replace_git_symlink,
    resolve_gts_home, storage_root, track_git_dir, validate_track_name, write_index, HomeOptions,
    StorageError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitShape {
    Missing,
    Directory,
    File,
    Symlink { target: PathBuf, managed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveMode {
    Rename,
    Copy,
}

#[derive(Default)]
pub struct MoveGitDirOptions {
    /// When set, used instead of `fs::rename`. Inject CrossesDevices to test EXDEV fallback.
    pub rename_fn: Option<fn(&Path, &Path) -> io::Result<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitWorkspaceResult {
    pub id: String,
    pub track: String,
    pub worktree: PathBuf,
    pub git_dir: PathBuf,
}

fn is_exdev(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::CrossesDevices || err.raw_os_error() == Some(18)
}

fn is_gts_managed_target(abs_target: &Path, opts: &HomeOptions) -> bool {
    let home = resolve_gts_home(opts);
    let storage_real = match fs::canonicalize(storage_root(&home)) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let target_real = match fs::canonicalize(abs_target) {
        Ok(p) => p,
        Err(_) => {
            let normalized = if abs_target.is_absolute() {
                abs_target.to_path_buf()
            } else {
                abs_target.to_path_buf()
            };
            return match normalized.strip_prefix(&storage_real) {
                Ok(rel) => !rel.as_os_str().is_empty(),
                Err(_) => false,
            };
        }
    };

    match target_real.strip_prefix(&storage_real) {
        Ok(rel) => !rel.as_os_str().is_empty(),
        Err(_) => false,
    }
}

pub fn detect_git_shape(worktree: &Path, opts: &HomeOptions) -> Result<GitShape, StorageError> {
    let root = fs::canonicalize(worktree).map_err(StorageError::io)?;
    let git_path = root.join(".git");
    let meta = match fs::symlink_metadata(&git_path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(GitShape::Missing),
        Err(err) => return Err(StorageError::io(err)),
    };

    let ft = meta.file_type();
    if ft.is_symlink() {
        let target = fs::read_link(&git_path).map_err(StorageError::io)?;
        let abs_target = if target.is_absolute() {
            target
        } else {
            git_path.parent().unwrap_or(Path::new(".")).join(target)
        };
        let managed = is_gts_managed_target(&abs_target, opts);
        return Ok(GitShape::Symlink {
            target: abs_target,
            managed,
        });
    }
    if ft.is_file() {
        return Ok(GitShape::File);
    }
    if ft.is_dir() {
        return Ok(GitShape::Directory);
    }
    Err(StorageError::message(format!(
        "Unsupported .git type at {}",
        git_path.display()
    )))
}

pub fn assert_not_registered(worktree: &Path, opts: &HomeOptions) -> Result<(), StorageError> {
    match find_workspace_root_from(worktree, opts) {
        Ok(found) => Err(StorageError::message(format!(
            "Workspace already registered as {}: {}",
            found.id,
            found.root.display()
        ))),
        Err(err) => {
            let msg = err.to_string();
            if msg.starts_with("No registered workspace found") {
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
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

pub fn move_git_dir(
    source_git: &Path,
    dest_git: &Path,
    options: &MoveGitDirOptions,
) -> Result<MoveMode, StorageError> {
    if let Some(parent) = dest_git.parent() {
        fs::create_dir_all(parent)?;
    }

    let rename_result = if let Some(rename_fn) = options.rename_fn {
        rename_fn(source_git, dest_git)
    } else {
        fs::rename(source_git, dest_git)
    };

    match rename_result {
        Ok(()) => Ok(MoveMode::Rename),
        Err(err) if is_exdev(&err) => {
            copy_dir_all(source_git, dest_git).map_err(StorageError::io)?;
            Ok(MoveMode::Copy)
        }
        Err(err) => Err(StorageError::io(err)),
    }
}

fn run_git_init(track_dir: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(track_dir)?;
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

fn remove_workspace_registration(id: &str, opts: &HomeOptions) -> Result<(), StorageError> {
    let home = resolve_gts_home(opts);
    let mut index = read_index(opts)?;
    index.remove(id);
    write_index(&index, opts)?;
    let storage_id = storage_root(&home).join(id);
    let _ = fs::remove_dir_all(&storage_id);
    Ok(())
}

fn path_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok() || fs::symlink_metadata(path).is_ok()
}

pub fn init_workspace(
    worktree: &Path,
    track: &str,
    opts: &HomeOptions,
    move_options: &MoveGitDirOptions,
) -> Result<InitWorkspaceResult, StorageError> {
    validate_track_name(track)?;
    let worktree = fs::canonicalize(worktree).map_err(StorageError::io)?;
    assert_not_registered(&worktree, opts)?;

    let shape = detect_git_shape(&worktree, opts)?;
    match &shape {
        GitShape::File => {
            return Err(StorageError::message(
                ".git is a file (gitfile); gts does not support gitfile, worktree, or submodule .git entries",
            ));
        }
        GitShape::Symlink { target, managed } => {
            if *managed {
                return Err(StorageError::message(format!(
                    ".git is already a gts-managed symlink: {}",
                    target.display()
                )));
            }
            return Err(StorageError::message(
                ".git is a symbolic link; gts init requires a plain .git directory or no .git",
            ));
        }
        GitShape::Missing | GitShape::Directory => {}
    }

    let registered = register_workspace(&worktree, track, opts, None)?;
    let id = registered.id.clone();
    let home = resolve_gts_home(opts);
    let dest_git = track_git_dir(&home, &id, track);
    let source_git = worktree.join(".git");

    let mut moved_by_rename = false;
    let mut copied_away = false;

    let finish = (|| -> Result<InitWorkspaceResult, StorageError> {
        match shape {
            GitShape::Directory => {
                let mode = move_git_dir(&source_git, &dest_git, move_options)?;
                match mode {
                    MoveMode::Rename => {
                        moved_by_rename = true;
                    }
                    MoveMode::Copy => {
                        fs::remove_dir_all(&source_git)?;
                        copied_away = true;
                    }
                }
            }
            GitShape::Missing => {
                let track_dir = dest_git.parent().ok_or_else(|| {
                    StorageError::message(format!("Invalid track git path: {}", dest_git.display()))
                })?;
                run_git_init(track_dir)?;
                if !path_exists(&dest_git) {
                    return Err(StorageError::message(format!(
                        "git init did not create {}",
                        dest_git.display()
                    )));
                }
            }
            GitShape::File | GitShape::Symlink { .. } => unreachable!(),
        }

        let abs_dest = fs::canonicalize(&dest_git).map_err(StorageError::io)?;
        replace_git_symlink(&worktree, &abs_dest)?;

        Ok(InitWorkspaceResult {
            id: id.clone(),
            track: track.to_string(),
            worktree: worktree.clone(),
            git_dir: abs_dest,
        })
    })();

    if let Err(err) = &finish {
        let _ = rollback_partial(
            &worktree,
            &source_git,
            &dest_git,
            moved_by_rename,
            copied_away,
        );
        let _ = remove_workspace_registration(&id, opts);
        let _ = err;
    }

    finish
}

fn rollback_partial(
    worktree: &Path,
    source_git: &Path,
    dest_git: &Path,
    moved_by_rename: bool,
    copied_away: bool,
) -> Result<(), StorageError> {
    let git_link = worktree.join(".git");
    if fs::symlink_metadata(&git_link)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        let _ = fs::remove_file(&git_link);
    }

    if moved_by_rename && path_exists(dest_git) {
        let _ = fs::remove_dir_all(source_git);
        let _ = fs::rename(dest_git, source_git);
    } else if copied_away && path_exists(dest_git) {
        let _ = fs::remove_dir_all(source_git);
        let _ = copy_dir_all(dest_git, source_git);
        let _ = fs::remove_dir_all(dest_git);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{index_path, replace_git_symlink_to_track};
    use std::os::unix::fs::symlink;
    use uuid::Uuid;

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_plain_git(worktree: &Path) {
        let git = worktree.join(".git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    fn always_exdev(_src: &Path, _dst: &Path) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::CrossesDevices, "cross-device"))
    }

    #[test]
    fn detects_missing_directory_file_and_managed_symlink() {
        let home = temp_dir("gts-init-home");
        let opts = HomeOptions::with_home(&home);

        let missing = temp_dir("gts-init-missing");
        assert_eq!(detect_git_shape(&missing, &opts).unwrap(), GitShape::Missing);

        let dir_wt = temp_dir("gts-init-dir");
        make_plain_git(&dir_wt);
        assert_eq!(detect_git_shape(&dir_wt, &opts).unwrap(), GitShape::Directory);

        let file_wt = temp_dir("gts-init-file");
        fs::write(file_wt.join(".git"), "gitdir: /tmp/somewhere\n").unwrap();
        assert_eq!(detect_git_shape(&file_wt, &opts).unwrap(), GitShape::File);

        let managed_wt = temp_dir("gts-init-managed");
        let registered = register_workspace(&managed_wt, "main", &opts, None).unwrap();
        fs::create_dir_all(track_git_dir(&home, &registered.id, "main")).unwrap();
        replace_git_symlink_to_track(&managed_wt, "main", &opts).unwrap();
        match detect_git_shape(&managed_wt, &opts).unwrap() {
            GitShape::Symlink { managed, .. } => assert!(managed),
            other => panic!("expected symlink, got {other:?}"),
        }
    }

    #[test]
    fn rejects_already_registered_workspace() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        let opts = HomeOptions::with_home(&home);
        register_workspace(&worktree, "main", &opts, None).unwrap();
        let err = assert_not_registered(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("Workspace already registered"));
    }

    #[test]
    fn move_git_dir_renames_on_same_volume() {
        let root = temp_dir("gts-move");
        let src = root.join("src");
        let dest = root.join("dest").join(".git");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("HEAD"), "ok\n").unwrap();
        let mode = move_git_dir(&src, &dest, &MoveGitDirOptions::default()).unwrap();
        assert_eq!(mode, MoveMode::Rename);
        assert_eq!(fs::read_to_string(dest.join("HEAD")).unwrap(), "ok\n");
    }

    #[test]
    fn move_git_dir_falls_back_to_copy_on_exdev() {
        let root = temp_dir("gts-exdev");
        let src = root.join("src");
        let dest = root.join("dest").join(".git");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("HEAD"), "copied\n").unwrap();
        let opts = MoveGitDirOptions {
            rename_fn: Some(always_exdev),
        };
        let mode = move_git_dir(&src, &dest, &opts).unwrap();
        assert_eq!(mode, MoveMode::Copy);
        assert_eq!(fs::read_to_string(dest.join("HEAD")).unwrap(), "copied\n");
        assert_eq!(fs::read_to_string(src.join("HEAD")).unwrap(), "copied\n");
    }

    #[test]
    fn init_workspace_moves_plain_git_and_registers() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        make_plain_git(&worktree);
        let marker = fs::read_to_string(worktree.join(".git").join("HEAD")).unwrap();
        let opts = HomeOptions::with_home(&home);
        let result =
            init_workspace(&worktree, "main", &opts, &MoveGitDirOptions::default()).unwrap();
        assert!(Uuid::parse_str(&result.id).is_ok());

        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
        let target = fs::read_link(&git_path).unwrap();
        assert!(target.is_absolute());
        assert_eq!(fs::canonicalize(&target).unwrap(), result.git_dir);
        assert_eq!(fs::read_to_string(result.git_dir.join("HEAD")).unwrap(), marker);

        let index = read_index(&opts).unwrap();
        assert_eq!(index.get(&result.id).unwrap().active, "main");
        assert_eq!(
            index.get(&result.id).unwrap().path,
            fs::canonicalize(&worktree).unwrap().to_string_lossy()
        );
        let real_gts = dirs::home_dir().unwrap().join(".gts");
        assert!(!home.starts_with(&real_gts));
    }

    #[test]
    fn init_workspace_exdev_copies_then_links() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        make_plain_git(&worktree);
        let opts = HomeOptions::with_home(&home);
        let move_opts = MoveGitDirOptions {
            rename_fn: Some(always_exdev),
        };
        let result = init_workspace(&worktree, "main", &opts, &move_opts).unwrap();
        let git_path = worktree.join(".git");
        assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
        assert_eq!(
            fs::canonicalize(fs::read_link(&git_path).unwrap()).unwrap(),
            result.git_dir
        );
        assert_eq!(
            fs::read_to_string(result.git_dir.join("HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
    }

    #[test]
    fn init_workspace_missing_git_runs_git_init() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        let opts = HomeOptions::with_home(&home);
        let result =
            init_workspace(&worktree, "main", &opts, &MoveGitDirOptions::default()).unwrap();
        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
        let target = fs::read_link(&git_path).unwrap();
        assert!(target.is_absolute());
        assert_eq!(fs::canonicalize(&target).unwrap(), result.git_dir);
        let head = fs::read_to_string(result.git_dir.join("HEAD")).unwrap();
        assert!(head.contains("ref:"));
    }

    #[test]
    fn rejects_gitfile_without_registering() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        fs::write(worktree.join(".git"), "gitdir: /tmp/x\n").unwrap();
        let opts = HomeOptions::with_home(&home);
        let err = init_workspace(&worktree, "main", &opts, &MoveGitDirOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("gitfile"));
        assert!(read_index(&opts).unwrap().is_empty());
    }

    #[test]
    fn rejects_managed_symlink_without_changing_state() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        fs::create_dir_all(track_git_dir(&home, &registered.id, "main")).unwrap();
        replace_git_symlink_to_track(&worktree, "main", &opts).unwrap();
        let before = fs::read_to_string(index_path(&home)).unwrap();
        let err = init_workspace(&worktree, "other", &opts, &MoveGitDirOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("already registered"));
        assert_eq!(fs::read_to_string(index_path(&home)).unwrap(), before);
    }

    #[test]
    fn rejects_unmanaged_symlink() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        let outside = temp_dir("gts-outside-git");
        symlink(&outside, worktree.join(".git")).unwrap();
        let opts = HomeOptions::with_home(&home);
        let err = init_workspace(&worktree, "main", &opts, &MoveGitDirOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("symbolic link"));
        assert!(read_index(&opts).unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_track_names() {
        let home = temp_dir("gts-init-home");
        let worktree = temp_dir("gts-init-wt");
        let opts = HomeOptions::with_home(&home);
        for track in ["", ".", "..", "a/b", "a\\b"] {
            let err =
                init_workspace(&worktree, track, &opts, &MoveGitDirOptions::default()).unwrap_err();
            assert!(err.to_string().contains("Invalid track name"));
        }
    }
}
