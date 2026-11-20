# Chapter 5: Extending grauwolf

grauwolf is cut so that the most common extensions slot in at well-known places. In this
chapter you will learn about three kinds:

1. **New node kind or edge condition** – a new building block for graphs in specs.
2. **New LLM provider** – another adapter behind the `LlmClient` port.
3. **Your own implementations of other ports** – e.g. your own store for conversation histories.

You should know [Chapter 1](01-architecture.md) (layers, ports & adapters) and
[Chapter 2](02-development-setup.md) (TDD, checks before committing). The terms node,
edge, condition and provider are explained in the [user guide](../user-guide/README.md), providers in
particular in [Chapter 2 of the user guide](../user-guide/02-models.md).

> In grauwolf, a **port** is a trait that a domain crate defines and a technical
> crate (an **adapter**) implements. The domain only knows the trait, never the technology behind it.

## 5.1 New node kind via the catalog

### Why a catalog?

A node kind is needed in five places: spec validation, the builder agent (which generates
specs), the typed Rust builder, the interpreter and the generator. So that these places do not
drift apart, there is **a single source** – the node catalog in `grauwolf-graph`
([ADR-0018](../adr/0018-node-catalog.md)). The same applies to edge conditions with the
condition catalog ([ADR-0021](../adr/0021-condition-catalog.md)).

```mermaid
flowchart LR
  cat["Catalog<br/>grauwolf-graph/src/catalog.rs"]
  val["Validation<br/>grauwolf-spec"]
  ba["Builder agent<br/>(prompt lists all kinds)"]
  typed["Typed node function<br/>grauwolf-agents/src/nodes.rs"]
  impl["Value implementation<br/>grauwolf-agents/src/implementations.rs"]
  tpl["Generator template<br/>grauwolf-generator/src/templates.rs"]
  cat --> val & ba
  cat -. conformance test .-> impl & tpl
  typed --> tpl
  typed --> impl
```

The catalog is **closed**: `NodeKindDescriptor` has private fields, and new entries
are only created in `NodeCatalog::builtin()`. A new node kind is therefore always a change to the
framework itself, not a plugin.

> The structural nodes `subgraph`, `plan`, `goap`, `parallel` and `for_each` are **not** in the catalog. They
> are constants in `grauwolf-spec-model` (re-exported by `grauwolf-spec`; `SUBGRAPH_KIND`, `PLAN_KIND`, …) and are handled specially by
> validation, interpreter and generator. For new building blocks you use the
> catalog.

### Step 1: Entry in the catalog

An entry describes the name, parameter schema, example parameters and the **signature** – that is,
which data type the node accepts and returns. The signature may depend on the parameters.
This is the real entry for `store` (`crates/domain/grauwolf-graph/src/catalog.rs`):

```rust
catalog.insert(NodeKindDescriptor {
    kind: "store",
    // Stores the value in the agent storage and passes it on (ADR-0044).
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
```

Important:

- `params_schema` supports only a subset of JSON Schema (object with `properties`,
  `required`, `additionalProperties: false`; values `string`, `integer` with `minimum`, `number`,
  `boolean`) – see `grauwolf-graph/src/params.rs`. Without parameters you use `no_params`.
- `example_params` must be valid: the tests in `crates/domain/grauwolf-graph/tests/node_catalog.rs`
  check for **every** kind that the example parameters satisfy the schema and yield a valid
  signature. A named type mentioned in them is called `Example`.
- `DataType` knows `String`, `Message`, `ToolCall`, `ToolResult`, `ToolCalls`, `ToolResults` and
  `Named(name)` for types from the spec's `types` section.

With the entry in place, spec validation already knows the kind: it checks `params` against the schema
and the types on the edges against the signature (the rules live in `grauwolf-spec/src/validate/`, `graph.rs` and `edges.rs` for nodes and edges). The builder agent's prompt
(`grauwolf-builder-agent/src/prompt.rs`) also lists the kinds automatically via `kinds()`.

