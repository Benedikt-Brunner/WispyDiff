//! Change blocks that are provably mechanical — reformatted code or consistent renames — so the
//! reader can skip them (type checking and tests cover those).
//!
//! Both versions of a file are parsed with tree-sitter and turned into token streams: whitespace
//! is dropped, comments become their words, trailing commas before a closing bracket vanish and
//! simple strings forget their quote style. A change block (a run of deleted/added rows) is
//! mechanical when its old and new tokens are equal, or equal up to a one-to-one renaming of
//! identifiers that is
//! - **local**: a variable declared in the enclosing (outermost) function, renamed at every one of
//!   its uses there, to a name that function didn't use before; or
//! - **diff-wide**: no touched file still uses the old name at the head, none used the new one at
//!   the base, and every block that renames it agrees. Callers outside the diff are left to the
//!   type checker.
//!
//! Anything the classifier can't prove (parse errors, unsupported languages, shorthand
//! properties, interpolated strings, ...) stays visible.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, Tree};

use crate::highlight::Language;
use crate::model::{row_kind, Row};

/// A run of a file's unified rows (header = offset 0) that only changes code mechanically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fold {
    pub offset: u32,
    pub len: u32,
    /// What the rows do, e.g. "reformatted" or "renamed `$total` → `$sum`".
    pub reason: String,
}

/// One file of the diff: its language (None: not classified) and its rows.
pub struct FileInput<'a> {
    pub language: Option<Language>,
    pub rows: &'a [Row],
}

/// Both versions' text of a file (when it has them).
struct Texts {
    old: Option<String>,
    new: Option<String>,
}

/// Files larger than this aren't parsed for classification.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// The mechanical folds of each file in `files` (same order). `text(i)` gives file `i`'s old and
/// new text; it's asked only for files that need it.
pub fn classify(files: &[FileInput], text: &(dyn Fn(usize) -> (Option<String>, Option<String>) + Sync)) -> Vec<Vec<Fold>> {
    use rayon::prelude::*;
    let texts: Vec<OnceLock<Texts>> = files.iter().map(|_| OnceLock::new()).collect();
    let text_of = |i: usize| -> &Texts {
        texts[i].get_or_init(|| {
            let (old, new) = text(i);
            Texts { old, new }
        })
    };
    // Only files with a block that may be mechanical are parsed (most changes aren't).
    let parsed: Vec<Option<Parsed>> = files
        .par_iter()
        .enumerate()
        .map(|(i, file)| {
            let wanted = file.language.and_then(grammar).is_some() && maybe_mechanical(file.rows);
            wanted.then(|| Parsed::new(file.language?, text_of(i))).flatten()
        })
        .collect();

    // Pass 1: per file, the blocks whose tokens match up to a renaming.
    let candidates: Vec<Vec<Candidate>> = files
        .par_iter()
        .zip(&parsed)
        .map(|(file, parsed)| parsed.as_ref().map(|p| candidates(file.rows, p)).unwrap_or_default())
        .collect();

    // Pass 2: renames that weren't proven local must hold across the whole diff.
    let mut forward: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut backward: HashMap<&str, HashSet<&str>> = HashMap::new();
    for rename in candidates.iter().flatten().flat_map(|c| &c.renames).filter(|r| !r.local) {
        forward.entry(&rename.old).or_default().insert(&rename.new);
        backward.entry(&rename.new).or_default().insert(&rename.old);
    }
    let mut verdicts: HashMap<(&str, &str), bool> = HashMap::new();
    for (&old, news) in &forward {
        for &new in news {
            let consistent = news.len() == 1 && backward[new].len() == 1;
            let ok = consistent
                && files.iter().zip(&parsed).enumerate().all(|(i, (file, parsed))| gone_everywhere(file, text_of(i), parsed.as_ref(), old, new));
            verdicts.insert((old, new), ok);
        }
    }

    candidates
        .iter()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|c| c.renames.iter().all(|r| r.local || verdicts.get(&(r.old.as_str(), r.new.as_str())) == Some(&true)))
                .map(|c| Fold { offset: c.offset, len: c.len, reason: c.reason() })
                .collect()
        })
        .collect()
}

