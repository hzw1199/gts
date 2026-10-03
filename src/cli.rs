use std::io::Read;

use clap::{ArgAction, Parser, Subcommand};

use crate::completion::{run_completion, CompletionInput};
use crate::create::{create_track, switch_to_new_track};
use crate::exit::{exit_failure, map_prompt_result, AppError};
use crate::ignore::{append_exclude_pattern, open_exclude_in_editor, print_exclude, set_exclude};
use crate::init::{init_workspace, MoveGitDirOptions};
use crate::interactive::{is_interactive, InteractiveOptions};
use crate::menu::{run_bare_command, BareCommandInput, MenuAction};
use crate::prompts::{get_prompts, ConfirmOpts, InputOpts, SelectItem, SelectOpts};
use crate::remove::{remove_track, RemoveTrackResult};
use crate::rename::{rename_track, RenameTrackResult};
use crate::status::{format_status_human, get_workspace_status};
use crate::storage::{
    find_workspace_root_from, list_tracks, resolve_active_track, HomeOptions,
};
use crate::switch::{prompt_switch_target, switch_track, SwitchTrackInput, SwitchTrackResult};

const MISSING_INIT_NAME_HINT: &str = "Missing required argument. Run: gts init <name>";
const MISSING_CREATE_NAME_HINT: &str = "Missing required argument. Run: gts create <track>";
const MISSING_SWITCH_NAME_HINT: &str = "Missing required argument. Run: gts switch <track>";
const MISSING_RENAME_HINT: &str = "Missing required argument. Run: gts rename <from> <to>";
const MISSING_REMOVE_HINT: &str = "Missing required argument. Run: gts remove <track> --yes";
const MISSING_REMOVE_YES_HINT: &str =
    "Non-interactive remove requires --yes. Run: gts remove <track> --yes";

