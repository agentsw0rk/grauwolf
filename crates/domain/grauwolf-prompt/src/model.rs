//! Selection of an LLM.

use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// An LLM, identified by provider and model ID, with optional sampling settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlModel {
    /// Provider, e.g. `openai`; selects the client in the executor.
    pub provider: String,
    /// Model ID at the provider, e.g. `gpt-4o`.
    pub id: String,
    /// Fallback models, asked in turn when a request fails (ADR-0088).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<LlModel>,
    /// Sampling temperature (0–2); `None` keeps the provider default (ADR-0097).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Reasoning switch of Ollama models; takes precedence over `OLLAMA_THINK` (ADR-0097).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
}

// Temperatures are finite numbers set by the user, never NaN, so equality is total.
impl Eq for LlModel {}

impl Hash for LlModel {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.provider.hash(state);
        self.id.hash(state);
        self.fallbacks.hash(state);
        self.temperature.map(f64::to_bits).hash(state);
        self.think.hash(state);
    }
}

impl LlModel {
    /// Model `id` of the provider `provider`.
    pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            id: id.into(),
            fallbacks: Vec::new(),
            temperature: None,
            think: None,
        }
    }

    /// The same model with the fallback models `fallbacks` (ADR-0088).
    pub fn with_fallbacks(self, fallbacks: Vec<LlModel>) -> Self {
        Self { fallbacks, ..self }
    }

    /// The same model with the sampling temperature `temperature` (ADR-0097).
    pub fn with_temperature(self, temperature: f64) -> Self {
        Self {
            temperature: Some(temperature),
            ..self
        }
    }

    /// The same model with Ollama's `think` switched to `think` (ADR-0097).
    pub fn with_think(self, think: bool) -> Self {
        Self {
            think: Some(think),
            ..self
        }
    }

    /// Provider, ID, temperature and think without fallback models, e.g. for messages.
    pub fn without_fallbacks(&self) -> Self {
        Self {
            fallbacks: Vec::new(),
            ..self.clone()
        }
    }
}
