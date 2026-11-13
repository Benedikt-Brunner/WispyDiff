//! Turns the core's highlighted segments (+ changed-word spans) into shaped-text input.

use gpui_kit::{Font, Hsla, SharedString, TextRun};

use crate::theme::Palette;
use wispy_core::highlight::Seg;
use wispy_core::wrap::TAB_WIDTH;


/// A code line ready for `StyledText::with_runs`: tabs expanded to the next tab stop, one run per
/// stretch of equal token class and changed-word state.
pub fn code_line(segs: &[Seg], words: &[(u32, u32)], font: &Font, p: &Palette, word_bg: Hsla) -> (SharedString, Vec<TextRun>) {
    let mut text = String::with_capacity(segs.iter().map(|(_, s)| s.len()).sum());
    let mut runs: Vec<TextRun> = Vec::with_capacity(segs.len() + words.len() * 2);
    // Current run: (token class, inside a changed word, byte length).
    let mut current: Option<(u8, bool, usize)> = None;
    let mut utf16 = 0u32;
    let mut column = 0u32;
    let mut span = 0usize;

    let flush = |current: &mut Option<(u8, bool, usize)>, runs: &mut Vec<TextRun>| {
        if let Some((class, in_word, len)) = current.take() {
            if len > 0 {
                runs.push(run(len, class, in_word, font, p, word_bg));
            }
        }
    };

    for (class, segment) in segs {
        for ch in segment.chars() {
            while span < words.len() && words[span].1 <= utf16 {
                span += 1;
            }
            let in_word = span < words.len() && words[span].0 <= utf16;
            let before = text.len();
            match ch {
                '\t' => {
                    let to = TAB_WIDTH - column % TAB_WIDTH;
                    text.extend(std::iter::repeat_n(' ', to as usize));
                    column += to;
                }
                _ => {
                    text.push(ch);
                    column += if is_wide(ch) { 2 } else { 1 };
                }
            }
            let added = text.len() - before;
            match &mut current {
                Some((c, w, len)) if *c == *class && *w == in_word => *len += added,
                _ => {
                    flush(&mut current, &mut runs);
                    current = Some((*class, in_word, added));
                }
            }
            utf16 += ch.len_utf16() as u32;
        }
    }
    flush(&mut current, &mut runs);
    (text.into(), runs)
}

/// Paints bytes `start..end` of the text with background `bg` (a text selection, a flash).
pub fn highlight_bytes(runs: &mut Vec<TextRun>, start: usize, end: usize, bg: Hsla) {
    if start >= end {
        return;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut at = 0;
    for run in runs.drain(..) {
        let (run_start, run_end) = (at, at + run.len);
        at = run_end;
        let cuts = [run_start, start.clamp(run_start, run_end), end.clamp(run_start, run_end), run_end];
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a >= b {
                continue;
            }
            let mut piece = run.clone();
            piece.len = b - a;
            if a >= start && b <= end {
                piece.background_color = Some(bg);
            }
            out.push(piece);
        }
    }
    *runs = out;
}

/// The byte in `original` (a line with tabs) for byte `expanded` of its tab-expanded text.
pub fn original_byte(original: &str, expanded: usize) -> usize {
    let mut at = 0;
    let mut column = 0u32;
    for (i, ch) in original.char_indices() {
        if at >= expanded {
            return i;
        }
        let width = match ch {
            '\t' => {
                let to = TAB_WIDTH - column % TAB_WIDTH;
                column += to;
                to as usize
            }
            _ => {
                column += if is_wide(ch) { 2 } else { 1 };
                ch.len_utf8()
            }
        };
        at += width;
    }
    original.len()
}

/// One run of plain text in `color`.
pub fn plain(text: impl Into<SharedString>, font: &Font, color: Hsla) -> (SharedString, Vec<TextRun>) {
    let text = text.into();
    let runs = vec![TextRun { len: text.len(), font: font.clone(), color, background_color: None, underline: None, strikethrough: None }];
    (text, runs)
}

fn run(len: usize, class: u8, in_word: bool, font: &Font, p: &Palette, word_bg: Hsla) -> TextRun {
    let mut font = font.clone();
    let i = (class as usize).min(19);
    font.style = p.token_styles[i];
    font.weight = p.token_weights[i];
    TextRun {
        len,
        font,
        color: p.tokens[i],
        background_color: in_word.then_some(word_bg),
        underline: None,
        strikethrough: None,
    }
}

/// Same table as `wispy_core::wrap` (private there).
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

    fn tokens() -> Palette {
        Palette::light()
    }

    fn lens(runs: &[TextRun]) -> Vec<(usize, bool)> {
        runs.iter().map(|r| (r.len, r.background_color.is_some())).collect()
    }

    #[test]
    fn expands_tabs_to_the_next_stop() {
        let segs = vec![(0, "a\tb".to_string())];
        let (text, runs) = code_line(&segs, &[], &font("Mono"), &tokens(), hsla(0., 0., 0., 1.));
        assert_eq!(text.as_ref(), "a       b");
        assert_eq!(lens(&runs), vec![(9, false)]);
    }

    #[test]
    fn splits_runs_at_changed_words_measured_in_utf16() {
        // "é" is one UTF-16 unit but two bytes; "😀" is two UTF-16 units and four bytes.
        let segs = vec![(1, "é😀x".to_string()), (2, "yz".to_string())];
        let (text, runs) = code_line(&segs, &[(1, 4)], &font("Mono"), &tokens(), hsla(0., 0., 0., 1.));
        assert_eq!(text.as_ref(), "é😀xyz");
        // é | 😀x (changed: UTF-16 1..4) | yz (other class, unchanged)
        assert_eq!(lens(&runs), vec![(2, false), (5, true), (2, false)]);
        let (_, runs) = code_line(&segs, &[(1, 5)], &font("Mono"), &tokens(), hsla(0., 0., 0., 1.));
        // The change reaching into the next segment splits it: y (changed) | z.
        assert_eq!(lens(&runs), vec![(2, false), (5, true), (1, true), (1, false)]);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), text.len());
    }

    #[test]
    fn highlights_a_byte_range_across_runs() {
        let segs = vec![(1, "abc".to_string()), (2, "def".to_string())];
        let (_, mut runs) = code_line(&segs, &[], &font("Mono"), &tokens(), hsla(0., 0., 0., 1.));
        highlight_bytes(&mut runs, 2, 4, hsla(0.5, 0.5, 0.5, 1.));
        assert_eq!(lens(&runs), vec![(2, false), (1, true), (1, true), (2, false)]);
    }

    #[test]
    fn maps_expanded_bytes_back_to_tabs() {
        // "a\tb" shows as "a       b": byte 8 of that is the "b".
        assert_eq!(original_byte("a\tb", 8), 2);
        assert_eq!(original_byte("a\tb", 1), 1);
        assert_eq!(original_byte("a\tb", 99), 3);
    }

    #[test]
    fn comments_are_italic() {
        let segs = vec![(3, "// hi".to_string())];
        let (_, runs) = code_line(&segs, &[], &font("Mono"), &tokens(), hsla(0., 0., 0., 1.));
        assert_eq!(runs[0].font.style, gpui_kit::FontStyle::Italic);
    }
}