#[derive(Debug, Parser)]
#[command(
    name = "gts",
    about = "Multiple independent Git histories per workspace"
)]
struct Cli {
    /// Disable interactive prompts
    #[arg(long = "no-interactive", global = true, action = ArgAction::SetTrue)]
    no_interactive: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Register the current directory as a gts workspace
    Init {
        /// Initial track name
        #[arg(value_name = "name")]
        name: Option<String>,
    },
    /// Show current track, link target, and track list
    Status {
        /// Emit machine-readable JSON
        #[arg(long, action = ArgAction::SetTrue)]
        json: bool,
    },
    /// Create a new track under storage
    Create {
        /// New track name
        #[arg(value_name = "track")]
        track: Option<String>,
        /// Copy the current track Git directory
        #[arg(long, action = ArgAction::SetTrue)]
        clone: bool,
        /// Switch .git symlink to the new track
        #[arg(long, action = ArgAction::SetTrue)]
        switch: bool,
        /// When switching, keep dirty changes without prompting
        #[arg(long, action = ArgAction::SetTrue)]
        force: bool,
        /// When switching, stash dirty changes without prompting
        #[arg(long, action = ArgAction::SetTrue)]
        stash: bool,
    },
    /// Switch the workspace .git symlink to another track
    Switch {
        /// Target track name
        #[arg(value_name = "track")]
        track: Option<String>,
        /// Keep dirty changes and switch without prompting
        #[arg(long, action = ArgAction::SetTrue)]
        force: bool,
        /// Stash dirty changes then switch without prompting
        #[arg(long, action = ArgAction::SetTrue)]
        stash: bool,
    },
    /// Alias for switch
    #[command(name = "to")]
    To {
        /// Target track name
        #[arg(value_name = "track")]
        track: Option<String>,
        /// Keep dirty changes and switch without prompting
        #[arg(long, action = ArgAction::SetTrue)]
        force: bool,
        /// Stash dirty changes then switch without prompting
        #[arg(long, action = ArgAction::SetTrue)]
        stash: bool,
    },
    /// Rename a track under storage
    Rename {
        /// Current track name
        #[arg(value_name = "from")]
        from: Option<String>,
        /// New track name
        #[arg(value_name = "to")]
        to: Option<String>,
    },
    /// Remove a track under storage
    Remove {
        /// Track name to remove
        #[arg(value_name = "track")]
        track: Option<String>,
        /// Confirm removal without prompting
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
    /// Append to, open, print, or set a track info/exclude
    Ignore {
        /// Pattern to append to info/exclude
        #[arg(value_name = "pattern")]
        pattern: Option<String>,
        /// Track name for --print / --set
        #[arg(long, value_name = "name")]
        track: Option<String>,
        /// Print the track info/exclude to stdout
        #[arg(long, action = ArgAction::SetTrue)]
        print: bool,
        /// Replace the track info/exclude with stdin
        #[arg(long, action = ArgAction::SetTrue)]
        set: bool,
    },
    /// Print shell completion script or list track names
    Completion {
        /// Shell type: bash or zsh
        #[arg(value_name = "shell")]
        shell: Option<String>,
        /// List track names for the current workspace
        #[arg(long, action = ArgAction::SetTrue)]
        tracks: bool,
    },
    /// Skeleton demo command (no track operations)
    Demo {
        /// Demo name
        #[arg(long)]
        name: Option<String>,
    },
}

pub fn run() -> i32 {
    match run_inner() {
        Ok(()) => 0,
        Err(err) => {
            if let AppError::Failure(message) = &err {
                eprintln!("{message}");
            }
            err.exit_code()
        }
    }
}

fn run_inner() -> Result<(), AppError> {
    let cli = Cli::parse();
    match cli.command {
        None => run_bare(cli.no_interactive),
        Some(Commands::Init { name }) => run_init(name, cli.no_interactive, false),
        Some(Commands::Status { json }) => run_status(json),
        Some(Commands::Create {
            track,
            clone,
            switch,
            force,
            stash,
        }) => run_create(
            track,
            clone,
            switch,
            force,
            stash,
            cli.no_interactive,
            false,
            false,
        ),
        Some(Commands::Switch { track, force, stash } | Commands::To { track, force, stash }) => {
            run_switch(track, force, stash, cli.no_interactive, false)
        }
        Some(Commands::Rename { from, to }) => run_rename(from, to, cli.no_interactive, false),
        Some(Commands::Remove { track, yes }) => {
            run_remove(track, yes, cli.no_interactive, false)
        }
        Some(Commands::Ignore {
            pattern,
            track,
            print,
            set,
        }) => run_ignore(pattern, track, print, set, cli.no_interactive, false),
        Some(Commands::Completion { shell, tracks }) => run_completion_cmd(shell, tracks),
        Some(Commands::Demo { name }) => run_demo(name, cli.no_interactive),
    }
}

fn run_completion_cmd(shell: Option<String>, tracks: bool) -> Result<(), AppError> {
    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    run_completion(CompletionInput {
        shell: shell.as_deref(),
        tracks,
        cwd: &cwd,
    })
}

fn run_bare(no_interactive: bool) -> Result<(), AppError> {
    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let opts = HomeOptions::default();
    run_bare_command(
        BareCommandInput {
            no_interactive,
            interactive: None,
            cwd: &cwd,
            home: &opts,
        },
        &mut || run_init(None, no_interactive, false),
        &mut |action| match action {
            MenuAction::Status => run_status(false),
            MenuAction::Switch => run_switch(None, false, false, no_interactive, true),
            MenuAction::Create => run_create(
                None,
                false,
                false,
                false,
                false,
                no_interactive,
                true,
                true,
            ),
            MenuAction::Rename => run_rename(None, None, no_interactive, true),
            MenuAction::Ignore => run_ignore(None, None, false, false, no_interactive, true),
            MenuAction::Remove => run_remove(None, false, no_interactive, true),
        },
    )
}

fn run_status(json: bool) -> Result<(), AppError> {
    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let opts = HomeOptions::default();
    let status = get_workspace_status(&cwd, &opts).map_err(|e| exit_failure(e.to_string()))?;
    if json {
        let payload = serde_json::to_string(&status).map_err(|e| exit_failure(e.to_string()))?;
        println!("{payload}");
    } else {
        println!("{}", format_status_human(&status));
    }
    Ok(())
}

fn run_init(name: Option<String>, no_interactive: bool, skip_intro: bool) -> Result<(), AppError> {
    if std::env::var("GTS_SIMULATE_CANCEL").ok().as_deref() == Some("1") {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        return Err(AppError::Cancelled);
    }

    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let dir_name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    let interactive = is_interactive(InteractiveOptions {
        no_interactive,
        ..Default::default()
    });

    let mut track = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    if interactive && !skip_intro {
        map_prompt_result(get_prompts().intro(&format!("gts init · {dir_name}")))?;
    }

    if track.is_none() {
        if !interactive {
            return Err(exit_failure(MISSING_INIT_NAME_HINT));
        }

        let answered = map_prompt_result(get_prompts().input(InputOpts {
            message: "Initial track name".into(),
            placeholder: Some("main".into()),
            default_input: Some("main".into()),
        }))?;
        let trimmed = answered.trim().to_string();
        track = Some(if trimmed.is_empty() {
            "main".to_string()
        } else {
            trimmed
        });
    }

    let track = track.expect("track name resolved");
    let opts = HomeOptions::default();
    let result = init_workspace(&cwd, &track, &opts, &MoveGitDirOptions::default())
        .map_err(|e| exit_failure(e.to_string()))?;

    let message = format!("Initialized track \"{}\" ({})", result.track, result.id);
    if interactive {
        map_prompt_result(get_prompts().outro(&message))?;
    } else {
        println!("{message}");
    }
    Ok(())
}

fn run_create(
    track: Option<String>,
    clone: bool,
    switch_flag: bool,
    force: bool,
    stash: bool,
    no_interactive: bool,
    skip_intro: bool,
    ask_clone: bool,
) -> Result<(), AppError> {
    if std::env::var("GTS_SIMULATE_CANCEL").ok().as_deref() == Some("1") {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        return Err(AppError::Cancelled);
    }

    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let dir_name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    let interactive = is_interactive(InteractiveOptions {
        no_interactive,
        ..Default::default()
    });

    let mut track = track.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let mut clone = clone;

    if interactive && !skip_intro {
        map_prompt_result(get_prompts().intro(&format!("gts create · {dir_name}")))?;
    }

    if track.is_none() {
        if !interactive {
            return Err(exit_failure(MISSING_CREATE_NAME_HINT));
        }

        let answered = map_prompt_result(get_prompts().input(InputOpts {
            message: "New track name".into(),
            placeholder: Some("feature".into()),
            default_input: None,
        }))?;
        let trimmed = answered.trim().to_string();
        if trimmed.is_empty() {
            return Err(exit_failure(MISSING_CREATE_NAME_HINT));
        }
        track = Some(trimmed);
    }

    let track = track.expect("track name resolved");

    if ask_clone && interactive && !clone {
        let confirmed = map_prompt_result(get_prompts().confirm(ConfirmOpts {
            message: "Clone current track history?".into(),
            initial_value: false,
        }))?;
        clone = confirmed;
    }

    if force && stash {
        return Err(exit_failure("Cannot use --force and --stash together"));
    }

    let opts = HomeOptions::default();
    let result = create_track(&cwd, &track, clone, &opts).map_err(|e| exit_failure(e.to_string()))?;

    let mut switched = false;
    let mut want_switch = switch_flag;
    if !want_switch && interactive {
        let confirmed = map_prompt_result(get_prompts().confirm(ConfirmOpts {
            message: format!("Switch to track \"{track}\" now?"),
            initial_value: false,
        }))?;
        want_switch = confirmed;
    }

    if want_switch {
        let sw = switch_to_new_track(&cwd, &track, &opts, force, stash, no_interactive)
            .map_err(|e| exit_failure(e.to_string()))?;
        match sw {
            SwitchTrackResult::Aborted => {
                let _ = get_prompts().outro_cancel("Operation cancelled.");
                return Err(AppError::Cancelled);
            }
            SwitchTrackResult::Switched { .. } | SwitchTrackResult::Noop { .. } => {
                switched = true;
            }
        }
    }

    let message = if switched {
        format!("Created and switched to track \"{}\"", result.track)
    } else {
        format!("Created track \"{}\"", result.track)
    };
    if interactive {
        map_prompt_result(get_prompts().outro(&message))?;
    } else {
        println!("{message}");
    }
    Ok(())
}

fn run_switch(
    track: Option<String>,
    force: bool,
    stash: bool,
    no_interactive: bool,
    skip_intro: bool,
) -> Result<(), AppError> {
    if std::env::var("GTS_SIMULATE_CANCEL").ok().as_deref() == Some("1") {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        return Err(AppError::Cancelled);
    }

    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let dir_name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    let interactive = is_interactive(InteractiveOptions {
        no_interactive,
        ..Default::default()
    });

    let mut track = track.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    if track.is_none() {
        if !interactive {
            return Err(exit_failure(MISSING_SWITCH_NAME_HINT));
        }
        if !skip_intro {
            map_prompt_result(get_prompts().intro(&format!("gts switch · {dir_name}")))?;
        }
        let opts = HomeOptions::default();
        track = Some(map_prompt_result(prompt_switch_target(&cwd, &opts))?);
    } else if interactive && !skip_intro {
        map_prompt_result(get_prompts().intro(&format!("gts switch · {dir_name}")))?;
    }

    let track = track.expect("track name resolved");
    if force && stash {
        return Err(exit_failure("Cannot use --force and --stash together"));
    }

    let opts = HomeOptions::default();
    let result = switch_track(SwitchTrackInput {
        worktree: &cwd,
        track: &track,
        force,
        stash,
        no_interactive,
        interactive: None,
        home: &opts,
        resolve_dirty: None,
    })
    .map_err(|e| exit_failure(e.to_string()))?;

    match result {
        SwitchTrackResult::Aborted => {
            let _ = get_prompts().outro_cancel("Operation cancelled.");
            return Err(AppError::Cancelled);
        }
        SwitchTrackResult::Noop { track } => {
            let message = format!("Already on track \"{track}\"");
            if interactive {
                map_prompt_result(get_prompts().outro(&message))?;
            } else {
                println!("{message}");
            }
        }
        SwitchTrackResult::Switched { to, .. } => {
            let message = format!("Switched to track \"{to}\"");
            if interactive {
                map_prompt_result(get_prompts().outro(&message))?;
            } else {
                println!("{message}");
            }
        }
    }
    Ok(())
}

fn run_rename(
    from: Option<String>,
    to: Option<String>,
    no_interactive: bool,
    skip_intro: bool,
) -> Result<(), AppError> {
    if std::env::var("GTS_SIMULATE_CANCEL").ok().as_deref() == Some("1") {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        return Err(AppError::Cancelled);
    }

    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let dir_name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    let interactive = is_interactive(InteractiveOptions {
        no_interactive,
        ..Default::default()
    });

    let mut from = from.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let mut to = to.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    let prompts = get_prompts();
    if interactive && !skip_intro {
        map_prompt_result(prompts.intro(&format!("gts rename · {dir_name}")))?;
    }

    if from.is_none() || to.is_none() {
        if !interactive {
            return Err(exit_failure(MISSING_RENAME_HINT));
        }

        let opts = HomeOptions::default();
        let found = find_workspace_root_from(&cwd, &opts).map_err(|e| exit_failure(e.to_string()))?;
        let tracks = list_tracks(&found.id, &opts).map_err(|e| exit_failure(e.to_string()))?;
        if tracks.is_empty() {
            return Err(exit_failure("No tracks found in this workspace"));
        }

        if from.is_none() {
            let selected = map_prompt_result(prompts.select(SelectOpts {
                message: "Track to rename".into(),
                items: tracks
                    .iter()
                    .map(|name| SelectItem {
                        value: name.clone(),
                        label: name.clone(),
                        hint: String::new(),
                    })
                    .collect(),
                initial_value: None,
            }))?;
            from = Some(selected);
        }

        if to.is_none() {
            let answered = map_prompt_result(prompts.input(InputOpts {
                message: "New track name".into(),
                placeholder: Some("new-name".into()),
                default_input: None,
            }))?;
            let trimmed = answered.trim().to_string();
            if trimmed.is_empty() {
                return Err(exit_failure(MISSING_RENAME_HINT));
            }
            to = Some(trimmed);
        }
    }

    let from = from.expect("from resolved");
    let to = to.expect("to resolved");
    let opts = HomeOptions::default();
    let result =
        rename_track(&cwd, &from, &to, &opts).map_err(|e| exit_failure(e.to_string()))?;

    let message = match result {
        RenameTrackResult::Noop { track } => format!("Track already named \"{track}\""),
        RenameTrackResult::Renamed { from, to, .. } => {
            format!("Renamed track \"{from}\" to \"{to}\"")
        }
    };
    if interactive {
        map_prompt_result(prompts.outro(&message))?;
    } else {
        println!("{message}");
    }
    Ok(())
}

fn run_remove(
    track: Option<String>,
    yes: bool,
    no_interactive: bool,
    skip_intro: bool,
) -> Result<(), AppError> {
    if std::env::var("GTS_SIMULATE_CANCEL").ok().as_deref() == Some("1") {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        return Err(AppError::Cancelled);
    }

    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let dir_name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    let interactive = is_interactive(InteractiveOptions {
        no_interactive,
        ..Default::default()
    });

    let mut track = track.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    let prompts = get_prompts();
    if interactive && !skip_intro {
        map_prompt_result(prompts.intro(&format!("gts remove · {dir_name}")))?;
    }

    if track.is_none() {
        if !interactive {
            return Err(exit_failure(MISSING_REMOVE_HINT));
        }

        let opts = HomeOptions::default();
        let found = find_workspace_root_from(&cwd, &opts).map_err(|e| exit_failure(e.to_string()))?;
        let active = resolve_active_track(&found.root, &opts)
            .map_err(|e| exit_failure(e.to_string()))?;
        let tracks = list_tracks(&found.id, &opts).map_err(|e| exit_failure(e.to_string()))?;
        let removable: Vec<_> = tracks.into_iter().filter(|name| name != &active).collect();
        if removable.is_empty() {
            return Err(exit_failure(
                "No removable tracks (cannot remove the active track)",
            ));
        }
        let selected = map_prompt_result(prompts.select(SelectOpts {
            message: "Track to remove".into(),
            items: removable
                .into_iter()
                .map(|name| SelectItem {
                    value: name.clone(),
                    label: name,
                    hint: String::new(),
                })
                .collect(),
            initial_value: None,
        }))?;
        track = Some(selected);
    }

    let track = track.expect("track resolved");

    if !interactive && !yes {
        return Err(exit_failure(
            MISSING_REMOVE_YES_HINT.replace("<track>", &track),
        ));
    }

    if interactive && !yes {
        let confirmed = map_prompt_result(prompts.confirm(ConfirmOpts {
            message: format!(
                "Delete track \"{track}\"? This permanently removes its Git history and info/exclude."
            ),
            initial_value: false,
        }))?;
        if !confirmed {
            let _ = prompts.outro_cancel("Operation cancelled.");
            return Err(AppError::Cancelled);
        }
    }

    let opts = HomeOptions::default();
    let RemoveTrackResult { track } =
        remove_track(&cwd, &track, &opts).map_err(|e| exit_failure(e.to_string()))?;

    let message = format!("Removed track \"{track}\"");
    if interactive {
        map_prompt_result(prompts.outro(&message))?;
    } else {
        println!("{message}");
    }
    Ok(())
}

fn run_ignore(
    pattern: Option<String>,
    track: Option<String>,
    print: bool,
    set: bool,
    no_interactive: bool,
    skip_intro: bool,
) -> Result<(), AppError> {
    if std::env::var("GTS_SIMULATE_CANCEL").ok().as_deref() == Some("1") {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        return Err(AppError::Cancelled);
    }

    let cwd = std::env::current_dir().map_err(|e| exit_failure(e.to_string()))?;
    let dir_name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string());
    let interactive = is_interactive(InteractiveOptions {
        no_interactive,
        ..Default::default()
    });

