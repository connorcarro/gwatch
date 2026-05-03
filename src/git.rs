use std::{
    ffi::OsStr,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::{
    diff::{DiffLine, DiffParser},
    diff_document::{AsyncDiffWriter, DiffDocument},
};

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
type Numstat = BTreeMap<PathBuf, (Option<u32>, Option<u32>)>;

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

    let mut files: Vec<_> = entries
        .into_iter()
        .map(|(path, status)| ChangedFile {
            display_path: display_path(&path),
            path,
            status,
            added: None,
            deleted: None,
        })
        .collect();

    files.sort_by(|a, b| a.display_path.cmp(&b.display_path));
    Ok(files)
}

pub fn git_diff_for_status(repo: &Path, path: &Path, status: FileStatus) -> Result<DiffDocument> {
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

#[cfg(test)]
fn parse_numstat(bytes: &[u8]) -> Numstat {
    let mut stats = BTreeMap::new();
    let mut parts = bytes
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

    stats
}

#[cfg(test)]
fn parse_count(bytes: &[u8]) -> Option<u32> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn git_diff(repo: &Path, path: &Path) -> Result<DiffDocument> {
    async_git_path(repo, path)
}

fn git_untracked_preview(repo: &Path, path: &Path) -> Result<DiffDocument> {
    let full_path = repo.join(path);
    if !full_path.is_file() {
        return DiffDocument::from_lines([DiffLine::context(
            "Untracked path is not a regular file.",
        )]);
    }

    if is_binary_file(&full_path)? {
        return DiffDocument::from_lines([DiffLine::context("Binary untracked file.")]);
    }

    DiffDocument::lazy_untracked(full_path, display_path(path))
}

fn is_binary_file(path: &Path) -> Result<bool> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut buffer = [0; 8192];
    let read = std::io::Read::read(&mut file, &mut buffer)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(buffer[..read].contains(&0))
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

fn async_git_path(repo: &Path, path: &Path) -> Result<DiffDocument> {
    let repo = repo.to_path_buf();
    let path = path.to_path_buf();
    DiffDocument::async_spool(move |writer| {
        match stream_git_path(&repo, ["diff", "HEAD", "--"], &path, writer) {
            Ok(()) => Ok(()),
            Err(writer) => match stream_git_path(&repo, ["diff", "--"], &path, writer) {
                Ok(()) => Ok(()),
                Err(_) => Err(anyhow!("failed to load git diff")),
            },
        }
    })
}

fn stream_git_path<I, S>(
    repo: &Path,
    args: I,
    path: &Path,
    mut writer: AsyncDiffWriter,
) -> std::result::Result<(), AsyncDiffWriter>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    match try_stream_git_path(repo, args, path, &mut writer) {
        Ok(()) => Ok(()),
        Err(_) => Err(writer),
    }
}

fn try_stream_git_path<I, S>(
    repo: &Path,
    args: I,
    path: &Path,
    writer: &mut AsyncDiffWriter,
) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run git")?;

    let stdout = child
        .stdout
        .take()
        .context("failed to capture git stdout")?;
    let mut parser = DiffParser::new();
    let mut wrote_any = false;

    for line in BufReader::new(stdout).lines() {
        let line = line.context("failed to read git diff output")?;
        writer.push(&parser.parse_line(&line))?;
        wrote_any = true;
    }

    let output = child
        .wait_with_output()
        .context("failed to finish git diff")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(stderr.trim().to_string()));
    }
    if !wrote_any {
        writer.push(&DiffLine::context("No current diff."))?;
    }
    Ok(())
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

    #[test]
    fn parses_regular_numstat_entries() {
        let parsed = parse_numstat(b"12\t3\tsrc/main.rs\0-\t-\tassets/logo.png\0");

        assert_eq!(
            parsed.get(&PathBuf::from("src/main.rs")),
            Some(&(Some(12), Some(3)))
        );
        assert_eq!(
            parsed.get(&PathBuf::from("assets/logo.png")),
            Some(&(None, None))
        );
    }

    #[test]
    fn parses_renamed_numstat_entries() {
        let parsed = parse_numstat(b"5\t2\0old.rs\0src/new.rs\0");

        assert_eq!(
            parsed,
            BTreeMap::from([(PathBuf::from("src/new.rs"), (Some(5), Some(2)))])
        );
    }
}
