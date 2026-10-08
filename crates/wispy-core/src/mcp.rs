//! A minimal MCP server over streamable HTTP (plain JSON responses, no SSE stream), on
//! 127.0.0.1, for the tools the assistant CLIs call while they answer. It lives as long as one
//! turn: [`serve`] runs it on scoped threads until the returned [`Server`] is dropped.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::Result;

/// The server name the CLIs know the tools by (`mcp__wispy__…` in Claude Code).
pub const SERVER_NAME: &str = "wispy";

/// A tool as `tools/list` describes it.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema of the arguments.
    pub input_schema: Value,
}

/// Carries out tool calls; called from the server's threads (concurrently, when the CLI calls
/// tools in parallel).
pub trait Tools: Sync {
    fn list(&self) -> Vec<Tool>;
    /// The result text, or the error text the agent sees (`isError`).
    fn call(&self, name: &str, arguments: &Value) -> std::result::Result<String, String>;
}

/// Where the CLI reaches the server: `url`, with `Authorization: Bearer <token>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub token: String,
}

/// A running server; stops accepting requests when dropped (the scope then waits for requests
/// still being answered).
pub struct Server {
    endpoint: Endpoint,
    stopped: Arc<AtomicBool>,
    port: u16,
}

impl Server {
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // Wakes the accept loop up.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// Serves `tools` on a free local port until the returned server is dropped.
pub fn serve<'scope, 'env>(scope: &'scope std::thread::Scope<'scope, 'env>, tools: &'env dyn Tools) -> Result<Server> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let token = crate::drafts::new_id();
    let stopped = Arc::new(AtomicBool::new(false));
    let endpoint = Endpoint { url: format!("http://127.0.0.1:{port}/mcp"), token: token.clone() };
    let stop = stopped.clone();
    scope.spawn(move || {
        for stream in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let token = token.clone();
            scope.spawn(move || {
                let _ = handle(stream, &token, tools);
            });
        }
    });
    Ok(Server { endpoint, stopped, port })
}

/// Answers one request and closes the connection.
fn handle(mut stream: TcpStream, token: &str, tools: &dyn Tools) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let (mut length, mut authorized) = (0, false);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0),
            "authorization" => authorized = value == format!("Bearer {token}"),
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let respond = |stream: &mut TcpStream, status: &str, body: Option<String>| {
        let body = body.unwrap_or_default();
        let kind = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
        write!(stream, "HTTP/1.1 {status}\r\n{kind}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    };
    if !authorized {
        return respond(&mut stream, "401 Unauthorized", None);
    }
    if !request_line.starts_with("POST ") {
        // No server-initiated stream (GET) and no sessions to end (DELETE).
        return respond(&mut stream, "405 Method Not Allowed", None);
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(message) => match answer(&message, tools) {
            Some(reply) => respond(&mut stream, "200 OK", Some(reply.to_string())),
            None => respond(&mut stream, "202 Accepted", None),
        },
        Err(_) => respond(&mut stream, "400 Bad Request", None),
    }
}

/// The JSON-RPC reply to `message`; `None` for notifications.
fn answer(message: &Value, tools: &dyn Tools) -> Option<Value> {
    let id = message.get("id")?.clone();
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => {
            let list: Vec<Value> = tools
                .list()
                .into_iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
                .collect();
            json!({ "tools": list })
        }
        "tools/call" => {
            let (text, error) = match tools.call(params["name"].as_str().unwrap_or_default(), &params["arguments"]) {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": error })
        }
        method => {
            return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method {method}") } }));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}
