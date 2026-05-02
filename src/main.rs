use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event as CEvent, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

const DEBOUNCE: Duration = Duration::from_millis(180);
const POLL_RATE: Duration = Duration::from_millis(50);

#[derive(Parser, Debug)]
#[command(name = "gwatch", version, about = "Realtime Git working-tree diff TUI")]
struct Cli {
    /// Directory inside the Git repo to watch.
    #[arg(long, value_name = "PATH")]
    repo: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Other,
}

impl FileStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "?",
            Self::Other => "!",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Added | Self::Untracked => Color::Green,
            Self::Modified => Color::Yellow,
            Self::Deleted => Color::Red,
            Self::Renamed => Color::Cyan,
            Self::Other => Color::Magenta,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangedFile {
    path: PathBuf,
    display_path: String,
    status: FileStatus,
    added: Option<u32>,
    deleted: Option<u32>,
}

#[derive(Debug, Clone)]
struct DiffLine {
    kind: DiffKind,
    text: String,
}

#[derive(Debug, Clone, Copy)]
enum DiffKind {
    Header,
    Hunk,
    Added,
    Deleted,
    Context,
}

struct App {
    repo: PathBuf,
    files: Vec<ChangedFile>,
    selected: usize,
    pinned: Option<PathBuf>,
    diff: Vec<DiffLine>,
    status: String,
    last_refresh: Instant,
}

impl App {
    fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            files: Vec::new(),
            selected: 0,
            pinned: None,
            diff: Vec::new(),
            status: "Starting".to_string(),
            last_refresh: Instant::now(),
        }
    }

    fn refresh(&mut self) -> Result<()> {
        let previous_selection = self.active_path().cloned();
        self.files = git_changed_files(&self.repo)?;
        self.reselect(previous_selection.as_ref());
        self.diff = self.load_active_diff()?;
        self.status = "Ready".to_string();
        self.last_refresh = Instant::now();
        Ok(())
    }

    fn reselect(&mut self, previous: Option<&PathBuf>) {
        if self.files.is_empty() {
            self.selected = 0;
            return;
        }

        if let Some(path) = previous {
            if let Some(index) = self.files.iter().position(|file| &file.path == path) {
                self.selected = index;
                return;
            }
        }

        self.selected = self.selected.min(self.files.len().saturating_sub(1));
    }

    fn active_path(&self) -> Option<&PathBuf> {
        if let Some(pinned) = &self.pinned {
            Some(pinned)
        } else {
            self.files.get(self.selected).map(|file| &file.path)
        }
    }

    fn active_status(&self) -> Option<FileStatus> {
        let path = self.active_path()?;
        self.files
            .iter()
            .find(|file| &file.path == path)
            .map(|file| file.status)
    }

    fn load_active_diff(&self) -> Result<Vec<DiffLine>> {
        let Some(path) = self.active_path() else {
            return Ok(vec![DiffLine::context("No working-tree changes.")]);
        };

        match self.active_status() {
            Some(FileStatus::Untracked) => git_untracked_preview(&self.repo, path),
            Some(_) => git_diff(&self.repo, path),
            None => Ok(vec![DiffLine::context(format!(
                "{} has no current diff.",
                display_path(path)
            ))]),
        }
    }

    fn next(&mut self) -> Result<()> {
        if !self.files.is_empty() {
            self.selected = (self.selected + 1).min(self.files.len() - 1);
            if self.pinned.is_none() {
                self.diff = self.load_active_diff()?;
            }
        }
        Ok(())
    }

    fn previous(&mut self) -> Result<()> {
        if !self.files.is_empty() {
            self.selected = self.selected.saturating_sub(1);
            if self.pinned.is_none() {
                self.diff = self.load_active_diff()?;
            }
        }
        Ok(())
    }

    fn select(&mut self) -> Result<()> {
        if let Some(file) = self.files.get(self.selected) {
            self.pinned = None;
            self.diff = git_diff_for_status(&self.repo, &file.path, file.status)?;
        }
        Ok(())
    }

    fn toggle_pin(&mut self) -> Result<()> {
        if let Some(file) = self.files.get(self.selected) {
            if self.pinned.as_ref() == Some(&file.path) {
                self.pinned = None;
            } else {
                self.pinned = Some(file.path.clone());
            }
            self.diff = self.load_active_diff()?;
        }
        Ok(())
    }
}

