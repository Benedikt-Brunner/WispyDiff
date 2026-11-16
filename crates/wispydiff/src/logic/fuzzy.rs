//! Spotlight-style file search; scoring and ranking from FileSearch.tsx.
use wispy_core::model::FileSummary;
pub const MAX_RESULTS: usize = 50;
#[derive(Debug, Clone, PartialEq)]
pub struct PathMatch {
    pub score: f64,
    /// UTF-8 byte offsets of matched characters, suitable for GPUI highlight ranges.
    pub positions: Vec<usize>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub index: usize,
    pub score: f64,
    pub positions: Vec<usize>,
}
/// Query characters in path order (case-insensitive, whitespace ignored). Word starts (after
/// `/ _ - .` or a camelCase hump), consecutive runs, and the file name count most. The best
/// placement wins; shorter paths break score ties. Whitespace removal follows JS `\s`.
pub fn match_path(path: &str, query: &str) -> Option<PathMatch> {
    // Score UTF-16 units exactly like the TS implementation (including lowercase expansions).
    // Only the final highlight positions are converted to UTF-8 bytes for GPUI.
    let q: Vec<u16> = query
        .to_lowercase()
        .chars()
        .filter(|c| !super::js_whitespace(*c))
        .collect::<String>()
        .encode_utf16()
        .collect();
    if q.is_empty() {
        return Some(PathMatch {
            score: 0.,
            positions: vec![],
        });
    }
    let units: Vec<u16> = path.encode_utf16().collect();
    let lower: Vec<u16> = path.to_lowercase().encode_utf16().collect();
    let n = units.len();
    let name_start = units
        .iter()
        .rposition(|u| *u == u16::from(b'/'))
        .map_or(0, |i| i + 1);
    let mut bytes = Vec::with_capacity(n);
    for (byte, ch) in path.char_indices() {
        bytes.extend(std::iter::repeat_n(byte, ch.len_utf16()));
    }
    let lowercase_unchanged = |unit: u16| {
        char::from_u32(u32::from(unit)).is_none_or(|ch| ch.to_lowercase().eq(std::iter::once(ch)))
    };
    let mut scores: Vec<Vec<f64>> = vec![];
    let mut from: Vec<Vec<Option<usize>>> = vec![];
    for (i, ch) in q.iter().enumerate() {
        let mut row = vec![f64::NEG_INFINITY; n];
        let mut back = vec![None; n];
        let prev = i.checked_sub(1).map(|p| &scores[p]);
        let mut best = if prev.is_some() {
            f64::NEG_INFINITY
        } else {
            0.
        };
        let mut best_at = None;
        for j in 0..n {
            if let Some(prev) = prev {
                if j > 0 && prev[j - 1] > best {
                    best = prev[j - 1];
                    best_at = Some(j - 1);
                }
            }
            if lower.get(j) != Some(ch) {
                continue;
            }
            let mut score = best;
            let mut at = best_at;
            if let Some(prev) = prev {
                if j > 0 && prev[j - 1] + 5. >= score {
                    score = prev[j - 1] + 5.;
                    at = Some(j - 1);
                }
            }
            if score == f64::NEG_INFINITY {
                continue;
            }
            let boundary = j == 0
                || [b'/', b'_', b'-', b'.', b' ']
                    .iter()
                    .any(|c| u16::from(*c) == units[j - 1])
                || (lowercase_unchanged(units[j - 1]) && Some(&units[j]) != lower.get(j));
            row[j] =
                score + 1. + if boundary { 8. } else { 0. } + if j >= name_start { 2. } else { 0. };
            back[j] = at;
        }
        scores.push(row);
        from.push(back);
    }
    let last = scores.last()?;
    let mut end = None;
    for j in 0..n {
        if last[j] > f64::NEG_INFINITY && end.is_none_or(|e| last[j] > last[e]) {
            end = Some(j);
        }
    }
    let end = end?;
    let mut positions = vec![0; q.len()];
    let mut j = end;
    for i in (0..q.len()).rev() {
        positions[i] = j;
        if i > 0 {
            j = from[i][j]?;
        }
    }
    let score = last[end] + if positions[0] >= name_start { 10. } else { 0. }
        - path.encode_utf16().count() as f64 / 100.;
    let mut positions: Vec<usize> = positions.into_iter().map(|unit| bytes[unit]).collect();
    // Both surrogate units of an emoji highlight the same native character.
    positions.dedup();
    Some(PathMatch { score, positions })
}
/// Hidden files come last; otherwise best score first, ties in diff order. At most 50 results.
pub fn rank_paths(paths: &[impl AsRef<str>], filtered: &[bool], query: &str) -> Vec<SearchResult> {
    let mut out: Vec<_> = paths
        .iter()
        .enumerate()
        .filter_map(|(index, p)| {
            match_path(p.as_ref(), query).map(|m| SearchResult {
                index,
                score: m.score,
                positions: m.positions,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        filtered
            .get(a.index)
            .copied()
            .unwrap_or(false)
            .cmp(&filtered.get(b.index).copied().unwrap_or(false))
            .then_with(|| b.score.total_cmp(&a.score))
            .then(a.index.cmp(&b.index))
    });
    out.truncate(MAX_RESULTS);
    out
}
pub fn search_files(files: &[FileSummary], filtered: &[bool], query: &str) -> Vec<SearchResult> {
    rank_paths(
        &files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        filtered,
        query,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_and_boundaries() {
        assert_eq!(
            match_path("src/FileSearch.tsx", " F S ").unwrap().positions,
            [4, 8]
        );
        assert_eq!(match_path("aaa/aa", "aa").unwrap().positions, [0, 4]);
        assert!(match_path("abc", "ca").is_none());
        assert!(match_path("", "x").is_none());
        assert_eq!(match_path("abc", "\t \n").unwrap().score, 0.);
        assert!(
            match_path("my-file.ts", "mf").unwrap().score
                > match_path("myfile.ts", "mf").unwrap().score
        );
    }
    #[test]
    fn rank_hidden_ties_limit_and_utf8() {
        let r = rank_paths(
            &["foo.rs", "dir/foo.rs", "x/f_o_o.rs"],
            &[true, false, false],
            "foo",
        );
        assert_eq!(r.iter().map(|r| r.index).collect::<Vec<_>>(), [2, 1, 0]);
        assert_eq!(
            rank_paths(&["a", "b"], &[], "")
                .iter()
                .map(|r| r.index)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(rank_paths(&vec!["file"; 60], &[], "").len(), 50);
        assert_eq!(match_path("界/Foo.rs", "Fo").unwrap().positions, [4, 5]);
        assert_eq!(match_path("src/Été.rs", "ét").unwrap().positions, [4, 6]);
    }
    #[test]
    fn typescript_score_parity_and_native_offsets() {
        // Captured from the original TS scorer; final positions use native byte offsets.
        for (path, query, score, positions) in [
            ("aaa/aa", "aa", 19.94, vec![0, 4]),
            ("dir/foo.rs", "foo", 36.9, vec![4, 5, 6]),
            ("x/f_o_o.rs", "foo", 42.9, vec![2, 4, 6]),
            ("src/FileSearch.tsx", " F S ", 31.82, vec![4, 8]),
            ("src/Été.rs", "ét", 28.9, vec![4, 6]),
            ("x😀.rs", "😀", 20.94, vec![1]),
        ] {
            let result = match_path(path, query).unwrap();
            assert!((result.score - score).abs() < 1e-10, "{path}");
            assert_eq!(result.positions, positions, "{path}");
            assert!(result.positions.iter().all(|i| path.is_char_boundary(*i)));
        }
        // JS indexes the lowercased string by the original UTF-16 offsets, even when it expands.
        assert!(match_path("İ/a", "a").is_none());
        assert!(match_path("file", "\u{85}").is_none());
        assert!(match_path("file", "\u{feff}").unwrap().positions.is_empty());
        assert!(
            match_path("file.rs", "file").unwrap().score
                > match_path("long/file.rs", "file").unwrap().score
        );
    }
}
