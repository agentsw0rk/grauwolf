//! Description of tools that are offered to an LLM.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Description of a tool for the LLM (F03.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Name under which the LLM calls the tool.
    pub name: String,
    /// Description for the LLM.
    pub description: String,
    /// JSON schema of the arguments.
    pub parameters: Value,
}
