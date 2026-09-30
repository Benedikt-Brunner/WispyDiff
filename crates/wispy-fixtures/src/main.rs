//! `wispy-fixtures generate --out DIR [--seed N]` builds a deterministic repository with a
//! four-PR stack (the bottom PR is the ~500 files / ~50k changed lines worst case, including
//! one 20k-line file) and publishes the PRs as `refs/pull/<n>/head`.
//!
//! `wispy-fixtures serve --fixture DIR --port P` answers the GitHub REST calls WispyDiff
//! makes for that repository, with `clone_url` pointing at the local origin.

mod content;
mod server;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use content::{Lang, Rng, SourceFile};

pub const OWNER: &str = "wispy";
pub const REPO: &str = "fixture";

#[derive(Serialize, Deserialize)]
pub struct Fixture {
    pub owner: String,
    pub repo: String,
    pub origin: PathBuf,
    pub prs: Vec<FixturePr>,
}

#[derive(Serialize, Deserialize)]
pub struct FixturePr {
    pub number: u64,
    pub title: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_sha: String,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    match args.first().map(String::as_str) {
        Some("generate") => {
            let out = PathBuf::from(flag("--out").expect("--out DIR is required"));
            let seed = flag("--seed").map(|s| s.parse().expect("--seed must be a number")).unwrap_or(42);
            let fixture = generate(&out, seed);
            println!("{}", serde_json::to_string_pretty(&fixture).unwrap());
        }
        Some("serve") => {
            let dir = PathBuf::from(flag("--fixture").expect("--fixture DIR is required"));
            let port: u16 = flag("--port").map(|p| p.parse().expect("--port must be a number")).unwrap_or(4600);
            let fixture: Fixture =
                serde_json::from_slice(&std::fs::read(dir.join("fixture.json")).expect("run `generate` first"))
                    .expect("valid fixture.json");
            server::serve(fixture, port);
        }
        _ => {
            eprintln!("usage: wispy-fixtures generate --out DIR [--seed N] | serve --fixture DIR [--port P]");
            std::process::exit(2);
        }
    }
}

fn generate(out: &Path, seed: u64) -> Fixture {
    let origin = out.join("origin");
    if origin.exists() {
        std::fs::remove_dir_all(&origin).unwrap();
    }
    std::fs::create_dir_all(&origin).unwrap();
    let origin = origin.canonicalize().unwrap();
    let git = |args: &[&str]| run_git(&origin, args);

    git(&["init", "--quiet", "-b", "main"]);
    git(&["config", "uploadpack.allowFilter", "true"]);
    git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);

    let mut rng = Rng::new(seed);
    let langs = [(Lang::Php, 170), (Lang::Ts, 150), (Lang::Vue, 100), (Lang::Twig, 80)];
    let mut files: Vec<SourceFile> = Vec::new();
    for (lang, count) in langs {
        for i in 0..count {
            files.push(SourceFile::generate(lang, i, 150, &mut rng));
        }
    }
    for file in &files {
        file.write(&origin);
    }
    commit(&origin, "Initial import", 0);

    // PR 1 — the worst case: touch 480 of the 500 files, add a 20k-line file and 19 small ones.
    git(&["checkout", "--quiet", "-b", "stack/1"]);
    for file in files.iter_mut().take(480) {
        file.mutate(40, &mut rng);
        file.write(&origin);
    }
    SourceFile::generate(Lang::Php, 10_000, 20_000, &mut rng).write(&origin);
    for i in 0..19 {
        SourceFile::generate(Lang::Ts, 20_000 + i, 60, &mut rng).write(&origin);
    }
    commit(&origin, "Rework pick list allocation", 1);

    // PRs 2–4 — small layers on top.
    let mut prs = vec![("stack/1", "main", "Rework pick list allocation")];
    let layers = [("stack/2", "Add bin capacity checks"), ("stack/3", "Show capacity in admin"), ("stack/4", "Capacity docs and tests")];
    for (layer, (branch, title)) in layers.iter().enumerate() {
        git(&["checkout", "--quiet", "-b", branch]);
        for offset in 0..20 {
            let index = (layer * 97 + offset * 13) % files.len();
            files[index].mutate(4, &mut rng);
            files[index].write(&origin);
        }
        commit(&origin, title, 2 + layer as u64);
        prs.push((branch, prs.last().unwrap().0, title));
    }
    git(&["checkout", "--quiet", "main"]);

    let prs: Vec<FixturePr> = prs
        .into_iter()
        .enumerate()
        .map(|(i, (head_ref, base_ref, title))| {
            let number = i as u64 + 1;
            git(&["update-ref", &format!("refs/pull/{number}/head"), head_ref]);
            FixturePr {
                number,
                title: title.to_string(),
                base_ref: base_ref.to_string(),
                head_ref: head_ref.to_string(),
                head_sha: git(&["rev-parse", head_ref]),
            }
        })
        .collect();

    let fixture = Fixture { owner: OWNER.into(), repo: REPO.into(), origin, prs };
    std::fs::write(out.join("fixture.json"), serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    fixture
}

fn commit(repo: &Path, message: &str, day: u64) {
    run_git(repo, &["add", "-A"]);
    let date = format!("2026-01-{:02}T10:00:00Z", day + 1);
    let out = Command::new("git")
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture Bot")
        .env("GIT_AUTHOR_EMAIL", "fixture@wispydiff.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture Bot")
        .env("GIT_COMMITTER_EMAIL", "fixture@wispydiff.invalid")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .args(["commit", "--quiet", "--no-gpg-sign", "-m", message])
        .output()
        .unwrap();
    assert!(out.status.success(), "commit failed: {}", String::from_utf8_lossy(&out.stderr));
}

pub fn run_git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}