> **Adding a strategy template.** Strategies (`single_run`, `react`, `verified`, `chat`) live in
> `crates/domain/grauwolf-strategies/src/lib.rs`: a function that builds a `GraphSpec` from catalog
> kinds, a `match` arm with the parameter schema in `expand`, and a test in
> `crates/domain/grauwolf-strategies/tests/expand.rs`. Tests that load and validate the expanded
> spec belong in `grauwolf-spec/tests/strategies.rs`. The crate depends only on
> `grauwolf-spec-model` and `grauwolf-graph` (`cargo xtask check-arch` enforces it).

### Step 2: Typed node function

You write the actual logic **once** as a typed function in
`crates/domain/grauwolf-agents/src/nodes.rs`. It receives the `AgentContext` (history, model,
tools, storage) and the input and returns a `BoxFuture`. Without parameters this is an
ordinary function like `llm_request`:

```rust
/// `llm_request`: sends `input` as a user message and returns the LLM's answer.
pub fn llm_request(
    ctx: &mut AgentContext,
    input: String,
) -> BoxFuture<'_, Result<Message, NodeError>> {
    Box::pin(async move {
        ctx.append(Message::user(input));
        Ok(ctx.request_llm().await?)
    })
}
```

With parameters it is a factory that returns a closure – the template is `store`:

```rust
pub fn store<T: NodeData>(
    key: &'static str,
) -> impl for<'a> Fn(&'a mut AgentContext, T) -> BoxFuture<'a, Result<T, NodeError>>
+ Send + Sync + 'static {
    move |ctx, value| {
        Box::pin(async move {
            ctx.store(key, &value)?;
            Ok(value)
        })
    }
}
```

Two of the three entry points use this function directly: the typed builder
(`g.node("keep", nodes::store::<String>("task"))`) and the generated code, which produces exactly this
call as text.

### Step 3: Value implementation for the interpreter

The interpreter works on `serde_json::Value` ([ADR-0017](../adr/0017-value-engine.md)). In
`crates/domain/grauwolf-agents/src/implementations.rs`, `node(kind, params, types)` maps every
kind to a `NodeFn<AgentContext>`. You wrap typed functions without parameters with
`erase_node`, which (de)serializes at the boundaries:

```rust
"llm_request" => Some(erase_node(nodes::llm_request)),
"execute_tool" => Some(erase_node(nodes::execute_tool)),
```

You read parameters from `params`; if something is missing, the function returns `None` (the interpreter
then reports `InterpreterError::MissingImplementation`). For `store` it looks like this:

```rust
"store" => {
    let key = params.get("key")?.as_str()?.to_owned();
    Some(Box::new(move |ctx: &mut AgentContext, value: Value| {
        let stored = ctx.store(key.clone(), &value);
        Box::pin(async move {
            stored?;
            Ok(value)
        })
    }))
}
```

The interpreter calls `implementations::node` in `build_graph`
(`crates/domain/grauwolf-interpreter/src/lib.rs`); you do not need to change anything there.

### Step 4: Generator template

In `crates/domain/grauwolf-generator/src/templates.rs`, `node(kind, params)` returns the
Rust expression that the generator writes into the generated crate – usually the path to the
typed function from step 2:

```rust
"store" => {
    return Some(format!(
        "grauwolf_agents::nodes::store::<{}>({:?})",
        rust_type(params["value"].as_str()?),
        params.get("key")?.as_str()?
    ));
}
```

`rust_type` translates catalog type names (`string`, `tool_result`, …) into Rust types; named
types become generated structs of the same name. More about the generator in
[Chapter 6](06-generator.md).

### Step 5: Tests

A new kind needs at least these tests – one test case per TDD cycle each:

| Test | Location | Checks |
|---|---|---|
| Signature | `grauwolf-graph/tests/node_catalog.rs` | Input and output type depending on the parameters |
| Behavior | `grauwolf-agents/tests/<topic>.rs` | Typed node in the `TypedGraph` and value implementation |
| Conformance (interpreter) | `grauwolf-agents/tests/catalog_conformance.rs` | Every catalog kind has an implementation |
| Conformance (generator) | `grauwolf-generator/tests/catalog_conformance.rs` | Every catalog kind has a template |
| Equivalence | `grauwolf-generator/tests/equivalence.rs` | Interpreter and generated crate produce the same result |

You do not need to adapt the two conformance tests – they run over all kinds in the catalog
and fail as soon as you have extended only the catalog (from
`crates/domain/grauwolf-agents/tests/catalog_conformance.rs`):

