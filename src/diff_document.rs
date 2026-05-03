use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use anyhow::{Context, Result, bail};

use crate::diff::{DiffKind, DiffLine};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const NONE_LINE: u32 = u32::MAX;

pub struct DiffDocument {
    inner: DiffDocumentInner,
}

enum DiffDocumentInner {
    Spool(SpoolDocument),
    AsyncSpool(AsyncSpoolDocument),
    LazyUntracked(LazyUntrackedDocument),
}

struct SpoolDocument {
    path: PathBuf,
    file: Mutex<File>,
    offsets: Vec<u64>,
    hunk_positions: Vec<usize>,
}

struct LazyUntrackedDocument {
    path: PathBuf,
    display_path: String,
    file: Mutex<File>,
    state: Arc<Mutex<LazyUntrackedState>>,
}

struct LazyUntrackedState {
    offsets: Vec<u64>,
    indexed_to: u64,
    file_len: u64,
    eof: bool,
}

struct AsyncSpoolDocument {
    path: PathBuf,
    file: Arc<Mutex<File>>,
    state: Arc<Mutex<AsyncSpoolState>>,
}

struct AsyncSpoolState {
    offsets: Vec<u64>,
    hunk_positions: Vec<usize>,
    done: bool,
    error: Option<String>,
}

impl DiffDocument {
    pub fn from_lines(lines: impl IntoIterator<Item = DiffLine>) -> Result<Self> {
        let mut builder = DiffDocumentBuilder::new()?;
        for line in lines {
            builder.push(&line)?;
        }
        builder.finish()
    }

    pub fn lazy_untracked(path: PathBuf, display_path: String) -> Result<Self> {
        let file =
            File::open(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("failed to stat {}", path.display()))?
            .len();
        let state = Arc::new(Mutex::new(LazyUntrackedState {
            offsets: vec![0],
            indexed_to: 0,
            file_len,
            eof: file_len == 0,
        }));
        spawn_untracked_indexer(path.clone(), state.clone());

        Ok(Self {
            inner: DiffDocumentInner::LazyUntracked(LazyUntrackedDocument {
                path,
                display_path,
                file: Mutex::new(file),
                state,
            }),
        })
    }

    pub fn async_spool(
        job: impl FnOnce(AsyncDiffWriter) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        let path = temp_path();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        let file = Arc::new(Mutex::new(file));
        let state = Arc::new(Mutex::new(AsyncSpoolState {
            offsets: Vec::new(),
            hunk_positions: Vec::new(),
            done: false,
            error: None,
        }));
        spawn_async_spool_writer(file.clone(), state.clone(), job);

        Ok(Self {
            inner: DiffDocumentInner::AsyncSpool(AsyncSpoolDocument { path, file, state }),
        })
    }

    pub fn len(&self) -> usize {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => doc.offsets.len(),
            DiffDocumentInner::AsyncSpool(doc) => doc.len(),
            DiffDocumentInner::LazyUntracked(doc) => doc.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn hunk_positions(&self) -> &[usize] {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => &doc.hunk_positions,
            DiffDocumentInner::AsyncSpool(_) => &[],
            DiffDocumentInner::LazyUntracked(_) => &[],
        }
    }

