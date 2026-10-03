use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use crate::interactive::{is_interactive, InteractiveOptions};
use crate::prompts::{get_prompts, SelectItem, SelectOpts};
use crate::storage::{
    find_workspace_root_from, list_tracks, replace_git_symlink_to_track, resolve_active_track,
    resolve_gts_home, track_git_dir, validate_track_name, HomeOptions, StorageError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyChoice {
    Stash,
    Keep,
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchTrackResult {
    Switched { from: String, to: String },
    Noop { track: String },
    Aborted,
}

fn path_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok() || fs::symlink_metadata(path).is_ok()
}

fn run_git(args: &[&str], cwd: &Path) -> Result<(i32, String, String), StorageError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(StorageError::io)?;
    let code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((code, stdout, stderr))
}

pub fn is_worktree_dirty(worktree: &Path) -> Result<bool, StorageError> {
    let (code, stdout, stderr) = run_git(&["status", "--porcelain"], worktree)?;
    if code != 0 {
        return Err(StorageError::message(format!(
            "git status --porcelain failed (exit {code}): {}",
            stderr.trim()
        )));
    }
    Ok(!stdout.trim().is_empty())
}

pub fn stash_worktree_changes(worktree: &Path, source_track: &str) -> Result<(), StorageError> {
    let message = format!("gts: stash before switch from {source_track}");
    let (code, _stdout, stderr) =
        run_git(&["stash", "push", "-u", "-m", &message], worktree)?;
    if code != 0 {
        return Err(StorageError::message(format!(
            "git stash push -u failed (exit {code}): {}",
            stderr.trim()
        )));
    }
    Ok(())
}

fn default_resolve_dirty() -> Result<DirtyChoice, StorageError> {
    let prompts = get_prompts();
    let value = match prompts.select(SelectOpts {
        message: "Worktree has uncommitted changes".into(),
        items: vec![
            SelectItem {
                value: "stash".into(),
                label: "Stash changes before switching".into(),
                hint: String::new(),
            },
            SelectItem {
                value: "keep".into(),
                label: "Keep changes and carry them to the next track".into(),
                hint: String::new(),
            },
            SelectItem {
                value: "abort".into(),
                label: "Abort".into(),
                hint: String::new(),
            },
        ],
        initial_value: Some("stash".into()),
    }) {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::Interrupted => {
            return Ok(DirtyChoice::Abort);
        }
        Err(err) => return Err(StorageError::message(err.to_string())),
    };
    match value.as_str() {
        "stash" => Ok(DirtyChoice::Stash),
        "keep" => Ok(DirtyChoice::Keep),
        "abort" => Ok(DirtyChoice::Abort),
        other => Err(StorageError::message(format!(
            "Unexpected dirty choice: {other}"
        ))),
    }
}

/// Returns `(active, other_tracks)` for omit-track selection.
pub fn switch_target_choices(
    worktree: &Path,
    opts: &HomeOptions,
) -> Result<(String, Vec<String>), StorageError> {
    let found = find_workspace_root_from(worktree, opts)?;
    let active = resolve_active_track(&found.root, opts)?;
    let tracks = list_tracks(&found.id, opts)?;
    let others: Vec<String> = tracks.into_iter().filter(|t| t != &active).collect();
    Ok((active, others))
}

/// Note the current track, then select among other tracks only (no disabled option).
pub fn prompt_switch_target(worktree: &Path, opts: &HomeOptions) -> io::Result<String> {
    let (active, others) = switch_target_choices(worktree, opts)
        .map_err(|e| io::Error::other(e.to_string()))?;
    if others.is_empty() {
        return Err(io::Error::other(
            "No other tracks available to switch to",
        ));
    }
    let prompts = get_prompts();
    prompts.note("Current track", &format!("{active} (current)"))?;
    prompts.select(SelectOpts {
        message: "Switch to track".into(),
        items: others
            .into_iter()
            .map(|name| SelectItem {
                value: name.clone(),
                label: name,
                hint: String::new(),
            })
            .collect(),
        initial_value: None,
    })
}

pub struct SwitchTrackInput<'a> {
    pub worktree: &'a Path,
    pub track: &'a str,
    pub force: bool,
    pub stash: bool,
    pub no_interactive: bool,
    /// When set, overrides interactive detection.
    pub interactive: Option<bool>,
    pub home: &'a HomeOptions,
    pub resolve_dirty: Option<&'a mut dyn FnMut() -> Result<DirtyChoice, StorageError>>,
}

