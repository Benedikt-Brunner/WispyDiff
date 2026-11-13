//! The file list as a directory tree, in the order the diff shows its files.
use std::collections::HashSet;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDir {
    /// Full path of the directory (the key for collapsing it).
    pub path: String,
    /// Shown name: chains of single-child directories are merged (`src/Module0`).
    pub name: String,
    pub children: Vec<TreeNode>,
    /// Indices of every file below this directory.
    pub files: Vec<usize>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    Dir(TreeDir),
    File(usize),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow<'a> {
    pub depth: usize,
    pub node: &'a TreeNode,
}

/// `None` paths are left out (filtered), keeping the other files' indices.
pub fn build_tree(paths: &[Option<String>]) -> Vec<TreeNode> {
    fn insert(nodes: &mut Vec<TreeNode>, parts: &[&str], prefix: &str, file: usize) {
        if parts.len() == 1 {
            nodes.push(TreeNode::File(file));
            return;
        }
        let path = if prefix.is_empty() {
            parts[0].to_string()
        } else {
            format!("{prefix}/{}", parts[0])
        };
        let index = nodes
            .iter()
            .position(|n| matches!(n, TreeNode::Dir(d) if d.path == path))
            .unwrap_or_else(|| {
                nodes.push(TreeNode::Dir(TreeDir {
                    path: path.clone(),
                    name: parts[0].into(),
                    children: vec![],
                    files: vec![],
                }));
                nodes.len() - 1
            });
        if let TreeNode::Dir(dir) = &mut nodes[index] {
            dir.files.push(file);
            insert(&mut dir.children, &parts[1..], &path, file);
        }
    }
    fn compress(node: TreeNode) -> TreeNode {
        let TreeNode::Dir(mut dir) = node else {
            return node;
        };
        while dir.children.len() == 1 && matches!(dir.children[0], TreeNode::Dir(_)) {
            let TreeNode::Dir(mut only) = dir.children.remove(0) else {
                unreachable!()
            };
            only.name = format!("{}/{}", dir.name, only.name);
            dir = only;
        }
        dir.children = dir.children.into_iter().map(compress).collect();
        TreeNode::Dir(dir)
    }
    let mut nodes = vec![];
    for (index, path) in paths.iter().enumerate() {
        if let Some(path) = path {
            insert(&mut nodes, &path.split('/').collect::<Vec<_>>(), "", index);
        }
    }
    nodes.into_iter().map(compress).collect()
}
/// The rows shown: every node, minus what's inside collapsed directories.
pub fn tree_rows<'a>(nodes: &'a [TreeNode], collapsed: &HashSet<String>) -> Vec<TreeRow<'a>> {
    fn visit<'a>(
        nodes: &'a [TreeNode],
        collapsed: &HashSet<String>,
        depth: usize,
        out: &mut Vec<TreeRow<'a>>,
    ) {
        for node in nodes {
            out.push(TreeRow { depth, node });
            if let TreeNode::Dir(dir) = node {
                if !collapsed.contains(&dir.path) {
                    visit(&dir.children, collapsed, depth + 1, out);
                }
            }
        }
    }
    let mut out = vec![];
    visit(nodes, collapsed, 0, &mut out);
    out
}
/// The directories a file sits in (`a/b/c.ts` → `a`, `a/b`).
pub fn ancestors(path: &str) -> Vec<String> {
    path.match_indices('/')
        .map(|(i, _)| path[..i].to_string())
        .collect()
}
/// Every directory in the tree (after chain compression).
pub fn all_dirs(nodes: &[TreeNode]) -> Vec<&TreeDir> {
    let mut out = vec![];
    for node in nodes {
        if let TreeNode::Dir(dir) = node {
            out.push(dir);
            out.extend(all_dirs(&dir.children));
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compress_filter_and_fold() {
        let paths = [
            Some("src/a/b.rs".into()),
            None,
            Some("src/a/c.rs".into()),
            Some("root".into()),
        ];
        let tree = build_tree(&paths);
        let dirs = all_dirs(&tree);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].name, "src/a");
        assert_eq!(dirs[0].path, "src/a");
        assert_eq!(dirs[0].files, [0, 2]);
        let rows = tree_rows(&tree, &HashSet::new());
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].depth, 1);
        assert_eq!(tree_rows(&tree, &HashSet::from(["src/a".into()])).len(), 2);
        assert_eq!(ancestors("a/b/c.ts"), ["a", "a/b"]);
        assert!(ancestors("root").is_empty());
        assert!(build_tree(&[]).is_empty());
    }
    #[test]
    fn preserves_input_order_and_branches() {
        let paths = ["root", "a/b/x", "a/c/y", "a/z"].map(|p| Some(p.into()));
        let tree = build_tree(&paths);
        assert_eq!(tree[0], TreeNode::File(0));
        assert_eq!(
            all_dirs(&tree)
                .iter()
                .map(|d| d.path.as_str())
                .collect::<Vec<_>>(),
            ["a", "a/b", "a/c"]
        );
    }
}
