use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use gts::storage::{
    find_workspace_by_path, find_workspace_root_from, index_path, read_index,
    register_workspace, replace_git_symlink, replace_git_symlink_to_track, resolve_active_track,
    track_git_dir, validate_track_name, HomeOptions,
};

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
    let home = temp_dir("gts-func-home");
    let real_gts = dirs::home_dir().unwrap().join(".gts");
    assert!(!home.starts_with(&real_gts));
    home
}

#[test]
fn realpath_lookup_and_upward_root_discovery() {
    let home = isolated_home();
    let worktree = temp_dir("gts-func-wt");
    let nested = worktree.join("a").join("b");
    fs::create_dir_all(&nested).unwrap();
    let opts = HomeOptions::with_home(&home);
    let registered = register_workspace(&worktree, "main", &opts, None).unwrap();

    let alias_dir = temp_dir("gts-func-alias");
    let alias = alias_dir.join("wt");
    symlink(&worktree, &alias).unwrap();

    let by_alias = find_workspace_by_path(&alias, &opts).unwrap();
    assert_eq!(by_alias.id, registered.id);

    let from_nested = find_workspace_root_from(&nested, &opts).unwrap();
    assert_eq!(from_nested.id, registered.id);
    assert_eq!(from_nested.root, fs::canonicalize(&worktree).unwrap());

    let orphan = temp_dir("gts-func-orphan");
    let err = find_workspace_root_from(&orphan, &opts).unwrap_err();
    assert!(err.to_string().contains("No registered workspace found"));
}

#[test]
fn rejects_invalid_track_names() {
    for name in ["", ".", "..", "x/y", "x\\y"] {
        let err = validate_track_name(name).unwrap_err();
        assert!(err.to_string().contains("Invalid track name"));
    }
}

#[test]
fn atomically_replaces_git_with_absolute_symlink() {
    let home = isolated_home();
    let worktree = temp_dir("gts-func-wt");
    let opts = HomeOptions::with_home(&home);
    let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
    fs::create_dir_all(track_git_dir(&home, &registered.id, "main")).unwrap();
    fs::create_dir_all(track_git_dir(&home, &registered.id, "other")).unwrap();

    replace_git_symlink_to_track(&worktree, "main", &opts).unwrap();
    replace_git_symlink_to_track(&worktree, "other", &opts).unwrap();

    let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
    assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
    let target = fs::read_link(&git_path).unwrap();
    assert!(target.is_absolute());
    assert_eq!(
        fs::canonicalize(&target).unwrap(),
        fs::canonicalize(track_git_dir(&home, &registered.id, "other")).unwrap()
    );
}

#[test]
fn rewrites_drifted_active_from_symlink() {
    let home = isolated_home();
    let worktree = temp_dir("gts-func-wt");
    let opts = HomeOptions::with_home(&home);
    let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
    fs::create_dir_all(track_git_dir(&home, &registered.id, "main")).unwrap();
    fs::create_dir_all(track_git_dir(&home, &registered.id, "feature-v1")).unwrap();
    replace_git_symlink(
        &worktree,
        &fs::canonicalize(track_git_dir(&home, &registered.id, "feature-v1")).unwrap(),
    )
    .unwrap();

    let before: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(index_path(&home)).unwrap()).unwrap();
    assert_eq!(before[&registered.id]["active"], "main");

    let track = resolve_active_track(&worktree, &opts).unwrap();
    assert_eq!(track, "feature-v1");
    assert_eq!(
        read_index(&opts).unwrap().get(&registered.id).unwrap().active,
        "feature-v1"
    );
}

#[test]
fn errors_on_broken_or_out_of_storage_links() {
    let home = isolated_home();
    let worktree = temp_dir("gts-func-wt");
    let opts = HomeOptions::with_home(&home);
    let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
    fs::create_dir_all(track_git_dir(&home, &registered.id, "main")).unwrap();

    symlink(worktree.join("nope"), worktree.join(".git")).unwrap();
    let err = resolve_active_track(&worktree, &opts).unwrap_err();
    assert!(err.to_string().contains("Broken .git symlink"));

    let outside = temp_dir("gts-func-out");
    let outside_git = outside.join(".git");
    fs::create_dir_all(&outside_git).unwrap();
    replace_git_symlink(&worktree, &outside_git).unwrap();
    let err = resolve_active_track(&worktree, &opts).unwrap_err();
    assert!(err.to_string().contains("outside workspace storage"));
    assert_eq!(
        read_index(&opts).unwrap().get(&registered.id).unwrap().active,
        "main"
    );
}