    let pattern = pattern.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let track_opt = track
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    if print && set {
        return Err(exit_failure("Use only one of --print or --set"));
    }
    if (print || set) && pattern.is_some() {
        return Err(exit_failure("Do not pass a pattern with --print or --set"));
    }
    if (print || set) && track_opt.is_none() {
        return Err(exit_failure(
            "Missing --track for --print/--set. Example: gts ignore --track <name> --print",
        ));
    }
    if !print && !set && track_opt.is_some() {
        return Err(exit_failure("--track requires --print or --set"));
    }

    let opts = HomeOptions::default();

    if print {
        let result = print_exclude(&cwd, track_opt.as_deref().unwrap(), &opts)
            .map_err(|e| exit_failure(e.to_string()))?;
        print!("{}", result.content);
        return Ok(());
    }

    if set {
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .map_err(|e| exit_failure(e.to_string()))?;
        let result = set_exclude(&cwd, track_opt.as_deref().unwrap(), &content, &opts)
            .map_err(|e| exit_failure(e.to_string()))?;
        let message = format!("Updated info/exclude for track \"{}\"", result.track);
        if interactive {
            map_prompt_result(get_prompts().outro(&message))?;
        } else {
            println!("{message}");
        }
        return Ok(());
    }

    let prompts = get_prompts();
    if interactive && !skip_intro {
        map_prompt_result(prompts.intro(&format!("gts ignore · {dir_name}")))?;
    }

