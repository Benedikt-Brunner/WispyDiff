//! The review assistant: runs the Claude Code or Codex CLI headless, read-only, in a
//! checkout of the range head, and streams its answer back.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    /// The CLI binary; `WISPY_CLAUDE_BIN` / `WISPY_CODEX_BIN` override it (tests use fakes).
    pub fn binary(self) -> String {
        let (var, default) = match self {
            Provider::Claude => ("WISPY_CLAUDE_BIN", "claude"),
            Provider::Codex => ("WISPY_CODEX_BIN", "codex"),
        };
        std::env::var(var).unwrap_or_else(|_| default.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    pub provider: Provider,
    /// `None` = the CLI's configured default.
    pub model: Option<String>,
    pub effort: Option<String>,
    pub prompt: String,
    /// Continue this CLI session (follow-up questions).
    pub resume: Option<String>,
    /// The review tools' MCP server for this turn (see [`crate::mcp`]).
    #[serde(skip)]
    pub tools: Option<crate::mcp::Endpoint>,
}

/// The variable that hands Codex the tools' token (it reads bearer tokens from the environment).
const TOOLS_TOKEN_VAR: &str = "WISPY_MCP_TOKEN";

/// The CLI invocation for `ask`, restricted to reading files (the review tools change only
/// the app's local drafts; the user's own MCP servers stay available). The prompt goes on stdin.
pub fn command(ask: &Ask, cwd: &Path) -> Command {
    let binary = ask.provider.binary();
    let mut cmd = Command::new(&binary);
    // Not on the app's PATH: try the interactive shell's (Command looks the binary up there too).
    if !on_path(&binary) {
        if let Some(path) = crate::shell_env::interactive_shell_path() {
            cmd.env("PATH", path);
        }
    }
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    match ask.provider {
        Provider::Claude => {
            cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
            // Read-only files: the only built-in tools are ones that can't change anything. MCP
            // tools (the user's own servers and the app's review tools) run without asking, since
            // nobody can be asked headless.
            cmd.args(["--tools", "Read,Grep,Glob", "--permission-mode", "bypassPermissions"]);
            if let Some(tools) = &ask.tools {
                let config = serde_json::json!({ "mcpServers": { crate::mcp::SERVER_NAME: {
                    "type": "http", "url": tools.url, "headers": { "Authorization": format!("Bearer {}", tools.token) },
                } } });
                cmd.args(["--mcp-config", &config.to_string()]);
            }
            if let Some(model) = &ask.model {
                cmd.args(["--model", model]);
            }
            if let Some(effort) = &ask.effort {
                cmd.args(["--effort", effort]);
            }
            if let Some(session) = &ask.resume {
                cmd.args(["--resume", session]);
            }
        }
        Provider::Codex => {
            cmd.arg("exec");
            if let Some(session) = &ask.resume {
                cmd.args(["resume", session]);
            }
            cmd.args(["--json", "--skip-git-repo-check", "-c", "sandbox_mode=\"read-only\""]);
            if let Some(tools) = &ask.tools {
                // exec never asks for approval, so the tools are approved up front; required: no
                // silent turn without them.
                let server = format!(
                    "mcp_servers.{}={{url=\"{}\", bearer_token_env_var=\"{TOOLS_TOKEN_VAR}\", default_tools_approval_mode=\"approve\", required=true}}",
                    crate::mcp::SERVER_NAME,
                    tools.url
                );
                cmd.args(["-c", &server]).env(TOOLS_TOKEN_VAR, &tools.token);
            }
            if let Some(model) = &ask.model {
                cmd.args(["-m", model]);
            }
            if let Some(effort) = &ask.effort {
                cmd.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
            }
            cmd.arg("-");
        }
    }
    cmd
}

fn on_path(binary: &str) -> bool {
    if binary.contains('/') {
        return true;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| dir.join(binary).is_file())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AssistantEvent {
    /// The CLI session id (for follow-ups).
    Session { id: String },
    /// More answer text (append).
    Delta { text: String },
    /// The complete answer so far (replaces what was streamed).
    Text { text: String },
    Error { message: String },
    /// The assistant added, changed or deleted a draft (or failed to): drafts changed.
    Comment { outcome: CommentOutcome },
}

/// Events in one line of the CLI's JSON output.
pub fn parse_line(provider: Provider, line: &str) -> Vec<AssistantEvent> {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else { return Vec::new() };
    let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    match provider {
        Provider::Claude => match (event["type"].as_str(), event["subtype"].as_str()) {
            (Some("system"), Some("init")) => vec![AssistantEvent::Session { id: text(&event["session_id"]) }],
            (Some("stream_event"), _) if event["event"]["delta"]["type"] == "text_delta" => {
                vec![AssistantEvent::Delta { text: text(&event["event"]["delta"]["text"]) }]
            }
            (Some("assistant"), _) => {
                let parts: Vec<String> = event["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| c["type"] == "text")
                    .map(|c| text(&c["text"]))
                    .collect();
                if parts.is_empty() {
                    Vec::new()
                } else {
                    vec![AssistantEvent::Text { text: parts.join("") }]
                }
            }
            (Some("result"), _) if event["is_error"].as_bool() == Some(true) => {
                vec![AssistantEvent::Error { message: text(&event["result"]) }]
            }
            _ => Vec::new(),
        },
        Provider::Codex => match event["type"].as_str() {
            Some("thread.started") => vec![AssistantEvent::Session { id: text(&event["thread_id"]) }],
            Some("item.completed" | "item.updated") if event["item"]["type"] == "agent_message" => {
                vec![AssistantEvent::Text { text: text(&event["item"]["text"]) }]
            }
            Some("error") => vec![AssistantEvent::Error { message: api_message(&text(&event["message"])) }],
            Some("turn.failed") => vec![AssistantEvent::Error { message: api_message(&text(&event["error"]["message"])) }],
            _ => Vec::new(),
        },
    }
}

/// Codex wraps API errors as a JSON string; show just the human part.
fn api_message(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| raw.to_string())
}

/// Runs the CLI, streaming events to `on_event`, and returns the final answer text.
pub fn run(ask: &Ask, cwd: &Path, mut on_event: impl FnMut(&AssistantEvent)) -> Result<String> {
    use std::io::Write;
    let mut child = command(ask, cwd).spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            crate::Error::Assistant(format!("`{}` not found — is it installed and on your shell's PATH?", ask.provider.binary()))
        }
        _ => e.into(),
    })?;
    {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin.write_all(ask.prompt.as_bytes())?;
    }
    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut text);
        text
    });
    let mut answer = String::new();
    let mut failed = None;
    for line in BufReader::new(child.stdout.take().expect("stdout is piped")).lines() {
        for event in parse_line(ask.provider, &line?) {
            match &event {
                AssistantEvent::Delta { text } => answer.push_str(text),
                AssistantEvent::Text { text } => answer = text.clone(),
                AssistantEvent::Error { message } => failed = Some(message.clone()),
                AssistantEvent::Session { .. } | AssistantEvent::Comment { .. } => {}
            }
            on_event(&event);
        }
    }
    let status = child.wait()?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if let Some(message) = failed {
        return Err(crate::Error::Assistant(message));
    }
    if !status.success() && answer.is_empty() {
        let detail = stderr.trim().lines().last().unwrap_or("no output").to_string();
        return Err(crate::Error::Assistant(format!("{} exited with {status}: {detail}", ask.provider.binary())));
    }
    Ok(answer)
}

