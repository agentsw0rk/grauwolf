//! The `think` tool (ADR-0098).

use grauwolf_builtin_tools::think_tool;
use grauwolf_tools::ToolError;
use serde_json::json;

#[tokio::test]
async fn answers_ok_to_any_thought() {
    let tool = think_tool();

    let answer = tool
        .call_json(json!({ "thought": "The user wants the sum, so I add first." }))
        .await
        .unwrap();

    assert_eq!(tool.name(), "think");
    assert_eq!(answer, "ok");
}

#[tokio::test]
async fn needs_a_thought() {
    let error = think_tool().call_json(json!({})).await.unwrap_err();

    assert!(matches!(error, ToolError::InvalidArgs { .. }), "{error}");
}
