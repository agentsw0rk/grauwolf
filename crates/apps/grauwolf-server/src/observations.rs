//! What a running agent has observed about itself: the models that answered and the MCP tools
//! it saw, in the format of the signature endpoint (ADR-0094).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use grauwolf_prompt::{LlModel, ToolDescriptor};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use grauwolf_agents::events::{AgentEvent, Feature};

/// How long a looked-up model digest is reused.
const DIGEST_TTL: Duration = Duration::from_secs(600);

/// Source of the current time; replaceable for tests.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// Looks up the digest of a model on its server, e.g. Ollama's `/api/tags`; returns it with or
/// without the `sha256:` prefix, `None` if unknown or unreachable.
pub trait ModelDigests: Send + Sync {
    /// Digest of `model` (as named by the provider).
    fn digest<'a>(&'a self, model: &'a str) -> BoxFuture<'a, Option<String>>;
}

/// When a value was first and last seen.
struct Seen<T> {
    value: T,
    first: SystemTime,
    last: SystemTime,
}

/// Adds a sighting of `value` at `at`: a new entry only if the value is new, otherwise only
/// `first`/`last` move.
fn merge<T: PartialEq>(entries: &mut Vec<Seen<T>>, value: T, first: SystemTime, last: SystemTime) {
    match entries.iter_mut().find(|entry| entry.value == value) {
        Some(entry) => {
            entry.first = entry.first.min(first);
            entry.last = entry.last.max(last);
        }
        None => entries.push(Seen { value, first, last }),
    }
}

/// An MCP tool as the signature lists it.
#[derive(PartialEq)]
struct ObservedTool {
    name: String,
    description: String,
    schema_hash: String,
}

/// A model of an Ollama server whose digest is looked up when the report is made.
struct PendingModel {
    agent: String,
    /// Name to look up: the reported model, else the requested one.
    name: String,
    reported: Option<String>,
    first: SystemTime,
    last: SystemTime,
}

#[derive(Default)]
struct State {
    models: BTreeMap<String, Vec<Seen<Option<String>>>>,
    pending: Vec<PendingModel>,
    mcp_tools: BTreeMap<String, Vec<Seen<Vec<ObservedTool>>>>,
}

/// Models and MCP tools observed since the start, in memory (ADR-0094).
pub struct Observations {
    started: SystemTime,
    clock: Clock,
    /// Spec name → agent name.
    names: HashMap<String, String>,
    state: Mutex<State>,
    /// Digests per model name with the time of the lookup.
    digests: Mutex<HashMap<String, (SystemTime, Option<String>)>>,
}

impl Observations {
    /// Observations from now on; `names` maps spec names to agent names (spec
    /// names without an entry are used as they are).
    pub fn new(names: HashMap<String, String>) -> Arc<Self> {
        Self::with_clock(names, Arc::new(SystemTime::now))
    }

