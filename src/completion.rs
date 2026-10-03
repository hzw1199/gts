use std::io::{self, Write};
use std::path::Path;

use crate::exit::{exit_failure, AppError};
use crate::storage::{
    find_workspace_root_from, list_tracks, resolve_active_track, HomeOptions, StorageError,
};

pub fn list_completion_tracks(
    cwd: &Path,
    opts: &HomeOptions,
) -> Result<Vec<String>, StorageError> {
    let found = find_workspace_root_from(cwd, opts)?;
    resolve_active_track(&found.root, opts)?;
    list_tracks(&found.id, opts)
}

pub fn render_bash_completion_script() -> String {
    r#"# gts bash completion — eval "$(gts completion bash)"
_gts() {
  local cur prev cmd i
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  prev="${COMP_WORDS[COMP_CWORD-1]}"
  cmd=""
  for ((i=1; i < COMP_CWORD; i++)); do
    case "${COMP_WORDS[i]}" in
      -*) ;;
      *) cmd="${COMP_WORDS[i]}"; break ;;
    esac
  done
  local bin="${COMP_WORDS[0]}"
  case "$cmd" in
    switch|to|remove)
      local tracks
      tracks="$("$bin" completion --tracks 2>/dev/null)" || return
      COMPREPLY=( $(compgen -W "$tracks" -- "$cur") )
      ;;
    rename)
      local positional=0
      for ((i=1; i < COMP_CWORD; i++)); do
        case "${COMP_WORDS[i]}" in
          -*) ;;
          rename) ;;
          *) positional=$((positional + 1)) ;;
        esac
      done
      if [[ $positional -eq 0 ]]; then
        local tracks
        tracks="$("$bin" completion --tracks 2>/dev/null)" || return
        COMPREPLY=( $(compgen -W "$tracks" -- "$cur") )
      fi
      ;;
  esac
}
complete -F _gts gts
"#
    .to_string()
}

pub fn render_zsh_completion_script() -> String {
    r#"# gts zsh completion — eval "$(gts completion zsh)"
_gts() {
  local cmd=${words[2]}
  local bin=${words[1]}
  case $cmd in
    switch|to|remove)
      if (( CURRENT == 3 )); then
        local -a tracks
        tracks=(${(f)"$("$bin" completion --tracks 2>/dev/null)"})
        _describe -t tracks 'track' tracks
      fi
      ;;
    rename)
      if (( CURRENT == 3 )); then
        local -a tracks
        tracks=(${(f)"$("$bin" completion --tracks 2>/dev/null)"})
        _describe -t tracks 'track' tracks
      fi
      ;;
  esac
}
compdef _gts gts
"#
    .to_string()
}

pub fn render_completion_script(shell: &str) -> Result<String, AppError> {
    let normalized = shell.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "bash" => Ok(render_bash_completion_script()),
        "zsh" => Ok(render_zsh_completion_script()),
        _ => Err(exit_failure(format!(
            "Unsupported shell: {shell}. Supported shells: bash, zsh. Run: gts completion bash"
        ))),
    }
}

pub fn run_completion(input: CompletionInput<'_>) -> Result<(), AppError> {
    let opts = HomeOptions::default();

    if input.tracks {
        let tracks = list_completion_tracks(input.cwd, &opts)
            .map_err(|e| exit_failure(e.to_string()))?;
        if !tracks.is_empty() {
            println!("{}", tracks.join("\n"));
        }
        return Ok(());
    }

    let shell = input.shell.map(str::trim).filter(|s| !s.is_empty());
    let Some(shell) = shell else {
        return Err(exit_failure(
            "Missing required argument. Run: gts completion bash|zsh",
        ));
    };

    let script = render_completion_script(shell)?;
    io::stdout()
        .write_all(script.as_bytes())
        .map_err(|e| exit_failure(e.to_string()))?;
    Ok(())
}

pub struct CompletionInput<'a> {
    pub shell: Option<&'a str>,
    pub tracks: bool,
    pub cwd: &'a Path,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        register_workspace, replace_git_symlink, track_git_dir, HomeOptions,
    };
    use std::fs;
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

    fn setup_registered() -> (PathBuf, PathBuf, String) {
        let home = temp_dir("gts-comp-home");
        let worktree = temp_dir("gts-comp-wt");
        let opts = HomeOptions::with_home(&home);
        let registered = register_workspace(&worktree, "main", &opts, None).unwrap();
        let main_git = track_git_dir(&home, &registered.id, "main");
        let feature_git = track_git_dir(&home, &registered.id, "feature");
        fs::create_dir_all(&main_git).unwrap();
        fs::create_dir_all(&feature_git).unwrap();
        replace_git_symlink(&worktree, &main_git).unwrap();
        (home, worktree, registered.id)
    }

    #[test]
    fn lists_tracks_matching_storage_directories() {
        let (home, worktree, _) = setup_registered();
        let opts = HomeOptions::with_home(&home);
        let tracks = list_completion_tracks(&worktree, &opts).unwrap();
        let set: std::collections::HashSet<_> = tracks.into_iter().collect();
        assert_eq!(
            set,
            ["main".into(), "feature".into()].into_iter().collect()
        );
    }

    #[test]
    fn fails_without_guessing_when_unregistered() {
        let home = temp_dir("gts-comp-home");
        let worktree = temp_dir("gts-comp-wt");
        let opts = HomeOptions::with_home(&home);
        let err = list_completion_tracks(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("No registered workspace found"));
    }

    #[test]
    fn fails_on_broken_git_symlink() {
        let (home, worktree, _) = setup_registered();
        let git_path = worktree.join(".git");
        fs::remove_file(&git_path).unwrap();
        symlink(worktree.join("missing-target"), &git_path).unwrap();
        let opts = HomeOptions::with_home(&home);
        let err = list_completion_tracks(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("Broken .git symlink"));
    }

    #[test]
    fn fails_when_link_points_outside_storage() {
        let (home, worktree, _) = setup_registered();
        let outside = temp_dir("gts-comp-out");
        let outside_git = outside.join(".git");
        fs::create_dir_all(&outside_git).unwrap();
        replace_git_symlink(&worktree, &outside_git).unwrap();
        let opts = HomeOptions::with_home(&home);
        let err = list_completion_tracks(&worktree, &opts).unwrap_err();
        assert!(err.to_string().contains("outside workspace storage"));
    }

    #[test]
    fn bash_script_completes_switch_to_remove_rename_not_create() {
        let script = render_bash_completion_script();
        assert!(script.contains("switch|to|remove"));
        assert!(script.contains("rename"));
        assert!(script.contains("completion --tracks"));
        assert!(!script.contains("create)"));
    }

    #[test]
    fn zsh_script_completes_switch_to_remove_rename_not_create() {
        let script = render_zsh_completion_script();
        assert!(script.contains("switch|to|remove"));
        assert!(script.contains("rename"));
        assert!(script.contains("completion --tracks"));
        assert!(!script.contains("create)"));
    }

    #[test]
    fn rejects_unknown_shell() {
        let err = render_completion_script("fish").unwrap_err();
        match err {
            AppError::Failure(msg) => assert!(msg.contains("Unsupported shell")),
            other => panic!("expected Failure, got {other:?}"),
        }
    }
}
