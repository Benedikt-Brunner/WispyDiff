//! Read-only geometry used by the comments/inbox headless integration tests.
use super::*;

impl DiffViewer {
    /// The real insert padding before a thread/draft/composer card.
    pub fn comment_card_indent(&self, cx: &App) -> Pixels {
        let digits = self
            .view
            .summary
            .max_line_number
            .max(1)
            .to_string()
            .len()
            .max(3) as f32;
        let column = self.char_width.unwrap_or(u_px(7.2, cx)) * digits + u_px(12., cx);
        column * 2. + u_px(24., cx)
    }
}
