//! Name-based code intelligence over the head version of every file a view touches: where a
//! symbol is defined and used, split into changed and unchanged lines.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use serde::Serialize;
use tree_sitter::{Node, Parser};

use crate::highlight::Language;
use crate::highlight_cache::Lines;
use crate::model::{file_slice, row_kind, DiffView, FileSummary};

/// Hits per section; more than this is noise for a reviewer.
const MAX_HITS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    /// Index of the file in the view.
    pub file: u32,
    pub path: String,
    /// Line on the head (new) side.
    pub line: u32,
    /// Column in characters.
    pub col: u32,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usages {
    pub name: String,
    pub definitions: Vec<Hit>,
    /// Uses on lines this view adds or changes.
    pub changed: Vec<Hit>,
    pub unchanged: Vec<Hit>,
}

#[derive(Debug, Clone, Copy)]
struct Occurrence {
    file: u32,
    line: u32,
    /// Byte column.
    col: u32,
    definition: bool,
}

pub struct SymbolIndex {
    names: HashMap<String, Vec<Occurrence>>,
    files: Vec<(String, Lines)>,
    changed: Vec<HashSet<u32>>,
}

impl SymbolIndex {
    /// Indexes the head side of every file in `view`; `lines` provides a file's highlighted
    /// head contents (`None` for deleted/binary files).
    pub fn build(view: &DiffView, lines: impl Fn(&FileSummary) -> Option<Lines> + Sync) -> SymbolIndex {
        let per_file: Vec<(String, Lines, Vec<(String, Occurrence)>, HashSet<u32>)> = view
            .summary
            .files
            .par_iter()
            .enumerate()
            .map(|(index, file)| {
                let content = lines(file).unwrap_or_default();
                let occurrences = extract(index as u32, &file.path, &content);
                let changed = file_slice(view, index).iter().filter(|r| r.k == row_kind::ADDED).filter_map(|r| r.n).collect();
                (file.path.clone(), content, occurrences, changed)
            })
            .collect();

        let mut names: HashMap<String, Vec<Occurrence>> = HashMap::new();
        let mut files = Vec::with_capacity(per_file.len());
        let mut changed = Vec::with_capacity(per_file.len());
        for (path, content, occurrences, changed_lines) in per_file {
            for (name, occurrence) in occurrences {
                names.entry(name).or_default().push(occurrence);
            }
            files.push((path, content));
            changed.push(changed_lines);
        }
        SymbolIndex { names, files, changed }
    }

    pub fn usages(&self, name: &str) -> Usages {
        let name = name.trim_start_matches('$');
        let mut usages = Usages { name: name.to_string(), ..Usages::default() };
        for occurrence in self.names.get(name).into_iter().flatten() {
            let (path, content) = &self.files[occurrence.file as usize];
            let line_text: String = content
                .get(occurrence.line as usize - 1)
                .map(|segs| segs.iter().map(|(_, t)| t.as_str()).collect())
                .unwrap_or_default();
            let col = line_text.get(..occurrence.col as usize).map(|p| p.chars().count()).unwrap_or(0) as u32;
            let hit = Hit {
                file: occurrence.file,
                path: path.clone(),
                line: occurrence.line,
                col,
                text: line_text.trim().chars().take(160).collect(),
            };
            let section = if occurrence.definition {
                &mut usages.definitions
            } else if self.changed[occurrence.file as usize].contains(&occurrence.line) {
                &mut usages.changed
            } else {
                &mut usages.unchanged
            };
            if section.len() < MAX_HITS {
                section.push(hit);
            }
        }
        usages
    }
}

/// Names and positions in one file.
fn extract(file: u32, path: &str, content: &[Vec<crate::highlight::Seg>]) -> Vec<(String, Occurrence)> {
    let lines: Vec<String> = content.iter().map(|segs| segs.iter().map(|(_, t)| t.as_str()).collect()).collect();
    match Language::from_path(path) {
        Some(lang @ (Language::Php | Language::TypeScript | Language::Tsx | Language::JavaScript)) => {
            parsed(file, lang, &lines.join("\n"), 0)
        }
        Some(Language::Vue) => {
            let mut found = words(file, &lines, 0, lines.len(), |_| true);
            if let Some((start, end, lang)) = vue_script(&lines) {
                // The script block parsed properly replaces the word scan of those lines.
                found.retain(|(_, o)| (o.line as usize) <= start || (o.line as usize) > end);
                found.extend(parsed(file, lang, &lines[start..end].join("\n"), start as u32));
            }
            found
        }
        Some(Language::Twig) => twig_words(file, &lines),
        Some(Language::Html) => words(file, &lines, 0, lines.len(), |_| true),
        _ => Vec::new(),
    }
}

