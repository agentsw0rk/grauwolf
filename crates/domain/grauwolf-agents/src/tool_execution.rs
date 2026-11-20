//! Tool execution: one tool call of the model from the interceptors' verdict to the result
//! (F03.8, ADR-0027, ADR-0047, ADR-0078, ADR-0098).
//!
//! [`ToolExecution`] is a view on the components a call needs – tool access, event bus,
//! accounting and agent storage – like `LlmGateway` for LLM requests, so the order of the steps
//! is testable without a `PromptExecutor`.

use grauwolf_prompt::{ToolCall, ToolResult};
use grauwolf_tools::ToolError;
use serde_json::Value;

use crate::accounting::Accounting;
use crate::event_bus::EventBus;
use crate::events::AgentEvent;
use crate::storage::Storage;
use crate::tool_access::ToolAccess;

/// View for tool calls on the tools, bus, accounting and storage of a context.
pub(crate) struct ToolExecution<'a> {
    pub(crate) tools: &'a ToolAccess,
    pub(crate) bus: &'a EventBus,
    pub(crate) accounting: &'a Accounting,
    pub(crate) storage: &'a Storage,
}

impl ToolExecution<'_> {
    /// Runs `call`. Errors, including a tool not available in the active section,
    /// are reported to the LLM as result text (F03.8, ADR-0027).
    pub(crate) async fn execute(&self, call: &ToolCall) -> ToolResult {
        let mut call = call.clone();
        // If the limit is already reached, no approval is asked for either.
        let denied = self
            .bus
            .judge_tool_call(&mut call, self.accounting.tool_limit_reached())
            .await;
        self.bus
            .emit(AgentEvent::ToolCallStarting { call: call.clone() });
        let mut loaded_skill = None;
        let content = if let Some(text) = denied {
            text
        } else if !self.tools.is_allowed(&call.tool) {
            format!(
                "Error: tool '{}' is not available in this section",
                call.tool
            )
        } else {
            match self.accounting.claim_tool_call() {
                Err(text) => text,
                Ok(()) => match grauwolf_tools::with_trace_headers(
                    self.bus.trace_headers(Some(&call.id)),
                    self.call(&call),
                )
                .await
                {
                    Ok(content) => {
                        // A successful `load_skill` is reported as an event of its own (ADR-0103).
                        if call.tool == grauwolf_tools::LOAD_SKILL_TOOL {
                            loaded_skill = call.args["name"].as_str().map(str::to_owned);
                        }
                        content
                    }
                    Err(error) => format!("Error: {error}"),
                },
            }
        };
        let mut result = ToolResult {
            id: call.id.clone(),
            tool: call.tool.clone(),
            content,
        };
        self.bus.after_tool_call(&mut result);
        self.bus.emit(AgentEvent::ToolCallCompleted {
            result: result.clone(),
        });
        if let Some(name) = loaded_skill {
            self.bus.emit(AgentEvent::SkillLoaded { name });
        }
        result
    }

    /// Calls the tool `call`; a tool with a state key works on its value in the agent storage
    /// (ADR-0098).
    async fn call(&self, call: &ToolCall) -> Result<String, ToolError> {
        let stateful = self
            .tools
            .registry()
            .find(&call.tool)
            .and_then(|tool| tool.state_key().map(str::to_owned).map(|key| (tool, key)));
        let Some((tool, key)) = stateful else {
            return self
                .tools
                .registry()
                .call(&call.tool, call.args.clone())
                .await;
        };
        // Under the lock for the whole call (it does not wait), so parallel calls in one
        // response see each other's changes.
        let mut storage = self.storage.lock();
        let mut state = storage.get(&key).cloned().unwrap_or(Value::Null);
        let result = tool.call_with_state(call.args.clone(), &mut state);
        if result.is_ok() {
            storage.insert(key, state);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use futures_executor::block_on;
    use futures_util::future::BoxFuture;
    use grauwolf_prompt::ToolDescriptor;
    use grauwolf_tools::{DynTool, ToolRegistry};
    use serde_json::json;

    use super::*;
    use crate::accounting::Budget;
    use crate::events::Feature;

    /// Echoes `text`; fails for `"fail"`.
    struct Echo;

    impl DynTool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "echo".into(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            }
        }
        fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
            Box::pin(async move {
                match args["text"].as_str() {
                    Some("fail") => Err(ToolError::Execution("broken".into())),
                    text => Ok(text.unwrap_or_default().to_owned()),
                }
            })
        }
    }

    /// Stands in for `load_skill`: loads `known` only.
    struct LoadSkill;

    impl DynTool for LoadSkill {
        fn name(&self) -> &str {
            "load_skill"
        }
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "load_skill".into(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            }
        }
        fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
            Box::pin(async move {
                match args["name"].as_str() {
                    Some("known") => Ok("instructions".to_owned()),
                    _ => Err(ToolError::Execution("unknown skill".into())),
                }
            })
        }
    }

    /// Counts in its state under `counter`; fails for `{"fail": true}` after changing it.
    struct Counter;

    impl DynTool for Counter {
        fn name(&self) -> &str {
            "count"
        }
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "count".into(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            }
        }
        fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
            Box::pin(async { Err(ToolError::Execution("stateful only".into())) })
        }
        fn state_key(&self) -> Option<&str> {
            Some("counter")
        }
        fn call_with_state(&self, args: Value, state: &mut Value) -> Result<String, ToolError> {
            let next = state.as_u64().unwrap_or(0) + 1;
            *state = json!(next);
            if args["fail"] == json!(true) {
                return Err(ToolError::Execution("failed".into()));
            }
            Ok(next.to_string())
        }
    }

    /// Records the tool events it sees.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl Feature for Recorder {
        fn on_event(&self, event: &AgentEvent) {
            let entry = match event {
                AgentEvent::ToolCallStarting { call } => format!("starting {}", call.tool),
                AgentEvent::ToolCallCompleted { result } => {
                    format!("completed {}: {}", result.tool, result.content)
                }
                AgentEvent::SkillLoaded { name } => format!("skill {name}"),
                _ => return,
            };
            self.0.lock().unwrap().push(entry);
        }
    }

    struct Parts {
        tools: ToolAccess,
        bus: EventBus,
        accounting: Accounting,
        storage: Storage,
        recorder: Arc<Recorder>,
    }

    impl Parts {
        fn new(budget: Budget) -> Self {
            let mut tools = ToolAccess::new();
            tools.set_registry(
                ToolRegistry::builder()
                    .tool(Echo)
                    .tool(Counter)
                    .tool(LoadSkill)
                    .build(),
            );
            let recorder = Arc::new(Recorder::default());
            let mut bus = EventBus::new();
            bus.install(recorder.clone());
            let mut accounting = Accounting::new();
            accounting.set_budget(budget);
            Parts {
                tools,
                bus,
                accounting,
                storage: Storage::new(BTreeMap::new()),
                recorder,
            }
        }

        fn run(&self, tool: &str, args: Value) -> String {
            let execution = ToolExecution {
                tools: &self.tools,
                bus: &self.bus,
                accounting: &self.accounting,
                storage: &self.storage,
            };
            let call = ToolCall {
                id: "1".into(),
                tool: tool.into(),
                args,
            };
            block_on(execution.execute(&call)).content
        }

        fn events(&self) -> Vec<String> {
            self.recorder.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn runs_the_tool_between_the_starting_and_completed_events_and_counts_it() {
        let parts = Parts::new(Budget::default());

        let content = parts.run("echo", json!({ "text": "hi" }));

        assert_eq!(content, "hi");
        assert_eq!(parts.events(), ["starting echo", "completed echo: hi"]);
        assert_eq!(parts.accounting.tool_calls(), 1);
    }

    #[test]
    fn reports_tool_errors_and_tools_outside_the_section_as_result_text() {
        let mut parts = Parts::new(Budget::default());
        let failed = parts.run("echo", json!({ "text": "fail" }));
        parts.tools.enter_tool_scope(vec!["count".into()]);

        let outside = parts.run("echo", json!({ "text": "hi" }));

        assert_eq!(failed, "Error: tool failed: broken");
        assert_eq!(
            outside,
            "Error: tool 'echo' is not available in this section"
        );
        assert_eq!(
            parts.accounting.tool_calls(),
            1,
            "only the call that ran counts"
        );
    }

    #[test]
    fn answers_with_the_limit_text_once_the_tool_call_limit_is_reached() {
        let parts = Parts::new(Budget {
            max_tool_calls: Some(1),
            ..Budget::default()
        });

        let first = parts.run("echo", json!({ "text": "one" }));
        let second = parts.run("echo", json!({ "text": "two" }));

        assert_eq!(first, "one");
        assert!(second.contains("limit of 1 tool calls"), "{second}");
        assert_eq!(parts.accounting.tool_calls(), 1);
    }

    /// Answers with the trace headers that the call sees.
    struct SeesHeaders;

    impl DynTool for SeesHeaders {
        fn name(&self) -> &str {
            "headers"
        }
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "headers".into(),
                description: String::new(),
                parameters: json!({ "type": "object" }),
            }
        }
        fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
            Box::pin(async { Ok(format!("{:?}", grauwolf_tools::current_trace_headers())) })
        }
    }

    /// Traces: gives the headers of call "1" only.
    struct Tracing;

    impl Feature for Tracing {
        fn on_event(&self, _event: &AgentEvent) {}
        fn trace_headers(&self, call: Option<&str>) -> Vec<(String, String)> {
            match call {
                Some("1") => vec![("traceparent".to_owned(), "00-call-1".to_owned())],
                _ => Vec::new(),
            }
        }
    }

    #[test]
    fn a_tool_sees_the_trace_headers_of_its_own_call_and_none_without_a_tracing_feature() {
        let mut parts = Parts::new(Budget::default());
        parts
            .tools
            .set_registry(ToolRegistry::builder().tool(SeesHeaders).build());
        let untraced = parts.run("headers", json!({}));
        parts.bus.install(Arc::new(Tracing));

        let traced = parts.run("headers", json!({}));

        assert_eq!(untraced, "[]");
        assert_eq!(traced, r#"[("traceparent", "00-call-1")]"#);
    }

    #[test]
    fn keeps_the_state_of_a_stateful_tool_only_when_its_call_succeeds() {
        let parts = Parts::new(Budget::default());

        let first = parts.run("count", json!({}));
        let failed = parts.run("count", json!({ "fail": true }));
        let second = parts.run("count", json!({}));

        assert_eq!(first, "1");
        assert!(failed.starts_with("Error:"), "{failed}");
        assert_eq!(second, "2");
        assert_eq!(parts.storage.lock().get("counter"), Some(&json!(2)));
    }

    #[test]
    fn a_successful_load_skill_reports_skill_loaded_after_the_tool_event() {
        let parts = Parts::new(Budget::default());

        parts.run("load_skill", json!({ "name": "known" }));
        parts.run("load_skill", json!({ "name": "other" }));

        assert_eq!(
            parts.events(),
            [
                "starting load_skill",
                "completed load_skill: instructions",
                "skill known",
                "starting load_skill",
                "completed load_skill: Error: tool failed: unknown skill",
            ]
        );
    }
}
