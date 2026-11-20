//! A2A 1.0 over JSON-RPC (ADR-0054): agent card, `SendMessage`, `SendStreamingMessage`,
//! `GetTask`, `CancelTask`.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ServedAgent;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use grauwolf_agents::events::TraceParent;
use grauwolf_agents::memory::{ChatHistoryStore, conversation_key};
use grauwolf_interpreter::{InterpretedAgent, RunExtras};
use grauwolf_llm::StreamFrame;
use grauwolf_spec::{AgentSpec, STRING_TYPE};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::auth::{Caller, Delegated};

/// Supported protocol version.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Information for the agent card.
#[derive(Debug, Clone)]
pub struct AgentCardInfo {
    /// Name of the agent (name of the spec).
    pub name: String,
    /// Description for other agents.
    pub description: String,
    /// Public base URL of the service, e.g. `http://127.0.0.1:8080`.
    pub base_url: String,
    /// Input as JSON (named type) instead of text.
    pub json_input: bool,
    /// Output as JSON (named type) instead of text.
    pub json_output: bool,
}

impl AgentCardInfo {
    /// Information from `spec`; the service is reachable at `base_url`.
    pub fn from_spec(spec: &AgentSpec, base_url: &str) -> Self {
        Self {
            name: spec.name.clone(),
            description: spec
                .system_prompt
                .clone()
                .unwrap_or_else(|| format!("grauwolf agent {}", spec.name)),
            base_url: base_url.trim_end_matches('/').to_owned(),
            json_input: spec.input != STRING_TYPE,
            json_output: spec.output != STRING_TYPE,
        }
    }

    fn card(&self) -> Value {
        let mode = |json: bool| {
            if json {
                "application/json"
            } else {
                "text/plain"
            }
        };
        json!({
            "name": self.name,
            "description": self.description,
            "supportedInterfaces": [ {
                "url": format!("{}/a2a", self.base_url),
                "protocolBinding": "JSONRPC",
                "protocolVersion": PROTOCOL_VERSION
            } ],
            "version": env!("CARGO_PKG_VERSION"),
            "capabilities": { "streaming": true },
            "defaultInputModes": [mode(self.json_input)],
            "defaultOutputModes": [mode(self.json_output)],
            "skills": [ {
                "id": self.name,
                "name": self.name,
                "description": self.description,
                "tags": ["grauwolf"]
            } ]
        })
    }
}

#[derive(Clone)]
struct A2a {
    agent: Arc<dyn ServedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    info: Arc<AgentCardInfo>,
    /// Finished tasks with their owner (user of a user token).
    tasks: Arc<Mutex<TaskStore>>,
    /// Tools and memory per run, as with `/run` (ADR-0089).
    handles: crate::ServiceOptions,
}

/// This many finished tasks are kept for `GetTask`; the oldest is dropped first.
pub const MAX_STORED_TASKS: usize = 1000;

/// Finished tasks with owner, limited to `capacity` (ADR-0054 addendum).
struct TaskStore {
    capacity: usize,
    order: VecDeque<String>,
    tasks: HashMap<String, (Option<String>, Value)>,
}

impl TaskStore {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            tasks: HashMap::new(),
        }
    }

    /// Stores `task` of `owner`; above the capacity the oldest task is dropped.
    fn insert(&mut self, id: String, owner: Option<String>, task: Value) {
        if self.tasks.insert(id.clone(), (owner, task)).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.tasks.remove(&oldest);
            }
        }
    }

    /// The task `id`, if it belongs to `owner`; foreign tasks count as unknown.
    fn get(&self, id: &str, owner: Option<&str>) -> Option<Value> {
        let (task_owner, task) = self.tasks.get(id)?;
        (task_owner.as_deref() == owner).then(|| task.clone())
    }
}

