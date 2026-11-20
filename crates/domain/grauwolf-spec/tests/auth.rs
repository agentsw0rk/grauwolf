//! `auth.spiffe` on `agent_tools` and `mcp_servers` (ADR-0109).

use grauwolf_spec::{AgentSpec, AgentSpecExt, SpiffeMode, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(mcp_servers: Value, agent_tools: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": mcp_servers,
            "agent_tools": agent_tools,
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn errors_of_agent(auth: Value, headers: Value) -> Vec<(String, ValidationErrorKind)> {
    spec(
        json!({}),
        json!({ "reviewer": { "url": "https://r.svc:8443", "headers": headers, "auth": auth } }),
    )
    .validate()
    .err()
    .unwrap_or_default()
    .into_iter()
    .map(|e| (e.path, e.kind))
    .collect()
}

const ID: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";

#[test]
fn valid_auth_parses_and_has_no_errors() {
    for auth in [
        json!({ "spiffe": { "id": ID } }),
        json!({ "spiffe": { "ids": [ID, "spiffe://agentswork.de/ns/b/sa/b"] } }),
        json!({ "spiffe": { "mode": "mtls", "trust_domain": "agentswork.de" } }),
        json!({ "spiffe": { "mode": "jwt", "audience": "tickets" } }),
        json!({ "spiffe": { "mode": "jwt", "audience": "tickets", "id": ID } }),
    ] {
        assert_eq!(errors_of_agent(auth.clone(), json!({})), [], "{auth}");
    }
    let parsed = spec(
        json!({}),
        json!({ "r": { "url": "https://r", "auth": { "spiffe": { "mode": "jwt", "audience": "a" } } } }),
    );
    assert_eq!(
        parsed.agent_tools["r"].auth.as_ref().unwrap().spiffe.mode(),
        SpiffeMode::Jwt
    );
}

#[test]
fn mtls_needs_exactly_one_way_to_name_the_server() {
    let at = "/agent_tools/reviewer/auth/spiffe";
    for auth in [
        json!({ "spiffe": {} }),
        json!({ "spiffe": { "id": ID, "trust_domain": "agentswork.de" } }),
    ] {
        assert_eq!(
            errors_of_agent(auth, json!({})),
            [(at.to_owned(), ValidationErrorKind::SpiffeMatch)]
        );
    }
}

#[test]
fn jwt_needs_an_audience_and_mtls_does_not_take_one() {
    assert_eq!(
        errors_of_agent(json!({ "spiffe": { "mode": "jwt" } }), json!({})),
        [(
            "/agent_tools/reviewer/auth/spiffe".to_owned(),
            ValidationErrorKind::SpiffeAudience
        )]
    );
    assert_eq!(
        errors_of_agent(
            json!({ "spiffe": { "id": ID, "audience": "x" } }),
            json!({})
        ),
        [(
            "/agent_tools/reviewer/auth/spiffe/audience".to_owned(),
            ValidationErrorKind::SpiffeAudienceWithMtls
        )]
    );
}

#[test]
fn ids_and_trust_domains_are_checked_with_pointers() {
    let errors = errors_of_agent(
        json!({ "spiffe": { "ids": [ID, "https://x/y", "spiffe://x/a//b"] } }),
        json!({}),
    );
    let paths: Vec<&str> = errors.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/agent_tools/reviewer/auth/spiffe/ids/1",
            "/agent_tools/reviewer/auth/spiffe/ids/2"
        ]
    );
    let errors = errors_of_agent(
        json!({ "spiffe": { "trust_domain": "Agents Work" } }),
        json!({}),
    );
    assert_eq!(
        errors[0].0,
        "/agent_tools/reviewer/auth/spiffe/trust_domain"
    );
    assert!(matches!(
        errors[0].1,
        ValidationErrorKind::InvalidSpiffe { .. }
    ));
    let errors = errors_of_agent(json!({ "spiffe": { "id": "spiffe://x" } }), json!({}));
    assert_eq!(errors, []);
}

#[test]
fn auth_and_an_authorization_header_exclude_each_other() {
    let errors = errors_of_agent(
        json!({ "spiffe": { "id": ID } }),
        json!({ "authorization": "Bearer ${T}", "X-Other": "1" }),
    );
    assert_eq!(
        errors,
        [(
            "/agent_tools/reviewer/auth".to_owned(),
            ValidationErrorKind::AuthWithAuthorizationHeader
        )]
    );
    assert_eq!(
        errors_of_agent(json!({ "spiffe": { "id": ID } }), json!({ "X-Other": "1" })),
        []
    );
}

#[test]
fn auth_needs_a_remote_target() {
    let stdio = spec(
        json!({ "files": { "command": "npx", "tools": ["x"], "auth": { "spiffe": { "id": ID } } } }),
        json!({}),
    );
    let errors = stdio.validate().unwrap_err();
    assert!(
        errors.iter().any(|e| e.path == "/mcp_servers/files/auth"
            && e.kind == ValidationErrorKind::AuthWithoutUrl),
        "{errors:?}"
    );
    let remote = spec(
        json!({ "tickets": { "url": "https://t/mcp", "tools": ["x"], "headers": { "Authorization": "x" }, "auth": { "spiffe": { "id": ID } } } }),
        json!({}),
    );
    let errors = remote.validate().unwrap_err();
    assert!(
        errors.iter().any(|e| e.path == "/mcp_servers/tickets/auth"
            && e.kind == ValidationErrorKind::AuthWithAuthorizationHeader),
        "{errors:?}"
    );
    let local = spec(
        json!({}),
        json!({ "sub": { "spec": "sub.json", "description": "d", "auth": { "spiffe": { "id": ID } } } }),
    );
    let errors = local.validate().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.path == "/agent_tools/sub/auth"
                && e.kind == ValidationErrorKind::AuthWithoutUrl),
        "{errors:?}"
    );
}

#[test]
fn unknown_fields_in_auth_are_refused() {
    let text = json!({ "spec_version": 1, "name": "a", "model": {"provider":"ollama","id":"m"},
        "agent_tools": { "r": { "url": "https://r", "auth": { "spiffe": { "idd": ID } } } },
        "graph": { "nodes": [], "edges": [] } })
    .to_string();
    assert!(AgentSpec::from_json(&text).is_err());
}