```rust
#[test]
fn every_builtin_node_kind_has_an_implementation() {
    let catalogs = Catalogs::builtin();
    let types = BTreeMap::from([("Example".to_string(), json!({ "type": "object" }))]);
    let missing: Vec<&str> = catalogs
        .nodes
        .kinds()
        .filter(|kind| {
            let params = catalogs.nodes.get(kind).unwrap().example_params();
            implementations::node(kind, &params, &types).is_none()
        })
        .collect();
    assert!(missing.is_empty(), "node kinds without implementation: {missing:?}");
}
```

A behavior test can compare the typed node and the value implementation in one run;
the template is `store_and_load_nodes_pass_values_around_the_edges` in
`crates/domain/grauwolf-agents/tests/storage.rs`:

```rust
let graph = TypedGraph::<AgentContext, String, String>::build(|g| {
    let keep = g.node("keep", nodes::store::<String>("task"));
    let other = g.node("other", |_, _: String| Box::pin(async { Ok(42_u32) }));
    let recall = g.node("recall", nodes::load::<u32, String>("task"));
    g.edge(&g.start(), &keep);
    g.edge(&keep, &other);
    g.edge(&other, &recall);
    g.edge(&recall, &g.finish());
})
.unwrap();
```

For the **equivalence test** you add an example spec under `examples/specs/` that uses the new
kind, and add a `Case` to `CASES` (spec, input, rules of the mock). How the test
runs and how you clean up is described in [Chapter 6](06-generator.md).

### Step 6: Documentation

The new kind belongs in the user guide's [spec reference](../user-guide/reference-spec.md),
its feature ID in the [feature catalog](../features/README.md) and – if there is a
design decision – in a new ADR.

### New edge condition

Conditions work the same way, just with other types. A condition is a
**filter-map**: it receives the edge's value and returns `Some(new_value)` if the edge
matches, otherwise `None`. The places at a glance:

| What | Where |
|---|---|
| `ConditionDescriptor` with `signature(params, input) -> DataType` | `ConditionCatalog::builtin()` in `grauwolf-graph/src/catalog.rs` |
| Typed function `fn(&AgentContext, B) -> Option<C>` | `grauwolf-agents/src/conditions.rs` |
| Value implementation (`erase_condition`) | `implementations::condition` |
| Template | `templates::condition` in `grauwolf-generator` |

The typed condition `on_tool_call` lets through only answers with exactly one tool call
(`crates/domain/grauwolf-agents/src/conditions.rs`):

```rust
pub fn on_tool_call(_ctx: &AgentContext, message: Message) -> Option<ToolCall> {
    let Message::Assistant(answer) = message else {
        return None;
    };
    let mut calls = answer.tool_calls();
    match (calls.next(), calls.next()) {
        (Some(call), None) => Some(call.clone()),
        _ => None,
    }
}
```

In the catalog, its signature checks the input type with the helper `expect_input`
(`Message` → `ToolCall`); `implementations.rs` contains
`"on_tool_call" => Some(erase_condition(conditions::on_tool_call))`, the template the path
`grauwolf_agents::conditions::on_tool_call`.

## 5.2 New LLM provider

### The `LlmClient` port

The port lives in `crates/domain/grauwolf-llm/src/lib.rs`. Only `execute` is required; all
other methods have default implementations that you override when the provider can do
better:

| Method | Default | Override if … |
|---|---|---|
| `execute(prompt, model, tools)` | – (required) | always |
| `execute_with_usage` | `execute`, usage unknown | the provider reports token counts ([ADR-0067](../adr/0067-token-usage.md)) |
| `execute_streaming` | whole answer as one set of frames | the provider really streams ([ADR-0033](../adr/0033-streaming-port.md)); emit `StreamFrame::Model(name)` once if the stream names the model that answered ([ADR-0094](../adr/0094-chat-api.md)) |
| `execute_structured(_with_usage)` | schema as a system instruction | the provider supports structured output natively ([ADR-0031](../adr/0031-structured-output.md)) |
| `execute_choices` | `n` individual requests | the provider returns `n` answers at once ([ADR-0042](../adr/0042-request-modes.md)) |
| `moderate` | structured classification request | there is a moderation API ([ADR-0066](../adr/0066-content-moderation.md)) |
| `forces_tool_choice` | `true` | the model cannot force a tool call ([ADR-0063](../adr/0063-tool-choice-without-forcing.md)) |

