//! Prints core timings for the synthetic worst case:
//! `cargo run --release -p wispy-core --example core_timings -- e2e/.fixtures/stack/origin`
use std::time::Instant;

use wispy_core::cache::Cache;
use wispy_core::highlight::Highlighter;
use wispy_core::model::DiffView;
use wispy_core::repo_store::RepoStore;

fn main() {
    let origin = std::env::args().nth(1).expect("path to fixture origin");
    let origin = std::fs::canonicalize(origin).unwrap();
    let data = std::env::temp_dir().join(format!("wispy-timings-{}", std::process::id()));
    let store = RepoStore::new(data.join("repos"), None);

    let t = Instant::now();
    let highlighter = Highlighter::new();
    println!("highlighter init      {:>7.1} ms", ms(t));

    let t = Instant::now();
    let git = store.ensure_repo("wispy", "fixture", &format!("file://{}", origin.display())).unwrap();
    let fetched = store.fetch_pr(&git, 1, "main").unwrap();
    println!("blobless fetch        {:>7.1} ms", ms(t));

    let t = Instant::now();
    let view = DiffView::compute(&git, &fetched.merge_base, &fetched.head_sha, &highlighter).unwrap();
    println!(
        "cold compute          {:>7.1} ms  ({} files, {} rows, +{} -{})",
        ms(t),
        view.summary.files.len(),
        view.summary.total_rows,
        view.summary.additions,
        view.summary.deletions
    );

    let cache = Cache::open(&data.join("cache.sqlite")).unwrap();
    let t = Instant::now();
    cache.put_diff_view("wispy/fixture", &view).unwrap();
    println!("cache write           {:>7.1} ms", ms(t));

    let t = Instant::now();
    let cached = cache.diff_view("wispy/fixture", &fetched.merge_base, &fetched.head_sha).unwrap().unwrap();
    println!("cache read (cold open){:>7.1} ms", ms(t));

    let t = Instant::now();
    let summary_json = serde_json::to_vec(&cached.summary).unwrap();
    let page_json = serde_json::to_vec(cached.rows(0, 512)).unwrap();
    println!(
        "summary + page json   {:>7.1} ms  ({} KB + {} KB)",
        ms(t),
        summary_json.len() / 1024,
        page_json.len() / 1024
    );
    std::fs::remove_dir_all(data).ok();
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
