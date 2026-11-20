//! Tool contract and registry (F03.1, F03.6, F03.7, F03.10).

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_tools::{Tool, ToolError, ToolRegistry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// Adds two numbers.
struct Add;

#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    a: f64,
    b: f64,
}

impl Tool for Add {
    type Args = AddArgs;
    type Output = f64;

    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> &str {
        "Adds two numbers"
    }

    fn call(&self, args: AddArgs) -> BoxFuture<'_, Result<f64, ToolError>> {
        Box::pin(async move { Ok(args.a + args.b) })
    }
}

#[test]
fn calls_a_registered_tool_by_name_with_json_arguments() {
    let registry = ToolRegistry::builder().tool(Add).build();

    let result = block_on(registry.call("add", json!({ "a": 2, "b": 3 }))).unwrap();

    assert_eq!(result, "5.0");
}

#[test]
fn describes_tools_with_name_description_and_argument_schema() {
    let registry = ToolRegistry::builder().tool(Add).build();

    let descriptors = registry.descriptors();

    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].name, "add");
    assert_eq!(descriptors[0].description, "Adds two numbers");
    assert_eq!(descriptors[0].parameters["type"], "object");
    assert_eq!(
        descriptors[0].parameters["properties"]["a"]["type"],
        "number"
    );
    assert_eq!(descriptors[0].parameters["required"], json!(["a", "b"]));
}

#[test]
fn reports_unknown_tools_and_invalid_arguments_as_tool_errors() {
    let registry = ToolRegistry::builder().tool(Add).build();

    let unknown = block_on(registry.call("mul", json!({}))).unwrap_err();
    let invalid = block_on(registry.call("add", json!({ "a": "two" }))).unwrap_err();

    assert!(
        matches!(&unknown, ToolError::UnknownTool { name } if name == "mul"),
        "{unknown}"
    );
    assert!(
        matches!(&invalid, ToolError::InvalidArgs { tool, .. } if tool == "add"),
        "{invalid}"
    );
}

#[test]
fn offers_tools_put_on_its_shelf_later_also_to_clones() {
    let shelf = grauwolf_tools::ToolShelf::default();
    let registry = ToolRegistry::builder().build().with_shelf(shelf.clone());
    let fork = registry.clone();

    shelf.put(std::sync::Arc::new(Add)).unwrap();

    assert_eq!(fork.descriptors()[0].name, "add");
    let result = block_on(fork.call("add", json!({ "a": 1, "b": 1 }))).unwrap();
    assert_eq!(result, "2.0");
    let again = shelf.put(std::sync::Arc::new(Add)).unwrap_err();
    assert!(again.contains("add"), "{again}");
}

/// Carries a system prompt addition, like the skill tool `load_skill` (ADR-0103).
struct Noted(&'static str, Option<&'static str>);

impl grauwolf_tools::DynTool for Noted {
    fn name(&self) -> &str {
        self.0
    }

    fn descriptor(&self) -> grauwolf_prompt::ToolDescriptor {
        grauwolf_prompt::ToolDescriptor {
            name: self.0.into(),
            description: String::new(),
            parameters: json!({ "type": "object" }),
        }
    }

    fn call_json(&self, _args: serde_json::Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok(String::new()) })
    }

    fn prompt_addition(&self) -> Option<String> {
        self.1.map(str::to_owned)
    }
}

#[test]
fn the_registry_joins_the_prompt_additions_of_its_tools_by_tool_name() {
    let registry = ToolRegistry::builder()
        .tool(Noted("b_tool", Some("second")))
        .tool(Noted("a_tool", Some("first")))
        .tool(Noted("plain", None))
        .tool(Add)
        .build();
    assert_eq!(
        registry.prompt_addition().as_deref(),
        Some("first\n\nsecond")
    );
    assert_eq!(
        ToolRegistry::builder().tool(Add).build().prompt_addition(),
        None
    );
}
