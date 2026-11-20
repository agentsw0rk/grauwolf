//! Provider clients from the environment, named like `model.provider` in specs.
//!
//! | Provider    | Variables                                              |
//! |-------------|--------------------------------------------------------|
//! | `ollama`    | `OLLAMA_HOST` (optional, default `http://localhost:11434`), `OLLAMA_THINK` (optional `true`/`false`) |
//! | `openai`    | `OPENAI_API_KEY`, `OPENAI_BASE_URL` (optional)         |
//! | `anthropic` | `ANTHROPIC_API_KEY`                                    |
//! | `gemini`    | `GEMINI_API_KEY`, `GEMINI_BASE_URL` (optional)         |
//! | `bedrock`   | `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` (+ `AWS_SESSION_TOKEN`) or `AWS_BEARER_TOKEN_BEDROCK` (wins); `AWS_REGION`/`AWS_DEFAULT_REGION` (default `us-west-2`), `BEDROCK_BASE_URL` (optional) |
//!
//! In addition, OpenAI-compatible providers with their own name (ADR-0082), see [`OPENAI_COMPATIBLE`]:
//! key in `<NAME>_API_KEY`, another address optionally in `<NAME>_BASE_URL`.

use std::sync::Arc;

use grauwolf_llm::{LlmClient, PromptExecutor, PromptExecutorBuilder, RetryPolicy};
use grauwolf_rag::Embedder;

use crate::config::{ProcessEnv, ProvidersConfig};
use crate::runtime::TokioClock;

/// A provider cannot be set up.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The provider is unknown or not built in via a Cargo feature.
    #[error("unknown provider '{provider}' (built-in: {})", BUILTIN.join(", "))]
    Unknown { provider: String },
    /// A required environment variable is missing.
    #[error("provider '{provider}' needs the environment variable {variable}")]
    MissingVariable {
        provider: String,
        variable: &'static str,
    },
    /// The provider has no embedding models in grauwolf (yet).
    #[error("provider '{provider}' cannot be used for embeddings yet")]
    NoEmbeddings { provider: String },
}

/// Built-in providers (depending on the Cargo features).
pub const BUILTIN: &[&str] = &[
    #[cfg(feature = "ollama")]
    "ollama",
    #[cfg(feature = "openai")]
    "openai",
    #[cfg(feature = "anthropic")]
    "anthropic",
    #[cfg(feature = "gemini")]
    "gemini",
    #[cfg(feature = "bedrock")]
    "bedrock",
    #[cfg(feature = "openai")]
    "openrouter",
    #[cfg(feature = "openai")]
    "deepseek",
    #[cfg(feature = "openai")]
    "mistral",
    #[cfg(feature = "openai")]
    "groq",
    #[cfg(feature = "openai")]
    "dashscope",
    #[cfg(feature = "openai")]
    "xai",
];

/// A provider that mimics the chat completions interface of OpenAI (ADR-0082).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAiCompatible {
    /// Name as in `model.provider`.
    pub provider: &'static str,
    /// Environment variable with the API key.
    pub api_key: &'static str,
    /// Environment variable for another address.
    pub base_url: &'static str,
    /// Address without `base_url`.
    pub default_base_url: &'static str,
}

impl OpenAiCompatible {
    /// Address from `base_url`, otherwise `default_base_url`.
    pub fn base_url_from(&self, env: impl Fn(&str) -> Option<String>) -> String {
        env(self.base_url).unwrap_or_else(|| self.default_base_url.to_owned())
    }

    /// Address from the settings in `config`, otherwise `default_base_url`.
    pub fn base_url_in(&self, config: &ProvidersConfig) -> String {
        config
            .compatible(self.base_url)
            .unwrap_or_else(|| self.default_base_url.to_owned())
    }
}