/// Neither `old` is used at the head nor `new` at the base of `file`.
fn gone_everywhere(file: &FileInput, texts: &Texts, parsed: Option<&Parsed>, old: &str, new: &str) -> bool {
    let bare = |name: &str| name.trim_start_matches('$').to_string();
    let head_uses_old = match (parsed.and_then(|p| p.new.as_ref()), texts.new.as_deref()) {
        (Some(side), _) => side.names.contains(old),
        (None, Some(text)) => contains_word(text, &bare(old)),
        (None, None) => false,
    };
    let base_used_new = match (parsed.and_then(|p| p.old.as_ref()), texts.old.as_deref()) {
        (Some(side), _) => side.names.contains(new),
        (None, Some(text)) => contains_word(text, &bare(new)),
        // The base wasn't loaded (the file only gained lines): it is the head minus those.
        (None, None) => texts.new.as_deref().is_some_and(|text| contains_word(&base_from_head(text, file.rows), &bare(new))),
    };
    !head_uses_old && !base_used_new
}

/// The head lines of a file that weren't added.
fn base_from_head(head: &str, rows: &[Row]) -> String {
    let added: HashSet<u32> = rows.iter().filter(|r| r.k == row_kind::ADDED).filter_map(|r| r.n).collect();
    let kept: Vec<&str> = head.split('\n').enumerate().filter(|(i, _)| !added.contains(&(*i as u32 + 1))).map(|(_, l)| l).collect();
    kept.join("\n")
}

fn contains_word(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

struct Rename {
    old: String,
    new: String,
    local: bool,
}

struct Candidate {
    offset: u32,
    len: u32,
    renames: Vec<Rename>,
    /// Whether the text differs beyond whitespace (after the renames).
    reformatted: bool,
}

impl Candidate {
    fn reason(&self) -> String {
        if self.renames.is_empty() {
            return if self.reformatted { "reformatted".into() } else { "whitespace only".into() };
        }
        let mut names: Vec<String> = self.renames.iter().take(2).map(|r| format!("`{}` → `{}`", r.old, r.new)).collect();
        if self.renames.len() > 2 {
            names.push(format!("+{} more", self.renames.len() - 2));
        }
        let mut reason = format!("renamed {}", names.join(", "));
        if self.reformatted {
            reason.push_str(", reformatted");
        }
        reason
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A variable: may be renamed locally (`$x` in PHP, `identifier` in JS/TS).
    Var,
    /// Any other name (property, method, type, function in PHP): renamed diff-wide only.
    Name,
    /// Must stay as it is (`$this`, object shorthand `{ foo }` whose key is its name).
    Fixed,
    /// A comment word.
    Word,
    /// Anything else, compared by text (punctuation, keywords, numbers, strings).
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// Declared here (a parameter, `let`/`const`, any PHP variable in a function).
    Decl,
    Use,
    /// Refers to something outside the function (`global $x`, a promoted constructor parameter).
    Outer,
}

#[derive(Debug, Clone)]
struct Tok<'a> {
    class: Class,
    text: &'a str,
    start: usize,
    /// First and last line (0-based).
    line: u32,
    end_line: u32,
    /// Byte range of the outermost function around it.
    scope: Option<(usize, usize)>,
    binding: Binding,
}

/// One version of a file as tokens.
struct Side<'a> {
    source: &'a str,
    toks: Vec<Tok<'a>>,
    /// Every name and variable used anywhere in the file.
    names: HashSet<&'a str>,
}

struct Parsed<'a> {
    old: Option<Side<'a>>,
    new: Option<Side<'a>>,
}

