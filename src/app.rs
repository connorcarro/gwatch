use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Result;

use crate::{
    diff::{DiffKind, DiffLine},
    git::{ChangedFile, FileStatus, display_path, git_changed_files, git_diff_for_status},
};

pub const RECENT_WINDOW: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Split,
    DiffOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Path,
    Status,
    Recent,
    Size,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            Self::Path => Self::Status,
            Self::Status => Self::Recent,
            Self::Recent => Self::Size,
            Self::Size => Self::Path,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Status => "status",
            Self::Recent => "recent",
            Self::Size => "size",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
    Help,
}

pub struct App {
    pub repo: PathBuf,
    pub branch: String,
    pub all_files: Vec<ChangedFile>,
    pub files: Vec<ChangedFile>,
    pub selected: usize,
    pub pinned: Option<PathBuf>,
    pub diff: Vec<DiffLine>,
    pub diff_scroll: u16,
    pub wrap_diff: bool,
    pub view_mode: ViewMode,
    pub sort_mode: SortMode,
    pub input_mode: InputMode,
    pub filter: String,
    pub recent: HashMap<PathBuf, Instant>,
    pub status: String,
    pub last_refresh: Instant,
}

impl App {
    pub fn new(repo: PathBuf) -> Self {
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

    pub fn refresh(&mut self) -> Result<()> {
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

    pub fn rebuild_files(&mut self) {
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

    pub fn reselect(&mut self, previous: Option<&PathBuf>) {
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

    pub fn active_path(&self) -> Option<&PathBuf> {
        if let Some(pinned) = &self.pinned {
            Some(pinned)
        } else {
            self.files.get(self.selected).map(|file| &file.path)
        }
    }

    pub fn active_status(&self) -> Option<FileStatus> {
        let path = self.active_path()?;
        self.all_files
            .iter()
            .find(|file| &file.path == path)
            .map(|file| file.status)
    }

    pub fn active_file(&self) -> Option<&ChangedFile> {
        let path = self.active_path()?;
        self.all_files.iter().find(|file| &file.path == path)
    }

    pub fn load_active_diff(&self) -> Result<Vec<DiffLine>> {
        let Some(path) = self.active_path() else {
            return Ok(vec![DiffLine::context("No working-tree changes.")]);
        };

        match self.active_status() {
            Some(status) => git_diff_for_status(&self.repo, path, status),
            None => Ok(vec![DiffLine::context(format!(
                "{} has no current diff.",
                display_path(path)
            ))]),
        }
    }

    pub fn next(&mut self) -> Result<()> {
        if !self.files.is_empty() {
            self.selected = (self.selected + 1).min(self.files.len() - 1);
            if self.pinned.is_none() {
                self.diff = self.load_active_diff()?;
                self.diff_scroll = 0;
            }
        }
        Ok(())
    }

    pub fn previous(&mut self) -> Result<()> {
        if !self.files.is_empty() {
            self.selected = self.selected.saturating_sub(1);
            if self.pinned.is_none() {
                self.diff = self.load_active_diff()?;
                self.diff_scroll = 0;
            }
        }
        Ok(())
    }

    pub fn select(&mut self) -> Result<()> {
        if let Some(file) = self.files.get(self.selected) {
            self.pinned = None;
            self.diff = git_diff_for_status(&self.repo, &file.path, file.status)?;
            self.diff_scroll = 0;
        }
        Ok(())
    }

    pub fn toggle_pin(&mut self) -> Result<()> {
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

    pub fn scroll_diff_down(&mut self, amount: u16) {
        self.diff_scroll = self.diff_scroll.saturating_add(amount);
        self.clamp_diff_scroll();
    }

    pub fn scroll_diff_up(&mut self, amount: u16) {
        self.diff_scroll = self.diff_scroll.saturating_sub(amount);
    }

    pub fn scroll_diff_top(&mut self) {
        self.diff_scroll = 0;
    }

    pub fn scroll_diff_bottom(&mut self) {
        self.diff_scroll = self.diff.len().saturating_sub(1).min(u16::MAX as usize) as u16;
    }

    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Split => ViewMode::DiffOnly,
            ViewMode::DiffOnly => ViewMode::Split,
        };
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap_diff = !self.wrap_diff;
    }

    pub fn cycle_sort(&mut self) -> Result<()> {
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

    pub fn enter_filter(&mut self) {
        self.input_mode = InputMode::Filter;
    }

    pub fn enter_help(&mut self) {
        self.input_mode = InputMode::Help;
    }

    pub fn clear_overlay(&mut self) {
        self.input_mode = InputMode::Normal;
    }

    pub fn update_filter(&mut self, next_filter: String) -> Result<()> {
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

    pub fn note_changed_paths(&mut self, paths: Vec<PathBuf>) {
        let now = Instant::now();
        for path in paths {
            if let Some(relative) = relative_repo_path(&self.repo, &path) {
                self.recent.insert(relative, now);
            }
        }
    }

    pub fn is_recent(&self, path: &Path) -> bool {
        self.recent
            .get(path)
            .is_some_and(|changed| changed.elapsed() <= RECENT_WINDOW)
    }

    pub fn hunk_positions(&self) -> Vec<usize> {
        self.diff
            .iter()
            .enumerate()
            .filter_map(|(index, line)| matches!(line.kind, DiffKind::Hunk).then_some(index))
            .collect()
    }

    pub fn next_hunk(&mut self) {
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

    pub fn previous_hunk(&mut self) {
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

    pub fn clamp_diff_scroll(&mut self) {
        let max = self.diff.len().saturating_sub(1).min(u16::MAX as usize) as u16;
        self.diff_scroll = self.diff_scroll.min(max);
    }

    pub fn totals(&self) -> (u32, u32) {
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

pub fn sort_files(
    files: &mut [ChangedFile],
    sort_mode: SortMode,
    recent: &HashMap<PathBuf, Instant>,
) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        app.diff = crate::diff::parse_diff_text(
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
