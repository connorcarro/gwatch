use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

struct SyntaxAssets {
    syntaxes: SyntaxSet,
    theme: Theme,
}

static ASSETS: OnceLock<Arc<SyntaxAssets>> = OnceLock::new();

pub fn highlighted_spans(path: Option<&Path>, text: &str, base_style: Style) -> Vec<Span<'static>> {
    let Some(path) = path else {
        return vec![Span::styled(text.to_string(), base_style)];
    };
    let assets = ASSETS.get_or_init(load_assets);
    let Some(syntax) = assets.syntax_for_path(path) else {
        return vec![Span::styled(text.to_string(), base_style)];
    };

    let mut highlighter = HighlightLines::new(syntax, &assets.theme);
    let highlighted = highlighter
        .highlight_line(text, &assets.syntaxes)
        .unwrap_or_default();

    if highlighted.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    highlighted
        .into_iter()
        .map(|(style, fragment)| Span::styled(fragment.to_string(), merge_style(base_style, style)))
        .collect()
}

pub fn has_syntax_for_path(path: &Path) -> bool {
    ASSETS
        .get_or_init(load_assets)
        .syntax_for_path(path)
        .is_some()
}

fn load_assets() -> Arc<SyntaxAssets> {
    let syntaxes = SyntaxSet::load_defaults_newlines();
    let themes = ThemeSet::load_defaults();
    let theme = themes
        .themes
        .get("base16-ocean.dark")
        .or_else(|| themes.themes.values().next())
        .cloned()
        .unwrap_or_default();
    Arc::new(SyntaxAssets { syntaxes, theme })
}

impl SyntaxAssets {
    fn syntax_for_path(&self, path: &Path) -> Option<&SyntaxReference> {
        self.syntaxes
            .find_syntax_for_file(path)
            .ok()
            .flatten()
            .or_else(|| {
                path.file_name()
                    .and_then(|file_name| file_name.to_str())
                    .and_then(|file_name| {
                        syntax_file_name_aliases(file_name)
                            .iter()
                            .find_map(|alias| self.syntaxes.find_syntax_by_extension(alias))
                    })
            })
            .or_else(|| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(|extension| {
                        syntax_extension_aliases(extension)
                            .iter()
                            .find_map(|alias| self.syntaxes.find_syntax_by_extension(alias))
                    })
            })
    }
}

fn syntax_file_name_aliases(file_name: &str) -> &'static [&'static str] {
    match file_name.to_ascii_lowercase().as_str() {
        "dockerfile" => &["dockerfile", "sh"],
        ".gitignore" | ".gitattributes" => &["gitignore", "sh"],
        _ => &[],
    }
}

fn syntax_extension_aliases(extension: &str) -> &'static [&'static str] {
    match extension.to_ascii_lowercase().as_str() {
        "cjs" | "mjs" | "jsx" => &["js"],
        "cts" | "mts" | "tsx" => &["ts", "js"],
        "bash" | "zsh" | "fish" => &["sh"],
        "dockerignore" | "gitattributes" => &["gitignore"],
        "jsonc" => &["json"],
        "toml" => &["toml", "json"],
        "yaml" | "yml" => &["yaml", "json"],
        _ => &[],
    }
}

fn merge_style(base: Style, syntax: syntect::highlighting::Style) -> Style {
    let mut style = base.fg(Color::Rgb(
        syntax.foreground.r,
        syntax.foreground.g,
        syntax.foreground.b,
    ));

    if syntax.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if syntax.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if syntax.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }

    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_common_language_extensions() {
        for path in [
            "main.rs",
            "app.tsx",
            "component.jsx",
            "server.mjs",
            "index.js",
            "server.py",
            "query.sql",
            "Dockerfile",
            "styles.css",
            "README.md",
            "config.toml",
            "workflow.yml",
        ] {
            assert!(has_syntax_for_path(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn falls_back_to_plain_span_for_unknown_extensions() {
        let spans = highlighted_spans(
            Some(Path::new("data.not-a-real-extension")),
            "plain text",
            Style::default().fg(Color::White),
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "plain text");
    }

    #[test]
    fn highlights_known_extensions_without_losing_text() {
        let spans = highlighted_spans(
            Some(Path::new("main.rs")),
            "fn main() { println!(\"hi\"); }",
            Style::default(),
        );
        let rendered: String = spans.iter().map(|span| span.content.as_ref()).collect();

        assert_eq!(rendered, "fn main() { println!(\"hi\"); }");
        assert!(spans.len() > 1);
    }
}