/// Whether a failed turn's `message` means the CLI's sign-in expired or is missing.
pub fn needs_sign_in(provider: Provider, message: &str) -> bool {
    let message = message.to_lowercase();
    let signs: &[&str] = match provider {
        Provider::Claude => &["failed to authenticate", "oauth", "not logged in", "/login", "invalid api key", "authentication_error"],
        Provider::Codex => &["401", "unauthorized", "not logged in", "codex login", "token_expired", "refresh token"],
    };
    signs.iter().any(|sign| message.contains(sign))
}

/// The CLI's sign-in command. Its own browser launch is suppressed (`BROWSER=true`); the app
/// opens the sign-in page itself.
pub fn sign_in_command(provider: Provider) -> Command {
    let mut cmd = Command::new(provider.binary());
    match provider {
        Provider::Claude => cmd.args(["auth", "login"]),
        Provider::Codex => cmd.arg("login"),
    };
    cmd.env("BROWSER", "true").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd
}

/// The sign-in page in a line of the sign-in command's output (the first `https://` link;
/// Codex also prints its `http://localhost` callback, terminals may wrap links in escapes).
pub fn sign_in_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let url: String = line[start..].chars().take_while(|c| !c.is_whitespace() && !c.is_control()).collect();
    Some(url)
}

/// Runs the CLI's sign-in in the background: `on_url` gets the sign-in page as soon as the CLI
/// prints it, then this waits until the user has finished signing in (the CLI exits). Blocking.
pub fn sign_in(provider: Provider, on_url: impl FnOnce(&str)) -> Result<()> {
    let mut child = sign_in_command(provider).spawn()?;
    // Held open until the CLI exits: it offers to read a pasted code from stdin.
    let _stdin = child.stdin.take();
    let (lines, received) = std::sync::mpsc::channel::<String>();
    let readers: Vec<_> = [
        Box::new(child.stdout.take().expect("stdout is piped")) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().expect("stderr is piped")),
    ]
    .into_iter()
    .map(|pipe| {
        let lines = lines.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(std::result::Result::ok) {
                let _ = lines.send(line);
            }
        })
    })
    .collect();
    drop(lines);
    let mut on_url = Some(on_url);
    let mut last = String::new();
    for line in received {
        if let Some(url) = sign_in_url(&line) {
            if let Some(on_url) = on_url.take() {
                on_url(&url);
            }
        }
        if !line.trim().is_empty() {
            last = line;
        }
    }
    readers.into_iter().for_each(|r| drop(r.join()));
    let status = child.wait()?;
    if !status.success() {
        return Err(crate::Error::Assistant(format!("{} sign-in failed: {}", provider.binary(), last.trim())));
    }
    Ok(())
}

