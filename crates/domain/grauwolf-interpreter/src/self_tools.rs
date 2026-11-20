//! Tool `create_tool`: the agent creates its own tools during the run (ADR-0075).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_spec::SelfToolsSpec;
use grauwolf_tools::{DynTool, NewTool, ToolDescriptor, ToolError, ToolFactory, ToolShelf};
use serde_json::{Value, json};

/// Creates tools via the factory and places them on the run's level.
pub(crate) struct CreateTool {
    pub(crate) spec: SelfToolsSpec,
    pub(crate) factory: Arc<dyn ToolFactory>,
    pub(crate) shelf: ToolShelf,
    /// Names of the tools that already exist at the start of the run.
    pub(crate) taken: Vec<String>,
}

impl CreateTool {
    fn invalid(&self, reason: impl Into<String>) -> ToolError {
        ToolError::InvalidArgs {
            tool: self.spec.tool.clone(),
            reason: reason.into(),
        }
    }

    /// Checks the arguments against name, limits and existing tools.
    fn new_tool(&self, args: Value) -> Result<NewTool, ToolError> {
        let text = |field: &str| {
            args[field]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| self.invalid(format!("`{field}` is missing or not text")))
        };
        let tool = NewTool {
            name: text("name")?,
            description: text("description")?,
            args: match &args["args"] {
                Value::Null => json!({ "type": "object" }),
                schema @ Value::Object(_) => schema.clone(),
                _ => return Err(self.invalid("`args` must be a JSON schema object")),
            },
            code: as_body(text("code")?),
        };
        if !is_snake_case(&tool.name) {
            return Err(self.invalid(format!(
                "name '{}' is not a snake_case identifier",
                tool.name
            )));
        }
        if tool.name == self.spec.tool || self.taken.contains(&tool.name) {
            return Err(self.invalid(format!("tool '{}' already exists", tool.name)));
        }
        if self.shelf.len() >= self.spec.max_tools {
            return Err(self.invalid(format!("at most {} new tools per run", self.spec.max_tools)));
        }
        if tool.code.len() > self.spec.max_code_bytes {
            return Err(self.invalid(format!(
                "code is longer than {} bytes",
                self.spec.max_code_bytes
            )));
        }
        Ok(tool)
    }
}

impl DynTool for CreateTool {
    fn name(&self) -> &str {
        &self.spec.tool
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.spec.tool.clone(),
            description: "Creates a new tool that is available from the next answer on. \
                `code` is only the body (without signature) of \
                `fn tool(args: serde_json::Value) -> Result<String, String>` in Rust, \
                e.g. `let n = args[\"n\"].as_i64().ok_or(\"n missing\")?; Ok((n * 2).to_string())`. \
                Only std and serde_json, no network, no files, no unsafe. \
                On compile errors the messages come back with line and column; \
                then improve the code."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "snake_case name of the new tool" },
                    "description": { "type": "string", "description": "What the tool does" },
                    "args": { "type": "object", "description": "JSON schema of the arguments" },
                    "code": { "type": "string", "description": "Rust body, reads `args` and returns `Ok(text)` or `Err(text)`" }
                },
                "required": ["name", "description", "args", "code"]
            }),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let tool = self.new_tool(args)?;
            let name = tool.name.clone();
            let built = self
                .factory
                .create(tool.clone())
                .await
                .map_err(ToolError::Execution)?;
            self.shelf
                .put_made(built, tool)
                .map_err(ToolError::Execution)?;
            Ok(format!("Tool '{name}' is available from now on."))
        })
    }
}

/// Models often write the whole function `fn tool(args: Value) -> Result<String, String>`
/// instead of only its body. Inserted into the body it would be just an inner function; the
/// appended call turns it into a valid body.
fn as_body(code: String) -> String {
    if code.trim_start().starts_with("fn tool(") {
        format!("{}\ntool(args)", code.trim_end())
    } else {
        code
    }
}

fn is_snake_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
