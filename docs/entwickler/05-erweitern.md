# Kapitel 5: grauwolf erweitern

grauwolf ist so geschnitten, dass sich die häufigsten Erweiterungen an bekannten Stellen einfügen. In
diesem Kapitel lernst du drei Arten kennen:

1. **Neue Node-Art oder Kanten-Bedingung** – ein neuer Baustein für Graphen in Specs.
2. **Neuer LLM-Anbieter** – ein weiterer Adapter hinter dem Port `LlmClient`.
3. **Eigene Implementierungen weiterer Ports** – z. B. ein eigener Speicher für Gesprächsverläufe.

Du solltest [Kapitel 1](01-architektur.md) (Schichten, Ports & Adapter) und
[Kapitel 2](02-entwicklungsumgebung.md) (TDD, Prüfungen vor dem Commit) kennen. Node, Kante,
Bedingung und Anbieter erklärt das [Nutzerhandbuch](../handbuch/README.md), Anbieter im
Besonderen [Kapitel 2 des Nutzerhandbuchs](../handbuch/02-modelle.md).

> **Port** heißt in grauwolf ein Trait, den eine fachliche Crate definiert und eine technische
> Crate (ein **Adapter**) implementiert. Die Domain kennt nur den Trait, nie die Technik dahinter.

## 5.1 Neue Node-Art über den Katalog

### Warum ein Katalog?

Eine Node-Art wird an fünf Stellen gebraucht: bei der Spec-Prüfung, beim Builder-Agenten (der Specs
erzeugt), beim typisierten Rust-Builder, beim Interpreter und beim Generator. Damit diese Stellen nicht
auseinanderlaufen, gibt es **eine einzige Quelle** – den Node-Katalog in `grauwolf-graph`
([ADR-0018](../adr/0018-node-catalog.md)). Für Kanten-Bedingungen gilt dasselbe mit dem
Bedingungs-Katalog ([ADR-0021](../adr/0021-condition-catalog.md)).

```mermaid
flowchart LR
  cat["Katalog<br/>grauwolf-graph/src/catalog.rs"]
  val["Prüfung<br/>grauwolf-spec"]
  ba["Builder-Agent<br/>(Prompt listet alle Arten)"]
  typed["Typisierte Node-Funktion<br/>grauwolf-agents/src/nodes.rs"]
  impl["Wert-Implementierung<br/>grauwolf-agents/src/implementations.rs"]
  tpl["Generator-Vorlage<br/>grauwolf-generator/src/templates.rs"]
  cat --> val & ba
  cat -. Konformitätstest .-> impl & tpl
  typed --> tpl
  typed --> impl
```

Der Katalog ist **geschlossen**: `NodeKindDescriptor` hat private Felder, und neue Einträge entstehen
nur in `NodeCatalog::builtin()`. Eine neue Node-Art ist also immer eine Änderung am Framework selbst,
kein Plugin.

> Die Strukturknoten `subgraph`, `plan`, `goap`, `parallel` und `for_each` stehen **nicht** im Katalog. Sie sind
> Konstanten in `grauwolf-spec-model` (wieder exportiert von `grauwolf-spec`; `SUBGRAPH_KIND`, `PLAN_KIND`, …) und
> werden von Prüfung, Interpreter und Generator gesondert behandelt. Für neue Bausteine nutzt du den
> Katalog.

### Schritt 1: Eintrag im Katalog

Ein Eintrag beschreibt den Namen, das Parameter-Schema, Beispielparameter und die **Signatur** – also,
welchen Datentyp die Node annimmt und zurückgibt. Die Signatur kann von den Parametern abhängen.
Das ist der echte Eintrag für `store` (`crates/domain/grauwolf-graph/src/catalog.rs`):

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

Wichtig:

- `params_schema` unterstützt nur eine Teilmenge von JSON Schema (Objekt mit `properties`,
  `required`, `additionalProperties: false`; Werte `string`, `integer` mit `minimum`, `number`,
  `boolean`) – siehe `grauwolf-graph/src/params.rs`. Ohne Parameter nimmst du `no_params`.