/// Built-in OpenAI-compatible providers (ADR-0082).
pub const OPENAI_COMPATIBLE: &[OpenAiCompatible] = &[
    OpenAiCompatible {
        provider: "openrouter",
        api_key: "OPENROUTER_API_KEY",
        base_url: "OPENROUTER_BASE_URL",
        default_base_url: "https://openrouter.ai/api/v1",
    },
    OpenAiCompatible {
        provider: "deepseek",
        api_key: "DEEPSEEK_API_KEY",
        base_url: "DEEPSEEK_BASE_URL",
        default_base_url: "https://api.deepseek.com",
    },
    OpenAiCompatible {
        provider: "mistral",
        api_key: "MISTRAL_API_KEY",
        base_url: "MISTRAL_BASE_URL",
        default_base_url: "https://api.mistral.ai/v1",
    },
    OpenAiCompatible {
        provider: "groq",
        api_key: "GROQ_API_KEY",
        base_url: "GROQ_BASE_URL",
        default_base_url: "https://api.groq.com/openai/v1",
    },
    OpenAiCompatible {
        provider: "dashscope",
        api_key: "DASHSCOPE_API_KEY",
        base_url: "DASHSCOPE_BASE_URL",
        default_base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
    },
    OpenAiCompatible {
        provider: "xai",
        api_key: "XAI_API_KEY",
        base_url: "XAI_BASE_URL",
        default_base_url: "https://api.x.ai/v1",
    },
];

/// The OpenAI-compatible provider `provider`, if built in.
#[cfg(feature = "openai")]
fn openai_compatible(provider: &str) -> Option<&'static OpenAiCompatible> {
    OPENAI_COMPATIBLE
        .iter()
        .find(|compatible| compatible.provider == provider)
}

/// Executor for `provider` with settings from the environment of the process; transient errors
/// are retried according to [`RetryPolicy::default`].
pub fn executor_from_env(provider: &str) -> Result<PromptExecutor, ProviderError> {
    executor_with(provider, &ProvidersConfig::from_env(&ProcessEnv))
}

/// Executor with one client each for all `providers` (e.g. from `AgentSpec::providers()`) and
/// retries; `env` returns the environment variables (ADR-0043).
pub fn executor_for<'p>(
    providers: impl IntoIterator<Item = &'p str>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<PromptExecutor, ProviderError> {
    executor_for_config(providers, &ProvidersConfig::from_lookup(&env))
}

/// Like [`executor_for`] with the settings in `config`.
pub fn executor_for_config<'p>(
    providers: impl IntoIterator<Item = &'p str>,
    config: &ProvidersConfig,
) -> Result<PromptExecutor, ProviderError> {
    Ok(executor_builder_with(providers, config)?.build())
}

/// Like [`executor_for`], but still open for further settings, e.g. a response cache
/// (`.cache(..)`, ADR-0086).
pub fn executor_builder_for<'p>(
    providers: impl IntoIterator<Item = &'p str>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<PromptExecutorBuilder, ProviderError> {
    executor_builder_with(providers, &ProvidersConfig::from_lookup(&env))
}

/// Like [`executor_builder_for`] with the settings in `config`.
pub fn executor_builder_with<'p>(
    providers: impl IntoIterator<Item = &'p str>,
    config: &ProvidersConfig,
) -> Result<PromptExecutorBuilder, ProviderError> {
    let mut builder = PromptExecutor::builder();
    for provider in providers {
        builder = builder.client(provider, client_with(provider, config)?);
    }
    Ok(builder.retry(RetryPolicy::default(), Arc::new(TokioClock)))
}

