//! Tools from functions with `#[tool]` (F03.5, ADR-0003).

use futures_executor::block_on;
use grauwolf_macros::tool;
use grauwolf_tools::{DynTool, ToolError, ToolRegistry};
use serde_json::json;

/// Adds two numbers.
#[tool]
async fn add(
    #[arg("First number")] a: f64,
    #[arg("Second number")] b: f64,
) -> Result<f64, ToolError> {
    Ok(a + b)
}

/// Greets someone.
#[tool]
fn greet(name: String) -> Result<String, ToolError> {
    Ok(format!("Hello {name}"))
}

#[test]
fn creates_a_tool_with_name_description_and_documented_arguments() {
    let descriptor = Add.descriptor();

    assert_eq!(descriptor.name, "add");
    assert_eq!(descriptor.description, "Adds two numbers.");
    assert_eq!(descriptor.parameters["required"], json!(["a", "b"]));
    assert_eq!(
        descriptor.parameters["properties"]["a"]["description"],
        json!("First number")
    );
}

#[test]
fn calls_async_and_sync_functions_with_json_arguments() {
    let registry = ToolRegistry::builder().tool(Add).tool(Greet).build();

    assert_eq!(
        block_on(registry.call("add", json!({ "a": 2, "b": 3 }))).unwrap(),
        "5.0"
    );
    assert_eq!(
        block_on(registry.call("greet", json!({ "name": "world" }))).unwrap(),
        "\"Hello world\""
    );
}