/// What the assistant is asked about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// e.g. "acme/shop #12–#14".
    pub range_label: String,
    /// PR titles of the range, bottom first.
    pub titles: Vec<String>,
    /// The range's diff (possibly truncated).
    pub diff: String,
    /// Set when asking about selected lines.
    pub selection: Option<Selection>,
    /// Paths hidden by the reviewer's file filter (left out of `diff`).
    #[serde(default)]
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub path: String,
    /// e.g. "#13".
    pub pr_label: String,
    pub start_line: u32,
    pub end_line: u32,
    /// The selected code as shown (diff markers included when selected in the unified view).
    pub text: String,
    /// The selected lines' numbers at the range head (the checkout), when they have any there.
    #[serde(default)]
    pub head_start: Option<u32>,
    #[serde(default)]
    pub head_end: Option<u32>,
}

/// The first prompt of a thread: the review context, then the question. Follow-ups resume the
/// CLI session and send just the question.
pub fn first_prompt(context: &Context, question: &str) -> String {
    let mut prompt = String::from(
        "You are helping review a GitHub pull request stack. The current directory is a read-only \
         checkout of the head of the PRs under review; read any file you need, but don't try to change anything. \
         When you mention code, write its repo-relative path and line numbers in that checkout in inline code, \
         like `src/app.ts:42` or `src/app.ts:42-50`: the reviewer clicks them to jump there in the diff.\n\n\
         The review's draft comments live in the reviewer's app, and its `wispy` tools work on them: \
         `add_draft_comment` adds one on lines of the checkout, `list_drafts` shows the pending drafts (the reviewer's \
         and yours) by ref, `edit_draft` and `delete_draft` change them. When the reviewer asks you to write review \
         comments, add each one with `add_draft_comment` instead of writing it into your answer. Nothing reaches \
         GitHub until the reviewer submits the review.\n\n",
    );
    prompt.push_str(&format!("Under review: {}\n", context.range_label));
    for title in &context.titles {
        prompt.push_str(&format!("- {title}\n"));
    }
    if let Some(selection) = &context.selection {
        // In a range, a higher PR may have moved the lines; the checkout's numbers are what to cite.
        let head = match (selection.head_start, selection.head_end) {
            (Some(start), Some(end)) if (start, end) != (selection.start_line, selection.end_line) => {
                format!("; lines {start}–{end} in the checkout")
            }
            _ => String::new(),
        };
        prompt.push_str(&format!(
            "\nThe question is about these lines of {} (in {}, lines {}–{}{head}):\n```\n{}\n```\n",
            selection.path, selection.pr_label, selection.start_line, selection.end_line, selection.text
        ));
    }
    if !context.hidden.is_empty() {
        prompt.push_str(
            "\nThe reviewer hid these changed files, so their changes are left out of the diff below \
             (they're in the checkout; read them only if the question needs them):\n",
        );
        for path in &context.hidden {
            prompt.push_str(&format!("- {path}\n"));
        }
    }
    prompt.push_str(&format!("\nThe diff under review:\n```diff\n{}\n```\n\n", context.diff));
    prompt.push_str(&format!("Question: {question}\n"));
    prompt
}

