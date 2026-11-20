//! `#[tool]` and `#[toolset]` through the facade (ADR-0003 addendum): only `grauwolf` needed.

use std::sync::{Arc, Mutex};

use grauwolf::tools::{DynTool, ToolError, ToolRegistry};
use serde_json::json;

/// Greets someone.
#[grauwolf::tool]
fn greet(#[arg("Name of the person")] name: String) -> Result<String, ToolError> {
    Ok(format!("Hello {name}"))
}

/// A counter that several tools share.
struct Counter(Mutex<u32>);

#[grauwolf::toolset]
impl Counter {
    /// Counts up by one.
    #[tool]
    async fn increment(&self) -> Result<u32, ToolError> {
        let mut value = self.0.lock().unwrap();
        *value += 1;
        Ok(*value)
    }
}

#[tokio::test]
async fn offers_tool_and_toolset_through_the_facade() {
    let counter = Arc::new(Counter(Mutex::new(0)));
    let registry = ToolRegistry::builder()
        .tool(Greet)
        .build()
        .extended(counter.tools());

    let greeting = registry
        .call("greet", json!({ "name": "World" }))
        .await
        .unwrap();
    let count = registry.call("increment", json!({})).await.unwrap();

    assert_eq!(greeting, "\"Hello World\"");
    assert_eq!(count, "1");
    assert_eq!(
        Greet.descriptor().parameters["properties"]["name"]["description"],
        "Name of the person"
    );
}
