//! Conformance suite against the real OpenAI API or a compatible gateway (opt-in).
//!
//! Runs only when `GRAUWOLF_OPENAI_API_KEY` is set; optionally `GRAUWOLF_OPENAI_BASE_URL`
//! (e.g. `http://localhost:11434/v1` for Ollama's compatible API) and `GRAUWOLF_OPENAI_MODEL`.

use grauwolf_openai::OpenAiClient;
use grauwolf_prompt::LlModel;
use grauwolf_test::conformance::check_llm_client;

#[tokio::test]
async fn passes_the_conformance_suite_against_a_real_api() {
    let Ok(api_key) = std::env::var("GRAUWOLF_OPENAI_API_KEY") else {
        eprintln!("skipped: GRAUWOLF_OPENAI_API_KEY not set");
        return;
    };
    let mut client = OpenAiClient::new(api_key);
    if let Ok(base_url) = std::env::var("GRAUWOLF_OPENAI_BASE_URL") {
        client = client.with_base_url(base_url);
    }
    let model = std::env::var("GRAUWOLF_OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());

    let failures = check_llm_client(&client, &LlModel::new("openai", model)).await;

    assert!(failures.is_empty(), "{failures:#?}");
}