impl DiffLine {
    fn new(kind: DiffKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }

    fn context(text: impl Into<String>) -> Self {
        Self::new(DiffKind::Context, text)
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let start_dir = cli.repo.unwrap_or(std::env::current_dir()?);
    let repo = discover_repo(&start_dir)?;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, repo);
    restore_terminal(&mut terminal)?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, repo: PathBuf) -> Result<()> {
    let (watch_tx, watch_rx) = mpsc::channel();
    let mut _watcher = setup_watcher(&repo, watch_tx)?;
    let mut app = App::new(repo);
    app.refresh()?;

    let mut pending_refresh: Option<Instant> = None;

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        drain_watch_events(&watch_rx, &mut pending_refresh);

        if pending_refresh.is_some_and(|at| Instant::now() >= at) {
            if let Err(err) = app.refresh() {
                app.status = format!("Refresh failed: {err}");
            }
            pending_refresh = None;
        }

        if event::poll(POLL_RATE)? {
            if let CEvent::Key(key) = event::read()? {
                match handle_key(&mut app, key) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(err) => app.status = err.to_string(),
                }
            }
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
        KeyCode::Down | KeyCode::Char('j') => app.next()?,
        KeyCode::Up | KeyCode::Char('k') => app.previous()?,
        KeyCode::Enter => app.select()?,
        KeyCode::Char('p') => app.toggle_pin()?,
        KeyCode::Char('r') => app.refresh()?,
        _ => {}
    }
    Ok(false)
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("failed to initialize terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    terminal::disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
    terminal.show_cursor()?;
    Ok(())
}

fn setup_watcher(repo: &Path, tx: mpsc::Sender<()>) -> Result<RecommendedWatcher> {
    let repo = repo.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            if event.paths.iter().any(|path| !is_git_internal(&repo, path)) {
                let _ = tx.send(());
            }
        }
    })
    .context("failed to create filesystem watcher")?;

    watcher
        .watch(&repo, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", repo.display()))?;
    Ok(watcher)
}

fn is_git_internal(repo: &Path, path: &Path) -> bool {
    path.strip_prefix(repo)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str() == ".git")
}

fn drain_watch_events(rx: &Receiver<()>, pending_refresh: &mut Option<Instant>) {
    while rx.try_recv().is_ok() {
        *pending_refresh = Some(Instant::now() + DEBOUNCE);
    }
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(root[1]);

    draw_header(frame, root[0], app);
    draw_files(frame, body[0], app);
    draw_diff(frame, body[1], app);
    draw_footer(frame, root[2]);
}