The smallest possible client is in a test (based on `crates/domain/grauwolf-agents/tests/interceptors.rs`):

```rust
impl LlmClient for Recorder {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.lock().unwrap().push((
            prompt.messages.clone(),
            tools.iter().map(|tool| tool.name.clone()).collect(),
        ));
        Box::pin(async { Ok(Message::assistant("The password is secret123.")) })
    }
}
```

You report errors as `LlmError`: `Transient { source, retry_after }` for overload, rate limits
and connection errors (the `PromptExecutor` retries them), `Client(..)` for everything else.

### Creating an adapter crate

The template is `crates/adapters/grauwolf-ollama`. A new provider crate

- lives under `crates/adapters/grauwolf-<provider>/` and depends only on `grauwolf-llm`,
  `grauwolf-llm-http`, `grauwolf-prompt` plus `serde`, `serde_json`, `futures-util`
  (dev: `grauwolf-test`, `tokio`, `wiremock`);
- has **its own DTOs** for the wire format in `src/dto.rs` instead of a third-party SDK
  ([ADR-0005](../adr/0005-provider-adapters.md));
- speaks HTTP exclusively via `grauwolf_llm_http::HttpClient` (`post_json`, `post_lines`,
  `post_sse`, `with_bearer_token`, `with_header`, `with_read_timeout`); its `HttpError` can be
  converted into `LlmError` via `From`;
- gets its own `AGENTS.md` and is automatically part of the workspace (`members = ["crates/*/*", …]`).

The core of the Ollama client shows the pattern: `execute` is derived from `execute_with_usage`,
which fills the DTOs and returns the answer together with the usage
(`crates/adapters/grauwolf-ollama/src/lib.rs`):

```rust
impl LlmClient for OllamaClient {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        Box::pin(async move { Ok(self.execute_with_usage(prompt, model, tools).await?.message) })
    }

    fn execute_with_usage<'a>(
        &'a self,
        prompt: &'a Prompt,
        model: &'a LlModel,
        tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Completion, LlmError>> {
        Box::pin(async move {
            let request = dto::ChatRequest {
                stream: false,
                ..chat_request(&model.id, prompt, tools, self.think)
            };
            self.chat(&request).await
        })
    }
    // execute_streaming, execute_structured, … overridden
}
```

### Testing: wiremock and conformance suite

Two kinds of tests are required:

1. **Recorded responses with wiremock** check the protocol mapping without a network. Template:
   `crates/adapters/grauwolf-ollama/tests/chat.rs`:

   ```rust
   #[tokio::test]
   async fn sends_the_conversation_and_returns_the_assistant_answer() {
       let server = MockServer::start().await;
       Mock::given(method("POST"))
           .and(path("/api/chat"))
           .and(body_json(json!({ "model": "qwen3.5:9b", "stream": false, "messages": [ … ] })))
           .respond_with(ResponseTemplate::new(200).set_body_json(json!({ … })))
           .expect(1)
           .mount(&server)
           .await;
       let client = OllamaClient::new(server.uri());
       // … call execute and check the answer
   }
   ```

2. **Conformance suite against a real model** (opt-in). `grauwolf_test::conformance::check_llm_client`
   checks text, tool call, answer after a tool result, structured answer and streaming.
   The live test skips itself without an environment variable (based on
   `crates/adapters/grauwolf-ollama/tests/live.rs`):

   ```rust
   #[tokio::test]
   async fn passes_the_conformance_suite_against_a_real_server() {
       let Ok(url) = std::env::var("GRAUWOLF_OLLAMA_URL") else {
           eprintln!("skipped: GRAUWOLF_OLLAMA_URL not set");
           return;
       };
       let model = std::env::var("GRAUWOLF_OLLAMA_MODEL").unwrap_or_else(|_| "qwen3.5:9b".into());
       let client = OllamaClient::new(url);

       let failures = check_llm_client(&client, &LlModel::new("ollama", model)).await;

       assert!(failures.is_empty(), "{failures:#?}");
   }
   ```

