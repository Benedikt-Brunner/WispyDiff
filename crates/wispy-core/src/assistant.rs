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
}

/// The CLI invocation for `ask`, restricted to reading. The prompt goes on stdin.
pub fn command(ask: &Ask, cwd: &Path) -> Command {
    let mut cmd = Command::new(ask.provider.binary());
    cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    match ask.provider {
        Provider::Claude => {
            cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
            // Read-only: only tools that can't change anything, nothing else is allowed.
            cmd.args(["--allowed-tools", "Read,Grep,Glob", "--permission-mode", "dontAsk"]);
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
    let mut child = command(ask, cwd).spawn()?;
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
                AssistantEvent::Session { .. } => {}
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
}

/// The first prompt of a thread: the review context, then the question. Follow-ups resume the
/// CLI session and send just the question.
pub fn first_prompt(context: &Context, question: &str) -> String {
    let mut prompt = String::from(
        "You are helping review a GitHub pull request stack. The current directory is a read-only \
         checkout of the head of the PRs under review; read any file you need, but don't try to change anything.\n\n",
    );
    prompt.push_str(&format!("Under review: {}\n", context.range_label));
    for title in &context.titles {
        prompt.push_str(&format!("- {title}\n"));
    }
    if let Some(selection) = &context.selection {
        prompt.push_str(&format!(
            "\nThe question is about these lines of {} (in {}, lines {}–{}):\n```\n{}\n```\n",
            selection.path, selection.pr_label, selection.start_line, selection.end_line, selection.text
        ));
    }
    prompt.push_str(&format!("\nThe diff under review:\n```diff\n{}\n```\n\nQuestion: {question}\n", context.diff));
    prompt
}

/// The combined diff of a range for the prompt, cut at `limit` bytes.
pub fn diff_text(git: &crate::git::Git, from: &str, to: &str, limit: usize) -> Result<String> {
    let out = git.run(&["diff", "--no-color", "--no-ext-diff", "-M", from, to, "--"])?;
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if text.len() > limit {
        let cut = (0..=limit).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        text.truncate(cut);
        text.push_str("\n… (diff truncated; read the files in the checkout for the rest)");
    }
    Ok(text)
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
