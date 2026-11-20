//! Conformance suite against the real Bedrock API (opt-in).
//!
//! Runs only with AWS credentials in the environment: `AWS_BEARER_TOKEN_BEDROCK`, or
//! `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` (optionally `AWS_SESSION_TOKEN`). Region from
//! `AWS_REGION` (default `us-west-2`); the model from `GRAUWOLF_BEDROCK_MODEL` (default
//! `us.anthropic.claude-3-5-haiku-20241022-v1:0`, which needs model access in the account).

use grauwolf_bedrock::{BedrockClient, Credentials, DEFAULT_REGION};
use grauwolf_prompt::LlModel;
use grauwolf_test::conformance::check_llm_client;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[tokio::test]
async fn passes_the_conformance_suite_against_the_real_api() {
    let region = var("AWS_REGION").unwrap_or_else(|| DEFAULT_REGION.into());
    let client = match (
        var("AWS_BEARER_TOKEN_BEDROCK"),
        var("AWS_ACCESS_KEY_ID"),
        var("AWS_SECRET_ACCESS_KEY"),
    ) {
        (Some(token), _, _) => BedrockClient::with_api_key(token, region),
        (None, Some(access_key_id), Some(secret_access_key)) => BedrockClient::new(
            Credentials {
                access_key_id,
                secret_access_key,
                session_token: var("AWS_SESSION_TOKEN"),
            },
            region,
        ),
        _ => {
            eprintln!("skipped: no AWS credentials in the environment");
            return;
        }
    };
    let model = var("GRAUWOLF_BEDROCK_MODEL")
        .unwrap_or_else(|| "us.anthropic.claude-3-5-haiku-20241022-v1:0".into());

    let failures = check_llm_client(&client, &LlModel::new("bedrock", model)).await;

    assert!(failures.is_empty(), "{failures:#?}");
}
