use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::{
    app::{App, InputMode},
    cli::Cli,
    git::discover_repo,
    ui::draw,
    watcher::setup_watcher,
};

const DEBOUNCE: Duration = Duration::from_millis(180);
const POLL_RATE: Duration = Duration::from_millis(50);
const WHEEL_SCROLL_LINES: usize = 3;
const CTRL_WHEEL_MIN_LINES: usize = 250;
const CTRL_WHEEL_MAX_LINES: usize = 250_000;
const CTRL_WHEEL_DIVISOR: usize = 100;

pub fn run_app(cli: Cli) -> Result<()> {
    let start_dir = cli.repo.unwrap_or(std::env::current_dir()?);
    let repo = discover_repo(&start_dir)?;

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, repo);
    let restore_result = restore_terminal(&mut terminal);

    match (result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), _) => Err(err),
        (Ok(()), Err(err)) => Err(err),
    }
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, repo: PathBuf) -> Result<()> {
    let (watch_tx, watch_rx) = mpsc::channel();
    let mut _watcher = setup_watcher(&repo, watch_tx)?;
    let mut app = App::new(repo);
    app.refresh()?;
    app.mark_baseline();

    let mut pending_refresh: Option<Instant> = None;

    loop {
        terminal.draw(|frame| draw(frame, &mut app))?;

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
                CEvent::Mouse(mouse) => handle_mouse(&mut app, mouse.kind, mouse.modifiers),
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
        KeyCode::Down | KeyCode::Char('j') => app.select_next_file()?,
        KeyCode::Up | KeyCode::Char('k') => app.select_previous_file()?,
        KeyCode::Enter => app.select()?,
        KeyCode::Char('p') => app.toggle_pin()?,
        KeyCode::Char('r') => app.refresh()?,
        KeyCode::PageDown | KeyCode::Char('d') => app.scroll_diff_down(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.scroll_diff_up(10),
        KeyCode::Home | KeyCode::Char('g') => app.scroll_diff_top(),
        KeyCode::End | KeyCode::Char('G') => app.scroll_diff_bottom(),
        KeyCode::Char('f') => app.toggle_view_mode(),
        KeyCode::Char('w') => app.toggle_wrap(),
        KeyCode::Char('b') => app.toggle_session_scope()?,
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

fn handle_mouse(app: &mut App, kind: MouseEventKind, modifiers: KeyModifiers) {
    let amount = if modifiers.contains(KeyModifiers::CONTROL) {
        accelerated_scroll_amount(app.diff_len())
    } else {
        WHEEL_SCROLL_LINES
    };

    match kind {
        MouseEventKind::ScrollDown => app.scroll_diff_down(amount),
        MouseEventKind::ScrollUp => app.scroll_diff_up(amount),
        _ => {}
    }
}

fn accelerated_scroll_amount(diff_len: usize) -> usize {
    if diff_len == 0 {
        return WHEEL_SCROLL_LINES;
    }

    diff_len
        .saturating_div(CTRL_WHEEL_DIVISOR)
        .clamp(CTRL_WHEEL_MIN_LINES, CTRL_WHEEL_MAX_LINES)
        .min(diff_len)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerated_scroll_scales_with_diff_size() {
        assert_eq!(accelerated_scroll_amount(0), WHEEL_SCROLL_LINES);
        assert_eq!(accelerated_scroll_amount(1_000), CTRL_WHEEL_MIN_LINES);
        assert_eq!(accelerated_scroll_amount(1_000_000), 10_000);
        assert_eq!(accelerated_scroll_amount(100_000_000), CTRL_WHEEL_MAX_LINES);
    }

    #[test]
    fn accelerated_scroll_never_exceeds_diff_size() {
        assert_eq!(accelerated_scroll_amount(10), 10);
    }
}
