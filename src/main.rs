use std::{
    collections::{BTreeMap, HashMap},
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
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

const DEBOUNCE: Duration = Duration::from_millis(180);
const POLL_RATE: Duration = Duration::from_millis(50);
const RECENT_WINDOW: Duration = Duration::from_secs(8);

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
    old_line: Option<u32>,
    new_line: Option<u32>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Split,
    DiffOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Path,
    Status,
    Recent,
    Size,
}

impl SortMode {
    fn next(self) -> Self {
        match self {
            Self::Path => Self::Status,
            Self::Status => Self::Recent,
            Self::Recent => Self::Size,
            Self::Size => Self::Path,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Status => "status",
            Self::Recent => "recent",
            Self::Size => "size",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Filter,
    Help,
}

struct App {
    repo: PathBuf,
    branch: String,
    all_files: Vec<ChangedFile>,
    files: Vec<ChangedFile>,
    selected: usize,
    pinned: Option<PathBuf>,
    diff: Vec<DiffLine>,
    diff_scroll: u16,
    wrap_diff: bool,
    view_mode: ViewMode,
    sort_mode: SortMode,
    input_mode: InputMode,
    filter: String,
    recent: HashMap<PathBuf, Instant>,
    status: String,
    last_refresh: Instant,
}

impl App {
    fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            branch: String::new(),
            all_files: Vec::new(),
            files: Vec::new(),
            selected: 0,
            pinned: None,
            diff: Vec::new(),
            diff_scroll: 0,
            wrap_diff: false,
            view_mode: ViewMode::Split,
            sort_mode: SortMode::Path,
            input_mode: InputMode::Normal,
            filter: String::new(),
            recent: HashMap::new(),
            status: "Starting".to_string(),
            last_refresh: Instant::now(),
        }
    }

    fn refresh(&mut self) -> Result<()> {
        let previous_selection = self.active_path().cloned();
        self.all_files = git_changed_files(&self.repo)?;
        self.rebuild_files();
        self.reselect(previous_selection.as_ref());
        self.diff = self.load_active_diff()?;
        self.clamp_diff_scroll();
        self.status = "Ready".to_string();
        self.last_refresh = Instant::now();
        Ok(())
    }

    fn rebuild_files(&mut self) {
        let filter = self.filter.to_ascii_lowercase();
        self.files = self
            .all_files
            .iter()
            .filter(|file| {
                filter.is_empty() || file.display_path.to_ascii_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        sort_files(&mut self.files, self.sort_mode, &self.recent);
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
        self.all_files
            .iter()
            .find(|file| &file.path == path)
            .map(|file| file.status)
    }

    fn active_file(&self) -> Option<&ChangedFile> {
        let path = self.active_path()?;
        self.all_files.iter().find(|file| &file.path == path)
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
                self.diff_scroll = 0;
            }
        }
        Ok(())
    }

    fn previous(&mut self) -> Result<()> {
        if !self.files.is_empty() {
            self.selected = self.selected.saturating_sub(1);
            if self.pinned.is_none() {
                self.diff = self.load_active_diff()?;
                self.diff_scroll = 0;
            }
        }
        Ok(())
    }

    fn select(&mut self) -> Result<()> {
        if let Some(file) = self.files.get(self.selected) {
            self.pinned = None;
            self.diff = git_diff_for_status(&self.repo, &file.path, file.status)?;
            self.diff_scroll = 0;
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
            self.diff_scroll = 0;
        }
        Ok(())
    }

    fn scroll_diff_down(&mut self, amount: u16) {
        self.diff_scroll = self.diff_scroll.saturating_add(amount);
        self.clamp_diff_scroll();
    }

    fn scroll_diff_up(&mut self, amount: u16) {
        self.diff_scroll = self.diff_scroll.saturating_sub(amount);
    }

    fn scroll_diff_top(&mut self) {
        self.diff_scroll = 0;
    }

    fn scroll_diff_bottom(&mut self) {
        self.diff_scroll = self.diff.len().saturating_sub(1).min(u16::MAX as usize) as u16;
    }

    fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Split => ViewMode::DiffOnly,
            ViewMode::DiffOnly => ViewMode::Split,
        };
    }

    fn toggle_wrap(&mut self) {
        self.wrap_diff = !self.wrap_diff;
    }

    fn cycle_sort(&mut self) -> Result<()> {
        let previous_selection = self.active_path().cloned();
        self.sort_mode = self.sort_mode.next();
        self.rebuild_files();
        self.reselect(previous_selection.as_ref());
        if self.pinned.is_none() {
            self.diff = self.load_active_diff()?;
            self.diff_scroll = 0;
        }
        Ok(())
    }

    fn enter_filter(&mut self) {
        self.input_mode = InputMode::Filter;
    }

    fn enter_help(&mut self) {
        self.input_mode = InputMode::Help;
    }

    fn clear_overlay(&mut self) {
        self.input_mode = InputMode::Normal;
    }

    fn update_filter(&mut self, next_filter: String) -> Result<()> {
        let previous_selection = self.active_path().cloned();
        self.filter = next_filter;
        self.rebuild_files();
        self.reselect(previous_selection.as_ref());
        if self.pinned.is_none() {
            self.diff = self.load_active_diff()?;
            self.diff_scroll = 0;
        }
        Ok(())
    }

    fn note_changed_paths(&mut self, paths: Vec<PathBuf>) {
        let now = Instant::now();
        for path in paths {
            if let Some(relative) = relative_repo_path(&self.repo, &path) {
                self.recent.insert(relative, now);
            }
        }
    }

    fn is_recent(&self, path: &Path) -> bool {
        self.recent
            .get(path)
            .is_some_and(|changed| changed.elapsed() <= RECENT_WINDOW)
    }

    fn hunk_positions(&self) -> Vec<usize> {
        self.diff
            .iter()
            .enumerate()
            .filter_map(|(index, line)| matches!(line.kind, DiffKind::Hunk).then_some(index))
            .collect()
    }

    fn next_hunk(&mut self) {
        let positions = self.hunk_positions();
        let Some(next) = positions
            .iter()
            .copied()
            .find(|position| *position > self.diff_scroll as usize)
            .or_else(|| positions.first().copied())
        else {
            return;
        };
        self.diff_scroll = next.min(u16::MAX as usize) as u16;
    }

    fn previous_hunk(&mut self) {
        let positions = self.hunk_positions();
        let Some(previous) = positions
            .iter()
            .rev()
            .copied()
            .find(|position| *position < self.diff_scroll as usize)
            .or_else(|| positions.last().copied())
        else {
            return;
        };
        self.diff_scroll = previous.min(u16::MAX as usize) as u16;
    }

    fn clamp_diff_scroll(&mut self) {
        let max = self.diff.len().saturating_sub(1).min(u16::MAX as usize) as u16;
        self.diff_scroll = self.diff_scroll.min(max);
    }

    fn totals(&self) -> (u32, u32) {
        self.all_files
            .iter()
            .fold((0, 0), |(added, deleted), file| {
                (
                    added + file.added.unwrap_or_default(),
                    deleted + file.deleted.unwrap_or_default(),
                )
            })
    }
}

impl DiffLine {
    fn new(kind: DiffKind, text: impl Into<String>) -> Self {
        Self::with_numbers(kind, None, None, text)
    }

