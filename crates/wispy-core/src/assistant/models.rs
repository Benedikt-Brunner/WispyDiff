//! Account-aware model discovery through Codex's app-server protocol. No turn is started.
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

fn discover(mut command: std::process::Command, timeout: Duration) -> Result<Vec<AssistantModel>> {
    let mut child = command
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
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
    let mut server = Server {
        child,
        stdin,
        rx,
        deadline: Instant::now() + timeout,
    };
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

struct Server {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<std::io::Result<String>>,
    deadline: Instant,
}

impl Server {
    fn send(&mut self, message: Value) -> Result<()> {
        writeln!(self.stdin, "{message}")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        self.send(json!({"id": id, "method": method, "params": params}))?;
        loop {
            let remaining = self.deadline.checked_duration_since(Instant::now())
                .ok_or_else(|| Error::Assistant("Loading Codex models timed out. Check Codex is up to date and signed in.".into()))?;
            let line = self.rx.recv_timeout(remaining)
                .map_err(|e| Error::Assistant(format!("Could not load Codex models: {e}. Check Codex is up to date and signed in.")))??;
            let message: Value = serde_json::from_str(&line)
                .map_err(|e| Error::Assistant(format!("Invalid Codex model response: {e}")))?;
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
}
