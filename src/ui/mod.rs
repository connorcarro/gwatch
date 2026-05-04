use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use std::{ops::Range, path::Path};

use crate::{
    app::{App, InputMode, ViewMode},
    diff::{DiffKind, DiffLine},
    git::{FileStatus, display_path},
    syntax::highlighted_spans,
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
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
    let scope = if app.session_only { "session" } else { "all" };
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
            Span::raw(format!("  {} files  ", app.all_files.len())),
            Span::styled(format!("+{added}"), Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(format!("-{deleted}"), Style::default().fg(Color::Red)),
            Span::raw(format!(
                "  mode:{mode}  scope:{scope}  sort:{}  wrap:{}{}",
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
                let session = if app.is_session_change(&file.path) {
                    "~"
                } else {
                    " "
                };
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
                        format!("{session} "),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{} ", file.status.label()),
                        Style::default()
                            .fg(Color::Black)
                            .bg(status_color(file.status))
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

fn draw_diff(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let title = app
        .active_path()
        .map(|path| {
            let diff_len = app.diff_len();
            let position = if app.diff_is_empty() {
                "0/0".to_string()
            } else {
                format!("{}/{}", app.diff_scroll.saturating_add(1), diff_len)
            };
            let hunk = hunk_position_label(app);
            format!("Diff {}  {}  {}", position, hunk, display_path(path))
        })
        .unwrap_or_else(|| "Diff".to_string());
    let active_path = app.active_path().cloned();
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    let visible_height = inner.height as usize;
    let visible = visible_diff_range(app, visible_height);
    let lines: Vec<Line<'_>> = app
        .diff_lines(visible)
        .unwrap_or_else(|err| vec![DiffLine::context(format!("Failed to read diff: {err}"))])
        .iter()
        .map(|line| render_diff_line(line, active_path.as_deref()))
        .collect();
    let mut paragraph = Paragraph::new(lines).block(block);

    if app.wrap_diff {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }

    frame.render_widget(paragraph, area);

    if app.diff_len() > inner.height as usize {
        let mut scrollbar_state = ScrollbarState::new(app.diff_len()).position(app.diff_scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect) {
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
            key_hint("b", "scope"),
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
        Line::from("  b                   toggle all changes/session changes"),
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
    let hunk_count = app.hunk_count();
    if hunk_count == 0 {
        return "hunks 0/0".to_string();
    }
    let current = app
        .hunk_ordinal_at_or_after_scroll()
        .unwrap_or_else(|| hunk_count.saturating_sub(1));
    format!("hunks {}/{}", current.saturating_add(1), hunk_count)
}

fn visible_diff_range(app: &App, visible_height: usize) -> Range<usize> {
    let diff_len = app.diff_len();
    if diff_len == 0 || visible_height == 0 {
        return 0..0;
    }

    let start = app.diff_scroll.min(diff_len.saturating_sub(1));
    let end = start
        .saturating_add(visible_height.saturating_add(1))
        .min(diff_len);
    start..end
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
    let char_len = path.chars().count();
    if max_len == 0 || char_len <= max_len {
        return path.to_string();
    }
    if max_len <= 3 {
        return ".".repeat(max_len);
    }

    let keep = max_len.saturating_sub(3);
    let suffix: String = path
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{suffix}")
}

fn render_diff_line(line: &DiffLine, path: Option<&Path>) -> Line<'static> {
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
            path,
        ),
        DiffKind::Deleted => render_changed_line(
            "-",
            line.old_line,
            line.new_line,
            line.text.strip_prefix('-').unwrap_or(&line.text),
            Color::Rgb(255, 104, 104),
            Color::Rgb(58, 24, 24),
            path,
        ),
        DiffKind::Context => {
            let mut spans = vec![
                render_line_number(line.old_line),
                render_line_number(line.new_line),
                Span::styled("    ", Style::default().fg(Color::DarkGray)),
            ];
            spans.extend(highlighted_spans(
                path,
                &clean_diff_text(&line.text),
                Style::default(),
            ));
            Line::from(spans)
        }
    }
}

fn render_changed_line(
    mark: &'static str,
    old_line: Option<u32>,
    new_line: Option<u32>,
    text: &str,
    fg: Color,
    bg: Color,
    path: Option<&Path>,
) -> Line<'static> {
    let style = Style::default().fg(fg).bg(bg);
    let mut spans = vec![
        render_line_number(old_line),
        render_line_number(new_line),
        Span::styled(
            format!(" {mark}  "),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
    ];
    spans.extend(highlighted_spans(path, text, style));
    Line::from(spans)
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

fn status_color(status: FileStatus) -> Color {
    match status {
        FileStatus::Added | FileStatus::Untracked => Color::Green,
        FileStatus::Modified => Color::Yellow,
        FileStatus::Deleted => Color::Red,
        FileStatus::Renamed => Color::Cyan,
        FileStatus::Other => Color::Magenta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_ascii_paths() {
        assert_eq!(compact_path("src/deep/main.rs", 10), "...main.rs");
    }

    #[test]
    fn compacts_unicode_paths_without_slicing_inside_codepoint() {
        assert_eq!(compact_path("src/naive/éclair.rs", 12), "...éclair.rs");
    }

    #[test]
    fn renders_highlighted_context_line_without_losing_content() {
        let line = DiffLine::with_numbers(DiffKind::Context, Some(1), Some(1), "fn main() {}");
        let rendered = render_diff_line(&line, Some(Path::new("main.rs")));
        let text: String = rendered
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert_eq!(text, "   1   1    fn main() {}");
    }

    #[test]
    fn renders_highlighted_added_line_with_diff_marker() {
        let line = DiffLine::with_numbers(DiffKind::Added, None, Some(3), "+let answer = 42;");
        let rendered = render_diff_line(&line, Some(Path::new("main.rs")));
        let text: String = rendered
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert_eq!(text, "       3 +  let answer = 42;");
    }

    #[test]
    fn visible_diff_range_only_includes_viewport_lines() {
        let mut app = App::new(Path::new(".").to_path_buf());
        app.set_diff(
            crate::diff::document::DiffDocument::from_lines(
                (0..10_000).map(|line| DiffLine::context(line.to_string())),
            )
            .unwrap(),
        );
        app.diff_scroll = 9_900;

        assert_eq!(visible_diff_range(&app, 20), 9_900..9_921);
    }

    #[test]
    fn visible_diff_range_handles_scroll_past_end() {
        let mut app = App::new(Path::new(".").to_path_buf());
        app.set_diff(
            crate::diff::document::DiffDocument::from_lines(
                (0..10).map(|line| DiffLine::context(line.to_string())),
            )
            .unwrap(),
        );
        app.diff_scroll = 999;

        assert_eq!(visible_diff_range(&app, 20), 9..10);
    }
}
