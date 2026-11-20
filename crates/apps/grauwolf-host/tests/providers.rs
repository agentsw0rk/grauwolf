//! Provider clients from environment variables (F11.8).

use std::collections::HashMap;

use grauwolf_host::providers::{ProviderError, client_from};
use grauwolf_prompt::{LlModel, Message, Prompt};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    move |key| vars.get(key).cloned()
}

#[test]
fn rejects_unknown_providers() {
    let error = client_from("unknown", env(&[])).err().unwrap();

    assert!(matches!(error, ProviderError::Unknown { .. }), "{error}");
}

#[test]
fn names_the_missing_api_key() {
    let error = client_from("anthropic", env(&[])).err().unwrap();

    assert_eq!(
        error.to_string(),
        "provider 'anthropic' needs the environment variable ANTHROPIC_API_KEY"
    );
}

#[tokio::test]
async fn reaches_ollama_at_ollama_host() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "Hi" }, "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_from("ollama", env(&[("OLLAMA_HOST", &server.uri())])).unwrap();
    let prompt = Prompt::builder("p").user("Hello").build();

    let answer = client
        .execute(&prompt, &LlModel::new("ollama", "m"), &[])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
}

#[tokio::test]
async fn retries_transient_failures_through_the_executor() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "Hi" }, "done": true
        })))
        .mount(&server)
        .await;
    let executor =
        grauwolf_host::providers::executor_from("ollama", env(&[("OLLAMA_HOST", &server.uri())]))
            .unwrap();
    let prompt = Prompt::builder("p").user("Hello").build();

    let answer = executor
        .execute(&prompt, &LlModel::new("ollama", "m"), &[])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
}

#[tokio::test]
async fn passes_ollama_think_from_the_environment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_partial_json(
            json!({ "think": false }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "Hi" }, "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_from(
        "ollama",
        env(&[("OLLAMA_HOST", &server.uri()), ("OLLAMA_THINK", "false")]),
    )
    .unwrap();
    let prompt = Prompt::builder("p").user("Hello").build();

    let answer = client
        .execute(&prompt, &LlModel::new("ollama", "m"), &[])
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("Hi"));
}

#[tokio::test]
async fn embeds_with_ollama_at_ollama_host() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "embeddings": [[1.0, 0.0]]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let embedder = grauwolf_host::providers::embedder_from(
        "ollama",
        "nomic-embed-text",
        env(&[("OLLAMA_HOST", &server.uri())]),
    )
    .unwrap();

    let embeddings = embedder.embed(&["cat".to_owned()]).await.unwrap();

    assert_eq!(embeddings, [grauwolf_rag::Embedding(vec![1.0, 0.0])]);
}

#[tokio::test]
async fn builds_one_executor_with_clients_for_several_providers() {
    let ollama = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "from Ollama" }, "done": true
        })))
        .mount(&ollama)
        .await;
    let openai = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [ { "index": 0, "message": { "role": "assistant", "content": "from OpenAI" } } ]
        })))
        .mount(&openai)
        .await;
    let executor = grauwolf_host::providers::executor_for(
        ["ollama", "openai"],
        env(&[
            ("OLLAMA_HOST", &ollama.uri()),
            ("OPENAI_API_KEY", "sk-test"),
            ("OPENAI_BASE_URL", &openai.uri()),
        ]),
    )
    .unwrap();
    let prompt = Prompt::builder("p").user("Hello").build();

    let from_ollama = executor
        .execute(&prompt, &LlModel::new("ollama", "m"), &[])
        .await
        .unwrap();
    let from_openai = executor
        .execute(&prompt, &LlModel::new("openai", "m"), &[])
        .await
        .unwrap();
    let missing = grauwolf_host::providers::executor_for(["ollama", "anthropic"], env(&[]))
        .err()
        .unwrap();

    assert_eq!(from_ollama, Message::assistant("from Ollama"));
    assert_eq!(from_openai, Message::assistant("from OpenAI"));
    assert!(
        matches!(missing, ProviderError::MissingVariable { .. }),
        "{missing}"
    );
}

#[tokio::test]
async fn embeds_with_openai_using_key_and_base_url_from_the_environment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer sk-test",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [ { "index": 0, "embedding": [0.5, 0.5] } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let embedder = grauwolf_host::providers::embedder_from(
        "openai",
        "text-embedding-3-small",
        env(&[
            ("OPENAI_API_KEY", "sk-test"),
            ("OPENAI_BASE_URL", &server.uri()),
        ]),
    )
    .unwrap();
    let missing = grauwolf_host::providers::embedder_from("openai", "m", env(&[]))
        .err()
        .unwrap();

    let embeddings = embedder.embed(&["cat".to_owned()]).await.unwrap();

    assert_eq!(embeddings, [grauwolf_rag::Embedding(vec![0.5, 0.5])]);
    assert!(
        matches!(missing, ProviderError::MissingVariable { .. }),
        "{missing}"
    );
}