    /// Like [`Observations::new`] with `clock` as the source of time.
    pub fn with_clock(names: HashMap<String, String>, clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            started: clock(),
            clock,
            names,
            state: Mutex::default(),
            digests: Mutex::default(),
        })
    }

    /// Feature that records the models that answer the agent `agent` (its spec name).
    pub fn feature_for(self: &Arc<Self>, agent: &str) -> Arc<dyn Feature> {
        Arc::new(AgentObserver {
            observations: Arc::clone(self),
            agent: self
                .names
                .get(agent)
                .cloned()
                .unwrap_or_else(|| agent.to_owned()),
            calls: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn record_model(&self, agent: &str, requested: Option<&LlModel>, reported: Option<String>) {
        let now = (self.clock)();
        match requested.filter(|model| model.provider == "ollama") {
            // The digest is looked up outside the run, when the report is made.
            Some(model) => {
                let name = reported.clone().unwrap_or_else(|| model.id.clone());
                let mut state = self.state();
                match state.pending.iter_mut().find(|pending| {
                    (&pending.agent, &pending.name, &pending.reported)
                        == (&agent.to_owned(), &name, &reported)
                }) {
                    Some(pending) => pending.last = now,
                    None => state.pending.push(PendingModel {
                        agent: agent.to_owned(),
                        name,
                        reported,
                        first: now,
                        last: now,
                    }),
                }
            }
            None => {
                let mut state = self.state();
                merge(
                    state.models.entry(agent.to_owned()).or_default(),
                    reported,
                    now,
                    now,
                );
            }
        }
    }

    /// The observer that records every MCP tool list it is handed here; give it to the loader
    /// (`AgentLoader::with_mcp_tools_observer`) or to `ProvisionOptions`.
    pub fn mcp_tools_observer(self: &Arc<Self>) -> grauwolf_host::mcp_observer::McpToolsObserver {
        let recorder = Arc::clone(self);
        Arc::new(move |server, tools| recorder.record_mcp_tools(server, tools))
    }

    /// Records that the agent listed the tools of the MCP server `server`.
    pub fn record_mcp_tools(&self, server: &str, tools: &[ToolDescriptor]) {
        let now = (self.clock)();
        let mut observed: Vec<ObservedTool> = tools
            .iter()
            .map(|tool| ObservedTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                schema_hash: schema_hash(tool),
            })
            .collect();
        observed.sort_by(|a, b| a.name.cmp(&b.name));
        let mut state = self.state();
        merge(
            state.mcp_tools.entry(server.to_owned()).or_default(),
            observed,
            now,
            now,
        );
    }

    /// The `observations` object of the signature endpoint. Models of an Ollama server get
    /// their digest from `digests` (cached for ten minutes).
    pub async fn report(&self, digests: Option<&dyn ModelDigests>) -> Value {
        let pending = std::mem::take(&mut self.state().pending);
        let mut resolved = Vec::new();
        for model in pending {
            let digest = match digests {
                Some(digests) => self.digest(digests, &model.name).await,
                None => None,
            };
            let id = match digest {
                Some(digest) => Some(format!("{}@{digest}", model.name)),
                None => model.reported.clone(),
            };
            resolved.push((model.agent, id, model.first, model.last));
        }
        let mut state = self.state();
        for (agent, id, first, last) in resolved {
            merge(state.models.entry(agent).or_default(), id, first, last);
        }
        let models: serde_json::Map<String, Value> = state
            .models
            .iter()
            .map(|(agent, entries)| {
                let entries = entries
                    .iter()
                    .map(|entry| {
                        json!({
                            "modelId": entry.value,
                            "firstSeen": rfc3339(entry.first),
                            "lastSeen": rfc3339(entry.last),
                        })
                    })
                    .collect();
                (agent.clone(), Value::Array(entries))
            })
            .collect();
        let mcp_tools: serde_json::Map<String, Value> = state
            .mcp_tools
            .iter()
            .map(|(server, entries)| {
                let entries = entries
                    .iter()
                    .map(|entry| {
                        let tools: Vec<Value> = entry
                            .value
                            .iter()
                            .map(|tool| {
                                json!({
                                    "name": tool.name,
                                    "description": tool.description,
                                    "schemaHash": tool.schema_hash,
                                })
                            })
                            .collect();
                        json!({
                            "tools": tools,
                            "firstSeen": rfc3339(entry.first),
                            "lastSeen": rfc3339(entry.last),
                        })
                    })
                    .collect();
                (server.clone(), Value::Array(entries))
            })
            .collect();
        json!({
            "startedAt": rfc3339(self.started),
            "models": models,
            "mcpTools": mcp_tools,
        })
    }

    /// First twelve hex digits of the digest of `model`, from the cache or a new lookup.
    async fn digest(&self, digests: &dyn ModelDigests, model: &str) -> Option<String> {
        let now = (self.clock)();
        let cached = self
            .digests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(model)
            .filter(|(at, _)| now.duration_since(*at).is_ok_and(|age| age < DIGEST_TTL))
            .cloned();
        match cached {
            Some((_, digest)) => digest,
            None => {
                let digest = digests.digest(model).await.map(|digest| {
                    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
                    hex.chars().take(12).collect::<String>()
                });
                self.digests
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(model.to_owned(), (now, digest.clone()));
                digest
            }
        }
    }
}

/// Watches the events of one agent.
struct AgentObserver {
    observations: Arc<Observations>,
    /// Name of the agent in the signature.
    agent: String,
    /// Models of the requests that are running.
    calls: Mutex<Vec<LlModel>>,
}

impl Feature for AgentObserver {
    fn on_event(&self, event: &AgentEvent) {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match event {
            AgentEvent::LlmCallStarting { model, .. } => calls.push(model.clone()),
            AgentEvent::LlmFallback { to, .. } => {
                if let Some(last) = calls.last_mut() {
                    *last = to.clone();
                }
            }
            AgentEvent::LlmCallFailed { .. } => {
                calls.pop();
            }
            AgentEvent::LlmCallCompleted { model, .. } => {
                let requested = calls.pop();
                drop(calls);
                self.observations
                    .record_model(&self.agent, requested.as_ref(), model.clone());
            }
            _ => {}
        }
    }
}

/// `2026-11-11T10:00:00.000Z` (UTC, milliseconds).
fn rfc3339(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (days, seconds) = (since.as_secs() / 86_400, since.as_secs() % 86_400);
    // Days since 1970-01-01 as a civil date (proleptic Gregorian, Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60,
        since.subsec_millis()
    )
}

