//! `oidc install`: the SPIRE OIDC discovery provider in a namespace of its own, with the image
//! version of the running SPIRE server and its `ClusterSPIFFEID` (the manifest of
//! `examples/spire-openbao/manifests/10-oidc-discovery-provider.yaml`).

use grauwolf_k8s::{Kind, kinds};
use serde_json::{Value, json};

use crate::io::Io;
use crate::report;
use crate::services::Services;
use crate::spire::{self, OIDC_NAME};

pub const SPIFFE_ID_TEMPLATE: &str =
    "spiffe://{{ .TrustDomain }}/ns/{{ .PodMeta.Namespace }}/sa/{{ .PodSpec.ServiceAccountName }}";

/// The in-cluster address of the provider.
pub fn service_host(namespace: &str) -> String {
    format!("{OIDC_NAME}.{namespace}.svc")
}

/// The objects of the provider, in the order they are applied.
pub fn objects(
    namespace: &str,
    trust_domain: &str,
    version: &str,
) -> Vec<(Kind, Option<String>, Value)> {
    let host = service_host(namespace);
    let selector = json!({"app.kubernetes.io/name": OIDC_NAME});
    let config = format!(
        "log_level = \"INFO\"\n\
         domains = [\"{host}\", \"{OIDC_NAME}\", \"localhost\"]\n\
         # Plain HTTP inside the cluster; put TLS in front for anything that leaves it.\n\
         allow_insecure_scheme = true\n\
         insecure_addr = \":8080\"\n\
         workload_api {{\n  socket_path = \"/spiffe-workload-api/agent.sock\"\n  trust_domain = \"{trust_domain}\"\n}}\n\
         health_checks {{\n  listener_enabled = true\n  bind_port = 8008\n  live_path = \"/live\"\n  ready_path = \"/ready\"\n}}\n"
    );
    let ns = Some(namespace.to_owned());
    vec![
        (
            kinds::NAMESPACE,
            None,
            json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": namespace}}),
        ),
        (
            kinds::SERVICE_ACCOUNT,
            ns.clone(),
            json!({"apiVersion": "v1", "kind": "ServiceAccount",
                "metadata": {"name": OIDC_NAME, "namespace": namespace}}),
        ),
        (
            kinds::CONFIG_MAP,
            ns.clone(),
            json!({"apiVersion": "v1", "kind": "ConfigMap",
                "metadata": {"name": OIDC_NAME, "namespace": namespace},
                "data": {"oidc-discovery-provider.conf": config}}),
        ),
        (
            kinds::DEPLOYMENT,
            ns.clone(),
            json!({"apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": OIDC_NAME, "namespace": namespace,
                "labels": {"app.kubernetes.io/name": OIDC_NAME}},
            "spec": {"replicas": 1, "selector": {"matchLabels": selector},
                "template": {"metadata": {"labels": selector}, "spec": {
                    "serviceAccountName": OIDC_NAME,
                    "containers": [{
                        "name": "oidc-discovery-provider",
                        "image": format!("ghcr.io/spiffe/oidc-discovery-provider:{version}"),
                        "args": ["-config", "/run/spire/oidc/config/oidc-discovery-provider.conf"],
                        "ports": [{"name": "http", "containerPort": 8080}],
                        "readinessProbe": {"httpGet": {"path": "/ready", "port": 8008}},
                        "livenessProbe": {"httpGet": {"path": "/live", "port": 8008}},
                        "volumeMounts": [
                            {"name": "config", "mountPath": "/run/spire/oidc/config", "readOnly": true},
                            {"name": "spiffe-workload-api", "mountPath": "/spiffe-workload-api", "readOnly": true}],
                    }],
                    "volumes": [
                        {"name": "config", "configMap": {"name": OIDC_NAME}},
                        {"name": "spiffe-workload-api", "csi": {"driver": "csi.spiffe.io", "readOnly": true}}],
                }}}}),
        ),
        (
            kinds::SERVICE,
            ns,
            json!({"apiVersion": "v1", "kind": "Service",
                "metadata": {"name": OIDC_NAME, "namespace": namespace},
                "spec": {"selector": selector,
                    "ports": [{"name": "http", "port": 80, "targetPort": "http"}]}}),
        ),
        (
            kinds::CLUSTER_SPIFFE_ID,
            None,
            json!({"apiVersion": "spire.spiffe.io/v1alpha1", "kind": "ClusterSPIFFEID",
                "metadata": {"name": OIDC_NAME},
                "spec": {"spiffeIDTemplate": SPIFFE_ID_TEMPLATE,
                    "podSelector": {"matchLabels": selector},
                    "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": namespace}}}}),
        ),
    ]
}

pub async fn install(services: &Services, io: &mut Io<'_>) -> Result<(), String> {
    let settings = &services.settings;
    let cluster = &services.cluster;
    let trust_domain = spire::trust_domain(cluster, settings).await?;
    let version = spire::server_version(cluster, settings).await?;
    let namespace = &settings.oidc_namespace;
    io.line(&format!(
        "trust domain {trust_domain}, SPIRE {version}; discovery provider in namespace {namespace}"
    ));
    for (kind, scope, object) in objects(namespace, &trust_domain, &version) {
        let name = object["metadata"]["name"].as_str().unwrap_or("").to_owned();
        let outcome = cluster
            .ensure(&kind, scope.as_deref(), &object)
            .await
            .map_err(|error| format!("{} {name}: {error}", kind.kind))?;
        io.line(&format!("{} {name}: {}", kind.kind, report::k8s(outcome)));
    }
    io.line(&format!(
        "keys: http://{}/keys (next: grauwolf-k8-ops openbao setup)",
        service_host(namespace)
    ));
    Ok(())
}
