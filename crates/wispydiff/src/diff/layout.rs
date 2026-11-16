//! The diff as a list: every file's rows in its mode (header, unified rows with shown gaps,
//! side-by-side rows; a collapsed file is its header alone), with comment items inserted below
//! their rows.

use std::collections::HashMap;

use wispy_core::model::DiffView;

use crate::logic::gaps::UnifiedMap;
pub use crate::logic::layout::Mode;

/// One file's rows; `start` is its first global row.
#[derive(Clone, Debug)]
pub struct Segment {
    pub file: usize,
    pub mode: Mode,
    pub start: u32,
    pub rows: u32,
}

/// A line of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// Global row (see `Built::row_at`).
    Row(u32),
    /// A comment item (index into `Built::inserts`).
    Insert(u32),
}

/// Where an inserted item goes: below global row `after`.
#[derive(Clone, Debug)]
pub struct Placed {
    pub key: String,
    pub after: u32,
}

#[derive(Default)]
pub struct Built {
    pub segments: Vec<Segment>,
    pub total_rows: u32,
    pub items: Vec<Item>,
    /// Item index of each global row.
    pub row_items: Vec<u32>,
    pub inserts: Vec<Placed>,
    /// Unified maps of the unified files (gaps shown or not).
    pub unified: HashMap<usize, UnifiedMap>,
}

impl Built {
    /// The files' segments; `place` then adds the comment items and makes the list.
    pub fn new(view: &DiffView, mode_of: impl Fn(usize) -> Mode, reveals: &HashMap<usize, Vec<[u32; 2]>>) -> Built {
        let mut segments = Vec::with_capacity(view.summary.files.len());
        let mut unified = HashMap::new();
        let mut start = 0u32;
        for (index, file) in view.summary.files.iter().enumerate() {
            let mode = mode_of(index);
            let rows = match mode {
                Mode::Hidden => 0,
                // Just its header, which says why and how to open it.
                Mode::Collapsed => 1,
                Mode::Split => file.split_rows,
                Mode::Unified => {
                    let map = UnifiedMap::new(file, reveals.get(&index).map(Vec::as_slice));
                    let rows = map.rows;
                    unified.insert(index, map);
                    rows
                }
            };
            segments.push(Segment { file: index, mode, start, rows });
            start += rows;
        }
        Built { segments, total_rows: start, items: Vec::new(), row_items: Vec::new(), inserts: Vec::new(), unified }
    }

    /// Lays out the rows with `inserts` below theirs (in the given order where they share a row).
    pub fn place(&mut self, mut inserts: Vec<Placed>) {
        inserts.sort_by_key(|i| i.after);
        let mut items = Vec::with_capacity(self.total_rows as usize + inserts.len());
        let mut row_items = Vec::with_capacity(self.total_rows as usize);
        let mut next_insert = 0;
        for row in 0..self.total_rows {
            row_items.push(items.len() as u32);
            items.push(Item::Row(row));
            while next_insert < inserts.len() && inserts[next_insert].after == row {
                items.push(Item::Insert(next_insert as u32));
                next_insert += 1;
            }
        }
        self.items = items;
        self.row_items = row_items;
        self.inserts = inserts;
    }

    /// The segment holding global row `row` (none in an empty diff).
    pub fn segment_at(&self, row: u32) -> Option<&Segment> {
        let at = self.segments.partition_point(|s| s.start <= row);
        // Skip back over hidden files (no rows) to the one that holds the row.
        self.segments[..at].iter().rev().find(|s| s.rows > 0 && row < s.start + s.rows).or_else(|| self.segments[..at].last())
    }

    /// The global row an item is (an insert counts as the row above it).
    pub fn row_of_item(&self, item: usize) -> u32 {
        match self.items.get(item) {
            Some(Item::Row(row)) => *row,
            Some(Item::Insert(i)) => self.inserts[*i as usize].after,
            None => self.total_rows.saturating_sub(1),
        }
    }

    pub fn item_of_row(&self, row: u32) -> usize {
        self.row_items.get(row as usize).copied().unwrap_or(self.items.len() as u32) as usize
    }
}