#[tokio::test]
async fn reaches_openai_compatible_providers_with_their_own_key_and_address() {
    for (provider, key, base_url) in [
        ("openrouter", "OPENROUTER_API_KEY", "OPENROUTER_BASE_URL"),
        ("deepseek", "DEEPSEEK_API_KEY", "DEEPSEEK_BASE_URL"),
        ("mistral", "MISTRAL_API_KEY", "MISTRAL_BASE_URL"),
        ("groq", "GROQ_API_KEY", "GROQ_BASE_URL"),
        ("dashscope", "DASHSCOPE_API_KEY", "DASHSCOPE_BASE_URL"),
        ("xai", "XAI_API_KEY", "XAI_BASE_URL"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(wiremock::matchers::header(
                "authorization",
                format!("Bearer key-{provider}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [ { "index": 0, "message": { "role": "assistant", "content": provider } } ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_from(
            provider,
            env(&[
                (key, &format!("key-{provider}")),
                (base_url, &server.uri()),
                // The variables of `openai` do not apply to the other providers.
                ("OPENAI_API_KEY", "sk-wrong"),
                ("OPENAI_BASE_URL", "http://127.0.0.1:9"),
            ]),
        )
        .unwrap();
        let missing = client_from(provider, env(&[])).err().unwrap();

        let answer = client
            .execute(
                &Prompt::builder("p").user("Hello").build(),
                &LlModel::new(provider, "m"),
                &[],
            )
            .await
            .unwrap();

        assert_eq!(answer, Message::assistant(provider));
        assert_eq!(
            missing.to_string(),
            format!("provider '{provider}' needs the environment variable {key}")
        );
    }
}

#[tokio::test]
async fn embeds_with_openai_compatible_providers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer key-mistral",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [ { "index": 0, "embedding": [0.25, 0.75] } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let embedder = grauwolf_host::providers::embedder_from(
        "mistral",
        "mistral-embed",
        env(&[
            ("MISTRAL_API_KEY", "key-mistral"),
            ("MISTRAL_BASE_URL", &server.uri()),
        ]),
    )
    .unwrap();
    let missing = grauwolf_host::providers::embedder_from("mistral", "mistral-embed", env(&[]))
        .err()
        .unwrap();

    let embeddings = embedder.embed(&["Wolf".to_owned()]).await.unwrap();

    assert_eq!(embeddings, [grauwolf_rag::Embedding(vec![0.25, 0.75])]);
    assert_eq!(
        missing.to_string(),
        "provider 'mistral' needs the environment variable MISTRAL_API_KEY"
    );
}

#[tokio::test]
async fn reaches_gemini_with_key_and_address_from_the_environment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-3-flash:generateContent"))
        .and(wiremock::matchers::header("x-goog-api-key", "g-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [ { "content": { "role": "model", "parts": [ { "text": "from Gemini" } ] }, "finishReason": "STOP" } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_from(
        "gemini",
        env(&[
            ("GEMINI_API_KEY", "g-test"),
            ("GEMINI_BASE_URL", &server.uri()),
        ]),
    )
    .unwrap();
    let missing = client_from("gemini", env(&[])).err().unwrap();

    let answer = client
        .execute(
            &Prompt::builder("p").user("Hello").build(),
            &LlModel::new("gemini", "gemini-3-flash"),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(answer, Message::assistant("from Gemini"));
    assert_eq!(
        missing.to_string(),
        "provider 'gemini' needs the environment variable GEMINI_API_KEY"
    );
}

#[tokio::test]
async fn embeds_with_gemini_using_key_and_address_from_the_environment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-embedding-001:batchEmbedContents",
        ))
        .and(wiremock::matchers::header("x-goog-api-key", "g-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "embeddings": [ { "values": [0.75, 0.25] } ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let embedder = grauwolf_host::providers::embedder_from(
        "gemini",
        "gemini-embedding-001",
        env(&[
            ("GEMINI_API_KEY", "g-test"),
            ("GEMINI_BASE_URL", &server.uri()),
        ]),
    )
    .unwrap();
    let missing =
        grauwolf_host::providers::embedder_from("gemini", "gemini-embedding-001", env(&[]))
            .err()
            .unwrap();

    let embeddings = embedder.embed(&["Wolf".to_owned()]).await.unwrap();

    assert_eq!(embeddings, [grauwolf_rag::Embedding(vec![0.75, 0.25])]);
    assert_eq!(
        missing.to_string(),
        "provider 'gemini' needs the environment variable GEMINI_API_KEY"
    );
}

