//! Test fixtures: throwaway "GitHub-side" origin repositories.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A non-bare repository standing in for the GitHub remote. PRs are published as
/// `refs/pull/<n>/head`, exactly like GitHub does.
pub struct OriginRepo {
    pub dir: TempDir,
}

impl OriginRepo {
    pub fn init() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let origin = OriginRepo { dir };
        origin.git(&["init", "--quiet", "-b", "main"]);
        origin.git(&["config", "user.email", "test@example.com"]);
        origin.git(&["config", "user.name", "Test"]);
        origin.git(&["config", "uploadpack.allowFilter", "true"]);
        origin.git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
        origin
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn url(&self) -> String {
        format!("file://{}", self.path().display())
    }

    pub fn write(&self, path: &str, content: &str) -> &Self {
        let full: PathBuf = self.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
        self
    }

    pub fn remove(&self, path: &str) -> &Self {
        self.git(&["rm", "--quiet", path]);
        self
    }

    pub fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "--quiet", "--allow-empty", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn checkout_new(&self, branch: &str) {
        self.git(&["checkout", "--quiet", "-b", branch]);
    }

    pub fn checkout(&self, branch: &str) {
        self.git(&["checkout", "--quiet", branch]);
    }

    pub fn publish_pr(&self, number: u64, branch: &str) {
        self.git(&["update-ref", &format!("refs/pull/{number}/head"), branch]);
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
}

pub fn pull_request_json(number: u64, base_ref: &str, head_ref: &str, head_sha: &str, clone_url: &str) -> serde_json::Value {
    serde_json::json!({
        "number": number,
        "title": format!("PR {number}"),
        "state": "open",
        "draft": false,
        "merged_at": null,
        "html_url": format!("https://github.com/acme/shop/pull/{number}"),
        "user": { "login": "octocat" },
        "base": { "ref": base_ref, "sha": "0000000000000000000000000000000000000000", "repo": { "clone_url": clone_url } },
        "head": { "ref": head_ref, "sha": head_sha, "repo": { "clone_url": clone_url } }
    })
}
