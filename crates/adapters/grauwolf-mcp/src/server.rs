//! MCP server (ADR-0056): offers arbitrary [`DynTool`]s over JSON-RPC, via stdio with
//! [`McpServer::serve`] or via an HTTP endpoint with [`McpServer::handle`].

use std::sync::Arc;

use grauwolf_tools::DynTool;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::PROTOCOL_VERSION;

/// Protocol versions the server accepts; otherwise it responds with [`PROTOCOL_VERSION`].
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// MCP server for tools, text resources and prompt templates.
#[derive(Clone)]
pub struct McpServer {
    name: String,
    tools: Vec<Arc<dyn DynTool>>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
}

/// A text resource that clients can read (ADR-0089).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResource {
    /// Unique URI, e.g. `grauwolf://lexicon/spec`.
    pub uri: String,
    /// Short name.
    pub name: String,
    /// What the resource is for.
    pub description: String,
    /// MIME-Typ, z. B. `text/markdown`.
    pub mime_type: String,
    /// Inhalt.
    pub text: String,
}

/// A prompt template; `{argument}` in the text is replaced by the value (ADR-0089).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPrompt {
    /// Name under which the client fetches the template.
    pub name: String,
    /// What the template is for.
    pub description: String,
    /// Arguments of the template.
    pub arguments: Vec<McpPromptArgument>,
    /// Text of the user message, with `{name}` placeholders.
    pub template: String,
}

/// An argument of an [`McpPrompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptArgument {
    /// Name, also the `{name}` placeholder.
    pub name: String,
    /// Meaning.
    pub description: String,
    /// Must the client supply it?
    pub required: bool,
}

impl McpServer {
    /// Server `name` that offers `tools`.
    pub fn new(name: impl Into<String>, tools: Vec<Arc<dyn DynTool>>) -> Self {
        Self {
            name: name.into(),
            tools,
            resources: Vec::new(),
            prompts: Vec::new(),
        }
    }

    /// Additionally offers `resources` (`resources/list`, `resources/read`).
    pub fn with_resources(self, resources: Vec<McpResource>) -> Self {
        Self { resources, ..self }
    }

    /// Additionally offers `prompts` (`prompts/list`, `prompts/get`).
    pub fn with_prompts(self, prompts: Vec<McpPrompt>) -> Self {
        Self { prompts, ..self }
    }

    /// Name of the server.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The same server with `tools` in place of the previous ones, e.g. per request (ADR-0089).
    pub fn with_tools(self, tools: Vec<Arc<dyn DynTool>>) -> Self {
        Self { tools, ..self }
    }

    /// Answers a JSON-RPC message; notifications (without `id`) return `None`.
    pub async fn handle(&self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned()?;
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let result = match message["method"].as_str() {
            Some("initialize") => Ok(self.initialize(&params)),
            Some("ping") => Ok(json!({})),
            Some("tools/list") => Ok(json!({
                "tools": self.tools.iter().map(|tool| {
                    let descriptor = tool.descriptor();
                    json!({
                        "name": descriptor.name,
                        "description": descriptor.description,
                        "inputSchema": descriptor.parameters
                    })
                }).collect::<Vec<_>>()
            })),
            Some("tools/call") => Ok(self.call(&params).await),
            Some("resources/list") => Ok(json!({
                "resources": self.resources.iter().map(|resource| json!({
                    "uri": resource.uri,
                    "name": resource.name,
                    "description": resource.description,
                    "mimeType": resource.mime_type
                })).collect::<Vec<_>>()
            })),
            Some("resources/read") => self.read(&params),
            Some("prompts/list") => Ok(json!({
                "prompts": self.prompts.iter().map(|prompt| json!({
                    "name": prompt.name,
                    "description": prompt.description,
                    "arguments": prompt.arguments.iter().map(|argument| json!({
                        "name": argument.name,
                        "description": argument.description,
                        "required": argument.required
                    })).collect::<Vec<_>>()
                })).collect::<Vec<_>>()
            })),
            Some("prompts/get") => self.prompt(&params),
            _ => Err((-32601, "method not found".to_owned())),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params["protocolVersion"].as_str().unwrap_or_default();
        let version = if SUPPORTED_VERSIONS.contains(&requested) {
            requested
        } else {
            PROTOCOL_VERSION
        };
        let mut capabilities = json!({ "tools": {} });
        if !self.resources.is_empty() {
            capabilities["resources"] = json!({});
        }
        if !self.prompts.is_empty() {
            capabilities["prompts"] = json!({});
        }
        json!({
            "protocolVersion": version,
            "capabilities": capabilities,
            "serverInfo": { "name": self.name, "version": env!("CARGO_PKG_VERSION") }
        })
    }

    /// `resources/read`: content of the resource `uri`; unknown → `-32002`.
    fn read(&self, params: &Value) -> Result<Value, (i64, String)> {
        let uri = params["uri"].as_str().unwrap_or_default();
        let resource = self
            .resources
            .iter()
            .find(|resource| resource.uri == uri)
            .ok_or_else(|| (-32002, format!("resource not found: {uri}")))?;
        Ok(json!({ "contents": [ {
            "uri": resource.uri,
            "mimeType": resource.mime_type,
            "text": resource.text
        } ] }))
    }

    /// `prompts/get`: the template with arguments filled in as a user message; unknown
    /// template or missing required argument → `-32602`.
    fn prompt(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["name"].as_str().unwrap_or_default();
        let prompt = self
            .prompts
            .iter()
            .find(|prompt| prompt.name == name)
            .ok_or_else(|| (-32602, format!("unknown prompt '{name}'")))?;
        let mut text = prompt.template.clone();
        for argument in &prompt.arguments {
            let value = params["arguments"][&argument.name].as_str();
            if value.is_none() && argument.required {
                return Err((-32602, format!("missing argument '{}'", argument.name)));
            }
            text = text.replace(&format!("{{{}}}", argument.name), value.unwrap_or_default());
        }
        Ok(json!({
            "description": prompt.description,
            "messages": [ { "role": "user", "content": { "type": "text", "text": text } } ]
        }))
    }

    /// Tool call; errors and unknown tools come back as `isError`, so the LLM of the
    /// caller sees them.
    async fn call(&self, params: &Value) -> Value {
        let name = params["name"].as_str().unwrap_or_default();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let result = match self.tools.iter().find(|tool| tool.name() == name) {
            Some(tool) => tool
                .call_json(arguments)
                .await
                .map_err(|error| error.to_string()),
            None => Err(format!("unknown tool '{name}'")),
        };
        let (text, is_error) = match result {
            Ok(text) => (text, false),
            Err(error) => (error, true),
        };
        json!({ "content": [ { "type": "text", "text": text } ], "isError": is_error })
    }

    /// Serves a connection line by line (stdio transport) until the input ends.
    pub async fn serve(
        &self,
        reader: impl AsyncRead + Unpin,
        mut writer: impl AsyncWrite + Unpin,
    ) -> std::io::Result<()> {
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Value>(&line) {
                Ok(message) => self.handle(message).await,
                Err(error) => Some(json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": { "code": -32700, "message": format!("invalid JSON: {error}") }
                })),
            };
            if let Some(response) = response {
                writer.write_all(format!("{response}\n").as_bytes()).await?;
                writer.flush().await?;
            }
        }
        Ok(())
    }
}