fn draw_header(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let pinned = app
        .pinned
        .as_ref()
        .map(|path| format!(" | pinned {}", display_path(path)))
        .unwrap_or_default();
    let header = vec![
        Line::from(vec![
            Span::styled(
                "gwatch",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " | {} | {} files{}",
                app.repo.display(),
                app.files.len(),
                pinned
            )),
        ]),
        Line::from(format!(
            "{} | refreshed {:?} ago",
            app.status,
            app.last_refresh.elapsed()
        )),
    ];

    frame.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_files(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let items = if app.files.is_empty() {
        vec![ListItem::new("No changes")]
    } else {
        app.files
            .iter()
            .map(|file| {
                let pin = if app.pinned.as_ref() == Some(&file.path) {
                    "*"
                } else {
                    " "
                };
                let counts = match (file.added, file.deleted) {
                    (Some(a), Some(d)) => format!(" +{a} -{d}"),
                    _ => String::new(),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}{} ", pin, file.status.label()),
                        Style::default()
                            .fg(file.status.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(file.display_path.clone()),
                    Span::styled(counts, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect()
    };

    let mut state = ListState::default();
    if !app.files.is_empty() {
        state.select(Some(app.selected));
    }

    let list = List::new(items)
        .block(Block::default().title("Files").borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_diff(frame: &mut Frame<'_>, area: ratatui::layout::Rect, app: &App) {
    let title = app
        .active_path()
        .map(|path| format!("Diff: {}", display_path(path)))
        .unwrap_or_else(|| "Diff".to_string());
    let lines: Vec<Line<'_>> = app
        .diff
        .iter()
        .map(|line| {
            let style = match line.kind {
                DiffKind::Header => Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                DiffKind::Hunk => Style::default().fg(Color::Magenta),
                DiffKind::Added => Style::default().fg(Color::Green),
                DiffKind::Deleted => Style::default().fg(Color::Red),
                DiffKind::Context => Style::default(),
            };
            Line::from(Span::styled(line.text.clone(), style))
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    frame.render_widget(
        Paragraph::new("j/k or arrows move | enter select | p pin | r refresh | q quit"),
        area,
    );
}

fn discover_repo(path: &Path) -> Result<PathBuf> {
    let output = git(path, ["rev-parse", "--show-toplevel"])?;
    let root = String::from_utf8(output.stdout)?.trim().to_string();
    if root.is_empty() {
        bail!("{} is not inside a Git worktree", path.display());
    }
    Ok(PathBuf::from(root))
}

fn git_changed_files(repo: &Path) -> Result<Vec<ChangedFile>> {
    let status_output = git(repo, ["status", "--porcelain=v1", "-z"])?;
    let entries = parse_status(&status_output.stdout);
    let stats = git_numstat(repo).unwrap_or_default();

    let mut files: Vec<_> = entries
        .into_iter()
        .map(|(path, status)| {
            let (added, deleted) = stats.get(&path).copied().unwrap_or((None, None));
            ChangedFile {
                display_path: display_path(&path),
                path,
                status,
                added,
                deleted,
            }
        })
        .collect();

    files.sort_by(|a, b| a.display_path.cmp(&b.display_path));
    Ok(files)
}

fn parse_status(bytes: &[u8]) -> Vec<(PathBuf, FileStatus)> {
    let mut result = Vec::new();
    let mut parts = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());

    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let x = entry[0] as char;
        let y = entry[1] as char;
        let status = status_from_xy(x, y);
        let path = path_from_git_bytes(&entry[3..]);

        if matches!(status, FileStatus::Renamed) {
            let _old_path = parts.next();
        }

        result.push((path, status));
    }

    result
}

fn status_from_xy(x: char, y: char) -> FileStatus {
    if x == '?' && y == '?' {
        return FileStatus::Untracked;
    }

    match [x, y] {
        chars if chars.contains(&'R') => FileStatus::Renamed,
        chars if chars.contains(&'A') => FileStatus::Added,
        chars if chars.contains(&'D') => FileStatus::Deleted,
        chars if chars.contains(&'M') => FileStatus::Modified,
        _ => FileStatus::Other,
    }
}

fn git_numstat(repo: &Path) -> Result<BTreeMap<PathBuf, (Option<u32>, Option<u32>)>> {
    let output = git(repo, ["diff", "--numstat", "-z"])?;
    let mut stats = BTreeMap::new();
    let mut parts = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());

    while let Some(entry) = parts.next() {
        let fields: Vec<_> = entry.split(|byte| *byte == b'\t').collect();
        if fields.len() >= 3 {
            let added = parse_count(fields[0]);
            let deleted = parse_count(fields[1]);
            stats.insert(path_from_git_bytes(fields[2]), (added, deleted));
        } else if fields.len() == 2 {
            // Rename entries can be emitted as counts followed by old/new path records.
            let added = parse_count(fields[0]);
            let deleted = parse_count(fields[1]);
            let _old = parts.next();
            if let Some(new_path) = parts.next() {
                stats.insert(path_from_git_bytes(new_path), (added, deleted));
            }
        }
    }

    Ok(stats)
}

fn parse_count(bytes: &[u8]) -> Option<u32> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn git_diff_for_status(repo: &Path, path: &Path, status: FileStatus) -> Result<Vec<DiffLine>> {
    match status {
        FileStatus::Untracked => git_untracked_preview(repo, path),
        _ => git_diff(repo, path),
    }
}

fn git_diff(repo: &Path, path: &Path) -> Result<Vec<DiffLine>> {
    let output = git_path(repo, ["diff", "--"], path)?;
    let text = String::from_utf8_lossy(&output.stdout);
    if text.is_empty() {
        return Ok(vec![DiffLine::context("No current diff.")]);
    }
    Ok(parse_diff_text(&text))
}

fn git_untracked_preview(repo: &Path, path: &Path) -> Result<Vec<DiffLine>> {
    let full_path = repo.join(path);
    if !full_path.is_file() {
        return Ok(vec![DiffLine::context(
            "Untracked path is not a regular file.",
        )]);
    }

    let content = std::fs::read(&full_path)
        .with_context(|| format!("failed to read {}", full_path.display()))?;
    if content.contains(&0) {
        return Ok(vec![DiffLine::context("Binary untracked file.")]);
    }

    let text = String::from_utf8_lossy(&content);
    let mut lines = vec![
        DiffLine::new(
            DiffKind::Header,
            format!(
                "diff --git a/{} b/{}",
                display_path(path),
                display_path(path)
            ),
        ),
        DiffLine::new(DiffKind::Header, "new file mode 100644"),
        DiffLine::new(DiffKind::Header, "--- /dev/null"),
        DiffLine::new(DiffKind::Header, format!("+++ b/{}", display_path(path))),
    ];
    for line in text.lines() {
        lines.push(DiffLine::new(DiffKind::Added, format!("+{line}")));
    }
    Ok(lines)
}

fn parse_diff_text(text: &str) -> Vec<DiffLine> {
    text.lines()
        .map(|line| {
            let kind = if line.starts_with("@@") {
                DiffKind::Hunk
            } else if line.starts_with("diff --git")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
            {
                DiffKind::Header
            } else if line.starts_with('+') {
                DiffKind::Added
            } else if line.starts_with('-') {
                DiffKind::Deleted
            } else {
                DiffKind::Context
            };
            DiffLine::new(kind, line)
        })
        .collect()
}

fn git<I, S>(repo: &Path, args: I) -> Result<std::process::Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .context("failed to run git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(stderr.trim().to_string()));
    }

    Ok(output)
}

fn git_path<I, S>(repo: &Path, args: I, path: &Path) -> Result<std::process::Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .arg(path)
        .output()
        .context("failed to run git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(stderr.trim().to_string()));
    }

    Ok(output)
}

#[cfg(unix)]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).to_string())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_entries() {
        let parsed = parse_status(b" M src/main.rs\0?? notes.txt\0 D old.txt\0");

        assert_eq!(
            parsed,
            vec![
                (PathBuf::from("src/main.rs"), FileStatus::Modified),
                (PathBuf::from("notes.txt"), FileStatus::Untracked),
                (PathBuf::from("old.txt"), FileStatus::Deleted),
            ]
        );
    }

    #[test]
    fn parses_rename_status_entry() {
        let parsed = parse_status(b"R  new.txt\0old.txt\0");

        assert_eq!(
            parsed,
            vec![(PathBuf::from("new.txt"), FileStatus::Renamed)]
        );
    }

    #[test]
    fn keeps_selection_on_same_path_after_refresh() {
        let mut app = App::new(PathBuf::from("."));
        app.files = vec![
            changed("a.txt", FileStatus::Modified),
            changed("b.txt", FileStatus::Modified),
        ];
        app.selected = 1;
        app.files = vec![
            changed("b.txt", FileStatus::Modified),
            changed("c.txt", FileStatus::Modified),
        ];

        app.reselect(Some(&PathBuf::from("b.txt")));

        assert_eq!(app.selected, 0);
    }

    #[test]
    fn pinned_path_remains_active_when_absent_from_file_list() {
        let mut app = App::new(PathBuf::from("."));
        app.files = vec![changed("a.txt", FileStatus::Modified)];
        app.pinned = Some(PathBuf::from("missing.txt"));

        assert_eq!(app.active_path(), Some(&PathBuf::from("missing.txt")));
        assert_eq!(app.active_status(), None);
    }

    fn changed(path: &str, status: FileStatus) -> ChangedFile {
        ChangedFile {
            path: PathBuf::from(path),
            display_path: path.to_string(),
            status,
            added: None,
            deleted: None,
        }
    }
}
