//! Node catalog: the single source of node kinds (ADR-0018).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::params::no_params;

/// Data type that flows over an edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataType {
    /// Free text.
    String,
    /// Response message of an LLM.
    Message,
    /// Call of a tool by the LLM.
    ToolCall,
    /// Result of a tool call.
    ToolResult,
    /// All tool calls of a response.
    ToolCalls,
    /// Results of several tool calls in call order.
    ToolResults,
    /// Named type from the `types` section of the spec; compared nominally (ADR-0022).
    Named(String),
    /// List of values of one type, written `array<T>` (ADR-0101).
    Array(Box<DataType>),
}

impl DataType {
    /// Data type for its name, as `Display` writes it; other names are named types.
    pub fn from_name(name: &str) -> Self {
        match name {
            "string" => DataType::String,
            "message" => DataType::Message,
            "tool_call" => DataType::ToolCall,
            "tool_result" => DataType::ToolResult,
            "tool_calls" => DataType::ToolCalls,
            "tool_results" => DataType::ToolResults,
            other => match other
                .strip_prefix("array<")
                .and_then(|rest| rest.strip_suffix('>'))
            {
                Some(element) => DataType::Array(Box::new(DataType::from_name(element))),
                None => DataType::Named(other.to_owned()),
            },
        }
    }
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataType::String => f.write_str("string"),
            DataType::Message => f.write_str("message"),
            DataType::ToolCall => f.write_str("tool_call"),
            DataType::ToolResult => f.write_str("tool_result"),
            DataType::ToolCalls => f.write_str("tool_calls"),
            DataType::ToolResults => f.write_str("tool_results"),
            DataType::Named(name) => f.write_str(name),
            DataType::Array(element) => write!(f, "array<{element}>"),
        }
    }
}

/// Error while evaluating a catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// The parameters do not fit the node kind.
    #[error("invalid parameters for '{kind}': {reason}")]
    InvalidParams { kind: String, reason: String },
    /// The input type does not fit the kind.
    #[error("'{kind}' expects input of type {expected}, got: {found}")]
    InputTypeMismatch {
        kind: String,
        expected: DataType,
        found: DataType,
    },
}

/// Signature function of a node kind: maps parameters to (input, output type).
type SignatureFn = fn(&Value) -> Result<(DataType, DataType), CatalogError>;

/// Description of a node kind.
#[derive(Debug, Clone)]
pub struct NodeKindDescriptor {
    kind: &'static str,
    description: &'static str,
    signature: SignatureFn,
    example_params: fn() -> Value,
    params_schema: fn() -> Value,
}

impl NodeKindDescriptor {
    /// Name of the node kind, as it appears in specs under `kind`.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// One sentence on what a node of this kind does, e.g. for an editor's palette.
    pub fn description(&self) -> &'static str {
        self.description
    }

    /// Valid example parameters, e.g. for conformance tests of implementations and
    /// templates. A type named in them is called `Example`.
    pub fn example_params(&self) -> Value {
        (self.example_params)()
    }

    /// JSON schema of the parameters; spec validation checks `params` against it.
    pub fn params_schema(&self) -> Value {
        (self.params_schema)()
    }

    /// Input and output type of a node of this kind with the given parameters.
    pub fn signature(&self, params: &Value) -> Result<(DataType, DataType), CatalogError> {
        (self.signature)(params)
    }
}

/// Catalog of all known node kinds.
#[derive(Debug, Clone, Default)]
pub struct NodeCatalog {
    kinds: BTreeMap<&'static str, NodeKindDescriptor>,
}

