//! Files collapsed by default because reviewing them line by line is rarely useful.

use std::collections::HashSet;

use crate::error::Result;
use crate::git::Git;

const LOCKFILES: &[&str] = &[
    "composer.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "Cargo.lock",
    "Gemfile.lock",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "go.sum",
];

/// Why `path` is noise judging by its name alone.
pub fn path_noise(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if LOCKFILES.contains(&name) {
        Some("lockfile")
    } else if [".min.js", ".min.css", ".min.mjs", ".js.map", ".css.map"].iter().any(|ext| name.ends_with(ext)) {
        Some("minified")
    } else if name.ends_with(".snap") || path.split('/').any(|dir| dir == "__snapshots__") {
        Some("snapshot")
    } else {
        None
    }
}

/// Paths marked `linguist-generated` by the `.gitattributes` in `commit`.
pub fn generated_paths(git: &Git, commit: &str, paths: &[&str]) -> Result<HashSet<String>> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut input = paths.join("\0");
    input.push('\0');
    let source = format!("--source={commit}");
    let out = git.run_with_stdin(&["check-attr", "-z", "--stdin", &source, "linguist-generated"], input.as_bytes())?;
    // Output: `<path>\0<attribute>\0<value>\0` per path.
    let fields: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    Ok(fields
        .chunks(3)
        .filter(|chunk| chunk.len() == 3 && matches!(chunk[2], b"set" | b"true"))
        .map(|chunk| String::from_utf8_lossy(chunk[0]).into_owned())
        .collect())
}
