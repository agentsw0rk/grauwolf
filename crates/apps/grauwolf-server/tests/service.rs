//! A generated agent as an HTTP service (ADR-0093, feature `service`).
#![cfg(feature = "service")]

use grauwolf_spec::AgentSpecExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use grauwolf_host::provisioning::ProvisionedAgent;
use grauwolf_server::service::{GeneratedAgent, RunApp, ServiceConfig, provision_options};
use grauwolf_spec::AgentSpec;
use serde_json::{Value, json};

fn config(variables: &[(&str, &str)]) -> Result<ServiceConfig, String> {
    let variables: HashMap<String, String> = variables
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    ServiceConfig::from_env_with(|name| variables.get(name).cloned())
}

#[test]
fn reads_the_settings_from_the_environment() {
    let defaults = config(&[]).unwrap();
    let set = config(&[
        ("SERVER_PORT", "9090"),
        ("GRAUWOLF_SERVER_TOKEN", "secret"),
        ("GRAUWOLF_DATA_DIR", "/data"),
        ("A2A_PORT", "9000"),
        ("A2A_BASE_PATH", "a2a/"),
    ])
    .unwrap();

    assert_eq!(defaults.port, 8080);
    assert_eq!(defaults.bind.to_string(), "0.0.0.0");
    assert_eq!(defaults.token, None);
    assert_eq!(defaults.data_dir, PathBuf::from("."));
    assert_eq!(defaults.a2a_port, None);
    assert_eq!(set.port, 9090);
    assert_eq!(set.token.as_deref(), Some("secret"));
    assert_eq!(set.data_dir, PathBuf::from("/data"));
    assert_eq!(set.a2a_port, Some(9000));
    assert_eq!(set.a2a_base_path, "/a2a");
    assert!(config(&[("SERVER_PORT", "eighty")]).is_err());
}

#[test]
fn refuses_to_start_without_any_token_unless_told_otherwise() {
    let open = config(&[]).unwrap();
    let allowed = config(&[("GRAUWOLF_ALLOW_ANONYMOUS", "true")]).unwrap();
    let protected = config(&[("GRAUWOLF_SERVER_TOKEN", "secret")]).unwrap();

    assert!(
        open.check_access()
            .unwrap_err()
            .contains("GRAUWOLF_SERVER_TOKEN")
    );
    assert!(allowed.check_access().is_ok());
    assert!(protected.check_access().is_ok());
}

/// The agent of `spec` provisioned the way the generated service does it.
async fn provisioned(spec: &AgentSpec, config: &ServiceConfig) -> Arc<ProvisionedAgent> {
    let dir = grauwolf_test::TestDir::new("service");
    Arc::new(
        ProvisionedAgent::new(
            spec,
            &dir,
            provision_options(config.chat_api.as_deref())
                .with_executor(Arc::new(grauwolf_llm::PromptExecutor::builder().build())),
        )
        .await
        .unwrap(),
    )
}

/// Serves a fake generated agent that answers with its input and the user.
async fn serve(config: &ServiceConfig) -> String {
    let run: RunApp = Arc::new(|_agent, input, options| {
        Box::pin(async move {
            Ok(json!(format!(
                "{} / {}",
                input.as_str().ok_or("not text")?,
                options.user.as_deref().unwrap_or("nobody"),
            )))
        })
    });
    serve_run(config, run).await
}

