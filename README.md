# gts

Multiple independent Git histories in one workspace. Each history is a track: a full Git directory stored outside the worktree. The workspace `.git` entry is an absolute symlink to the active track. Switching tracks replaces that symlink. Files in the worktree stay put, so the same edits become a different diff under the next history.

This repository is the command-line tool. The desktop app that calls it is [gts-ui](https://github.com/hzw1199/gts-ui).

`gts` does not add worktrees, sparse checkout, or extra remotes. Shared ignore rules stay in the workspace `.gitignore`. Per-track ignores live in that track’s `info/exclude`.

## Requirements

- `git` on `PATH`

The release file is one executable. Running it does not need Node.js or a Rust toolchain. `git` stays an external program and is not inside the binary.

## Install

Download the executable for your OS and CPU from the [latest release](https://github.com/hzw1199/gts/releases/latest). If the filename has an OS suffix, rename it to `gts`.

On macOS the binary is not notarized. A file downloaded in a browser is quarantined, and Gatekeeper blocks the first run. In the directory that contains the file:

```sh
chmod +x gts
xattr -d com.apple.quarantine gts
mkdir -p "$HOME/.local/bin"
mv gts "$HOME/.local/bin/gts"
```

`$HOME/.local/bin` must be on your `PATH`. If `xattr` reports that the attribute is missing, the file was not quarantined. If the shell then prints `Killed: 9`, ad-hoc sign the file and run it again:

```sh
codesign --force --sign - "$HOME/.local/bin/gts"
```

On Linux, make the file executable and move it onto your `PATH`:

```sh
chmod +x gts
mkdir -p "$HOME/.local/bin"
mv gts "$HOME/.local/bin/gts"
```

## Storage

Data lives in `~/.gts/` unless `GTS_HOME` points somewhere else. An existing `~/.gts/` directory uses this layout and works with this binary.

```text
~/.gts/
  index.json
  storage/<id>/<track>/.git/
```

`index.json` maps a workspace id to its absolute path and the cached active track name. The id is a UUID created by `gts init`. The track list is the set of directories under `storage/<id>/`, not a second copy inside the index.

The `.git` symlink is the source of truth for the active track. If it disagrees with `active` in the index, `gts` rewrites the index to match the link. A broken link, or a link that points outside `storage/<id>/`, is an error.

Commands start at the current directory and walk upward to a registered workspace root. `gts init` registers the current directory only.

## Track names

A track name cannot be empty, `.`, `..`, or contain `/` or `\`. An existing name is rejected. On a case-insensitive volume, a name that differs only by case is the same track.

## Quick start

```sh
cd my-project
gts init main
gts create feature --clone
gts switch feature
gts status
```

`gts init main` registers this directory. An existing `.git` directory is moved into storage and linked back. A directory with no `.git` gets a new repository created in storage, then linked. A `.git` file — a linked worktree, a submodule, or any other gitfile — is rejected, as is a symlink that `gts` does not already manage. If init fails partway through, it rolls back. Moving `.git` across filesystems copies the directory and then removes the original.

## Commands

Run `gts` with no arguments in a terminal to pick an action. Outside a TTY, when `CI` is set, or with `--no-interactive`, a missing command or a missing required argument fails and prints a command you can copy. Cancelling a prompt (Esc or Ctrl+C) exits 0. Any other failure exits non-zero.

| Command | What it does |
| --- | --- |
| `gts init [name]` | Register the current directory. The default name in a prompt is `main`. |
| `gts status` | Print the active track, the `.git` link target, and every track. The active track is marked `ACTIVE`. |
| `gts status --json` | Same data as `{ id, path, active, link, tracks }`. `link` is the resolved symlink target. `tracks` is sorted. |
| `gts create <track>` | Create an empty track with `git init` inside storage. The worktree is left unchanged. |
| `gts create <track> --clone` | Copy the current track’s Git directory into the new track. Fails if that directory contains `objects/info/alternates`. |
| `gts switch <track>` | Point `.git` at `<track>`. `gts to <track>` is the same command. Switching to the current track succeeds and changes nothing. |
| `gts rename <from> <to>` | Rename a track directory. Renaming the active track updates the symlink and the index. |
| `gts remove <track>` | Delete that track’s Git directory, including `info/exclude`. The active track cannot be removed. |
| `gts ignore [pattern]` | With a pattern, append one line to the active track’s `info/exclude`. With no pattern, open that file in `$VISUAL`, or `$EDITOR` if `VISUAL` is unset. |
| `gts ignore --track <name> --print` | Print that track’s `info/exclude`. |
| `gts ignore --track <name> --set` | Replace that file with stdin. |
| `gts completion bash` | Print a bash completion script. |
| `gts completion zsh` | Print a zsh completion script. |
| `gts completion --tracks` | Print track names for the current workspace, one per line. |

In a terminal, omitting the track name on `gts switch` or `gts to` shows the current track in a note, labeled `current`, then asks you to pick one of the other tracks. The current track is not in that list. If no other track exists, the command fails and the symlink stays. `gts create` with no name asks for one, and after a successful create asks whether to switch. `gts remove` asks for confirmation. The prompt states that the track’s Git history and `info/exclude` will be deleted. Non-interactive remove requires `--yes`.

`--print` and `--set` each require `--track`, and they cannot be combined with each other or with a pattern argument.

## Dirty worktree

Only a switch checks the worktree, including a switch immediately after create. `gts` runs `git status --porcelain`. When that prints nothing, the symlink is replaced.

When it prints something, a terminal stops and asks:

- **Stash changes before switching** runs `git stash push -u` on the current track. The stash message includes that track’s name. The stash stays with the source history. If stash fails, the symlink stays where it is.
- **Keep changes and carry them to the next track** switches with the edits still in the worktree.
- **Abort** leaves the symlink unchanged and exits 0.

`--stash` selects the first choice. `--force` selects the second. They cannot be used together. Outside a terminal, a dirty worktree with neither flag fails and does not switch.

A switch never runs `reset`, `checkout`, or `clean`.

Create with `--switch` uses the same check. Abort after the track already exists leaves the new track in storage and keeps the previous symlink. Without `--switch`, a non-interactive create does not switch.

## Shell completion

bash:

```sh
eval "$(gts completion bash)"
```

zsh:

```sh
eval "$(gts completion zsh)"
```

Completion fills track names for `switch`, `to`, `remove`, and the first argument of `rename`.

## Develop

Rust stable, edition 2024 (Rust 1.85 or newer).

```sh
git clone https://github.com/hzw1199/gts.git
cd gts
cargo test
cargo build --release
```

`cargo test` runs the unit tests and the functional tests. Functional tests launch the debug binary against a temporary directory. They do not use your `~/.gts/`.

`cargo build --release` writes a stripped `target/release/gts`. The release profile uses size optimization, link-time optimization, and `panic = "abort"`. `target/` is gitignored.

[gts-ui](https://github.com/hzw1199/gts-ui), when run from source, starts this file by absolute path and, on macOS, copies it into the app. Clone the two repositories next to each other so that build finds `../gts/target/release/gts`:

```text
parent/
  gts/
  gts-ui/
```

## Related

- [gts-ui](https://github.com/hzw1199/gts-ui) — desktop app. Install the macOS DMG from its GitHub releases.
- [gts-node](https://github.com/hzw1199/gts-node) — Node.js implementation. It is not published.