impl NodeCatalog {
    /// Catalog with the built-in node kinds.
    pub fn builtin() -> Self {
        let mut catalog = Self::default();
        catalog.insert(NodeKindDescriptor {
            kind: "llm_request",
            description: "Sends the text to the model and returns its answer, which may carry tool calls.",
            signature: |_| Ok((DataType::String, DataType::Message)),
            example_params: || Value::Null,
            params_schema: no_params,
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_request_best_of",
            description: "Asks the model for several answers and keeps the best one by `criteria`; only that one enters the history.",
            // Several responses, an evaluation picks the best one (ADR-0042).
            signature: |_| Ok((DataType::String, DataType::Message)),
            example_params: || serde_json::json!({ "choices": 3, "criteria": "The shortest" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "choices": { "type": "integer", "minimum": 1 },
                        "criteria": { "type": "string" }
                    },
                    "required": ["choices"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_request_only_tools",
            description: "Like `llm_request`, but the model must call a tool.",
            // Like `llm_request`, but a tool call is mandatory (ADR-0042).
            signature: |_| Ok((DataType::String, DataType::Message)),
            example_params: || Value::Null,
            params_schema: no_params,
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_request_force_tool",
            description: "Like `llm_request`, but the model must call the tool named in `tool`.",
            // Like `llm_request`, but the tool `params.tool` must be called (ADR-0042).
            signature: |_| Ok((DataType::String, DataType::Message)),
            example_params: || serde_json::json!({ "tool": "add" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "tool": { "type": "string" } },
                    "required": ["tool"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "set_model",
            description: "Switches the model for the rest of the run and passes the value through.",
            // Switches the model for the rest of the run, passes `value` through (ADR-0043).
            signature: |params| {
                let value = params["value"].as_str().ok_or_else(|| CatalogError::InvalidParams {
                    kind: "set_model".into(),
                    reason: "missing \"value\" with the type of the passed-through value".into(),
                })?;
                let data_type = DataType::from_name(value);
                Ok((data_type.clone(), data_type))
            },
            example_params: || serde_json::json!({ "provider": "ollama", "id": "qwen3.5:9b", "value": "string" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "provider": { "type": "string" },
                        "id": { "type": "string" },
                        "temperature": { "type": "number" },
                        "think": { "type": "boolean" },
                        "fallbacks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": { "provider": { "type": "string" }, "id": { "type": "string" }, "temperature": { "type": "number" }, "think": { "type": "boolean" } },
                                "required": ["provider", "id"],
                                "additionalProperties": false
                            }
                        },
                        "value": { "type": "string" }
                    },
                    "required": ["provider", "id", "value"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "store",
            description: "Stores the value in the agent storage under `key` and passes it through.",
            // Stores the value in the agent storage and passes it through (ADR-0044).
            signature: |params| {
                let value = DataType::from_name(params["value"].as_str().unwrap_or_default());
                Ok((value.clone(), value))
            },
            example_params: || serde_json::json!({ "key": "task", "value": "string" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "key": { "type": "string" }, "value": { "type": "string" } },
                    "required": ["key", "value"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "load",
            description: "Loads the value stored under `key` from the agent storage.",
            // Returns the value from the agent storage; the input is ignored (ADR-0044).
            signature: |params| {
                Ok((
                    DataType::from_name(params["value"].as_str().unwrap_or_default()),
                    DataType::from_name(params["type"].as_str().unwrap_or_default()),
                ))
            },
            example_params: || serde_json::json!({ "key": "task", "value": "string", "type": "string" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string" },
                        "value": { "type": "string" },
                        "type": { "type": "string" }
                    },
                    "required": ["key", "value", "type"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "decide",
            description: "Decides about the value (choice, yes/no or score) with System One or a model; the edges branch with `on_decision`.",
            // Asks a decider a typed question about the value, stores the verdict and passes
            // the value through unchanged (ADR-0100).
            signature: |params| {
                let invalid = |reason: String| CatalogError::InvalidParams {
                    kind: "decide".into(),
                    reason,
                };
                let value = params["value"].as_str().ok_or_else(|| {
                    invalid("missing \"value\" with the type of the passed-through value".into())
                })?;
                crate::decision_params::check_decide(params).map_err(invalid)?;
                let data_type = DataType::from_name(value);
                Ok((data_type.clone(), data_type))
            },
            example_params: || {
                serde_json::json!({
                    "value": "string",
                    "kind": "choice",
                    "question": "Which team handles the ticket?",
                    "options": [
                        { "name": "billing", "description": "Invoices and payments" },
                        { "name": "technical" }
                    ]
                })
            },
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" },
                        "kind": { "type": "string" },
                        "question": { "type": "string" },
                        "options": { "type": "array" },
                        "levels": { "type": "array" },
                        "threshold": { "type": "number", "minimum": 0 },
                        "yes": { "type": "string" },
                        "no": { "type": "string" },
                        "decider": { "type": "string" },
                        "model": { "type": "string" },
                        "temperature": { "type": "number" },
                        "think": { "type": "boolean" }
                    },
                    "required": ["value", "kind", "question"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "agent",
            description: "Runs a sub-agent from `agent_tools` as one step of the flow.",
            // Runs a sub-agent from `agent_tools` as a step, without the model deciding; the
            // types are those of the sub-agent's spec, declared here because a single spec
            // cannot read the others (ADR-0101).
            signature: |params| {
                let invalid = |reason: String| CatalogError::InvalidParams {
                    kind: "agent".into(),
                    reason,
                };
                if params["agent"].as_str().is_none_or(str::is_empty) {
                    return Err(invalid(
                        "missing \"agent\" with the name of an entry of agent_tools".into(),
                    ));
                }
                let type_of = |key: &str| match params.get(key) {
                    None => Ok(DataType::String),
                    Some(Value::String(name)) => match DataType::from_name(name) {
                        DataType::Array(_) => Err(invalid(format!(
                            "\"{key}\" is text or a named type, not a list"
                        ))),
                        data_type => Ok(data_type),
                    },
                    Some(_) => Err(invalid(format!("\"{key}\" must be a type name"))),
                };
                Ok((type_of("input")?, type_of("output")?))
            },
            example_params: || serde_json::json!({ "agent": "researcher" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "agent": { "type": "string" },
                        "input": { "type": "string" },
                        "output": { "type": "string" }
                    },
                    "required": ["agent"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_continue",
            description: "Asks the model to continue without appending the input.",
            // Requests again without appending the input; it is already in the history (ADR-0041).
            signature: |_| Ok((DataType::String, DataType::Message)),
            example_params: || Value::Null,
            params_schema: no_params,
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_reason",
            description: "Lets the model think about the value without tools and returns its text.",
            // Thinking without tools about text or tool results (ReAct, ADR-0041).
            signature: |params| {
                let value = params["value"].as_str().unwrap_or_default();
                match DataType::from_name(value) {
                    input @ (DataType::String | DataType::ToolResult | DataType::ToolResults) => {
                        Ok((input, DataType::String))
                    }
                    _ => Err(CatalogError::InvalidParams {
                        kind: "llm_reason".into(),
                        reason: "\"value\" must be \"string\", \"tool_result\" or \"tool_results\""
                            .into(),
                    }),
                }
            },
            example_params: || serde_json::json!({ "value": "tool_result", "prompt": "Think it through." }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" },
                        "prompt": { "type": "string" }
                    },
                    "required": ["value"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_verify",
            description: "Has the model check an answer; the verdict holds `correct`, `feedback` and `answer`.",
            // Checks a response; verdict with `correct`, `feedback`, `answer` (F05.13, ADR-0041).
            signature: |params| {
                let name = params["type"]
                    .as_str()
                    .ok_or_else(|| CatalogError::InvalidParams {
                        kind: "llm_verify".into(),
                        reason: "missing \"type\" with the name of the verdict type from `types`"
                            .into(),
                    })?;
                Ok((DataType::String, DataType::Named(name.into())))
            },
            example_params: || serde_json::json!({ "type": "Example", "prompt": "Check carefully." }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "type": { "type": "string" }, "prompt": { "type": "string" } },
                    "required": ["type"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_moderate",
            description: "Has the model moderate the text; the verdict holds `harmful`, `categories`, `text` and `message`.",
            // Checks text for harmful content; verdict with `harmful`, `categories`, `text`,
            // `message`; optionally a dedicated model via `provider` and `id` (F05.17, ADR-0066).
            signature: |params| {
                let name = params["type"]
                    .as_str()
                    .ok_or_else(|| CatalogError::InvalidParams {
                        kind: "llm_moderate".into(),
                        reason: "missing \"type\" with the name of the verdict type from `types`".into(),
                    })?;
                if params.get("provider").is_some() != params.get("id").is_some() {
                    return Err(CatalogError::InvalidParams {
                        kind: "llm_moderate".into(),
                        reason: "give \"provider\" and \"id\" only together".into(),
                    });
                }
                Ok((DataType::String, DataType::Named(name.into())))
            },
            example_params: || serde_json::json!({ "type": "Example" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "type": { "type": "string" },
                        "provider": { "type": "string" },
                        "temperature": { "type": "number" },
                        "think": { "type": "boolean" },
                        "fallbacks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": { "provider": { "type": "string" }, "id": { "type": "string" }, "temperature": { "type": "number" }, "think": { "type": "boolean" } },
                                "required": ["provider", "id"],
                                "additionalProperties": false
                            }
                        },
                        "id": { "type": "string" }
                    },
                    "required": ["type"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "get_field",
            description: "Takes one field of a structured value as text.",
            // A field of a structured value as text (ADR-0041).
            signature: |params| {
                let name = params["type"]
                    .as_str()
                    .ok_or_else(|| CatalogError::InvalidParams {
                        kind: "get_field".into(),
                        reason: "missing \"type\" with the name of a type from `types`".into(),
                    })?;
                Ok((DataType::Named(name.into()), DataType::String))
            },
            example_params: || serde_json::json!({ "type": "Example", "field": "feedback" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "type": { "type": "string" }, "field": { "type": "string" } },
                    "required": ["type", "field"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "execute_tool",
            description: "Runs the one tool call of the answer and returns its result.",
            signature: |_| Ok((DataType::ToolCall, DataType::ToolResult)),
            example_params: || Value::Null,
            params_schema: no_params,
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_send_tool_result",
            description: "Sends a tool result back to the model and returns its next answer.",
            signature: |_| Ok((DataType::ToolResult, DataType::Message)),
            example_params: || Value::Null,
            params_schema: no_params,
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_request_streaming",
            description: "Like `llm_request`, streaming the answer as text deltas.",
            signature: |_| Ok((DataType::String, DataType::Message)),
            example_params: || Value::Null,
            params_schema: no_params,
        });
        catalog.insert(NodeKindDescriptor {
            kind: "execute_multiple_tools",
            description: "Runs all tool calls of an answer, concurrently or with `sequential` one after another.",
            // `sequential: true` runs the calls one after another instead of concurrently.
            signature: |_| Ok((DataType::ToolCalls, DataType::ToolResults)),
            example_params: || serde_json::json!({ "sequential": true }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "sequential": { "type": "boolean" } },
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_send_multiple_tool_results",
            description: "Sends all tool results back to the model and returns its next answer.",
            signature: |_| Ok((DataType::ToolResults, DataType::Message)),
            example_params: || Value::Null,
            params_schema: no_params,
        });
        catalog.insert(NodeKindDescriptor {
            kind: "compress_history",
            description: "Compresses the conversation history (summarize, keep the last messages or keep facts) and passes the value through.",
            // Passes `value` through and shortens the history (ADR-0037).
            signature: |params| {
                let invalid = |reason: &str| CatalogError::InvalidParams {
                    kind: "compress_history".into(),
                    reason: reason.into(),
                };
                let value = params["value"]
                    .as_str()
                    .ok_or_else(|| invalid("missing \"value\" with the type of the passed-through value"))?;
                match params["strategy"].as_str() {
                    Some("summarize") => {}
                    Some("keep_last" | "facts") if params["keep"].is_u64() => {}
                    Some(strategy @ ("keep_last" | "facts")) => {
                        return Err(invalid(&format!("\"{strategy}\" needs \"keep\"")));
                    }
                    _ => {
                        return Err(invalid(
                            "\"strategy\" must be \"summarize\", \"keep_last\" or \"facts\"",
                        ));
                    }
                }
                let data_type = DataType::from_name(value);
                Ok((data_type.clone(), data_type))
            },
            example_params: || serde_json::json!({ "value": "string", "strategy": "keep_last", "keep": 10 }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" },
                        "strategy": { "type": "string" },
                        "keep": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["value", "strategy"],
                    "additionalProperties": false
                })
            },
        });
        catalog.insert(NodeKindDescriptor {
            kind: "llm_request_structured",
            description: "Asks the model for an answer of the named type `type`, with `retries` on invalid output.",
            signature: |params| {
                let name = params.get("type").and_then(Value::as_str).ok_or_else(|| {
                    CatalogError::InvalidParams {
                        kind: "llm_request_structured".into(),
                        reason: "missing \"type\" with the name of a type from \"types\"".into(),
                    }
                })?;
                if params
                    .get("retries")
                    .is_some_and(|retries| !retries.is_u64())
                {
                    return Err(CatalogError::InvalidParams {
                        kind: "llm_request_structured".into(),
                        reason: "\"retries\" must be a non-negative integer".into(),
                    });
                }
                Ok((DataType::String, DataType::Named(name.into())))
            },
            example_params: || serde_json::json!({ "type": "Example" }),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "type": { "type": "string" },
                        "retries": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["type"],
                    "additionalProperties": false
                })
            },
        });
        catalog
    }

    /// Names of all node kinds.
    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.kinds.keys().copied()
    }

    /// Description of node kind `kind`, if known.
    pub fn get(&self, kind: &str) -> Option<&NodeKindDescriptor> {
        self.kinds.get(kind)
    }

    fn insert(&mut self, descriptor: NodeKindDescriptor) {
        self.kinds.insert(descriptor.kind, descriptor);
    }
}

/// Signature function of a condition: maps parameters and input type to the output type.
type ConditionSignatureFn = fn(&Value, &DataType) -> Result<DataType, CatalogError>;

/// Description of an edge condition (filter map on the edge value, ADR-0021).
#[derive(Debug, Clone)]
pub struct ConditionDescriptor {
    kind: &'static str,
    description: &'static str,
    signature: ConditionSignatureFn,
    params_schema: fn() -> Value,
    example_params: fn() -> Value,
}

impl ConditionDescriptor {
    /// Name of the condition, as it appears in specs under `when.kind`.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// One sentence on when the condition matches and what it passes on.
    pub fn description(&self) -> &'static str {
        self.description
    }

    /// JSON schema of the parameters; spec validation checks `params` against it.
    pub fn params_schema(&self) -> Value {
        (self.params_schema)()
    }

    /// Valid example parameters for conformance tests.
    pub fn example_params(&self) -> Value {
        (self.example_params)()
    }

    /// Output type of the condition for the given parameters and the input type.
    pub fn signature(&self, params: &Value, input: &DataType) -> Result<DataType, CatalogError> {
        (self.signature)(params, input)
    }
}

/// Catalog of all known edge conditions.
#[derive(Debug, Clone, Default)]
pub struct ConditionCatalog {
    kinds: BTreeMap<&'static str, ConditionDescriptor>,
}

impl ConditionCatalog {
    /// Catalog with the built-in conditions.
    pub fn builtin() -> Self {
        let mut catalog = Self::default();
        catalog.insert(ConditionDescriptor {
            kind: "on_assistant_message",
            description: "The answer is a plain message without tool calls; passes its text on.",
            signature: |_, input| {
                expect_input(
                    "on_assistant_message",
                    DataType::Message,
                    input,
                    DataType::String,
                )
            },
            params_schema: no_params,
            example_params: || Value::Null,
        });
        catalog.insert(ConditionDescriptor {
            kind: "on_multiple_tool_calls",
            description: "The answer carries tool calls; passes all of them on.",
            signature: |_, input| {
                expect_input(
                    "on_multiple_tool_calls",
                    DataType::Message,
                    input,
                    DataType::ToolCalls,
                )
            },
            params_schema: no_params,
            example_params: || Value::Null,
        });
        catalog.insert(ConditionDescriptor {
            kind: "on_tool_call",
            description: "The answer carries exactly one tool call; passes it on.",
            signature: |_, input| {
                expect_input("on_tool_call", DataType::Message, input, DataType::ToolCall)
            },
            params_schema: no_params,
            example_params: || Value::Null,
        });
        catalog.insert(ConditionDescriptor {
            kind: "on_field",
            description: "A field of the structured value equals `equals`; passes the value through.",
            // Branches on a field of a structured value and passes it through (ADR-0041).
            signature: |_, input| match input {
                DataType::Named(_) => Ok(input.clone()),
                other => Err(CatalogError::InputTypeMismatch {
                    kind: "on_field".into(),
                    expected: DataType::Named("<structured type>".into()),
                    found: other.clone(),
                }),
            },
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "field": { "type": "string" }, "equals": {} },
                    "required": ["field", "equals"],
                    "additionalProperties": false
                })
            },
            example_params: || serde_json::json!({ "field": "correct", "equals": true }),
        });
        catalog.insert(ConditionDescriptor {
            kind: "on_decision",
            description: "The latest decision's answer is one of `answers`; passes the value through.",
            // Passes the value through if the latest verdict's answer is one of `answers`
            // (ADR-0100).
            signature: |params, input| {
                crate::decision_params::check_on_decision(params).map_err(|reason| {
                    CatalogError::InvalidParams {
                        kind: "on_decision".into(),
                        reason,
                    }
                })?;
                Ok(input.clone())
            },
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "answers": { "type": "array" } },
                    "required": ["answers"],
                    "additionalProperties": false
                })
            },
            example_params: || serde_json::json!({ "answers": ["billing"] }),
        });
        catalog.insert(ConditionDescriptor {
            kind: "on_history_longer",
            description: "The history has more than `messages` messages; passes the value through.",
            // Passes the value through if the history is longer (ADR-0037).
            signature: |_, input| Ok(input.clone()),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "messages": { "type": "integer", "minimum": 0 } },
                    "required": ["messages"],
                    "additionalProperties": false
                })
            },
            example_params: || serde_json::json!({ "messages": 20 }),
        });
        catalog.insert(ConditionDescriptor {
            kind: "on_history_tokens_above",
            description: "The history has more than `tokens` tokens; passes the value through.",
            // Passes the value through if the history has more tokens (ADR-0087).
            signature: |_, input| Ok(input.clone()),
            params_schema: || {
                serde_json::json!({
                    "type": "object",
                    "properties": { "tokens": { "type": "integer", "minimum": 0 } },
                    "required": ["tokens"],
                    "additionalProperties": false
                })
            },
            example_params: || serde_json::json!({ "tokens": 4000 }),
        });
        catalog
    }

    /// Names of all conditions.
    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.kinds.keys().copied()
    }

    /// Description of condition `kind`, if known.
    pub fn get(&self, kind: &str) -> Option<&ConditionDescriptor> {
        self.kinds.get(kind)
    }

    fn insert(&mut self, descriptor: ConditionDescriptor) {
        self.kinds.insert(descriptor.kind, descriptor);
    }
}

/// Returns `output` if `input` matches the expected type.
fn expect_input(
    kind: &str,
    expected: DataType,
    input: &DataType,
    output: DataType,
) -> Result<DataType, CatalogError> {
    if *input == expected {
        Ok(output)
    } else {
        Err(CatalogError::InputTypeMismatch {
            kind: kind.to_owned(),
            expected,
            found: input.clone(),
        })
    }
}

/// All catalogs against which specs are validated and run.
#[derive(Debug, Clone, Default)]
pub struct Catalogs {
    /// Node kinds.
    pub nodes: NodeCatalog,
    /// Edge conditions.
    pub conditions: ConditionCatalog,
}

impl Catalogs {
    /// The built-in catalogs.
    pub fn builtin() -> Self {
        Self {
            nodes: NodeCatalog::builtin(),
            conditions: ConditionCatalog::builtin(),
        }
    }
}
