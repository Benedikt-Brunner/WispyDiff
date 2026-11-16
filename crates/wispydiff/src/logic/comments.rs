//! Review-comment labels and path routing, using the core's comment types.
use wispy_core::{
    anchors::Anchor,
    github::{PullRequest, Side},
    model::FileSummary,
};
/// A PR across repositories, for per-PR preferences.
pub fn pr_key(pr: &PullRequest) -> String {
    format!("{}#{}", pr.base_repo, pr.number)
}
pub fn anchor_key(a: &Anchor) -> String {
    format!(
        "{}|{}|{}|{}",
        a.pr,
        match a.side {
            Side::Left => "LEFT",
            Side::Right => "RIGHT",
        },
        a.path,
        a.line
    )
}
/// The path a stack PR knew a file under (it may have been renamed later in the stack).
pub fn path_in_pr(file: &FileSummary, pr: u8, side: Side) -> &str {
    if let Some((_, path)) = file.pr_paths.iter().find(|(index, _)| *index == pr) {
        if !path.is_empty() {
            return path;
        }
    }
    if side == Side::Left {
        file.old_path.as_deref().unwrap_or(&file.path)
    } else {
        &file.path
    }
}
pub fn file_index_for(files: &[FileSummary], path: &str) -> Option<usize> {
    files.iter().position(|f| {
        f.path == path
            || f.old_path.as_deref() == Some(path)
            || f.pr_paths.iter().any(|(_, p)| p == path)
    })
}
pub fn line_label(start: Option<u32>, end: Option<u32>) -> String {
    match (start, end) {
        (_, None) => String::new(),
        (Some(s), Some(e)) if s != e => format!("L{s}–{e}"),
        (_, Some(e)) => format!("L{e}"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_labels_keys() {
        let mut f = super::super::test_file();
        f.pr_paths = vec![(0, "earlier.rs".into())];
        assert_eq!(path_in_pr(&f, 0, Side::Left), "earlier.rs");
        assert_eq!(path_in_pr(&f, 1, Side::Left), "old.rs");
        assert_eq!(path_in_pr(&f, 1, Side::Right), "src/new.rs");
        let fs = vec![f];
        for p in ["old.rs", "src/new.rs", "earlier.rs"] {
            assert_eq!(file_index_for(&fs, p), Some(0));
        }
        assert_eq!(file_index_for(&fs, "absent"), None);
        assert_eq!(line_label(Some(2), Some(4)), "L2–4");
        assert_eq!(line_label(Some(4), Some(4)), "L4");
        assert_eq!(line_label(None, Some(4)), "L4");
        assert_eq!(line_label(Some(2), None), "");
        assert_eq!(
            anchor_key(&Anchor {
                pr: 2,
                path: "a|b".into(),
                side: Side::Left,
                line: 7
            }),
            "2|LEFT|a|b|7"
        );
        let mut p: PullRequest=serde_json::from_str(r#"{"number":12,"title":"t","body":"","state":"OPEN","draft":false,"html_url":"","author":"x","head_sha":"h","base_sha":"b","head_ref":"h","base_ref":"b","base_repo":"a/b","head_repo":"a/b","clone_url":"","default_branch":"main","created_at":"","url":""}"#).unwrap();
        assert_eq!(pr_key(&p), "a/b#12");
        p.base_repo = "else/repo".into();
        assert_eq!(pr_key(&p), "else/repo#12");
    }
}
