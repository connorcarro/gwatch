use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Result;

use crate::{
    diff::DiffLine,
    diff_document::DiffDocument,
    git::{
        ChangedFile, FileStatus, display_path, git_branch, git_changed_files, git_diff_for_status,
    },
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
    pub diff: Option<DiffDocument>,
    pub diff_line_cache: Option<DiffLineCache>,
    pub diff_scroll: usize,
    pub wrap_diff: bool,
    pub view_mode: ViewMode,
    pub sort_mode: SortMode,
    pub input_mode: InputMode,
    pub filter: String,
    pub session_only: bool,
    pub baseline_paths: HashSet<PathBuf>,
    pub session_paths: HashSet<PathBuf>,
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
            diff: None,
            diff_line_cache: None,
            diff_scroll: 0,
            wrap_diff: false,
            view_mode: ViewMode::Split,
            sort_mode: SortMode::Path,
            input_mode: InputMode::Normal,
            filter: String::new(),
            session_only: false,
            baseline_paths: HashSet::new(),
            session_paths: HashSet::new(),
            recent: HashMap::new(),
            status: "Starting".to_string(),
            last_refresh: Instant::now(),
        }
    }

    pub fn refresh(&mut self) -> Result<()> {
        let previous_selection = self.active_path().cloned();
        self.branch = git_branch(&self.repo).unwrap_or_else(|_| "unknown".to_string());
        self.all_files = git_changed_files(&self.repo)?;
        self.rebuild_files();
        self.reselect(previous_selection.as_ref());
        self.set_diff(self.load_active_diff()?);
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
                let matches_filter =
                    filter.is_empty() || file.display_path.to_ascii_lowercase().contains(&filter);
                matches_filter && (!self.session_only || self.is_session_change(&file.path))
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

        if let Some(path) = previous
            && let Some(index) = self.files.iter().position(|file| &file.path == path)
        {
            self.selected = index;
            return;
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

    pub fn load_active_diff(&self) -> Result<DiffDocument> {
        let Some(path) = self.active_path() else {
            return DiffDocument::from_lines([DiffLine::context("No working-tree changes.")]);
        };

        match self.active_status() {
            Some(status) => git_diff_for_status(&self.repo, path, status),
            None => DiffDocument::from_lines([DiffLine::context(format!(
                "{} has no current diff.",
                display_path(path)
            ))]),
        }
    }

    pub fn select_next_file(&mut self) -> Result<()> {
        if !self.files.is_empty() {
            self.selected = (self.selected + 1).min(self.files.len() - 1);
            if self.pinned.is_none() {
                self.set_diff(self.load_active_diff()?);
                self.diff_scroll = 0;
            }
        }
        Ok(())
    }

    pub fn select_previous_file(&mut self) -> Result<()> {
        if !self.files.is_empty() {
            self.selected = self.selected.saturating_sub(1);
            if self.pinned.is_none() {
                self.set_diff(self.load_active_diff()?);
                self.diff_scroll = 0;
            }
        }
        Ok(())
    }

    pub fn select(&mut self) -> Result<()> {
        if let Some(file) = self.files.get(self.selected) {
            self.pinned = None;
            self.set_diff(git_diff_for_status(&self.repo, &file.path, file.status)?);
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
            self.set_diff(self.load_active_diff()?);
            self.diff_scroll = 0;
        }
        Ok(())
    }

    pub fn scroll_diff_down(&mut self, amount: usize) {
        self.diff_scroll = self.diff_scroll.saturating_add(amount);
        self.clamp_diff_scroll();
    }

    pub fn scroll_diff_up(&mut self, amount: usize) {
        self.diff_scroll = self.diff_scroll.saturating_sub(amount);
    }

    pub fn scroll_diff_top(&mut self) {
        self.diff_scroll = 0;
    }

    pub fn scroll_diff_bottom(&mut self) {
        self.diff_scroll = self.diff_len().saturating_sub(1);
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

    pub fn mark_baseline(&mut self) {
        self.baseline_paths = self
            .all_files
            .iter()
            .map(|file| file.path.clone())
            .collect();
        self.session_paths.clear();
    }

    pub fn toggle_session_scope(&mut self) -> Result<()> {
        let previous_selection = self.active_path().cloned();
        self.session_only = !self.session_only;
        self.rebuild_files();
        self.reselect(previous_selection.as_ref());
        if self.pinned.is_none() {
            self.set_diff(self.load_active_diff()?);
            self.diff_scroll = 0;
        }
        Ok(())
    }

    pub fn cycle_sort(&mut self) -> Result<()> {
        let previous_selection = self.active_path().cloned();
        self.sort_mode = self.sort_mode.next();
        self.rebuild_files();
        self.reselect(previous_selection.as_ref());
        if self.pinned.is_none() {
            self.set_diff(self.load_active_diff()?);
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
            self.set_diff(self.load_active_diff()?);
            self.diff_scroll = 0;
        }
        Ok(())
    }

    pub fn note_changed_paths(&mut self, paths: Vec<PathBuf>) {
        let now = Instant::now();
        for path in paths {
            if let Some(relative) = relative_repo_path(&self.repo, &path) {
                self.recent.insert(relative.clone(), now);
                self.session_paths.insert(relative);
            }
        }
    }

    pub fn is_recent(&self, path: &Path) -> bool {
        self.recent
            .get(path)
            .is_some_and(|changed| changed.elapsed() <= RECENT_WINDOW)
    }

    pub fn is_session_change(&self, path: &Path) -> bool {
        self.session_paths.contains(path) || !self.baseline_paths.contains(path)
    }

    pub fn set_diff(&mut self, diff: DiffDocument) {
        self.diff = Some(diff);
        self.diff_line_cache = None;
    }

    pub fn diff_len(&self) -> usize {
        self.diff.as_ref().map_or(0, DiffDocument::len)
    }

    pub fn diff_is_empty(&self) -> bool {
        self.diff.as_ref().is_none_or(DiffDocument::is_empty)
    }

    pub fn diff_hunks(&self) -> &[usize] {
        self.diff.as_ref().map_or(&[], DiffDocument::hunk_positions)
    }

    pub fn hunk_count(&self) -> usize {
        self.diff.as_ref().map_or(0, DiffDocument::hunk_count)
    }

    pub fn hunk_ordinal_at_or_after_scroll(&self) -> Option<usize> {
        self.diff
            .as_ref()
            .and_then(|diff| diff.hunk_ordinal_at_or_after(self.diff_scroll))
    }

    pub fn diff_lines(&mut self, range: Range<usize>) -> Result<Vec<DiffLine>> {
        let Some(diff) = &self.diff else {
            return Ok(Vec::new());
        };

        if let Some(cache) = &self.diff_line_cache
            && cache.range.start <= range.start
            && cache.range.end >= range.end
        {
            let start = range.start - cache.range.start;
            let end = start + (range.end - range.start);
            if end <= cache.lines.len() {
                return Ok(cache.lines[start..end].to_vec());
            }
        }

        let lines = diff.lines(range.clone())?;
        if diff.is_complete() {
            self.diff_line_cache = Some(DiffLineCache {
                range,
                lines: lines.clone(),
            });
        } else {
            self.diff_line_cache = None;
        }
        Ok(lines)
    }

    pub fn next_hunk(&mut self) {
        let Some(next) = self
            .diff
            .as_ref()
            .and_then(|diff| diff.next_hunk_after(self.diff_scroll))
        else {
            return;
        };
        self.diff_scroll = next;
    }

    pub fn previous_hunk(&mut self) {
        let Some(previous) = self
            .diff
            .as_ref()
            .and_then(|diff| diff.previous_hunk_before(self.diff_scroll))
        else {
            return;
        };
        self.diff_scroll = previous;
    }

    pub fn clamp_diff_scroll(&mut self) {
        let max = self.diff_len().saturating_sub(1);
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

pub struct DiffLineCache {
    pub range: Range<usize>,
    pub lines: Vec<DiffLine>,
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
        app.set_diff(
            DiffDocument::from_lines(crate::diff::parse_diff_text(
                "\
diff --git a/a.txt b/a.txt
@@ -1 +1 @@
-a
+b
@@ -10 +10 @@
-x
+y",
            ))
            .unwrap(),
        );

        app.next_hunk();
        assert_eq!(app.diff_scroll, 1);
        app.next_hunk();
        assert_eq!(app.diff_scroll, 4);
        app.previous_hunk();
        assert_eq!(app.diff_scroll, 1);
    }

    #[test]
    fn diff_scroll_supports_positions_beyond_u16() {
        let mut app = App::new(PathBuf::from("."));
        app.set_diff(
            DiffDocument::from_lines(
                (0..100_000).map(|index| DiffLine::context(index.to_string())),
            )
            .unwrap(),
        );

        app.scroll_diff_bottom();

        assert_eq!(app.diff_scroll, 99_999);
    }

    #[test]
    fn caches_hunks_when_diff_is_set() {
        let mut app = App::new(PathBuf::from("."));
        app.set_diff(
            DiffDocument::from_lines(crate::diff::parse_diff_text(
                "\
diff --git a/a.txt b/a.txt
@@ -1 +1 @@
-a
+b
@@ -50 +50 @@
-x
+y",
            ))
            .unwrap(),
        );

        assert_eq!(app.diff_hunks(), &[1, 4]);
    }

    #[test]
    fn diff_lines_reuses_cached_superset_range() {
        let mut app = App::new(PathBuf::from("."));
        app.set_diff(
            DiffDocument::from_lines((0..100).map(|index| DiffLine::context(index.to_string())))
                .unwrap(),
        );

        let first = app.diff_lines(10..30).unwrap();
        let second = app.diff_lines(15..18).unwrap();

        assert_eq!(first[5].text, "15");
        assert_eq!(
            second
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            vec!["15", "16", "17"]
        );
        assert_eq!(app.diff_line_cache.as_ref().unwrap().range, 10..30);
    }

    #[test]
    fn diff_line_cache_miss_when_cached_lines_are_shorter_than_range() {
        let mut app = App::new(PathBuf::from("."));
        app.set_diff(
            DiffDocument::from_lines((0..5).map(|index| DiffLine::context(index.to_string())))
                .unwrap(),
        );
        app.diff_line_cache = Some(DiffLineCache {
            range: 0..10,
            lines: vec![DiffLine::context("short")],
        });

        let lines = app.diff_lines(0..5).unwrap();

        assert_eq!(lines.len(), 5);
        assert_eq!(lines[4].text, "4");
    }

    #[test]
    fn records_recent_paths_relative_to_repo() {
        let repo = PathBuf::from("C:/repo");
        let mut app = App::new(repo.clone());

        app.note_changed_paths(vec![repo.join("src/main.rs")]);

        assert!(app.is_recent(Path::new("src/main.rs")));
    }

    #[test]
    fn session_scope_filters_to_paths_changed_after_baseline() {
        let repo = PathBuf::from("C:/repo");
        let mut app = App::new(repo.clone());
        app.all_files = vec![
            changed("src/main.rs", FileStatus::Modified),
            changed("src/lib.rs", FileStatus::Modified),
        ];
        app.mark_baseline();
        app.note_changed_paths(vec![repo.join("src/lib.rs")]);
        app.pinned = Some(PathBuf::from("pinned.rs"));

        app.toggle_session_scope().unwrap();

        assert_eq!(app.files.len(), 1);
        assert_eq!(app.files[0].path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn session_scope_includes_files_added_after_baseline_even_without_event() {
        let mut app = App::new(PathBuf::from("."));
        app.all_files = vec![changed("existing.rs", FileStatus::Modified)];
        app.mark_baseline();
        app.all_files.push(changed("new.rs", FileStatus::Untracked));
        app.pinned = Some(PathBuf::from("pinned.rs"));

        app.toggle_session_scope().unwrap();

        assert_eq!(app.files.len(), 1);
        assert_eq!(app.files[0].path, PathBuf::from("new.rs"));
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
