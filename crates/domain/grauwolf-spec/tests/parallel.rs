//! Node kind `parallel`: subgraphs run concurrently, results are merged (ADR-0045).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(params: Value) -> AgentSpec {
    let text_branch = json!({ "graph": {
        "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
        "edges": [ { "from": "start", "to": "call_llm" },
                   { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
    } });
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "perspectives",
            "model": { "provider": "ollama", "id": "m" },
            "types": { "Note": { "type": "object", "properties": { "text": { "type": "string" } } } },
            "subgraphs": {
                "optimist": text_branch,
                "pessimist": text_branch,
                "typed": { "output": "Note", "graph": { "nodes": [ { "id": "note", "kind": "llm_request_structured", "params": { "type": "Note" } } ],
                    "edges": [ { "from": "start", "to": "note" }, { "from": "note", "to": "finish" } ] } }
            },
            "graph": {
                "nodes": [ { "id": "views", "kind": "parallel", "params": params } ],
                "edges": [ { "from": "start", "to": "views" }, { "from": "views", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn param_errors(params: Value) -> Vec<String> {
    match spec(params).validate() {
        Ok(()) => Vec::new(),
        Err(errors) => errors
            .into_iter()
            .filter(|error| error.path == "/graph/nodes/0/params")
            .map(|error| error.kind.to_string())
            .collect(),
    }
}

#[test]
fn accepts_parallel_nodes_over_branches_with_the_same_signature() {
    for params in [
        json!({ "subgraphs": ["optimist", "pessimist"], "merge": "concat" }),
        json!({ "subgraphs": ["optimist", "pessimist"], "merge": "index", "index": 1 }),
        json!({ "subgraphs": ["optimist", "pessimist"], "merge": "llm_select", "criteria": "Most intelligent" }),
    ] {
        assert_eq!(spec(params.clone()).validate(), Ok(()), "{params}");
    }
}

#[test]
fn rejects_unknown_branches_mixed_signatures_and_bad_merges() {
    assert_eq!(
        spec(json!({ "subgraphs": ["optimist", "realist"], "merge": "concat" }))
            .validate()
            .unwrap_err(),
        [ValidationError {
            path: "/graph/nodes/0/params".into(),
            kind: ValidationErrorKind::UnknownSubgraph {
                name: "realist".into()
            },
        }]
    );
    for (params, reason) in [
        (
            json!({ "subgraphs": ["optimist"], "merge": "concat" }),
            "at least two",
        ),
        (
            json!({ "subgraphs": ["optimist", "typed"], "merge": "index", "index": 0 }),
            "the same signature",
        ),
        (
            json!({ "subgraphs": ["optimist", "pessimist"], "merge": "vote" }),
            "\"merge\"",
        ),
        (
            json!({ "subgraphs": ["optimist", "pessimist"], "merge": "index" }),
            "\"index\"",
        ),
        (
            json!({ "subgraphs": ["optimist", "pessimist"], "merge": "index", "index": 2 }),
            "\"index\"",
        ),
        (
            json!({ "subgraphs": ["optimist", "pessimist"], "merge": "concat", "extra": 1 }),
            "/extra",
        ),
    ] {
        let errors = param_errors(params.clone());
        assert!(
            errors.iter().any(|error| error.contains(reason)),
            "{params}: {errors:?}"
        );
    }
}
