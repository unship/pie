//! pie-mcp — minimal MCP (Model Context Protocol) stdio client.
//!
//! Scope (v1): subprocess-based stdio transport, JSON-RPC 2.0 framing over newline-delimited
//! JSON, initialize handshake, tools/list, tools/call. Out of scope for v1: SSE transport,
//! sampling, resource subscriptions, server-side mode, reconnect/backoff (planned but not
//! shipped here).
//!
//! The crate intentionally does not depend on `pie-agent-core` so it can be reused from
//! places that don't carry the harness — `pie-coding-agent` provides the adapter that wraps
//! MCP tools as `AgentTool`s.

pub mod client;
pub mod errors;
pub mod protocol;
pub mod stdio;
pub mod transport;

pub use client::{ClientCapabilities, McpClient};
pub use errors::McpError;
pub use protocol::{InitializeResult, McpTool, McpToolCallResult, ServerInfo};
pub use stdio::StdioTransport;
pub use transport::Transport;