### Registration in the facade

So that specs can use the provider via `model.provider`, you register it in
`crates/apps/grauwolf-host` (the composition root; the facade re-exports its modules, ADR-0108):

1. **Cargo feature** in `crates/apps/grauwolf-host/Cargo.toml`, e.g. `myprovider = ["dep:grauwolf-myprovider"]`,
   optionally also in `default`; add the crate with `optional = true`. The facade
   (`crates/apps/grauwolf/Cargo.toml`) gets the same feature, forwarding to the host's:
   `myprovider = ["dep:grauwolf-myprovider", "grauwolf-host/myprovider"]`.
2. **Re-export** in the facade's `src/lib.rs`: `#[cfg(feature = "myprovider")] pub use grauwolf_myprovider as myprovider;`
3. Extend **`BUILTIN`** and **`client_with`** in `crates/apps/grauwolf-host/src/providers.rs`. Add the provider's
   variables to `ProvidersConfig` and to `VARIABLES` in `crates/apps/grauwolf-host/src/config.rs` (and the table in
   `docs/user-guide/reference-cli.md`, a test checks it); `client_with` reads the settings from that section and
   `client_from` wraps it for an `env` function – this keeps it testable without real environment variables
   (`crates/apps/grauwolf-host/tests/providers.rs`):

   ```rust
   match provider {
       #[cfg(feature = "anthropic")]
       "anthropic" => Ok(Arc::new(grauwolf_anthropic::AnthropicClient::new(
           required("ANTHROPIC_API_KEY", &config.anthropic_api_key)?,
       ))),
       // new branch for "myprovider"
       _ => Err(ProviderError::Unknown { provider: provider.to_owned() }),
   }
   ```

4. If the provider supports embeddings, also extend `embedder_with` (port `Embedder`, see below).
5. Document the variables in the table at the top of `providers.rs` and in the
   [user guide](../user-guide/02-models.md).

A provider that only speaks the OpenAI Chat Completions API needs no crate of its own: add an
`OpenAiCompatible { provider, api_key, base_url, default_base_url }` entry to
`OPENAI_COMPATIBLE`, its name to `BUILTIN` and its two variables to `COMPATIBLE_VARIABLES` and `VARIABLES`
([ADR-0082](../adr/0082-openai-compatible-providers.md); a test compares the lists); the variables are then
`<NAME>_API_KEY` and `<NAME>_BASE_URL`.

`executor_for` builds the `PromptExecutor` from this, with retries according to `RetryPolicy::default()`
and the `TokioClock`; `AgentLoader` uses it for every spec. Afterwards, the architecture rules are checked by
`cargo xtask check-arch` (part of `cargo test --workspace`).

## 5.3 Other ports

Besides `LlmClient` there are other ports whose adapters you can swap:

| Port | Defined in | Bundled implementations | Plugged in via |
|---|---|---|---|
| `ChatHistoryStore` | `grauwolf-agents::memory` | `MemoryChatHistoryStore`; `FileChatHistoryStore` (facade); `SqlChatHistoryStore` (`grauwolf-store-sql`) | `RunExtras { conversation: Some((store, id)), .. }` or `run_conversation` |
| `CheckpointStore` | `grauwolf-agents::checkpoints` | `FileCheckpointStore` (facade); `SqlCheckpointStore` | `run_checkpointed(input, store, run_id)`, `resume(store, run_id)` |
| `FactMemory` | `grauwolf-agents::memory` | The host's long-term memory (`long_term_memory.rs`, used through provisioning) | `RunExtras { memory: Some(..), .. }` |
| `Embedder` | `grauwolf-rag` | `OllamaEmbedder`, `OpenAiEmbedder` (also for the OpenAI-compatible providers), `GeminiEmbedder` | `grauwolf::providers::embedder_from` |
| `VectorStore` | `grauwolf-rag` | `InMemoryVectorStore`; `PgChunkStore` and `PgMemoryStore` (`grauwolf-store-pgvector`, ADR-0106) | The host's knowledge index (`knowledge.rs`) and memory (`long_term_memory.rs`) |
| `Decider` | `grauwolf-agents::decision` | The model itself (`decider: llm`); `SystemOneClient` (`grauwolf-systemone`) | `Interpreter::with_decider`, `AgentLoader::with_decider`, `ProvisionOptions::with_decider` |
| `Clock` | `grauwolf-llm` | `TokioClock` (facade) | `PromptExecutor::builder().retry(policy, clock)` |
| `ToolRuntime` | `grauwolf-tools` | `SandboxRuntime` (`grauwolf-sandbox`) | `Interpreter::with_tool_runtime` |
| `ToolFactory` | `grauwolf-tools` | `BuildingToolFactory` (facade) | `Interpreter::with_tool_factory` |
| `UserIo` | `grauwolf-builtin-tools` | `ConsoleUserIo` | `AgentLoader::with_user_io` |