impl<'a> Parsed<'a> {
    fn new(lang: Language, texts: &'a Texts) -> Option<Parsed<'a>> {
        let language = grammar(lang)?;
        let side = |source: Option<&'a str>| -> Option<Option<Side<'a>>> {
            let Some(source) = source else { return Some(None) };
            if source.len() > MAX_BYTES {
                return None;
            }
            let mut parser = Parser::new();
            parser.set_language(&language).ok()?;
            let tree = parser.parse(source, None)?;
            if tree.root_node().has_error() {
                return None;
            }
            Some(Some(Side::new(source, &tree, lang)))
        };
        let parsed = Parsed { old: side(texts.old.as_deref())?, new: side(texts.new.as_deref())? };
        (parsed.old.is_some() || parsed.new.is_some()).then_some(parsed)
    }
}

fn grammar(language: Language) -> Option<tree_sitter::Language> {
    Some(match language {
        Language::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::Json => tree_sitter_json::LANGUAGE.into(),
        Language::Css => tree_sitter_css::LANGUAGE.into(),
        // Whitespace is meaningful (YAML, HTML text) or the grammar is a composite.
        Language::Yaml | Language::Html | Language::Vue | Language::Twig | Language::Sql => return None,
    })
}

const FUNCTIONS: &[&str] = &[
    "function_declaration",
    "function_expression",
    "function",
    "generator_function_declaration",
    "generator_function",
    "arrow_function",
    "method_definition",
    "function_definition",
    "method_declaration",
    "anonymous_function",
    "anonymous_function_creation_expression",
];

const STRINGS: &[&str] = &["string", "template_string", "encapsed_string", "heredoc", "nowdoc", "regex", "string_value", "shell_command_expression"];

impl<'a> Side<'a> {
    fn new(source: &'a str, tree: &Tree, language: Language) -> Side<'a> {
        let mut toks = Vec::new();
        collect(tree.root_node(), source, language, &mut toks);
        drop_trailing_commas(&mut toks);
        let names = toks.iter().filter(|t| matches!(t.class, Class::Var | Class::Name | Class::Fixed)).map(|t| t.text).collect();
        Side { source, toks, names }
    }

    /// Tokens touching lines `first..=last` (0-based).
    fn lines(&self, first: u32, last: u32) -> &[Tok<'a>] {
        let from = self.toks.partition_point(|t| t.end_line < first);
        let to = self.toks.partition_point(|t| t.line <= last).max(from);
        &self.toks[from..to]
    }

    /// Tokens inside byte range `scope`.
    fn within(&self, scope: (usize, usize)) -> &[Tok<'a>] {
        let from = self.toks.partition_point(|t| t.start < scope.0);
        let to = self.toks.partition_point(|t| t.start < scope.1).max(from);
        &self.toks[from..to]
    }
}

fn collect<'a>(root: Node, source: &'a str, language: Language, out: &mut Vec<Tok<'a>>) {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        let atomic = node.child_count() == 0 || kind == "variable_name" || kind == "comment" || STRINGS.contains(&kind);
        if !atomic {
            let children: Vec<Node> = node.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
            continue;
        }
        if node.start_byte() == node.end_byte() {
            continue; // automatic semicolons and other zero-width tokens
        }
        let text = &source[node.byte_range()];
        let tok = |class: Class, text: &'a str| Tok {
            class,
            text,
            start: node.start_byte(),
            line: node.start_position().row as u32,
            end_line: node.end_position().row as u32,
            scope: None,
            binding: Binding::Use,
        };
        if kind == "comment" {
            let mut offset = 0;
            for word in text.split_whitespace() {
                // Comment markers (`//`, `/**`, `*`, `#`) are layout, not content.
                if word.chars().all(|c| matches!(c, '/' | '*' | '#')) {
                    continue;
                }
                let at = text[offset..].find(word).map_or(offset, |i| offset + i);
                offset = at + word.len();
                let line = node.start_position().row as u32 + text[..at].matches('\n').count() as u32;
                out.push(Tok { line, end_line: line, ..tok(Class::Word, word) });
            }
            continue;
        }
        if STRINGS.contains(&kind) {
            out.push(tok(Class::Other, simple_string(text, language).unwrap_or(text)));
            continue;
        }
        let class = match (language, kind) {
            (Language::Php, "variable_name") if text == "$this" => Class::Fixed,
            (Language::Php, "variable_name") => Class::Var,
            (Language::Php, "name") => Class::Name,
            (_, "identifier") if language != Language::Php => Class::Var,
            (_, "shorthand_property_identifier" | "shorthand_property_identifier_pattern") => Class::Fixed,
            (_, "property_identifier" | "type_identifier" | "private_property_identifier" | "statement_identifier") => Class::Name,
            _ => Class::Other,
        };
        let mut t = tok(class, text);
        if class == Class::Var {
            t.scope = outermost_function(node).map(|f| (f.start_byte(), f.end_byte()));
            t.binding = binding(node, language);
        }
        out.push(t);
    }
}