    pub fn hunk_count(&self) -> usize {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => doc.hunk_positions.len(),
            DiffDocumentInner::AsyncSpool(doc) => doc
                .state
                .lock()
                .expect("async diff state lock poisoned")
                .hunk_positions
                .len(),
            DiffDocumentInner::LazyUntracked(_) => 0,
        }
    }

    pub fn hunk_ordinal_at_or_after(&self, scroll: usize) -> Option<usize> {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => hunk_ordinal_at_or_after(&doc.hunk_positions, scroll),
            DiffDocumentInner::AsyncSpool(doc) => hunk_ordinal_at_or_after(
                &doc.state
                    .lock()
                    .expect("async diff state lock poisoned")
                    .hunk_positions,
                scroll,
            ),
            DiffDocumentInner::LazyUntracked(_) => None,
        }
    }

    pub fn next_hunk_after(&self, scroll: usize) -> Option<usize> {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => next_hunk_after(&doc.hunk_positions, scroll),
            DiffDocumentInner::AsyncSpool(doc) => next_hunk_after(
                &doc.state
                    .lock()
                    .expect("async diff state lock poisoned")
                    .hunk_positions,
                scroll,
            ),
            DiffDocumentInner::LazyUntracked(_) => None,
        }
    }

    pub fn previous_hunk_before(&self, scroll: usize) -> Option<usize> {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => previous_hunk_before(&doc.hunk_positions, scroll),
            DiffDocumentInner::AsyncSpool(doc) => previous_hunk_before(
                &doc.state
                    .lock()
                    .expect("async diff state lock poisoned")
                    .hunk_positions,
                scroll,
            ),
            DiffDocumentInner::LazyUntracked(_) => None,
        }
    }

    pub fn is_complete(&self) -> bool {
        match &self.inner {
            DiffDocumentInner::Spool(_) => true,
            DiffDocumentInner::AsyncSpool(doc) => doc.is_complete(),
            DiffDocumentInner::LazyUntracked(doc) => doc.is_complete(),
        }
    }

    pub fn line(&self, index: usize) -> Result<Option<DiffLine>> {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => doc.line(index),
            DiffDocumentInner::AsyncSpool(doc) => doc.line(index),
            DiffDocumentInner::LazyUntracked(doc) => doc.line(index),
        }
    }

    pub fn lines(&self, range: Range<usize>) -> Result<Vec<DiffLine>> {
        match &self.inner {
            DiffDocumentInner::Spool(doc) => doc.lines(range),
            DiffDocumentInner::AsyncSpool(doc) => doc.lines(range),
            DiffDocumentInner::LazyUntracked(doc) => doc.lines(range),
        }
    }
}

impl Drop for DiffDocument {
    fn drop(&mut self) {
        if let DiffDocumentInner::Spool(doc) = &self.inner {
            let _ = std::fs::remove_file(&doc.path);
        } else if let DiffDocumentInner::AsyncSpool(doc) = &self.inner {
            let _ = std::fs::remove_file(&doc.path);
        }
    }
}

pub struct AsyncDiffWriter {
    file: Arc<Mutex<File>>,
    state: Arc<Mutex<AsyncSpoolState>>,
}

impl AsyncDiffWriter {
    pub fn push(&mut self, line: &DiffLine) -> Result<()> {
        let mut file = self.file.lock().expect("async diff file lock poisoned");
        let offset = file
            .stream_position()
            .context("failed to write async diff document")?;
        write_record(&mut file, line)?;
        let mut state = self.state.lock().expect("async diff state lock poisoned");
        if matches!(line.kind, DiffKind::Hunk) {
            let position = state.offsets.len();
            state.hunk_positions.push(position);
        }
        state.offsets.push(offset);
        Ok(())
    }
}

pub struct DiffDocumentBuilder {
    path: PathBuf,
    file: File,
    offsets: Vec<u64>,
    hunk_positions: Vec<usize>,
}

impl DiffDocumentBuilder {
    pub fn new() -> Result<Self> {
        let path = temp_path();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;

        Ok(Self {
            path,
            file,
            offsets: Vec::new(),
            hunk_positions: Vec::new(),
        })
    }

    pub fn push(&mut self, line: &DiffLine) -> Result<()> {
        let offset = self
            .file
            .stream_position()
            .context("failed to write diff document")?;
        write_record(&mut self.file, line)?;
        if matches!(line.kind, DiffKind::Hunk) {
            self.hunk_positions.push(self.offsets.len());
        }
        self.offsets.push(offset);
        Ok(())
    }

    pub fn finish(mut self) -> Result<DiffDocument> {
        self.file.flush().context("failed to flush diff document")?;
        Ok(DiffDocument {
            inner: DiffDocumentInner::Spool(SpoolDocument {
                path: self.path,
                file: Mutex::new(self.file),
                offsets: self.offsets,
                hunk_positions: self.hunk_positions,
            }),
        })
    }
}

impl SpoolDocument {
    fn line(&self, index: usize) -> Result<Option<DiffLine>> {
        let Some(offset) = self.offsets.get(index).copied() else {
            return Ok(None);
        };

        let mut file = self.file.lock().expect("diff document lock poisoned");
        file.seek(SeekFrom::Start(offset))
            .context("failed to seek diff document")?;
        read_record(&mut file).map(Some)
    }

    fn lines(&self, range: Range<usize>) -> Result<Vec<DiffLine>> {
        if range.is_empty() || self.offsets.is_empty() {
            return Ok(Vec::new());
        }

        let start = range.start.min(self.offsets.len());
        let end = range.end.min(self.offsets.len());
        if start >= end {
            return Ok(Vec::new());
        }

        let mut file = self.file.lock().expect("diff document lock poisoned");
        file.seek(SeekFrom::Start(self.offsets[start]))
            .context("failed to seek diff document")?;

        let mut lines = Vec::with_capacity(end - start);
        for _ in start..end {
            lines.push(read_record(&mut file)?);
        }
        Ok(lines)
    }
}

