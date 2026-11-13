//! Locally pasted images stay in the draft until submission uploads them.
/// How a draft refers to a pasted image until submitting uploads it.
pub const IMAGE_SCHEME: &str = "wispy-image:";
/// Exact, case-sensitive scheme check and suffix extraction used by Markdown.tsx.
/// Empty suffixes are preserved so the image loader can report a missing image.
pub fn image_id(src: &str) -> Option<&str> {
    src.strip_prefix(IMAGE_SCHEME)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_scheme() {
        assert_eq!(image_id("wispy-image:abc"), Some("abc"));
        assert_eq!(image_id(IMAGE_SCHEME), Some(""));
        assert_eq!(image_id("https://example/a.png"), None);
        assert_eq!(image_id("Wispy-image:abc"), None);
    }
}
