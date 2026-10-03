mod cli;
pub mod completion;
pub mod create;
mod exit;
pub mod ignore;
pub mod init;
mod interactive;
mod menu;
mod prompts;
pub mod remove;
pub mod rename;
pub mod status;
pub mod storage;
pub mod switch;

pub use cli::run;
pub use completion::{
    list_completion_tracks, render_bash_completion_script, render_completion_script,
    render_zsh_completion_script, run_completion, CompletionInput,
};
pub use create::{create_track, CreateTrackResult};
pub use exit::{exit_failure, handle_prompt_cancel, map_prompt_result, AppError};
pub use ignore::{
    append_exclude_pattern, open_exclude_in_editor, print_exclude, resolve_editor_command,
    set_exclude, AppendExcludeResult, OpenExcludeResult, PrintExcludeResult, SetExcludeResult,
};
pub use init::{
    assert_not_registered, detect_git_shape, init_workspace, move_git_dir, GitShape,
    InitWorkspaceResult, MoveGitDirOptions, MoveMode,
};
pub use interactive::{is_interactive, InteractiveOptions};
pub use menu::{
    build_action_menu_options, run_bare_command, try_find_registered_workspace, BareCommandInput,
    MenuAction, MISSING_BARE_COMMAND_HINT,
};
pub use prompts::{
    create_scripted_prompts, get_prompts, set_prompts_for_tests, ConfirmOpts, InputOpts,
    PromptsApi, SelectItem, SelectOpts,
};
pub use remove::{remove_track, RemoveTrackResult};
pub use rename::{rename_track, RenameTrackResult};
pub use status::{format_status_human, get_workspace_status, WorkspaceStatus};
pub use switch::{
    is_worktree_dirty, prompt_switch_target, stash_worktree_changes, switch_target_choices,
    switch_track, DirtyChoice, SwitchTrackInput, SwitchTrackResult,
};
