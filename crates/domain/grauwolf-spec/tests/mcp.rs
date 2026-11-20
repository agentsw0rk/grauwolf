//! MCP servers in specs (F09.1, ADR-0036).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ToolOrigin, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(mcp_servers: Value, subgraph_tools: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "tools": [ { "name": "add", "description": "Addiert", "args": { "type": "object" }, "code": "Ok(String::new())" } ],
            "mcp_servers": mcp_servers,
            "subgraphs": { "work": {
                "tools": subgraph_tools,
                "graph": {
                    "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                    "edges": [ { "from": "start", "to": "call_llm" },
                               { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
                }
            } },
            "graph": {
                "nodes": [ { "id": "work", "kind": "subgraph", "params": { "subgraph": "work" } } ],
                "edges": [ { "from": "start", "to": "work" }, { "from": "work", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn parses_mcp_servers_and_lets_subgraphs_use_their_tools() {
    let spec = spec(
        json!({ "files": { "command": "npx", "args": ["-y", "server-filesystem"], "tools": ["read_file"] } }),
        json!(["add", "read_file"]),
    );

    let files = &spec.mcp_servers["files"];
    assert_eq!(files.command.as_deref(), Some("npx"));
    assert_eq!(files.args, ["-y", "server-filesystem"]);
    assert_eq!(files.tools, ["read_file"]);
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn reports_invalid_server_names_and_tool_name_collisions() {
    let spec = spec(
        json!({ "Files": { "command": "x", "tools": ["add"] } }),
        json!(["add"]),
    );

    let errors = spec.validate().unwrap_err();

    assert_eq!(
        errors,
        [
            ValidationError {
                path: "/mcp_servers/Files".into(),
                kind: ValidationErrorKind::InvalidId { id: "Files".into() },
            },
            ValidationError {
                path: "/mcp_servers/Files/tools/0".into(),
                kind: ValidationErrorKind::DuplicateToolName {
                    name: "add".into(),
                    other_origin: ToolOrigin::Own,
                    other_pointer: "/tools/0/name".into()
                },
            },
        ]
    );
}

#[test]
fn accepts_remote_servers_by_url_and_requires_exactly_one_transport() {
    let remote = spec(
        json!({ "docs": { "url": "https://mcp.example.com/mcp", "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" }, "tools": ["search"] } }),
        json!(["search"]),
    );
    let both = spec(
        json!({ "docs": { "command": "npx", "url": "https://mcp.example.com/mcp", "tools": ["search"] } }),
        json!([]),
    );
    let neither = spec(json!({ "docs": { "tools": ["search"] } }), json!([]));

    let docs = &remote.mcp_servers["docs"];
    assert_eq!(docs.url.as_deref(), Some("https://mcp.example.com/mcp"));
    assert_eq!(docs.headers["Authorization"], "Bearer ${DOCS_TOKEN}");
    assert_eq!(remote.validate(), Ok(()));
    for spec in [both, neither] {
        assert_eq!(
            spec.validate().unwrap_err(),
            [ValidationError {
                path: "/mcp_servers/docs".into(),
                kind: ValidationErrorKind::McpTransport,
            }]
        );
    }
}

#[test]
fn selects_the_sse_transport_of_remote_servers_only() {
    use grauwolf_spec::McpHttpTransport;
    let sse = spec(
        json!({ "legacy": { "url": "https://mcp.example.com/sse", "transport": "sse", "tools": ["search"] } }),
        json!(["search"]),
    );
    let explicit = spec(
        json!({ "docs": { "url": "https://mcp.example.com/mcp", "transport": "streamable_http", "tools": [] } }),
        json!([]),
    );
    let default = spec(
        json!({ "docs": { "url": "https://mcp.example.com/mcp", "tools": [] } }),
        json!([]),
    );
    let stdio = spec(
        json!({ "files": { "command": "npx", "transport": "sse", "tools": [] } }),
        json!([]),
    );

    assert_eq!(
        sse.mcp_servers["legacy"].http_transport(),
        McpHttpTransport::Sse
    );
    assert_eq!(
        explicit.mcp_servers["docs"].http_transport(),
        McpHttpTransport::StreamableHttp
    );
    assert_eq!(
        default.mcp_servers["docs"].http_transport(),
        McpHttpTransport::StreamableHttp
    );
    for valid in [sse, explicit, default] {
        assert_eq!(valid.validate(), Ok(()));
    }
    assert_eq!(
        stdio.validate().unwrap_err(),
        [ValidationError {
            path: "/mcp_servers/files/transport".into(),
            kind: ValidationErrorKind::McpTransportWithoutUrl,
        }]
    );
}

#[test]
fn rejects_unknown_transports_when_parsing() {
    let error = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "docs": { "url": "https://x.example/mcp", "transport": "websocket", "tools": [] } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("websocket"), "{error}");
}

#[test]
fn offers_resource_tools_for_servers_with_resources() {
    let spec = spec(
        json!({ "docs": { "url": "https://mcp.example.com/mcp", "tools": ["search"], "resources": true } }),
        json!(["docs_read_resource"]),
    );
    let clash = spec_with_tool_named("docs_list_resources");

    assert_eq!(
        spec.external_tools(),
        ["search", "docs_list_resources", "docs_read_resource"]
    );
    assert_eq!(spec.validate(), Ok(()));
    assert!(
        clash.validate().unwrap_err().iter().any(|error| matches!(
            &error.kind,
            ValidationErrorKind::DuplicateToolName { name, other_origin: ToolOrigin::Own, .. } if name == "docs_list_resources"
        )),
        "resource tool does not collide"
    );
}

/// Spec with its own tool `name` and a server `docs` with resources.
fn spec_with_tool_named(name: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "tools": [ { "name": name, "description": "eigen", "args": { "type": "object" }, "code": "Ok(String::new())" } ],
            "mcp_servers": { "docs": { "url": "https://mcp.example.com/mcp", "tools": [], "resources": true } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}
