//! The default address of every model provider: the one place of the egress derivation that knows
//! them. A test of `grauwolf-host` keeps this table equal to the provider adapters and to
//! `OPENAI_COMPATIBLE` (a provider with another address is a change there and here).

/// Region of the `bedrock` provider without `AWS_REGION` (`grauwolf_bedrock::DEFAULT_REGION`).
pub const BEDROCK_DEFAULT_REGION: &str = "us-west-2";

/// Where a provider is reached without configuration, and which variable changes that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderAddress {
    /// Name as in `model.provider`.
    pub provider: &'static str,
    /// Default address.
    pub default_url: &'static str,
    /// The variable that replaces it: an address, or for `bedrock` the region.
    pub from_env: Option<&'static str>,
}

const fn row(
    provider: &'static str,
    default_url: &'static str,
    from_env: Option<&'static str>,
) -> ProviderAddress {
    ProviderAddress {
        provider,
        default_url,
        from_env,
    }
}

/// Every built-in provider.
pub const PROVIDER_ADDRESSES: &[ProviderAddress] = &[
    row("ollama", "http://localhost:11434", Some("OLLAMA_HOST")),
    row(
        "openai",
        "https://api.openai.com/v1",
        Some("OPENAI_BASE_URL"),
    ),
    row("anthropic", "https://api.anthropic.com", None),
    row(
        "gemini",
        "https://generativelanguage.googleapis.com",
        Some("GEMINI_BASE_URL"),
    ),
    // The host names the region: `bedrock_url`.
    row("bedrock", "", Some("AWS_REGION")),
    row(
        "openrouter",
        "https://openrouter.ai/api/v1",
        Some("OPENROUTER_BASE_URL"),
    ),
    row(
        "deepseek",
        "https://api.deepseek.com",
        Some("DEEPSEEK_BASE_URL"),
    ),
    row(
        "mistral",
        "https://api.mistral.ai/v1",
        Some("MISTRAL_BASE_URL"),
    ),
    row(
        "groq",
        "https://api.groq.com/openai/v1",
        Some("GROQ_BASE_URL"),
    ),
    row(
        "dashscope",
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        Some("DASHSCOPE_BASE_URL"),
    ),
    row("xai", "https://api.x.ai/v1", Some("XAI_BASE_URL")),
];

/// The row of `provider`.
pub fn address_of(provider: &str) -> Option<&'static ProviderAddress> {
    PROVIDER_ADDRESSES
        .iter()
        .find(|row| row.provider == provider)
}

/// The Bedrock Runtime endpoint of `region`.
pub fn bedrock_url(region: &str) -> String {
    format!("https://bedrock-runtime.{region}.amazonaws.com")
}