/// Hash of a tool's parameters: SHA-256 (lowercase hex) over `required(<p>,…);optional(<p>,…)`
/// with each parameter rendered as `name:type:description` in the order of the schema's
/// properties. Types render as `string`, `integer`, `number`, `boolean`, `enum[a,b]`,
/// `list[<type>]`, `anyOf[<type>,…]` and `object[<p>,…;required=a,b;additional=<bool>:<type>]`
/// (the rule of AgentWeaver, the main consumer, with JSON-schema type names). It only has to be stable for one agent.
pub fn schema_hash(tool: &ToolDescriptor) -> String {
    let required: Vec<&str> = tool.parameters["required"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let (mut mandatory, mut optional) = (Vec::new(), Vec::new());
    if let Some(properties) = tool.parameters["properties"].as_object() {
        for (name, schema) in properties {
            let rendered = parameter(name, schema);
            if required.contains(&name.as_str()) {
                mandatory.push(rendered);
            } else {
                optional.push(rendered);
            }
        }
    }
    let text = format!(
        "required({});optional({})",
        mandatory.join(","),
        optional.join(",")
    );
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parameter(name: &str, schema: &Value) -> String {
    let description = schema["description"].as_str().unwrap_or_default();
    format!("{name}:{}:{description}", type_of(schema))
}

fn type_of(schema: &Value) -> String {
    if let Some(entries) = schema["enum"].as_array() {
        let entries: Vec<String> = entries
            .iter()
            .map(|entry| match entry {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect();
        return format!("enum[{}]", entries.join(","));
    }
    if let Some(alternatives) = schema["anyOf"].as_array() {
        let types: Vec<String> = alternatives.iter().map(type_of).collect();
        return format!("anyOf[{}]", types.join(","));
    }
    match &schema["type"] {
        Value::String(name) => match name.as_str() {
            "array" => format!("list[{}]", type_of(&schema["items"])),
            "object" => object_type(schema),
            other => other.to_owned(),
        },
        Value::Array(names) => {
            let types: Vec<String> = names
                .iter()
                .filter_map(Value::as_str)
                .map(|name| type_of(&serde_json::json!({ "type": name })))
                .collect();
            format!("anyOf[{}]", types.join(","))
        }
        _ => "any".to_owned(),
    }
}

fn object_type(schema: &Value) -> String {
    let properties: Vec<String> = schema["properties"]
        .as_object()
        .map(|properties| {
            properties
                .iter()
                .map(|(name, schema)| parameter(name, schema))
                .collect()
        })
        .unwrap_or_default();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let (additional, additional_type) = match &schema["additionalProperties"] {
        Value::Bool(flag) => (*flag, String::new()),
        Value::Object(_) => (true, type_of(&schema["additionalProperties"])),
        _ => (false, String::new()),
    };
    format!(
        "object[{};required={};additional={additional}:{additional_type}]",
        properties.join(","),
        required.join(",")
    )
}