- `example_params` müssen gültig sein: Die Tests in `crates/domain/grauwolf-graph/tests/node_catalog.rs`
  prüfen für **jede** Art, dass die Beispielparameter das Schema erfüllen und eine gültige Signatur
  ergeben. Ein benannter Typ, der darin vorkommt, heißt `Example`.
- `DataType` kennt `String`, `Message`, `ToolCall`, `ToolResult`, `ToolCalls`, `ToolResults` und
  `Named(name)` für Typen aus dem Abschnitt `types` der Spec.

Mit dem Eintrag kennt die Spec-Prüfung die Art schon: Sie prüft `params` gegen das Schema und die Typen
an den Kanten gegen die Signatur (die Regeln liegen in `grauwolf-spec/src/validate/`, `graph.rs` und `edges.rs` für Nodes und Kanten). Auch der Prompt des Builder-Agenten
(`grauwolf-builder-agent/src/prompt.rs`) listet die Arten automatisch über `kinds()`.

> **Eine Strategie-Vorlage hinzufügen.** Strategien (`single_run`, `react`, `verified`, `chat`) stehen in
> `crates/domain/grauwolf-strategies/src/lib.rs`: eine Funktion, die aus Katalog-Arten eine `GraphSpec`
> baut, ein `match`-Zweig mit dem Parameter-Schema in `expand`, und ein Test in
> `crates/domain/grauwolf-strategies/tests/expand.rs`. Tests, die die expandierte Spec laden und prüfen,
> gehören nach `grauwolf-spec/tests/strategies.rs`. Das Crate hängt nur von `grauwolf-spec-model` und
> `grauwolf-graph` ab (`cargo xtask check-arch` erzwingt das).

### Schritt 2: Typisierte Node-Funktion

Die eigentliche Logik schreibst du **einmal** als typisierte Funktion in
`crates/domain/grauwolf-agents/src/nodes.rs`. Sie bekommt den `AgentContext` (Verlauf, Modell,
Tools, Speicher) und die Eingabe und gibt eine `BoxFuture` zurück. Ohne Parameter ist es eine
gewöhnliche Funktion wie `llm_request`:

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

Mit Parametern ist es eine Fabrik, die eine Closure zurückgibt – Vorlage ist `store`:

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

Zwei von den drei Einstiegspunkten nutzen diese Funktion direkt: der typisierte Builder
(`g.node("keep", nodes::store::<String>("task"))`) und der erzeugte Code, der genau diesen
Aufruf als Text schreibt.

### Schritt 3: Wert-Implementierung für den Interpreter

Der Interpreter arbeitet auf `serde_json::Value` ([ADR-0017](../adr/0017-value-engine.md)). In
`crates/domain/grauwolf-agents/src/implementations.rs` bildet `node(kind, params, types)` jede Art auf
eine `NodeFn<AgentContext>` ab. Typisierte Funktionen ohne Parameter umhüllst du mit `erase_node`, das
an den Grenzen (de)serialisiert:

```rust
"llm_request" => Some(erase_node(nodes::llm_request)),
"execute_tool" => Some(erase_node(nodes::execute_tool)),
```

Parameter liest du aus `params`; fehlt etwas, gibt die Funktion `None` zurück (der Interpreter
meldet dann `InterpreterError::MissingImplementation`). Für `store` sieht es so aus:

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

Der Interpreter ruft `implementations::node` in `build_graph` auf
(`crates/domain/grauwolf-interpreter/src/lib.rs`); dort musst du nichts ändern.

### Schritt 4: Generator-Vorlage

In `crates/domain/grauwolf-generator/src/templates.rs` gibt `node(kind, params)` den Rust-Ausdruck
zurück, den der Generator in das erzeugte Crate schreibt – meist der Pfad zur typisierten Funktion aus
Schritt 2:

```rust
"store" => {
    return Some(format!(
        "grauwolf_agents::nodes::store::<{}>({:?})",
        rust_type(params["value"].as_str()?),
        params.get("key")?.as_str()?
    ));
}
```

`rust_type` übersetzt die Typnamen des Katalogs (`string`, `tool_result`, …) in Rust-Typen; benannte
Typen werden zu gleichnamigen generierten Structs. Mehr zum Generator in
[Kapitel 6](06-generator.md).

