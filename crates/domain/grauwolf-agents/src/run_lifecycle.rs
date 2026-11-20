//! The run lifecycle (ADR-0116): the one sequence of an agent run – `agent_starting`, the graph
//! run, `agent_completed` or `agent_failed` – for the interpreter and generated code. The graph
//! run itself is passed in as a function, so the untyped interpreter graph and the typed
//! generated graph share the order of the steps and the events.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_graph::{EngineError, NodeError};
use grauwolf_prompt::Attachment;
use grauwolf_tools::KnowledgeContext;
use serde::Serialize;
use serde_json::Value;

use crate::AgentContext;
use crate::checkpoints::CheckpointStore;
use crate::events::{AgentEvent, OnBehalfOf, TraceParent};
use crate::knowledge::inject_knowledge;
use crate::memory::{ChatHistoryStore, FactMemory, recall_facts, remember_facts, text_of};
use crate::observer::AgentObserver;

/// Why a run failed; the run reported it as `agent_failed`.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The graph run failed.
    #[error(transparent)]
    Graph(#[from] EngineError),
    /// The output of the graph cannot be converted to JSON.
    #[error("output cannot be converted: {0}")]
    Output(#[source] serde_json::Error),
    /// The chat store failed.
    #[error("chat store failed: {0}")]
    ChatHistory(#[source] NodeError),
}

/// The trace context of the call that this run is part of, if any: a sub-agent in the same
/// process runs inside the traced call of its caller (ADR-0123), and the headers of that call
/// are its parent. An explicit parent from a request wins.
fn ambient_trace_parent() -> Option<TraceParent> {
    let headers = grauwolf_tools::current_trace_headers();
    let value = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    Some(TraceParent {
        traceparent: value("traceparent")?,
        tracestate: value("tracestate"),
    })
}

/// Storage key of the input of a checkpoint run, for the remember step after resuming
/// (ADR-0077).
const RUN_INPUT_KEY: &str = "grauwolf.run_input";

/// One agent run: the options of the lifecycle around a graph run (ADR-0116).
pub struct AgentRun {
    input: Value,
    memory: Option<Arc<dyn FactMemory>>,
    recall: bool,
    remember: bool,
    compressed: bool,
    knowledge: Option<Arc<dyn KnowledgeContext>>,
    conversation: Option<(Arc<dyn ChatHistoryStore>, String)>,
    attachments: Vec<Attachment>,
    checkpoints: Option<(Arc<dyn CheckpointStore>, String)>,
    resumed: bool,
    caller: Option<String>,
    trace_parent: Option<TraceParent>,
    on_behalf_of: Option<OnBehalfOf>,
}

impl AgentRun {
    /// A run for `input`, the value the graph starts with (already normalized).
    pub fn new(input: Value) -> Self {
        Self {
            input,
            memory: None,
            recall: false,
            remember: false,
            compressed: false,
            knowledge: None,
            conversation: None,
            attachments: Vec::new(),
            checkpoints: None,
            resumed: false,
            caller: None,
            trace_parent: None,
            on_behalf_of: None,
        }
    }

    /// Automatic long-term memory (ADR-0069): with `recall` the relevant facts are added to the
    /// prompt before the graph runs, with `remember` new facts are stored after it succeeded.
    pub fn with_memory(
        mut self,
        memory: Arc<dyn FactMemory>,
        recall: bool,
        remember: bool,
    ) -> Self {
        self.memory = Some(memory);
        self.recall = recall;
        self.remember = remember;
        self
    }

    /// Also stores the durable facts of a history compression with the `Facts` strategy in the
    /// memory of [`AgentRun::with_memory`] (ADR-0079); without a memory it has no effect. Both
    /// the interpreter and generated code ask for it here, so they store the same facts.
    pub fn remember_compressed(mut self) -> Self {
        self.compressed = true;
        self
    }

    /// Runs as part of the conversation `id` (ADR-0060): the stored history follows the system
    /// prompt, and after a successful run the history is stored without it – before the
    /// completion event, so a failed save is a failed run.
    pub fn with_conversation(mut self, store: Arc<dyn ChatHistoryStore>, id: &str) -> Self {
        self.conversation = Some((store, id.to_owned()));
        self
    }

    /// Saves a checkpoint of run `run_id` to `store` before each node (ADR-0020). With
    /// automatic remember, the input is kept in the agent storage so that the remember step
    /// can use it after a resume (ADR-0077).
    pub fn with_checkpoints(mut self, store: Arc<dyn CheckpointStore>, run_id: &str) -> Self {
        self.checkpoints = Some((store, run_id.to_owned()));
        self
    }

    /// The run continues from a checkpoint (ADR-0034): `input` is the input of the checkpoint,
    /// and the context is restored. Recall, knowledge, conversation and attachments are
    /// skipped – they are already part of the checkpoint's history. The graph run passed to
    /// [`AgentRun::run`] resumes from the checkpoint.
    pub fn resumed(mut self) -> Self {
        self.resumed = true;
        self
    }

    /// The SPIFFE ID of the calling workload (ADR-0109); reported as `caller` in
    /// `agent_starting`. Never a token.
    pub fn with_caller(mut self, caller: Option<String>) -> Self {
        self.caller = caller;
        self
    }

    /// The trace context the caller sent (ADR-0123); reported as `trace_parent` in
    /// `agent_starting`, so that the tracing feature continues the caller's trace.
    pub fn with_trace_parent(mut self, trace_parent: Option<TraceParent>) -> Self {
        self.trace_parent = trace_parent;
        self
    }

    /// The person the run acts for (ADR-0111); reported as `on_behalf_of` in `agent_starting`.
    pub fn with_on_behalf_of(mut self, on_behalf_of: Option<OnBehalfOf>) -> Self {
        self.on_behalf_of = on_behalf_of;
        self
    }

    /// Attachments for the input of the run (ADR-0081).
    pub fn with_attachments(mut self, attachments: Vec<Attachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Knowledge as context (ADR-0113): the best sections for the input are added to the
    /// prompt before the graph runs.
    pub fn with_knowledge(mut self, knowledge: Arc<dyn KnowledgeContext>) -> Self {
        self.knowledge = Some(knowledge);
        self
    }

    /// Runs the lifecycle on `ctx`: reports `agent_starting`, calls `graph` with the context and
    /// the observer for the node events, and ends with exactly one of `agent_completed` or
    /// `agent_failed`, carrying usage and cost.
    pub async fn run<O, G>(self, ctx: &mut AgentContext, graph: G) -> Result<O, RunError>
    where
        O: Serialize + Send,
        G: for<'a> FnOnce(
                &'a mut AgentContext,
                &'a AgentObserver,
            ) -> BoxFuture<'a, Result<O, EngineError>>
            + Send,
    {
        let fresh = !self.resumed;
        if let (true, Some(memory)) = (self.compressed, &self.memory) {
            ctx.set_fact_memory(Arc::clone(memory));
        }
        // The history of the conversation follows the system prompt; what the run adds to it
        // (including recalled facts) is stored again.
        let system = ctx.prompt().messages.len();
        if let (true, Some((store, id))) = (fresh, &self.conversation) {
            let history = store.load(id).await.map_err(RunError::ChatHistory)?;
            let mut prompt = ctx.prompt().clone();
            prompt.messages.extend(history);
            ctx.replace_prompt(prompt);
        }
        if fresh {
            ctx.attach_to_input(self.attachments.clone());
        }
        let input_text = text_of(&self.input);
        ctx.emit(AgentEvent::AgentStarting {
            input: self.input.clone(),
            caller: self.caller.clone(),
            trace_parent: self.trace_parent.clone().or_else(ambient_trace_parent),
            on_behalf_of: self.on_behalf_of.clone(),
        });
        if fresh {
            if let (true, Some(memory)) = (self.recall, &self.memory) {
                recall_facts(ctx, memory.as_ref(), &input_text).await;
            }
            if let Some(knowledge) = &self.knowledge {
                inject_knowledge(ctx, knowledge.as_ref(), &input_text).await;
            }
            if self.checkpoints.is_some() && self.remember && self.memory.is_some() {
                let _ = ctx.store(RUN_INPUT_KEY, &input_text);
            }
        }
        let mut observer = AgentObserver::new();
        if let Some((store, run_id)) = &self.checkpoints {
            observer = observer.with_checkpoints(Arc::clone(store), run_id);
            ctx.set_progress_saver(observer.progress_saver());
        }
        let result = graph(ctx, &observer).await.map_err(RunError::from);
        let outcome = self.conclude(ctx, result, &input_text, system).await;
        match outcome {
            Ok((output, value)) => {
                ctx.emit(AgentEvent::AgentCompleted {
                    output: value,
                    usage: ctx.usage(),
                    cost: ctx.cost(),
                });
                Ok(output)
            }
            Err(error) => {
                ctx.emit(AgentEvent::AgentFailed {
                    error: error.to_string(),
                    usage: ctx.usage(),
                    cost: ctx.cost(),
                });
                Err(error)
            }
        }
    }

    /// What happens to a finished graph run before it is reported: the output as JSON, the
    /// facts to remember and the conversation to save. Any error here fails the run.
    async fn conclude<O: Serialize>(
        &self,
        ctx: &AgentContext,
        result: Result<O, RunError>,
        input_text: &str,
        system: usize,
    ) -> Result<(O, Value), RunError> {
        let output = result?;
        let value = serde_json::to_value(&output).map_err(RunError::Output)?;
        if let (true, Some(memory)) = (self.remember, &self.memory) {
            // A checkpoint run remembers the input kept in the storage, also after a resume.
            let input = match &self.checkpoints {
                Some(_) => ctx.load::<String>(RUN_INPUT_KEY).ok().flatten(),
                None => Some(input_text.to_owned()),
            };
            if let Some(input) = input {
                remember_facts(ctx, memory.as_ref(), &input, &text_of(&value)).await;
            }
        }
        if let Some((store, id)) = &self.conversation {
            let messages = ctx.prompt().messages.get(system..).unwrap_or_default();
            store
                .save(id, messages)
                .await
                .map_err(RunError::ChatHistory)?;
        }
        Ok((output, value))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures_executor::block_on;
    use futures_util::future::BoxFuture;
    use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
    use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
    use grauwolf_tools::{KnowledgeHit, ToolError};
    use serde::ser::Error as _;
    use serde_json::json;

    use crate::checkpoints::MemoryCheckpointStore;
    use crate::memory::MemoryError;
    use grauwolf_graph::CheckpointObserver as _;

    use super::*;
    use crate::events::Feature;

    /// Feature that collects all events.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<AgentEvent>>);

    impl Feature for Recorder {
        fn on_event(&self, event: &AgentEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    impl Recorder {
        /// The `event` tags in order.
        fn names(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|event| {
                    serde_json::to_value(event).unwrap()["event"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect()
        }
    }

    /// Answers every request with the fact list `["The user is called Ada."]`.
    struct FactsClient;

    impl LlmClient for FactsClient {
        fn execute<'a>(
            &'a self,
            _prompt: &'a Prompt,
            _model: &'a LlModel,
            _tools: &'a [ToolDescriptor],
        ) -> BoxFuture<'a, Result<Message, LlmError>> {
            Box::pin(std::future::ready(Ok(Message::assistant(
                r#"{"facts": ["The user is called Ada."]}"#,
            ))))
        }
    }

    /// Memory that knows one fact and keeps what it is asked to store.
    #[derive(Default)]
    struct FakeMemory(Mutex<Vec<String>>, Mutex<Vec<String>>);

    impl FactMemory for FakeMemory {
        fn recall<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Vec<String>, MemoryError>> {
            self.1.lock().unwrap().push(query.to_owned());
            Box::pin(async { Ok(vec!["The user likes tea.".to_owned()]) })
        }

        fn remember<'a>(&'a self, facts: &'a [String]) -> BoxFuture<'a, Result<(), MemoryError>> {
            self.0.lock().unwrap().extend_from_slice(facts);
            Box::pin(async { Ok(()) })
        }
    }

    /// Knowledge base with one section.
    struct FakeKnowledge;

    impl KnowledgeContext for FakeKnowledge {
        fn retrieve<'a>(
            &'a self,
            _query: &'a str,
        ) -> BoxFuture<'a, Result<Vec<KnowledgeHit>, ToolError>> {
            Box::pin(async {
                Ok(vec![KnowledgeHit {
                    source: "guide.md".into(),
                    text: "Tea needs hot water.".into(),
                }])
            })
        }
    }

    /// A graph run that adds one node event and answers "done".
    fn node_graph<'a>(
        ctx: &'a mut AgentContext,
        _observer: &'a AgentObserver,
    ) -> BoxFuture<'a, Result<Value, EngineError>> {
        Box::pin(async move {
            ctx.emit(AgentEvent::NodeStarting {
                path: vec!["n".into()],
                input: json!("hi"),
            });
            Ok(json!("done"))
        })
    }

    fn context(recorder: &Arc<Recorder>) -> AgentContext {
        AgentContext::new(
            Arc::new(
                PromptExecutor::builder()
                    .client("p", Arc::new(FactsClient))
                    .build(),
            ),
            LlModel::new("p", "m"),
            Prompt::builder("system").build(),
        )
        .install(recorder.clone())
    }

    /// Output whose conversion to JSON fails.
    struct Unserializable;

    impl Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(S::Error::custom("no JSON"))
        }
    }

    #[test]
    fn a_plain_run_reports_start_and_completion_around_the_graph() {
        let recorder = Arc::new(Recorder::default());
        let mut ctx = context(&recorder);
        let output = block_on(AgentRun::new(json!("hi")).run(&mut ctx, |ctx, _| {
            Box::pin(async move {
                ctx.emit(AgentEvent::NodeStarting {
                    path: vec!["n".into()],
                    input: json!("hi"),
                });
                Ok(json!("done"))
            })
        }))
        .unwrap();
        assert_eq!(output, json!("done"));
        assert_eq!(
            recorder.names(),
            ["agent_starting", "node_starting", "agent_completed"]
        );
    }

    #[test]
    fn the_caller_is_reported_in_agent_starting_only_when_there_is_one() {
        let starting = |caller: Option<&str>| {
            let recorder = Arc::new(Recorder::default());
            let mut ctx = context(&recorder);
            block_on(
                AgentRun::new(json!("hi"))
                    .with_caller(caller.map(str::to_owned))
                    .run(&mut ctx, |_, _| Box::pin(async { Ok(json!("done")) })),
            )
            .unwrap();
            let events = recorder.0.lock().unwrap();
            serde_json::to_value(&events[0]).unwrap()
        };
        assert_eq!(
            starting(Some("spiffe://example.org/ns/a/sa/coordinator"))["caller"],
            "spiffe://example.org/ns/a/sa/coordinator"
        );
        assert!(starting(None).get("caller").is_none());
    }

    #[test]
    fn the_person_is_reported_in_agent_starting_only_when_there_is_one() {
        let starting = |on_behalf_of: Option<OnBehalfOf>| {
            let recorder = Arc::new(Recorder::default());
            let mut ctx = context(&recorder);
            block_on(
                AgentRun::new(json!("hi"))
                    .with_on_behalf_of(on_behalf_of)
                    .run(&mut ctx, |_, _| Box::pin(async { Ok(json!("done")) })),
            )
            .unwrap();
            let events = recorder.0.lock().unwrap();
            serde_json::to_value(&events[0]).unwrap()
        };
        let person = OnBehalfOf {
            oid: "oid-alice".into(),
            name: Some("Alice".into()),
            upn: None,
            txn: "txn-1".into(),
            grant: None,
        };
        assert_eq!(
            starting(Some(person))["on_behalf_of"],
            json!({ "oid": "oid-alice", "name": "Alice", "txn": "txn-1" })
        );
        assert!(starting(None).get("on_behalf_of").is_none());
    }

    #[test]
    fn the_trace_parent_is_reported_in_agent_starting_only_when_there_is_one() {
        let starting = |trace_parent: Option<TraceParent>| {
            let recorder = Arc::new(Recorder::default());
            let mut ctx = context(&recorder);
            block_on(
                AgentRun::new(json!("hi"))
                    .with_trace_parent(trace_parent)
                    .run(&mut ctx, |_, _| Box::pin(async { Ok(json!("done")) })),
            )
            .unwrap();
            let events = recorder.0.lock().unwrap();
            serde_json::to_value(&events[0]).unwrap()
        };
        let parent = TraceParent {
            traceparent: "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into(),
            tracestate: Some("vendor=1".into()),
        };

        assert_eq!(
            starting(Some(parent))["trace_parent"],
            json!({
                "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
                "tracestate": "vendor=1"
            })
        );
        assert!(starting(None).get("trace_parent").is_none());
    }

    /// An agent that runs inside a traced call of another agent (a sub-agent in the same
    /// process) continues the trace of that call (ADR-0123).
    #[test]
    fn a_run_inside_a_traced_call_takes_its_trace_context_as_the_parent() {
        let recorder = Arc::new(Recorder::default());
        let mut ctx = context(&recorder);
        let headers = vec![
            (
                "traceparent".to_owned(),
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned(),
            ),
            ("tracestate".to_owned(), "vendor=1".to_owned()),
        ];

        block_on(grauwolf_tools::with_trace_headers(
            headers,
            AgentRun::new(json!("hi")).run(&mut ctx, |_, _| Box::pin(async { Ok(json!("done")) })),
        ))
        .unwrap();

        let events = recorder.0.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&events[0]).unwrap()["trace_parent"],
            json!({
                "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
                "tracestate": "vendor=1"
            })
        );
    }

    #[test]
    fn agent_starting_without_a_caller_still_parses() {
        let event: AgentEvent =
            serde_json::from_value(json!({ "event": "agent_starting", "input": "hi" })).unwrap();
        assert_eq!(
            event,
            AgentEvent::AgentStarting {
                input: json!("hi"),
                caller: None,
                trace_parent: None,
                on_behalf_of: None
            }
        );
    }

    #[test]
    fn a_failing_graph_reports_agent_failed_and_no_completion() {
        let recorder = Arc::new(Recorder::default());
        let mut ctx = context(&recorder);
        let result: Result<Value, _> =
            block_on(AgentRun::new(json!("hi")).run(&mut ctx, |_, _| {
                Box::pin(async { Err(EngineError::InvalidData(serde_json::Error::custom("boom"))) })
            }));
        assert!(result.is_err());
        assert_eq!(recorder.names(), ["agent_starting", "agent_failed"]);
    }

    #[test]
    fn an_output_that_cannot_be_converted_reports_agent_failed() {
        let recorder = Arc::new(Recorder::default());
        let mut ctx = context(&recorder);
        let result = block_on(
            AgentRun::new(json!("hi")).run(&mut ctx, |_, _| Box::pin(async { Ok(Unserializable) })),
        );
        assert!(matches!(result, Err(RunError::Output(_))));
        assert_eq!(recorder.names(), ["agent_starting", "agent_failed"]);
    }

    #[test]
    fn recall_and_knowledge_come_before_the_graph_and_remember_after_it() {
        let recorder = Arc::new(Recorder::default());
        let memory = Arc::new(FakeMemory::default());
        let mut ctx = context(&recorder);
        block_on(
            AgentRun::new(json!("hi"))
                .with_memory(memory.clone(), true, true)
                .with_knowledge(Arc::new(FakeKnowledge))
                .run(&mut ctx, node_graph),
        )
        .unwrap();
        assert_eq!(
            recorder.names(),
            [
                "agent_starting",
                "memory_recalled",
                "knowledge_injected",
                "node_starting",
                "memory_remembered",
                "agent_completed",
            ]
        );
        assert_eq!(*memory.0.lock().unwrap(), ["The user is called Ada."]);
    }

    #[test]
    fn memory_is_only_used_as_far_as_the_run_asks_for_it() {
        let recorder = Arc::new(Recorder::default());
        let memory = Arc::new(FakeMemory::default());
        let mut ctx = context(&recorder);
        block_on(
            AgentRun::new(json!("hi"))
                .with_memory(memory.clone(), false, false)
                .run(&mut ctx, node_graph),
        )
        .unwrap();
        assert_eq!(
            recorder.names(),
            ["agent_starting", "node_starting", "agent_completed"]
        );
        assert!(memory.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_graph_remembers_nothing() {
        let recorder = Arc::new(Recorder::default());
        let memory = Arc::new(FakeMemory::default());
        let mut ctx = context(&recorder);
        let result: Result<Value, _> = block_on(
            AgentRun::new(json!("hi"))
                .with_memory(memory.clone(), false, true)
                .run(&mut ctx, |_, _| {
                    Box::pin(async {
                        Err(EngineError::InvalidData(serde_json::Error::custom("boom")))
                    })
                }),
        );
        assert!(result.is_err());
        assert!(memory.0.lock().unwrap().is_empty());
    }

    /// Chat store that keeps what it is given and may refuse to save.
    #[derive(Default)]
    struct FakeChats {
        history: Mutex<Vec<Message>>,
        refuse_save: bool,
        recorder: Mutex<Option<Arc<Recorder>>>,
        names_at_save: Mutex<Vec<String>>,
    }

    impl ChatHistoryStore for FakeChats {
        fn load<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<Vec<Message>, NodeError>> {
            let history = self.history.lock().unwrap().clone();
            Box::pin(async move { Ok(history) })
        }

        fn save<'a>(
            &'a self,
            _id: &'a str,
            messages: &'a [Message],
        ) -> BoxFuture<'a, Result<(), NodeError>> {
            if let Some(recorder) = &*self.recorder.lock().unwrap() {
                *self.names_at_save.lock().unwrap() = recorder.names();
            }
            *self.history.lock().unwrap() = messages.to_vec();
            if self.refuse_save {
                return Box::pin(async { Err("disk full".into()) });
            }
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn a_conversation_is_saved_before_the_completion_event() {
        let recorder = Arc::new(Recorder::default());
        let chats = Arc::new(FakeChats::default());
        *chats.recorder.lock().unwrap() = Some(recorder.clone());
        let mut ctx = context(&recorder);
        block_on(
            AgentRun::new(json!("hi"))
                .with_conversation(chats.clone(), "c1")
                .run(&mut ctx, |ctx, _| {
                    Box::pin(async move {
                        ctx.append(Message::user("hi"));
                        Ok(json!("done"))
                    })
                }),
        )
        .unwrap();
        assert_eq!(
            *chats.names_at_save.lock().unwrap(),
            ["agent_starting"],
            "saved before agent_completed"
        );
        assert_eq!(
            chats.history.lock().unwrap().len(),
            1,
            "without the system prompt"
        );
        assert_eq!(recorder.names(), ["agent_starting", "agent_completed"]);
    }

    #[test]
    fn a_failed_conversation_save_reports_agent_failed_and_no_completion() {
        let recorder = Arc::new(Recorder::default());
        let chats = Arc::new(FakeChats {
            refuse_save: true,
            ..FakeChats::default()
        });
        let mut ctx = context(&recorder);
        let result: Result<Value, _> = block_on(
            AgentRun::new(json!("hi"))
                .with_conversation(chats, "c1")
                .run(&mut ctx, node_graph),
        );
        assert!(matches!(result, Err(RunError::ChatHistory(_))));
        assert_eq!(
            recorder.names(),
            ["agent_starting", "node_starting", "agent_failed"]
        );
    }

    #[test]
    fn the_stored_history_follows_the_system_prompt() {
        let recorder = Arc::new(Recorder::default());
        let chats = Arc::new(FakeChats::default());
        chats.history.lock().unwrap().push(Message::user("earlier"));
        let mut ctx = context(&recorder);
        block_on(
            AgentRun::new(json!("hi"))
                .with_conversation(chats, "c1")
                .run(&mut ctx, |ctx, _| {
                    let messages = ctx.prompt().messages.clone();
                    Box::pin(async move { Ok(json!(format!("{messages:?}"))) })
                }),
        )
        .map(|seen: Value| assert!(seen.as_str().unwrap().contains("earlier")))
        .unwrap();
    }

    #[test]
    fn a_checkpoint_run_saves_a_checkpoint_before_a_node() {
        let recorder = Arc::new(Recorder::default());
        let store = Arc::new(MemoryCheckpointStore::default());
        let mut ctx = context(&recorder);
        block_on(
            AgentRun::new(json!("hi"))
                .with_checkpoints(store.clone(), "r1")
                .run(&mut ctx, |ctx, observer| {
                    Box::pin(async move {
                        let checkpoint = grauwolf_graph::Checkpoint {
                            path: vec!["n".into()],
                            input: json!("hi"),
                        };
                        observer.checkpoint(ctx, &checkpoint).await.unwrap();
                        Ok(json!("done"))
                    })
                }),
        )
        .map(|_: Value| ())
        .unwrap();
        let state = block_on(store.load("r1")).unwrap().unwrap();
        assert_eq!(state.checkpoint.path, ["n"]);
    }

    #[test]
    fn a_resumed_run_neither_recalls_nor_injects_and_remembers_the_stored_input() {
        let recorder = Arc::new(Recorder::default());
        let memory = Arc::new(FakeMemory::default());
        let mut ctx = context(&recorder);
        ctx.store(RUN_INPUT_KEY, &"the original input").unwrap();
        block_on(
            AgentRun::new(json!("input of the checkpoint"))
                .with_memory(memory.clone(), true, true)
                .with_knowledge(Arc::new(FakeKnowledge))
                .with_checkpoints(Arc::new(MemoryCheckpointStore::default()), "r1")
                .resumed()
                .run(&mut ctx, node_graph),
        )
        .map(|_: Value| ())
        .unwrap();
        assert_eq!(
            recorder.names(),
            [
                "agent_starting",
                "node_starting",
                "memory_remembered",
                "agent_completed",
            ]
        );
        assert_eq!(*memory.1.lock().unwrap(), ["the original input"]);
    }

    #[test]
    fn a_checkpoint_run_keeps_its_input_for_the_remember_step() {
        let recorder = Arc::new(Recorder::default());
        let memory = Arc::new(FakeMemory::default());
        let mut ctx = context(&recorder);
        block_on(
            AgentRun::new(json!("hi"))
                .with_memory(memory.clone(), false, true)
                .with_checkpoints(Arc::new(MemoryCheckpointStore::default()), "r1")
                .run(&mut ctx, node_graph),
        )
        .map(|_: Value| ())
        .unwrap();
        assert_eq!(
            ctx.load::<String>(RUN_INPUT_KEY).unwrap().as_deref(),
            Some("hi")
        );
        assert_eq!(*memory.0.lock().unwrap(), ["The user is called Ada."]);
    }

    /// Answers every request with a fact list whose durable part is one fact.
    struct DurableClient;

    impl LlmClient for DurableClient {
        fn execute<'a>(
            &'a self,
            _prompt: &'a Prompt,
            _model: &'a LlModel,
            _tools: &'a [ToolDescriptor],
        ) -> BoxFuture<'a, Result<Message, LlmError>> {
            Box::pin(std::future::ready(Ok(Message::assistant(
                r#"{"facts": ["The user is called Ada."], "durable": ["The user is called Ada."]}"#,
            ))))
        }
    }

    /// Runs a graph that compresses a two-message history into facts; returns what the memory
    /// was asked to store.
    fn durable_facts_of_a_compressing_run(
        configure: impl FnOnce(AgentRun, Arc<FakeMemory>) -> AgentRun,
    ) -> Vec<String> {
        let recorder = Arc::new(Recorder::default());
        let memory = Arc::new(FakeMemory::default());
        let mut prompt = Prompt::builder("system").build();
        prompt.messages.extend([
            Message::user("My name is Ada."),
            Message::assistant("Hello Ada!"),
        ]);
        let mut ctx = AgentContext::new(
            Arc::new(
                PromptExecutor::builder()
                    .client("p", Arc::new(DurableClient))
                    .build(),
            ),
            LlModel::new("p", "m"),
            prompt,
        )
        .install(recorder);
        let run = configure(AgentRun::new(json!("hi")), memory.clone());
        block_on(run.run(&mut ctx, |ctx, _| {
            Box::pin(async move {
                ctx.compress_history(crate::HistoryCompression::Facts(0))
                    .await
                    .map_err(|error| EngineError::InvalidData(serde_json::Error::custom(error)))?;
                Ok(json!("done"))
            })
        }))
        .map(|_: Value| ())
        .unwrap();
        memory.0.lock().unwrap().clone()
    }

    #[test]
    fn durable_facts_of_a_compression_reach_the_memory_when_the_run_asks_for_it() {
        let stored = durable_facts_of_a_compressing_run(|run, memory| {
            run.with_memory(memory, false, false).remember_compressed()
        });
        assert_eq!(stored, ["The user is called Ada."]);
    }

    #[test]
    fn durable_facts_of_a_compression_stay_out_of_the_memory_unless_asked_for() {
        let stored =
            durable_facts_of_a_compressing_run(|run, memory| run.with_memory(memory, false, false));
        assert!(stored.is_empty());
    }
}