    let message = if let Some(pattern) = pattern {
        let result =
            append_exclude_pattern(&cwd, &pattern, &opts).map_err(|e| exit_failure(e.to_string()))?;
        format!(
            "Appended \"{}\" to track \"{}\" info/exclude",
            result.pattern, result.track
        )
    } else {
        let result =
            open_exclude_in_editor(&cwd, &opts, None).map_err(|e| exit_failure(e.to_string()))?;
        format!("Opened info/exclude for track \"{}\"", result.track)
    };

    if interactive {
        map_prompt_result(prompts.outro(&message))?;
    } else {
        println!("{message}");
    }
    Ok(())
}

fn run_demo(name: Option<String>, no_interactive: bool) -> Result<(), AppError> {
    const MISSING_NAME_HINT: &str = "Missing required argument. Run: gts demo --name <name>";

    if std::env::var("GTS_SIMULATE_CANCEL").ok().as_deref() == Some("1") {
        let _ = get_prompts().outro_cancel("Operation cancelled.");
        return Err(AppError::Cancelled);
    }

    let mut name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let interactive = is_interactive(InteractiveOptions {
        no_interactive,
        ..Default::default()
    });

    if name.is_none() {
        if !interactive {
            return Err(exit_failure(MISSING_NAME_HINT));
        }

        let prompts = get_prompts();
        map_prompt_result(prompts.intro("gts demo"))?;
        let answered = map_prompt_result(prompts.input(InputOpts {
            message: "Enter a name".into(),
            placeholder: Some("example".into()),
            default_input: None,
        }))?;
        let trimmed = answered.trim().to_string();
        if trimmed.is_empty() {
            return Err(exit_failure(MISSING_NAME_HINT));
        }
        map_prompt_result(prompts.outro(&format!("Demo name: {trimmed}")))?;
        return Ok(());
    }

    println!("Demo name: {}", name.take().unwrap());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_includes_switch_create_flags() {
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let mut help = Vec::new();
        cmd.write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();
        assert!(help.contains("--no-interactive"));
        assert!(help.contains("init"));
        assert!(help.contains("status"));
        assert!(help.contains("create"));
        assert!(help.contains("switch"));
        assert!(help.contains("to"));
        assert!(help.contains("rename"));
        assert!(help.contains("remove"));
        assert!(help.contains("ignore"));
        assert!(help.contains("completion"));
        assert!(help.contains("Usage:"));

        let mut status_cmd = Cli::command();
        let status = status_cmd.find_subcommand_mut("status").unwrap();
        let mut status_help = Vec::new();
        status.write_long_help(&mut status_help).unwrap();
        let status_help = String::from_utf8(status_help).unwrap();
        assert!(status_help.contains("--json"));

        let mut create_cmd = Cli::command();
        let create = create_cmd.find_subcommand_mut("create").unwrap();
        let mut create_help = Vec::new();
        create.write_long_help(&mut create_help).unwrap();
        let create_help = String::from_utf8(create_help).unwrap();
        assert!(create_help.contains("--clone"));
        assert!(create_help.contains("--switch"));
        assert!(create_help.contains("--force"));
        assert!(create_help.contains("--stash"));

        let mut switch_cmd = Cli::command();
        let switch = switch_cmd.find_subcommand_mut("switch").unwrap();
        let mut switch_help = Vec::new();
        switch.write_long_help(&mut switch_help).unwrap();
        let switch_help = String::from_utf8(switch_help).unwrap();
        assert!(switch_help.contains("--force"));
        assert!(switch_help.contains("--stash"));

        let mut rename_cmd = Cli::command();
        let rename = rename_cmd.find_subcommand_mut("rename").unwrap();
        let mut rename_help = Vec::new();
        rename.write_long_help(&mut rename_help).unwrap();
        let rename_help = String::from_utf8(rename_help).unwrap();
        assert!(rename_help.contains("[from]"));
        assert!(rename_help.contains("[to]"));

        let mut remove_cmd = Cli::command();
        let remove = remove_cmd.find_subcommand_mut("remove").unwrap();
        let mut remove_help = Vec::new();
        remove.write_long_help(&mut remove_help).unwrap();
        let remove_help = String::from_utf8(remove_help).unwrap();
        assert!(remove_help.contains("[track]"));
        assert!(remove_help.contains("--yes"));

        let mut ignore_cmd = Cli::command();
        let ignore = ignore_cmd.find_subcommand_mut("ignore").unwrap();
        let mut ignore_help = Vec::new();
        ignore.write_long_help(&mut ignore_help).unwrap();
        let ignore_help = String::from_utf8(ignore_help).unwrap();
        assert!(ignore_help.contains("[pattern]"));
        assert!(ignore_help.contains("info/exclude"));
        assert!(ignore_help.contains("--track"));
        assert!(ignore_help.contains("--print"));
        assert!(ignore_help.contains("--set"));

        let mut completion_cmd = Cli::command();
        let completion = completion_cmd.find_subcommand_mut("completion").unwrap();
        let mut completion_help = Vec::new();
        completion.write_long_help(&mut completion_help).unwrap();
        let completion_help = String::from_utf8(completion_help).unwrap();
        assert!(completion_help.contains("[shell]"));
        assert!(completion_help.contains("--tracks"));
    }

    #[test]
    fn missing_name_hints_are_copy_pasteable() {
        assert!(MISSING_CREATE_NAME_HINT.contains("gts create <track>"));
        assert!(MISSING_SWITCH_NAME_HINT.contains("gts switch <track>"));
        assert!(MISSING_RENAME_HINT.contains("gts rename <from> <to>"));
        assert!(MISSING_REMOVE_HINT.contains("gts remove <track> --yes"));
        assert!(MISSING_REMOVE_YES_HINT.contains("gts remove <track> --yes"));
        assert!(MISSING_REMOVE_YES_HINT.replace("<track>", "feature").contains("feature"));
        assert!(crate::menu::MISSING_BARE_COMMAND_HINT.contains("gts status"));
    }
}
