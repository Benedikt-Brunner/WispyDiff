//! Tab / Shift+Tab and Enter inside a comment's ```suggestion block.
//!
//! All offsets are UTF-8 **byte** offsets (including the resulting selection), as required by
//! GPUI text input. Invalid ranges/non-character boundaries return `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Replace text[from, to) with `insert`, then select [start, end).
    pub from: usize,
    pub to: usize,
    pub insert: String,
    pub start: usize,
    pub end: usize,
}
fn valid(text: &str, start: usize, end: usize) -> bool {
    start <= end && end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end)
}
/// Whether lines [first, last] all sit inside one suggestion block, between its fences.
fn in_block(lines: &[&str], first: usize, last: usize) -> bool {
    let mut open = None;
    for (i, line) in lines.iter().enumerate() {
        if open.is_none() && super::js_trim(line) == "```suggestion" {
            open = Some(i);
        } else if open.is_some() && super::js_trim(line) == "```" {
            if open.unwrap() < first && last < i {
                return true;
            }
            open = None;
        }
    }
    false
}
/// Enter inside a suggestion block: replaces [start, end) with a line break and the
/// indentation of the caret's line (up to the caret). None outside a single block's lines.
pub fn newline(text: &str, start: usize, end: usize) -> Option<Edit> {
    if !valid(text, start, end) {
        return None;
    }
    let lines: Vec<_> = text.split('\n').collect();
    let first = text[..start].bytes().filter(|b| *b == b'\n').count();
    let last = text[..end].bytes().filter(|b| *b == b'\n').count();
    if !in_block(&lines, first, last) {
        return None;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line_start..start]
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect();
    let insert = format!("\n{indent}");
    let caret = start + insert.len();
    Some(Edit {
        from: start,
        to: end,
        insert,
        start: caret,
        end: caret,
    })
}
/// Indents (or outdents) by `unit` the lines of a suggestion block touched by [start, end).
/// None outside a single block's lines, or if nothing changes. Multi-line selections leave
/// empty lines empty; a selection ending at a line's start excludes that line.
pub fn reindent(text: &str, start: usize, end: usize, unit: &str, outdent: bool) -> Option<Edit> {
    if !valid(text, start, end) {
        return None;
    }
    let lines: Vec<_> = text.split('\n').collect();
    let mut starts = vec![];
    let mut at = 0;
    for l in &lines {
        starts.push(at);
        at += l.len() + 1;
    }
    let line_of = |pos| starts.partition_point(|at| *at <= pos) - 1;
    let first = line_of(start);
    let end_line = line_of(end);
    let last = if end > start && end == starts[end_line] {
        end_line - 1
    } else {
        end_line
    };
    if !in_block(&lines, first, last) {
        return None;
    }
    let outdent_width = if unit == "\t" {
        4
    } else {
        unit.encode_utf16().count()
    };
    let mut deltas = vec![0isize; lines.len()];
    let mut changed = vec![];
    for i in first..=last {
        let line = lines[i];
        let next = if outdent {
            if let Some(rest) = line.strip_prefix('\t') {
                rest.to_string()
            } else {
                // trimStart in JS accepts Unicode whitespace. Remove whole scalars, never split UTF-8.
                let remove = line
                    .chars()
                    .take_while(|c| super::js_whitespace(*c))
                    .take(outdent_width)
                    .map(char::len_utf8)
                    .sum::<usize>();
                line[remove..].to_string()
            }
        } else if !line.is_empty() || first == last {
            format!("{unit}{line}")
        } else {
            line.to_string()
        };
        deltas[i] = next.len() as isize - line.len() as isize;
        changed.push(next);
    }
    let from = starts[first];
    let to = starts[last] + lines[last].len();
    let insert = changed.join("\n");
    if insert == text[from..to] {
        return None;
    }
    // A position moves with its line's text, staying at its line start in a selection.
    let shifted = |pos| {
        let line = line_of(pos).clamp(first, last);
        let column = pos - starts[line];
        let before: isize = deltas[first..line].iter().sum();
        let moved = if column == 0 && end > start {
            0
        } else {
            (column as isize + deltas[line]).max(0) as usize
        };
        (starts[line] as isize + before) as usize + moved
    };
    let new_end = if end > start && last < end_line {
        (end as isize + insert.len() as isize - (to - from) as isize) as usize
    } else {
        shifted(end)
    };
    Some(Edit {
        from,
        to,
        start: shifted(start),
        end: new_end,
        insert,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newline_caret_and_selection() {
        let t = "```suggestion\n  écho\n```";
        let p = t.find('é').unwrap() + 2;
        let e = newline(t, p, p).unwrap();
        assert_eq!(e.insert, "\n  ");
        assert_eq!(e.start, p + 3);
        assert!(newline(t, 0, 0).is_none());
        assert!(newline(t, p, p + 20).is_none());
        assert!(newline(t, p - 1, p).is_none());
        assert!(newline("```suggestion\nx", 14, 14).is_none());
        let p = t.find('é').unwrap() - 1;
        assert_eq!(newline(t, p, p).unwrap().insert, "\n ");
    }
    #[test]
    fn indent_multiline_preserves_empty_and_selection() {
        let t = "```suggestion\na\n\nb\n```";
        let start = t.find("\na").unwrap() + 1;
        let end = t.rfind("```").unwrap();
        let e = reindent(t, start, end, "  ", false).unwrap();
        assert_eq!(e.insert, "  a\n\n  b");
        assert_eq!(e.start, start);
        assert_eq!(e.end, end + 4);
        let caret = reindent(t, start, start, "\t", false).unwrap();
        assert_eq!(caret.start, start + 1);
    }
    #[test]
    fn outdent_tabs_spaces_and_utf8() {
        let t = "```suggestion\n\t界\n  x\n```";
        let s = t.find('界').unwrap();
        let end = t.find("\n```").unwrap();
        let e = reindent(t, s, end, "\t", true).unwrap();
        assert_eq!(e.insert, "界\nx");
        assert_eq!(e.start, s - 1);
        assert_eq!(e.end, end - 3);
        assert!(reindent("text", 0, 0, "  ", false).is_none());
        let t = "```suggestion\nx\n```";
        assert!(reindent(t, 14, 14, "  ", true).is_none());
    }
}
