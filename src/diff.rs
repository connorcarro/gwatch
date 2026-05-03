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
    let mut parser = DiffParser::new();
    let mut lines = Vec::new();

    for line in text.lines() {
        lines.push(parser.parse_line(line));
    }

    lines
}

pub struct DiffParser {
    old_line: u32,
    new_line: u32,
}

impl DiffParser {
    pub fn new() -> Self {
        Self {
            old_line: 0,
            new_line: 0,
        }
    }

    pub fn parse_line(&mut self, line: &str) -> DiffLine {
        if line.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_starts(line) {
                self.old_line = old_start;
                self.new_line = new_start;
            }
            DiffLine::new(DiffKind::Hunk, line)
        } else if is_diff_header(line) {
            DiffLine::new(DiffKind::Header, line)
        } else if line.starts_with('+') {
            let parsed = DiffLine::with_numbers(DiffKind::Added, None, Some(self.new_line), line);
            self.new_line = self.new_line.saturating_add(1);
            parsed
        } else if line.starts_with('-') {
            let parsed = DiffLine::with_numbers(DiffKind::Deleted, Some(self.old_line), None, line);
            self.old_line = self.old_line.saturating_add(1);
            parsed
        } else {
            let parsed = DiffLine::with_numbers(
                DiffKind::Context,
                Some(self.old_line),
                Some(self.new_line),
                line,
            );
            self.old_line = self.old_line.saturating_add(1);
            self.new_line = self.new_line.saturating_add(1);
            parsed
        }
    }
}

impl Default for DiffParser {
    fn default() -> Self {
        Self::new()
    }
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

    #[test]
    fn classifies_extended_git_headers() {
        let diff = parse_diff_text(
            "\
diff --git a/old.rs b/new.rs
similarity index 92%
rename from old.rs
rename to new.rs
index 1111111..2222222 100644
--- a/old.rs
+++ b/new.rs
@@ -1 +1 @@
-old
+new",
        );

        assert!(
            diff[..7]
                .iter()
                .all(|line| matches!(line.kind, DiffKind::Header | DiffKind::Hunk))
        );
        let hunk = diff
            .iter()
            .position(|line| matches!(line.kind, DiffKind::Hunk))
            .unwrap();
        let deleted = diff
            .iter()
            .position(|line| matches!(line.kind, DiffKind::Deleted))
            .unwrap();
        let added = diff
            .iter()
            .position(|line| matches!(line.kind, DiffKind::Added))
            .unwrap();

        assert!(hunk < deleted);
        assert!(deleted < added);
    }

    #[test]
    fn parses_single_line_hunk_without_explicit_counts() {
        let starts = parse_hunk_starts("@@ -42 +99 @@ fn main()");

        assert_eq!(starts, Some((42, 99)));
    }

    #[test]
    fn parser_can_incrementally_parse_without_full_diff_string() {
        let mut parser = DiffParser::new();

        let hunk = parser.parse_line("@@ -100 +200 @@");
        let deleted = parser.parse_line("-old");
        let added = parser.parse_line("+new");
        let context = parser.parse_line("same");

        assert!(matches!(hunk.kind, DiffKind::Hunk));
        assert_eq!(deleted.old_line, Some(100));
        assert_eq!(deleted.new_line, None);
        assert_eq!(added.old_line, None);
        assert_eq!(added.new_line, Some(200));
        assert_eq!(context.old_line, Some(101));
        assert_eq!(context.new_line, Some(201));
    }
}
