//! The assistant's view of the changes: the first prompt lists the changed files, and the
//! `file_diff` tool shows their diffs. Its own test binary: it points the CLI variables at the
//! fakes without racing the other assistant tests' logs.

mod support;

use std::path::PathBuf;

use serde_json::json;
use support::{pull_request, OriginRepo};
use wispy_core::assistant::Provider;
use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::{NewThread, PrService};
use wispy_core::stack::Stack;

#[test]
fn lists_the_changed_files_in_the_prompt_and_shows_their_diffs_on_request() {
    let log = tempfile::NamedTempFile::new().unwrap();
    // SAFETY: the only test in this binary that reads them.
    unsafe {
        std::env::set_var("WISPY_CLAUDE_BIN", fake("claude"));
        std::env::set_var("WISPY_FAKE_LOG", log.path());
    }

    let origin = OriginRepo::init();
    origin.write("src/Order.php", "<?php\na\nb\n");
    origin.write("src/Old.php", "<?php\nold\n");
    origin.write("README.md", "# Shop\n");
    origin.commit("base");
    origin.checkout_new("s/1");
    origin.write("src/Order.php", "<?php\nA1\nb\n");
    origin.commit("one");
    origin.publish_pr(1, "s/1");
    origin.checkout_new("s/2");
    origin.write("src/Order.php", "<?php\nA1\nB2\n");
    origin.remove("src/Old.php");
    origin.write("src/New.php", "<?php\nold\n");
    origin.write("docs/notes.md", "hidden\n");
    origin.commit("two");
    origin.publish_pr(2, "s/2");
    let url = origin.url();
    let data = tempfile::tempdir().unwrap();
    let service = PrService::new(
        GitHubClient::new("http://127.0.0.1:9", "t").unwrap(),
        RepoStore::new(data.path().join("repos"), None),
        Cache::open(&data.path().join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let snapshot = service.fetch(Stack { prs: vec![pull_request(1, "main", "s/1", &url), pull_request(2, "s/1", "s/2", &url)], focus: 0 }).unwrap();

    let steps = json!([
        ["file_diff", { "path": "src/Order.php" }],
        ["file_diff", { "path": "src/Order.php", "pr": 1 }],
        ["file_diff", { "path": "src/New.php", "pr": 2 }],
        ["file_diff", { "path": "src/New.php", "pr": 1 }],
        ["file_diff", { "path": "src/Order.php", "pr": 7 }],
        ["file_diff", { "path": "README.md" }],
    ]);
    let new = NewThread { provider: Provider::Claude, model: None, effort: None, selection: None, anchor: None, hidden: vec!["docs/notes.md".into()] };
    let thread = service.ask(&snapshot, (0, 1), None, Some(new), &format!("tools:{steps}"), |_| {}).unwrap();

    // The prompt names the changed files without their changes; hidden ones apart.
    let prompt = last_prompt(log.path());
    assert!(!prompt.contains("diff --git"), "{prompt}");
    assert!(prompt.contains("\n- src/New.php (renamed from src/Old.php, +0 −0)\n"), "{prompt}");
    assert!(prompt.contains("\n- src/Order.php (modified, +2 −2, in #1 #2)\n"), "{prompt}");
    assert!(prompt.contains("hid these changed files from their view; look at them only if the question needs them:\n- docs/notes.md\n"), "{prompt}");
    assert!(!prompt.contains("- docs/notes.md ("), "{prompt}");

    let answer = &thread.messages.last().unwrap().text;
    let calls: Vec<&str> = answer.split("\nfile_diff: ").collect();
    assert_eq!(calls.len(), 6, "{answer}");
    assert!(calls[0].contains("-a\n-b\n+A1\n+B2"), "across the range: {answer}");
    assert!(calls[1].contains("-a\n+A1\n b") && !calls[1].contains("B2"), "only #1's changes: {answer}");
    assert!(calls[2].contains("rename from src/Old.php\nrename to src/New.php"), "{answer}");
    assert_eq!(calls[3], "error: #1 doesn't change src/New.php");
    assert_eq!(calls[4], "error: #7 isn't one of the PRs under review");
    assert_eq!(calls[5], "error: README.md isn't changed in this range");
}

fn fake(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../e2e/fake-cli").join(name).canonicalize().unwrap()
}

fn last_prompt(log: &std::path::Path) -> String {
    let entries = std::fs::read_to_string(log).unwrap_or_default();
    let last = entries.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|c| c.get("prompt").is_some()).last();
    last.map(|c| c["prompt"].as_str().unwrap().to_string()).unwrap_or_default()
}
