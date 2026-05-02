#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
pub enum DiffKind {
    Header,
    Hunk,
    Added,
    Deleted,
    Context,
}

impl DiffLine {
    pub fn new(kind: DiffKind, text: impl Into<String>) -> Self {
        Self::with_numbers(kind, None, None, text)
    }

    pub fn with_numbers(
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

    pub fn context(text: impl Into<String>) -> Self {
        Self::new(DiffKind::Context, text)
    }
}

pub fn parse_diff_text(text: &str) -> Vec<DiffLine> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
