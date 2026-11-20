//! Conformance suite against the real Gemini API (opt-in).
//!
//! Runs only with `GRAUWOLF_GEMINI_API_KEY` set; optionally `GRAUWOLF_GEMINI_MODEL`.

use grauwolf_gemini::GeminiClient;
use grauwolf_prompt::LlModel;
use grauwolf_test::conformance::check_llm_client;

#[tokio::test]
async fn passes_the_conformance_suite_against_the_real_api() {
    let Ok(api_key) = std::env::var("GRAUWOLF_GEMINI_API_KEY") else {
        eprintln!("skipped: GRAUWOLF_GEMINI_API_KEY not set");
        return;
    };
    let model =
        std::env::var("GRAUWOLF_GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
    let client = GeminiClient::new(api_key);

    let failures = check_llm_client(&client, &LlModel::new("gemini", model)).await;

    assert!(failures.is_empty(), "{failures:#?}");
}
