//! Read-only accessors for the headless UI tests (one file per test suite author).

#[allow(unused_imports)]
use super::*;

impl DiffViewer {
    /// Already loaded rows; observing a test must not initiate IO.
    pub fn reading_split(&self, file: usize) -> Option<&[SplitRow]> {
        self.split.get(&file).map(|rows| rows.as_slice())
    }

    pub fn reading_gliding(&self) -> bool {
        self.glide.is_some()
    }

    /// A head line's measured top, in either reading mode.
    pub fn reading_line_top(&self, file: usize, line: u32) -> Option<Pixels> {
        let segment = self.segment(file);
        let offset = match segment.mode {
            Mode::Unified => {
                (0..segment.rows).find(|&offset| self.unified_lines(file, offset).0 == Some(line))
            }
            Mode::Split => self
                .reading_split(file)?
                .iter()
                .position(|row| row.n == Some(line))
                .map(|i| i as u32 + 1),
            _ => None,
        }?;
        self.list
            .bounds_for_item(self.built.item_of_row(segment.start + offset))
            .map(|b| b.top())
    }

    pub fn reading_first_line(&self) -> Option<(usize, u32, Pixels)> {
        let viewport = self.list.viewport_bounds();
        for item in self.list.logical_scroll_top().item_ix..self.built.items.len() {
            let Some(bounds) = self.list.bounds_for_item(item) else {
                continue;
            };
            if bounds.top() < viewport.top() {
                continue;
            }
            if bounds.top() >= viewport.bottom() {
                break;
            }
            let Item::Row(row) = self.built.items[item] else {
                continue;
            };
            let segment = self.built.segment_at(row)?;
            let offset = row - segment.start;
            let line = match segment.mode {
                Mode::Unified => self.unified_lines(segment.file, offset).0,
                Mode::Split => self
                    .reading_split(segment.file)
                    .and_then(|rows| rows.get(offset.wrapping_sub(1) as usize))
                    .and_then(|row| row.n),
                _ => None,
            };
            if let Some(line) = line {
                return Some((segment.file, line, bounds.top()));
            }
        }
        None
    }
}