/// A pending draft as the assistant is shown it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedDraft {
    pub id: String,
    /// Where it is, e.g. "src/app.ts L42–45 in #12".
    pub label: String,
    pub body: String,
    pub by_assistant: bool,
}

/// The review's drafts by the refs a thread's assistant knows them by ("d1", ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftRef {
    pub name: String,
    pub draft: String,
    /// Its text as the assistant last saw it (listed or written); `None` when the last listing
    /// didn't have it.
    pub seen: Option<String>,
}

/// The `list_drafts` answer; records what the assistant is shown (each draft gets a ref the
/// first time it's listed, and keeps it for the thread).
pub fn draft_listing(refs: &mut Vec<DraftRef>, drafts: &[ListedDraft]) -> String {
    refs.iter_mut().for_each(|r| r.seen = None);
    if drafts.is_empty() {
        return "The review has no draft comments.\n".into();
    }
    let mut text = String::from("The review's draft comments (not posted yet), by ref:\n");
    for draft in drafts {
        let name = draft_ref(refs, &draft.id, &draft.body);
        let author = if draft.by_assistant { "you" } else { "the reviewer" };
        text.push_str(&format!("\n[{name}] {} — by {author}:\n", draft.label));
        for line in draft.body.lines() {
            text.push_str(&format!("> {line}\n"));
        }
    }
    text
}

/// The ref of `draft` (a new one if it has none yet), noting that the assistant has seen `body`.
pub fn draft_ref(refs: &mut Vec<DraftRef>, draft: &str, body: &str) -> String {
    match refs.iter_mut().find(|r| r.draft == draft) {
        Some(known) => {
            known.seen = Some(body.to_string());
            known.name.clone()
        }
        None => {
            let name = format!("d{}", refs.len() + 1);
            refs.push(DraftRef { name: name.clone(), draft: draft.to_string(), seen: Some(body.to_string()) });
            name
        }
    }
}

/// Where a draft is, for the assistant and the reviewer: "src/app.ts L42–45 in #12".
pub fn draft_label(kind: crate::drafts::DraftKind, path: Option<&str>, lines: Option<(u32, u32)>, pr: u64) -> String {
    use crate::drafts::DraftKind;
    let path = path.unwrap_or("?");
    match (kind, lines) {
        (DraftKind::Summary, _) => format!("the review summary of #{pr}"),
        (DraftKind::Reply, _) => format!("a reply in a thread on {path} in #{pr}"),
        (DraftKind::File, _) => format!("{path} (whole file) in #{pr}"),
        (_, Some((start, end))) if start < end => format!("{path} L{start}–{end} in #{pr}"),
        (_, Some((_, end))) => format!("{path} L{end} in #{pr}"),
        (_, None) => format!("{path} (outdated lines) in #{pr}"),
    }
}

