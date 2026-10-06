//! Minimal, stateful fake of the GitHub endpoints WispyDiff calls: PRs, stack lookups, review
//! threads (GraphQL), and posting reviews, file comments, replies and resolves.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::{run_git, Fixture, FixturePr};

#[derive(Default)]
struct State {
    /// PR number → review threads in GitHub's GraphQL shape.
    threads: HashMap<u64, Vec<Value>>,
    /// PR number → review bodies.
    reviews: HashMap<u64, Vec<String>>,
    /// PR number → bodies of every review comment (for reconciliation lookups).
    comments: HashMap<u64, Vec<String>>,
    /// PR number → the user's latest review (`viewerLatestReview`).
    latest_review: HashMap<u64, Value>,
    next_id: u64,
    /// Simulated network outage: every request is dropped without a response.
    offline: bool,
}

impl State {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn comment(&mut self, pr: u64, author: &str, body: &str) -> Value {
        let id = self.id();
        self.comments.entry(pr).or_default().push(body.to_string());
        json!({
            "id": format!("PRRC_{id}"), "databaseId": id, "body": body, "bodyHTML": body_html(body), "isMinimized": false, "createdAt": "2026-09-30T10:00:00Z",
            "url": format!("https://github.com/wispy/fixture/pull/{pr}#discussion_r{id}"), "author": { "login": author }
        })
    }

    fn thread(&mut self, pr: u64, path: &str, line: Option<u64>, start: Option<u64>, side: &str, file_level: bool, first: Value) {
        let id = self.id();
        self.threads.entry(pr).or_default().push(json!({
            "id": format!("PRRT_{id}"), "isResolved": false, "isOutdated": false, "path": path,
            "line": line, "startLine": start, "originalLine": line, "diffSide": side,
            "subjectType": if file_level { "FILE" } else { "LINE" },
            "comments": { "nodes": [first] }
        }));
    }
}

/// A stand-in for GitHub's Markdown rendering: paragraphs of escaped text, HTML comments dropped,
/// `**bold**` as `<strong>`.
fn body_html(body: &str) -> String {
    let mut text = body.to_string();
    while let Some(start) = text.find("<!--") {
        let end = text[start..].find("-->").map_or(text.len(), |e| start + e + 3);
        text.replace_range(start..end, "");
    }
    let escaped = text.trim().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let bold = escaped.split("**").enumerate().map(|(i, part)| if i % 2 == 1 { format!("<strong>{part}</strong>") } else { part.to_string() });
    let html: String = bold.collect();
    html.split("\n\n").map(|p| format!("<p>{}</p>", p.replace('\n', "<br>\n"))).collect()
}

pub fn serve(fixture: Fixture, port: u16) {
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| panic!("bind 127.0.0.1:{port}: {e}"));
    eprintln!("fake GitHub API for {}/{} on http://127.0.0.1:{port}", fixture.owner, fixture.repo);
    let state = Arc::new(Mutex::new(seed(&fixture)));
    let fixture = Arc::new(fixture);
    for stream in listener.incoming().flatten() {
        let (fixture, state) = (fixture.clone(), state.clone());
        std::thread::spawn(move || handle(stream, &fixture, &state));
    }
}

/// A teammate's open thread on the first line PR 2 adds, so there's something to reply to.
fn seed(fixture: &Fixture) -> State {
    let mut state = State::default();
    let Some(pr) = fixture.prs.iter().find(|p| p.number == 2) else { return state };
    let diff = run_git(&fixture.origin, &["diff", "-U0", &pr.base_ref, &pr.head_ref, "--", "src/Module0/Service0.php"]);
    let line = diff
        .lines()
        .find_map(|l| l.strip_prefix("@@ ")?.split(" +").nth(1)?.split([',', ' ']).next()?.parse::<u64>().ok());
    if let Some(line) = line {
        let first = state.comment(2, "teammate", "Is this **threshold** right?");
        state.thread(2, "src/Module0/Service0.php", Some(line), None, "RIGHT", false, first);
    }
    state
}

