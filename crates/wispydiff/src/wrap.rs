//! Character-based wrapping of tab-expanded, styled monospace text.

use gpui_kit::{SharedString, TextRun};

/// Splits a shaped-text input into visual lines of at most `columns` monospace columns each
/// (wide characters take two; one never starts a line it doesn't fit on unless the line is empty).
///
/// `runs` must cover `text` in UTF-8 bytes. A zero column limit disables wrapping.
/// Breaks occur between Unicode scalar values, so combining sequences and other grapheme
/// clusters may be split. Tabs should already be expanded by `text::code_line`.
pub fn wrap_columns(
    text: &SharedString,
    runs: &[TextRun],
    columns: usize,
) -> Vec<(SharedString, Vec<TextRun>)> {
    if text.is_empty() {
        return vec![(text.clone(), Vec::new())];
    }
    let mut ends = line_ends(text, columns);
    let first = ends.next().unwrap();
    if first == text.len() {
        return vec![(
            text.clone(),
            runs.iter().filter(|r| r.len > 0).cloned().collect(),
        )];
    }

    let mut lines = Vec::new();
    let (mut start, mut run_index, mut run_used) = (0, 0, 0);
    for end in std::iter::once(first).chain(ends) {
        let mut line_runs = Vec::new();
        let mut remaining = end - start;
        while remaining > 0 {
            while run_used == runs[run_index].len {
                run_index += 1;
                run_used = 0;
            }
            let mut run = runs[run_index].clone();
            run.len = remaining.min(run.len - run_used);
            remaining -= run.len;
            run_used += run.len;
            line_runs.push(run);
        }
        lines.push((text[start..end].to_owned().into(), line_runs));
        start = end;
    }
    lines
}

/// Counts visual lines without allocation, using the same rules as [`wrap_columns`].
/// Empty text counts as one line; zero columns disables wrapping.
pub fn visual_lines(text: &str, columns: usize) -> usize {
    line_ends(text, columns).count()
}

/// UTF-8 byte offsets ending each visual line, including the final (possibly empty) line.
fn line_ends(text: &str, columns: usize) -> impl Iterator<Item = usize> + '_ {
    let mut chars = text.char_indices();
    let mut width = 0;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        if columns > 0 {
            for (offset, ch) in chars.by_ref() {
                let next_width = if is_wide(ch) { 2 } else { 1 };
                if width > 0 && next_width > columns.saturating_sub(width) {
                    width = next_width;
                    return Some(offset);
                }
                width += next_width;
            }
        }
        done = true;
        Some(text.len())
    })
}

/// Same table as `text::is_wide` and `wispy_core::wrap` (private there).
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{font, hsla};

    fn run(len: usize, hue: f32) -> TextRun {
        TextRun {
            len,
            font: font("Mono"),
            color: hsla(hue, 0.5, 0.5, 1.0),
            background_color: Some(hsla(hue, 0.3, 0.3, 1.0)),
            underline: None,
            strikethrough: None,
        }
    }

    fn check(text: &str, columns: usize, expected: &[&str]) {
        let runs = if text.is_empty() {
            Vec::new()
        } else {
            vec![run(text.len(), 0.0)]
        };
        let lines = wrap_columns(&text.to_owned().into(), &runs, columns);
        assert_eq!(
            lines.iter().map(|(s, _)| s.as_ref()).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(visual_lines(text, columns), lines.len());
        for (line, runs) in lines {
            assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), line.len());
            assert!(runs.iter().all(|r| r.len > 0));
        }
    }

    #[test]
    fn ascii_exact_multiple() {
        check("abcdef", 3, &["abc", "def"]);
    }

    #[test]
    fn ascii_remainder_and_breaks_inside_words() {
        check("ab cdef", 3, &["ab ", "cde", "f"]);
    }

    #[test]
    fn wide_character_moves_to_next_line() {
        check("ab界c", 3, &["ab", "界c"]);
        check("a界b", 3, &["a界", "b"]);
    }

    #[test]
    fn emoji_uses_two_columns_and_four_bytes() {
        check("a😀b😀", 3, &["a😀", "b😀"]);
    }

    #[test]
    fn oversized_character_still_makes_progress() {
        check("界😀a界", 1, &["界", "😀", "a", "界"]);
    }

    #[test]
    fn run_spanning_break_keeps_style() {
        let text = SharedString::from("é😀xy");
        let original = run(text.len(), 0.25);
        let lines = wrap_columns(&text, std::slice::from_ref(&original), 3);
        assert_eq!(lines[0].0.as_ref(), "é😀");
        assert_eq!(lines[1].0.as_ref(), "xy");
        for (line, runs) in lines {
            let mut expected = original.clone();
            expected.len = line.len();
            assert_eq!(runs, vec![expected]);
        }
    }

    #[test]
    fn multiple_runs_per_line() {
        let runs = vec![run(1, 0.0), run(4, 0.25), run(1, 0.5)];
        let lines = wrap_columns(&SharedString::from("abcdef"), &runs, 3);
        assert_eq!(lines[0].0.as_ref(), "abc");
        assert_eq!(lines[1].0.as_ref(), "def");
        assert_eq!(lines[0].1, vec![runs[0].clone(), run(2, 0.25)]);
        assert_eq!(lines[1].1, vec![run(2, 0.25), runs[2].clone()]);
        for (line, runs) in lines {
            assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), line.len());
        }
    }

    #[test]
    fn empty_unwrapped_and_fitting_text() {
        check("", 3, &[""]);
        check("", 0, &[""]);
        check("a界😀", 0, &["a界😀"]);
        check("a界", 3, &["a界"]);
        let text = SharedString::from("abcd");
        let runs = vec![run(1, 0.0), run(3, 0.5)];
        for columns in [0, 4, 5, usize::MAX] {
            assert_eq!(
                wrap_columns(&text, &runs, columns),
                vec![(text.clone(), runs.clone())]
            );
        }
    }

    #[test]
    fn zero_length_runs_are_omitted() {
        let runs = vec![
            run(0, 0.0),
            run(2, 0.5),
            run(0, 0.0),
            run(2, 0.5),
            run(0, 0.0),
        ];
        for columns in [0, 1, 2, 4] {
            let lines = wrap_columns(&SharedString::from("abcd"), &runs, columns);
            for (line, runs) in lines {
                assert!(runs.iter().all(|r| r.len > 0));
                assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), line.len());
            }
        }
        assert!(wrap_columns(&SharedString::from(""), &[run(0, 0.0)], 2)[0]
            .1
            .is_empty());
    }

    #[test]
    fn count_matches_wrapping() {
        for text in [
            "",
            "abc def ghi",
            "éèx",
            "a界b界",
            "😀😀😀",
            "e\u{301}x",
            "\u{20000}ab",
        ] {
            for columns in [0, 1, 2, 3, 4, 8, usize::MAX] {
                let runs = vec![run(text.len(), 0.0)];
                let lines = wrap_columns(&text.to_owned().into(), &runs, columns);
                assert_eq!(
                    visual_lines(text, columns),
                    lines.len(),
                    "{text:?}, {columns}"
                );
                assert_eq!(
                    lines.iter().map(|(s, _)| s.as_ref()).collect::<String>(),
                    text
                );
                for (line, runs) in lines {
                    assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), line.len());
                    assert!(runs.iter().all(|r| r.len > 0));
                }
            }
        }
    }
}