`ToolRuntime` and `ToolFactory` are covered in detail in [Chapter 4](04-tools-internals.md).

### Example: your own conversation store

`ChatHistoryStore` has two methods: `load` returns a conversation's history (without the
system prompt), `save` replaces it. The in-memory reference implementation
(`crates/domain/grauwolf-agents/src/memory.rs`) is short enough to copy:

```rust
#[derive(Debug, Default)]
pub struct MemoryChatHistoryStore(Mutex<HashMap<String, Vec<Message>>>);

impl ChatHistoryStore for MemoryChatHistoryStore {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Message>, NodeError>> {
        let messages = self
            .0
            .lock()
            .expect("chat store")
            .get(conversation_id)
            .cloned()
            .unwrap_or_default();
        Box::pin(async move { Ok(messages) })
    }

    fn save<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [Message],
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        self.0
            .lock()
            .expect("chat store")
            .insert(conversation_id.to_owned(), messages.to_vec());
        Box::pin(async { Ok(()) })
    }
}
```

A store is plugged in per run:

```rust
let store: Arc<dyn ChatHistoryStore> = Arc::new(MemoryChatHistoryStore::default());
let answer = agent.run_conversation(json!("Hello"), Arc::clone(&store), "conversation-1").await?;
```

Keep in mind when writing your own store:

- Conversation IDs can contain a user (`users/<hex>/<id>`, see
  `conversation_key` and [ADR-0060](../adr/0060-conversations-per-user.md)). If you translate IDs into
  file names or paths, you must validate them – `FileChatHistoryStore` shows how.
- Save atomically (intermediate file and `rename`, transaction) so that an abort does not leave
  half-written histories behind.
- An adapter with IO belongs in `crates/adapters/` or in the facade, **not** in the domain.

### Example: your own memory (`FactMemory`)

For automatically recalling and storing facts ([ADR-0069](../adr/0069-automatic-memory.md)),
a trait with `recall` and `remember` is enough. Based on
`crates/domain/grauwolf-interpreter/tests/auto_memory.rs`:

```rust
#[derive(Default)]
struct Facts(Mutex<Vec<String>>);

impl FactMemory for Facts {
    fn recall<'a>(
        &'a self,
        _query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, grauwolf_agents::memory::MemoryError>> {
        Box::pin(async { Ok(vec!["Anna's favorite drink is green tea.".to_owned()]) })
    }

    fn remember<'a>(
        &'a self,
        facts: &'a [String],
    ) -> BoxFuture<'a, Result<(), grauwolf_agents::memory::MemoryError>> {
        self.0.lock().unwrap().extend(facts.iter().cloned());
        Box::pin(async { Ok(()) })
    }
}

// Run with memory
let output = agent
    .run_with(json!("What do I like to drink?"), RunExtras {
        memory: Some(Arc::new(Facts::default())),
        ..RunExtras::default()
    })
    .await?;
```

### Example: your own embedder

An `Embedder` returns one vector per text, in the same order. The test embedder in
`crates/domain/grauwolf-rag/tests/retriever.rs` simply counts the words of a vocabulary – enough to
test search and index without a model:

```rust
impl Embedder for WordCounter {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        let embeddings = texts
            .iter()
            .map(|text| {
                let text = text.to_lowercase();
                Embedding(VOCABULARY.iter().map(|word| text.matches(word).count() as f32).collect())
            })
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }
}
```

### Example: your own `Decider`