    fn with_numbers(
        kind: DiffKind,
        old_line: Option<u32>,
        new_line: Option<u32>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            old_line,
            new_line,
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
    app.branch = git_branch(&app.repo).unwrap_or_else(|_| "unknown".to_string());
    app.refresh()?;

    let mut pending_refresh: Option<Instant> = None;

    loop {
        terminal.draw(|frame| draw(frame, &app))?;

        drain_watch_events(&watch_rx, &mut pending_refresh, &mut app);

        if pending_refresh.is_some_and(|at| Instant::now() >= at) {
            if let Err(err) = app.refresh() {
                app.status = format!("Refresh failed: {err}");
            }
            pending_refresh = None;
        }

        if event::poll(POLL_RATE)? {
            match event::read()? {
                CEvent::Key(key) => match handle_key(&mut app, key) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(err) => app.status = err.to_string(),
                },
                CEvent::Mouse(mouse) => {
                    handle_mouse(&mut app, mouse.kind);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    if key.kind != KeyEventKind::Press {
        return Ok(false);
    }

    if matches!(key.code, KeyCode::Char('c')) && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }

    match app.input_mode {
        InputMode::Filter => return handle_filter_key(app, key),
        InputMode::Help => {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
            ) {
                app.clear_overlay();
            }
            return Ok(false);
        }
        InputMode::Normal => {}
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Down | KeyCode::Char('j') => app.next()?,
        KeyCode::Up | KeyCode::Char('k') => app.previous()?,
        KeyCode::Enter => app.select()?,
        KeyCode::Char('p') => app.toggle_pin()?,
        KeyCode::Char('r') => app.refresh()?,
        KeyCode::PageDown | KeyCode::Char('d') => app.scroll_diff_down(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.scroll_diff_up(10),
        KeyCode::Home | KeyCode::Char('g') => app.scroll_diff_top(),
        KeyCode::End | KeyCode::Char('G') => app.scroll_diff_bottom(),
        KeyCode::Char('f') => app.toggle_view_mode(),
        KeyCode::Char('w') => app.toggle_wrap(),
        KeyCode::Char('s') => app.cycle_sort()?,
        KeyCode::Char('/') => app.enter_filter(),
        KeyCode::Char('?') => app.enter_help(),
        KeyCode::Char('n') => app.next_hunk(),
        KeyCode::Char('N') => app.previous_hunk(),
        _ => {}
    }
    Ok(false)
}

fn handle_filter_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => app.clear_overlay(),
        KeyCode::Backspace => {
            let mut filter = app.filter.clone();
            filter.pop();
            app.update_filter(filter)?;
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.update_filter(String::new())?;
        }
        KeyCode::Char(ch) => {
            let mut filter = app.filter.clone();
            filter.push(ch);
            app.update_filter(filter)?;
        }
        _ => {}
    }
    Ok(false)
}

fn handle_mouse(app: &mut App, kind: MouseEventKind) {
    match kind {
        MouseEventKind::ScrollDown => app.scroll_diff_down(3),
        MouseEventKind::ScrollUp => app.scroll_diff_up(3),
        _ => {}
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide
    )?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("failed to initialize terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        cursor::Show
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn setup_watcher(repo: &Path, tx: mpsc::Sender<Vec<PathBuf>>) -> Result<RecommendedWatcher> {
    let repo = repo.to_path_buf();
    let watched_repo = repo.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let paths: Vec<_> = event
                .paths
                .into_iter()
                .filter(|path| !is_git_internal(&repo, path))
                .collect();
            if !paths.is_empty() {
                let _ = tx.send(paths);
            }
        }
    })
    .context("failed to create filesystem watcher")?;

    watcher
        .watch(&watched_repo, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", watched_repo.display()))?;
    Ok(watcher)
}

fn is_git_internal(repo: &Path, path: &Path) -> bool {
    path.strip_prefix(repo)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str() == ".git")
}

