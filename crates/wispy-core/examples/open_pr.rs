//! Opens a PR's stack through the full service, like the app does:
//! `cargo run --release -p wispy-core --example open_pr -- <api-base> <owner/repo#N>`
use std::time::Instant;

use wispy_core::cache::Cache;
use wispy_core::github::GitHubClient;
use wispy_core::highlight::Highlighter;
use wispy_core::pr_ref::PrRef;
use wispy_core::repo_store::RepoStore;
use wispy_core::service::PrService;
use wispy_core::token::resolve_github_token;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (api, pr) = (args.next().expect("api base"), PrRef::parse(&args.next().expect("PR")).unwrap());
    let data = std::env::temp_dir().join(format!("wispy-open-{}", std::process::id()));
    let token = resolve_github_token().unwrap_or_else(|_| "token".into());
    let service = PrService::new(
        GitHubClient::new(api, token.clone()).unwrap(),
        RepoStore::new(data.join("repos"), Some(token)),
        Cache::open(&data.join("cache.sqlite")).unwrap(),
        Highlighter::new(),
    );
    let t = Instant::now();
    let stack = service.discover(&pr).await.unwrap();
    println!("discover {:.0} ms: {:?}", t.elapsed().as_secs_f64() * 1e3, stack.prs.iter().map(|p| p.number).collect::<Vec<_>>());
    let t = Instant::now();
    let snapshot = service.fetch(stack).unwrap();
    println!("fetch {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let focus = snapshot.stack.focus;
    let view = service.range(&snapshot, focus, focus).unwrap();
    println!("range {:.0} ms, {} files", t.elapsed().as_secs_f64() * 1e3, view.summary.files.len());
    std::fs::remove_dir_all(data).ok();
}
