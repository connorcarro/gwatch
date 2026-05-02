# gwatch

Realtime Git working-tree diff TUI.

`gwatch` is for watching **uncommitted code changes** in a Git repo: staged changes, unstaged changes, and untracked files. It is most useful when another terminal, editor, or AI agent is actively editing files and you want a live terminal view of what changed.

## Usage

If you have not installed the command yet, run this once from the repo root:

```powershell
.\install.ps1
```

Run it from inside a Git repo:

```powershell
gwatch
```

Or point it at a repo from anywhere:

```powershell
gwatch --repo C:\path\to\repo
```

If you are running from source:

```powershell
cargo run -- --repo C:\path\to\repo
```

From this repo, you can also run it before installing with:

```powershell
cargo run --
```

To install the command locally:

```powershell
cargo install --path .
```

## What You Should See

The app opens a full-screen terminal UI:

- left pane: changed files
- right pane: colored inline diff for the selected file
- header: branch, watched repo path, active file, changed-file count, total added/deleted lines, refresh status
- footer: compact command hints

Diff rows include old/new line-number gutters, muted Git metadata, highlighted hunk headers, and colored added/deleted rows. Press `f` to toggle a full-width diff view when the split view feels too cramped. Press `w` to toggle wrapping for long lines.

`gwatch` also keeps a lightweight in-memory cockpit view while it is open:

- recently touched files are marked in the file list
- `s` cycles sort modes: path, status, recent, size
- `/` filters the changed-file list
- `n`/`N` jumps between hunks in the active diff
- `?` opens the help overlay

If the repo has no uncommitted changes, the file list will say `No changes`. Committed history is not shown. To test it quickly:

```powershell
Add-Content .\README.md "`nTesting gwatch"
gwatch
```

For continuous realtime testing, run this in one terminal:

```powershell
.\churn-test-file.ps1
```

Then run `gwatch` in another terminal. The script randomly adds or removes one line from `test.md` every second until you stop it with `Ctrl+C`.

## Controls

- `j`/`Down`: next file
- `k`/`Up`: previous file
- mouse wheel: scroll diff
- `d`/`PageDown`: scroll diff down
- `u`/`PageUp`: scroll diff up
- `g`/`Home`: jump to top of diff
- `G`/`End`: jump to bottom of diff
- `n`: jump to next hunk
- `N`: jump to previous hunk
- `/`: filter changed files
- `s`: cycle file sort mode
- `f`: toggle split view / full-width diff view
- `w`: toggle diff line wrapping
- `?`: show help
- `Enter`: select file
- `p`: pin or unpin the selected file
- `r`: refresh
- `q`/`Esc`: quit

`gwatch` is read-only. It watches filesystem changes, debounces refreshes, and shells out to Git for status and diffs.

## Architecture

The codebase is split by responsibility:

- `cli`: command-line arguments
- `runtime`: terminal lifecycle, input handling, refresh loop
- `watcher`: filesystem watching and `.git` event filtering
- `app`: review cockpit state, sorting, filtering, pinning, hunk navigation
- `git`: Git repository discovery, status parsing, diff loading
- `diff`: unified diff model and parser
- `ui`: ratatui rendering and terminal components

## Troubleshooting

If it looks like nothing happened:

- If PowerShell says `gwatch` is not recognized, install it first with `.\install.ps1` or run it with `cargo run --`.
- Make sure you are running it inside a Git repo, or pass `--repo`.
- Make sure the repo has uncommitted changes.
- If you installed it with Cargo, confirm Cargo's bin directory is on `PATH`.
- On Windows, if the build says `link.exe` failed and mentions Visual Studio Build Tools, Rust is probably seeing Git for Windows' `link.exe` instead of Microsoft's linker. Install the C++ build tools and Windows SDK:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

After that finishes, open a new PowerShell window and run:

```powershell
.\install.ps1
```
