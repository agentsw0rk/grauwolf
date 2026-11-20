//! Long-term memory in specs (ADR-0058).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ModelSpec, ToolOrigin, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(memory: Value, subgraph_tools: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "long_term_memory": memory,
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
fn parses_long_term_memory_with_defaults_and_offers_both_tools() {
    let spec = spec(
        json!({
            "store": "memory.json",
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
        }),
        json!(["remember", "recall"]),
    );

    let memory = spec.long_term_memory.as_ref().unwrap();
    assert_eq!(memory.store.as_deref(), Some("memory.json"));
    assert_eq!(
        memory.embedding_model,
        ModelSpec {
            provider: "ollama".into(),
            id: "nomic-embed-text".into(),
            fallbacks: Vec::new(),
            temperature: None,
            think: None,
        }
    );
    assert_eq!(memory.remember_tool, "remember");
    assert_eq!(memory.recall_tool, "recall");
    assert_eq!(memory.top_k, 3);
    assert_eq!(spec.external_tools(), ["remember", "recall"]);
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn reports_memory_tool_names_that_are_not_snake_case() {
    let spec = spec(
        json!({
            "store": "memory.json",
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
            "remember_tool": "Merke-Dir",
            "recall_tool": "Merke-Dir"
        }),
        json!([]),
    );

    let errors = spec.validate().unwrap_err();

    assert_eq!(
        errors,
        [
            ValidationError {
                path: "/long_term_memory/remember_tool".into(),
                kind: ValidationErrorKind::InvalidId {
                    id: "Merke-Dir".into()
                },
            },
            ValidationError {
                path: "/long_term_memory/recall_tool".into(),
                kind: ValidationErrorKind::InvalidId {
                    id: "Merke-Dir".into()
                },
            },
        ]
    );
}

#[test]
fn reports_memory_tool_names_that_collide() {
    let spec = spec(
        json!({
            "store": "memory.json",
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
            "recall_tool": "remember"
        }),
        json!([]),
    );

    let errors = spec.validate().unwrap_err();

    assert_eq!(
        errors,
        [ValidationError {
            path: "/long_term_memory/recall_tool".into(),
            kind: ValidationErrorKind::DuplicateToolName {
                name: "remember".into(),
                other_origin: ToolOrigin::Memory,
                other_pointer: "/long_term_memory/remember_tool".into()
            },
        }]
    );
}

/// Automatic memory (ADR-0069): off by default, each switch can be enabled separately.
#[test]
fn reads_automatic_recall_and_remember() {
    let model = json!({ "provider": "ollama", "id": "nomic-embed-text" });
    let plain = spec(
        json!({ "store": "m.json", "embedding_model": model }),
        json!([]),
    );
    let automatic = spec(
        json!({ "store": "m.json", "embedding_model": model,
                "auto_recall": true, "auto_remember": true }),
        json!([]),
    );

    let plain = plain.long_term_memory.unwrap();
    let automatic = automatic.long_term_memory.unwrap();
    assert!(!plain.auto_recall && !plain.auto_remember);
    assert!(automatic.auto_recall && automatic.auto_remember);
}

/// Memory without tools (ADR-0069): only useful with automatic recall or storage.
#[test]
fn offers_no_memory_tools_when_asked_to_and_needs_automatic_memory_then() {
    let model = json!({ "provider": "ollama", "id": "nomic-embed-text" });
    let automatic = spec(
        json!({ "store": "m.json", "embedding_model": model, "tools": false, "auto_recall": true }),
        json!([]),
    );
    let useless = spec(
        json!({ "store": "m.json", "embedding_model": model, "tools": false }),
        json!([]),
    );

    assert!(
        automatic.external_tools().is_empty(),
        "{:?}",
        automatic.external_tools()
    );
    assert!(automatic.validate().is_ok());
    let errors = useless.validate().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.path == "/long_term_memory/tools"
                && error.kind == ValidationErrorKind::UnusedMemory),
        "{errors:?}"
    );
}

#[test]
fn counts_remembering_compressed_facts_as_using_the_memory() {
    let spec = spec(
        json!({
            "store": "memory.json",
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
            "tools": false,
            "remember_compressed": true
        }),
        json!([]),
    );

    assert!(spec.long_term_memory.as_ref().unwrap().remember_compressed);
    assert_eq!(spec.validate(), Ok(()));
}

fn memory_errors(memory: Value) -> Vec<ValidationError> {
    spec(memory, json!([])).validate().err().unwrap_or_default()
}

fn invalid_store(path: &str, reason: &str) -> ValidationError {
    ValidationError {
        path: path.into(),
        kind: ValidationErrorKind::InvalidStore {
            reason: reason.into(),
        },
    }
}

fn vector_memory(vector_store: Value) -> Value {
    json!({
        "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
        "vector_store": vector_store
    })
}

#[test]
fn parses_the_postgres_vector_store_with_expiry_and_minimum_similarity() {
    let spec = spec(
        vector_memory(json!({
            "kind": "postgres", "url_env": "PGVECTOR_URL", "ttl_days": 90, "min_similarity": 0.4
        })),
        json!(["remember", "recall"]),
    );

    let memory = spec.long_term_memory.as_ref().unwrap();
    assert_eq!(memory.store, None);
    let vector = memory.vector_store.as_ref().unwrap();
    assert_eq!(vector.kind, grauwolf_spec::VectorStoreKind::Postgres);
    assert_eq!(vector.url_env, "PGVECTOR_URL");
    assert_eq!(vector.ttl_days, Some(90));
    assert_eq!(vector.min_similarity, Some(0.4));
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn ttl_and_minimum_similarity_are_optional() {
    let spec = spec(
        vector_memory(json!({ "kind": "postgres", "url_env": "PGVECTOR_URL" })),
        json!(["remember", "recall"]),
    );

    let vector = spec.long_term_memory.unwrap().vector_store.unwrap();
    assert_eq!((vector.ttl_days, vector.min_similarity), (None, None));
}

#[test]
fn reports_neither_and_both_of_store_and_vector_store() {
    let both = invalid_store(
        "/long_term_memory",
        "give exactly one of `store` (JSON file) and `vector_store` (PostgreSQL with pgvector)",
    );
    assert_eq!(
        memory_errors(json!({ "embedding_model": { "provider": "ollama", "id": "m" } })),
        std::slice::from_ref(&both)
    );
    let mut memory = vector_memory(json!({ "kind": "postgres", "url_env": "PGVECTOR_URL" }));
    memory["store"] = json!("memory.json");
    assert_eq!(memory_errors(memory), [both]);
}

#[test]
fn reports_a_bad_url_env_ttl_and_minimum_similarity() {
    let errors = memory_errors(vector_memory(json!({
        "kind": "postgres", "url_env": "postgres://u:secret@h/db", "ttl_days": 0,
        "min_similarity": 1.5
    })));

    assert_eq!(
        errors,
        [
            invalid_store(
                "/long_term_memory/vector_store/url_env",
                "`url_env` must be the name of an environment variable (letters, digits, `_`), not the URL itself"
            ),
            invalid_store(
                "/long_term_memory/vector_store/ttl_days",
                "`ttl_days` must be at least 1; leave it out for facts that never expire"
            ),
            invalid_store(
                "/long_term_memory/vector_store/min_similarity",
                "`min_similarity` must be between 0 and 1"
            ),
        ]
    );
    assert!(!format!("{errors:?}").contains("secret"));
}