/// The combined diff of a range for the prompt without the `hidden` paths, cut at `limit` bytes.
pub fn diff_text(git: &crate::git::Git, from: &str, to: &str, hidden: &[String], limit: usize) -> Result<String> {
    let mut excluded: Vec<&str> = hidden.iter().map(String::as_str).collect();
    // Pathspecs apply before rename detection: a hidden rename's old path has to go too, or it
    // would show up as a deletion.
    let renames = if hidden.is_empty() { Vec::new() } else { git.run(&["diff", "--name-status", "-z", "-M", from, to, "--"])? };
    let renames = String::from_utf8_lossy(&renames);
    let mut fields = renames.split('\0');
    while let Some(status) = fields.next() {
        let paths = if status.starts_with('R') || status.starts_with('C') { 2 } else { 1 };
        let paths: Vec<&str> = fields.by_ref().take(paths).collect();
        if let [old, new] = paths[..] {
            if hidden.iter().any(|h| h == new) && status.starts_with('R') {
                excluded.push(old);
            }
        }
    }
    let excludes: Vec<String> = excluded.iter().map(|p| format!(":(exclude,literal){p}")).collect();
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-M", from, to, "--"];
    args.extend(excludes.iter().map(String::as_str));
    let out = git.run(&args)?;
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if text.len() > limit {
        let cut = (0..=limit).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        text.truncate(cut);
        text.push_str("\n… (diff truncated; read the files in the checkout for the rest)");
    }
    Ok(text)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentAction {
    #[default]
    Add,
    Edit,
    Delete,
}

/// A review tool call that changed (or failed to change) a draft, as the reviewer is shown it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentOutcome {
    #[serde(default)]
    pub action: CommentAction,
    /// A new comment's place, as the assistant asked for it.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub lines: Option<(u32, u32)>,
    /// The draft's ref ("d3"), and where the draft is.
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// The text added or written (not for deletes).
    #[serde(default)]
    pub body: Option<String>,
    pub draft: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// "user" or "assistant".
    pub role: String,
    pub text: String,
    pub at: i64,
    /// Set on a failed assistant turn.
    #[serde(default)]
    pub error: bool,
    /// Set on a failed turn caused by the CLI's sign-in (offers to sign in again).
    #[serde(default)]
    pub sign_in: bool,
    /// What the assistant did to the review's drafts during the turn, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<CommentOutcome>,
}

/// A conversation, stored locally per repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub repo: String,
    /// PR numbers of the range it was started on.
    pub prs: Vec<u64>,
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub session: Option<String>,
    /// Set for threads about selected lines (for gutter markers and "turn into comment").
    pub selection: Option<ThreadAnchor>,
    pub messages: Vec<Message>,
    pub created_at: i64,
    /// The review context while no turn has succeeded yet, so asking again still sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsent_context: Option<Context>,
    /// The drafts the assistant was shown, by ref (it edits and deletes them by ref).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draft_refs: Vec<DraftRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadAnchor {
    pub path: String,
    /// Stack index of the PR the lines belong to, and the lines in that PR's diff.
    pub pr_index: usize,
    pub side: crate::github::Side,
    pub start_line: u32,
    pub end_line: u32,
    /// Head lines in the view it was asked from (for gutter markers).
    pub head_start: Option<u32>,
    pub head_end: Option<u32>,
}

/// A read-only checkout of `commit` for the assistant, reused per commit. The clone's blobs for
/// that tree are fetched first (`hydrate`), so checking out doesn't fetch file by file.
pub fn worktree(git: &crate::git::Git, root: &Path, commit: &str) -> Result<PathBuf> {
    let dir = root.join(&commit[..commit.len().min(16)]);
    if dir.join(".git").exists() {
        return Ok(dir);
    }
    std::fs::create_dir_all(root)?;
    git.run(&["worktree", "prune"])?;
    git.run(&["worktree", "add", "--detach", "--force", &dir.to_string_lossy(), commit])?;
    Ok(dir)
}