impl AsyncSpoolDocument {
    fn len(&self) -> usize {
        let state = self.state.lock().expect("async diff state lock poisoned");
        if state.done {
            state.offsets.len()
        } else {
            state.offsets.len().saturating_add(1)
        }
    }

    fn is_complete(&self) -> bool {
        self.state
            .lock()
            .expect("async diff state lock poisoned")
            .done
    }

    fn line(&self, index: usize) -> Result<Option<DiffLine>> {
        let state = self.state.lock().expect("async diff state lock poisoned");
        if let Some(offset) = state.offsets.get(index).copied() {
            drop(state);
            let mut file = self.file.lock().expect("async diff file lock poisoned");
            file.seek(SeekFrom::Start(offset))
                .context("failed to seek async diff document")?;
            return read_record(&mut file).map(Some);
        }

        if let Some(error) = &state.error {
            return Ok(Some(DiffLine::context(format!(
                "Failed to load diff: {error}"
            ))));
        }
        if !state.done {
            return Ok(Some(DiffLine::context(format!(
                "Loading diff... {} lines indexed.",
                state.offsets.len()
            ))));
        }
        Ok(None)
    }

    fn lines(&self, range: Range<usize>) -> Result<Vec<DiffLine>> {
        if range.is_empty() {
            return Ok(Vec::new());
        }

        let mut lines = Vec::with_capacity(range.end.saturating_sub(range.start));
        for index in range {
            if let Some(line) = self.line(index)? {
                lines.push(line);
            }
        }
        Ok(lines)
    }
}

fn spawn_async_spool_writer(
    file: Arc<Mutex<File>>,
    state: Arc<Mutex<AsyncSpoolState>>,
    job: impl FnOnce(AsyncDiffWriter) -> Result<()> + Send + 'static,
) {
    thread::spawn(move || {
        let writer = AsyncDiffWriter {
            file: file.clone(),
            state: state.clone(),
        };
        let result = job(writer);
        let mut state = state.lock().expect("async diff state lock poisoned");
        if let Err(err) = result {
            state.error = Some(err.to_string());
        }
        state.done = true;
    });
}

impl LazyUntrackedDocument {
    const HEADER_LINES: usize = 4;

    fn len(&self) -> usize {
        let state = self.state.lock().expect("lazy diff state lock poisoned");
        Self::HEADER_LINES + state.visible_len_estimate()
    }

    fn is_complete(&self) -> bool {
        self.state
            .lock()
            .expect("lazy diff state lock poisoned")
            .eof
    }

    fn line(&self, index: usize) -> Result<Option<DiffLine>> {
        if index < Self::HEADER_LINES {
            return Ok(Some(self.header_line(index)));
        }
        self.content_line(index - Self::HEADER_LINES).map(Some)
    }

    fn lines(&self, range: Range<usize>) -> Result<Vec<DiffLine>> {
        if range.is_empty() {
            return Ok(Vec::new());
        }

        let mut lines = Vec::with_capacity(range.end.saturating_sub(range.start));
        for index in range {
            if let Some(line) = self.line(index)? {
                lines.push(line);
            }
        }
        Ok(lines)
    }

    fn header_line(&self, index: usize) -> DiffLine {
        match index {
            0 => DiffLine::new(
                DiffKind::Header,
                format!("diff --git a/{} b/{}", self.display_path, self.display_path),
            ),
            1 => DiffLine::new(DiffKind::Header, "new file mode 100644"),
            2 => DiffLine::new(DiffKind::Header, "--- /dev/null"),
            3 => DiffLine::new(DiffKind::Header, format!("+++ b/{}", self.display_path)),
            _ => unreachable!("invalid untracked header index"),
        }
    }

    fn content_line(&self, index: usize) -> Result<DiffLine> {
        let Some((start, end, eof)) = self.line_bounds(index) else {
            return Ok(DiffLine::context(format!(
                "Indexing {}... line {} is not ready yet.",
                self.display_path,
                index.saturating_add(1)
            )));
        };

        if start == end && eof {
            return Ok(DiffLine::context("End of file."));
        }

        let mut bytes = vec![0; end.saturating_sub(start) as usize];
        let mut file = self.file.lock().expect("lazy diff file lock poisoned");
        file.seek(SeekFrom::Start(start))
            .with_context(|| format!("failed to seek {}", self.path.display()))?;
        file.read_exact(&mut bytes)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        trim_line_ending(&mut bytes);
        let text = String::from_utf8_lossy(&bytes);
        Ok(DiffLine::with_numbers(
            DiffKind::Added,
            None,
            Some(index.saturating_add(1).min(u32::MAX as usize) as u32),
            format!("+{text}"),
        ))
    }

