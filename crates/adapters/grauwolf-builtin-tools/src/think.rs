//! The `think` tool (ADR-0098): room for the model to reason between tool calls.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Names of the think tool.
pub const THINK_TOOLS: &[&str] = &["think"];

/// `think { thought }`: returns `ok`, has no side effects.
pub fn think_tool() -> Arc<dyn DynTool> {
    Arc::new(ThinkTool)
}

struct ThinkTool;

impl DynTool for ThinkTool {
    fn name(&self) -> &str {
        "think"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "think".to_owned(),
            description: "Use this tool to think about something. It does not obtain new \
                          information or change anything; it only records the thought. Use it \
                          to reason about the results of tool calls or to plan the next steps."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": { "thought": { "type": "string", "description": "Your thought" } },
                "required": ["thought"]
            }),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            if !args["thought"].is_string() {
                return Err(ToolError::InvalidArgs {
                    tool: "think".to_owned(),
                    reason: "missing field 'thought' (text)".to_owned(),
                });
            }
            Ok("ok".to_owned())
        })
    }
}