pub fn switch_track(input: SwitchTrackInput<'_>) -> Result<SwitchTrackResult, StorageError> {
    if input.force && input.stash {
        return Err(StorageError::message(
            "Cannot use --force and --stash together",
        ));
    }

    validate_track_name(input.track)?;
    let home = resolve_gts_home(input.home);
    let found = find_workspace_root_from(input.worktree, input.home)?;
    let worktree = found.root;
    let id = found.id;

    let current = resolve_active_track(&worktree, input.home)?;
    if current == input.track {
        return Ok(SwitchTrackResult::Noop {
            track: current,
        });
    }

    let tracks = list_tracks(&id, input.home)?;
    if !tracks.iter().any(|t| t == input.track) {
        return Err(StorageError::message(format!(
            "Track not found: {}",
            input.track
        )));
    }
    let dest_git = track_git_dir(&home, &id, input.track);
    if !path_exists(&dest_git) {
        return Err(StorageError::message(format!(
            "Track not found: {}",
            input.track
        )));
    }

    let dirty = is_worktree_dirty(&worktree)?;
    if dirty {
        let choice = if input.force {
            DirtyChoice::Keep
        } else if input.stash {
            DirtyChoice::Stash
        } else {
            let interactive = input.interactive.unwrap_or_else(|| {
                is_interactive(InteractiveOptions {
                    no_interactive: input.no_interactive,
                    ..Default::default()
                })
            });
            if !interactive {
                return Err(StorageError::message(
                    "Worktree has uncommitted changes. Re-run with --force or --stash: gts switch <track> --force",
                ));
            }
            if let Some(resolve) = input.resolve_dirty {
                resolve()?
            } else {
                default_resolve_dirty()?
            }
        };

        match choice {
            DirtyChoice::Abort => return Ok(SwitchTrackResult::Aborted),
            DirtyChoice::Stash => stash_worktree_changes(&worktree, &current)?,
            DirtyChoice::Keep => {}
        }
    }

    replace_git_symlink_to_track(&worktree, input.track, input.home)?;
    Ok(SwitchTrackResult::Switched {
        from: current,
        to: input.track.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create::create_track;
    use crate::init::{init_workspace, MoveGitDirOptions};
    use crate::storage::read_index;
    use std::path::PathBuf;
    use std::process::Command;

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
        let home = temp_dir("gts-switch-home");
        let real_gts = dirs::home_dir().unwrap().join(".gts");
        assert!(!home.starts_with(&real_gts));
        home
    }

    fn run_git_ok(args: &[&str], cwd: &Path) {
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
        run_git_ok(&["init"], worktree);
        fs::write(worktree.join("tracked.txt"), "base\n").unwrap();
        run_git_ok(&["add", "tracked.txt"], worktree);
        run_git_ok(&["commit", "-m", "init"], worktree);
        fs::write(worktree.join(".git").join("marker"), "from-main\n").unwrap();
        let opts = HomeOptions::with_home(home);
        let result =
            init_workspace(worktree, "main", &opts, &MoveGitDirOptions::default()).unwrap();
        result.id
    }

    fn base_input<'a>(
        worktree: &'a Path,
        track: &'a str,
        opts: &'a HomeOptions,
    ) -> SwitchTrackInput<'a> {
        SwitchTrackInput {
            worktree,
            track,
            force: false,
            stash: false,
            no_interactive: true,
            interactive: None,
            home: opts,
            resolve_dirty: None,
        }
    }

    #[test]
    fn noop_when_target_is_current() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        let before = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

        let result = switch_track(base_input(&worktree, "main", &opts)).unwrap();
        assert_eq!(
            result,
            SwitchTrackResult::Noop {
                track: "main".into()
            }
        );
        assert_eq!(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
            before
        );
    }

    #[test]
    fn retargets_symlink_and_active() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();

        let result = switch_track(base_input(&worktree, "feature", &opts)).unwrap();
        assert_eq!(
            result,
            SwitchTrackResult::Switched {
                from: "main".into(),
                to: "feature".into()
            }
        );
        let git_path = fs::canonicalize(&worktree).unwrap().join(".git");
        assert!(fs::symlink_metadata(&git_path).unwrap().file_type().is_symlink());
        let target = fs::read_link(&git_path).unwrap();
        assert!(target.is_absolute());
        assert_eq!(
            fs::canonicalize(&target).unwrap(),
            fs::canonicalize(track_git_dir(&home, &id, "feature")).unwrap()
        );
        assert_eq!(read_index(&opts).unwrap().get(&id).unwrap().active, "feature");
        assert_eq!(
            fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
            "base\n"
        );
    }

    #[test]
    fn missing_target_fails_without_write() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        let before = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

        let err = switch_track(base_input(&worktree, "missing", &opts)).unwrap_err();
        assert!(err.to_string().contains("Track not found"));
        assert_eq!(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
            before
        );
    }

    #[test]
    fn force_and_stash_together_fail() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();

        let mut input = base_input(&worktree, "feature", &opts);
        input.force = true;
        input.stash = true;
        let err = switch_track(input).unwrap_err();
        assert!(err.to_string().contains("--force and --stash"));
    }

    #[test]
    fn non_interactive_dirty_without_flags_fails() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();
        let before = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

        let err = switch_track(base_input(&worktree, "feature", &opts)).unwrap_err();
        assert!(err.to_string().contains("uncommitted changes"));
        assert_eq!(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
            before
        );
    }

    #[test]
    fn force_keeps_dirty_and_switches() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

        let mut input = base_input(&worktree, "feature", &opts);
        input.force = true;
        let result = switch_track(input).unwrap();
        assert!(matches!(result, SwitchTrackResult::Switched { .. }));
        assert_eq!(
            fs::read_to_string(worktree.join("dirty.txt")).unwrap(),
            "dirty\n"
        );
        assert_eq!(
            fs::canonicalize(
                fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
            )
            .unwrap(),
            fs::canonicalize(track_git_dir(&home, &id, "feature")).unwrap()
        );
    }

    #[test]
    fn stash_flag_stashes_then_switches() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

        let mut input = base_input(&worktree, "feature", &opts);
        input.stash = true;
        let result = switch_track(input).unwrap();
        assert!(matches!(result, SwitchTrackResult::Switched { .. }));
        assert!(!path_exists(&worktree.join("dirty.txt")));
        assert_eq!(
            fs::canonicalize(
                fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
            )
            .unwrap(),
            fs::canonicalize(track_git_dir(&home, &id, "feature")).unwrap()
        );

        let mut back = base_input(&worktree, "main", &opts);
        back.force = true;
        switch_track(back).unwrap();
        let (code, stdout, _) = run_git(&["stash", "list"], &worktree).unwrap();
        assert_eq!(code, 0);
        assert!(stdout.contains("stash before switch from main"));
    }

    #[test]
    fn abort_via_resolve_dirty_leaves_link() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();
        let before = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

        let mut resolver = || Ok(DirtyChoice::Abort);
        let mut input = base_input(&worktree, "feature", &opts);
        input.no_interactive = false;
        input.interactive = Some(true);
        input.resolve_dirty = Some(&mut resolver);
        let result = switch_track(input).unwrap();
        assert_eq!(result, SwitchTrackResult::Aborted);
        assert_eq!(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
            before
        );
    }

    #[test]
    fn stash_failure_does_not_switch() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();
        let git_real =
            fs::canonicalize(fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap())
                .unwrap();
        let objects = git_real.join("objects");
        let objects_bak = git_real.join("objects.bak");
        let before = fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap();

        let objects_move = objects.clone();
        let objects_bak_move = objects_bak.clone();
        let mut resolver = move || {
            fs::rename(&objects_move, &objects_bak_move).unwrap();
            Ok(DirtyChoice::Stash)
        };
        let mut input = base_input(&worktree, "feature", &opts);
        input.no_interactive = false;
        input.interactive = Some(true);
        input.resolve_dirty = Some(&mut resolver);
        let err = switch_track(input).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("stash"));
        assert_eq!(
            fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap(),
            before
        );
        let _ = fs::rename(&objects_bak, &objects);
    }

    #[test]
    fn keep_via_resolve_dirty_switches() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        let id = init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();

        let mut resolver = || Ok(DirtyChoice::Keep);
        let mut input = base_input(&worktree, "feature", &opts);
        input.no_interactive = false;
        input.interactive = Some(true);
        input.resolve_dirty = Some(&mut resolver);
        let result = switch_track(input).unwrap();
        assert!(matches!(result, SwitchTrackResult::Switched { .. }));
        assert_eq!(
            fs::read_to_string(worktree.join("dirty.txt")).unwrap(),
            "dirty\n"
        );
        assert_eq!(
            fs::canonicalize(
                fs::read_link(fs::canonicalize(&worktree).unwrap().join(".git")).unwrap()
            )
            .unwrap(),
            fs::canonicalize(track_git_dir(&home, &id, "feature")).unwrap()
        );
        assert!(!home.starts_with(dirs::home_dir().unwrap().join(".gts")));
    }

    #[test]
    fn switch_target_choices_excludes_current() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);
        create_track(&worktree, "feature", false, &opts).unwrap();
        create_track(&worktree, "other", false, &opts).unwrap();

        let (active, others) = switch_target_choices(&worktree, &opts).unwrap();
        assert_eq!(active, "main");
        assert!(!others.iter().any(|t| t == "main"));
        assert!(others.contains(&"feature".to_string()));
        assert!(others.contains(&"other".to_string()));
    }

    #[test]
    fn switch_target_choices_empty_when_only_current() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let (active, others) = switch_target_choices(&worktree, &opts).unwrap();
        assert_eq!(active, "main");
        assert!(others.is_empty());
    }

    #[test]
    fn prompt_switch_target_fails_when_no_others() {
        let home = temp_home();
        let worktree = temp_dir("gts-switch-wt");
        init_plain(&worktree, &home);
        let opts = HomeOptions::with_home(&home);

        let err = prompt_switch_target(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("No other tracks"));
    }
}
