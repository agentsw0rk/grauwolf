//! Serves a small MCP server over the HTTP+SSE transport (protocol 2024-11-05) with one tool,
//! `reverse { text }`, on 127.0.0.1 until it is killed; for live checks of specs with
//! `transport: "sse"` without a real server (ADR-0112).
//!
//! `cargo run -p grauwolf-mcp --features test-sse-server --example serve_sse_mcp [port]`
//!
//! Prints `sse mcp server: http://127.0.0.1:<port>/sse` (default port 8931, as in
//! `docs/user-guide/examples/08-mcp-sse.json`).

use grauwolf_mcp::testing::{SseBehavior, SseTestServer};

#[tokio::main]
async fn main() {
    let port = std::env::args()
        .nth(1)
        .map_or(8931, |port| port.parse().expect("port must be a number"));
    let server = SseTestServer::start_with(SseBehavior {
        port,
        only_reverse: true,
        ..SseBehavior::default()
    })
    .await;
    println!("sse mcp server: {}", server.url());
    std::future::pending::<()>().await;
}
