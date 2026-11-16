//! Read-only observations for progress/intelligence/assistant root tests.
use super::*;
impl DiffViewer {
    pub fn test_since_c(&self) -> bool {
        self.since
    }
    pub fn test_split_c(&self, file: usize) -> Option<&[SplitRow]> {
        self.split.get(&file).map(|s| s.as_slice())
    }
}