fn drain_watch_events(
    rx: &Receiver<Vec<PathBuf>>,
    pending_refresh: &mut Option<Instant>,
    app: &mut App,
) {
    while let Ok(paths) = rx.try_recv() {
        app.note_changed_paths(paths);
        *pending_refresh = Some(Instant::now() + DEBOUNCE);
    }
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    draw_header(frame, root[0], app);
    match app.view_mode {
        ViewMode::Split => {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(26), Constraint::Percentage(70)])
                .split(root[1]);
            draw_files(frame, body[0], app);
            draw_diff(frame, body[1], app);
        }
        ViewMode::DiffOnly => draw_diff(frame, root[1], app),
    }
    draw_footer(frame, root[2]);
    match app.input_mode {
        InputMode::Help => draw_help_overlay(frame, frame.area()),
        InputMode::Filter => draw_filter_overlay(frame, frame.area(), app),
        InputMode::Normal => {}
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let pinned = app
        .pinned
        .as_ref()
        .map(|path| format!("  pinned:{}", display_path(path)))
        .unwrap_or_default();
    let (added, deleted) = app.totals();
    let mode = match app.view_mode {
        ViewMode::Split => "split",
        ViewMode::DiffOnly => "diff",
    };
    let active = app
        .active_file()
        .map(|file| {
            let added = file
                .added
                .map(|count| format!("+{count}"))
                .unwrap_or_else(|| "+?".to_string());
            let deleted = file
                .deleted
                .map(|count| format!("-{count}"))
                .unwrap_or_else(|| "-?".to_string());
            format!(
                "{}  {}  {} {}",
                file.status.label(),
                file.display_path,
                added,
                deleted
            )
        })
        .unwrap_or_else(|| "no active file".to_string());
    let header = vec![
        Line::from(vec![
            Span::styled(
                " gwatch ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(app.branch.clone(), Style::default().fg(Color::Cyan)),
            Span::raw(format!("  {} files  ", app.files.len())),
            Span::styled(format!("+{added}"), Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(format!("-{deleted}"), Style::default().fg(Color::Red)),
            Span::raw(format!(
                "  mode:{mode}  sort:{}  wrap:{}{}",
                app.sort_mode.label(),
                app.wrap_diff,
                pinned
            )),
        ]),
        Line::from(vec![
            Span::styled(" repo ", Style::default().fg(Color::DarkGray)),
            Span::raw(app.repo.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled(" file ", Style::default().fg(Color::DarkGray)),
            Span::raw(active),
            Span::raw("  "),
            status_badge(&app.status),
            Span::raw(" "),
            Span::styled(
                format!("{}ms ago", app.last_refresh.elapsed().as_millis()),
                Style::default().fg(Color::DarkGray),
            ),
            if app.filter.is_empty() {
                Span::raw("")
            } else {
                Span::styled(
                    format!("  filter:/{}", app.filter),
                    Style::default().fg(Color::Yellow),
                )
            },
        ]),
    ];

    frame.render_widget(
        Paragraph::new(header).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_files(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items = if app.files.is_empty() {
        vec![
            ListItem::new(Line::from(vec![Span::styled(
                "No working-tree changes",
                Style::default().fg(Color::DarkGray),
            )])),
            ListItem::new(Line::from(vec![Span::styled(
                "Start editing files in another terminal.",
                Style::default().fg(Color::DarkGray),
            )])),
        ]
    } else {
        app.files
            .iter()
            .map(|file| {
                let pin = if app.pinned.as_ref() == Some(&file.path) {
                    "*"
                } else {
                    " "
                };
                let recent = if app.is_recent(&file.path) { "!" } else { " " };
                let added = file
                    .added
                    .map(|count| format!(" +{count}"))
                    .unwrap_or_default();
                let deleted = file
                    .deleted
                    .map(|count| format!(" -{count}"))
                    .unwrap_or_default();
                let max_path_len = area.width.saturating_sub(17) as usize;
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{pin} "),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{recent} "),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", file.status.label()),
                        Style::default()
                            .fg(Color::Black)
                            .bg(file.status.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        compact_path(&file.display_path, max_path_len),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(added, Style::default().fg(Color::Green)),
                    Span::styled(deleted, Style::default().fg(Color::Red)),
                ]))
            })
            .collect()
    };

    let mut state = ListState::default();
    if !app.files.is_empty() {
        state.select(Some(app.selected));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title(file_title(app))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(46, 52, 64))
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(list, area, &mut state);

    if app.files.len() > area.height.saturating_sub(2) as usize {
        let mut scrollbar_state = ScrollbarState::new(app.files.len()).position(app.selected);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn file_title(app: &App) -> String {
    if app.files.is_empty() {
        if app.filter.is_empty() {
            "Files".to_string()
        } else {
            format!("Files 0/{} /{}", app.all_files.len(), app.filter)
        }
    } else {
        format!(
            "Files {}/{} of {}",
            app.selected.saturating_add(1),
            app.files.len(),
            app.all_files.len()
        )
    }
}

fn draw_diff(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let title = app
        .active_path()
        .map(|path| {
            let position = if app.diff.is_empty() {
                "0/0".to_string()
            } else {
                format!("{}/{}", app.diff_scroll.saturating_add(1), app.diff.len())
            };
            let hunk = hunk_position_label(app);
            format!("Diff {}  {}  {}", position, hunk, display_path(path))
        })
        .unwrap_or_else(|| "Diff".to_string());
    let lines: Vec<Line<'_>> = app.diff.iter().map(render_diff_line).collect();
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    let mut paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((app.diff_scroll, 0));

    if app.wrap_diff {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }

    frame.render_widget(paragraph, area);

    if app.diff.len() > inner.height as usize {
        let mut scrollbar_state =
            ScrollbarState::new(app.diff.len()).position(app.diff_scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn draw_footer(frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            key_hint("j/k", "files"),
            Span::raw("  "),
            key_hint("wheel u/d", "diff"),
            Span::raw("  "),
            key_hint("g/G", "top/bottom"),
            Span::raw("  "),
            key_hint("n/N", "hunks"),
            Span::raw("  "),
            key_hint("/", "filter"),
            Span::raw("  "),
            key_hint("s", "sort"),
            Span::raw("  "),
            key_hint("f", "focus"),
            Span::raw("  "),
            key_hint("w", "wrap"),
            Span::raw("  "),
            key_hint("p", "pin"),
            Span::raw("  "),
            key_hint("?", "help"),
            Span::raw("  "),
            key_hint("r", "refresh"),
            Span::raw("  "),
            key_hint("q", "quit"),
        ])),
        area,
    );
}

fn draw_help_overlay(frame: &mut Frame<'_>, area: Rect) {
    let popup = centered_rect(72, 72, area);
    let lines = vec![
        Line::from(vec![Span::styled(
            "gwatch help",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from("Navigation"),
        Line::from("  j/k or arrows       move between files"),
        Line::from("  mouse wheel, u/d    scroll diff"),
        Line::from("  g/G                 jump diff top/bottom"),
        Line::from("  n/N                 next/previous hunk"),
        Line::from(""),
        Line::from("Cockpit"),
        Line::from("  /                   filter changed files"),
        Line::from("  s                   cycle sort: path, status, recent, size"),
        Line::from("  p                   pin/unpin selected file"),
        Line::from("  r                   refresh now"),
        Line::from(""),
        Line::from("View"),
        Line::from("  f                   split/focus diff"),
        Line::from("  w                   toggle line wrap"),
        Line::from("  ? or Esc            close help"),
        Line::from("  q                   quit"),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Help").borders(Borders::ALL)),
        popup,
    );
}

fn draw_filter_overlay(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let popup = centered_rect(70, 20, area);
    let lines = vec![
        Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Yellow)),
            Span::raw(app.filter.clone()),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Type to filter files. Enter/Esc closes. Ctrl+U clears.",
            Style::default().fg(Color::DarkGray),
        )]),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Filter files").borders(Borders::ALL)),
        popup,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn hunk_position_label(app: &App) -> String {
    let positions = app.hunk_positions();
    if positions.is_empty() {
        return "hunks 0/0".to_string();
    }
    let current = positions
        .iter()
        .position(|position| *position >= app.diff_scroll as usize)
        .unwrap_or_else(|| positions.len().saturating_sub(1));
    format!("hunks {}/{}", current.saturating_add(1), positions.len())
}

fn status_badge(status: &str) -> Span<'_> {
    let color = if status.starts_with("Refresh failed") {
        Color::Red
    } else {
        Color::Green
    };
    Span::styled(
        format!(" {status} "),
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn key_hint<'a>(key: &'a str, label: &'a str) -> Span<'a> {
    Span::styled(
        format!("[{key}] {label}"),
        Style::default().fg(Color::DarkGray),
    )
}

fn compact_path(path: &str, max_len: usize) -> String {
    if max_len == 0 || path.len() <= max_len {
        return path.to_string();
    }
    if max_len <= 3 {
        return ".".repeat(max_len);
    }

    let keep = max_len.saturating_sub(3);
    let start = path.len().saturating_sub(keep);
    format!("...{}", &path[start..])
}

fn sort_files(files: &mut [ChangedFile], sort_mode: SortMode, recent: &HashMap<PathBuf, Instant>) {
    match sort_mode {
        SortMode::Path => files.sort_by(|a, b| a.display_path.cmp(&b.display_path)),
        SortMode::Status => files.sort_by(|a, b| {
            status_rank(a.status)
                .cmp(&status_rank(b.status))
                .then_with(|| a.display_path.cmp(&b.display_path))
        }),
        SortMode::Recent => files.sort_by(|a, b| {
            recent
                .get(&b.path)
                .cmp(&recent.get(&a.path))
                .then_with(|| a.display_path.cmp(&b.display_path))
        }),
        SortMode::Size => files.sort_by(|a, b| {
            change_size(b)
                .cmp(&change_size(a))
                .then_with(|| a.display_path.cmp(&b.display_path))
        }),
    }
}

fn status_rank(status: FileStatus) -> u8 {
    match status {
        FileStatus::Other => 0,
        FileStatus::Deleted => 1,
        FileStatus::Modified => 2,
        FileStatus::Renamed => 3,
        FileStatus::Added => 4,
        FileStatus::Untracked => 5,
    }
}

fn change_size(file: &ChangedFile) -> u32 {
    file.added.unwrap_or_default() + file.deleted.unwrap_or_default()
}

fn relative_repo_path(repo: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(repo).ok().map(Path::to_path_buf)
}

fn render_diff_line(line: &DiffLine) -> Line<'_> {
    match line.kind {
        DiffKind::Header => Line::from(vec![
            render_line_number(None),
            render_line_number(None),
            Span::styled("  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                clean_diff_text(&line.text),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        DiffKind::Hunk => Line::from(vec![
            render_line_number(None),
            render_line_number(None),
            Span::styled(
                " @@ ",
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(82, 48, 132))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                clean_diff_text(&line.text),
                Style::default()
                    .fg(Color::Rgb(218, 198, 255))
                    .bg(Color::Rgb(35, 28, 49)),
            ),
        ]),
        DiffKind::Added => render_changed_line(
            "+",
            line.old_line,
            line.new_line,
            line.text.strip_prefix('+').unwrap_or(&line.text),
            Color::Rgb(44, 214, 117),
            Color::Rgb(12, 45, 28),
        ),
        DiffKind::Deleted => render_changed_line(
            "-",
            line.old_line,
            line.new_line,
            line.text.strip_prefix('-').unwrap_or(&line.text),
            Color::Rgb(255, 104, 104),
            Color::Rgb(58, 24, 24),
        ),
        DiffKind::Context => Line::from(vec![
            render_line_number(line.old_line),
            render_line_number(line.new_line),
            Span::styled("    ", Style::default().fg(Color::DarkGray)),
            Span::raw(clean_diff_text(&line.text)),
        ]),
    }
}

fn render_changed_line<'a>(
    mark: &'static str,
    old_line: Option<u32>,
    new_line: Option<u32>,
    text: &'a str,
    fg: Color,
    bg: Color,
) -> Line<'a> {
    Line::from(vec![
        render_line_number(old_line),
        render_line_number(new_line),
        Span::styled(
            format!(" {mark}  "),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(text, Style::default().fg(fg).bg(bg)),
    ])
}

fn render_line_number(line: Option<u32>) -> Span<'static> {
    let text = line
        .map(|line| format!("{line:>4}"))
        .unwrap_or_else(|| "    ".to_string());
    Span::styled(text, Style::default().fg(Color::Rgb(96, 103, 118)))
}

fn clean_diff_text(text: &str) -> String {
    text.replace('\t', "    ")
}

fn discover_repo(path: &Path) -> Result<PathBuf> {
    let output = git(path, ["rev-parse", "--show-toplevel"])?;
    let root = String::from_utf8(output.stdout)?.trim().to_string();
    if root.is_empty() {
        bail!("{} is not inside a Git worktree", path.display());
    }
    Ok(PathBuf::from(root))
}

fn git_branch(repo: &Path) -> Result<String> {
    let output = git(repo, ["branch", "--show-current"])?;
    let branch = String::from_utf8(output.stdout)?.trim().to_string();
    if branch.is_empty() {
        let output = git(repo, ["rev-parse", "--short", "HEAD"])?;
        Ok(format!(
            "detached@{}",
            String::from_utf8(output.stdout)?.trim()
        ))
    } else {
        Ok(branch)
    }
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
    let output = git(repo, ["diff", "--numstat", "-z", "HEAD", "--"])
        .or_else(|_| git(repo, ["diff", "--numstat", "-z"]))?;
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
    let output = git_path(repo, ["diff", "HEAD", "--"], path)
        .or_else(|_| git_path(repo, ["diff", "--"], path))?;
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
    for (index, line) in text.lines().enumerate() {
        lines.push(DiffLine::with_numbers(
            DiffKind::Added,
            None,
            Some(index.saturating_add(1).min(u32::MAX as usize) as u32),
            format!("+{line}"),
        ));
    }
    Ok(lines)
}

fn parse_diff_text(text: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    let mut old_line = 0;
    let mut new_line = 0;

    for line in text.lines() {
        if line.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_starts(line) {
                old_line = old_start;
                new_line = new_start;
            }
            lines.push(DiffLine::new(DiffKind::Hunk, line));
        } else if is_diff_header(line) {
            lines.push(DiffLine::new(DiffKind::Header, line));
        } else if line.starts_with('+') {
            lines.push(DiffLine::with_numbers(
                DiffKind::Added,
                None,
                Some(new_line),
                line,
            ));
            new_line = new_line.saturating_add(1);
        } else if line.starts_with('-') {
            lines.push(DiffLine::with_numbers(
                DiffKind::Deleted,
                Some(old_line),
                None,
                line,
            ));
            old_line = old_line.saturating_add(1);
        } else {
            lines.push(DiffLine::with_numbers(
                DiffKind::Context,
                Some(old_line),
                Some(new_line),
                line,
            ));
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
        }
    }

    lines
}

