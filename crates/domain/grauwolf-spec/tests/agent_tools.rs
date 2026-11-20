//! Sub-agents as tools in specs (F03.12, ADR-0040).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ToolOrigin, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(agent_tools: Value, subgraph_tools: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "coordinator",
            "model": { "provider": "ollama", "id": "m" },
            "tools": [ { "name": "add", "description": "Addiert", "args": { "type": "object" }, "code": "Ok(String::new())" } ],
            "agent_tools": agent_tools,
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
fn parses_agent_tools_and_lets_subgraphs_use_them() {
    let spec = spec(
        json!({ "researcher": { "spec": "researcher.json", "description": "Researches facts" } }),
        json!(["add", "researcher"]),
    );

    let researcher = &spec.agent_tools["researcher"];
    assert_eq!(researcher.spec.as_deref(), Some("researcher.json"));
    assert_eq!(researcher.description.as_deref(), Some("Researches facts"));
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn reports_invalid_agent_tool_names_and_collisions() {
    let spec = spec(
        json!({
            "Researcher": { "spec": "r.json", "description": "R" },
            "add": { "spec": "a.json", "description": "A" }
        }),
        json!([]),
    );

    let errors = spec.validate().unwrap_err();

    assert_eq!(
        errors,
        [
            ValidationError {
                path: "/agent_tools/Researcher".into(),
                kind: ValidationErrorKind::InvalidId {
                    id: "Researcher".into()
                },
            },
            ValidationError {
                path: "/agent_tools/add".into(),
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
fn accepts_remote_agents_by_url_and_requires_exactly_one_source() {
    let remote = spec(
        json!({ "weather": { "url": "https://agents.example.com", "headers": { "Authorization": "Bearer ${WEATHER_TOKEN}" } } }),
        json!(["weather"]),
    );
    let both = spec(
        json!({ "weather": { "spec": "w.json", "url": "https://agents.example.com", "description": "Weather" } }),
        json!([]),
    );
    let local_without_description = spec(json!({ "weather": { "spec": "w.json" } }), json!([]));

    let weather = &remote.agent_tools["weather"];
    assert_eq!(weather.url.as_deref(), Some("https://agents.example.com"));
    assert_eq!(weather.headers["Authorization"], "Bearer ${WEATHER_TOKEN}");
    assert_eq!(weather.description, None);
    assert_eq!(remote.validate(), Ok(()));
    assert_eq!(
        both.validate().unwrap_err(),
        [ValidationError {
            path: "/agent_tools/weather".into(),
            kind: ValidationErrorKind::AgentToolSource,
        }]
    );
    assert_eq!(
        local_without_description.validate().unwrap_err(),
        [ValidationError {
            path: "/agent_tools/weather/description".into(),
            kind: ValidationErrorKind::MissingAgentDescription,
        }]
    );
}