/// Router with `/.well-known/agent-card.json` and `/a2a`; conversations (`contextId`) live in
/// `history`.
pub fn router(
    agent: Arc<InterpretedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    info: AgentCardInfo,
) -> Router {
    router_with(agent, history, info, crate::ServiceOptions::default())
}

/// Like [`router`]; every task additionally runs with the handles from `options` for its
/// owner – the user's tools, tools directory, memory (ADR-0089).
pub fn router_with(
    agent: Arc<InterpretedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    info: AgentCardInfo,
    options: crate::ServiceOptions,
) -> Router {
    router_for(agent, history, info, options)
}

/// Like [`router_with`] for any [`ServedAgent`], e.g. a generated one (ADR-0093).
pub fn router_for(
    agent: Arc<dyn ServedAgent>,
    history: Arc<dyn ChatHistoryStore>,
    info: AgentCardInfo,
    options: crate::ServiceOptions,
) -> Router {
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card))
        .route("/a2a", post(rpc))
        .with_state(A2a {
            agent,
            history,
            info: Arc::new(info),
            tasks: Arc::new(Mutex::new(TaskStore::new(MAX_STORED_TASKS))),
            handles: options,
        })
}

async fn agent_card(State(a2a): State<A2a>) -> Json<Value> {
    Json(a2a.info.card())
}

/// JSON-RPC error as the response to the request `id`.
fn error(id: &Value, code: i64, message: &str) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
        .into_response()
}

