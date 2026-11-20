//! chat API mode (ADR-0094): the signature file, the agent names and the observations that
//! `GET /api/signature` reports.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::observations::{ModelDigests, Observations};
use serde_json::Value;

/// The signature file and the agent names next to the service (defined with the project that
/// copies them, `grauwolf_host::project`).
pub use grauwolf_host::project::{NAMES_FILE, SIGNATURE_FILE};

/// Environment variable with the path of the signature file.
pub const SIGNATURE_PATH_VARIABLE: &str = "AGENT_SIGNATURE_PATH";

/// Signature and observations of an agent in chat API mode.
pub struct ChatApi {
    signature: Option<Value>,
    observations: Arc<Observations>,
    digests: Option<Arc<dyn ModelDigests>>,
}

impl ChatApi {
    /// Reads the signature (`AGENT_SIGNATURE_PATH`, else `agent-signature.json` in `base_dir`;
    /// `None` if absent or unreadable) and the names (`agent-names.json` in `base_dir`) and
    /// starts the observations (hand `observations().mcp_tools_observer()` to whoever starts the
    /// agent's MCP servers).
    pub fn from_dir(base_dir: &Path, env: impl Fn(&str) -> Option<String>) -> Arc<Self> {
        let signature_path = grauwolf_host::config::ChatApiConfig::from_lookup(&env)
            .signature_path
            .map_or_else(|| base_dir.join(SIGNATURE_FILE), Into::into);
        let signature = std::fs::read_to_string(signature_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        let observations = Observations::new(agent_names(base_dir));
        Arc::new(Self {
            signature,
            observations,
            digests: digests(&env),
        })
    }

    /// The signature file's content, if there is a readable one.
    pub fn signature(&self) -> Option<&Value> {
        self.signature.as_ref()
    }

    /// The observations; install `feature_for` of the agents in the loader.
    pub fn observations(&self) -> &Arc<Observations> {
        &self.observations
    }

    /// The options that switch the routes on in `grauwolf-server`.
    pub fn options(self: &Arc<Self>) -> crate::ChatApiOptions {
        let chat_api = Arc::clone(self);
        crate::ChatApiOptions {
            signature: self.signature.clone(),
            observations: Arc::new(move || {
                let chat_api = Arc::clone(&chat_api);
                Box::pin(async move { chat_api.report().await })
            }),
        }
    }

    /// The `observations` object of the signature endpoint.
    pub async fn report(&self) -> Value {
        self.observations.report(self.digests.as_deref()).await
    }
}

/// `{ "agents": { "<spec name>": "<agent name>" } }` from `agent-names.json`; empty if the
/// file is absent or unreadable.
fn agent_names(base_dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(base_dir.join(NAMES_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|file| {
            file["agents"].as_object().map(|agents| {
                agents
                    .iter()
                    .filter_map(|(spec, name)| Some((spec.clone(), name.as_str()?.to_owned())))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Digests of the models of the Ollama server the agent uses.
#[cfg(feature = "ollama")]
fn digests(env: &impl Fn(&str) -> Option<String>) -> Option<Arc<dyn ModelDigests>> {
    struct Tags(grauwolf_ollama::OllamaClient);

    impl ModelDigests for Tags {
        fn digest<'a>(
            &'a self,
            model: &'a str,
        ) -> futures_util::future::BoxFuture<'a, Option<String>> {
            Box::pin(self.0.model_digest(model))
        }
    }

    Some(Arc::new(Tags(grauwolf_ollama::OllamaClient::new(
        grauwolf_host::providers::ollama_host(
            &grauwolf_host::config::ProvidersConfig::from_lookup(env).with_chat_api_mode(true),
        ),
    ))))
}

#[cfg(not(feature = "ollama"))]
fn digests(_env: &impl Fn(&str) -> Option<String>) -> Option<Arc<dyn ModelDigests>> {
    None
}
