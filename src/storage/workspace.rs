use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::error::StorageError;
use super::home::{resolve_gts_home, storage_root, workspace_storage_dir, HomeOptions};
use super::index_file::{read_index, write_index, WorkspaceEntry};
use super::tracks::validate_track_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredWorkspace {
    pub id: String,
    pub entry: WorkspaceEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoot {
    pub id: String,
    pub entry: WorkspaceEntry,
    pub root: PathBuf,
}

pub fn register_workspace(
    worktree: &Path,
    active: &str,
    opts: &HomeOptions,
    id: Option<&str>,
) -> Result<RegisteredWorkspace, StorageError> {
    validate_track_name(active)?;
    let home = resolve_gts_home(opts);
    fs::create_dir_all(storage_root(&home))?;

    let worktree_real = fs::canonicalize(worktree).map_err(|_| {
        StorageError::message(format!("Workspace path does not exist: {}", worktree.display()))
    })?;

    let mut index = read_index(opts)?;
    for (existing_id, entry) in &index {
        let Ok(entry_real) = fs::canonicalize(&entry.path) else {
            continue;
        };
        if entry_real == worktree_real {
            return Err(StorageError::message(format!(
                "Workspace already registered as {existing_id}: {}",
                worktree_real.display()
            )));
        }
    }

    let id = id
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let entry = WorkspaceEntry {
        path: worktree_real.to_string_lossy().into_owned(),
        active: active.to_string(),
    };
    index.insert(id.clone(), entry.clone());
    write_index(&index, opts)?;
    fs::create_dir_all(workspace_storage_dir(&home, &id))?;
    Ok(RegisteredWorkspace { id, entry })
}

pub fn find_workspace_by_path(
    worktree: &Path,
    opts: &HomeOptions,
) -> Result<RegisteredWorkspace, StorageError> {
    let worktree_real = fs::canonicalize(worktree).map_err(|_| {
        StorageError::message(format!("Workspace path does not exist: {}", worktree.display()))
    })?;

    let index = read_index(opts)?;
    for (id, entry) in index {
        let Ok(entry_real) = fs::canonicalize(&entry.path) else {
            continue;
        };
        if entry_real == worktree_real {
            return Ok(RegisteredWorkspace { id, entry });
        }
    }
    Err(StorageError::message(format!(
        "Workspace not registered: {}",
        worktree_real.display()
    )))
}

pub fn find_workspace_root_from(
    start_dir: &Path,
    opts: &HomeOptions,
) -> Result<WorkspaceRoot, StorageError> {
    let mut current = fs::canonicalize(start_dir).map_err(|_| {
        StorageError::message(format!("Path does not exist: {}", start_dir.display()))
    })?;

    loop {
        match find_workspace_by_path(&current, opts) {
            Ok(found) => {
                return Ok(WorkspaceRoot {
                    id: found.id,
                    entry: found.entry,
                    root: current,
                });
            }
            Err(err) => {
                let msg = err.to_string();
                if !msg.starts_with("Workspace not registered:") {
                    return Err(err);
                }
            }
        }

        let parent = match current.parent() {
            Some(p) if p != current.as_path() => p.to_path_buf(),
            _ => break,
        };
        if parent.as_os_str().is_empty() {
            break;
        }
        current = parent;
    }

    Err(StorageError::message(format!(
        "No registered workspace found from: {}",
        start_dir.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn registers_with_uuid_absolute_path_and_active() {
        let home = temp_dir("gts-home");
        let worktree = temp_dir("gts-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        assert!(Uuid::parse_str(&registered.id).is_ok());
        assert_eq!(
            registered.entry.path,
            fs::canonicalize(&worktree).unwrap().to_string_lossy()
        );
        assert_eq!(registered.entry.active, "main");

        let raw = fs::read_to_string(super::super::home::index_path(&home)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let entry = &parsed[&registered.id];
        assert_eq!(entry["path"], registered.entry.path);
        assert_eq!(entry["active"], "main");
        assert!(entry.get("tracks").is_none());
        let real_gts = dirs::home_dir().unwrap().join(".gts");
        assert!(!home.starts_with(&real_gts));
    }

    #[test]
    fn lookup_succeeds_via_realpath_alias() {
        let home = temp_dir("gts-home");
        let worktree = temp_dir("gts-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        let parent = temp_dir("gts-alias");
        let link = parent.join("link");
        symlink(&worktree, &link).unwrap();
        let found = find_workspace_by_path(&link, &opts).unwrap();
        assert_eq!(found.id, registered.id);
    }

    #[test]
    fn lookup_fails_without_guessing() {
        let home = temp_dir("gts-home");
        let a = temp_dir("gts-wt-a");
        let b = temp_dir("gts-wt-b");
        let opts = HomeOptions::with_home(&home);
        register_workspace(&a, "main", &opts, None).unwrap();
        let err = find_workspace_by_path(&b, &opts).unwrap_err();
        assert!(err.to_string().contains("Workspace not registered"));
    }

    #[test]
    fn finds_registered_ancestor_from_nested() {
        let home = temp_dir("gts-home");
        let worktree = temp_dir("gts-wt");
        let nested = worktree.join("pkg").join("deep");
        fs::create_dir_all(&nested).unwrap();
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        let found = find_workspace_root_from(&nested, &opts).unwrap();
        assert_eq!(found.id, registered.id);
        assert_eq!(found.root, fs::canonicalize(&worktree).unwrap());
    }

    #[test]
    fn reports_unregistered_when_no_ancestor() {
        let home = temp_dir("gts-home");
        let orphan = temp_dir("gts-orphan");
        let opts = HomeOptions::with_home(&home);
        let err = find_workspace_root_from(&orphan, &opts).unwrap_err();
        assert!(err.to_string().contains("No registered workspace found"));
    }
}