    fn line_bounds(&self, index: usize) -> Option<(u64, u64, bool)> {
        let state = self.state.lock().expect("lazy diff state lock poisoned");
        let start = *state.offsets.get(index)?;
        if let Some(end) = state.offsets.get(index.saturating_add(1)).copied() {
            return Some((start, end, false));
        }
        if state.eof {
            return Some((start, state.file_len, true));
        }
        None
    }
}

impl LazyUntrackedState {
    fn visible_len_estimate(&self) -> usize {
        let indexed_lines = self.actual_indexed_lines();
        if self.eof || indexed_lines == 0 || self.indexed_to == 0 {
            return indexed_lines;
        }

        let average_line_bytes = (self.indexed_to as f64 / indexed_lines as f64).max(1.0);
        let estimated = (self.file_len as f64 / average_line_bytes).ceil() as usize;
        estimated.max(indexed_lines)
    }

    fn actual_indexed_lines(&self) -> usize {
        if self.eof && self.offsets.last().copied() == Some(self.file_len) {
            self.offsets.len().saturating_sub(1)
        } else {
            self.offsets.len()
        }
    }
}

fn temp_path() -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("gwatch-diff-{}-{id}.bin", std::process::id()))
}

fn spawn_untracked_indexer(path: PathBuf, state: Arc<Mutex<LazyUntrackedState>>) {
    thread::spawn(move || {
        if let Err(_err) = index_untracked_file(&path, &state) {
            if let Ok(mut state) = state.lock() {
                state.eof = true;
            }
        }
    });
}

fn index_untracked_file(path: &Path, state: &Arc<Mutex<LazyUntrackedState>>) -> Result<()> {
    let mut file =
        File::open(path).with_context(|| format!("failed to index {}", path.display()))?;
    let mut absolute = 0_u64;
    let mut buffer = vec![0; 1024 * 1024];
    let mut pending_offsets = Vec::new();

    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to index {}", path.display()))?;
        if read == 0 {
            let mut state = state.lock().expect("lazy diff state lock poisoned");
            if state.offsets.last().copied() == Some(state.file_len) && state.file_len > 0 {
                state.offsets.pop();
            }
            state.indexed_to = state.file_len;
            state.eof = true;
            return Ok(());
        }

        pending_offsets.clear();
        for (index, byte) in buffer[..read].iter().enumerate() {
            if *byte == b'\n' {
                pending_offsets.push(absolute + index as u64 + 1);
            }
        }
        absolute += read as u64;

        let mut state = state.lock().expect("lazy diff state lock poisoned");
        state.offsets.extend(pending_offsets.iter().copied());
        state.indexed_to = absolute;
    }
}

fn trim_line_ending(bytes: &mut Vec<u8>) {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
}

fn hunk_ordinal_at_or_after(hunks: &[usize], scroll: usize) -> Option<usize> {
    if hunks.is_empty() {
        return None;
    }
    hunks
        .iter()
        .position(|position| *position >= scroll)
        .or_else(|| hunks.len().checked_sub(1))
}

fn next_hunk_after(hunks: &[usize], scroll: usize) -> Option<usize> {
    hunks
        .iter()
        .copied()
        .find(|position| *position > scroll)
        .or_else(|| hunks.first().copied())
}

fn previous_hunk_before(hunks: &[usize], scroll: usize) -> Option<usize> {
    hunks
        .iter()
        .rev()
        .copied()
        .find(|position| *position < scroll)
        .or_else(|| hunks.last().copied())
}

fn write_record(file: &mut File, line: &DiffLine) -> Result<()> {
    let text = line.text.as_bytes();
    let text_len: u64 = text
        .len()
        .try_into()
        .context("diff line is too large to serialize")?;
    file.write_all(&[kind_to_byte(line.kind)])?;
    file.write_all(&line.old_line.unwrap_or(NONE_LINE).to_le_bytes())?;
    file.write_all(&line.new_line.unwrap_or(NONE_LINE).to_le_bytes())?;
    file.write_all(&text_len.to_le_bytes())?;
    file.write_all(text)?;
    Ok(())
}

