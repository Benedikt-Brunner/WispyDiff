//! DOM-independent row, wrapping and insert mapping from layout.ts. The native list may
//! measure heights itself; this model is also useful for navigation/scroll anchors and zoom.
use super::gaps::UnifiedMap;
use wispy_core::model::{DiffSummary, FileSummary};
pub const ROW_HEIGHT: f64 = 20.;
/// Hidden files are left out by the filter (no rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Unified,
    Split,
    Collapsed,
    Hidden,
}
/// One file's rows in its chosen mode; start is its first global row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub file: usize,
    pub mode: Mode,
    pub start: u32,
    pub rows: u32,
}
/// A variable-height block (comment box, thread, ...) below global row `after`.
#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    pub key: String,
    pub after: u32,
    pub height: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedInsert {
    pub insert: Insert,
    pub y: f64,
}
/// [offset, extra lines] of a file's wrapped rows, sorted by offset.
pub type WrappedRows = Vec<[u32; 2]>;
pub fn segment_rows(file: &FileSummary, mode: Mode, unified: Option<&UnifiedMap>) -> u32 {
    match mode {
        Mode::Hidden => 0,
        Mode::Collapsed => 1,
        Mode::Split => file.split_rows,
        Mode::Unified => unified.map_or(file.row_count, |u| u.rows),
    }
}
/// Files stacked in their modes, plus inserts. Rows are `row_height` tall, or a multiple
/// when wrapped. Positions include extra wrapped lines above and insert heights above.
#[derive(Debug, Clone)]
pub struct Layout {
    pub segments: Vec<Segment>,
    pub total_rows: u32,
    row_height: f64,
    inserts: Vec<Insert>,
    cumulative: Vec<f64>,
    wrapped: Vec<u32>,
    wrapped_cumulative: Vec<u32>,
    unified: Vec<Option<UnifiedMap>>,
}
impl Layout {
    pub fn new(
        summary: &DiffSummary,
        mode_of: impl Fn(usize) -> Mode,
        unified_of: impl Fn(usize) -> UnifiedMap,
        inserts: &[Insert],
        wrapped_of: impl Fn(usize, Mode) -> WrappedRows,
    ) -> Self {
        let mut out = Self {
            segments: vec![],
            total_rows: 0,
            row_height: ROW_HEIGHT,
            inserts: inserts.to_vec(),
            cumulative: vec![0.],
            wrapped: vec![],
            wrapped_cumulative: vec![0],
            unified: vec![],
        };
        for (index, file) in summary.files.iter().enumerate() {
            let mode = mode_of(index);
            let unified = (mode == Mode::Unified).then(|| unified_of(index));
            let rows = segment_rows(file, mode, unified.as_ref());
            let segment = Segment {
                file: index,
                mode,
                start: out.total_rows,
                rows,
            };
            for [offset, extra] in wrapped_of(index, mode) {
                if offset >= rows {
                    break;
                }
                out.wrapped.push(segment.start + offset);
                out.wrapped_cumulative
                    .push(out.wrapped_cumulative.last().unwrap() + extra);
            }
            out.total_rows += rows;
            out.segments.push(segment);
            out.unified.push(unified);
        }
        out.inserts.sort_by_key(|i| i.after);
        for insert in &out.inserts {
            out.cumulative
                .push(out.cumulative.last().unwrap() + insert.height);
        }
        out
    }
    /// Scale row geometry for the app's zoom/font size; insert heights are already measured.
    pub fn with_row_height(mut self, height: f64) -> Self {
        assert!(height.is_finite() && height > 0.);
        self.row_height = height;
        self
    }
    pub fn height(&self) -> f64 {
        (self.total_rows + self.wrapped_cumulative.last().unwrap()) as f64 * self.row_height
            + self.cumulative.last().unwrap()
    }
    /// Top of global row `row`.
    pub fn row_y(&self, row: u32) -> f64 {
        (row + self.wrapped_cumulative[self.wrapped_before(row)]) as f64 * self.row_height
            + self.cumulative[self.inserts.partition_point(|i| i.after < row)]
    }
    /// Height of global row `row` (taller when it wraps).
    pub fn row_height(&self, row: u32) -> f64 {
        let i = self.wrapped_before(row);
        if self.wrapped.get(i) == Some(&row) {
            (1 + self.wrapped_cumulative[i + 1] - self.wrapped_cumulative[i]) as f64
                * self.row_height
        } else {
            self.row_height
        }
    }
    /// Row at vertical position y; an insert counts as part of the row above it.
    pub fn row_at(&self, y: f64) -> u32 {
        let mut lo = 0;
        let mut hi = self.total_rows.saturating_sub(1);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.row_y(mid) <= y {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }
    /// Inserts below rows first..=last, plus the one keyed always wherever it is, with each
    /// top position (stacked in order under its row).
    pub fn inserts_between(
        &self,
        first: u32,
        last: u32,
        always: Option<&str>,
    ) -> Vec<PositionedInsert> {
        self.inserts
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                (i.after >= first && i.after <= last) || always == Some(i.key.as_str())
            })
            .map(|(index, i)| {
                let below = i.after + 1;
                PositionedInsert {
                    insert: i.clone(),
                    y: (below + self.wrapped_cumulative[self.wrapped_before(below)]) as f64
                        * self.row_height
                        + self.cumulative[index],
                }
            })
            .collect()
    }
    /// Segment holding row (never hidden for an in-range row unless all files are hidden).
    /// Empty summaries have no segment.
    pub fn segment_at(&self, row: u32) -> Option<&Segment> {
        self.segments.get(
            self.segments
                .partition_point(|s| s.start <= row)
                .saturating_sub(1),
        )
    }
    /// Global change starts: unified hunks, split change blocks, or collapsed file headers.
    pub fn change_rows(&self, summary: &DiffSummary) -> Vec<u32> {
        let mut out = vec![];
        for s in &self.segments {
            let f = &summary.files[s.file];
            match s.mode {
                Mode::Hidden => (),
                Mode::Collapsed => out.push(s.start),
                Mode::Split => out.extend(f.split_blocks.iter().map(|o| s.start + o)),
                Mode::Unified => out.extend(
                    f.hunks
                        .iter()
                        .map(|o| s.start + self.unified[s.file].as_ref().unwrap().from_base(*o)),
                ),
            }
        }
        out
    }
    fn wrapped_before(&self, row: u32) -> usize {
        self.wrapped.partition_point(|r| *r < row)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn summary() -> DiffSummary {
        DiffSummary {
            base_sha: String::new(),
            head_sha: String::new(),
            files: vec![super::super::test_file(); 3],
            total_rows: 24,
            max_line_chars: 0,
            max_line_number: 0,
            additions: 0,
            deletions: 0,
        }
    }
    #[test]
    fn wrap_insert_coordinates_and_zoom() {
        let s = summary();
        let inserts = vec![
            Insert {
                key: "b".into(),
                after: 2,
                height: 10.,
            },
            Insert {
                key: "a".into(),
                after: 2,
                height: 30.,
            },
        ];
        let l = Layout::new(
            &s,
            |_| Mode::Unified,
            |i| UnifiedMap::new(&s.files[i], None),
            &inserts,
            |i, _| if i == 0 { vec![[2, 2]] } else { vec![] },
        );
        assert_eq!(l.row_y(2), 40.);
        assert_eq!(l.row_height(2), 60.);
        assert_eq!(l.row_y(3), 140.);
        assert_eq!(l.row_at(139.), 2);
        assert_eq!(l.row_at(140.), 3);
        assert_eq!(l.row_at(-5.), 0);
        let i = l.inserts_between(2, 2, None);
        assert_eq!(i[0].y, 100.);
        assert_eq!(i[1].y, 110.);
        assert_eq!(l.inserts_between(8, 9, Some("a")).len(), 1);
        assert_eq!(l.height(), 560.);
        assert_eq!(l.clone().with_row_height(10.).height(), 300.);
    }
    #[test]
    fn hidden_collapsed_split_and_empty() {
        let s = summary();
        let l = Layout::new(
            &s,
            |i| [Mode::Hidden, Mode::Collapsed, Mode::Split][i],
            |i| UnifiedMap::new(&s.files[i], None),
            &[],
            |_, _| vec![],
        );
        // A collapsed file is its header alone.
        assert_eq!(l.total_rows, 13);
        assert_eq!(l.segment_at(0).unwrap().file, 1);
        assert_eq!(l.segment_at(1).unwrap().file, 2);
        assert_eq!(l.change_rows(&s), [0, 4, 9]);
        let mut s = s;
        s.files.clear();
        let l = Layout::new(&s, |_| Mode::Unified, |_| panic!(), &[], |_, _| vec![]);
        assert_eq!(l.height(), 0.);
        assert_eq!(l.row_at(20.), 0);
        assert!(l.segment_at(0).is_none());
    }
}
