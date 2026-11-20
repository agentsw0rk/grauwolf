//! Conformance suite against a real Ollama server (opt-in).
//!
//! Runs only when `GRAUWOLF_OLLAMA_URL` is set, e.g.
//! `GRAUWOLF_OLLAMA_URL=http://localhost:11434 GRAUWOLF_OLLAMA_MODEL=qwen3.5:9b cargo test -p grauwolf-ollama --test live`.

use grauwolf_ollama::OllamaClient;
use grauwolf_prompt::LlModel;
use grauwolf_test::conformance::check_llm_client;

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