/// A string's content when its quotes don't change what it means (no escapes, no
/// interpolation, no quotes inside), so `'a'` and `"a"` compare equal.
fn simple_string(text: &str, language: Language) -> Option<&str> {
    let quote = text.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') || text.len() < 2 || !text.ends_with(quote) {
        return None;
    }
    if quote == '`' && language == Language::Php {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    let plain = !inner.contains(['\\', '\'', '"', '`', '$', '\n']);
    plain.then_some(inner)
}

fn outermost_function(node: Node) -> Option<Node> {
    let mut found = None;
    let mut current = node.parent();
    while let Some(n) = current {
        if FUNCTIONS.contains(&n.kind()) {
            found = Some(n);
        }
        current = n.parent();
    }
    found
}

fn binding(node: Node, language: Language) -> Binding {
    let parent = node.parent();
    let parent_kind = parent.map_or("", |p| p.kind());
    if language == Language::Php {
        let mut current = node.parent();
        while let Some(n) = current {
            match n.kind() {
                "global_declaration" | "property_promotion_parameter" => return Binding::Outer,
                k if FUNCTIONS.contains(&k) => break,
                _ => current = n.parent(),
            }
        }
        // Any variable in a function is local to it (closures included, as their function is
        // inside the outermost one).
        return Binding::Decl;
    }
    let is_field = |field: &str| parent.and_then(|p| p.child_by_field_name(field)) == Some(node);
    match parent_kind {
        "variable_declarator" | "function_declaration" | "generator_function_declaration" | "class_declaration" if is_field("name") => {
            Binding::Decl
        }
        "required_parameter" | "optional_parameter" if is_field("pattern") => Binding::Decl,
        "formal_parameters" | "rest_pattern" | "array_pattern" | "object_pattern" | "catch_clause" => Binding::Decl,
        "assignment_pattern" if is_field("left") => Binding::Decl,
        "pair_pattern" if is_field("value") => Binding::Decl,
        "arrow_function" if is_field("parameter") => Binding::Decl,
        _ => Binding::Use,
    }
}

/// Drops commas whose next non-comment token closes a bracket (`[a, b,]` = `[a, b]`).
fn drop_trailing_commas(toks: &mut Vec<Tok>) {
    let mut keep = vec![true; toks.len()];
    for i in 0..toks.len() {
        if toks[i].class != Class::Other || toks[i].text != "," {
            continue;
        }
        let next = toks[i + 1..].iter().find(|t| t.class != Class::Word);
        if next.is_some_and(|t| t.class == Class::Other && matches!(t.text, ")" | "]" | "}")) {
            keep[i] = false;
        }
    }
    let mut flags = keep.into_iter();
    toks.retain(|_| flags.next().unwrap_or(true));
}

