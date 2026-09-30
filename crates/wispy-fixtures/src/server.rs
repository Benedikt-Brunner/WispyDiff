//! Minimal fake of the GitHub REST endpoints WispyDiff calls.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::{run_git, Fixture, FixturePr};

pub fn serve(fixture: Fixture, port: u16) {
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| panic!("bind 127.0.0.1:{port}: {e}"));
    eprintln!("fake GitHub API for {}/{} on http://127.0.0.1:{port}", fixture.owner, fixture.repo);
    let fixture = Arc::new(fixture);
    for stream in listener.incoming().flatten() {
        let fixture = fixture.clone();
        std::thread::spawn(move || handle(stream, &fixture));
    }
}

fn handle(mut stream: TcpStream, fixture: &Fixture) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drain headers; requests carry no body.
    let mut header = String::new();
    while reader.read_line(&mut header).map(|n| n > 0).unwrap_or(false) && header != "\r\n" {
        header.clear();
    }

    let target = request_line.split_whitespace().nth(1).unwrap_or_default();
    let (status, body) = route(target, fixture);
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn route(target: &str, fixture: &Fixture) -> (&'static str, serde_json::Value) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let param = |key: &str| {
        query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=')).map(decode)
    };
    let list = format!("/repos/{}/{}/pulls", fixture.owner, fixture.repo);
    if path == list {
        let owner_prefix = format!("{}:", fixture.owner);
        let head = param("head").map(|h| h.strip_prefix(&owner_prefix).map(str::to_string).unwrap_or(h));
        let base = param("base");
        let matching: Vec<serde_json::Value> = fixture
            .prs
            .iter()
            .filter(|pr| head.as_ref().is_none_or(|h| &pr.head_ref == h))
            .filter(|pr| base.as_ref().is_none_or(|b| &pr.base_ref == b))
            .map(|pr| pull_json(pr, fixture))
            .collect();
        return ("200 OK", serde_json::Value::Array(matching));
    }
    let pr = path
        .strip_prefix(&format!("{list}/"))
        .and_then(|n| n.parse::<u64>().ok())
        .and_then(|n| fixture.prs.iter().find(|pr| pr.number == n));
    match pr {
        Some(pr) => ("200 OK", pull_json(pr, fixture)),
        None => ("404 Not Found", serde_json::json!({ "message": "Not Found" })),
    }
}

fn pull_json(pr: &FixturePr, fixture: &Fixture) -> serde_json::Value {
    let repo = serde_json::json!({
        "clone_url": format!("file://{}", fixture.origin.display()),
        "full_name": format!("{}/{}", fixture.owner, fixture.repo),
        "default_branch": "main",
    });
    let base_sha = run_git(&fixture.origin, &["rev-parse", &pr.base_ref]);
    serde_json::json!({
        "number": pr.number,
        "title": pr.title,
        "state": "open",
        "draft": false,
        "merged_at": null,
        "html_url": format!("https://github.com/{}/{}/pull/{}", fixture.owner, fixture.repo, pr.number),
        "user": { "login": "fixture-bot" },
        "base": { "ref": pr.base_ref, "sha": base_sha, "repo": repo },
        "head": { "ref": pr.head_ref, "sha": pr.head_sha, "repo": repo },
    })
}

/// Minimal percent-decoding for branch names in query strings (`stack%2F1` → `stack/1`).
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
