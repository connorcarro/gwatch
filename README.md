# gwatch

Realtime Git working-tree diff TUI.

`gwatch` is for watching **uncommitted working-tree changes** in a Git repo. It is most useful when another terminal, editor, or AI agent is actively editing files and you want a live terminal view of what changed.

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
- right pane: inline diff for the selected file
- header: watched repo path and refresh status
- footer: keyboard controls

If the repo has no uncommitted changes, the file list will say `No changes`. To test it quickly:

```powershell
Add-Content .\README.md "`nTesting gwatch"
gwatch
```

## Controls

- `j`/`Down`: next file
- `k`/`Up`: previous file
- `Enter`: select file
- `p`: pin or unpin the selected file
- `r`: refresh
- `q`/`Esc`: quit

`gwatch` is read-only. It watches filesystem changes, debounces refreshes, and shells out to Git for status and diffs.

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