fn handle(mut stream: TcpStream, fixture: &Fixture, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut length = 0usize;
    let mut header = String::new();
    while reader.read_line(&mut header).map(|n| n > 0).unwrap_or(false) && header != "\r\n" {
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        header.clear();
    }
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    let mut parts = request_line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    {
        let mut state = state.lock().unwrap();
        match target {
            "/__offline" => state.offline = true,
            "/__online" => state.offline = false,
            _ if state.offline => return, // drop the connection, like a dead network
            _ => {}
        }
    }
    let (status, response) = route(method, target, &body, fixture, &mut state.lock().unwrap());
    let response = response.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
}

fn route(method: &str, target: &str, body: &Value, fixture: &Fixture, state: &mut State) -> (&'static str, Value) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let param = |key: &str| query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=')).map(decode);
    let pulls = format!("/repos/{}/{}/pulls", fixture.owner, fixture.repo);
    let not_found = ("404 Not Found", json!({ "message": "Not Found" }));

    if path == "/user" {
        return ("200 OK", json!({ "login": "you" }));
    }
    if path == "/graphql" {
        return graphql(body, fixture, state);
    }
    if path == pulls {
        let owner_prefix = format!("{}:", fixture.owner);
        let head = param("head").map(|h| h.strip_prefix(&owner_prefix).map(str::to_string).unwrap_or(h));
        let base = param("base");
        let matching: Vec<Value> = fixture
            .prs
            .iter()
            .filter(|pr| head.as_ref().is_none_or(|h| &pr.head_ref == h))
            .filter(|pr| base.as_ref().is_none_or(|b| &pr.base_ref == b))
            .map(|pr| pull_json(pr, fixture))
            .collect();
        return ("200 OK", Value::Array(matching));
    }
    let Some(rest) = path.strip_prefix(&format!("{pulls}/")) else { return not_found };
    let mut segments = rest.split('/');
    let Some(number) = segments.next().and_then(|n| n.parse::<u64>().ok()) else { return not_found };
    let Some(pr) = fixture.prs.iter().find(|pr| pr.number == number) else { return not_found };
    let text = |key: &str| body[key].as_str().unwrap_or_default().to_string();

    match (method, segments.next(), segments.next(), segments.next()) {
        ("GET", None, _, _) => ("200 OK", pull_json(pr, fixture)),
        ("GET", Some("reviews"), None, _) => {
            let reviews: Vec<Value> = state.reviews.get(&number).into_iter().flatten().enumerate().map(|(i, b)| json!({ "id": i + 1, "body": b })).collect();
            ("200 OK", Value::Array(reviews))
        }
        ("GET", Some("comments"), None, _) => {
            let comments: Vec<Value> = state.comments.get(&number).into_iter().flatten().map(|b| json!({ "body": b })).collect();
            ("200 OK", Value::Array(comments))
        }
        ("POST", Some("reviews"), None, _) => {
            if text("event").is_empty() || (body["comments"].as_array().is_none_or(Vec::is_empty) && text("body").trim().is_empty() && text("event") == "COMMENT") {
                return ("422 Unprocessable Entity", json!({ "message": "Review cannot be empty" }));
            }
            for c in body["comments"].as_array().into_iter().flatten() {
                let first = state.comment(number, "you", c["body"].as_str().unwrap_or_default());
                let side = c["side"].as_str().unwrap_or("RIGHT").to_string();
                state.thread(number, c["path"].as_str().unwrap_or_default(), c["line"].as_u64(), c["start_line"].as_u64(), &side, false, first);
            }
            state.reviews.entry(number).or_default().push(text("body"));
            let review_state = match text("event").as_str() {
                "APPROVE" => "APPROVED",
                "REQUEST_CHANGES" => "CHANGES_REQUESTED",
                _ => "COMMENTED",
            };
            state.latest_review.insert(
                number,
                json!({ "state": review_state, "submittedAt": "2026-09-30T10:00:00Z", "commit": { "oid": text("commit_id") } }),
            );
            let id = state.id();
            ("200 OK", json!({ "id": id, "state": text("event") }))
        }
        ("POST", Some("comments"), None, _) => {
            let first = state.comment(number, "you", &text("body"));
            let id = first["databaseId"].clone();
            state.thread(number, &text("path"), None, None, "RIGHT", true, first);
            ("201 Created", json!({ "id": id }))
        }
        ("POST", Some("comments"), Some(comment), Some("replies")) => {
            let Ok(parent) = comment.parse::<u64>() else { return not_found };
            let reply = state.comment(number, "you", &text("body"));
            let id = reply["databaseId"].clone();
            let thread = state.threads.get_mut(&number).into_iter().flatten().find(|t| {
                t["comments"]["nodes"].as_array().is_some_and(|c| c.iter().any(|c| c["databaseId"].as_u64() == Some(parent)))
            });
            match thread {
                Some(thread) => {
                    thread["comments"]["nodes"].as_array_mut().unwrap().push(reply);
                    ("201 Created", json!({ "id": id }))
                }
                None => not_found,
            }
        }
        _ => not_found,
    }
}

