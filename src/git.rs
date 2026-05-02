use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};

use crate::diff::{DiffKind, DiffLine, parse_diff_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Other,
}

impl FileStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "?",
            Self::Other => "!",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub display_path: String,
    pub status: FileStatus,
    pub added: Option<u32>,
    pub deleted: Option<u32>,
}

pub fn discover_repo(path: &Path) -> Result<PathBuf> {
    let output = git(path, ["rev-parse", "--show-toplevel"])?;
    let root = String::from_utf8(output.stdout)?.trim().to_string();
    if root.is_empty() {
        bail!("{} is not inside a Git worktree", path.display());
    }
    Ok(PathBuf::from(root))
}

pub fn git_branch(repo: &Path) -> Result<String> {
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

pub fn git_changed_files(repo: &Path) -> Result<Vec<ChangedFile>> {
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

pub fn git_diff_for_status(repo: &Path, path: &Path, status: FileStatus) -> Result<Vec<DiffLine>> {
    match status {
        FileStatus::Untracked => git_untracked_preview(repo, path),
        _ => git_diff(repo, path),
    }
}

pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
}
