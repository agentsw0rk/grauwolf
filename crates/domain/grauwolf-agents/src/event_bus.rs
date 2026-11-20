//! Installed features and interceptors: event delivery and the interception of requests,
//! answers and tool calls (F06.1, ADR-0047, ADR-0061).

use std::borrow::Cow;
use std::sync::Arc;

use grauwolf_llm::LlmError;
use grauwolf_prompt::{Message, Prompt, ToolCall, ToolDescriptor, ToolResult};

use crate::events::{AgentEvent, Feature, Interceptor, ToolDecision};

/// ID of the prompt that interceptors see before a moderation (ADR-0047 addendum).
pub const MODERATION_PROMPT_ID: &str = "moderation";

/// The installed features, which receive all events, and the interceptors, which intervene in
/// LLM and tool calls in the order of installation.
#[derive(Clone, Default)]
pub(crate) struct EventBus {
    /// Installed features; they receive all events (F06.1).
    features: Vec<Arc<dyn Feature>>,
    /// Installed interceptors, in order (ADR-0047).
    interceptors: Vec<Arc<dyn Interceptor>>,
}

impl EventBus {
    /// Bus without features and interceptors.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Installs `feature`; from now on it receives all events.
    pub(crate) fn install(&mut self, feature: Arc<dyn Feature>) {
        self.features.push(feature);
    }

    /// Installs `interceptor`; from now on it intervenes in LLM and tool calls.
    pub(crate) fn intercept(&mut self, interceptor: Arc<dyn Interceptor>) {
        self.interceptors.push(interceptor);
    }

    /// Reports `event` to all installed features.
    pub(crate) fn emit(&self, event: AgentEvent) {
        for feature in &self.features {
            feature.on_event(&event);
        }
    }

    /// The trace headers for a call out of the run (ADR-0123): those of the first installed
    /// feature that traces, empty without one.
    pub(crate) fn trace_headers(&self, call: Option<&str>) -> Vec<(String, String)> {
        self.features
            .iter()
            .map(|feature| feature.trace_headers(call))
            .find(|headers| !headers.is_empty())
            .unwrap_or_default()
    }

    /// Lets the interceptors change `prompt` and `tools` of a request that is about to be sent.
    pub(crate) fn before_llm_call(&self, prompt: &mut Prompt, tools: &mut Vec<ToolDescriptor>) {
        for interceptor in &self.interceptors {
            interceptor.before_llm_call(prompt, tools);
        }
    }

