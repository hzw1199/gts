use std::path::{Path, PathBuf};

use crate::exit::{exit_failure, map_prompt_result, AppError};
use crate::interactive::{is_interactive, InteractiveOptions};
use crate::prompts::{get_prompts, SelectItem, SelectOpts};
use crate::storage::{find_workspace_root_from, resolve_active_track, HomeOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Status,
    Switch,
    Create,
    Rename,
    Ignore,
    Remove,
}

impl MenuAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Switch => "switch",
            Self::Create => "create",
            Self::Rename => "rename",
            Self::Ignore => "ignore",
            Self::Remove => "remove",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "status" => Some(Self::Status),
            "switch" => Some(Self::Switch),
            "create" => Some(Self::Create),
            "rename" => Some(Self::Rename),
            "ignore" => Some(Self::Ignore),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }
}

pub const MISSING_BARE_COMMAND_HINT: &str = "No command specified. Run: gts status";

pub fn build_action_menu_options(active: &str) -> Vec<SelectItem> {
    vec![
        SelectItem {
            value: MenuAction::Status.as_str().into(),
            label: "Status".into(),
            hint: String::new(),
        },
        SelectItem {
            value: MenuAction::Switch.as_str().into(),
            label: format!("Switch track (current: {active})"),
            hint: String::new(),
        },
        SelectItem {
            value: MenuAction::Create.as_str().into(),
            label: "Create track".into(),
            hint: String::new(),
        },
        SelectItem {
            value: MenuAction::Rename.as_str().into(),
            label: "Rename track".into(),
            hint: String::new(),
        },
        SelectItem {
            value: MenuAction::Ignore.as_str().into(),
            label: "Edit local ignore (exclude)".into(),
            hint: String::new(),
        },
        SelectItem {
            value: MenuAction::Remove.as_str().into(),
            label: "Remove track".into(),
            hint: String::new(),
        },
    ]
}

#[derive(Debug, Clone)]
pub struct RegisteredWorkspaceInfo {
    pub id: String,
    pub root: PathBuf,
    pub active: String,
}

pub fn try_find_registered_workspace(
    cwd: &Path,
    opts: &HomeOptions,
) -> Option<RegisteredWorkspaceInfo> {
    let found = find_workspace_root_from(cwd, opts).ok()?;
    let active = resolve_active_track(&found.root, opts)
        .unwrap_or_else(|_| found.entry.active.clone());
    Some(RegisteredWorkspaceInfo {
        id: found.id,
        root: found.root,
        active,
    })
}

pub struct BareCommandInput<'a> {
    pub no_interactive: bool,
    /// When set, overrides interactive detection (for tests).
    pub interactive: Option<bool>,
    pub cwd: &'a Path,
    pub home: &'a HomeOptions,
}

