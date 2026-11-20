//! Costs from token usage and price table (ADR-0071).

use grauwolf_prompt::{LlModel, Prices, Usage};

fn prices() -> Prices {
    serde_json::from_str(
        r#"{ "currency": "USD", "models": {
            "anthropic:claude-haiku-5-5": { "input": 0.10, "output": 0.50,
                                            "cache_read": 0.01, "cache_write": 0.125 },
            "ollama:qwen3.5:9b": { "input": 0, "output": 0 },
            "openai:gpt-mini": { "input": 1.0, "output": 2.0 }
        } }"#,
    )
    .unwrap()
}

#[test]
fn prices_uncached_input_cache_reads_cache_writes_and_output_separately() {
    let usage = Usage {
        input_tokens: 3538,
        output_tokens: 11,
        cache_read_tokens: Some(3495),
        cache_write_tokens: Some(39),
    };

    let cost = prices()
        .cost(&LlModel::new("anthropic", "claude-haiku-5-5"), &usage)
        .unwrap();

    // 4 uncached × 0.10 + 3495 × 0.01 + 39 × 0.125 + 11 × 0.50 – per million tokens each.
    let expected = (4.0 * 0.10 + 3495.0 * 0.01 + 39.0 * 0.125 + 11.0 * 0.50) / 1e6;
    assert!((cost - expected).abs() < 1e-12, "{cost} ≠ {expected}");
    assert_eq!(prices().currency, "USD");
}

#[test]
fn uses_the_input_price_for_cache_tokens_without_own_price_and_knows_no_unknown_models() {
    let usage = Usage {
        input_tokens: 1000,
        output_tokens: 100,
        cache_read_tokens: Some(400),
        cache_write_tokens: None,
    };

    let openai = prices()
        .cost(&LlModel::new("openai", "gpt-mini"), &usage)
        .unwrap();
    let unknown = prices().cost(&LlModel::new("openai", "gpt-unknown"), &usage);

    assert!((openai - (1000.0 * 1.0 + 100.0 * 2.0) / 1e6).abs() < 1e-12);
    assert_eq!(unknown, None);
}
