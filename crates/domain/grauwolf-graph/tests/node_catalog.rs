//! Node catalog (ADR-0018).

use grauwolf_graph::{ConditionCatalog, DataType, NodeCatalog};
use serde_json::Value;

#[test]
fn builtin_catalog_describes_llm_request_as_string_to_message() {
    let catalog = NodeCatalog::builtin();

    let descriptor = catalog
        .get("llm_request")
        .expect("llm_request missing from the catalog");

    assert_eq!(
        descriptor.signature(&Value::Null).unwrap(),
        (DataType::String, DataType::Message)
    );
}

#[test]
fn builtin_catalogs_describe_the_tool_loop() {
    let nodes = NodeCatalog::builtin();
    let conditions = ConditionCatalog::builtin();
    let signature = |kind: &str| nodes.get(kind).unwrap().signature(&Value::Null).unwrap();

    assert_eq!(
        conditions
            .get("on_tool_call")
            .unwrap()
            .signature(&Value::Null, &DataType::Message),
        Ok(DataType::ToolCall)
    );
    assert_eq!(
        signature("execute_tool"),
        (DataType::ToolCall, DataType::ToolResult)
    );
    assert_eq!(
        signature("llm_send_tool_result"),
        (DataType::ToolResult, DataType::Message)
    );
}

#[test]
fn structured_request_produces_the_named_type_from_its_params() {
    let catalog = NodeCatalog::builtin();
    let descriptor = catalog.get("llm_request_structured").unwrap();

    assert_eq!(
        descriptor.signature(&serde_json::json!({ "type": "Ticket" })),
        Ok((DataType::String, DataType::Named("Ticket".into())))
    );
    assert!(matches!(
        descriptor.signature(&Value::Null),
        Err(grauwolf_graph::CatalogError::InvalidParams { .. })
    ));
}

#[test]
fn example_params_of_every_kind_have_a_valid_signature() {
    let catalog = NodeCatalog::builtin();
    let invalid: Vec<&str> = catalog
        .kinds()
        .filter(|kind| {
            let descriptor = catalog.get(kind).unwrap();
            descriptor.signature(&descriptor.example_params()).is_err()
        })
        .collect();

    assert!(
        invalid.is_empty(),
        "invalid example parameters: {invalid:?}"
    );
}

#[test]
fn structured_request_accepts_only_a_non_negative_retry_count() {
    let catalog = NodeCatalog::builtin();
    let descriptor = catalog.get("llm_request_structured").unwrap();

    assert!(
        descriptor
            .signature(&serde_json::json!({ "type": "Ticket", "retries": 2 }))
            .is_ok()
    );
    for retries in [serde_json::json!(-1), serde_json::json!("two")] {
        assert!(matches!(
            descriptor.signature(&serde_json::json!({ "type": "Ticket", "retries": retries })),
            Err(grauwolf_graph::CatalogError::InvalidParams { .. })
        ));
    }
}

#[test]
fn builtin_catalogs_describe_the_loop_for_several_tool_calls() {
    let nodes = NodeCatalog::builtin();
    let conditions = ConditionCatalog::builtin();
    let signature = |kind: &str| nodes.get(kind).unwrap().signature(&Value::Null).unwrap();

    assert_eq!(
        conditions
            .get("on_multiple_tool_calls")
            .unwrap()
            .signature(&Value::Null, &DataType::Message),
        Ok(DataType::ToolCalls)
    );
    assert_eq!(
        signature("execute_multiple_tools"),
        (DataType::ToolCalls, DataType::ToolResults)
    );
    assert_eq!(
        signature("llm_send_multiple_tool_results"),
        (DataType::ToolResults, DataType::Message)
    );
}

#[test]
fn streaming_request_has_the_signature_of_a_plain_request() {
    let catalog = NodeCatalog::builtin();

    assert_eq!(
        catalog
            .get("llm_request_streaming")
            .unwrap()
            .signature(&Value::Null),
        Ok((DataType::String, DataType::Message))
    );
}

#[test]
fn example_params_of_every_kind_satisfy_its_params_schema() {
    let catalog = NodeCatalog::builtin();
    let invalid: Vec<String> = catalog
        .kinds()
        .filter_map(|kind| {
            let descriptor = catalog.get(kind).unwrap();
            grauwolf_graph::params::check_params(
                &descriptor.params_schema(),
                &descriptor.example_params(),
            )
            .err()
            .map(|reason| format!("{kind}: {reason}"))
        })
        .collect();

    assert!(invalid.is_empty(), "{invalid:?}");
}

