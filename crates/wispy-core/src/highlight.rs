//! Whole-file tree-sitter highlighting, split into per-line segments.

use std::cell::RefCell;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent};

/// A run of text with one token class (index into [`TOKEN_CLASSES`], 0 = plain).
pub type Seg = (u8, String);

/// Token classes the frontend styles. Index 0 is plain text.
pub const TOKEN_CLASSES: &[&str] = &[
    "",
    "keyword",
    "string",
    "comment",
    "number",
    "boolean",
    "function",
    "type",
    "constant",
    "constructor",
    "property",
    "variable",
    "operator",
    "punctuation",
    "tag",
    "attribute",
    "escape",
    "embedded",
    "label",
    "module",
];

/// Files above these limits are shown without highlighting.
const MAX_HIGHLIGHT_BYTES: usize = 4 * 1024 * 1024;
const MAX_HIGHLIGHT_LINE: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Php,
    TypeScript,
    Tsx,
    JavaScript,
    Json,
    Yaml,
    Sql,
}

impl Language {
    pub const ALL: [Language; 7] = [
        Language::Php,
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Json,
        Language::Yaml,
        Language::Sql,
    ];

    pub fn from_path(path: &str) -> Option<Language> {
        let file_name = path.rsplit('/').next().unwrap_or(path);
        let ext = file_name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase())?;
        Some(match ext.as_str() {
            "php" | "phtml" => Language::Php,
            "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "json" | "jsonc" => Language::Json,
            "yml" | "yaml" | "neon" => Language::Yaml,
            "sql" => Language::Sql,
            _ => return None,
        })
    }

    fn configuration(self) -> HighlightConfiguration {
        let (language, highlights, locals): (tree_sitter::Language, String, String) = match self {
            Language::Php => (tree_sitter_php::LANGUAGE_PHP.into(), tree_sitter_php::HIGHLIGHTS_QUERY.into(), String::new()),
            Language::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                [tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n"),
                [tree_sitter_typescript::LOCALS_QUERY, tree_sitter_javascript::LOCALS_QUERY].join("\n"),
            ),
            Language::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                [
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                ]
                .join("\n"),
                [tree_sitter_typescript::LOCALS_QUERY, tree_sitter_javascript::LOCALS_QUERY].join("\n"),
            ),
            Language::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                [tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY].join("\n"),
                tree_sitter_javascript::LOCALS_QUERY.into(),
            ),
            Language::Json => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.into(), String::new()),
            Language::Yaml => (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.into(), String::new()),
            Language::Sql => (tree_sitter_sequel::LANGUAGE.into(), tree_sitter_sequel::HIGHLIGHTS_QUERY.into(), String::new()),
        };
        let mut config = HighlightConfiguration::new(language, format!("{self:?}"), &highlights, "", &locals)
            .unwrap_or_else(|e| panic!("built-in {self:?} highlight query is invalid: {e}"));
        config.configure(&TOKEN_CLASSES[1..]);
        config
    }
}

thread_local! {
    static TS_HIGHLIGHTER: RefCell<tree_sitter_highlight::Highlighter> =
        RefCell::new(tree_sitter_highlight::Highlighter::new());
}

pub struct Highlighter {
    configs: HashMap<Language, HighlightConfiguration>,
}

impl Highlighter {
    /// Compiles the queries for every supported language (~tens of ms; do it once).
    pub fn new() -> Self {
        Highlighter { configs: Language::ALL.into_iter().map(|lang| (lang, lang.configuration())).collect() }
    }

    /// Returns one segment list per line of `source` (lines split on `\n`, `\r` stripped; a
    /// final newline doesn't start another line, so the result has exactly one entry per line).
    /// Falls back to plain text when the file is too large or parsing fails.
    pub fn highlight(&self, language: Option<Language>, source: &str) -> Vec<Vec<Seg>> {
        if source.is_empty() {
            return Vec::new();
        }
        let highlightable = source.len() <= MAX_HIGHLIGHT_BYTES
            && source.split('\n').all(|line| line.len() <= MAX_HIGHLIGHT_LINE);
        let mut lines = match language.and_then(|lang| self.configs.get(&lang)).filter(|_| highlightable) {
            Some(config) => highlight_with(config, source).unwrap_or_else(|| plain_lines(source)),
            None => plain_lines(source),
        };
        if source.ends_with('\n') {
            lines.pop();
        }
        lines
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn plain_lines(source: &str) -> Vec<Vec<Seg>> {
    source
        .split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                Vec::new()
            } else {
                vec![(0, line.to_string())]
            }
        })
        .collect()
}

fn highlight_with(config: &HighlightConfiguration, source: &str) -> Option<Vec<Vec<Seg>>> {
    TS_HIGHLIGHTER.with(|cell| {
        let mut highlighter = cell.borrow_mut();
        let events = highlighter.highlight(config, source.as_bytes(), None, None, |_| None).ok()?;
        let mut lines: Vec<Vec<Seg>> = vec![Vec::new()];
        let mut stack: Vec<u8> = Vec::new();
        for event in events {
            match event.ok()? {
                HighlightEvent::HighlightStart(h) => stack.push((h.0 + 1) as u8),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let class = stack.last().copied().unwrap_or(0);
                    let text = source.get(start..end)?;
                    for (i, piece) in text.split('\n').enumerate() {
                        if i > 0 {
                            lines.push(Vec::new());
                        }
                        let piece = piece.strip_suffix('\r').unwrap_or(piece);
                        if piece.is_empty() {
                            continue;
                        }
                        let line = lines.last_mut().expect("at least one line");
                        match line.last_mut() {
                            Some((last_class, last_text)) if *last_class == class => last_text.push_str(piece),
                            _ => line.push((class, piece.to_string())),
                        }
                    }
                }
            }
        }
        Some(lines)
    })
}
