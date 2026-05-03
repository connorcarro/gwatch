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
        "containerfile" => &["dockerfile", "sh"],
        "makefile" | "gnumakefile" | "rakefile" | "gemfile" | "podfile" => &["make", "rb", "sh"],
        "justfile" | "taskfile" | "brewfile" | "procfile" => &["make", "sh"],
        "cmakelists.txt" => &["cmake", "make"],
        ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | ".zprofile" | ".envrc" => &["sh"],
        ".gitignore" | ".gitattributes" | ".gitmodules" => &["gitignore", "sh"],
        ".editorconfig" => &["ini", "conf", "sh"],
        ".env" | ".env.local" | ".env.example" | ".env.sample" => &["sh"],
        "license" | "copying" | "notice" => &["txt"],
        _ => &[],
    }
}

fn syntax_extension_aliases(extension: &str) -> &'static [&'static str] {
    match extension.to_ascii_lowercase().as_str() {
        // Rust and systems languages.
        "rs" => &["rs", "rust"],
        "c" | "h" => &["c"],
        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "ipp" => &["cpp", "c"],
        "m" | "mm" => &["objc", "c"],
        "zig" => &["zig", "c"],
        "odin" | "v" => &["c"],

        // Web and frontend.
        "cjs" | "mjs" | "jsx" => &["js"],
        "cts" | "mts" | "tsx" => &["ts", "js"],
        "vue" | "svelte" | "astro" => &["html", "js"],
        "htm" | "xhtml" => &["html"],
        "scss" | "sass" | "less" => &["css"],
        "pcss" | "postcss" => &["css"],
        "svg" => &["xml", "html"],
        "graphql" | "gql" => &["graphql", "js"],

        // Scripting languages and shells.
        "bash" | "zsh" | "fish" => &["sh"],
        "ps1" | "psm1" | "psd1" => &["ps1", "sh"],
        "bat" | "cmd" => &["bat", "sh"],
        "pyw" | "pyi" | "bzl" | "bazel" | "star" => &["py"],
        "rbw" | "gemspec" => &["rb"],
        "pl" | "pm" | "t" => &["pl", "perl"],
        "lua" | "luau" => &["lua"],
        "r" | "rmd" => &["r"],

        // JVM, .NET, and backend languages.
        "java" | "gradle" => &["java"],
        "kt" | "kts" => &["kotlin", "java"],
        "scala" | "sc" => &["scala", "java"],
        "groovy" | "gvy" => &["groovy", "java"],
        "cs" | "csx" => &["cs", "csharp", "java"],
        "fs" | "fsi" | "fsx" => &["fsharp", "cs"],
        "go" | "mod" | "sum" => &["go"],
        "php" | "phtml" | "php3" | "php4" | "php5" | "phps" => &["php", "html"],
        "ex" | "exs" => &["elixir", "rb"],
        "erl" | "hrl" => &["erlang"],
        "clj" | "cljs" | "cljc" | "edn" => &["clojure", "lisp"],
        "hs" | "lhs" => &["haskell"],
        "ml" | "mli" | "fsproj" | "csproj" | "vbproj" => &["xml"],

        // Mobile, Apple, and UI languages.
        "swift" => &["swift", "c"],
        "dart" => &["dart", "java"],
        "xaml" | "storyboard" | "xib" => &["xml"],

        // Data, config, and markup.
        "dockerignore" | "gitattributes" => &["gitignore"],
        "jsonc" => &["json"],
        "json5" | "webmanifest" | "ipynb" => &["json"],
        "toml" => &["toml", "json"],
        "yaml" | "yml" => &["yaml", "json"],
        "lock" => &["json", "yaml", "txt"],
        "xml" | "xsd" | "xsl" | "xslt" | "plist" | "resx" | "rss" | "atom" => &["xml"],
        "ini" | "cfg" | "conf" | "cnf" | "properties" | "prefs" => &["ini", "conf", "sh"],
        "csv" | "tsv" => &["csv", "txt"],
        "sql" | "psql" | "mysql" | "pgsql" | "ddl" | "dml" => &["sql"],
        "md" | "mdx" | "markdown" | "mkd" => &["md", "markdown"],
        "rst" | "adoc" | "asciidoc" | "tex" | "bib" => &["tex", "txt"],

        // Build, infrastructure, and deployment.
        "cmake" => &["cmake", "make"],
        "mk" | "mak" => &["make", "sh"],
        "ninja" => &["make", "sh"],
        "tf" | "tfvars" | "hcl" => &["terraform", "ruby", "json"],
        "nomad" => &["terraform", "ruby", "json"],
        "proto" | "thrift" => &["protobuf", "java"],
        "sol" => &["js", "c"],

        // Editor, package, and repo metadata.
        "npmrc" | "yarnrc" | "prettierrc" | "eslintrc" | "babelrc" | "browserslistrc" => {
            &["json", "yaml", "ini"]
        }
        "ignore" => &["gitignore", "sh"],
        "diff" | "patch" => &["diff"],

        // Common logs and text-ish files.
        "log" | "txt" | "text" | "out" | "err" => &["txt"],
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
            "lib.cpp",
            "app.go",
            "server.java",
            "Program.cs",
            "App.swift",
            "app.tsx",
            "component.jsx",
            "server.mjs",
            "index.js",
            "widget.vue",
            "page.svelte",
            "server.py",
            "script.ps1",
            "build.gradle",
            "query.sql",
            "Dockerfile",
            "Makefile",
            ".github/workflows/ci.yml",
            "main.tf",
            "schema.proto",
            "styles.css",
            "theme.scss",
            "README.md",
            "docs.mdx",
            "config.toml",
            "workflow.yml",
            "package-lock.json",
            ".env",
            ".gitignore",
        ] {
            assert!(has_syntax_for_path(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn recognizes_wide_alias_set_without_exact_bundled_grammar() {
        let paths = [
            "component.astro",
            "manifest.webmanifest",
            "notebook.ipynb",
            "settings.jsonc",
            "compose.yaml",
            "values.tfvars",
            "build.kts",
            "module.mts",
            "module.cts",
            "app.pyi",
            "Gemfile",
            "Containerfile",
            "CMakeLists.txt",
            "config.properties",
            "diagram.svg",
            "changes.patch",
        ];

        for path in paths {
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