pub fn run_bare_command(
    input: BareCommandInput<'_>,
    on_init: &mut dyn FnMut() -> Result<(), AppError>,
    on_action: &mut dyn FnMut(MenuAction) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let interactive = input.interactive.unwrap_or_else(|| {
        is_interactive(InteractiveOptions {
            no_interactive: input.no_interactive,
            ..Default::default()
        })
    });

    if !interactive {
        return Err(exit_failure(MISSING_BARE_COMMAND_HINT));
    }

    let Some(registered) = try_find_registered_workspace(input.cwd, input.home) else {
        return on_init();
    };

    let prompts = get_prompts();
    let dir_name = input
        .cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.cwd.display().to_string());
    map_prompt_result(prompts.intro(&format!("gts · {dir_name}")))?;

    let selected = map_prompt_result(prompts.select(SelectOpts {
        message: "What do you want to do?".into(),
        items: build_action_menu_options(&registered.active),
        initial_value: None,
    }))?;

    let action = MenuAction::parse(&selected)
        .ok_or_else(|| exit_failure(format!("Unknown menu action: {selected}")))?;
    on_action(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create::create_track;
    use crate::init::{init_workspace, MoveGitDirOptions};
    use crate::prompts::{create_scripted_prompts, set_prompts_for_tests};
    use serde_json::json;
    use std::fs;
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
        let home = temp_dir("gts-menu-home");
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

    fn seeded(worktree: &Path, home: &Path) {
        run_git_ok(&["init"], worktree);
        fs::write(worktree.join("f.txt"), "x\n").unwrap();
        run_git_ok(&["add", "f.txt"], worktree);
        run_git_ok(&["commit", "-m", "i"], worktree);
        let opts = HomeOptions::with_home(home);
        init_workspace(worktree, "main", &opts, &MoveGitDirOptions::default()).unwrap();
        create_track(worktree, "feature", false, &opts).unwrap();
    }

    #[test]
    fn builds_six_options_status_first_with_current_in_switch() {
        let opts = build_action_menu_options("main");
        let labels: Vec<_> = opts.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Status",
                "Switch track (current: main)",
                "Create track",
                "Rename track",
                "Edit local ignore (exclude)",
                "Remove track",
            ]
        );
        assert_eq!(opts[0].value, "status");
    }

    #[test]
    fn bare_hint_is_copy_pasteable() {
        assert!(MISSING_BARE_COMMAND_HINT.contains("gts status"));
    }

    #[test]
    fn try_find_registered_none_when_unregistered() {
        let home = temp_home();
        let wt = temp_dir("gts-menu-wt");
        let opts = HomeOptions::with_home(&home);
        assert!(try_find_registered_workspace(&wt, &opts).is_none());
    }

    #[test]
    fn try_find_registered_returns_active() {
        let home = temp_home();
        let wt = temp_dir("gts-menu-wt");
        seeded(&wt, &home);
        let opts = HomeOptions::with_home(&home);
        let found = try_find_registered_workspace(&wt, &opts).unwrap();
        assert_eq!(found.active, "main");
        assert!(!home.starts_with(dirs::home_dir().unwrap().join(".gts")));
    }

    #[test]
    fn non_interactive_bare_fails_with_hint() {
        let home = temp_home();
        let wt = temp_dir("gts-menu-wt");
        seeded(&wt, &home);
        let opts = HomeOptions::with_home(&home);
        let mut init_called = false;
        let mut action_called = false;
        let err = run_bare_command(
            BareCommandInput {
                no_interactive: true,
                interactive: Some(false),
                cwd: &wt,
                home: &opts,
            },
            &mut || {
                init_called = true;
                Ok(())
            },
            &mut |_| {
                action_called = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(matches!(err, AppError::Failure(ref m) if m.contains("gts status")));
        assert!(!init_called);
        assert!(!action_called);
    }

    #[test]
    fn interactive_bare_routes_status_action() {
        let home = temp_home();
        let wt = temp_dir("gts-menu-wt");
        seeded(&wt, &home);
        let opts = HomeOptions::with_home(&home);
        set_prompts_for_tests(Some(create_scripted_prompts(vec![json!("status")])));
        let mut got = None;
        run_bare_command(
            BareCommandInput {
                no_interactive: false,
                interactive: Some(true),
                cwd: &wt,
                home: &opts,
            },
            &mut || Ok(()),
            &mut |action| {
                got = Some(action);
                Ok(())
            },
        )
        .unwrap();
        set_prompts_for_tests(None);
        assert_eq!(got, Some(MenuAction::Status));
    }

    #[test]
    fn interactive_unregistered_calls_init() {
        let home = temp_home();
        let wt = temp_dir("gts-menu-wt");
        let opts = HomeOptions::with_home(&home);
        let mut init_called = false;
        run_bare_command(
            BareCommandInput {
                no_interactive: false,
                interactive: Some(true),
                cwd: &wt,
                home: &opts,
            },
            &mut || {
                init_called = true;
                Ok(())
            },
            &mut |_| panic!("should not show menu"),
        )
        .unwrap();
        assert!(init_called);
    }
}
