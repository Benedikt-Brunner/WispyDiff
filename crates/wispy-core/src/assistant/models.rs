//! Account-aware model discovery: Codex's app-server protocol, Claude Code's `initialize`
//! control request. No turn is started.
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use super::Provider;
use crate::{Error, Result};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantModel {
    pub model: String,
    pub display_name: String,
    #[serde(default)]
    pub hidden: bool,
    pub is_default: bool,
    pub supported_reasoning_efforts: Vec<ReasoningEffort>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffort {
    pub reasoning_effort: String,
}

/// Blocking; callers run this off the UI thread. Bound the entire exchange and
/// always reap the short-lived server, including on protocol errors and timeouts.
pub fn codex_models() -> Result<Vec<AssistantModel>> {
    discover(Provider::Codex.command(), Duration::from_secs(15))
}

/// The models Claude Code offers the signed-in account (its `/model` list), from the CLI's
/// `initialize` control request. Blocking, like [`codex_models`]. Hooks are off: nothing runs
/// but the handshake.
pub fn claude_models() -> Result<Vec<AssistantModel>> {
    let mut command = Provider::Claude.command();
    command.current_dir(std::env::temp_dir());
    discover_claude(command, Duration::from_secs(15))
}

/// Claude Code's newest Mythos model, when the account has one (the CLI lists newest first).
pub fn mythos(models: &[AssistantModel]) -> Option<&AssistantModel> {
    models.iter().find(|m| m.model.starts_with("claude-mythos"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeModel {
    value: String,
    display_name: String,
    #[serde(default)]
    supported_effort_levels: Vec<String>,
}

fn discover_claude(mut command: std::process::Command, timeout: Duration) -> Result<Vec<AssistantModel>> {
    let child = command
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .args(["--settings", r#"{"disableAllHooks":true}"#, "--tools", ""])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut server = Server::start(child, timeout, "Claude Code");
    server.send(json!({"type": "control_request", "request_id": "models", "request": {"subtype": "initialize"}}))?;
    let response = loop {
        let message = server.next()?;
        if message["type"] == "control_response" && message["response"]["request_id"] == "models" {
            break message["response"].clone();
        }
    };
    if response["subtype"] != "success" {
        let error = response["error"].as_str().unwrap_or("Claude Code model discovery failed");
        return Err(Error::Assistant(error.into()));
    }
    let models: Vec<ClaudeModel> = serde_json::from_value(response["response"]["models"].clone())
        .map_err(|e| Error::Assistant(format!("Could not read Claude Code models: {e}")))?;
    Ok(models.into_iter().map(|m| AssistantModel {
        is_default: m.value == "default",
        model: m.value,
        display_name: m.display_name,
        hidden: false,
        supported_reasoning_efforts: m.supported_effort_levels.into_iter().map(|reasoning_effort| ReasoningEffort { reasoning_effort }).collect(),
    }).collect())
}

fn discover(mut command: std::process::Command, timeout: Duration) -> Result<Vec<AssistantModel>> {
    let child = command
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut server = Server::start(child, timeout, "Codex");
    server.request(
        0,
        "initialize",
        json!({"clientInfo": {"name": "wispydiff", "version": env!("CARGO_PKG_VERSION")}}),
    )?;
    server.send(json!({"method": "initialized", "params": {}}))?;
    let mut models = Vec::new();
    let mut cursor = Value::Null;
    for id in 1.. {
        let result = server.request(
            id,
            "model/list",
            json!({"limit": 100, "cursor": cursor, "includeHidden": false}),
        )?;
        let page: Vec<AssistantModel> = serde_json::from_value(result["data"].clone())
            .map_err(|e| Error::Assistant(format!("Could not read Codex models: {e}")))?;
        models.extend(page.into_iter().filter(|m| !m.hidden));
        let next = result["nextCursor"].clone();
        if next.is_null() {
            break;
        }
        if next == cursor {
            return Err(Error::Assistant(
                "Codex repeated the model-list cursor".into(),
            ));
        }
        cursor = next;
    }
    Ok(models)
}

/// A short-lived CLI speaking JSON lines on stdin / stdout, read against one deadline.
struct Server {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<std::io::Result<String>>,
    deadline: Instant,
    name: &'static str,
}

impl Server {
    fn start(mut child: Child, timeout: Duration, name: &'static str) -> Self {
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Server { child, stdin, rx, deadline: Instant::now() + timeout, name }
    }

    fn send(&mut self, message: Value) -> Result<()> {
        writeln!(self.stdin, "{message}")?;
        self.stdin.flush()?;
        Ok(())
    }

    /// The next message, or an error once the deadline passes or the CLI exits.
    fn next(&mut self) -> Result<Value> {
        let name = self.name;
        let timed_out = || Error::Assistant(format!("Loading {name} models timed out. Check {name} is up to date and signed in."));
        let remaining = self.deadline.checked_duration_since(Instant::now()).ok_or_else(timed_out)?;
        let line = self.rx.recv_timeout(remaining).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => timed_out(),
            mpsc::RecvTimeoutError::Disconnected => Error::Assistant(format!("{name} quit while loading models. Check {name} is up to date and signed in.")),
        })??;
        serde_json::from_str(&line).map_err(|e| Error::Assistant(format!("Invalid {name} model response: {e}")))
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        self.send(json!({"id": id, "method": method, "params": params}))?;
        loop {
            let message = self.next()?;
            if message["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(Error::Assistant(
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex model discovery failed")
                        .into(),
                ));
            }
            return Ok(message["result"].clone());
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_paginated_models_and_supported_efforts() {
        let command = std::process::Command::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testing/fake-cli/codex"
        ));
        let models = discover(command, Duration::from_secs(3)).unwrap();
        assert_eq!(
            models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>(),
            ["fake-fast", "fake-deep"]
        );
        assert!(models[0].is_default);
        assert_eq!(
            models[1].supported_reasoning_efforts[1].reasoning_effort,
            "xhigh"
        );
    }

    #[test]
    fn reports_discovery_errors_and_times_out_unresponsive_cli() {
        for mode in ["error", "timeout"] {
            let mut command = std::process::Command::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../testing/fake-cli/codex"
            ));
            command.env("WISPY_FAKE_MODELS", mode);
            let start = Instant::now();
            let message = discover(
                command,
                if mode == "timeout" {
                    Duration::from_millis(300)
                } else {
                    Duration::from_secs(3)
                },
            )
            .unwrap_err()
            .to_string();
            assert!(start.elapsed() < Duration::from_secs(4));
            assert!(
                message.contains(if mode == "error" {
                    "Sign in to Codex"
                } else {
                    "timed out"
                }),
                "{message}"
            );
        }
    }

    fn fake_claude(mode: &str) -> std::process::Command {
        let mut command = std::process::Command::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testing/fake-cli/claude"));
        command.env("WISPY_FAKE_MODELS", mode);
        command
    }

    #[test]
    fn finds_claude_mythos_only_when_the_account_lists_it() {
        let models = discover_claude(fake_claude("ok"), Duration::from_secs(3)).unwrap();
        assert!(models[0].is_default && !models[1].is_default);
        assert_eq!(models[1].supported_reasoning_efforts[3].reasoning_effort, "xhigh");
        let newest = mythos(&models).unwrap();
        assert_eq!((newest.model.as_str(), newest.display_name.as_str()), ("claude-mythos-5-2", "Mythos 5.2"));

        let models = discover_claude(fake_claude("no-mythos"), Duration::from_secs(3)).unwrap();
        assert!(!models.is_empty() && mythos(&models).is_none());
    }

    #[test]
    fn reports_claude_discovery_errors_and_times_out() {
        let message = discover_claude(fake_claude("error"), Duration::from_secs(3)).unwrap_err().to_string();
        assert!(message.contains("Not logged in"), "{message}");
        let start = Instant::now();
        let message = discover_claude(fake_claude("timeout"), Duration::from_millis(300)).unwrap_err().to_string();
        assert!(start.elapsed() < Duration::from_secs(4) && message.contains("Claude Code models timed out"), "{message}");
    }
}