A decision node with `decider: system_one` asks the `Decider` of the run: `decide(&Decision, &state)`
returns a `Verdict` whose `answer` is one of the answers of the decision (option name, `yes`/`no`,
level description) or fails with `DecideError`. The answer rules are in the decision itself, so
every decider gives the same answers: `DecisionKind::answer_for_probability(p)` applies the
threshold of a `yes_no` question, `answer_for_score(mean)` rounds the mean level index and clamps
it to the scale. A failure that may pass is `DecideError::Transient`; wrap the decider in
`RetryingDecider::new(inner, RetryPolicy::default(), clock)` to ask again as the retry policy
allows. A fixed decider for tests (based on `crates/domain/grauwolf-agents/tests/decide_node.rs`):

```rust
struct AlwaysTechnical;

impl Decider for AlwaysTechnical {
    fn decide<'a>(
        &'a self,
        _decision: &'a Decision,
        _state: &'a Value,
    ) -> BoxFuture<'a, Result<Verdict, DecideError>> {
        Box::pin(std::future::ready(Ok(Verdict::new("technical"))))
    }
}

let agent = Interpreter::new(executor)
    .with_decider(Arc::new(AlwaysTechnical))
    .load(&spec)?;
```

The adapter `grauwolf-systemone` is the real implementation; it is built from `RKEV_BASE_URL` and
`RKEV_API_KEY` by provisioning (`ProvisionedAgent::new`, `decider()`), which also fails
before the run when a spec needs a service and there is none. The decider of the model itself
(`decider: llm`) needs no port: `AgentContext::decide_with_model` asks the active model.

### Example: `Clock`

The domain must not wait by itself because it knows no async runtime
([ADR-0026](../adr/0026-runtime-abstractions.md)). That is why the `PromptExecutor` waits before
retries via the `Clock` port. The facade's tokio implementation
(`crates/apps/grauwolf-host/src/runtime.rs`) is a one-liner:

```rust
impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
```

For a different runtime – or for tests that should not really wait – you implement
`sleep` accordingly and pass the clock when building the executor:

```rust
let executor = PromptExecutor::builder()
    .client("ollama", client)
    .retry(RetryPolicy::default(), Arc::new(TokioClock))
    .build();
```

Without `retry(..)` the executor does not retry at all.

### Sub-agents as steps: `call_step`, `agent` and `for_each`

The nodes `agent` and `for_each` ([ADR-0101](../adr/0101-multi-agent-flows.md)) run sub-agents
deterministically. A sub-agent is a `DynTool` in the registry – the same thing that `agent_tools`
produces (a local `InterpretedAgent::into_tool` or a remote A2A tool). What is new is one optional
method of the tool port:

```rust
fn call_step(&self, input: Value, parent: Option<StepHandles>) -> BoxFuture<'_, Result<Value, ToolError>>
```

`AgentContext::run_agent_step(name, input)` finds the tool (regardless of tool scopes – the node
names the agent) and calls `call_step` with the calling run as `StepHandles`, an
`Arc<dyn Any + Send + Sync>` that holds a `grauwolf_agents::flows::ParentRun`. The default
implementation sends text as `{ "request": text }` to `call_json` and returns the text. An agent
tool overrides it: it passes the input as the JSON value (typed values stay typed) and runs with
`InterpretedAgent::run_as_step(input, extras, parent)`, which makes its context
`adopt_run(&parent)`: usage, cost, tool-call counters and budget are the parent's, while history,
tools, model and storage stay its own. `AgentContext::with_run_handle(handle)` puts an opaque handle
of the runtime into the context that sub-agents find in `ParentRun::handle::<T>()` – the
interpreter puts its `ToolApprover` there, so approvals of tools in sub-agents ask the same person.

`for_each::run_for_each(ctx, &ForEach, node_id, input)` is the shared runtime of the `for_each`
node: `Body::Agent(name)` calls `run_agent_step`, `Body::Graph { graph, scope, .. }` runs a graph on
the fork, each element on `ctx.fork()`, `max_parallel` at a time with `buffer_unordered` – within
one task, as `parallel` does. Finished elements are saved under `for_each::progress_key(id)` and
`save_progress()` (ADR-0052). The interpreter builds the config in `for_each_node`, the generator
emits the same calls (`for_each::for_each::<I, O>`), and `ForEachParams::parse` in
`grauwolf-spec-model` reads the parameters for validation, interpreter and generator alike.

