use std::io::{Read, Write};

use serde_json::{json, Value};
use wispy_core::mcp::{serve, Tool, Tools};

struct Echo;

impl Tools for Echo {
    fn list(&self) -> Vec<Tool> {
        vec![Tool { name: "echo", description: "Says it back.", input_schema: json!({ "type": "object" }) }]
    }

    fn call(&self, name: &str, arguments: &Value) -> Result<String, String> {
        match arguments["text"].as_str() {
            Some(text) => Ok(format!("{name}: {text}")),
            None => Err("no text".into()),
        }
    }
}

/// The status line and body of a raw HTTP request to the server.
fn request(url: &str, method: &str, token: &str, body: &str) -> (String, String) {
    let address = url.trim_start_matches("http://").split('/').next().unwrap();
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    write!(stream, "{method} /mcp HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_string(), body.to_string())
}

#[test]
fn serves_tools_to_whoever_has_the_token_until_dropped() {
    std::thread::scope(|scope| {
        let server = serve(scope, &Echo).unwrap();
        let (url, token) = (server.endpoint().url.clone(), server.endpoint().token.clone());
        let rpc = |message: Value| {
            let (status, body) = request(&url, "POST", &token, &message.to_string());
            (status, serde_json::from_str::<Value>(&body).unwrap_or(Value::Null))
        };

        let (status, init) = rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-11-25" } }));
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(init["result"]["serverInfo"]["name"], "wispy");
        assert_eq!(rpc(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).0, "HTTP/1.1 202 Accepted");
        let (_, list) = rpc(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        assert_eq!(list["result"]["tools"][0]["name"], "echo");
        assert_eq!(list["result"]["tools"][0]["inputSchema"], json!({ "type": "object" }));
        let (_, ok) = rpc(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "echo", "arguments": { "text": "hi" } } }));
        assert_eq!(ok["result"], json!({ "content": [{ "type": "text", "text": "echo: hi" }], "isError": false }));
        let (_, failed) = rpc(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "echo", "arguments": {} } }));
        assert_eq!(failed["result"], json!({ "content": [{ "type": "text", "text": "no text" }], "isError": true }));
        // Newer clients probe for methods first; unknown ones are JSON-RPC errors.
        let (_, unknown) = rpc(json!({ "jsonrpc": "2.0", "id": 5, "method": "server/discover" }));
        assert_eq!(unknown["error"]["code"], -32601);

        assert_eq!(request(&url, "GET", &token, "").0, "HTTP/1.1 405 Method Not Allowed", "no server-sent stream");
        let call = json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/list" }).to_string();
        assert_eq!(request(&url, "POST", "guessed", &call).0, "HTTP/1.1 401 Unauthorized");

        // Dropping the server stops it: otherwise the scope would never end.
        drop(server);
    });
}
