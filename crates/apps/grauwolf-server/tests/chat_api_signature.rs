//! Observations and files of chat API mode (ADR-0094).

use grauwolf_server::chat_api::ChatApi;
use serde_json::json;

fn scratch(name: &str) -> grauwolf_test::TestDir {
    grauwolf_test::TestDir::new(&format!("chat_api-{name}"))
}

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn reads_the_signature_from_the_base_directory() {
    let dir = scratch("signature");
    std::fs::write(dir.join("agent-signature.json"), r#"{ "harnessId": "h1" }"#).unwrap();

    let chat_api = ChatApi::from_dir(&dir, no_env);

    assert_eq!(chat_api.signature(), Some(&json!({ "harnessId": "h1" })));
}

#[test]
fn takes_the_signature_from_agent_signature_path_when_set() {
    let dir = scratch("path");
    std::fs::write(
        dir.join("agent-signature.json"),
        r#"{ "harnessId": "base" }"#,
    )
    .unwrap();
    let elsewhere = dir.join("elsewhere.json");
    std::fs::write(&elsewhere, r#"{ "harnessId": "env" }"#).unwrap();
    let path = elsewhere.to_string_lossy().into_owned();

    let chat_api = ChatApi::from_dir(&dir, |name| {
        (name == "AGENT_SIGNATURE_PATH").then(|| path.clone())
    });

    assert_eq!(chat_api.signature(), Some(&json!({ "harnessId": "env" })));
}

#[test]
fn a_missing_or_unreadable_signature_is_null() {
    let missing = scratch("missing");
    let broken = scratch("broken");
    std::fs::write(broken.join("agent-signature.json"), "{ not json").unwrap();

    assert_eq!(ChatApi::from_dir(&missing, no_env).signature(), None);
    assert_eq!(ChatApi::from_dir(&broken, no_env).signature(), None);
}

/// The names of `agent-names.json` are used for the models per agent.
#[tokio::test]
async fn maps_spec_names_to_agent_names() {
    let dir = scratch("names");
    std::fs::write(
        dir.join("agent-names.json"),
        r#"{ "agents": { "support": "Support Agent" } }"#,
    )
    .unwrap();
    let chat_api = ChatApi::from_dir(&dir, no_env);

    let feature = chat_api.observations().feature_for("support");
    feature.on_event(&grauwolf_agents::events::AgentEvent::LlmCallStarting {
        model: grauwolf_prompt::LlModel::new("openai", "m"),
        tools: vec![],
    });
    feature.on_event(&grauwolf_agents::events::AgentEvent::LlmCallCompleted {
        answer: grauwolf_prompt::Message::assistant("Hi"),
        usage: None,
        cost: None,
        model: Some("m-1".into()),
    });

    let report = chat_api.report().await;
    assert_eq!(report["models"]["Support Agent"][0]["modelId"], "m-1");
}

/// A streamed run reports the model of the provider's stream like any other run.
#[tokio::test]
async fn a_streamed_run_records_the_model_the_provider_reported() {
    use std::sync::Arc;

    use futures_util::StreamExt;
    use futures_util::stream::BoxStream;
    use grauwolf_agents::AgentContext;
    use grauwolf_llm::{LlmClient, LlmError, PromptExecutor, StreamFrame};
    use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};

    /// Streams an answer and names a dated model version in the stream.
    struct Streaming;
    impl LlmClient for Streaming {
        fn execute<'a>(
            &'a self,
            _prompt: &'a Prompt,
            _model: &'a LlModel,
            _tools: &'a [ToolDescriptor],
        ) -> futures_util::future::BoxFuture<'a, Result<Message, LlmError>> {
            Box::pin(async { Ok(Message::assistant("Hi")) })
        }

        fn execute_streaming<'a>(
            &'a self,
            _prompt: &'a Prompt,
            _model: &'a LlModel,
            _tools: &'a [ToolDescriptor],
        ) -> BoxStream<'a, Result<StreamFrame, LlmError>> {
            futures_util::stream::iter([
                Ok(StreamFrame::Model("m-2026-11-01".into())),
                Ok(StreamFrame::TextDelta { text: "Hi".into() }),
                Ok(StreamFrame::End {
                    finish_reason: None,
                }),
            ])
            .boxed()
        }
    }

    let chat_api = ChatApi::from_dir(&scratch("streamed"), no_env);
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(Streaming))
        .build();
    let mut ctx = AgentContext::new(
        Arc::new(executor),
        LlModel::new("mock", "m"),
        Prompt::builder("p").user("Hello").build(),
    )
    .install(chat_api.observations().feature_for("agent"));

    ctx.request_llm_streaming().await.unwrap();

    let report = chat_api.report().await;
    assert_eq!(report["models"]["agent"][0]["modelId"], "m-2026-11-01");
}
