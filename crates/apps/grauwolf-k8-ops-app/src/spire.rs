//! What the tool reads from SPIRE and the cluster: the trust domain (one function per source),
//! the version of the running SPIRE server, ready state of workloads.

use grauwolf_k8s::{Cluster, kinds};
use serde_json::Value;

use crate::cli::Settings;
use crate::hcl::{trust_domain_of_hcl, trust_domain_of_yaml};

pub const SERVER_CONFIG: (&str, &str) = ("spire-server", "server.conf");
pub const AGENT_CONFIG: (&str, &str) = ("spire-agent", "agent.conf");
pub const CONTROLLER_CONFIG: (&str, &str) = ("spire-controller-manager", "controller-manager.yaml");
pub const OIDC_CONFIG: (&str, &str) = (
    "spire-oidc-discovery-provider",
    "oidc-discovery-provider.conf",
);
pub const OIDC_NAME: &str = "spire-oidc-discovery-provider";

/// Where a trust domain was read.
pub struct Source {
    pub label: &'static str,
    pub domain: Result<String, String>,
}

async fn config_text(
    cluster: &Cluster,
    namespace: &str,
    (map, key): (&str, &str),
) -> Result<String, String> {
    let object = cluster
        .get(&kinds::CONFIG_MAP, Some(namespace), map)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("ConfigMap {namespace}/{map} not found"))?;
    object["data"][key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("ConfigMap {namespace}/{map} has no key {key}"))
}

/// The one source of truth (ADR-0120 decision 4): `trust_domain` in the SPIRE server's
/// configuration. Stage 2 (`trust-domain set`) rewrites exactly this and the sources below.
pub async fn trust_domain(cluster: &Cluster, settings: &Settings) -> Result<String, String> {
    let text = config_text(cluster, &settings.spire_namespace, SERVER_CONFIG).await?;
    trust_domain_of_hcl(&text).ok_or_else(|| {
        format!(
            "no trust_domain in {}/{} of the SPIRE server",
            SERVER_CONFIG.0, SERVER_CONFIG.1
        )
    })
}

/// The trust domain in every place of the cluster that holds one (not OpenBao).
pub async fn trust_domain_sources(cluster: &Cluster, settings: &Settings) -> Vec<Source> {
    let spire = &settings.spire_namespace;
    let mut sources = Vec::new();
    for (label, namespace, config, yaml) in [
        ("SPIRE server", spire.as_str(), SERVER_CONFIG, false),
        ("SPIRE agent", spire.as_str(), AGENT_CONFIG, false),
        (
            "spire-controller-manager",
            spire.as_str(),
            CONTROLLER_CONFIG,
            true,
        ),
        (
            "OIDC discovery provider",
            settings.oidc_namespace.as_str(),
            OIDC_CONFIG,
            false,
        ),
    ] {
        let domain = match config_text(cluster, namespace, config).await {
            Ok(text) => {
                let found = if yaml {
                    trust_domain_of_yaml(&text)
                } else {
                    trust_domain_of_hcl(&text)
                };
                found.ok_or_else(|| format!("no trust domain in {}/{}", config.0, config.1))
            }
            Err(error) => Err(error),
        };
        sources.push(Source { label, domain });
    }
    sources
}

/// `spiffe://<trust domain>/ns/<namespace>/sa/<service account>`.
pub fn spiffe_id(trust_domain: &str, namespace: &str, service_account: &str) -> String {
    format!("spiffe://{trust_domain}/ns/{namespace}/sa/{service_account}")
}

/// The trust domain of a SPIFFE ID.
pub fn domain_of_id(id: &str) -> Option<&str> {
    id.strip_prefix("spiffe://")?.split('/').next()
}

/// The image tag of the running SPIRE server (`ghcr.io/spiffe/spire-server:1.15.5` -> `1.15.5`).
pub async fn server_version(cluster: &Cluster, settings: &Settings) -> Result<String, String> {
    let server = cluster
        .get(&kinds::STATEFUL_SET, Some(&settings.spire_namespace), "spire-server")
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "StatefulSet {}/spire-server not found: SPIRE is not installed (grauwolf-k8-ops does not install SPIRE)",
                settings.spire_namespace
            )
        })?;
    let containers = server["spec"]["template"]["spec"]["containers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let image = containers
        .iter()
        .filter_map(|container| container["image"].as_str())
        .find(|image| image.contains("spire-server"))
        .ok_or("the StatefulSet spire-server has no spire-server container")?;
    image_tag(image).ok_or_else(|| format!("the image {image} has no version tag"))
}

fn image_tag(image: &str) -> Option<String> {
    let without_digest = image.split('@').next()?;
    let (name, tag) = without_digest.rsplit_once(':')?;
    (!name.is_empty() && !tag.contains('/')).then(|| tag.to_owned())
}

/// `(wanted, ready)` replicas of a Deployment, StatefulSet or DaemonSet.
pub fn replicas(workload: &Value) -> (u64, u64) {
    let number = |pointer: &str| workload.pointer(pointer).and_then(Value::as_u64);
    if workload["kind"] == "DaemonSet" {
        (
            number("/status/desiredNumberScheduled").unwrap_or(0),
            number("/status/numberReady").unwrap_or(0),
        )
    } else {
        (
            number("/spec/replicas").unwrap_or(1),
            number("/status/readyReplicas").unwrap_or(0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tags() {
        assert_eq!(
            image_tag("ghcr.io/spiffe/spire-server:1.15.5").as_deref(),
            Some("1.15.5")
        );
        assert_eq!(image_tag("localhost:5000/spire-server").as_deref(), None);
        assert_eq!(
            image_tag("x/spire-server:1.2.3@sha256:abc").as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn ids() {
        assert_eq!(spiffe_id("d.de", "a", "b"), "spiffe://d.de/ns/a/sa/b");
        assert_eq!(domain_of_id("spiffe://d.de/ns/a/sa/b"), Some("d.de"));
        assert_eq!(domain_of_id("x"), None);
    }
}
