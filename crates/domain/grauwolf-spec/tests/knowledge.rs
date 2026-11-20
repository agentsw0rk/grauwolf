//! Knowledge sources for RAG in specs (F08.8, ADR-0038).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ModelSpec, ToolOrigin, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(knowledge: Value, subgraph_tools: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "tools": [ { "name": "add", "description": "Addiert", "args": { "type": "object" }, "code": "Ok(String::new())" } ],
            "knowledge": knowledge,
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
fn parses_knowledge_with_defaults_and_lets_subgraphs_use_the_search_tool() {
    let spec = spec(
        json!({
            "sources": ["docs/", "README.md"],
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
        }),
        json!(["add", "search_knowledge"]),
    );

    let knowledge = spec.knowledge.as_ref().unwrap();
    assert_eq!(knowledge.sources, ["docs/", "README.md"]);
    assert_eq!(
        knowledge.embedding_model,
        ModelSpec {
            provider: "ollama".into(),
            id: "nomic-embed-text".into(),
            fallbacks: Vec::new(),
            temperature: None,
            think: None,
        }
    );
    assert_eq!(knowledge.tool, "search_knowledge");
    assert_eq!(knowledge.top_k, 3);
    assert_eq!(knowledge.chunk_size, 1000);
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn reports_search_tool_name_collisions_and_missing_sources() {
    let spec = spec(
        json!({
            "sources": [],
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
            "tool": "add"
        }),
        json!(["add"]),
    );

    let errors = spec.validate().unwrap_err();

    assert_eq!(
        errors,
        [
            ValidationError {
                path: "/knowledge/tool".into(),
                kind: ValidationErrorKind::DuplicateToolName {
                    name: "add".into(),
                    other_origin: ToolOrigin::Own,
                    other_pointer: "/tools/0/name".into()
                },
            },
            ValidationError {
                path: "/knowledge/sources".into(),
                kind: ValidationErrorKind::NoKnowledgeSources,
            },
        ]
    );
}

fn store_errors(extra: Value) -> Vec<ValidationError> {
    let mut knowledge = json!({
        "sources": ["docs/"],
        "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
    });
    for (key, value) in extra.as_object().unwrap() {
        knowledge[key] = value.clone();
    }
    spec(knowledge, json!([]))
        .validate()
        .err()
        .unwrap_or_default()
}

fn invalid_store(path: &str, reason: &str) -> ValidationError {
    ValidationError {
        path: path.into(),
        kind: ValidationErrorKind::InvalidStore {
            reason: reason.into(),
        },
    }
}

#[test]
fn accepts_the_postgres_store_with_a_variable_name() {
    let spec = spec(
        json!({
            "sources": ["docs/"],
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
            "store": { "kind": "postgres", "url_env": "PGVECTOR_URL" }
        }),
        json!([]),
    );

    let store = spec.knowledge.as_ref().unwrap().store.as_ref().unwrap();
    assert_eq!(store.kind, grauwolf_spec::KnowledgeStoreKind::Postgres);
    assert_eq!(store.url_env.as_deref(), Some("PGVECTOR_URL"));
    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn the_file_store_is_the_default_and_needs_no_url() {
    assert_eq!(store_errors(json!({ "store": { "kind": "file" } })), []);
    assert_eq!(store_errors(json!({})), []);
}

#[test]
fn reports_a_postgres_store_without_url_env() {
    assert_eq!(
        store_errors(json!({ "store": { "kind": "postgres" } })),
        [invalid_store(
            "/knowledge/store/url_env",
            "kind `postgres` needs `url_env`, the name of the environment variable with the connection URL"
        )]
    );
}

#[test]
fn reports_a_url_in_place_of_the_variable_name_without_repeating_it() {
    let errors = store_errors(
        json!({ "store": { "kind": "postgres", "url_env": "postgres://user:secret@host/db" } }),
    );

    assert_eq!(
        errors,
        [invalid_store(
            "/knowledge/store/url_env",
            "`url_env` must be the name of an environment variable (letters, digits, `_`), not the URL itself"
        )]
    );
    assert!(!format!("{errors:?}").contains("secret"));
}

#[test]
fn reports_a_url_env_on_the_file_store_and_an_index_with_postgres() {
    assert_eq!(
        store_errors(json!({ "store": { "kind": "file", "url_env": "X" } })),
        [invalid_store(
            "/knowledge/store/url_env",
            "kind `file` takes no `url_env`"
        )]
    );
    assert_eq!(
        store_errors(json!({
            "store": { "kind": "postgres", "url_env": "PGVECTOR_URL" },
            "index": "index.json"
        })),
        [invalid_store(
            "/knowledge/index",
            "`index` applies to the store kind `file`; PostgreSQL keeps the index itself"
        )]
    );
}

fn url_errors(urls: Value) -> Vec<ValidationError> {
    let knowledge = json!({
        "urls": urls,
        "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
    });
    spec(knowledge, json!([]))
        .validate()
        .err()
        .unwrap_or_default()
}

fn invalid_url(path: &str, reason: &str) -> ValidationError {
    ValidationError {
        path: path.into(),
        kind: ValidationErrorKind::InvalidUrl {
            reason: reason.into(),
        },
    }
}

#[test]
fn accepts_urls_without_sources() {
    assert_eq!(
        url_errors(json!([
            "https://example.com/a.html",
            "http://localhost:8080/b.md"
        ])),
        []
    );
}

#[test]
fn reports_unusable_knowledge_urls_without_repeating_them() {
    let secure = "must start with https:// (http:// only for localhost and 127.0.0.1)";
    assert_eq!(
        url_errors(json!([
            "http://example.com/a",
            "ftp://example.com/a",
            "https://user:secret@example.com/a",
            "https://example.com/a#part",
            "https:///a",
            "https://example.com/ok",
            "https://example.com/ok"
        ])),
        [
            invalid_url("/knowledge/urls/0", secure),
            invalid_url("/knowledge/urls/1", secure),
            invalid_url(
                "/knowledge/urls/2",
                "must not contain credentials (user:password@)"
            ),
            invalid_url("/knowledge/urls/3", "must not contain a fragment (#...)"),
            invalid_url("/knowledge/urls/4", "has no host"),
            invalid_url("/knowledge/urls/6", "is listed twice"),
        ]
    );
}

#[test]
fn needs_at_least_one_source_or_url() {
    assert_eq!(url_errors(json!([])).len(), 1);
}

#[test]
fn parses_the_minimum_similarity_and_reports_values_outside_zero_to_one() {
    let knowledge = |value: Value| {
        json!({
            "sources": ["docs/"], "min_similarity": value,
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
        })
    };
    let ok = spec(knowledge(json!(0.4)), json!([]));
    assert_eq!(ok.knowledge.as_ref().unwrap().min_similarity, Some(0.4));
    assert_eq!(ok.validate(), Ok(()));

    assert_eq!(
        spec(knowledge(json!(1.5)), json!([])).validate(),
        Err(vec![ValidationError {
            path: "/knowledge/min_similarity".into(),
            kind: ValidationErrorKind::InvalidLimit {
                reason: "`min_similarity` must be between 0 and 1".into()
            },
        }])
    );
}

#[test]
fn knowledge_as_context_only_needs_inject_and_without_either_is_an_error() {
    let knowledge = |extra: Value| {
        let mut base = json!({
            "sources": ["docs/"],
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
        });
        for (key, value) in extra.as_object().unwrap() {
            base[key] = value.clone();
        }
        base
    };
    let defaults = spec(knowledge(json!({})), json!([]));
    let k = defaults.knowledge.as_ref().unwrap();
    assert_eq!((k.inject, k.tool_enabled), (false, true));
    assert_eq!(defaults.external_tools(), ["search_knowledge"]);

    let context_only = spec(
        knowledge(json!({ "inject": true, "tool_enabled": false })),
        json!([]),
    );
    assert_eq!(context_only.validate(), Ok(()));
    assert!(context_only.external_tools().is_empty());

    let unused = spec(knowledge(json!({ "tool_enabled": false })), json!([]));
    assert_eq!(
        unused.validate(),
        Err(vec![ValidationError {
            path: "/knowledge/tool_enabled".into(),
            kind: ValidationErrorKind::UnusedKnowledge,
        }])
    );
}
