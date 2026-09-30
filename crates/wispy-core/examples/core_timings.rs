//! Prints core timings for the synthetic worst case (a 4-PR stack whose bottom PR has ~500
//! files / ~50k changed lines):
//! `cargo run --release -p wispy-core --example core_timings -- e2e/.fixtures/stack/origin`
use std::time::Instant;

use wispy_core::cache::Cache;
use wispy_core::github::PullRequest;
use wispy_core::highlight::Highlighter;
use wispy_core::highlight_cache::HighlightCache;
use wispy_core::range::compute_range;
use wispy_core::repo_store::RepoStore;
use wispy_core::stack::{Stack, StackSnapshot};

fn pr(number: u64, base_ref: &str, head_ref: &str, clone_url: &str) -> PullRequest {
    PullRequest {
        number,
        title: String::new(),
        state: "open".into(),
        draft: false,
        html_url: String::new(),
        author: String::new(),
        base_ref: base_ref.into(),
        base_sha: String::new(),
        head_ref: head_ref.into(),
        head_sha: String::new(),
        clone_url: clone_url.into(),
        base_repo: "wispy/fixture".into(),
        head_repo: Some("wispy/fixture".into()),
        default_branch: "main".into(),
    }
}

fn main() {
    let origin = std::fs::canonicalize(std::env::args().nth(1).expect("path to fixture origin")).unwrap();
    let url = format!("file://{}", origin.display());
    let data = std::env::temp_dir().join(format!("wispy-timings-{}", std::process::id()));
    let store = RepoStore::new(data.join("repos"), None);

    let t = Instant::now();
    let highlighter = Highlighter::new();
    println!("highlighter init        {:>7.1} ms", ms(t));

    let stack = Stack {
        prs: vec![
            pr(1, "main", "stack/1", &url),
            pr(2, "stack/1", "stack/2", &url),
            pr(3, "stack/2", "stack/3", &url),
            pr(4, "stack/3", "stack/4", &url),
        ],
        focus: 0,
    };
    let t = Instant::now();
    let git = store.ensure_repo("wispy", "fixture", &url).unwrap();
    let snapshot = StackSnapshot::new(stack.clone(), store.fetch_stack(&git, &stack.prs).unwrap());
    println!("blobless stack fetch    {:>7.1} ms", ms(t));

    let highlights = HighlightCache::default();
    let t = Instant::now();
    let view = compute_range(&git, &snapshot.range(0, 0), &highlighter, &highlights).unwrap();
    println!(
        "cold compute #1         {:>7.1} ms  ({} files, {} rows, +{} -{})",
        ms(t),
        view.summary.files.len(),
        view.summary.total_rows,
        view.summary.additions,
        view.summary.deletions
    );

    let t = Instant::now();
    let whole = compute_range(&git, &snapshot.range(0, 3), &highlighter, &highlights).unwrap();
    println!("compute #1-#4 (warm hl) {:>7.1} ms  ({} rows, attributed)", ms(t), whole.summary.total_rows);

    let cache = Cache::open(&data.join("cache.sqlite")).unwrap();
    let t = Instant::now();
    cache.put_diff_view("wispy/fixture", &snapshot.range(0, 3), &whole).unwrap();
    println!("cache write             {:>7.1} ms", ms(t));

    let t = Instant::now();
    let cached = cache.diff_view("wispy/fixture", &snapshot.range(0, 3)).unwrap().unwrap();
    println!("cache read (reopen)     {:>7.1} ms", ms(t));

    let t = Instant::now();
    let summary_json = serde_json::to_vec(&cached.summary).unwrap();
    let page_json = serde_json::to_vec(cached.rows(0, 512)).unwrap();
    println!(
        "summary + page json     {:>7.1} ms  ({} KB + {} KB)",
        ms(t),
        summary_json.len() / 1024,
        page_json.len() / 1024
    );
    std::fs::remove_dir_all(data).ok();
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