const IDENTIFIER_KINDS: &[&str] = &[
    "identifier",
    "property_identifier",
    "shorthand_property_identifier",
    "shorthand_property_identifier_pattern",
    "type_identifier",
    "private_property_identifier",
    "name",
];

fn grammar(lang: Language) -> tree_sitter::Language {
    match lang {
        Language::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => tree_sitter_javascript::LANGUAGE.into(),
    }
}

/// Identifiers from a tree-sitter parse. A name is a definition when it is the `name` field
/// of a declaration-like parent (not of a call, member access, import or re-export).
fn parsed(file: u32, lang: Language, source: &str, line_offset: u32) -> Vec<(String, Occurrence)> {
    let mut parser = Parser::new();
    if parser.set_language(&grammar(lang)).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else { return Vec::new() };
    let mut out = Vec::new();
    let mut stack: Vec<Node> = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if IDENTIFIER_KINDS.contains(&node.kind()) && node.child_count() == 0 {
            let Ok(text) = node.utf8_text(source.as_bytes()) else { continue };
            let definition = node.parent().is_some_and(|parent| {
                let kind = parent.kind();
                !kind.ends_with("expression")
                    && !kind.contains("call")
                    && !kind.contains("import")
                    && !kind.contains("export_specifier")
                    && parent.child_by_field_name("name").is_some_and(|n| n.id() == node.id())
            });
            let position = node.start_position();
            out.push((
                text.to_string(),
                Occurrence { file, line: line_offset + position.row as u32 + 1, col: position.column as u32, definition },
            ));
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    out
}

/// Word-like tokens on lines `start..end` (0-based) that pass `keep`.
fn words(file: u32, lines: &[String], start: usize, end: usize, keep: impl Fn(&str) -> bool) -> Vec<(String, Occurrence)> {
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate().take(end).skip(start) {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                let begin = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = &line[begin..i];
                if word.len() > 1 && keep(word) {
                    out.push((word.to_string(), Occurrence { file, line: index as u32 + 1, col: begin as u32, definition: false }));
                }
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Words inside Twig's `{{ }}` and `{% %}`; `{% set name` / `{% block name` / `{% macro name`
/// define `name`.
fn twig_words(file: u32, lines: &[String]) -> Vec<(String, Occurrence)> {
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let mut rest = line.as_str();
        let mut offset = 0;
        while let Some(open) = rest.find("{{").into_iter().chain(rest.find("{%")).min() {
            let close_token = if rest[open..].starts_with("{{") { "}}" } else { "%}" };
            let body_start = open + 2;
            let body_end = rest[body_start..].find(close_token).map(|p| body_start + p).unwrap_or(rest.len());
            let body = &rest[body_start..body_end];
            let found = words(file, &[body.to_string()], 0, 1, |w| !matches!(w, "in" | "is" | "not" | "and" | "or"));
            let mut defines_next = false;
            for (position, (word, mut occurrence)) in found.into_iter().enumerate() {
                occurrence.line = index as u32 + 1;
                occurrence.col += (offset + body_start) as u32;
                if close_token == "%}" && position == 0 {
                    defines_next = matches!(word.as_str(), "set" | "block" | "macro");
                    continue;
                }
                occurrence.definition = std::mem::take(&mut defines_next);
                out.push((word, occurrence));
            }
            let consumed = (body_end + 2).min(rest.len());
            offset += consumed;
            rest = &rest[consumed..];
        }
    }
    out
}

/// The `<script>` block of a Vue file: (first line, end line) 0-based exclusive, and language.
fn vue_script(lines: &[String]) -> Option<(usize, usize, Language)> {
    let open = lines.iter().position(|l| l.trim_start().starts_with("<script"))?;
    let lang = if lines[open].contains("lang=\"tsx\"") {
        Language::Tsx
    } else if lines[open].contains("lang=\"ts\"") {
        Language::TypeScript
    } else {
        Language::JavaScript
    };
    let close = (open + 1..lines.len()).find(|&i| lines[i].contains("</script>")).unwrap_or(lines.len());
    Some((open + 1, close, lang))
}