/// Executor for `provider` with retries; `env` returns the environment variables.
pub fn executor_from(
    provider: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<PromptExecutor, ProviderError> {
    executor_with(provider, &ProvidersConfig::from_lookup(&env))
}

/// Like [`executor_from`] with the settings in `config`.
pub fn executor_with(
    provider: &str,
    config: &ProvidersConfig,
) -> Result<PromptExecutor, ProviderError> {
    Ok(PromptExecutor::builder()
        .client(provider, client_with(provider, config)?)
        .retry(RetryPolicy::default(), Arc::new(TokioClock))
        .build())
}

/// Client for `provider` with settings from the environment variables of the process.
pub fn client_from_env(provider: &str) -> Result<Arc<dyn LlmClient>, ProviderError> {
    client_with(provider, &ProvidersConfig::from_env(&ProcessEnv))
}

/// Client for `provider`; `env` returns the values of the environment variables.
pub fn client_from(
    provider: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Arc<dyn LlmClient>, ProviderError> {
    client_with(provider, &ProvidersConfig::from_lookup(&env))
}

/// Client for `provider` with the settings in `config`.
pub fn client_with(
    provider: &str,
    config: &ProvidersConfig,
) -> Result<Arc<dyn LlmClient>, ProviderError> {
    let _ = config;
    #[allow(unused_variables)]
    let required = |variable: &'static str, value: &Option<String>| {
        value.clone().ok_or_else(|| ProviderError::MissingVariable {
            provider: provider.to_owned(),
            variable,
        })
    };
    match provider {
        #[cfg(feature = "ollama")]
        "ollama" => {
            let client = grauwolf_ollama::OllamaClient::new(ollama_host(config));
            Ok(Arc::new(match config.ollama_think.as_deref() {
                Some("false" | "0") => client.with_think(false),
                Some("true" | "1") => client.with_think(true),
                _ => client,
            }))
        }
        #[cfg(feature = "openai")]
        "openai" => {
            let client = grauwolf_openai::OpenAiClient::new(required(
                "OPENAI_API_KEY",
                &config.openai_api_key,
            )?);
            Ok(Arc::new(match config.openai_base_url.clone() {
                Some(base_url) => client.with_base_url(base_url),
                None => client,
            }))
        }
        #[cfg(feature = "anthropic")]
        "anthropic" => Ok(Arc::new(grauwolf_anthropic::AnthropicClient::new(
            required("ANTHROPIC_API_KEY", &config.anthropic_api_key)?,
        ))),
        #[cfg(feature = "gemini")]
        "gemini" => {
            let client = grauwolf_gemini::GeminiClient::new(required(
                "GEMINI_API_KEY",
                &config.gemini_api_key,
            )?);
            Ok(Arc::new(match config.gemini_base_url.clone() {
                Some(base_url) => client.with_base_url(base_url),
                None => client,
            }))
        }
        #[cfg(feature = "bedrock")]
        "bedrock" => Ok(Arc::new(bedrock_client(config)?)),
        _ => {
            #[cfg(feature = "openai")]
            if let Some(compatible) = openai_compatible(provider) {
                let client = grauwolf_openai::OpenAiClient::new(required(
                    compatible.api_key,
                    &config.compatible(compatible.api_key),
                )?);
                return Ok(Arc::new(
                    client.with_base_url(compatible.base_url_in(config)),
                ));
            }
            Err(ProviderError::Unknown {
                provider: provider.to_owned(),
            })
        }
    }
}

/// Embedder for the model `model` of `provider` (RAG, ADR-0038), settings from `env`;
/// OpenAI-compatible providers via `/embeddings` (ADR-0082), `gemini` via `:batchEmbedContents`
/// (ADR-0083).
pub fn embedder_from(
    provider: &str,
    model: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Arc<dyn Embedder>, ProviderError> {
    embedder_with(provider, model, &ProvidersConfig::from_lookup(&env))
}

/// Like [`embedder_from`] with the settings in `config`.
pub fn embedder_with(
    provider: &str,
    model: &str,
    config: &ProvidersConfig,
) -> Result<Arc<dyn Embedder>, ProviderError> {
    // Without provider features, `model` and `config` remain unused.
    let _ = (model, config);
    match provider {
        #[cfg(feature = "ollama")]
        "ollama" => Ok(Arc::new(grauwolf_ollama::OllamaEmbedder::new(
            ollama_host(config),
            model,
        ))),
        #[cfg(feature = "openai")]
        "openai" => {
            let api_key =
                config
                    .openai_api_key
                    .clone()
                    .ok_or_else(|| ProviderError::MissingVariable {
                        provider: provider.to_owned(),
                        variable: "OPENAI_API_KEY",
                    })?;
            let embedder = grauwolf_openai::OpenAiEmbedder::new(api_key, model);
            Ok(Arc::new(match config.openai_base_url.clone() {
                Some(base_url) => embedder.with_base_url(base_url),
                None => embedder,
            }))
        }
        #[cfg(feature = "gemini")]
        "gemini" => {
            let api_key =
                config
                    .gemini_api_key
                    .clone()
                    .ok_or_else(|| ProviderError::MissingVariable {
                        provider: provider.to_owned(),
                        variable: "GEMINI_API_KEY",
                    })?;
            let embedder = grauwolf_gemini::GeminiEmbedder::new(api_key, model);
            Ok(Arc::new(match config.gemini_base_url.clone() {
                Some(base_url) => embedder.with_base_url(base_url),
                None => embedder,
            }))
        }
        // Embedding models on Bedrock (Titan, Cohere) are not supported yet (ADR-0105).
        #[cfg(feature = "bedrock")]
        "bedrock" => Err(ProviderError::NoEmbeddings {
            provider: provider.to_owned(),
        }),
        _ => {
            #[cfg(feature = "openai")]
            if let Some(compatible) = openai_compatible(provider) {
                let api_key = config.compatible(compatible.api_key).ok_or_else(|| {
                    ProviderError::MissingVariable {
                        provider: provider.to_owned(),
                        variable: compatible.api_key,
                    }
                })?;
                let embedder = grauwolf_openai::OpenAiEmbedder::new(api_key, model);
                return Ok(Arc::new(
                    embedder.with_base_url(compatible.base_url_in(config)),
                ));
            }
            Err(ProviderError::Unknown {
                provider: provider.to_owned(),
            })
        }
    }
}

/// The bedrock client: a Bedrock API key (`AWS_BEARER_TOKEN_BEDROCK`) wins over access keys.
#[cfg(feature = "bedrock")]
fn bedrock_client(
    config: &ProvidersConfig,
) -> Result<grauwolf_bedrock::BedrockClient, ProviderError> {
    let missing = |variable: &'static str| ProviderError::MissingVariable {
        provider: "bedrock".to_owned(),
        variable,
    };
    let region = config
        .aws_region
        .clone()
        .unwrap_or_else(|| grauwolf_bedrock::DEFAULT_REGION.to_owned());
    let client = match (&config.aws_bearer_token_bedrock, &config.aws_access_key_id) {
        (Some(token), _) => grauwolf_bedrock::BedrockClient::with_api_key(token, region),
        (None, Some(access_key_id)) => {
            let secret_access_key = config
                .aws_secret_access_key
                .clone()
                .ok_or_else(|| missing("AWS_SECRET_ACCESS_KEY"))?;
            grauwolf_bedrock::BedrockClient::new(
                grauwolf_bedrock::Credentials {
                    access_key_id: access_key_id.clone(),
                    secret_access_key,
                    session_token: config.aws_session_token.clone(),
                },
                region,
            )
        }
        (None, None) => {
            return Err(missing(
                "AWS_BEARER_TOKEN_BEDROCK, or AWS_ACCESS_KEY_ID with AWS_SECRET_ACCESS_KEY",
            ));
        }
    };
    Ok(match config.bedrock_base_url.clone() {
        Some(base_url) => client.with_base_url(base_url),
        None => client,
    })
}

/// Address of the Ollama server from `OLLAMA_HOST` (in chat API mode also `OLLAMA_BASE_URL`),
/// otherwise the default address.
#[cfg(feature = "ollama")]
pub fn ollama_host(config: &ProvidersConfig) -> String {
    let host = config
        .ollama_host
        .clone()
        .or_else(|| {
            // AgentWeaver's deployments set OLLAMA_BASE_URL (ADR-0094).
            config
                .chat_api_mode
                .then(|| config.ollama_base_url.clone())
                .flatten()
        })
        .unwrap_or_else(|| grauwolf_ollama::DEFAULT_BASE_URL.to_owned());
    with_scheme(host)
}

/// `OLLAMA_HOST` may be given without a scheme, as with Ollama itself (`localhost:11434`).
#[cfg(feature = "ollama")]
fn with_scheme(host: String) -> String {
    if host.contains("://") {
        host
    } else {
        format!("http://{host}")
    }
}

#[cfg(all(test, feature = "openai"))]
mod tests {
    use super::*;
    use crate::config::COMPATIBLE_VARIABLES;

    /// The configuration reads exactly the variables of the built-in compatible providers.
    #[test]
    fn the_configuration_knows_the_variables_of_every_compatible_provider() {
        let mut table: Vec<&str> = OPENAI_COMPATIBLE
            .iter()
            .flat_map(|compatible| [compatible.api_key, compatible.base_url])
            .collect();
        let mut known = COMPATIBLE_VARIABLES.to_vec();
        table.sort_unstable();
        known.sort_unstable();
        assert_eq!(table, known);
    }
}