A sub-agent for a test needs no spec: implement `DynTool` with `call_step`, put it into a
`ToolRegistry` and call `ctx.run_agent_step("worker", json!("Task"))`
(`crates/domain/grauwolf-agents/tests/agent_step.rs`).

## Key points

- New node kinds and conditions are created **only** via the catalog in `grauwolf-graph`; they
  come with a typed function (`nodes.rs`/`conditions.rs`), a value implementation
  (`implementations.rs`) and a generator template (`templates.rs`).
- The conformance tests in `grauwolf-agents` and `grauwolf-generator` fail automatically
  if one of these is missing; the equivalence test proves identical behavior.
- An LLM provider is an adapter crate with its own DTOs that implements `LlmClient`, is tested with
  wiremock and the conformance suite, and is registered in `grauwolf::providers`.
- Other ports (`ChatHistoryStore`, `CheckpointStore`, `FactMemory`, `Embedder`, `Clock`,
  `ToolRuntime`, `UserIo` …) are small traits with `BoxFuture` methods; you plug in your own
  implementations via `RunExtras`, `Interpreter` or `AgentLoader`.

**Continue with [Chapter 6: Generator](06-generator.md).**

### PostgreSQL stores for knowledge and memory (ADR-0106)

`grauwolf-store-pgvector` keeps the SQL and the logic apart so that the logic runs without a
database:

- `sql` holds the statements as constants and small builders (`create_chunks(dim)`, `vector_literal`);
  vectors travel as pgvector text literals bound as text and cast with `::text::vector`.
- `chunks::{ChunkDb, ChunkTx}` and `memory::MemoryDb` are the database ports. `sync_collection`
  (incremental indexing: hash per chunk, delete removed sources and surplus chunks, rebuild on model
  or dimension change, all in one transaction under `pg_advisory_xact_lock`) and `PgMemoryStore`
  (TTL, `min_similarity`, user areas, `access_count`) are plain Rust on top of them.
- `postgres::PgVector` implements the ports with sqlx and shares one pool per URL.
  `InMemoryPgVector` implements them in memory: use it in tests of code that takes a `ChunkDb` or
  `MemoryDb` (`grauwolf-host/tests/pgvector_stores.rs` does).
- Tests against a real database (`tests/postgres.rs`) run only with `PGVECTOR_TEST_URL`, for example
  `docker run -p 5432:5432 -e POSTGRES_PASSWORD=postgres pgvector/pgvector:pg17` and
  `PGVECTOR_TEST_URL=postgres://postgres:postgres@localhost:5432/postgres`; without it they return
  early. Use a database whose pgvector tables are empty or have 3 dimensions.
- The host reads the URL from the variable named by `url_env` (`grauwolf_host::pgvector::connect`);
  generated crates get the same code through the facade feature `pgvector`.

### URL sources, similarity threshold and knowledge as context (ADR-0113)

- **Fetching:** `grauwolf_tools_web::fetch_page(url, &Validators)` (conditional GET, same-origin
  redirects, 5 MiB, text/Markdown/HTML through `html_to_text`) is called by
  `grauwolf_host::urls::load_url`, which decides between `Unchanged` (`304`, or a failure with
  indexed content: warning on stderr) and `Fetched`. The file store keeps the validators per URL in
  its index file; `ChunkDb::url_validators` and `sync_collection_with(.., keep, validators)` do it for
  PostgreSQL (table `grauwolf_url_validators`). Tests use wiremock; the PostgreSQL logic uses
  `InMemoryPgVector`.
- **Threshold:** `Retriever::with_min_similarity` filters after the store search, so both stores
  share it.
- **Context:** `DynTool::knowledge_context()` returns a `KnowledgeContext` for the search tool of
  `knowledge`; both the interpreter and generated crates find it by the tool's name
  (`grauwolf_tools::knowledge_context_of`). The run lifecycle (`grauwolf_agents::run_lifecycle`,
  ADR-0116) calls `grauwolf_agents::knowledge::inject_knowledge` after the memory recall; with
  `knowledge.tool_enabled: false` the tool is still provided by provisioning but is not in
  `AgentSpec::external_tools()`, so the model does not see it.