/// `OLLAMA_BASE_URL` stands in for a missing `OLLAMA_HOST` only in chat API mode, and
/// `OLLAMA_HOST` always wins (ADR-0094).
#[test]
fn ollama_base_url_stands_in_only_in_chat_api_mode() {
    use grauwolf_host::config::{MapEnv, ProvidersConfig};
    let only_base = MapEnv::new([("OLLAMA_BASE_URL", "http://base:1")]);
    let both = MapEnv::new([
        ("OLLAMA_BASE_URL", "http://base:1"),
        ("OLLAMA_HOST", "host:2"),
    ]);
    let plain = ProvidersConfig::from_env(&only_base);
    assert_eq!(
        grauwolf_host::providers::ollama_host(&plain),
        "http://localhost:11434"
    );
    assert_eq!(
        grauwolf_host::providers::ollama_host(&plain.with_chat_api_mode(true)),
        "http://base:1"
    );
    let chat = ProvidersConfig::from_env(&both).with_chat_api_mode(true);
    assert_eq!(
        grauwolf_host::providers::ollama_host(&chat),
        "http://host:2"
    );
}

fn bedrock_answer(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "output": { "message": { "role": "assistant", "content": [ { "text": text } ] } },
        "stopReason": "end_turn"
    }))
}

async fn ask_bedrock(client: std::sync::Arc<dyn grauwolf_llm::LlmClient>) -> Message {
    client
        .execute(
            &Prompt::builder("p").user("Hello").build(),
            &LlModel::new("bedrock", "eu.anthropic.claude-3-haiku-20240307-v1:0"),
            &[],
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn reaches_bedrock_signed_with_access_keys_in_the_region_from_the_environment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/eu.anthropic.claude-3-haiku-20240307-v1%3A0/converse"))
        .and(wiremock::matchers::header_regex(
            "authorization",
            r"Credential=AKID/\d{8}/eu-north-1/bedrock/aws4_request, SignedHeaders=host;x-amz-date;x-amz-security-token,",
        ))
        .and(wiremock::matchers::header("x-amz-security-token", "tok"))
        .respond_with(bedrock_answer("from Bedrock"))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_from(
        "bedrock",
        env(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_SESSION_TOKEN", "tok"),
            ("AWS_DEFAULT_REGION", "eu-north-1"),
            ("BEDROCK_BASE_URL", &server.uri()),
        ]),
    )
    .unwrap();

    assert_eq!(
        ask_bedrock(client).await,
        Message::assistant("from Bedrock")
    );
}

#[tokio::test]
async fn bedrock_uses_the_api_key_in_preference_to_access_keys() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer api-key",
        ))
        .respond_with(bedrock_answer("from Bedrock"))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_from(
        "bedrock",
        env(&[
            ("AWS_BEARER_TOKEN_BEDROCK", "api-key"),
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("BEDROCK_BASE_URL", &server.uri()),
        ]),
    )
    .unwrap();

    ask_bedrock(client).await;
}

#[test]
fn bedrock_names_the_missing_credentials() {
    let none = client_from("bedrock", env(&[])).err().unwrap();
    let half = client_from("bedrock", env(&[("AWS_ACCESS_KEY_ID", "AKID")]))
        .err()
        .unwrap();

    assert_eq!(
        none.to_string(),
        "provider 'bedrock' needs the environment variable AWS_BEARER_TOKEN_BEDROCK, or AWS_ACCESS_KEY_ID with AWS_SECRET_ACCESS_KEY"
    );
    assert_eq!(
        half.to_string(),
        "provider 'bedrock' needs the environment variable AWS_SECRET_ACCESS_KEY"
    );
}

#[test]
fn bedrock_has_no_embedding_models_yet() {
    let error = grauwolf_host::providers::embedder_from(
        "bedrock",
        "amazon.titan-embed-text-v2:0",
        env(&[("AWS_BEARER_TOKEN_BEDROCK", "k")]),
    )
    .err()
    .unwrap();

    assert!(
        matches!(error, ProviderError::NoEmbeddings { .. }),
        "{error}"
    );
}
