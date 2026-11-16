//! Selecting code with the mouse across rows (for copying, and as the assistant's context). GPUI
//! has no native text selection over a virtualized list, so the diff keeps its own.

use super::layout::Mode;
use super::RowRange;

/// Which text of a row: the unified code, or one half side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Unified,
    Old,
    New,
}

/// A point in the code: a row of a file (offset in its mode) and a byte in its shown text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub offset: u32,
    pub byte: usize,
}

#[derive(Clone, Debug)]
pub struct TextSelection {
    pub file: usize,
    pub mode: Mode,
    pub side: Side,
    pub anchor: Point,
    pub head: Point,
    /// Still dragging.
    pub active: bool,
    /// The selected text (set when the drag ends).
    pub text: String,
}

impl TextSelection {
    pub fn start(file: usize, mode: Mode, side: Side, at: Point) -> Self {
        TextSelection { file, mode, side, anchor: at, head: at, active: true, text: String::new() }
    }

    pub fn ordered(&self) -> (Point, Point) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn rows(&self) -> RowRange {
        let (a, b) = self.ordered();
        RowRange { file: self.file, mode: self.mode, from: a.offset, to: b.offset }
    }

    /// The selected bytes of row `offset`'s text (`len` bytes long), if any.
    pub fn range_in(&self, file: usize, mode: Mode, side: Side, offset: u32, len: usize) -> Option<(usize, usize)> {
        if self.is_empty() || file != self.file || mode != self.mode || side != self.side {
            return None;
        }
        let (a, b) = self.ordered();
        if offset < a.offset || offset > b.offset {
            return None;
        }
        let start = if offset == a.offset { a.byte.min(len) } else { 0 };
        let end = if offset == b.offset { b.byte.min(len) } else { len };
        (start < end || (offset != b.offset && len == 0)).then_some((start, end))
    }
}
