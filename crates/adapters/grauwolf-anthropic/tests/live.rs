//! Conformance suite against the real Anthropic API (opt-in).
//!
//! Runs only when `GRAUWOLF_ANTHROPIC_API_KEY` is set; optionally `GRAUWOLF_ANTHROPIC_MODEL`.

use grauwolf_anthropic::AnthropicClient;
use grauwolf_prompt::LlModel;
use grauwolf_test::conformance::check_llm_client;

#[tokio::test]
async fn passes_the_conformance_suite_against_the_real_api() {
    let Ok(api_key) = std::env::var("GRAUWOLF_ANTHROPIC_API_KEY") else {
        eprintln!("skipped: GRAUWOLF_ANTHROPIC_API_KEY not set");
        return;
    };
    let model =
        std::env::var("GRAUWOLF_ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-opus-5-5".into());
    let client = AnthropicClient::new(api_key);

    let failures = check_llm_client(&client, &LlModel::new("anthropic", model)).await;

    assert!(failures.is_empty(), "{failures:#?}");
}
