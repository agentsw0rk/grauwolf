//! Node kind `plan`: an LLM planner whose steps a subgraph runs (ADR-0039).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(plan_params: Value, step_output: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "planner",
            "model": { "provider": "ollama", "id": "m" },
            "types": { "Answer": { "type": "object", "properties": { "text": { "type": "string" } } } },
            "subgraphs": { "step": {
                "output": step_output,
                "graph": {
                    "nodes": [ { "id": "call_llm", "kind": "llm_request" } ],
                    "edges": [ { "from": "start", "to": "call_llm" },
                               { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
                }
            } },
            "graph": {
                "nodes": [ { "id": "solve", "kind": "plan", "params": plan_params } ],
                "edges": [ { "from": "start", "to": "solve" }, { "from": "solve", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn accepts_a_plan_node_whose_steps_run_in_a_text_subgraph() {
    let spec = spec(
        json!({ "subgraph": "step", "critic": true, "max_iterations": 8 }),
        "string",
    );

    assert_eq!(spec.validate(), Ok(()));
}

#[test]
fn rejects_unknown_plan_params_and_step_subgraphs_without_text_output() {
    let unknown_param = spec(json!({ "subgraph": "step", "depth": 3 }), "string");
    let typed_step = spec(json!({ "subgraph": "step" }), "Answer");

    // Only the errors at the `plan` node; the typed subgraph also has type errors of its own.
    let errors = [unknown_param, typed_step].map(|spec| {
        let mut errors = spec.validate().unwrap_err();
        errors.retain(|error| error.path == "/graph/nodes/0/params");
        errors
    });

    assert_eq!(
        errors,
        [
            vec![ValidationError {
                path: "/graph/nodes/0/params".into(),
                kind: ValidationErrorKind::InvalidParams {
                    kind: "plan".into(),
                    reason: "/depth: unknown parameter".into(),
                },
            }],
            vec![ValidationError {
                path: "/graph/nodes/0/params".into(),
                kind: ValidationErrorKind::InvalidParams {
                    kind: "plan".into(),
                    reason: "subgraph 'step' executes plan steps and must be string → string"
                        .into(),
                },
            }],
        ]
    );
}
