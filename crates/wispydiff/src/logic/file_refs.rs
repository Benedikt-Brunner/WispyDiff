//! Text-only parts of Markdown.tsx. The UI applies these to inline code (outside pre/link
//! blocks) and non-http/mailto link targets, then resolves files before making links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRef {
    pub path: String,
    pub line: Option<u32>,
    pub end: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPart {
    Text(String),
    Reference { label: String, reference: CodeRef },
}

fn number(s: &mut &str) -> Option<u32> {
    let len = s.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    let value = s[..len].parse().ok()?;
    *s = &s[len..];
    Some(value)
}
/// Parse `path`, `path:42`, `path:42-50`, `path:42:3`, `path#L42C3-L50`.
/// Descending/equal range ends are discarded. Ordinary absolute paths and URLs are refused;
/// Codex checkout paths `.../worktrees/owner/repo/<7..40 hex sha>/...` are made repo-relative.
pub fn parse_ref(text: &str) -> Option<CodeRef> {
    let mut text = super::js_trim(text);
    text = text.strip_prefix("file://").unwrap_or(text);
    // The original regexp prefix is greedy: use the last matching worktrees prefix.
    for (i, _) in text.rmatch_indices("/worktrees/") {
        if text[..i].contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
            continue;
        }
        let mut parts = text[i + 11..].splitn(4, '/');
        if let (Some(owner), Some(repo), Some(sha), Some(rest)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            if !owner.is_empty()
                && !repo.is_empty()
                && (7..=40).contains(&sha.len())
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                text = rest;
                break;
            }
        }
    }
    text = text.strip_prefix("./").unwrap_or(text);
    let cut = text
        .char_indices()
        .find_map(|(i, c)| matches!(c, ':' | '#').then_some(i))
        .unwrap_or(text.len());
    let path = &text[..cut];
    if path.is_empty()
        || path.starts_with('/')
        || path.chars().any(|c| {
            super::js_whitespace(c)
                || matches!(c, ':' | '#' | '`' | '\'' | '"' | '(' | ')' | '<' | '>')
        })
    {
        return None;
    }
    let mut tail = &text[cut..];
    let mut line = None;
    let mut end = None;
    if let Some(rest) = tail.strip_prefix(':') {
        tail = rest;
        line = Some(number(&mut tail)?);
        if let Some(rest) = tail.strip_prefix(['-', '–']) {
            tail = rest.strip_prefix('L').unwrap_or(rest);
            end = Some(number(&mut tail)?);
        }
        if let Some(rest) = tail.strip_prefix(':') {
            tail = rest;
            number(&mut tail)?;
        }
    } else if let Some(rest) = tail.strip_prefix("#L") {
        tail = rest;
        line = Some(number(&mut tail)?);
        if let Some(rest) = tail.strip_prefix('C') {
            tail = rest;
            number(&mut tail)?;
        }
        if let Some(rest) = tail.strip_prefix('-') {
            tail = rest.strip_prefix('L').unwrap_or(rest);
            end = Some(number(&mut tail)?);
        }
    }
    if !tail.is_empty() {
        return None;
    }
    end = end.filter(|e| line.is_some_and(|l| *e > l));
    Some(CodeRef {
        path: path.into(),
        line,
        end,
    })
}
/// Parse a list `path:42, 50-52`, preserving separators and labels. Each line/range becomes
/// its own reference. Returns None for a lone reference or a malformed list.
pub fn parse_line_list(text: &str) -> Option<Vec<RefPart>> {
    let text = super::js_trim(text);
    for (colon, _) in text.match_indices(':') {
        let file = &text[..colon];
        let lines = &text[colon + 1..];
        if file.is_empty()
            || file.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            || !lines.contains(',')
        {
            continue;
        }
        let mut out = vec![];
        let mut tail = lines;
        let mut first = true;
        let mut valid = true;
        loop {
            let part_end = tail.find(',').unwrap_or(tail.len());
            let part = &tail[..part_end];
            let mut rest = part;
            if number(&mut rest).is_none() {
                valid = false;
                break;
            }
            if let Some(r) = rest.strip_prefix(['-', '–']) {
                rest = r;
                if number(&mut rest).is_none() {
                    valid = false;
                    break;
                }
            }
            if !rest.is_empty() {
                valid = false;
                break;
            }
            let reference = parse_ref(&format!("{file}:{part}"));
            let label = if first {
                format!("{file}:{part}")
            } else {
                part.into()
            };
            out.push(match reference {
                Some(reference) => RefPart::Reference { label, reference },
                None => RefPart::Text(label),
            });
            first = false;
            if part_end == tail.len() {
                break;
            }
            let sep_len = 1 + tail[part_end + 1..]
                .chars()
                .take_while(|c| super::js_whitespace(*c))
                .map(char::len_utf8)
                .sum::<usize>();
            out.push(RefPart::Text(tail[part_end..part_end + sep_len].into()));
            tail = &tail[part_end + sep_len..];
        }
        if valid {
            return Some(out);
        }
    }
    None
}
/// Resolve exact paths first, then unique suffixes (including bare names), then a/b diff
/// prefixes. `exists` checks original paths outside the diff at the range head; the UI may
/// enqueue an asynchronous check and return false until its result is cached.
pub fn resolve_ref(
    reference: &CodeRef,
    paths: &[impl AsRef<str>],
    mut exists: impl FnMut(&str) -> bool,
) -> Option<CodeRef> {
    fn known<'a, P: AsRef<str>>(path: &str, paths: &'a [P]) -> Option<&'a str> {
        if let Some(p) = paths.iter().find(|p| p.as_ref() == path) {
            return Some(p.as_ref());
        }
        let suffix = format!("/{path}");
        let mut matches = paths
            .iter()
            .map(AsRef::as_ref)
            .filter(|p| p.ends_with(&suffix));
        let first = matches.next()?;
        matches.all(|p| p == first).then_some(first)
    }
    let path = known(&reference.path, paths).or_else(|| {
        known(
            reference
                .path
                .strip_prefix("a/")
                .or_else(|| reference.path.strip_prefix("b/"))
                .unwrap_or(&reference.path),
            paths,
        )
    });
    if let Some(path) = path {
        return Some(CodeRef {
            path: path.into(),
            ..reference.clone()
        });
    }
    exists(&reference.path).then(|| reference.clone())
}
/// Resolve list references using only the first entry's file check, as Markdown.tsx does.
/// All entries then share that canonical path. A refused first entry leaves the list plain.
pub fn resolve_line_list(
    parts: &[RefPart],
    mut resolve: impl FnMut(&CodeRef) -> Option<CodeRef>,
) -> Option<Vec<RefPart>> {
    let RefPart::Reference { reference, .. } = parts.first()? else {
        return None;
    };
    let path = resolve(reference)?.path;
    Some(
        parts
            .iter()
            .map(|part| match part {
                RefPart::Text(t) => RefPart::Text(t.clone()),
                RefPart::Reference { label, reference } => RefPart::Reference {
                    label: label.clone(),
                    reference: CodeRef {
                        path: path.clone(),
                        ..reference.clone()
                    },
                },
            })
            .collect(),
    )
}
/// `decodeURI`, leaving reserved URI punctuation encoded. Malformed escapes/UTF-8 leave
/// the whole input unchanged (rather than partially decoding it).
pub fn safe_decode(text: &str) -> String {
    fn decode(text: &str) -> Option<String> {
        let mut bytes = vec![];
        let input = text.as_bytes();
        let mut i = 0;
        while i < input.len() {
            if input[i] != b'%' {
                bytes.push(input[i]);
                i += 1;
                continue;
            }
            let hex = text.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let b = u8::from_str_radix(hex, 16).ok()?;
            if b";/?:@&=+$,#".contains(&b) {
                bytes.extend_from_slice(&input[i..i + 3]);
            } else {
                bytes.push(b);
            }
            i += 3;
        }
        String::from_utf8(bytes).ok()
    }
    decode(text).unwrap_or_else(|| text.into())
}
/// HTML escaping for suggestion code and assistant comment cards.
pub fn escape_html(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}
/// Render an already-parsed Markdown soft break. Comments/replies/review summaries use
/// `breaks=true` (GitHub-style line breaks); assistant answers default to false. Apply this
/// at the parser's soft-break node, never to raw source or fenced/inline code newlines.
pub fn soft_break(breaks: bool) -> &'static str {
    if breaks {
        "<br>"
    } else {
        "\n"
    }
}
/// Marked's suggestion code-block override; None delegates to the normal code renderer.
pub fn suggestion_html(text: &str, language: Option<&str>) -> Option<String> {
    let lang = language?;
    let rest = lang.strip_prefix("suggestion")?;
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(format!("<div class=\"suggestion\"><div class=\"suggestion-head\">Suggested change</div><pre><code>{}\n</code></pre></div>",escape_html(text)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn syntax_and_ranges() {
        for (s, l, e) in [
            (" ./src/a.rs ", None, None),
            ("a.rs:42", Some(42), None),
            ("a.rs:42–L50:3", Some(42), Some(50)),
            ("a.rs#L42C3-L50", Some(42), Some(50)),
            ("a.rs:42-20", Some(42), None),
            ("a.rs:0", Some(0), None),
        ] {
            let r = parse_ref(s).unwrap();
            assert_eq!((r.line, r.end), (l, e));
        }
        for s in [
            "",
            "/tmp/a.rs",
            "https://example/a",
            "foo bar",
            "a.rs#42",
            "a.rs:2:3:4",
            "a.rs#L2–L3",
            "a.rs:2-",
            "`x`",
            "x(y)",
        ] {
            assert!(parse_ref(s).is_none(), "{s}");
        }
        assert_eq!(
            parse_ref("file:///data/worktrees/owner/repo/abcdef0/src/a.rs:4")
                .unwrap()
                .path,
            "src/a.rs"
        );
        assert!(parse_ref("/data/worktrees/owner/repo/ABCDEF0/src/a.rs").is_none());
    }
    #[test]
    fn lists_and_resolution() {
        let list = parse_line_list(" a.rs:42, 50-52,\t60–62 ").unwrap();
        assert_eq!(list.len(), 5);
        let RefPart::Reference { label, reference } = &list[2] else {
            panic!()
        };
        assert_eq!(label, "50-52");
        assert_eq!((reference.line, reference.end), (Some(50), Some(52)));
        let paths = ["src/a.rs", "other/b.rs"];
        let resolved = resolve_line_list(&list, |r| resolve_ref(r, &paths, |_| false)).unwrap();
        assert!(resolved
            .iter()
            .all(|p| !matches!(p,RefPart::Reference{reference,..} if reference.path!="src/a.rs")));
        assert!(parse_line_list("a.rs:42").is_none());
        assert!(parse_line_list("a.rs:42, bad").is_none());
        let r = parse_ref("a.rs").unwrap();
        assert!(resolve_ref(&r, &["x/a.rs", "y/a.rs"], |_| false).is_none());
        assert_eq!(
            resolve_ref(&parse_ref("b/src/a.rs").unwrap(), &paths, |_| false)
                .unwrap()
                .path,
            "src/a.rs"
        );
        assert!(
            resolve_ref(&parse_ref("outside/file").unwrap(), &paths, |p| p
                == "outside/file")
            .is_some()
        );
        assert!(resolve_ref(&parse_ref("src/*.rs").unwrap(), &paths, |_| false).is_none());
    }
    #[test]
    fn decoding_breaks_and_suggestions() {
        assert_eq!(safe_decode("src/%C3%A9.rs%3A42"), "src/é.rs%3A42");
        assert_eq!(safe_decode("%61%GG"), "%61%GG");
        assert_eq!(safe_decode("%FF"), "%FF");
        assert_eq!(safe_decode("%+1"), "%+1");
        assert_eq!(soft_break(true), "<br>");
        assert_eq!(soft_break(false), "\n");
        assert_eq!(escape_html("<&\"'"), "&lt;&amp;&quot;'");
        assert!(suggestion_html("<&", Some("suggestion:-1+2"))
            .unwrap()
            .contains("&lt;&amp;\n</code>"));
        assert!(suggestion_html("x", Some("suggestions")).is_none());
        assert!(suggestion_html("x", None).is_none());
    }
    #[test]
    fn line_list_canonical_path_and_refused_first_entry() {
        let parts = parse_line_list("a.rs:5, 9-2").unwrap();
        let mut calls = 0;
        let resolved = resolve_line_list(&parts, |r| {
            calls += 1;
            Some(CodeRef {
                path: "src/a.rs".into(),
                ..r.clone()
            })
        })
        .unwrap();
        assert_eq!(calls, 1);
        let RefPart::Reference { label, reference } = &resolved[2] else {
            panic!()
        };
        assert_eq!(label, "9-2");
        assert_eq!(reference.end, None);
        assert_eq!(reference.path, "src/a.rs");
        assert!(resolve_line_list(&parts, |_| None).is_none());
        assert!(parse_line_list("a.rs:5, 9-L10").is_none());
        assert!(parse_ref("prefix\n/data/worktrees/o/r/abcdef0/a.rs").is_none());
    }
}