/// Serves a generated agent whose run entry point is `run`.
async fn serve_run(config: &ServiceConfig, run: RunApp) -> String {
    let spec = AgentSpec::from_json(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let agent = Arc::new(GeneratedAgent::new(
        &spec,
        provisioned(&spec, config).await,
        run,
    ));
    let router = grauwolf_server::service::router(agent, &spec, config)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

#[tokio::test]
async fn serves_a_generated_agent_with_a_user() {
    let base = serve(&config(&[("GRAUWOLF_DATA_DIR", "/data")]).unwrap()).await;
    let client = reqwest::Client::new();

    let health = reqwest::get(format!("{base}/health")).await.unwrap();
    let answer: Value = client
        .post(format!("{base}/run"))
        .json(&json!({ "input": "hi", "user_id": "anna" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(health.text().await.unwrap(), "ok");
    assert_eq!(answer, json!({ "output": "hi / anna" }));
}

#[tokio::test]
async fn protects_the_routes_with_the_token_but_not_health() {
    let base = serve(&config(&[("GRAUWOLF_SERVER_TOKEN", "secret")]).unwrap()).await;
    let client = reqwest::Client::new();

    let anonymous = client
        .post(format!("{base}/run"))
        .json(&json!({ "input": "hi" }))
        .send()
        .await
        .unwrap();
    let authorized: Value = client
        .post(format!("{base}/run"))
        .bearer_auth("secret")
        .json(&json!({ "input": "hi" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let health = reqwest::get(format!("{base}/health")).await.unwrap();

    assert_eq!(anonymous.status(), 401);
    assert_eq!(authorized["output"], "hi / nobody");
    assert_eq!(health.status(), 200);
}

#[test]
fn the_chat_api_is_opt_in_through_the_environment() {
    assert!(config(&[]).unwrap().chat_api.is_none());
    assert!(
        config(&[("GRAUWOLF_CHAT_API", "1")])
            .unwrap()
            .chat_api
            .is_some()
    );
    assert!(
        config(&[("GRAUWOLF_CHAT_API", "true")])
            .unwrap()
            .chat_api
            .is_some()
    );
    assert!(
        config(&[("GRAUWOLF_CHAT_API", "0")])
            .unwrap()
            .chat_api
            .is_none()
    );
}

/// `/api/chat` runs the generated agent; the model it reports shows up in `/api/signature`
/// under the spec's name from `agent-names.json`.
#[tokio::test]
async fn serves_the_chat_api_of_a_generated_agent_and_reports_its_models() {
    use grauwolf_agents::events::AgentEvent;
    use grauwolf_prompt::{LlModel, Message};

    let dir = grauwolf_test::TestDir::new("service-chat-api");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("agent-signature.json"), r#"{ "harnessId": "h1" }"#).unwrap();
    std::fs::write(
        dir.join("agent-names.json"),
        r#"{ "agents": { "echo": "Echo Agent" } }"#,
    )
    .unwrap();
    let data_dir = dir.to_string_lossy().into_owned();
    let config = config(&[("GRAUWOLF_CHAT_API", "1"), ("GRAUWOLF_DATA_DIR", &data_dir)]).unwrap();
    let spec = AgentSpec::from_json(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let run: RunApp = Arc::new(|_agent, input, options| {
        Box::pin(async move {
            for feature in &options.features {
                feature.on_event(&AgentEvent::LlmCallStarting {
                    model: LlModel::new("openai", "gpt-4o-mini"),
                    tools: vec![],
                });
                feature.on_event(&AgentEvent::LlmCallCompleted {
                    answer: Message::assistant("Hi"),
                    usage: None,
                    cost: None,
                    model: Some("gpt-4o-mini-2024-07-18".into()),
                });
            }
            Ok(input)
        })
    });
    let agent = Arc::new(
        GeneratedAgent::new(&spec, provisioned(&spec, &config).await, run)
            .observed_by(config.chat_api.as_ref().unwrap(), &spec.name),
    );
    let router = grauwolf_server::service::router(agent, &spec, &config)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();

    let answer: Value = client
        .post(format!("{base}/api/chat"))
        .json(&json!({ "message": "hello" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let signature: Value = client
        .get(format!("{base}/api/signature"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(answer, json!({ "response": "hello" }));
    assert_eq!(signature["signature"], json!({ "harnessId": "h1" }));
    assert_eq!(
        signature["observations"]["models"]["Echo Agent"][0]["modelId"],
        "gpt-4o-mini-2024-07-18"
    );
}

/// The provisioning hands the observation of sub-agents to the loader: a feature per spec that
/// records the sub-agent's model under its own (mapped) name.
#[tokio::test]
async fn observes_sub_agents_under_their_own_names() {
    use grauwolf_agents::events::AgentEvent;
    use grauwolf_interpreter::RunExtras;
    use grauwolf_prompt::{LlModel, Message};
    use grauwolf_server::ServedAgent;

    let dir = grauwolf_test::TestDir::new("service-sub-agents");
    std::fs::write(
        dir.join("agent-names.json"),
        r#"{ "agents": { "translator": "Translator Agent" } }"#,
    )
    .unwrap();
    let data_dir = dir.to_string_lossy().into_owned();
    let config = config(&[("GRAUWOLF_CHAT_API", "1"), ("GRAUWOLF_DATA_DIR", &data_dir)]).unwrap();
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs");
    let load = |file: &str| {
        AgentSpec::from_json(&std::fs::read_to_string(examples.join(file)).unwrap()).unwrap()
    };
    let (main, sub) = (load("coordinator.json"), load("translator.json"));
    let sub_for_run = sub.clone();
    let chat_api = config.chat_api.clone().unwrap();
    let features = provision_options(Some(&chat_api))
        .agent_features()
        .cloned()
        .unwrap();
    let run: RunApp = Arc::new(move |_agent, input, _options| {
        let sub = sub_for_run.clone();
        let features = Arc::clone(&features);
        Box::pin(async move {
            // What the loader does for each sub-agent it loads.
            for feature in features(&sub) {
                feature.on_event(&AgentEvent::LlmCallStarting {
                    model: LlModel::new("ollama", "llama3.2"),
                    tools: vec![],
                });
                feature.on_event(&AgentEvent::LlmCallCompleted {
                    answer: Message::assistant("Hi"),
                    usage: None,
                    cost: None,
                    model: Some("llama3.2-sub".into()),
                });
            }
            Ok(input)
        })
    });
    // The sub-agents are not loaded here; the feature factory is what is under test.
    let mut plain = main.clone();
    plain.agent_tools.clear();
    let agent = GeneratedAgent::new(&main, provisioned(&plain, &config).await, run)
        .observed_by(&chat_api, &main.name);

    agent
        .run(json!("hello"), RunExtras::default(), None)
        .await
        .unwrap();

    let report = chat_api.report().await;
    assert_eq!(
        report["models"]["Translator Agent"][0]["modelId"],
        "llama3.2-sub"
    );
}

/// Reads SSE blocks until one with `event: <name>` arrives, and returns its data.
async fn next_event(response: &mut reqwest::Response, buffer: &mut String, name: &str) -> String {
    let marker = format!("event: {name}\n");
    loop {
        if let Some(start) = buffer.find(&marker)
            && let Some(end) = buffer[start..].find("\n\n")
        {
            let block = buffer[start..start + end].to_owned();
            buffer.replace_range(..start + end + 2, "");
            return block
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .collect::<Vec<_>>()
                .join("\n");
        }
        let chunk = response.chunk().await.unwrap().expect("stream ended early");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    }
}

/// ADR-0119: what the request brings – conversation, attachments – reaches the run options.
#[tokio::test]
async fn a_generated_service_hands_conversation_and_attachments_to_the_run() {
    let run: RunApp = Arc::new(|_agent, _input, options| {
        Box::pin(async move {
            Ok(json!({
                "conversation": options.conversation.as_ref().map(|(_, id)| id.clone()),
                "attachments": options.attachments.len(),
                "approver": options.approver.is_some(),
            }))
        })
    });
    let base = serve_run(&config(&[("GRAUWOLF_DATA_DIR", "/data")]).unwrap(), run).await;
    let client = reqwest::Client::new();
    let post = |body: Value| client.post(format!("{base}/run")).json(&body).send();

    let plain: Value = post(json!({ "input": "hi" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let kept: Value = post(json!({
        "input": "hi", "conversation_id": "chat-1", "user_id": "anna",
        "attachments": [ { "media_type": "image/png", "url": "https://example.com/a.png" } ]
    }))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(plain["output"]["conversation"], Value::Null);
    assert_eq!(plain["output"]["attachments"], 0);
    let key = kept["output"]["conversation"].as_str().unwrap();
    assert!(
        key.starts_with("users/") && key.ends_with("/chat-1"),
        "{key}"
    );
    assert_eq!(kept["output"]["attachments"], 1);
    assert_eq!(
        kept["output"]["approver"], false,
        "/run has no back channel"
    );
}

/// ADR-0119: `/run/stream` gives the run its frame sink (text deltas) and its approver.
#[tokio::test]
async fn a_generated_service_streams_text_deltas_and_asks_for_approvals() {
    let run: RunApp = Arc::new(|_agent, _input, options| {
        Box::pin(async move {
            let sink = options.frame_sink.ok_or("no frame sink")?;
            sink(&grauwolf_llm::StreamFrame::TextDelta { text: "Hel".into() });
            sink(&grauwolf_llm::StreamFrame::TextDelta { text: "lo".into() });
            let approver = options.approver.ok_or("no approver")?;
            let call = grauwolf_prompt::ToolCall {
                id: "1".into(),
                tool: "run_command".into(),
                args: json!({ "command": "ls" }),
            };
            Ok(json!(match approver(&call).await {
                grauwolf_agents::events::ToolDecision::Proceed => "approved".to_owned(),
                grauwolf_agents::events::ToolDecision::Deny(reason) => format!("denied: {reason}"),
            }))
        })
    });
    let base = serve_run(&config(&[("GRAUWOLF_DATA_DIR", "/data")]).unwrap(), run).await;
    let client = reqwest::Client::new();
    let mut stream = client
        .post(format!("{base}/run/stream"))
        .json(&json!({ "input": "hi" }))
        .send()
        .await
        .unwrap();
    let mut buffer = String::new();

    let first = next_event(&mut stream, &mut buffer, "text").await;
    let second = next_event(&mut stream, &mut buffer, "text").await;
    let approval: Value =
        serde_json::from_str(&next_event(&mut stream, &mut buffer, "approval").await).unwrap();
    let id = approval["id"].as_str().unwrap();
    let decided = client
        .post(format!("{base}/approvals/{id}"))
        .json(&json!({ "approved": true }))
        .send()
        .await
        .unwrap();
    let output = next_event(&mut stream, &mut buffer, "output").await;

    assert_eq!((first.as_str(), second.as_str()), ("Hel", "lo"));
    assert_eq!(approval["tool"], "run_command");
    assert_eq!(decided.status(), 204);
    assert_eq!(output, "\"approved\"");
}

/// A run that remembers: it appends the input to its conversation and answers with the number of
/// messages the conversation holds now.
fn counting_run() -> RunApp {
    Arc::new(|_agent, input, options| {
        Box::pin(async move {
            let (store, key) = options.conversation.ok_or("no conversation")?;
            let mut messages = store.load(&key).await.map_err(|error| error.to_string())?;
            messages.push(grauwolf_prompt::Message::User {
                content: input.as_str().unwrap_or_default().to_owned(),
                attachments: Vec::new(),
            });
            store
                .save(&key, &messages)
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!(messages.len()))
        })
    })
}

async fn ask(base: &str, conversation_id: &str) -> u64 {
    let answer: Value = reqwest::Client::new()
        .post(format!("{base}/run"))
        .json(&json!({ "input": "hi", "conversation_id": conversation_id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    answer["output"].as_u64().unwrap()
}

/// ADR-0119: with `GRAUWOLF_CONVERSATIONS_DIR` (relative to the data directory) conversations
/// outlive a service instance; without it they live in the memory of the instance.
#[tokio::test]
async fn conversations_survive_a_restart_with_a_conversations_directory() {
    let data = grauwolf_test::TestDir::new("conv");
    let data_dir = data.to_str().unwrap();
    let persistent = config(&[
        ("GRAUWOLF_DATA_DIR", data_dir),
        ("GRAUWOLF_CONVERSATIONS_DIR", "chats"),
    ])
    .unwrap();

    let first = serve_run(&persistent, counting_run()).await;
    assert_eq!(ask(&first, "c1").await, 1);
    assert_eq!(ask(&first, "c1").await, 2);
    let second = serve_run(&persistent, counting_run()).await;
    assert_eq!(ask(&second, "c1").await, 3, "the new instance remembers");
    assert_eq!(ask(&second, "c2").await, 1);
    assert!(data.join("chats").is_dir());

    let in_memory = config(&[("GRAUWOLF_DATA_DIR", data_dir)]).unwrap();
    let third = serve_run(&in_memory, counting_run()).await;
    assert_eq!(ask(&third, "c1").await, 1, "in memory: nothing from before");
    let fourth = serve_run(&in_memory, counting_run()).await;
    assert_eq!(ask(&fourth, "c1").await, 1);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn refuses_a_conversations_directory_together_with_a_database() {
    let error = config(&[
        ("GRAUWOLF_CONVERSATIONS_DIR", "chats"),
        ("GRAUWOLF_CONVERSATIONS_DB", "sqlite://secret-chats.db"),
    ])
    .err()
    .expect("both are set");

    assert!(error.contains("GRAUWOLF_CONVERSATIONS_DIR"), "{error}");
    assert!(error.contains("GRAUWOLF_CONVERSATIONS_DB"), "{error}");
    assert!(!error.contains("secret-chats"), "{error}");
}

#[test]
fn tracing_is_off_unless_the_environment_switches_it_on() {
    let env = grauwolf_host::config::MapEnv::default();
    let off = config(&[]).unwrap();
    assert!(!off.otel);
    assert!(
        grauwolf_server::service::ServiceTracing::start(&off, &env)
            .unwrap()
            .is_none()
    );
    let error = config(&[("GRAUWOLF_OTEL", "maybe")]).err().unwrap();
    assert!(error.contains("GRAUWOLF_OTEL"), "{error}");
    let on = config(&[("GRAUWOLF_OTEL", "1"), ("GRAUWOLF_OTEL_CONTENT", "true")]).unwrap();
    assert!(on.otel && on.otel_content);
}

/// What `grauwolf_host::features::JsonlEventLog` writes into: shared, so a test can read it.
#[derive(Clone, Default)]
struct SharedBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl SharedBuffer {
    fn lines(&self) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
            .collect()
    }
}

#[test]
fn the_event_log_is_off_unless_the_environment_switches_it_on() {
    assert!(!config(&[]).unwrap().events);
    assert!(!config(&[("GRAUWOLF_EVENTS", "false")]).unwrap().events);
    assert!(config(&[("GRAUWOLF_EVENTS", "1")]).unwrap().events);
    assert!(config(&[("GRAUWOLF_EVENTS", "true")]).unwrap().events);
    let error = config(&[("GRAUWOLF_EVENTS", "maybe")]).err().unwrap();
    assert!(error.contains("GRAUWOLF_EVENTS"), "{error}");
}

/// With `GRAUWOLF_EVENTS` the main agent and every sub-agent report their events to the log, one
/// JSON line each, like `spec serve --events`.
#[tokio::test]
async fn an_event_log_sees_the_events_of_the_agent_and_of_its_sub_agents() {
    use grauwolf_agents::events::AgentEvent;
    use grauwolf_interpreter::RunExtras;
    use grauwolf_server::ServedAgent;

    let buffer = SharedBuffer::default();
    let log: Arc<dyn grauwolf_agents::events::Feature> =
        Arc::new(grauwolf_host::features::JsonlEventLog::new(buffer.clone()));
    let spec = AgentSpec::from_json(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/echo.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let starting = || AgentEvent::AgentStarting {
        input: json!("hi"),
        caller: None,
        trace_parent: None,
        on_behalf_of: None,
    };
    let run: RunApp = Arc::new(move |_agent, input, options| {
        Box::pin(async move {
            for feature in &options.features {
                feature.on_event(&starting());
            }
            Ok(input)
        })
    });
    let agent = GeneratedAgent::new(&spec, provisioned(&spec, &config(&[]).unwrap()).await, run)
        .logged_by(Arc::clone(&log));

    agent
        .run(json!("hi"), RunExtras::default(), None)
        .await
        .unwrap();

    let lines = buffer.lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["event"], "agent_starting");

    let features = grauwolf_server::service::provision_options_logged(
        Arc::new(grauwolf_host::config::MapEnv::default()),
        None,
        None,
        Some(log),
    )
    .agent_features()
    .cloned()
    .expect("a feature factory for the sub-agents");
    for feature in features(&spec) {
        feature.on_event(&starting());
    }
    assert_eq!(buffer.lines().len(), 2);

    // Without the log the sub-agents get no factory at all.
    assert!(
        grauwolf_server::service::provision_options_logged(
            Arc::new(grauwolf_host::config::MapEnv::default()),
            None,
            None,
            None,
        )
        .agent_features()
        .is_none()
    );
}

#[cfg(feature = "otel")]
#[test]
fn tracing_starts_with_the_feature_and_gives_a_feature_per_agent() {
    let env = grauwolf_host::config::MapEnv::new([("OTEL_SERVICE_NAME", "svc")]);
    let on = config(&[("GRAUWOLF_OTEL", "1")]).unwrap();
    let tracing = grauwolf_server::service::ServiceTracing::start(&on, &env)
        .unwrap()
        .expect("tracing is on");
    let _feature = tracing.feature("echo");
    // Nothing was recorded: there is no collector to send to and nothing to wait for.
    drop(tracing);
}

#[cfg(not(feature = "otel"))]
#[test]
fn a_build_without_otel_refuses_the_switch() {
    let env = grauwolf_host::config::MapEnv::default();
    let on = config(&[("GRAUWOLF_OTEL", "1")]).unwrap();
    let error = grauwolf_server::service::ServiceTracing::start(&on, &env)
        .err()
        .unwrap();
    assert!(
        error.contains("GRAUWOLF_OTEL")
            && error.contains("this build has no OpenTelemetry (feature `otel`)"),
        "{error}"
    );
}

#[test]
fn an_identity_service_without_spiffe_is_a_start_error() {
    let error = config(&[("GRAUWOLF_IDENTITY_URL", "https://identity:8443")])
        .err()
        .unwrap();
    assert_eq!(error, "GRAUWOLF_IDENTITY_URL needs GRAUWOLF_SPIFFE");
    assert!(
        config(&[
            ("GRAUWOLF_IDENTITY_URL", "https://identity:8443"),
            ("GRAUWOLF_SPIFFE", "1"),
        ])
        .is_ok()
    );
}
