//! Limits per run in the spec (ADR-0078).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, LimitsSpec, ValidationError, ValidationErrorKind};
use serde_json::{Value, json};

fn parse(limits: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "limits": limits,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

#[test]
fn parses_limits_and_reports_impossible_values() {
    let spec = parse(
        json!({ "max_steps": 20, "max_tool_calls": 10, "max_tokens": 5000, "max_cost": 0.05, "max_prompt_tokens": 8000 }),
    );
    let bad =
        parse(json!({ "max_steps": 0, "max_tokens": 0, "max_cost": 0.0, "max_prompt_tokens": 0 }));

    assert_eq!(
        spec.limits,
        Some(LimitsSpec {
            max_steps: Some(20),
            max_tool_calls: Some(10),
            max_tokens: Some(5000),
            max_cost: Some(0.05),
            max_prompt_tokens: Some(8000),
        })
    );
    assert_eq!(spec.validate(), Ok(()));
    let errors = bad.validate().unwrap_err();
    let paths: Vec<&str> = errors.iter().map(|error| error.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/limits/max_steps",
            "/limits/max_tokens",
            "/limits/max_prompt_tokens",
            "/limits/max_cost"
        ]
    );
    assert!(matches!(
        &errors[0],
        ValidationError {
            kind: ValidationErrorKind::InvalidLimit { .. },
            ..
        }
    ));
}