/// Whether some change block (or a hunk's blocks together) keeps its punctuation — a cheap
/// necessary condition for being mechanical: reformatting and renaming only move or drop commas,
/// quotes and comment markers.
fn maybe_mechanical(rows: &[Row]) -> bool {
    fn add(sig: &mut String, row: &Row) {
        for (_, piece) in &row.s {
            sig.extend(piece.chars().filter(|c| !c.is_whitespace() && !c.is_alphanumeric() && !matches!(c, '_' | ',' | '\'' | '"' | '`' | '/' | '*' | '#')));
        }
    }
    let (mut hunk_old, mut hunk_new, mut old, mut new) = (String::new(), String::new(), String::new(), String::new());
    let mut in_block = false;
    for row in rows.iter().chain(std::iter::once(&Row { k: row_kind::HUNK, f: 0, o: None, n: None, s: Vec::new(), a: None, h: Vec::new(), l: None, w: Vec::new() })) {
        match row.k {
            row_kind::DELETED => {
                add(&mut old, row);
                in_block = true;
            }
            row_kind::ADDED => {
                add(&mut new, row);
                in_block = true;
            }
            kind => {
                if in_block {
                    if old == new {
                        return true;
                    }
                    hunk_old.push_str(&old);
                    hunk_new.push_str(&new);
                    old.clear();
                    new.clear();
                    in_block = false;
                }
                if kind == row_kind::HUNK {
                    if !hunk_old.is_empty() && hunk_old == hunk_new {
                        return true;
                    }
                    hunk_old.clear();
                    hunk_new.clear();
                }
            }
        }
    }
    false
}

/// A run of changed rows (and, when merged, the context rows between them).
#[derive(Clone, Copy)]
struct Block {
    offset: u32,
    len: u32,
    /// 1-based old / new line ranges, inclusive; None when the side has no lines.
    old: Option<(u32, u32)>,
    new: Option<(u32, u32)>,
}

fn blocks(rows: &[Row]) -> Vec<Vec<Block>> {
    let mut hunks: Vec<Vec<Block>> = Vec::new();
    let mut current: Option<Block> = None;
    let widen = |range: &mut Option<(u32, u32)>, line: Option<u32>| {
        if let Some(line) = line {
            *range = Some(range.map_or((line, line), |(a, b)| (a.min(line), b.max(line))));
        }
    };
    for (offset, row) in rows.iter().enumerate() {
        let changed = row.k == row_kind::ADDED || row.k == row_kind::DELETED;
        if changed {
            let block = current.get_or_insert(Block { offset: offset as u32, len: 0, old: None, new: None });
            block.len += 1;
            if row.k == row_kind::DELETED {
                widen(&mut block.old, row.o);
            } else {
                widen(&mut block.new, row.n);
            }
            continue;
        }
        if let (Some(block), Some(hunk)) = (current.take(), hunks.last_mut()) {
            hunk.push(block);
        }
        if row.k == row_kind::HUNK {
            hunks.push(Vec::new());
        }
    }
    if let (Some(block), Some(hunk)) = (current, hunks.last_mut()) {
        hunk.push(block);
    }
    hunks
}