fn is_diff_header(line: &str) -> bool {
    line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
}

fn parse_hunk_starts(line: &str) -> Option<(u32, u32)> {
    let mut parts = line.split_whitespace();
    let _marker = parts.next()?;
    let old = parse_hunk_start(parts.next()?, '-')?;
    let new = parse_hunk_start(parts.next()?, '+')?;
    Some((old, new))
}

fn parse_hunk_start(part: &str, prefix: char) -> Option<u32> {
    let part = part.strip_prefix(prefix)?;
    let start = part.split(',').next()?;
    start.parse().ok()
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

    #[test]
    fn parses_hunk_line_numbers() {
        let diff = parse_diff_text(
            "\
diff --git a/a.txt b/a.txt
@@ -10,2 +20,3 @@
 unchanged
-old
+new
+extra",
        );

        assert_eq!(diff[2].old_line, Some(10));
        assert_eq!(diff[2].new_line, Some(20));
        assert_eq!(diff[3].old_line, Some(11));
        assert_eq!(diff[3].new_line, None);
        assert_eq!(diff[4].old_line, None);
        assert_eq!(diff[4].new_line, Some(21));
        assert_eq!(diff[5].old_line, None);
        assert_eq!(diff[5].new_line, Some(22));
    }

    #[test]
    fn filters_visible_files_without_losing_all_files() {
        let mut app = App::new(PathBuf::from("."));
        app.all_files = vec![
            changed("src/main.rs", FileStatus::Modified),
            changed("README.md", FileStatus::Modified),
        ];

        app.update_filter("read".to_string()).unwrap();

        assert_eq!(app.all_files.len(), 2);
        assert_eq!(app.files.len(), 1);
        assert_eq!(app.files[0].path, PathBuf::from("README.md"));
    }

    #[test]
    fn sorts_visible_files_by_change_size() {
        let mut files = vec![
            changed_with_counts("small.rs", FileStatus::Modified, 1, 1),
            changed_with_counts("large.rs", FileStatus::Modified, 20, 5),
        ];

        sort_files(&mut files, SortMode::Size, &HashMap::new());

        assert_eq!(files[0].path, PathBuf::from("large.rs"));
    }

    #[test]
    fn jumps_between_hunks() {
        let mut app = App::new(PathBuf::from("."));
        app.diff = parse_diff_text(
            "\
diff --git a/a.txt b/a.txt
@@ -1 +1 @@
-a
+b
@@ -10 +10 @@
-x
+y",
        );

        app.next_hunk();
        assert_eq!(app.diff_scroll, 1);
        app.next_hunk();
        assert_eq!(app.diff_scroll, 4);
        app.previous_hunk();
        assert_eq!(app.diff_scroll, 1);
    }

    #[test]
    fn records_recent_paths_relative_to_repo() {
        let repo = PathBuf::from("C:/repo");
        let mut app = App::new(repo.clone());

        app.note_changed_paths(vec![repo.join("src/main.rs")]);

        assert!(app.is_recent(Path::new("src/main.rs")));
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

    fn changed_with_counts(
        path: &str,
        status: FileStatus,
        added: u32,
        deleted: u32,
    ) -> ChangedFile {
        ChangedFile {
            path: PathBuf::from(path),
            display_path: path.to_string(),
            status,
            added: Some(added),
            deleted: Some(deleted),
        }
    }
}
