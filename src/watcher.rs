use std::{path::Path, path::PathBuf, sync::mpsc};

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub fn setup_watcher(repo: &Path, tx: mpsc::Sender<Vec<PathBuf>>) -> Result<RecommendedWatcher> {
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
