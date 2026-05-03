# gwatch

[![CI](https://github.com/connorcarro/gwatch/actions/workflows/ci.yml/badge.svg)](https://github.com/connorcarro/gwatch/actions/workflows/ci.yml)

Read-only realtime Git working-tree diff TUI.

`gwatch` helps you monitor **uncommitted code changes** in a Git repository: staged changes, unstaged changes, deleted files, renamed files, and untracked files. It is designed for review-heavy workflows where another terminal, editor, script, or AI coding agent is actively editing files and you want a live terminal view of what changed.

## Why

General Git TUIs are built around performing Git operations. `gwatch` is intentionally narrower: it stays read-only and focuses on fast situational awareness while code is changing.

Useful when you want to:

- supervise an AI agent without repeatedly running `git status` and `git diff`
- keep a live diff open next to an editor or terminal session
- separate changes that existed before you started watching from changes made during the current session
- review untracked files with inline previews

## Usage

Run it from inside a Git repository:

```powershell
gwatch
```

Or point it at a repository from anywhere:

```powershell
gwatch --repo C:\path\to\repo
```

Run from source:

```powershell
cargo run -- --repo C:\path\to\repo
```

Install locally with Cargo:

```powershell
cargo install --path .
```

On Windows, you can also run the included installer from this repository:

```powershell
.\install.ps1
```

## Features

The app opens a full-screen terminal UI with:

- changed-file list with status, added/deleted counts, recent-change markers, and session-change markers
- colored inline diff with old/new line number gutters and syntax highlighting for common languages and config formats
- branch, repository path, active file, total changed files, total added/deleted lines, refresh status, current sort mode, and current scope
- split view and full-width diff view
- filtering, sorting, pinning, hunk navigation, mouse-wheel scrolling, accelerated Ctrl+mouse-wheel scrolling, and line wrapping
- session scope, which hides changes that existed before `gwatch` started and shows only files touched or added during the current watch session
- large-diff handling: diff output is streamed into a disk-backed document, rendering is virtualized to the visible terminal rows, and hunk positions are cached for fast navigation

`gwatch` shells out to Git for status and diffs, watches filesystem events, and debounces refreshes so rapid editor or agent writes do not constantly redraw the UI.

Syntax highlighting is powered by `syntect`, using bundled Sublime Text syntax definitions plus `gwatch` aliases for many modern file types that do not always have exact bundled grammars. Common Rust, C/C++, Go, Java, C#, Swift, JavaScript, TypeScript, JSX/TSX, Python, Ruby, PHP, shell, PowerShell, SQL, HTML, CSS, SCSS, Markdown, JSON, TOML, YAML, Terraform, Dockerfile, Makefile, protobuf, XML, SVG, and config files are highlighted.

## Testing It

If the repository has no uncommitted changes, the file list will say `No working-tree changes`. To test it quickly:

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
- `Ctrl` + mouse wheel: accelerated diff scroll, proportional to diff size
- `d`/`PageDown`: scroll diff down
- `u`/`PageUp`: scroll diff up
- `g`/`Home`: jump to top of diff
- `G`/`End`: jump to bottom of diff
- `n`: jump to next hunk
- `N`: jump to previous hunk
- `/`: filter changed files
- `s`: cycle file sort mode
- `b`: toggle all changes / current-session changes
- `f`: toggle split view / full-width diff view
- `w`: toggle diff line wrapping
- `?`: show help
- `Enter`: select file
- `p`: pin or unpin the selected file
- `r`: refresh
- `q`/`Esc`: quit

`gwatch` is read-only. It watches filesystem changes, debounces refreshes, and shells out to Git for status and diffs.

## Development

Run the full local check before publishing changes:

```powershell
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo package --allow-dirty --offline --no-verify
```

## GitHub Tests

This repository includes a GitHub Actions workflow at `.github/workflows/ci.yml`. It runs automatically on every push and pull request, and you can also start it manually from GitHub with **Actions** -> **CI** -> **Run workflow**.

The workflow has four jobs:

- **Format**: runs `cargo fmt -- --check` on Ubuntu.
- **Test**: runs `cargo clippy --locked --all-targets --all-features -- -D warnings` and `cargo test --locked --all-targets --all-features` on Ubuntu, Windows, and macOS.
- **MSRV**: runs `cargo check --locked --all-targets --all-features` on Rust 1.85, matching the `rust-version` in `Cargo.toml`.
- **Package**: runs `cargo package --locked` to verify the crate can be packaged cleanly.

How to use it on GitHub:

1. Push the repo to GitHub.
2. Open the repository page and click the **Actions** tab.
3. Click the latest **CI** run.
4. A green check means formatting, linting, tests, minimum Rust version, and packaging all passed.
5. If a job fails, open the failed job, expand the failed command, fix the reported error locally, then push again.
6. To re-run without pushing, open the failed workflow run and use **Re-run jobs**.

The CI badge at the top of this README will show the current status after the workflow exists on GitHub.

## Architecture

The codebase is split by responsibility:

- `cli`: command-line arguments
- `runtime`: terminal lifecycle, input handling, refresh loop
- `watcher`: filesystem watching and `.git` event filtering
- `app`: review cockpit state, sorting, filtering, pinning, hunk navigation
- `git`: Git repository discovery, status parsing, diff loading
- `diff`: unified diff model and parser
- `syntax`: syntax highlighting and extension aliases
- `ui`: ratatui rendering and terminal components

## License

MIT

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