#[test]
fn history_compression_passes_its_value_through() {
    let nodes = NodeCatalog::builtin();
    let conditions = ConditionCatalog::builtin();
    let compress = nodes.get("compress_history").unwrap();

    assert_eq!(
        compress.signature(&serde_json::json!({ "value": "tool_result", "strategy": "summarize" })),
        Ok((DataType::ToolResult, DataType::ToolResult))
    );
    assert_eq!(
        compress.signature(
            &serde_json::json!({ "value": "Ticket", "strategy": "keep_last", "keep": 4 })
        ),
        Ok((
            DataType::Named("Ticket".into()),
            DataType::Named("Ticket".into())
        ))
    );
    assert!(
        compress
            .signature(&serde_json::json!({ "value": "string", "strategy": "keep_last" }))
            .is_err(),
        "keep_last needs keep"
    );
    assert_eq!(
        conditions
            .get("on_history_longer")
            .unwrap()
            .signature(&serde_json::json!({ "messages": 20 }), &DataType::Message),
        Ok(DataType::Message)
    );
}

/// Moderation (ADR-0066): a dedicated model needs `provider` and `id` together.
#[test]
fn llm_moderate_accepts_a_model_only_with_provider_and_id() {
    let catalog = NodeCatalog::builtin();
    let moderate = catalog.get("llm_moderate").unwrap();
    let signature = |params: Value| moderate.signature(&params);

    assert_eq!(
        signature(serde_json::json!({ "type": "Moderation" })).unwrap(),
        (DataType::String, DataType::Named("Moderation".into()))
    );
    assert!(
        signature(serde_json::json!({ "type": "Moderation", "provider": "openai", "id": "omni-moderation-latest" }))
            .is_ok()
    );
    assert!(signature(serde_json::json!({ "type": "Moderation", "provider": "openai" })).is_err());
}

fn decide_signature(params: Value) -> Result<(DataType, DataType), grauwolf_graph::CatalogError> {
    NodeCatalog::builtin()
        .get("decide")
        .unwrap()
        .signature(&params)
}

fn invalid_reason(params: Value) -> String {
    match decide_signature(params) {
        Err(grauwolf_graph::CatalogError::InvalidParams { reason, .. }) => reason,
        other => panic!("expected invalid parameters, got {other:?}"),
    }
}

#[test]
fn decide_passes_the_value_of_the_named_type_through() {
    let params = serde_json::json!({
        "value": "Ticket",
        "kind": "choice",
        "question": "Which team?",
        "options": [{ "name": "billing", "description": "Invoices" }, { "name": "technical" }]
    });

    assert_eq!(
        decide_signature(params),
        Ok((
            DataType::Named("Ticket".into()),
            DataType::Named("Ticket".into())
        ))
    );
}

#[test]
fn decide_accepts_every_kind_with_its_own_parameters() {
    let value = |extra: Value| {
        let mut params = serde_json::json!({ "value": "string", "question": "Q?" });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        params
    };

    assert!(decide_signature(value(serde_json::json!({
        "kind": "yes_no", "threshold": 0.7, "yes": "Urgent", "no": "Not urgent", "decider": "system_one", "model": "kev-1"
    }))).is_ok());
    assert!(decide_signature(value(serde_json::json!({
        "kind": "score", "levels": ["low", "high"], "decider": "llm", "model": "ollama/qwen3.5:9b"
    }))).is_ok());
}

#[test]
fn decide_rejects_impossible_parameters() {
    let choice = |options: Value| serde_json::json!({ "value": "string", "kind": "choice", "question": "Q?", "options": options });
    let too_many: Vec<Value> = (0..256)
        .map(|n| serde_json::json!({ "name": format!("o{n}") }))
        .collect();

    assert!(
        invalid_reason(serde_json::json!({ "value": "string", "kind": "maybe", "question": "Q?" }))
            .contains("kind")
    );
    assert!(invalid_reason(choice(serde_json::json!([]))).contains("/options"));
    assert!(invalid_reason(choice(Value::Array(too_many))).contains("255"));
    assert!(
        invalid_reason(choice(
            serde_json::json!([{ "name": "a" }, { "name": "a" }])
        ))
        .contains("unique")
    );
    assert!(
        invalid_reason(choice(serde_json::json!([{ "name": "" }]))).contains("/options/0/name")
    );
    assert!(invalid_reason(serde_json::json!({ "value": "string", "kind": "score", "question": "Q?", "levels": ["a", "a"] })).contains("unique"));
    assert!(
        invalid_reason(serde_json::json!({ "value": "string", "kind": "score", "question": "Q?" }))
            .contains("/levels")
    );
    assert!(invalid_reason(serde_json::json!({ "value": "string", "kind": "yes_no", "question": "Q?", "threshold": 1.5 })).contains("/threshold"));
    assert!(invalid_reason(serde_json::json!({ "value": "string", "kind": "yes_no", "question": "Q?", "options": [{ "name": "a" }] })).contains("/options"));
    assert!(invalid_reason(serde_json::json!({ "value": "string", "kind": "yes_no", "question": "Q?", "decider": "oracle" })).contains("/decider"));
    assert!(invalid_reason(serde_json::json!({ "value": "string", "kind": "yes_no", "question": "Q?", "model": "qwen3.5:9b" })).contains("provider/id"));
    assert!(
        invalid_reason(serde_json::json!({ "value": "string", "kind": "yes_no", "question": "" }))
            .contains("/question")
    );
}