    /// Prompt and tools of a request after the interceptors; without interceptors, no copy.
    pub(crate) fn prepare<'a>(
        &self,
        prompt: &'a Prompt,
        tools: Vec<ToolDescriptor>,
    ) -> (Cow<'a, Prompt>, Vec<ToolDescriptor>) {
        if self.interceptors.is_empty() {
            return (Cow::Borrowed(prompt), tools);
        }
        let (mut prompt, mut tools) = (prompt.clone(), tools);
        self.before_llm_call(&mut prompt, &mut tools);
        (Cow::Owned(prompt), tools)
    }

    /// Answer after the interceptors.
    pub(crate) fn finish(&self, result: Result<Message, LlmError>) -> Result<Message, LlmError> {
        result.map(|mut answer| {
            for interceptor in &self.interceptors {
                interceptor.after_llm_call(&mut answer);
            }
            answer
        })
    }

    /// Text to check after the interceptors: as the prompt [`MODERATION_PROMPT_ID`] with a
    /// user message through `before_llm_call`, then the text of all user messages
    /// (ADR-0047 addendum).
    pub(crate) fn intercepted_moderation_text(&self, text: &str) -> String {
        if self.interceptors.is_empty() {
            return text.to_owned();
        }
        let mut prompt = Prompt::builder(MODERATION_PROMPT_ID).user(text).build();
        let mut no_tools = Vec::new();
        self.before_llm_call(&mut prompt, &mut no_tools);
        prompt
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Asks the interceptors about `call`: first all `before_tool_call` hooks (which may change
    /// the call), then all `approve_tool_call` hooks. Returns the text of the first refusal.
    /// If `denied` is already set, nobody is asked and it is returned unchanged.
    pub(crate) async fn judge_tool_call(
        &self,
        call: &mut ToolCall,
        mut denied: Option<String>,
    ) -> Option<String> {
        for interceptor in self.interceptors.iter().filter(|_| denied.is_none()) {
            if let ToolDecision::Deny(text) = interceptor.before_tool_call(call) {
                denied = Some(text);
                break;
            }
        }
        if denied.is_none() {
            for interceptor in &self.interceptors {
                if let ToolDecision::Deny(text) = interceptor.approve_tool_call(call).await {
                    denied = Some(text);
                    break;
                }
            }
        }
        denied
    }

    /// Lets the interceptors change the `result` of a tool call.
    pub(crate) fn after_tool_call(&self, result: &mut ToolResult) {
        for interceptor in &self.interceptors {
            interceptor.after_tool_call(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_executor::block_on;
    use futures_util::future::BoxFuture;
    use serde_json::json;

    use super::*;

    /// Writes its name into a shared log for each hook and appends it to what it changes.
    struct Tagger {
        name: &'static str,
        log: Arc<Mutex<Vec<String>>>,
        deny_before: bool,
        deny_approval: bool,
    }

    impl Tagger {
        fn new(name: &'static str, log: &Arc<Mutex<Vec<String>>>) -> Arc<Self> {
            Arc::new(Self {
                name,
                log: Arc::clone(log),
                deny_before: false,
                deny_approval: false,
            })
        }

        fn note(&self, hook: &str) {
            self.log
                .lock()
                .unwrap()
                .push(format!("{}:{hook}", self.name));
        }
    }

    impl Interceptor for Tagger {
        fn before_llm_call(&self, prompt: &mut Prompt, tools: &mut Vec<ToolDescriptor>) {
            self.note("before_llm");
            prompt.messages.push(Message::user(self.name));
            tools.retain(|tool| tool.name != self.name);
        }

        fn after_llm_call(&self, answer: &mut Message) {
            self.note("after_llm");
            if let Message::Assistant(answer) = answer {
                answer.parts.push(grauwolf_prompt::AssistantPart::Text {
                    text: self.name.into(),
                });
            }
        }

        fn before_tool_call(&self, call: &mut ToolCall) -> ToolDecision {
            self.note("before_tool");
            call.tool.push_str(self.name);
            if self.deny_before {
                ToolDecision::Deny(format!("{} refuses", self.name))
            } else {
                ToolDecision::Proceed
            }
        }

        fn approve_tool_call<'a>(&'a self, _call: &'a ToolCall) -> BoxFuture<'a, ToolDecision> {
            self.note("approve");
            let decision = if self.deny_approval {
                ToolDecision::Deny(format!("{} does not approve", self.name))
            } else {
                ToolDecision::Proceed
            };
            Box::pin(std::future::ready(decision))
        }

        fn after_tool_call(&self, result: &mut ToolResult) {
            self.note("after_tool");
            result.content.push_str(self.name);
        }
    }

    fn log() -> Arc<Mutex<Vec<String>>> {
        Arc::default()
    }

    fn taken(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        std::mem::take(&mut log.lock().unwrap())
    }

    fn call() -> ToolCall {
        ToolCall {
            id: "1".into(),
            tool: "t".into(),
            args: json!({}),
        }
    }

    #[test]
    fn emit_reaches_all_features_in_installation_order() {
        struct Named(&'static str, Arc<Mutex<Vec<String>>>);
        impl Feature for Named {
            fn on_event(&self, event: &AgentEvent) {
                self.1.lock().unwrap().push(format!(
                    "{}:{}",
                    self.0,
                    matches!(event, AgentEvent::LlmCallFailed { .. })
                ));
            }
        }
        let log = log();
        let mut bus = EventBus::new();
        bus.install(Arc::new(Named("a", log.clone())));
        bus.install(Arc::new(Named("b", log.clone())));
        bus.emit(AgentEvent::LlmCallFailed { error: "x".into() });
        assert_eq!(taken(&log), ["a:true", "b:true"]);
    }

    #[test]
    fn prepare_without_interceptors_borrows_the_prompt() {
        let prompt = Prompt::builder("p").user("hi").build();
        let (prepared, tools) = EventBus::new().prepare(&prompt, Vec::new());
        assert!(matches!(prepared, Cow::Borrowed(_)));
        assert!(tools.is_empty());
    }

    #[test]
    fn prepare_applies_the_interceptors_in_order_to_a_copy() {
        let log = log();
        let mut bus = EventBus::new();
        bus.intercept(Tagger::new("first", &log));
        bus.intercept(Tagger::new("second", &log));
        let prompt = Prompt::builder("p").user("hi").build();
        let (prepared, _) = bus.prepare(&prompt, Vec::new());
        assert_eq!(taken(&log), ["first:before_llm", "second:before_llm"]);
        assert_eq!(prepared.messages.len(), 3);
        assert_eq!(prompt.messages.len(), 1);
    }

    #[test]
    fn finish_changes_an_answer_but_passes_an_error_on() {
        let log = log();
        let mut bus = EventBus::new();
        bus.intercept(Tagger::new("a", &log));
        bus.intercept(Tagger::new("b", &log));
        let answer = bus.finish(Ok(Message::assistant("x"))).unwrap();
        let Message::Assistant(answer) = answer else {
            panic!("assistant message expected");
        };
        assert_eq!(answer.text(), "xab");
        assert!(bus.finish(Err(LlmError::Client("down".into()))).is_err());
        assert_eq!(taken(&log), ["a:after_llm", "b:after_llm"]);
    }

    #[test]
    fn moderation_text_is_the_text_of_the_user_messages_after_the_interceptors() {
        let log = log();
        let mut bus = EventBus::new();
        assert_eq!(bus.intercepted_moderation_text("plain"), "plain");
        bus.intercept(Tagger::new("a", &log));
        assert_eq!(bus.intercepted_moderation_text("plain"), "plain\na");
    }

    #[test]
    fn tool_call_goes_through_before_hooks_then_approvals() {
        let log = log();
        let mut bus = EventBus::new();
        bus.intercept(Tagger::new("a", &log));
        bus.intercept(Tagger::new("b", &log));
        let mut call = call();
        let denied = block_on(bus.judge_tool_call(&mut call, None));
        assert_eq!(denied, None);
        assert_eq!(call.tool, "tab");
        assert_eq!(
            taken(&log),
            ["a:before_tool", "b:before_tool", "a:approve", "b:approve"]
        );
    }

    #[test]
    fn a_refusal_before_the_call_skips_the_remaining_hooks_and_approvals() {
        let log = log();
        let mut bus = EventBus::new();
        bus.intercept(Arc::new(Tagger {
            deny_before: true,
            ..Arc::try_unwrap(Tagger::new("a", &log)).ok().unwrap()
        }));
        bus.intercept(Tagger::new("b", &log));
        let denied = block_on(bus.judge_tool_call(&mut call(), None));
        assert_eq!(denied.as_deref(), Some("a refuses"));
        assert_eq!(taken(&log), ["a:before_tool"]);
    }

    #[test]
    fn a_refused_approval_names_the_first_refusing_interceptor() {
        let log = log();
        let mut bus = EventBus::new();
        bus.intercept(Arc::new(Tagger {
            deny_approval: true,
            ..Arc::try_unwrap(Tagger::new("a", &log)).ok().unwrap()
        }));
        bus.intercept(Tagger::new("b", &log));
        let denied = block_on(bus.judge_tool_call(&mut call(), None));
        assert_eq!(denied.as_deref(), Some("a does not approve"));
        assert_eq!(taken(&log), ["a:before_tool", "b:before_tool", "a:approve"]);
    }

    #[test]
    fn an_existing_refusal_asks_nobody() {
        let log = log();
        let mut bus = EventBus::new();
        bus.intercept(Tagger::new("a", &log));
        let denied = block_on(bus.judge_tool_call(&mut call(), Some("limit".into())));
        assert_eq!(denied.as_deref(), Some("limit"));
        assert!(taken(&log).is_empty());
    }

    #[test]
    fn after_tool_call_changes_the_result_in_order() {
        let log = log();
        let mut bus = EventBus::new();
        bus.intercept(Tagger::new("a", &log));
        bus.intercept(Tagger::new("b", &log));
        let mut result = ToolResult {
            id: "1".into(),
            tool: "t".into(),
            content: "ok".into(),
        };
        bus.after_tool_call(&mut result);
        assert_eq!(result.content, "okab");
    }
}
