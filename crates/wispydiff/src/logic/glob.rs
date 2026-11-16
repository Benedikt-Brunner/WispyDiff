//! Gitignore-style globs: `*` and `?` stay within a path segment, `**` crosses segments,
//! and a pattern without `/` matches a file name anywhere. No negation or character classes.

const SLASH: u16 = b'/' as u16;
const STAR: u16 = b'*' as u16;
const QUESTION: u16 = b'?' as u16;

#[derive(Debug, Clone)]
pub struct GlobMatcher {
    patterns: Vec<Pattern>,
}
#[derive(Debug, Clone)]
struct Pattern {
    tokens: Vec<Token>,
    anywhere: bool,
}
#[derive(Debug, Clone, Copy)]
enum Token {
    Literal(u16),
    One,
    Star,
    Globstar,
    GlobstarSlash,
}

impl GlobMatcher {
    pub fn new(patterns: &[String]) -> Self {
        let patterns = patterns
            .iter()
            .filter_map(|p| {
                let p = super::js_trim(p);
                if p.is_empty() {
                    return None;
                }
                let p = p.strip_prefix('/').unwrap_or(p);
                let anywhere = !p.contains('/');
                let mut chars = p.encode_utf16().peekable();
                let mut tokens = vec![];
                while let Some(c) = chars.next() {
                    tokens.push(match c {
                        STAR if chars.peek() == Some(&STAR) => {
                            chars.next();
                            if chars.peek() == Some(&SLASH) {
                                chars.next();
                                Token::GlobstarSlash
                            } else {
                                Token::Globstar
                            }
                        }
                        STAR => Token::Star,
                        QUESTION => Token::One,
                        c => Token::Literal(c),
                    });
                }
                Some(Pattern { tokens, anywhere })
            })
            .collect();
        Self { patterns }
    }

    pub fn matches(&self, path: &str) -> bool {
        // JavaScript regexps without /u match UTF-16 units (e.g. an emoji takes two ?s).
        let path: Vec<u16> = path.encode_utf16().collect();
        self.patterns.iter().any(|p| {
            // DP avoids exponential backtracking for alternating wildcards/literals.
            let mut row = vec![false; path.len() + 1];
            row[0] = true;
            if p.anywhere {
                for i in 1..=path.len() {
                    row[i] = path[i - 1] == SLASH;
                }
            }
            for token in &p.tokens {
                let mut next = vec![false; row.len()];
                let mut across = false;
                for i in 0..=path.len() {
                    match token {
                        Token::Literal(c) => {
                            if i > 0 {
                                next[i] = row[i - 1] && path[i - 1] == *c;
                            }
                        }
                        Token::One => {
                            if i > 0 {
                                next[i] = row[i - 1] && path[i - 1] != SLASH;
                            }
                        }
                        Token::Star => {
                            next[i] = row[i] || (i > 0 && next[i - 1] && path[i - 1] != SLASH)
                        }
                        // JS regexp `.` excludes line terminators; negated [^/] does not.
                        Token::Globstar => {
                            next[i] = row[i] || (i > 0 && next[i - 1] && dot(path[i - 1]))
                        }
                        Token::GlobstarSlash => {
                            if i > 0 {
                                across = row[i - 1] || (across && dot(path[i - 1]));
                            }
                            next[i] = row[i] || (i > 0 && path[i - 1] == SLASH && across);
                        }
                    }
                }
                row = next;
            }
            if row[path.len()] {
                return true;
            }
            if !p.anywhere {
                let mut suffix = true;
                for i in (0..path.len()).rev() {
                    if row[i] && path[i] == SLASH && suffix {
                        return true;
                    }
                    suffix &= dot(path[i]);
                }
            }
            false
        })
    }
}
fn dot(c: u16) -> bool {
    !matches!(c, 10 | 13 | 0x2028 | 0x2029)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(pattern: &str, path: &str) -> bool {
        GlobMatcher::new(&[pattern.into()]).matches(path)
    }
    #[test]
    fn basename_and_segments() {
        assert!(m(" *.lock ", "a/b/Cargo.lock"));
        assert!(!m("a/*.rs", "a/b/c.rs"));
        assert!(m("a/*.rs", "a/lib.rs/child"));
        assert!(m("/foo", "nested/foo")); // Leading slash is stripped before deciding 'anywhere'.
        assert!(!m("foo", "foo/child"));
        assert!(m("x?.rs", "x界.rs"));
    }
    #[test]
    fn globstars_and_literals() {
        for path in ["a.rs", "x/a.rs", "x/y/a.rs"] {
            assert!(m("**/a.rs", path));
        }
        assert!(m("a/**/b", "a/b"));
        assert!(m("a/**/b", "a/x/y/b"));
        assert!(m("a/**", "a/x/y"));
        assert!(!m("a/**", "a/x\ny"));
        assert!(m("[a].rs", "[a].rs"));
        assert!(!m("[a].rs", "a.rs"));
        assert!(!GlobMatcher::new(&["  ".into()]).matches("anything"));
    }
    #[test]
    fn exact_js_whitespace_utf16_and_dot_semantics() {
        assert!(m("\u{feff}foo\u{feff}", "foo"));
        assert!(!m("\u{85}foo", "foo"));
        assert!(!m("?.rs", "😀.rs"));
        assert!(m("??.rs", "😀.rs"));
        assert!(m("😀.rs", "dir/😀.rs"));
        assert!(m("*.rs", "a\nb.rs"));
        assert!(!m("**.rs", "a\nb.rs"));
        assert!(!m("a/**/b", "a/x\ny/b"));
        assert!(m("a/**/b", "a/x/y/b/child"));
        assert!(!m("foo", "foo\n"));
        assert!(GlobMatcher::new(&["a".into(), "b".into()]).matches("dir/b"));
    }
}