#[test]
fn on_decision_passes_any_value_through_and_needs_answers() {
    let conditions = ConditionCatalog::builtin();
    let condition = conditions.get("on_decision").unwrap();
    let ticket = DataType::Named("Ticket".into());

    assert_eq!(
        condition.signature(&serde_json::json!({ "answers": ["billing"] }), &ticket),
        Ok(ticket.clone())
    );
    for answers in [
        serde_json::json!([]),
        serde_json::json!([1]),
        serde_json::json!(["a", "a"]),
    ] {
        assert!(matches!(
            condition.signature(&serde_json::json!({ "answers": answers }), &ticket),
            Err(grauwolf_graph::CatalogError::InvalidParams { .. })
        ));
    }
}

#[test]
fn decide_accepts_temperature_and_think_for_the_decision_request() {
    let params = |extra: Value| {
        let mut params = serde_json::json!({ "value": "string", "kind": "yes_no", "question": "Q?", "model": "ollama/qwen3.5:9b" });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        params
    };

    assert!(
        decide_signature(params(
            serde_json::json!({ "temperature": 0, "think": false })
        ))
        .is_ok()
    );
    assert!(decide_signature(params(serde_json::json!({ "temperature": "cold" }))).is_err());
    assert!(decide_signature(params(serde_json::json!({ "think": "no" }))).is_err());
}

#[test]
fn execute_multiple_tools_accepts_a_boolean_sequential_flag() {
    let nodes = NodeCatalog::builtin();
    let schema = nodes.get("execute_multiple_tools").unwrap().params_schema();
    let check = |params: Value| grauwolf_graph::params::check_params(&schema, &params);

    assert_eq!(check(serde_json::json!({ "sequential": true })), Ok(()));
    assert_eq!(check(Value::Null), Ok(()));
    assert!(check(serde_json::json!({ "sequential": "yes" })).is_err());
}

#[test]
fn array_types_are_written_array_of_their_element() {
    let list = DataType::from_name("array<Topic>");

    assert_eq!(
        list,
        DataType::Array(Box::new(DataType::Named("Topic".into())))
    );
    assert_eq!(list.to_string(), "array<Topic>");
    assert_eq!(
        DataType::from_name("array<array<string>>").to_string(),
        "array<array<string>>"
    );
}

#[test]
fn agent_node_takes_and_returns_the_declared_types_of_its_sub_agent() {
    let catalog = NodeCatalog::builtin();
    let descriptor = catalog.get("agent").unwrap();

    assert_eq!(
        descriptor.signature(&serde_json::json!({ "agent": "writer" })),
        Ok((DataType::String, DataType::String))
    );
    assert_eq!(
        descriptor.signature(
            &serde_json::json!({ "agent": "writer", "input": "Topic", "output": "Article" })
        ),
        Ok((
            DataType::Named("Topic".into()),
            DataType::Named("Article".into())
        ))
    );
    for params in [
        serde_json::json!({}),
        serde_json::json!({ "agent": "" }),
        serde_json::json!({ "agent": "writer", "input": "array<string>" }),
        serde_json::json!({ "agent": "writer", "output": 3 }),
    ] {
        assert!(
            matches!(
                descriptor.signature(&params),
                Err(grauwolf_graph::CatalogError::InvalidParams { .. })
            ),
            "{params}"
        );
    }
}

#[test]
fn every_node_kind_and_condition_describes_itself_in_one_sentence() {
    let catalogs = grauwolf_graph::Catalogs::builtin();
    let nodes = catalogs
        .nodes
        .kinds()
        .map(|kind| (kind, catalogs.nodes.get(kind).unwrap().description()));
    let conditions = catalogs
        .conditions
        .kinds()
        .map(|kind| (kind, catalogs.conditions.get(kind).unwrap().description()));
    let missing: Vec<&str> = nodes
        .chain(conditions)
        .filter(|(_, description)| description.is_empty() || !description.ends_with('.'))
        .map(|(kind, _)| kind)
        .collect();

    assert!(missing.is_empty(), "{missing:?}");
}
