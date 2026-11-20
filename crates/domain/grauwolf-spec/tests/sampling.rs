//! `temperature` and `think` in models (ADR-0097).

use grauwolf_spec::AgentSpecExt;
use grauwolf_spec::{AgentSpec, ValidationErrorKind};
use serde_json::{Value, json};

fn spec(model: Value, graph_nodes: Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "name": "sampling",
            "spec_version": 1,
            "model": model,
            "system_prompt": "Be brief.",
            "graph": {
                "nodes": graph_nodes,
                "edges": [ { "from": "start", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap()
}

fn errors(spec: &AgentSpec) -> Vec<(String, ValidationErrorKind)> {
    spec.validate()
        .unwrap_err()
        .into_iter()
        .map(|error| (error.path, error.kind))
        .collect()
}

#[test]
fn accepts_temperature_everywhere_and_think_for_ollama() {
    let good = spec(
        json!({ "provider": "ollama", "id": "q", "temperature": 0, "think": false,
                "fallbacks": [ { "provider": "openai", "id": "o", "temperature": 2.0 } ] }),
        json!([]),
    );

    assert_eq!(good.validate(), Ok(()));
    assert_eq!(good.model.temperature, Some(0.0));
    assert_eq!(good.model.think, Some(false));
    let again = AgentSpec::from_json(&good.to_json()).unwrap();
    assert_eq!(again.model, good.model);
}

#[test]
fn rejects_a_temperature_outside_0_to_2_and_think_for_other_providers() {
    let bad = spec(
        json!({ "provider": "openai", "id": "o", "temperature": 2.5, "think": true,
                "fallbacks": [ { "provider": "anthropic", "id": "a", "think": true },
                               { "provider": "ollama", "id": "q", "temperature": -0.1 } ] }),
        json!([]),
    );

    assert_eq!(
        errors(&bad),
        [
            (
                "/model/temperature".to_owned(),
                ValidationErrorKind::TemperatureOutOfRange
            ),
            (
                "/model/think".to_owned(),
                ValidationErrorKind::ThinkNotSupported {
                    provider: "openai".into()
                }
            ),
            (
                "/model/fallbacks/0/think".to_owned(),
                ValidationErrorKind::ThinkNotSupported {
                    provider: "anthropic".into()
                }
            ),
            (
                "/model/fallbacks/1/temperature".to_owned(),
                ValidationErrorKind::TemperatureOutOfRange
            ),
        ]
    );
}

#[test]
fn checks_the_sampling_of_set_model_nodes_too() {
    let spec = spec(
        json!({ "provider": "ollama", "id": "q" }),
        json!([ { "id": "switch", "kind": "set_model",
                  "params": { "provider": "openai", "id": "o", "value": "string", "temperature": 3, "think": true } } ]),
    );
    let spec = AgentSpec {
        graph: grauwolf_spec::GraphSpec {
            edges: serde_json::from_value(json!([
                { "from": "start", "to": "switch" }, { "from": "switch", "to": "finish" }
            ]))
            .unwrap(),
            ..spec.graph.clone()
        },
        ..spec
    };

    let found = errors(&spec);

    assert!(found.contains(&(
        "/graph/nodes/0/params/temperature".to_owned(),
        ValidationErrorKind::TemperatureOutOfRange
    )));
    assert!(found.contains(&(
        "/graph/nodes/0/params/think".to_owned(),
        ValidationErrorKind::ThinkNotSupported {
            provider: "openai".into()
        }
    )));
}
