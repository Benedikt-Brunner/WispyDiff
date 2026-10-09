//! Intra-line ("word") diff: which parts of a changed line differ from the line it replaced,
//! so only those are highlighted (as on GitHub). Background: docs/research/intraline-diff-highlighting.md.

/// Changed parts of a line as `[start, end)` offsets in UTF-16 code units (the frontend's
/// string indices), sorted and disjoint.
pub type Spans = Vec<(u32, u32)>;

/// A pair is highlighted only while at most this share of each line's non-blank text changed,
/// as `numerator / denominator`; above it the line was rewritten, not edited.
const MAX_CHANGED: (u64, u64) = (3, 5);
/// Token-diff table size per pair (after trimming the common ends); larger edits stay plain.
const MAX_CELLS: usize = 40_000;

struct Token<'a> {
    text: &'a str,
    start: u32,
    end: u32,
}

impl Token<'_> {
    fn blank(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }

    /// What a change of this token weighs: blank text counts for nothing.
    fn weight(&self) -> u64 {
        if self.blank() {
            0
        } else {
            u64::from(self.end - self.start)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Words and whitespace runs are one token each; any other character is a token of its own.
fn tokenize(line: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut pos = 0u32;
    let mut chars = line.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let kind = class(c);
        let (mut end, mut units) = (start + c.len_utf8(), c.len_utf16() as u32);
        if kind != Class::Other {
            while let Some(&(i, next)) = chars.peek().filter(|(_, next)| class(*next) == kind) {
                end = i + next.len_utf8();
                units += next.len_utf16() as u32;
                chars.next();
            }
        }
        tokens.push(Token { text: &line[start..end], start: pos, end: pos + units });
        pos += units;
    }
    tokens
}

/// The changed spans of a deleted line and the added line that replaced it, or `None` when
/// they are identical, either was mostly rewritten, or they are too long to compare.
pub fn diff_words(old: &str, new: &str) -> Option<(Spans, Spans)> {
    let (old_tokens, new_tokens) = (tokenize(old), tokenize(new));
    let prefix = old_tokens.iter().zip(&new_tokens).take_while(|(a, b)| a.text == b.text).count();
    let suffix = old_tokens[prefix..]
        .iter()
        .rev()
        .zip(new_tokens[prefix..].iter().rev())
        .take_while(|(a, b)| a.text == b.text)
        .count();
    let old = &old_tokens[prefix..old_tokens.len() - suffix];
    let new = &new_tokens[prefix..new_tokens.len() - suffix];
    if old.is_empty() && new.is_empty() || old.len() * new.len() > MAX_CELLS {
        return None;
    }
    let (old_changed, new_changed) = changed_tokens(old, new);

    let rewritten = |line: &[Token], middle: &[Token], changed: &[bool]| {
        let total: u64 = line.iter().map(Token::weight).sum();
        let changed: u64 = middle.iter().zip(changed).filter(|(_, c)| **c).map(|(t, _)| t.weight()).sum();
        changed * MAX_CHANGED.1 > total * MAX_CHANGED.0
    };
    if rewritten(&old_tokens, old, &old_changed) || rewritten(&new_tokens, new, &new_changed) {
        return None;
    }
    Some((spans(old, &old_changed), spans(new, &new_changed)))
}

/// Marks the tokens outside a longest common subsequence of `old` and `new`.
fn changed_tokens(old: &[Token], new: &[Token]) -> (Vec<bool>, Vec<bool>) {
    let (n, m) = (old.len(), new.len());
    // lcs[i * (m + 1) + j]: common subsequence length of old[i..] and new[j..].
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * (m + 1) + j] = if old[i].text == new[j].text {
                lcs[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
            };
        }
    }
    let (mut old_changed, mut new_changed) = (vec![true; n], vec![true; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i].text == new[j].text {
            old_changed[i] = false;
            new_changed[j] = false;
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (old_changed, new_changed)
}

/// Joins changed tokens into spans, bridging unchanged whitespace between two changes.
fn spans(tokens: &[Token], changed: &[bool]) -> Spans {
    let mut out: Spans = Vec::new();
    let mut bridge = false;
    for (token, &changed) in tokens.iter().zip(changed) {
        if changed {
            match out.last_mut() {
                Some(last) if bridge => last.1 = token.end,
                _ => out.push((token.start, token.end)),
            }
            bridge = true;
        } else {
            bridge &= token.blank();
        }
    }
    out
}
