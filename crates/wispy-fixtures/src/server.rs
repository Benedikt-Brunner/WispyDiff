//! Minimal fake of the GitHub REST endpoints WispyDiff calls.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::{run_git, Fixture};

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

    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    let (status, body) = route(path, fixture);
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn route(path: &str, fixture: &Fixture) -> (&'static str, serde_json::Value) {
    let prefix = format!("/repos/{}/{}/pulls/", fixture.owner, fixture.repo);
    let pr = path
        .strip_prefix(&prefix)
        .and_then(|n| n.parse::<u64>().ok())
        .and_then(|n| fixture.prs.iter().find(|pr| pr.number == n));
    let Some(pr) = pr else {
        return ("404 Not Found", serde_json::json!({ "message": "Not Found" }));
    };
    let clone_url = format!("file://{}", fixture.origin.display());
    let base_sha = run_git(&fixture.origin, &["rev-parse", &pr.base_ref]);
    let body = serde_json::json!({
        "number": pr.number,
        "title": pr.title,
        "state": "open",
        "draft": false,
        "merged_at": null,
        "html_url": format!("https://github.com/{}/{}/pull/{}", fixture.owner, fixture.repo, pr.number),
        "user": { "login": "fixture-bot" },
        "base": { "ref": pr.base_ref, "sha": base_sha, "repo": { "clone_url": clone_url } },
        "head": { "ref": pr.head_ref, "sha": pr.head_sha, "repo": { "clone_url": clone_url } },
    });
    ("200 OK", body)
}