### Schritt 5: Tests

Eine neue Art braucht mindestens diese Tests – je einen Testfall pro TDD-Zyklus:

| Test | Ort | Prüft |
|---|---|---|
| Signatur | `grauwolf-graph/tests/node_catalog.rs` | Ein- und Ausgabetyp abhängig von den Parametern |
| Verhalten | `grauwolf-agents/tests/<thema>.rs` | Typisierte Node im `TypedGraph` und Wert-Implementierung |
| Konformanz (Interpreter) | `grauwolf-agents/tests/catalog_conformance.rs` | Jede Katalog-Art hat eine Implementierung |
| Konformanz (Generator) | `grauwolf-generator/tests/catalog_conformance.rs` | Jede Katalog-Art hat eine Vorlage |
| Äquivalenz | `grauwolf-generator/tests/equivalence.rs` | Interpreter und erzeugtes Crate liefern dasselbe Ergebnis |

Die beiden Konformanz-Tests musst du nicht anpassen – sie laufen über alle Arten im Katalog und
schlagen an, sobald du nur den Katalog erweitert hast (aus
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

Ein Verhaltenstest kann die typisierte Node und die Wert-Implementierung in einem Lauf vergleichen;
Vorlage ist `store_and_load_nodes_pass_values_around_the_edges` in
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

Für den **Äquivalenztest** legst du unter `examples/specs/` eine Beispiel-Spec an, die die neue Art
nutzt, und fügst der Liste `CASES` einen `Case` hinzu (Spec, Eingabe, Regeln des Mocks). Wie der Test
läuft und wie du aufräumst, beschreibt [Kapitel 6](06-generator.md).

### Schritt 6: Dokumentation

Die neue Art gehört in die [Spec-Referenz](../handbuch/referenz-spec.md) des Nutzerhandbuchs, ihre
Feature-ID in den [Feature-Katalog](../features/README.md) und – wenn eine Designentscheidung dahintersteht –
in ein neues ADR.

### Neue Kanten-Bedingung

Bedingungen funktionieren genauso, nur mit anderen Typen. Eine Bedingung ist eine
**Filter-Map**: Sie bekommt den Wert der Kante und liefert `Some(neuer_wert)`, wenn die Kante
passt, sonst `None`. Die Stellen auf einen Blick:

| Was | Wo |
|---|---|
| `ConditionDescriptor` mit `signature(params, input) -> DataType` | `ConditionCatalog::builtin()` in `grauwolf-graph/src/catalog.rs` |
| Typisierte Funktion `fn(&AgentContext, B) -> Option<C>` | `grauwolf-agents/src/conditions.rs` |
| Wert-Implementierung (`erase_condition`) | `implementations::condition` |
| Vorlage | `templates::condition` in `grauwolf-generator` |

Die typisierte Bedingung `on_tool_call` lässt nur Antworten mit genau einem Tool-Aufruf durch
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

Im Katalog prüft ihre Signatur den Eingabetyp mit dem Helfer `expect_input` (`Message` → `ToolCall`);
`implementations.rs` enthält
`"on_tool_call" => Some(erase_condition(conditions::on_tool_call))`, die Vorlage den Pfad
`grauwolf_agents::conditions::on_tool_call`.

## 5.2 Neuer LLM-Anbieter

### Der Port `LlmClient`

Der Port liegt in `crates/domain/grauwolf-llm/src/lib.rs`. Pflicht ist nur `execute`; alle anderen
Methoden haben Standardimplementierungen, die du überschreibst, wenn der Anbieter mehr kann:

| Methode | Standard | Überschreiben, wenn … |
|---|---|---|
| `execute(prompt, model, tools)` | – (Pflicht) | immer |
| `execute_with_usage` | `execute`, Verbrauch unbekannt | der Anbieter Token-Zahlen meldet ([ADR-0067](../adr/0067-token-usage.md)) |
| `execute_streaming` | ganze Antwort als ein Satz Frames | der Anbieter wirklich streamt ([ADR-0033](../adr/0033-streaming-port.md)); einmal `StreamFrame::Model(name)` senden, wenn der Stream das antwortende Modell nennt ([ADR-0094](../adr/0094-chat-api.md)) |
| `execute_structured(_with_usage)` | Schema als Systemanweisung | der Anbieter strukturierte Ausgabe nativ kann ([ADR-0031](../adr/0031-structured-output.md)) |
| `execute_choices` | `n` einzelne Anfragen | der Anbieter `n` Antworten auf einmal liefert ([ADR-0042](../adr/0042-request-modes.md)) |
| `moderate` | strukturierte Klassifikationsanfrage | es eine Moderations-API gibt ([ADR-0066](../adr/0066-content-moderation.md)) |
| `forces_tool_choice` | `true` | das Modell keinen Tool-Aufruf erzwingen kann ([ADR-0063](../adr/0063-tool-choice-without-forcing.md)) |

Der kleinste mögliche Client steht in einem Test (nach `crates/domain/grauwolf-agents/tests/interceptors.rs`):

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

Fehler meldest du als `LlmError`: `Transient { source, retry_after }` bei Überlastung, Ratenlimits
und Verbindungsfehlern (der `PromptExecutor` wiederholt sie), `Client(..)` für alles Übrige.

### Eine Adapter-Crate anlegen

Vorlage ist `crates/adapters/grauwolf-ollama`. Eine neue Anbieter-Crate

- liegt unter `crates/adapters/grauwolf-<anbieter>/` und hängt nur von `grauwolf-llm`,
  `grauwolf-llm-http`, `grauwolf-prompt` sowie `serde`, `serde_json`, `futures-util` ab
  (dev: `grauwolf-test`, `tokio`, `wiremock`);
- hat **eigene DTOs** für das Wire-Format in `src/dto.rs` statt eines SDKs von Drittanbietern
  ([ADR-0005](../adr/0005-provider-adapters.md));
- spricht HTTP ausschließlich über `grauwolf_llm_http::HttpClient` (`post_json`, `post_lines`,
  `post_sse`, `with_bearer_token`, `with_header`, `with_read_timeout`); dessen `HttpError` lässt sich
  per `From` in `LlmError` umwandeln;
- bekommt eine eigene `AGENTS.md` und ist automatisch Teil des Workspaces (`members = ["crates/*/*", …]`).

Der Kern des Ollama-Clients zeigt das Muster: `execute` ist von `execute_with_usage` abgeleitet, das
die DTOs füllt und die Antwort zusammen mit dem Verbrauch zurückgibt
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

### Testen: wiremock und Konformanz-Suite

Zwei Arten von Tests sind Pflicht:

1. **Aufgezeichnete Antworten mit wiremock** prüfen die Protokollabbildung ohne Netz. Vorlage:
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

2. **Konformanz-Suite gegen ein echtes Modell** (opt-in). `grauwolf_test::conformance::check_llm_client`
   prüft Text, Tool-Aufruf, Antwort nach einem Tool-Ergebnis, strukturierte Antwort und Streaming.
   Der Live-Test überspringt sich selbst ohne Umgebungsvariable (nach
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

### Registrierung in der Fassade

Damit Specs den Anbieter über `model.provider` nutzen können, registrierst du ihn in
`crates/apps/grauwolf-host` (der Compositions-Root; die Fassade reicht seine Module weiter, ADR-0108):

1. **Cargo-Feature** in `crates/apps/grauwolf-host/Cargo.toml`, z. B. `myprovider = ["dep:grauwolf-myprovider"]`,
   optional auch in `default`; das Crate mit `optional = true` aufnehmen. Die Fassade
   (`crates/apps/grauwolf/Cargo.toml`) bekommt dasselbe Feature, das an das des Hosts weiterreicht:
   `myprovider = ["dep:grauwolf-myprovider", "grauwolf-host/myprovider"]`.
2. **Re-Export** in `src/lib.rs` der Fassade: `#[cfg(feature = "myprovider")] pub use grauwolf_myprovider as myprovider;`
3. **`BUILTIN`** und **`client_with`** in `crates/apps/grauwolf-host/src/providers.rs` erweitern. Die
   Variablen des Anbieters in `ProvidersConfig` und in `VARIABLES` in `crates/apps/grauwolf-host/src/config.rs`
   aufnehmen (und in die Tabelle in `docs/handbuch/referenz-cli.md`; ein Test prüft sie); `client_with` liest die
   Einstellungen aus diesem Abschnitt, und `client_from` umhüllt es für eine `env`-Funktion – so bleibt es ohne
   echte Umgebungsvariablen testbar (`crates/apps/grauwolf-host/tests/providers.rs`):

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

4. Unterstützt der Anbieter Embeddings, erweiterst du außerdem `embedder_with` (Port `Embedder`, siehe unten).
5. Dokumentiere die Variablen in der Tabelle oben in `providers.rs` und im
   [Nutzerhandbuch](../handbuch/02-modelle.md).

Ein Anbieter, der nur die OpenAI-Chat-Completions-API spricht, braucht kein eigenes Crate: Ergänze einen
Eintrag `OpenAiCompatible { provider, api_key, base_url, default_base_url }` in `OPENAI_COMPATIBLE`, seinen
Namen in `BUILTIN` und seine beiden Variablen in `COMPATIBLE_VARIABLES` und `VARIABLES`
([ADR-0082](../adr/0082-openai-compatible-providers.md); ein Test vergleicht die Listen); die Variablen heißen dann
`<NAME>_API_KEY` und `<NAME>_BASE_URL`.

`executor_for` baut daraus den `PromptExecutor`, mit Wiederholungen nach `RetryPolicy::default()`
und dem `TokioClock`; `AgentLoader` nutzt ihn für jede Spec. Danach prüft `cargo xtask check-arch` die
Architekturregeln (Teil von `cargo test --workspace`).

## 5.3 Weitere Ports

Neben `LlmClient` gibt es weitere Ports, deren Adapter du austauschen kannst:

| Port | Definiert in | Mitgelieferte Implementierungen | Einhängen über |
|---|---|---|---|
| `ChatHistoryStore` | `grauwolf-agents::memory` | `MemoryChatHistoryStore`; `FileChatHistoryStore` (Fassade); `SqlChatHistoryStore` (`grauwolf-store-sql`) | `RunExtras { conversation: Some((store, id)), .. }` oder `run_conversation` |
| `CheckpointStore` | `grauwolf-agents::checkpoints` | `FileCheckpointStore` (Fassade); `SqlCheckpointStore` | `run_checkpointed(input, store, run_id)`, `resume(store, run_id)` |
| `FactMemory` | `grauwolf-agents::memory` | Das Langzeitgedächtnis des Hosts (`long_term_memory.rs`, über Provisioning genutzt) | `RunExtras { memory: Some(..), .. }` |
| `Embedder` | `grauwolf-rag` | `OllamaEmbedder`, `OpenAiEmbedder` (auch für die OpenAI-kompatiblen Anbieter), `GeminiEmbedder` | `grauwolf::providers::embedder_from` |
| `VectorStore` | `grauwolf-rag` | `InMemoryVectorStore`; `PgChunkStore` und `PgMemoryStore` (`grauwolf-store-pgvector`, ADR-0106) | Der Wissensindex (`knowledge.rs`) und das Gedächtnis (`long_term_memory.rs`) des Hosts |
| `Decider` | `grauwolf-agents::decision` | Das Modell selbst (`decider: llm`); `SystemOneClient` (`grauwolf-systemone`) | `Interpreter::with_decider`, `AgentLoader::with_decider`, `ProvisionOptions::with_decider` |
| `Clock` | `grauwolf-llm` | `TokioClock` (Fassade) | `PromptExecutor::builder().retry(policy, clock)` |
| `ToolRuntime` | `grauwolf-tools` | `SandboxRuntime` (`grauwolf-sandbox`) | `Interpreter::with_tool_runtime` |
| `ToolFactory` | `grauwolf-tools` | `BuildingToolFactory` (Fassade) | `Interpreter::with_tool_factory` |
| `UserIo` | `grauwolf-builtin-tools` | `ConsoleUserIo` | `AgentLoader::with_user_io` |

`ToolRuntime` und `ToolFactory` behandelt [Kapitel 4](04-tools-intern.md) ausführlich.

### Beispiel: ein eigener Gesprächsspeicher

`ChatHistoryStore` hat zwei Methoden: `load` liefert den Verlauf eines Gesprächs (ohne Systemprompt),
`save` ersetzt ihn. Die Referenzimplementierung im Speicher
(`crates/domain/grauwolf-agents/src/memory.rs`) ist kurz genug zum Abschreiben:

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

Ein Speicher wird pro Lauf eingehängt:

```rust
let store: Arc<dyn ChatHistoryStore> = Arc::new(MemoryChatHistoryStore::default());
let answer = agent.run_conversation(json!("Hello"), Arc::clone(&store), "conversation-1").await?;
```

Beim Schreiben eigener Speicher beachten:

- Gesprächs-IDs können einen Benutzer enthalten (`users/<hex>/<id>`, siehe
  `conversation_key` und [ADR-0060](../adr/0060-conversations-per-user.md)). Übersetzt du IDs in Dateinamen
  oder Pfade, musst du sie prüfen – `FileChatHistoryStore` zeigt, wie.
- Speichere atomar (Zwischendatei und `rename`, Transaktion), damit ein Abbruch keine halb geschriebenen
  Verläufe hinterlässt.
- Ein Adapter mit IO gehört nach `crates/adapters/` oder in die Fassade, **nicht** in die Domain.

### Beispiel: ein eigenes Gedächtnis (`FactMemory`)

Für das automatische Abrufen und Speichern von Fakten ([ADR-0069](../adr/0069-automatic-memory.md))
genügt ein Trait mit `recall` und `remember`. Nach
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

### Beispiel: ein eigener Embedder

Ein `Embedder` liefert einen Vektor je Text, in derselben Reihenfolge. Der Test-Embedder in
`crates/domain/grauwolf-rag/tests/retriever.rs` zählt einfach die Wörter eines Vokabulars – genug, um
Suche und Index ohne Modell zu testen:

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

### Beispiel: ein eigener `Decider`

Ein Entscheidungsknoten mit `decider: system_one` fragt den `Decider` des Laufs: `decide(&Decision, &state)`
liefert ein `Verdict`, dessen `answer` eine der Antworten der Entscheidung ist (Optionsname, `yes`/`no`,
Stufenbeschreibung), oder schlägt mit `DecideError` fehl. Die Regeln für Antworten stehen in der
Entscheidung selbst, sodass jeder Decider dieselben Antworten gibt: `DecisionKind::answer_for_probability(p)`
wendet die Schwelle einer `yes_no`-Frage an, `answer_for_score(mean)` rundet den mittleren Stufenindex und
begrenzt ihn auf die Skala. Ein Fehler, der vorübergehend sein kann, ist `DecideError::Transient`; wickle
den Decider in `RetryingDecider::new(inner, RetryPolicy::default(), clock)`, um gemäß Wiederholungsregel
erneut zu fragen. Ein fester Decider für Tests (nach `crates/domain/grauwolf-agents/tests/decide_node.rs`):

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

Der Adapter `grauwolf-systemone` ist die echte Implementierung; er wird aus `RKEV_BASE_URL` und
`RKEV_API_KEY` vom Provisioning gebaut (`ProvisionedAgent::new`, `decider()`), das auch vor dem Lauf
fehlschlägt, wenn eine Spec einen Dienst braucht und keiner da ist. Der Decider des Modells selbst
(`decider: llm`) braucht keinen Port: `AgentContext::decide_with_model` fragt das aktive Modell.

### Beispiel: `Clock`

Die Domain darf nicht selbst warten, weil sie keine Async-Laufzeit kennt
([ADR-0026](../adr/0026-runtime-abstractions.md)). Darum wartet der `PromptExecutor` vor Wiederholungen
über den Port `Clock`. Die tokio-Implementierung der Fassade (`crates/apps/grauwolf-host/src/runtime.rs`)
ist ein Einzeiler:

```rust
impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
```

Für eine andere Laufzeit – oder für Tests, die nicht wirklich warten sollen – implementierst du
`sleep` entsprechend und übergibst die Clock beim Bauen des Executors:

```rust
let executor = PromptExecutor::builder()
    .client("ollama", client)
    .retry(RetryPolicy::default(), Arc::new(TokioClock))
    .build();
```

Ohne `retry(..)` wiederholt der Executor gar nicht.

### Unteragenten als Schritte: `call_step`, `agent` und `for_each`

Die Nodes `agent` und `for_each` ([ADR-0101](../adr/0101-multi-agent-flows.md)) führen Unteragenten
deterministisch aus. Ein Unteragent ist ein `DynTool` in der Registry – dasselbe, was `agent_tools`
erzeugt (ein lokaler `InterpretedAgent::into_tool` oder ein entferntes A2A-Tool). Neu ist eine optionale
Methode des Tool-Ports:

```rust
fn call_step(&self, input: Value, parent: Option<StepHandles>) -> BoxFuture<'_, Result<Value, ToolError>>
```

`AgentContext::run_agent_step(name, input)` findet das Tool (unabhängig von den Tool-Scopes – die Node
nennt den Agenten) und ruft `call_step` mit dem aufrufenden Lauf als `StepHandles`, einem
`Arc<dyn Any + Send + Sync>`, der ein `grauwolf_agents::flows::ParentRun` hält. Die Standardimplementierung
schickt Text als `{ "request": text }` an `call_json` und gibt den Text zurück. Ein Agenten-Tool
überschreibt sie: Es übergibt die Eingabe als JSON-Wert (typisierte Werte bleiben typisiert) und läuft
mit `InterpretedAgent::run_as_step(input, extras, parent)`, das seinen Kontext per `adopt_run(&parent)`
übernehmen lässt: Verbrauch, Kosten, Zähler für Tool-Aufrufe und Budget gehören dem Elternlauf, während
Verlauf, Tools, Modell und Speicher eigene bleiben. `AgentContext::with_run_handle(handle)` legt einen
undurchsichtigen Handle der Laufzeit in den Kontext, den Unteragenten über `ParentRun::handle::<T>()`
finden – der Interpreter legt dort seinen `ToolApprover` ab, sodass Freigaben von Tools in Unteragenten
dieselbe Person fragen.

`for_each::run_for_each(ctx, &ForEach, node_id, input)` ist die gemeinsame Laufzeit der Node `for_each`:
`Body::Agent(name)` ruft `run_agent_step`, `Body::Graph { graph, scope, .. }` führt einen Graphen auf dem
Fork aus, wobei jedes Element auf `ctx.fork()` läuft, `max_parallel` gleichzeitig mit `buffer_unordered` –
innerhalb einer Task, so wie `parallel` es tut. Fertige Elemente werden unter `for_each::progress_key(id)`
und mit `save_progress()` gespeichert (ADR-0052). Der Interpreter baut die Konfiguration in `for_each_node`,
der Generator erzeugt dieselben Aufrufe (`for_each::for_each::<I, O>`), und `ForEachParams::parse` in
`grauwolf-spec-model` liest die Parameter für Prüfung, Interpreter und Generator gleichermaßen.

Einen Unteragenten für einen Test brauchst du ohne Spec: Implementiere `DynTool` mit `call_step`, lege es in
eine `ToolRegistry` und rufe `ctx.run_agent_step("worker", json!("Task"))`
(`crates/domain/grauwolf-agents/tests/agent_step.rs`).

## Das Wichtigste in Kürze

- Neue Node-Arten und Bedingungen entstehen **nur** über den Katalog in `grauwolf-graph`; sie kommen mit einer
  typisierten Funktion (`nodes.rs`/`conditions.rs`), einer Wert-Implementierung (`implementations.rs`) und einer
  Generator-Vorlage (`templates.rs`).
- Die Konformanz-Tests in `grauwolf-agents` und `grauwolf-generator` schlagen automatisch an, wenn eines davon
  fehlt; der Äquivalenztest belegt das gleiche Verhalten.
- Ein LLM-Anbieter ist ein Adapter-Crate mit eigenen DTOs, das `LlmClient` implementiert, mit wiremock und der
  Konformanz-Suite getestet wird und in `grauwolf::providers` registriert ist.
- Andere Ports (`ChatHistoryStore`, `CheckpointStore`, `FactMemory`, `Embedder`, `Clock`,
  `ToolRuntime`, `UserIo` …) sind kleine Traits mit `BoxFuture`-Methoden; eigene Implementierungen hängst du
  über `RunExtras`, `Interpreter` oder `AgentLoader` ein.

**Weiter mit [Kapitel 6: Generator](06-generator.md).**

### PostgreSQL-Speicher für Wissen und Gedächtnis (ADR-0106)

`grauwolf-store-pgvector` trennt SQL und Logik, damit die Logik ohne Datenbank läuft:

- `sql` hält die Anweisungen als Konstanten und kleine Builder (`create_chunks(dim)`, `vector_literal`);
  Vektoren reisen als pgvector-Textliterale, als Text gebunden und mit `::text::vector` umgewandelt.
- `chunks::{ChunkDb, ChunkTx}` und `memory::MemoryDb` sind die Datenbank-Ports. `sync_collection`
  (inkrementelle Indizierung: Hash je Chunk, Löschen entfernter Quellen und überzähliger Chunks, Neuaufbau bei
  Modell- oder Dimensionswechsel, alles in einer Transaktion unter `pg_advisory_xact_lock`) und `PgMemoryStore`
  (TTL, `min_similarity`, Benutzerbereiche, `access_count`) sind reines Rust darauf.
- `postgres::PgVector` implementiert die Ports mit sqlx und teilt sich einen Pool je URL.
  `InMemoryPgVector` implementiert sie im Speicher: Nutze es in Tests von Code, der einen `ChunkDb` oder
  `MemoryDb` entgegennimmt (`grauwolf-host/tests/pgvector_stores.rs` tut das).
- Tests gegen eine echte Datenbank (`tests/postgres.rs`) laufen nur mit `PGVECTOR_TEST_URL`, zum Beispiel
  `docker run -p 5432:5432 -e POSTGRES_PASSWORD=postgres pgvector/pgvector:pg17` und
  `PGVECTOR_TEST_URL=postgres://postgres:postgres@localhost:5432/postgres`; ohne sie kehren sie sofort zurück.
  Verwende eine Datenbank, deren pgvector-Tabellen leer sind oder 3 Dimensionen haben.
- Der Host liest die URL aus der Variablen, die `url_env` nennt (`grauwolf_host::pgvector::connect`);
  erzeugte Crates bekommen denselben Code über das Fassaden-Feature `pgvector`.

### URL-Quellen, Ähnlichkeitsschwelle und Wissen als Kontext (ADR-0113)

- **Abruf:** `grauwolf_tools_web::fetch_page(url, &Validators)` (bedingtes GET, Weiterleitungen nur auf
  dieselbe Herkunft, 5 MiB, Text/Markdown/HTML über `html_to_text`) wird von
  `grauwolf_host::urls::load_url` aufgerufen, das zwischen `Unchanged` (`304`, oder einem Fehler bei
  vorhandenem indiziertem Inhalt: Warnung auf stderr) und `Fetched` entscheidet. Der Dateispeicher hält die
  Validatoren je URL in seiner Indexdatei; `ChunkDb::url_validators` und `sync_collection_with(.., keep, validators)`
  leisten das für PostgreSQL (Tabelle `grauwolf_url_validators`). Tests nutzen wiremock; die PostgreSQL-Logik
  nutzt `InMemoryPgVector`.
- **Schwelle:** `Retriever::with_min_similarity` filtert nach der Suche im Speicher, sodass beide Speicher sie
  gemeinsam nutzen.
- **Kontext:** `DynTool::knowledge_context()` liefert einen `KnowledgeContext` für das Suchtool von
  `knowledge`; sowohl der Interpreter als auch erzeugte Crates finden ihn über den Namen des Tools
  (`grauwolf_tools::knowledge_context_of`). Der Lauf-Lebenszyklus (`grauwolf_agents::run_lifecycle`,
  ADR-0116) ruft nach dem Abruf des Gedächtnisses `grauwolf_agents::knowledge::inject_knowledge` auf; mit
  `knowledge.tool_enabled: false` stellt das Provisioning das Tool trotzdem bereit, es steht aber nicht in
  `AgentSpec::external_tools()`, sodass das Modell es nicht sieht.
