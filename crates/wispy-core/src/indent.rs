//! A file's indentation style, so suggested changes indent the way the file does.

use std::collections::HashMap;

/// Spaces per level when the file doesn't show one.
const DEFAULT_WIDTH: usize = 4;

/// One level of indentation in `text`: a tab, or the step (in spaces) its indentation most
/// often grows by.
pub fn indent_unit(text: &str) -> String {
    let (mut tabbed, mut spaced) = (0, 0);
    let mut steps: HashMap<usize, usize> = HashMap::new();
    let mut previous = Some(0);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if line.starts_with('\t') {
            tabbed += 1;
            previous = None;
            continue;
        }
        let width = line.len() - line.trim_start_matches(' ').len();
        // One space is a comment's continuation (` * …` under `/**`), not a level.
        if width == 1 {
            continue;
        }
        if width > 0 {
            spaced += 1;
        }
        if let Some(step) = previous.filter(|&p| width > p).map(|p| width - p) {
            *steps.entry(step).or_default() += 1;
        }
        previous = Some(width);
    }
    if tabbed > spaced {
        return "\t".into();
    }
    let width = steps
        .into_iter()
        .filter(|&(step, _)| step <= 8)
        .max_by_key(|&(step, count)| (count, std::cmp::Reverse(step)))
        .map_or(DEFAULT_WIDTH, |(step, _)| step);
    " ".repeat(width)
}
