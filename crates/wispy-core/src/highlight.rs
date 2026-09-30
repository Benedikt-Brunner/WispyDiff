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
    Html,
    Css,
    /// Vue single-file component: HTML, with `<script>` / `<style>` blocks in their languages.
    Vue,
    /// Twig template: HTML, with `{{ }}` / `{% %}` / `{# #}` tokenized as Twig.
    Twig,
}

impl Language {
    pub const ALL: [Language; 11] = [
        Language::Php,
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Json,
        Language::Yaml,
        Language::Sql,
        Language::Html,
        Language::Css,
        Language::Vue,
        Language::Twig,
    ];

    /// Languages highlighted by a tree-sitter grammar directly (the others are composites).
    const GRAMMARS: [Language; 9] = [
        Language::Php,
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Json,
        Language::Yaml,
        Language::Sql,
        Language::Html,
        Language::Css,
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
            "html" | "htm" => Language::Html,
            "css" | "scss" | "less" => Language::Css,
            "vue" => Language::Vue,
            "twig" => Language::Twig,
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
            Language::Html => (tree_sitter_html::LANGUAGE.into(), tree_sitter_html::HIGHLIGHTS_QUERY.into(), String::new()),
            Language::Css => (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.into(), String::new()),
            Language::Vue | Language::Twig => unreachable!("composite languages have no grammar of their own"),
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
        Highlighter { configs: Language::GRAMMARS.into_iter().map(|lang| (lang, lang.configuration())).collect() }
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
        let mut lines = match language.filter(|_| highlightable) {
            Some(Language::Vue) => self.highlight_vue(source),
            Some(Language::Twig) => self.highlight_twig(source),
            Some(lang) => self.grammar(lang, source),
            None => plain_lines(source),
        };
        if source.ends_with('\n') {
            lines.pop();
        }
        lines
    }

    fn grammar(&self, lang: Language, source: &str) -> Vec<Vec<Seg>> {
        match self.configs.get(&lang) {
            Some(config) => highlight_with(config, source).unwrap_or_else(|| plain_lines(source)),
            None => plain_lines(source),
        }
    }

    /// HTML for the whole file, then each `<script>` / `<style>` block's lines re-highlighted
    /// in the block's language.
    fn highlight_vue(&self, source: &str) -> Vec<Vec<Seg>> {
        let mut lines = self.grammar(Language::Html, source);
        let raw: Vec<&str> = source.split('\n').collect();
        let mut i = 0;
        while i < raw.len() {
            let opening = raw[i].trim_start();
            let block = if opening.starts_with("<script") {
                let lang = if opening.contains("lang=\"tsx\"") {
                    Language::Tsx
                } else if opening.contains("lang=\"ts\"") {
                    Language::TypeScript
                } else {
                    Language::JavaScript
                };
                Some((lang, "</script>"))
            } else if opening.starts_with("<style") {
                Some((Language::Css, "</style>"))
            } else {
                None
            };
            let Some((lang, closing)) = block.filter(|(_, closing)| !raw[i].contains(closing)) else {
                i += 1;
                continue;
            };
            let start = i + 1;
            let end = (start..raw.len()).find(|&j| raw[j].contains(closing)).unwrap_or(raw.len());
            let inner = raw[start..end].join("\n");
            for (offset, segs) in self.grammar(lang, &inner).into_iter().enumerate() {
                if let Some(line) = lines.get_mut(start + offset) {
                    *line = segs;
                }
            }
            i = end + 1;
        }
        lines
    }

