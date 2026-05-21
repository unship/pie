//! In-process MCP client fixture test. Instead of spawning a real subprocess, we drive the
//! client over a custom Transport implementation that exchanges JSON lines with a mock
//! "server" running on the same tokio runtime.

use std::sync::Arc;

use async_trait::async_trait;
use pie_mcp::{McpClient, McpError, Transport};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;

/// Pipe transport: two unbounded channels that emulate stdin (we write) and stdout (we read).
struct PipeTransport {
    tx: AsyncMutex<mpsc::UnboundedSender<String>>,
    rx: AsyncMutex<mpsc::UnboundedReceiver<String>>,
}

#[async_trait]
impl Transport for PipeTransport {
    async fn send_line(&self, line: String) -> Result<(), McpError> {
        self.tx
            .lock()
            .await
            .send(line)
            .map_err(|e| McpError::Transport(e.to_string()))
    }
    async fn recv_line(&self) -> Result<Option<String>, McpError> {
        Ok(self.rx.lock().await.recv().await)
    }
    async fn close(&self) {
        // dropping the senders inside the test is enough
    }
}

fn pair() -> (Arc<PipeTransport>, Arc<PipeTransport>) {
    let (a_tx, b_rx) = mpsc::unbounded_channel();
    let (b_tx, a_rx) = mpsc::unbounded_channel();
    let a = PipeTransport {
        tx: AsyncMutex::new(a_tx),
        rx: AsyncMutex::new(a_rx),
    };
    let b = PipeTransport {
        tx: AsyncMutex::new(b_tx),
        rx: AsyncMutex::new(b_rx),
    };
    (Arc::new(a), Arc::new(b))
}

/// Mock server: handles initialize → tools/list → tools/call by writing back canned responses.
async fn run_mock_server(transport: Arc<PipeTransport>) {
    loop {
        let line = match transport.recv_line().await {
            Ok(Some(l)) => l,
            _ => break,
        };
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = v.get("method").and_then(|x| x.as_str()).unwrap_or("");
        let id = v.get("id").and_then(|x| x.as_u64());

        if method == "notifications/initialized" {
            continue; // no response
        }

        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "serverInfo": { "name": "mock-server", "version": "0.0.1" }
            }),
            "tools/list" => serde_json::json!({
                "tools": [{
                    "name": "echo",
                    "description": "echo text back",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "text": { "type": "string" } },
                        "required": ["text"]
                    }
                }]
            }),
            "tools/call" => {
                let args = v
                    .get("params")
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let text = args
                    .get("text")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                serde_json::json!({
                    "content": [{ "type": "text", "text": format!("echo: {text}") }],
                    "isError": false
                })
            }
            _ => serde_json::json!(null),
        };
        let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let _ = transport.send_line(resp.to_string()).await;
    }
}

#[tokio::test]
async fn handshake_list_and_call_round_trip() {
    let (client_side, server_side) = pair();
    tokio::spawn(run_mock_server(server_side));

    let client = McpClient::new(client_side);
    let init = client.initialize("pie-test").await.unwrap();
    assert_eq!(init.server_info.name, "mock-server");
    assert!(client.is_initialized());

    let tools = client.tools_list().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let res = client
        .tools_call("echo", Some(serde_json::json!({ "text": "hi" })))
        .await
        .unwrap();
    assert!(!res.is_error);
    let body = match &res.content[0] {
        pie_mcp::protocol::ToolContent::Text { text } => text.clone(),
        _ => panic!("expected text"),
    };
    assert_eq!(body, "echo: hi");
}

#[tokio::test]
async fn tools_list_before_initialize_is_rejected() {
    let (client_side, _server_side) = pair();
    let client = McpClient::new(client_side);
    let err = client.tools_list().await.unwrap_err();
    matches!(err, McpError::NotInitialized);
}
