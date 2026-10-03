use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::error::StorageError;
use super::home::{resolve_gts_home, workspace_storage_dir, HomeOptions};

pub fn validate_track_name(name: &str) -> Result<(), StorageError> {
    if name.is_empty() || name == "." || name == ".." {
        let label = if name.is_empty() { "(empty)" } else { name };
        return Err(StorageError::message(format!("Invalid track name: {label}")));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(StorageError::message(format!("Invalid track name: {name}")));
    }
    Ok(())
}

pub fn list_tracks(id: &str, opts: &HomeOptions) -> Result<Vec<String>, StorageError> {
    let home = resolve_gts_home(opts);
    let dir = workspace_storage_dir(&home, id);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(err) => return Err(StorageError::io(err)),
    };
    let mut tracks = Vec::new();
    for entry in entries {
        let entry = entry.map_err(StorageError::io)?;
        let file_type = entry.file_type().map_err(StorageError::io)?;
        if file_type.is_dir() {
            tracks.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(tracks)
}

fn names_collide_on_fs(storage_dir: &Path, existing: &str, candidate: &str) -> bool {
    if existing == candidate {
        return true;
    }
    if existing.to_lowercase() != candidate.to_lowercase() {
        return false;
    }
    let existing_path = storage_dir.join(existing);
    let candidate_path = storage_dir.join(candidate);
    match (fs::metadata(&existing_path), fs::metadata(&candidate_path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

pub fn assert_track_name_available(
    id: &str,
    name: &str,
    opts: &HomeOptions,
) -> Result<(), StorageError> {
    validate_track_name(name)?;
    let home = resolve_gts_home(opts);
    let storage_dir = workspace_storage_dir(&home, id);
    let tracks = list_tracks(id, opts)?;
    for existing in tracks {
        if existing == name {
            return Err(StorageError::message(format!("Track already exists: {name}")));
        }
        if names_collide_on_fs(&storage_dir, &existing, name) {
            return Err(StorageError::message(format!(
                "Track name conflicts with existing track on a case-insensitive volume: {existing}"
            )));
        }
    }
    Ok(())
}

/// Like [`assert_track_name_available`], but skips `from` so a case-only rename of the
/// same track is allowed on case-insensitive volumes.
pub fn assert_destination_available(
    id: &str,
    from: &str,
    to: &str,
    opts: &HomeOptions,
) -> Result<(), StorageError> {
    validate_track_name(to)?;
    let home = resolve_gts_home(opts);
    let storage_dir = workspace_storage_dir(&home, id);
    let tracks = list_tracks(id, opts)?;
    for existing in tracks {
        if existing == from {
            continue;
        }
        if existing == to {
            return Err(StorageError::message(format!("Track already exists: {to}")));
        }
        if names_collide_on_fs(&storage_dir, &existing, to) {
            return Err(StorageError::message(format!(
                "Track name conflicts with existing track on a case-insensitive volume: {existing}"
            )));
        }
    }
    Ok(())
}

pub fn ensure_track_dir(
    id: &str,
    track: &str,
    opts: &HomeOptions,
) -> Result<PathBuf, StorageError> {
    validate_track_name(track)?;
    let home = resolve_gts_home(opts);
    let git_dir = workspace_storage_dir(&home, id).join(track).join(".git");
    fs::create_dir_all(&git_dir)?;
    fs::canonicalize(&git_dir).map_err(StorageError::io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::workspace::register_workspace;

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
    fn rejects_invalid_track_names() {
        for name in ["", ".", "..", "a/b", "a\\b"] {
            let err = validate_track_name(name).unwrap_err();
            assert!(err.to_string().contains("Invalid track name"));
        }
    }

    #[test]
    fn lists_tracks_from_storage_directories_only() {
        let home = temp_dir("gts-home");
        let worktree = temp_dir("gts-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        let storage = workspace_storage_dir(&home, &registered.id);
        fs::create_dir_all(storage.join("main").join(".git")).unwrap();
        fs::create_dir_all(storage.join("feature-v1").join(".git")).unwrap();
        fs::write(storage.join("note.txt"), "ignore").unwrap();
        let tracks = list_tracks(&registered.id, &opts).unwrap();
        let set: std::collections::HashSet<_> = tracks.into_iter().collect();
        assert_eq!(
            set,
            ["main".into(), "feature-v1".into()]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn detects_case_insensitive_conflict_when_fs_folds() {
        let home = temp_dir("gts-home");
        let worktree = temp_dir("gts-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "Main", &opts, None).unwrap();
        let storage = workspace_storage_dir(&home, &registered.id);
        fs::create_dir_all(storage.join("Main").join(".git")).unwrap();

        let folds = match (
            fs::metadata(storage.join("Main")),
            fs::metadata(storage.join("main")),
        ) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        };

        if folds {
            let err = assert_track_name_available(&registered.id, "main", &opts).unwrap_err();
            assert!(err.to_string().contains("case-insensitive"));
        } else {
            assert_track_name_available(&registered.id, "main", &opts).unwrap();
        }
    }

    #[test]
    fn destination_available_skips_from_and_rejects_other_duplicate() {
        let home = temp_dir("gts-home");
        let worktree = temp_dir("gts-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        let storage = workspace_storage_dir(&home, &registered.id);
        fs::create_dir_all(storage.join("main").join(".git")).unwrap();
        fs::create_dir_all(storage.join("feature").join(".git")).unwrap();

        assert_destination_available(&registered.id, "main", "main", &opts).unwrap();
        assert_destination_available(&registered.id, "main", "trunk", &opts).unwrap();

        let err = assert_destination_available(&registered.id, "main", "feature", &opts).unwrap_err();
        assert!(err.to_string().contains("already exists"));

        let folds = match (
            fs::metadata(storage.join("feature")),
            fs::metadata(storage.join("Feature")),
        ) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        };
        if folds {
            let err =
                assert_destination_available(&registered.id, "main", "Feature", &opts).unwrap_err();
            assert!(err.to_string().contains("case-insensitive"));
            assert_destination_available(&registered.id, "feature", "Feature", &opts).unwrap();
        }
    }
}
