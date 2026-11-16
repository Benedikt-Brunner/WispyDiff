//! Unified rows interleaved with revealed unchanged lines and gap headers.
use wispy_core::{
    highlight::Seg,
    model::{FileSummary, Gap},
};
/// Lines a click on a gap shows.
pub const GAP_STEP: u32 = 20;
/// Per gap (by index): shown lines [below the hunk above, above the hunk below].
pub type Reveal = Vec<[u32; 2]>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayRow {
    Base { base: u32 },
    Line { o: u32, n: u32 },
    Gap { gap: usize, base: Option<u32> },
}
#[derive(Debug, Clone)]
enum Kind {
    Base(u32),
    Lines { new_start: u32, old_start: u32 },
    Gap { gap: usize, base: Option<u32> },
}
#[derive(Debug, Clone)]
struct Piece {
    at: u32,
    n: u32,
    kind: Kind,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    pub base: Option<[u32; 2]>,
    pub lines: Option<[u32; 2]>,
}
/// How many lines of `gap` are still hidden.
pub fn hidden_lines(gap: &Gap, shown: Option<&[u32; 2]>) -> u32 {
    gap.len
        .saturating_sub(shown.map_or(0, |s| s[0].saturating_add(s[1])))
}
/// A file's unified rows with some gaps shown: maps display offsets (header = 0) to the
/// file's unified rows and shown head lines. A gap row stays for whatever is still hidden.
#[derive(Debug, Clone)]
pub struct UnifiedMap {
    pub file: FileSummary,
    pub reveal: Option<Reveal>,
    pub rows: u32,
    pieces: Vec<Piece>,
}
impl UnifiedMap {
    pub fn new(file: &FileSummary, reveal: Option<&[[u32; 2]]>) -> Self {
        let mut out = Self {
            file: file.clone(),
            reveal: reveal.map(<[_]>::to_vec),
            rows: 0,
            pieces: vec![],
        };
        let mut base = 0;
        for (index, gap) in file.gaps.iter().enumerate() {
            let trailing = gap.offset >= file.row_count;
            out.push(
                gap.offset.min(file.row_count).saturating_sub(base),
                Kind::Base(base),
            );
            let [down, up] = reveal.and_then(|r| r.get(index)).copied().unwrap_or([0, 0]);
            if down.saturating_add(up) >= gap.len {
                out.push(
                    gap.len,
                    Kind::Lines {
                        new_start: gap.new_start,
                        old_start: gap.old_start,
                    },
                );
            } else {
                out.push(
                    down,
                    Kind::Lines {
                        new_start: gap.new_start,
                        old_start: gap.old_start,
                    },
                );
                out.push(
                    1,
                    Kind::Gap {
                        gap: index,
                        base: (!trailing).then_some(gap.offset),
                    },
                );
                let skip = gap.len - up;
                out.push(
                    up,
                    Kind::Lines {
                        new_start: gap.new_start + skip,
                        old_start: gap.old_start + skip,
                    },
                );
            }
            base = if trailing {
                file.row_count
            } else {
                gap.offset + 1
            };
        }
        out.push(file.row_count.saturating_sub(base), Kind::Base(base));
        out
    }
    fn push(&mut self, n: u32, kind: Kind) {
        if n > 0 {
            self.pieces.push(Piece {
                at: self.rows,
                n,
                kind,
            });
        }
        self.rows += n;
    }
    /// What display row `offset` is.
    pub fn at(&self, offset: u32) -> DisplayRow {
        let i = self
            .pieces
            .partition_point(|p| p.at <= offset)
            .saturating_sub(1);
        let Some(p) = self.pieces.get(i) else {
            return DisplayRow::Base { base: offset };
        };
        let i = offset - p.at;
        match p.kind {
            Kind::Base(base) => DisplayRow::Base { base: base + i },
            Kind::Lines {
                new_start,
                old_start,
            } => DisplayRow::Line {
                o: old_start + i,
                n: new_start + i,
            },
            Kind::Gap { gap, base } => DisplayRow::Gap { gap, base },
        }
    }
    /// Where unified row `base` is drawn (a hunk header shown away: where its hunk starts).
    pub fn from_base(&self, base: u32) -> u32 {
        for p in &self.pieces {
            match p.kind {
                Kind::Base(b) if base < b + p.n => return p.at + base.saturating_sub(b),
                Kind::Gap { base: Some(b), .. } if b == base => return p.at,
                _ => (),
            }
        }
        self.rows
    }
    /// Where the row standing for gap `index` is drawn, if the gap isn't all shown.
    pub fn gap_row(&self, index: usize) -> Option<u32> {
        self.pieces
            .iter()
            .find_map(|p| matches!(p.kind, Kind::Gap { gap,.. } if gap == index).then_some(p.at))
    }
    /// Where shown head line `line` of a gap is drawn, if it is.
    pub fn from_new_line(&self, line: u32) -> Option<u32> {
        self.pieces.iter().find_map(|p| match p.kind {
            Kind::Lines { new_start, .. } if line >= new_start && line < new_start + p.n => {
                Some(p.at + line - new_start)
            }
            _ => None,
        })
    }
    /// Unified rows and head lines that display rows `start..end` need loaded (half-open).
    pub fn needs(&self, start: u32, end: u32) -> Needs {
        fn merge(to: &mut Option<[u32; 2]>, r: [u32; 2]) {
            *to = Some(to.map_or(r, |v| [v[0].min(r[0]), v[1].max(r[1])]));
        }
        let mut out = Needs::default();
        for p in &self.pieces {
            let from = start.max(p.at);
            let to = end.min(p.at + p.n);
            if from >= to {
                continue;
            }
            match p.kind {
                Kind::Lines { new_start, .. } => merge(
                    &mut out.lines,
                    [new_start + from - p.at, new_start + to - p.at],
                ),
                Kind::Base(b) => merge(&mut out.base, [b + from - p.at, b + to - p.at]),
                Kind::Gap { base: Some(b), .. } => merge(&mut out.base, [b, b + 1]),
                _ => (),
            }
        }
        out
    }
    /// `[display offset, width]` of every shown gap line, given the loaded head lines.
    pub fn line_widths<'a>(&self, mut line: impl FnMut(u32) -> Option<&'a [Seg]>) -> Vec<[u32; 2]> {
        let mut out = vec![];
        for p in &self.pieces {
            if let Kind::Lines { new_start, .. } = p.kind {
                for i in 0..p.n {
                    if let Some(segs) = line(new_start + i) {
                        out.push([p.at + i, text_width(segs)]);
                    }
                }
            }
        }
        out
    }
}
/// Shows `count` more lines of gap `index` (`down`: below the hunk above it).
/// `u32::MAX` means all remaining lines (the TypeScript API's `Infinity`).
pub fn reveal_more(
    file: &FileSummary,
    reveal: Option<&[[u32; 2]]>,
    index: usize,
    down: bool,
    count: u32,
) -> Reveal {
    let mut next: Reveal = (0..file.gaps.len())
        .map(|i| reveal.and_then(|r| r.get(i)).copied().unwrap_or([0, 0]))
        .collect();
    let add = count.min(hidden_lines(&file.gaps[index], Some(&next[index])));
    next[index][if down { 0 } else { 1 }] += add;
    next
}
/// Every gap shown.
pub fn reveal_all(file: &FileSummary) -> Reveal {
    file.gaps.iter().map(|g| [g.len, 0]).collect()
}
pub fn all_shown(file: &FileSummary, reveal: Option<&[[u32; 2]]>) -> bool {
    file.gaps
        .iter()
        .enumerate()
        .all(|(i, g)| hidden_lines(g, reveal.and_then(|r| r.get(i))) == 0)
}
/// Monospace columns, with tabs at eight-column stops and the core's wide-character ranges.
pub fn text_width(segs: &[Seg]) -> u32 {
    wispy_core::wrap::text_width(segs)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file() -> FileSummary {
        let mut f = super::super::test_file();
        f.gaps = vec![
            Gap {
                offset: 1,
                old_start: 1,
                new_start: 1,
                len: 30,
            },
            Gap {
                offset: 5,
                old_start: 40,
                new_start: 41,
                len: 5,
            },
            Gap {
                offset: 8,
                old_start: 50,
                new_start: 51,
                len: 21,
            },
        ];
        f
    }
    #[test]
    fn hidden_headers_trailing_and_needs() {
        let f = file();
        let map = UnifiedMap::new(&f, None);
        assert_eq!(map.rows, 9);
        assert_eq!(map.gap_row(2), Some(8));
        assert_eq!(
            map.at(1),
            DisplayRow::Gap {
                gap: 0,
                base: Some(1)
            }
        );
        assert_eq!(map.at(8), DisplayRow::Gap { gap: 2, base: None });
        assert_eq!(map.needs(8, 9), Needs::default());
        assert_eq!(map.needs(0, 8).base, Some([0, 8]));
        assert_eq!(map.from_base(5), 5);
    }
    #[test]
    fn partial_full_and_bidirectional_mapping() {
        let f = file();
        let r = vec![[10, 10], [5, 0], [0, 3]];
        let m = UnifiedMap::new(&f, Some(&r));
        assert_eq!(m.rows, 36);
        assert_eq!(m.at(1), DisplayRow::Line { o: 1, n: 1 });
        assert_eq!(m.gap_row(0), Some(11));
        assert_eq!(m.gap_row(1), None);
        assert_eq!(m.from_new_line(21), Some(12));
        assert_eq!(m.from_new_line(15), None);
        assert_eq!(m.from_base(1), 11);
        assert_eq!(m.from_base(5), 30);
        assert_eq!(
            m.needs(1, 12),
            Needs {
                base: Some([1, 2]),
                lines: Some([1, 11])
            }
        );
        let segs = vec![(0, "界\tA".into())];
        assert_eq!(
            m.line_widths(|n| (n == 1).then_some(segs.as_slice())),
            [[1, 9]]
        );
        let full = UnifiedMap::new(&f, Some(&reveal_all(&f)));
        assert_eq!(full.rows, 62);
        assert_eq!(full.from_base(1), 31);
        assert!(full.gap_row(2).is_none());
    }
    #[test]
    fn reveals_saturate_and_empty_files() {
        let f = file();
        let r = reveal_more(&f, None, 0, true, GAP_STEP);
        assert_eq!(r[0], [20, 0]);
        let r = reveal_more(&f, Some(&r), 0, false, u32::MAX);
        assert_eq!(r[0], [20, 10]);
        assert_eq!(hidden_lines(&f.gaps[0], Some(&[100, 100])), 0);
        assert!(!all_shown(&f, Some(&r)));
        assert!(all_shown(&f, Some(&reveal_all(&f))));
        let mut f = f;
        f.gaps.clear();
        f.row_count = 0;
        assert!(all_shown(&f, None));
        assert_eq!(
            UnifiedMap::new(&f, None).at(0),
            DisplayRow::Base { base: 0 }
        );
    }
}
