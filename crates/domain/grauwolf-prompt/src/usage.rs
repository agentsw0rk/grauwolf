//! Token usage of an LLM call (ADR-0067).

use serde::{Deserialize, Serialize};

/// Tokens consumed by a call; counted the same way for all providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// All input tokens, including cached ones.
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
    /// Of these, read from the prompt cache, if the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Of these, written to the prompt cache, if the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

/// Sum of two usages; a cache field stays unknown only if it is unknown in both.
impl std::ops::Add for Usage {
    type Output = Usage;

    fn add(self, other: Usage) -> Usage {
        let sum = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        Usage {
            input_tokens: self.input_tokens + other.input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            cache_read_tokens: sum(self.cache_read_tokens, other.cache_read_tokens),
            cache_write_tokens: sum(self.cache_write_tokens, other.cache_write_tokens),
        }
    }
}

/// Prices of a model per million tokens (ADR-0071).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    /// Uncached input.
    pub input: f64,
    /// Output.
    pub output: f64,
    /// Input read from the cache; without a value, the input price applies.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Input written to the cache; without a value, the input price applies.
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// Price table: currency and prices per `<provider>:<id>` (ADR-0071).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prices {
    /// Currency of all prices, e.g. `USD`.
    pub currency: String,
    /// Prices per model as `<provider>:<id>`.
    pub models: std::collections::BTreeMap<String, ModelPrice>,
}

impl Prices {
    /// Cost of `usage` with `model`; `None` if the table does not know the model.
    pub fn cost(&self, model: &crate::LlModel, usage: &Usage) -> Option<f64> {
        let price = self
            .models
            .get(&format!("{}:{}", model.provider, model.id))?;
        let read = usage.cache_read_tokens.unwrap_or(0);
        let write = usage.cache_write_tokens.unwrap_or(0);
        let uncached = usage.input_tokens.saturating_sub(read + write);
        let per_token = |tokens: u64, price: f64| tokens as f64 * price;
        let total = per_token(uncached, price.input)
            + per_token(read, price.cache_read.unwrap_or(price.input))
            + per_token(write, price.cache_write.unwrap_or(price.input))
            + per_token(usage.output_tokens, price.output);
        Some(total / 1e6)
    }
}