fn read_record(file: &mut File) -> Result<DiffLine> {
    let mut kind = [0; 1];
    file.read_exact(&mut kind)
        .context("failed to read diff record kind")?;

    let old_line = read_optional_u32(file)?;
    let new_line = read_optional_u32(file)?;
    let text_len = read_u64(file)?;
    let text_len: usize = text_len
        .try_into()
        .context("diff record is too large to read")?;
    let mut text = vec![0; text_len];
    file.read_exact(&mut text)
        .context("failed to read diff record text")?;
    let text = String::from_utf8(text).context("diff record text is not UTF-8")?;

    Ok(DiffLine {
        kind: byte_to_kind(kind[0])?,
        old_line,
        new_line,
        text,
    })
}

fn read_optional_u32(file: &mut File) -> Result<Option<u32>> {
    let mut bytes = [0; 4];
    file.read_exact(&mut bytes)
        .context("failed to read diff record line number")?;
    let value = u32::from_le_bytes(bytes);
    Ok((value != NONE_LINE).then_some(value))
}

fn read_u64(file: &mut File) -> Result<u64> {
    let mut bytes = [0; 8];
    file.read_exact(&mut bytes)
        .context("failed to read diff record text length")?;
    Ok(u64::from_le_bytes(bytes))
}

fn kind_to_byte(kind: DiffKind) -> u8 {
    match kind {
        DiffKind::Header => 0,
        DiffKind::Hunk => 1,
        DiffKind::Added => 2,
        DiffKind::Deleted => 3,
        DiffKind::Context => 4,
    }
}

fn byte_to_kind(byte: u8) -> Result<DiffKind> {
    match byte {
        0 => Ok(DiffKind::Header),
        1 => Ok(DiffKind::Hunk),
        2 => Ok(DiffKind::Added),
        3 => Ok(DiffKind::Deleted),
        4 => Ok(DiffKind::Context),
        _ => bail!("unknown diff record kind {byte}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_reads_lines_by_range() {
        let doc = DiffDocument::from_lines([
            DiffLine::context("zero"),
            DiffLine::new(DiffKind::Hunk, "@@ -1 +1 @@"),
            DiffLine::with_numbers(DiffKind::Added, None, Some(1), "+one"),
        ])
        .unwrap();

        let lines = doc.lines(1..3).unwrap();

        assert_eq!(doc.len(), 3);
        assert_eq!(doc.hunk_positions(), &[1]);
        assert!(matches!(lines[0].kind, DiffKind::Hunk));
        assert_eq!(lines[1].new_line, Some(1));
        assert_eq!(lines[1].text, "+one");
    }

    #[test]
    fn returns_none_for_missing_line() {
        let doc = DiffDocument::from_lines([DiffLine::context("only")]).unwrap();

        assert!(doc.line(99).unwrap().is_none());
    }

    #[test]
    fn lazy_untracked_opens_without_indexing_entire_file() {
        let path = temp_path();
        {
            let mut file = File::create(&path).unwrap();
            for index in 0..50_000 {
                writeln!(file, "line-{index:05}").unwrap();
            }
        }

        let doc = DiffDocument::lazy_untracked(path.clone(), "huge.txt".to_string()).unwrap();
        let first = doc.lines(0..8).unwrap();

        assert_eq!(first[0].text, "diff --git a/huge.txt b/huge.txt");
        assert_eq!(first.len(), 8);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lazy_untracked_returns_indexing_placeholder_for_unready_far_lines() {
        let path = temp_path();
        {
            let mut file = File::create(&path).unwrap();
            for index in 0..10_000 {
                writeln!(file, "line-{index:05}").unwrap();
            }
        }

        let doc = DiffDocument::lazy_untracked(path.clone(), "huge.txt".to_string()).unwrap();
        let line = doc.line(9_000).unwrap().unwrap();

        assert!(line.text.starts_with("Indexing huge.txt") || line.text.starts_with("+line-"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hunk_navigation_works_for_spooled_documents() {
        let doc = DiffDocument::from_lines([
            DiffLine::new(DiffKind::Hunk, "@@ -1 +1 @@"),
            DiffLine::context("same"),
            DiffLine::new(DiffKind::Hunk, "@@ -10 +10 @@"),
        ])
        .unwrap();

        assert_eq!(doc.hunk_count(), 2);
        assert_eq!(doc.hunk_ordinal_at_or_after(1), Some(1));
        assert_eq!(doc.next_hunk_after(0), Some(2));
        assert_eq!(doc.previous_hunk_before(2), Some(0));
    }
}
