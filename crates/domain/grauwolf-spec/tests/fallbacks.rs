//! Fallback models in the spec (ADR-0088): parsing, checking, providers.

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationErrorKind};
use serde_json::json;

fn spec(model: serde_json::Value, embedding: serde_json::Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "ausweichend",
            "model": model,
            "knowledge": { "sources": ["docs"], "embedding_model": embedding },
            "subgraphs": {
                "schritt": {
                    "input": "string", "output": "string",
                    "model": { "provider": "gemini", "id": "g", "fallbacks": [ { "provider": "mistral", "id": "m" } ] },
                    "graph": {
                        "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                        "edges": [
                            { "from": "start", "to": "call_llm" },
                            { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                        ]
                    }
                }
            },
            "graph": {
                "nodes": [ { "id": "schritt", "kind": "subgraph", "params": { "subgraph": "schritt" } } ],
                "edges": [ { "from": "start", "to": "schritt" }, { "from": "schritt", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn checks_fallbacks_and_counts_their_providers() {
    let good = spec(
        json!({ "provider": "anthropic", "id": "claude-haiku-5-5",
                "fallbacks": [ { "provider": "openai", "id": "gpt-4o-mini" }, { "provider": "ollama", "id": "qwen3.5:9b" } ] }),
        json!({ "provider": "ollama", "id": "nomic-embed-text" }),
    );
    let bad = spec(
        json!({ "provider": "anthropic", "id": "a",
                "fallbacks": [ { "provider": "anthropic", "id": "a" }, { "provider": "openai", "id": "o" }, { "provider": "openai", "id": "o" } ] }),
        json!({ "provider": "ollama", "id": "e", "fallbacks": [ { "provider": "openai", "id": "e" } ] }),
    );

    assert_eq!(good.validate(), Ok(()));
    assert_eq!(
        good.providers(),
        ["anthropic", "gemini", "mistral", "ollama", "openai"]
    );
    let uses: Vec<String> = good
        .provider_uses()
        .into_iter()
        .map(|(pointer, _)| pointer)
        .collect();
    assert!(
        uses.contains(&"/model/fallbacks/0/provider".to_owned()),
        "{uses:?}"
    );
    assert!(
        uses.contains(&"/subgraphs/schritt/model/fallbacks/0/provider".to_owned()),
        "{uses:?}"
    );
    let errors = bad.validate().unwrap_err();
    let found: Vec<(String, String)> = errors
        .iter()
        .map(|error| (error.path.clone(), error.kind.to_string()))
        .collect();
    assert_eq!(
        found,
        [
            (
                "/model/fallbacks/0".to_owned(),
                "fallback model 'anthropic:a' is already listed".to_owned()
            ),
            (
                "/model/fallbacks/2".to_owned(),
                "fallback model 'openai:o' is already listed".to_owned()
            ),
            (
                "/knowledge/embedding_model/fallbacks".to_owned(),
                "embedding models have no fallback models: a different model would produce different vectors".to_owned()
            ),
        ]
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error.kind, ValidationErrorKind::DuplicateFallback { .. }))
    );
}

#[test]
fn checks_fallbacks_of_model_switches_too() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "wechselnd",
            "model": { "provider": "ollama", "id": "m" },
            "graph": {
                "nodes": [
                    { "id": "switch", "kind": "set_model",
                      "params": { "provider": "anthropic", "id": "a", "value": "string",
                                  "fallbacks": [ { "provider": "gemini", "id": "g" }, { "provider": "gemini", "id": "g" } ] } },
                    { "id": "call_llm", "kind": "llm_request" }
                ],
                "edges": [
                    { "from": "start", "to": "switch" },
                    { "from": "switch", "to": "call_llm" },
                    { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let errors = spec.validate().unwrap_err();

    assert_eq!(spec.providers(), ["anthropic", "gemini", "ollama"]);
    let uses: Vec<String> = spec
        .provider_uses()
        .into_iter()
        .map(|(pointer, _)| pointer)
        .collect();
    assert!(
        uses.contains(&"/graph/nodes/0/params/fallbacks/1/provider".to_owned()),
        "{uses:?}"
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].path, "/graph/nodes/0/params/fallbacks/1");
}
