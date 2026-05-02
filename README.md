# gwatch

Realtime Git working-tree diff TUI.

## Usage

```powershell
gwatch
gwatch --repo C:\path\to\repo
```

## Controls

- `j`/`Down`: next file
- `k`/`Up`: previous file
- `Enter`: select file
- `p`: pin or unpin the selected file
- `r`: refresh
- `q`/`Esc`: quit

`gwatch` is read-only. It watches filesystem changes, debounces refreshes, and shells out to Git for status and diffs.