fn candidates(rows: &[Row], parsed: &Parsed) -> Vec<Candidate> {
    let mut out = Vec::new();
    for hunk in blocks(rows) {
        let mut failed = Vec::new();
        for block in &hunk {
            match compare(block, parsed) {
                Some(c) => out.push(c),
                None => failed.push(*block),
            }
        }
        // A reformatting git split around a common line (`}`, `)`): try the failed blocks of
        // the hunk together, context rows between them included.
        if failed.len() > 1 {
            let (first, last) = (failed[0], failed[failed.len() - 1]);
            let span = |a: Option<(u32, u32)>, b: Option<(u32, u32)>| match (a, b) {
                (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
                (a, b) => a.or(b),
            };
            let merged = Block {
                offset: first.offset,
                len: last.offset + last.len - first.offset,
                old: failed.iter().fold(None, |acc, b| span(acc, b.old)),
                new: failed.iter().fold(None, |acc, b| span(acc, b.new)),
            };
            let covered: HashSet<u32> = out.iter().map(|c| c.offset).collect();
            let inside = |c: &u32| *c >= merged.offset && *c < merged.offset + merged.len;
            // Only when the merged span doesn't swallow a block that already matched alone.
            if !covered.iter().any(inside) {
                if let Some(c) = compare(&merged, parsed) {
                    out.push(c);
                }
            }
        }
    }
    out.sort_by_key(|c| c.offset);
    out
}

fn compare(block: &Block, parsed: &Parsed) -> Option<Candidate> {
    fn toks<'s>(side: &'s Option<Side<'_>>, range: Option<(u32, u32)>) -> Option<&'s [Tok<'s>]> {
        match range {
            None => Some(&[]),
            Some((a, b)) => Some(side.as_ref()?.lines(a - 1, b - 1)),
        }
    }
    let old = toks(&parsed.old, block.old)?;
    let new = toks(&parsed.new, block.new)?;
    if old.len() != new.len() {
        return None;
    }

    let mut forward: HashMap<&str, &str> = HashMap::new();
    let mut backward: HashMap<&str, &str> = HashMap::new();
    let mut pairs: Vec<(&Tok, &Tok)> = Vec::new();
    let mut words: Vec<(&str, &str)> = Vec::new();
    for (a, b) in old.iter().zip(new) {
        if a.class != b.class {
            return None;
        }
        match a.class {
            Class::Other | Class::Fixed => {
                if a.text != b.text {
                    return None;
                }
            }
            Class::Word => {
                if a.text != b.text {
                    words.push((a.text, b.text));
                }
            }
            Class::Var | Class::Name => {
                if *forward.entry(a.text).or_insert(b.text) != b.text || *backward.entry(b.text).or_insert(a.text) != a.text {
                    return None;
                }
                if a.text != b.text {
                    pairs.push((a, b));
                }
            }
        }
    }
    // Comment words may only change along with the renames (`@param $old` → `@param $new`).
    for (a, b) in &words {
        let renamed = forward.iter().any(|(old, new)| old != new && a.replace(old, new) == *b);
        if !renamed {
            return None;
        }
    }

    let mut renames: Vec<Rename> = Vec::new();
    for (a, b) in pairs {
        if renames.iter().any(|r| r.old == a.text) {
            continue;
        }
        let local = a.class == Class::Var && local_rename(parsed, a, b);
        renames.push(Rename { old: a.text.to_string(), new: b.text.to_string(), local });
    }

    let text = |side: &Option<Side>, range: Option<(u32, u32)>| -> String {
        let (Some(side), Some((a, b))) = (side, range) else { return String::new() };
        side.source.split('\n').skip(a as usize - 1).take((b - a + 1) as usize).flat_map(|l| l.split_whitespace()).collect()
    };
    let mut before = text(&parsed.old, block.old);
    for rename in &renames {
        before = before.replace(&rename.old, &rename.new);
    }
    let reformatted = before != text(&parsed.new, block.new);
    Some(Candidate { offset: block.offset, len: block.len, renames, reformatted })
}

/// `a` → `b` renames a variable declared in its function, at all of its uses there, to a name the
/// function didn't use.
fn local_rename(parsed: &Parsed, a: &Tok, b: &Tok) -> bool {
    let (Some(old), Some(new), Some(old_scope), Some(new_scope)) = (&parsed.old, &parsed.new, a.scope, b.scope) else {
        return false;
    };
    let before = old.within(old_scope);
    let after = new.within(new_scope);
    let uses = |toks: &[Tok], name: &str| toks.iter().filter(|t| t.class == Class::Var && t.text == name).count();
    let named = |toks: &[Tok], name: &str| toks.iter().any(|t| matches!(t.class, Class::Var | Class::Name | Class::Fixed) && t.text == name);
    let declared = before.iter().any(|t| t.class == Class::Var && t.text == a.text && t.binding == Binding::Decl);
    let outer = before.iter().any(|t| t.class == Class::Var && t.text == a.text && t.binding == Binding::Outer);
    declared && !outer && !named(before, b.text) && !named(after, a.text) && uses(before, a.text) == uses(after, b.text)
}
