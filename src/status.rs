use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::storage::{
    find_workspace_root_from, list_tracks, resolve_active_track, HomeOptions, StorageError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceStatus {
    pub id: String,
    pub path: String,
    pub active: String,
    pub link: String,
    pub tracks: Vec<String>,
}

pub fn get_workspace_status(
    start_dir: &Path,
    opts: &HomeOptions,
) -> Result<WorkspaceStatus, StorageError> {
    let found = find_workspace_root_from(start_dir, opts)?;
    let active = resolve_active_track(&found.root, opts)?;
    let mut tracks = list_tracks(&found.id, opts)?;
    tracks.sort();

    let git_path = found.root.join(".git");
    let meta = fs::symlink_metadata(&git_path).map_err(StorageError::io)?;
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
    let link = fs::canonicalize(&abs_target).map_err(StorageError::io)?;

    Ok(WorkspaceStatus {
        id: found.id,
        path: found.root.to_string_lossy().into_owned(),
        active,
        link: link.to_string_lossy().into_owned(),
        tracks,
    })
}

pub fn format_status_human(status: &WorkspaceStatus) -> String {
    let mut lines = vec![
        format!("Active: {}", status.active),
        format!("Link: {}", status.link),
        "Tracks:".to_string(),
    ];
    for name in &status.tracks {
        if name == &status.active {
            lines.push(format!("  {name}  ACTIVE"));
        } else {
            lines.push(format!("  {name}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        index_path, read_index, register_workspace, replace_git_symlink, track_git_dir,
    };
    use std::os::unix::fs::symlink;
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

    fn setup() -> (PathBuf, PathBuf, String, PathBuf, PathBuf) {
        let home = temp_dir("gts-status-home");
        let real_gts = dirs::home_dir().unwrap().join(".gts");
        assert!(!home.starts_with(&real_gts));
        let worktree = temp_dir("gts-status-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        let main_git = track_git_dir(&home, &registered.id, "main");
        let feature_git = track_git_dir(&home, &registered.id, "feature-v1");
        fs::create_dir_all(&main_git).unwrap();
        fs::create_dir_all(&feature_git).unwrap();
        replace_git_symlink(&worktree, &fs::canonicalize(&main_git).unwrap()).unwrap();
        (home, worktree, registered.id, main_git, feature_git)
    }

    #[test]
    fn returns_id_path_active_link_tracks() {
        let (home, worktree, id, main_git, _feature_git) = setup();
        let opts = HomeOptions::with_home(&home);
        let status = get_workspace_status(&worktree, &opts).unwrap();
        assert_eq!(status.id, id);
        assert_eq!(
            status.path,
            fs::canonicalize(&worktree).unwrap().to_string_lossy()
        );
        assert_eq!(status.active, "main");
        assert_eq!(
            status.link,
            fs::canonicalize(&main_git).unwrap().to_string_lossy()
        );
        assert_eq!(status.tracks, vec!["feature-v1".to_string(), "main".to_string()]);
        let json = serde_json::to_value(&status).unwrap();
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 5);
        for key in ["id", "path", "active", "link", "tracks"] {
            assert!(obj.contains_key(key));
        }
    }

    #[test]
    fn rewrites_drifted_active_in_index() {
        let (home, worktree, id, _main_git, feature_git) = setup();
        let opts = HomeOptions::with_home(&home);
        replace_git_symlink(&worktree, &fs::canonicalize(&feature_git).unwrap()).unwrap();
        let before: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(index_path(&home)).unwrap()).unwrap();
        assert_eq!(before[&id]["active"], "main");
        let status = get_workspace_status(&worktree, &opts).unwrap();
        assert_eq!(status.active, "feature-v1");
        assert_eq!(
            read_index(&opts).unwrap().get(&id).unwrap().active,
            "feature-v1"
        );
    }

    #[test]
    fn fails_on_broken_symlink() {
        let (home, worktree, _id, _main_git, _feature_git) = setup();
        let opts = HomeOptions::with_home(&home);
        let git_path = worktree.join(".git");
        fs::remove_file(&git_path).unwrap();
        symlink(worktree.join("missing-target"), &git_path).unwrap();
        let err = get_workspace_status(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("Broken .git symlink"));
    }

    #[test]
    fn fails_when_link_points_outside_storage() {
        let (home, worktree, _id, _main_git, _feature_git) = setup();
        let opts = HomeOptions::with_home(&home);
        let outside = temp_dir("gts-status-outside");
        let outside_git = outside.join(".git");
        fs::create_dir_all(&outside_git).unwrap();
        replace_git_symlink(&worktree, &outside_git).unwrap();
        let err = get_workspace_status(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("outside workspace storage"));
    }

    #[test]
    fn fails_when_unregistered() {
        let home = temp_dir("gts-status-home");
        let worktree = temp_dir("gts-status-wt");
        let opts = HomeOptions::with_home(&home);
        let err = get_workspace_status(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("No registered workspace found"));
    }

    #[test]
    fn marks_active_in_human_format() {
        let text = format_status_human(&WorkspaceStatus {
            id: "x".into(),
            path: "/tmp/wt".into(),
            active: "main".into(),
            link: "/tmp/link".into(),
            tracks: vec!["feature-v1".into(), "main".into()],
        });
        assert!(text.contains("Active: main"));
        assert!(text.contains("Link: /tmp/link"));
        assert!(text.contains("main  ACTIVE"));
        assert!(text.contains("feature-v1"));
        assert!(!text.contains("feature-v1  ACTIVE"));
    }
}