fn search_nodes(fixture: &Fixture, state: &State, numbers: &[u64]) -> Vec<Value> {
    fixture
        .prs
        .iter()
        .enumerate()
        .filter(|(_, pr)| numbers.contains(&pr.number))
        .map(|(i, pr)| {
            json!({
                "number": pr.number, "title": pr.title, "isDraft": false,
                "url": format!("https://github.com/{}/{}/pull/{}", fixture.owner, fixture.repo, pr.number),
                "updatedAt": format!("2026-09-{:02}T10:00:00Z", 10 + i),
                "headRefName": pr.head_ref, "baseRefName": pr.base_ref,
                "headRefOid": run_git(&fixture.origin, &["rev-parse", &format!("refs/pull/{}/head", pr.number)]),
                "author": { "login": if pr.number == 1 { "you" } else { "fixture-bot" } },
                "repository": { "nameWithOwner": format!("{}/{}", fixture.owner, fixture.repo) },
                "viewerLatestReview": state.latest_review.get(&pr.number).cloned().unwrap_or(Value::Null)
            })
        })
        .collect()
}

fn graphql(body: &Value, fixture: &Fixture, state: &mut State) -> (&'static str, Value) {
    let query = body["query"].as_str().unwrap_or_default();
    let variables = &body["variables"];
    if query.contains("resolveReviewThread") {
        let id = variables["id"].as_str().unwrap_or_default();
        for thread in state.threads.values_mut().flatten() {
            if thread["id"].as_str() == Some(id) {
                thread["isResolved"] = json!(true);
            }
        }
        return ("200 OK", json!({ "data": { "resolveReviewThread": { "thread": { "id": id, "isResolved": true } } } }));
    }
    if query.contains("search(") {
        // Review requested on the stack's upper PRs and the solo PR; the bottom PR is "mine".
        let q = variables["q"].as_str().unwrap_or_default();
        let numbers: &[u64] = if q.contains("review-requested:@me") { &[2, 3, 4, 5] } else { &[1] };
        let nodes = search_nodes(fixture, state, numbers);
        return ("200 OK", json!({ "data": { "search": { "nodes": nodes } } }));
    }
    if query.contains("reviewThreads") {
        let number = variables["number"].as_u64().unwrap_or_default();
        let nodes = state.threads.get(&number).cloned().unwrap_or_default();
        return (
            "200 OK",
            json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
                "pageInfo": { "hasNextPage": false, "endCursor": null }, "nodes": nodes
            } } } } }),
        );
    }
    ("200 OK", json!({ "errors": [{ "message": "unsupported query" }] }))
}

fn pull_json(pr: &FixturePr, fixture: &Fixture) -> Value {
    let repo = json!({
        "clone_url": format!("file://{}", fixture.origin.display()),
        "full_name": format!("{}/{}", fixture.owner, fixture.repo),
        "default_branch": "main",
    });
    let base_sha = run_git(&fixture.origin, &["rev-parse", &pr.base_ref]);
    // Live, so tests can push to the fixture origin while the app runs.
    let head_sha = run_git(&fixture.origin, &["rev-parse", &format!("refs/pull/{}/head", pr.number)]);
    json!({
        "number": pr.number,
        "title": pr.title,
        "state": "open",
        "draft": false,
        "merged_at": null,
        "html_url": format!("https://github.com/{}/{}/pull/{}", fixture.owner, fixture.repo, pr.number),
        "user": { "login": "fixture-bot" },
        "base": { "ref": pr.base_ref, "sha": base_sha, "repo": repo },
        "head": { "ref": pr.head_ref, "sha": head_sha, "repo": repo },
    })
}

/// Minimal percent-decoding for branch names in query strings (`stack%2F1` → `stack/1`).
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