    /// HTML for the whole file with Twig's `{{ }}`, `{% %}` and `{# #}` regions tokenized on top.
    fn highlight_twig(&self, source: &str) -> Vec<Vec<Seg>> {
        let base = self.grammar(Language::Html, source);
        overlay(base, source, &twig_spans(source))
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

const TWIG_KEYWORDS: &[&str] = &[
    "and", "apply", "as", "autoescape", "block", "do", "else", "elseif", "embed", "endapply", "endautoescape", "endblock",
    "endembed", "endfilter", "endfor", "endif", "endmacro", "endset", "endspaceless", "endverbatim", "endwith", "extends",
    "false", "filter", "flush", "for", "from", "if", "import", "in", "include", "is", "macro", "not", "null", "only", "or",
    "set", "sw_extends", "sw_include", "true", "use", "verbatim", "with",
];

/// Byte spans `(start, end, class)` of Twig tokens in `source`.
fn twig_spans(source: &str) -> Vec<(usize, usize, u8)> {
    const KEYWORD: u8 = 1;
    const STRING: u8 = 2;
    const COMMENT: u8 = 3;
    const NUMBER: u8 = 4;
    const FUNCTION: u8 = 6;
    const PROPERTY: u8 = 10;
    const PUNCTUATION: u8 = 13;
    const TAG: u8 = 14;

    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let (open, close) = match (bytes[i], bytes[i + 1]) {
            (b'{', b'{') => ("{{", "}}"),
            (b'{', b'%') => ("{%", "%}"),
            (b'{', b'#') => ("{#", "#}"),
            _ => {
                i += 1;
                continue;
            }
        };
        let body_start = i + 2;
        let end = source[body_start..].find(close).map(|p| body_start + p).unwrap_or(bytes.len());
        let region_end = (end + 2).min(bytes.len());
        if open == "{#" {
            spans.push((i, region_end, COMMENT));
            i = region_end;
            continue;
        }
        spans.push((i, body_start, TAG));
        let (mut j, mut first_word, mut after) = (body_start, open == "{%", 0u8);
        while j < end {
            let c = bytes[j];
            if c == b'\'' || c == b'"' {
                let close_quote = source[j + 1..end].find(c as char).map(|p| j + 1 + p + 1).unwrap_or(end);
                spans.push((j, close_quote, STRING));
                j = close_quote;
            } else if c.is_ascii_digit() {
                let start = j;
                while j < end && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                    j += 1;
                }
                spans.push((start, j, NUMBER));
            } else if c.is_ascii_alphabetic() || c == b'_' {
                let start = j;
                while j < end && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let word = &source[start..j];
                let next = source[j..end].trim_start().as_bytes().first().copied();
                let class = if first_word || TWIG_KEYWORDS.contains(&word) {
                    KEYWORD
                } else if after == b'|' || next == Some(b'(') {
                    FUNCTION
                } else if after == b'.' {
                    PROPERTY
                } else {
                    0
                };
                if class != 0 {
                    spans.push((start, j, class));
                }
                first_word = false;
                after = 0;
            } else {
                if !c.is_ascii_whitespace() {
                    spans.push((j, j + 1, PUNCTUATION));
                    after = c;
                }
                j += 1;
            }
        }
        if end < bytes.len() {
            spans.push((end, region_end, TAG));
        }
        i = region_end;
    }
    spans
}

/// Re-segments `lines` (of `source`) with `spans` (absolute byte ranges) taking precedence.
fn overlay(lines: Vec<Vec<Seg>>, source: &str, spans: &[(usize, usize, u8)]) -> Vec<Vec<Seg>> {
    if spans.is_empty() {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut line_start = 0usize;
    let mut next_span = 0usize;
    for (segs, raw) in lines.into_iter().zip(source.split('\n')) {
        let text = raw.strip_suffix('\r').unwrap_or(raw);
        let line_end = line_start + text.len();
        while next_span < spans.len() && spans[next_span].1 <= line_start {
            next_span += 1;
        }
        let touching: Vec<&(usize, usize, u8)> =
            spans[next_span..].iter().take_while(|s| s.0 < line_end).filter(|s| s.1 > line_start).collect();
        if touching.is_empty() {
            out.push(segs);
        } else {
            let mut classes: Vec<u8> = Vec::with_capacity(text.len());
            for (class, piece) in &segs {
                classes.extend(std::iter::repeat_n(*class, piece.len()));
            }
            classes.resize(text.len(), 0);
            for &&(start, end, class) in &touching {
                for c in classes.iter_mut().take(end.min(line_end) - line_start).skip(start.max(line_start) - line_start) {
                    *c = class;
                }
            }
            let mut merged: Vec<Seg> = Vec::new();
            let mut run_start = 0;
            for k in 1..=text.len() {
                if k == text.len() || (classes[k] != classes[run_start] && text.is_char_boundary(k)) {
                    merged.push((classes[run_start], text[run_start..k].to_string()));
                    run_start = k;
                }
            }
            out.push(merged);
        }
        line_start = line_end + (raw.len() - text.len()) + 1;
    }
    out
}
