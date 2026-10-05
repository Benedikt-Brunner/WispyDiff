/** The file list as a directory tree, in the order the diff shows its files. */

export interface TreeDir {
  /** Full path of the directory (the key for collapsing it). */
  path: string;
  /** Shown name: chains of single-child directories are merged ("src/Module0"). */
  name: string;
  children: TreeNode[];
  /** Indices of every file below this directory. */
  files: number[];
}

export type TreeNode = { dir: TreeDir } | { file: number };

export type TreeRow = { depth: number } & ({ dir: TreeDir } | { file: number });

export function buildTree(paths: string[]): TreeNode[] {
  const root: TreeDir = { path: "", name: "", children: [], files: [] };
  const dirs = new Map<string, TreeDir>([["", root]]);
  paths.forEach((path, index) => {
    const parts = path.split("/");
    let parent = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const dirPath = parts.slice(0, i + 1).join("/");
      let dir = dirs.get(dirPath);
      if (!dir) {
        dir = { path: dirPath, name: parts[i], children: [], files: [] };
        dirs.set(dirPath, dir);
        parent.children.push({ dir });
      }
      parent.files.push(index);
      parent = dir;
    }
    parent.files.push(index);
    parent.children.push({ file: index });
  });
  return root.children.map(compress);
}

/** Merges a directory holding nothing but one directory into it. */
function compress(node: TreeNode): TreeNode {
  if (!("dir" in node)) return node;
  let dir = node.dir;
  while (dir.children.length === 1 && "dir" in dir.children[0]) {
    const only = dir.children[0].dir;
    dir = { ...only, name: `${dir.name}/${only.name}` };
  }
  return { dir: { ...dir, children: dir.children.map(compress) } };
}

/** The rows shown: every node, minus what's inside collapsed directories. */
export function treeRows(nodes: TreeNode[], collapsed: Set<string>, depth = 0, out: TreeRow[] = []): TreeRow[] {
  for (const node of nodes) {
    out.push({ ...node, depth });
    if ("dir" in node && !collapsed.has(node.dir.path)) treeRows(node.dir.children, collapsed, depth + 1, out);
  }
  return out;
}

/** The directories a file sits in ("a/b/c.ts" → "a", "a/b"). */
export function ancestors(path: string): string[] {
  const parts = path.split("/");
  return parts.slice(1).map((_, i) => parts.slice(0, i + 1).join("/"));
}

/** Every directory in the tree. */
export function allDirs(nodes: TreeNode[], out: TreeDir[] = []): TreeDir[] {
  for (const node of nodes) {
    if ("dir" in node) {
      out.push(node.dir);
      allDirs(node.dir.children, out);
    }
  }
  return out;
}