fn success(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

async fn rpc(
    State(a2a): State<A2a>,
    caller: Option<Extension<Caller>>,
    spiffe: Option<Extension<crate::CallerSpiffe>>,
    delegation: Option<Extension<Delegated>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    // With a user token, the task belongs to that user (ADR-0068).
    let owner = match caller {
        Some(Extension(Caller::User(user))) => Some(user),
        _ => None,
    };
    let spiffe = spiffe.map(|Extension(crate::CallerSpiffe(id))| id.to_string());
    let delegated = delegation.map(|Extension(delegated)| delegated);
    let trace_parent = crate::trace_parent_of(&headers);
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let version = headers
        .get("A2A-Version")
        .and_then(|value| value.to_str().ok());
    if version.is_some_and(|version| version.split('.').next() != Some("1")) {
        return error(&id, -32009, "A2A version not supported (supported: 1.0)");
    }
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    match request["method"].as_str() {
        Some("SendMessage") => {
            match a2a
                .start(&params, owner.clone(), spiffe.clone(), trace_parent.clone())
                .map(|run| run.acting_for(delegated.clone()))
            {
                Ok(run) => {
                    Json(success(&id, json!({ "task": a2a.finish(run).await }))).into_response()
                }
                Err(message) => error(&id, -32602, &message),
            }
        }
        Some("SendStreamingMessage") => {
            match a2a
                .start(&params, owner.clone(), spiffe.clone(), trace_parent.clone())
                .map(|run| run.acting_for(delegated.clone()))
            {
                Ok(run) => a2a.stream(id, run).into_response(),
                Err(message) => error(&id, -32602, &message),
            }
        }
        Some("GetTask") => match a2a.task(&params, owner.as_deref()) {
            Some(task) => Json(success(&id, task)).into_response(),
            None => error(&id, -32001, "task not found"),
        },
        Some("CancelTask") => match a2a.task(&params, owner.as_deref()) {
            Some(_) => error(&id, -32002, "task is finished and cannot be canceled"),
            None => error(&id, -32001, "task not found"),
        },
        // Protocol methods that this service does not offer (ADR-0054).
        Some(
            "ListTasks"
            | "SubscribeToTask"
            | "CreateTaskPushNotificationConfig"
            | "GetTaskPushNotificationConfig"
            | "ListTaskPushNotificationConfigs"
            | "DeleteTaskPushNotificationConfig"
            | "GetExtendedAgentCard",
        ) => error(&id, -32004, "method not supported by this service"),
        _ => error(&id, -32601, "method not found"),
    }
}

/// An accepted task: task ID, context, input and the user's message.
struct Run {
    task_id: String,
    context_id: String,
    /// Conversation key: the context in the owner's area (ADR-0060).
    conversation: String,
    /// User of a user token; only they see the task.
    owner: Option<String>,
    /// SPIFFE ID of the calling workload (ADR-0109).
    caller: Option<String>,
    /// Trace context of the caller (ADR-0123).
    trace_parent: Option<TraceParent>,
    /// The person the task acts for (ADR-0111).
    delegated: Option<Delegated>,
    input: Value,
    user_message: Value,
}

impl Run {
    fn acting_for(mut self, delegated: Option<Delegated>) -> Self {
        self.delegated = delegated;
        self
    }
}

impl A2a {
    /// ID with `prefix` and 128 random bits – unguessable (ADR-0054 addendum).
    fn new_id(&self, prefix: &str) -> String {
        format!("{prefix}-{}", crate::approvals::new_id())
    }

    /// Reads input and context from `params.message`; `owner` is the user of a
    /// user token.
    fn start(
        &self,
        params: &Value,
        owner: Option<String>,
        caller: Option<String>,
        trace_parent: Option<TraceParent>,
    ) -> Result<Run, String> {
        let message = params.get("message").ok_or("params.message is missing")?;
        let parts = message["parts"]
            .as_array()
            .ok_or("message.parts is missing")?;
        let text: Vec<&str> = parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect();
        let input = if self.info.json_input {
            match parts.iter().find_map(|part| part.get("data")) {
                Some(data) => data.clone(),
                None => serde_json::from_str(&text.join("\n"))
                    .map_err(|error| format!("input is not JSON: {error}"))?,
            }
        } else {
            Value::String(text.join("\n"))
        };
        // The context is the conversation key; only plain IDs, so that no context can point at
        // another user's conversation (ADR-0060).
        let context_id = match message["contextId"].as_str() {
            Some(id) => conversation_key(None, id)?,
            None => self.new_id("context"),
        };
        let conversation = conversation_key(owner.as_deref(), &context_id)?;
        Ok(Run {
            task_id: self.new_id("task"),
            context_id,
            conversation,
            owner,
            caller,
            trace_parent,
            delegated: None,
            input,
            user_message: message.clone(),
        })
    }

    /// Handles of the task: its conversation and the tools and memory of its owner.
    async fn extras(&self, run: &Run) -> RunExtras {
        RunExtras {
            conversation: Some((Arc::clone(&self.history), run.conversation.clone())),
            caller: run.caller.clone(),
            trace_parent: run.trace_parent.clone(),
            ..self.handles.extras_for(run.owner.as_deref()).await
        }
    }

    /// Runs the task, stores the finished task and returns it.
    async fn finish(&self, run: Run) -> Value {
        let result = crate::run_agent(
            &*self.agent,
            run.input.clone(),
            self.extras(&run).await,
            run.owner.clone(),
            run.delegated.as_ref(),
        )
        .await;
        self.record(&run, result)
    }

    /// Task in its final state after `result`, stored for `GetTask`.
    fn record(&self, run: &Run, result: Result<Value, String>) -> Value {
        let (state, parts) = match &result {
            Ok(output) => ("TASK_STATE_COMPLETED", output_parts(output)),
            Err(error) => ("TASK_STATE_FAILED", json!([ { "text": error } ])),
        };
        let reply = json!({
            "messageId": self.new_id("message"),
            "contextId": run.context_id,
            "taskId": run.task_id,
            "role": "ROLE_AGENT",
            "parts": parts
        });
        let mut task = json!({
            "id": run.task_id,
            "contextId": run.context_id,
            "status": { "state": state, "timestamp": timestamp() },
            "history": [run.user_message, reply]
        });
        if result.is_ok() {
            task["artifacts"] = json!([ { "artifactId": "answer", "parts": parts } ]);
        } else {
            task["status"]["message"] = reply;
        }
        self.tasks.lock().expect("A2A-Tasks").insert(
            run.task_id.clone(),
            run.owner.clone(),
            task.clone(),
        );
        task
    }

    /// The task `params.id`, if it belongs to `owner`; foreign tasks count as unknown.
    fn task(&self, params: &Value, owner: Option<&str>) -> Option<Value> {
        let id = params["id"].as_str()?;
        self.tasks.lock().expect("A2A-Tasks").get(id, owner)
    }

    /// SSE: task in state `WORKING`, text deltas as artifact updates, status at the end.
    fn stream(
        self,
        id: Value,
        run: Run,
    ) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
        let (sender, receiver) = mpsc::unbounded_channel::<Value>();
        let working = json!({ "task": {
            "id": run.task_id,
            "contextId": run.context_id,
            "status": { "state": "TASK_STATE_WORKING", "timestamp": timestamp() },
            "history": [run.user_message]
        } });
        let _ = sender.send(working);
        let deltas = sender.clone();
        let streamed = Arc::new(AtomicBool::new(false));
        let streamed_deltas = Arc::clone(&streamed);
        let (task_id, context_id) = (run.task_id.clone(), run.context_id.clone());
        let frame_sink: grauwolf_agents::FrameSink = Arc::new(move |frame: &StreamFrame| {
            if let StreamFrame::TextDelta { text } = frame {
                streamed_deltas.store(true, Ordering::Relaxed);
                let _ = deltas.send(json!({ "artifactUpdate": {
                    "taskId": task_id,
                    "contextId": context_id,
                    "artifact": { "artifactId": "answer", "parts": [ { "text": text } ] },
                    "append": true
                } }));
            }
        });
        let a2a = self;
        tokio::spawn(async move {
            let mut extras = a2a.extras(&run).await;
            extras.frame_sink = Some(frame_sink);
            let result = crate::run_agent(
                &*a2a.agent,
                run.input.clone(),
                extras,
                run.owner.clone(),
                run.delegated.as_ref(),
            )
            .await;
            let task = a2a.record(&run, result);
            // Without a streaming node there were no deltas: the whole answer as one artifact.
            if let Some(artifact) = task["artifacts"].get(0)
                && !streamed.load(Ordering::Relaxed)
            {
                let _ = sender.send(json!({ "artifactUpdate": {
                    "taskId": task["id"],
                    "contextId": task["contextId"],
                    "artifact": artifact,
                    "lastChunk": true
                } }));
            }
            let _ = sender.send(json!({ "statusUpdate": {
                "taskId": task["id"],
                "contextId": task["contextId"],
                "status": task["status"]
            } }));
        });
        Sse::new(futures_util::stream::unfold(
            receiver,
            move |mut receiver| {
                let id = id.clone();
                async move {
                    let result = receiver.recv().await?;
                    let event = Event::default().data(success(&id, result).to_string());
                    Some((Ok(event), receiver))
                }
            },
        ))
    }
}

/// Output as parts: text as `text`, structured values as `data`.
fn output_parts(output: &Value) -> Value {
    match output {
        Value::String(text) => json!([ { "text": text } ]),
        other => json!([ { "data": other, "mediaType": "application/json" } ]),
    }
}

/// Now as an RFC 3339 timestamp in UTC (seconds).
fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Days since 1970 → date (Howard Hinnant, "civil_from_days").
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
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
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgets_the_oldest_task_beyond_its_capacity() {
        let mut store = TaskStore::new(2);

        store.insert("a".into(), None, json!(1));
        store.insert("b".into(), None, json!(2));
        store.insert("c".into(), Some("anna".into()), json!(3));

        assert_eq!(store.get("a", None), None);
        assert_eq!(store.get("b", None), Some(json!(2)));
        assert_eq!(store.get("c", Some("anna")), Some(json!(3)));
        assert_eq!(store.get("c", None), None);
    }
}
