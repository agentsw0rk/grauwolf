//! `identity install | status | uninstall | allow | deny`: the identity service of the trust
//! domain (ADR-0111 Q3, Q4) – `grauwolf identity serve` of the CLI image in namespace `identity`,
//! with its SPIFFE identity, an mTLS port like the agents, its policy as a ConfigMap and its own
//! OpenBao role that may only sign with one Transit key.
//!
//! The pod carries the labels of an agent's pod (`app.kubernetes.io/name=grauwolf`,
//! `grauwolf.io/agent=grauwolf-identity`), so the rollout wait and the egress policy of
//! `agent deploy` apply to it; its namespace carries no agent label, so it is not an agent.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use grauwolf_egress::{Destination, Phase};
use grauwolf_k8s::{Cluster, hash_of, kinds};
use grauwolf_openbao::jwt_role;
use serde_json::{Map, Value, json};

use crate::agent::{self, AGENT_LABEL, MANAGED_BY, MANAGER};
use crate::cli::IdentityCommand;
use crate::deploy;
use crate::egress::{self, EgressOptions};
use crate::io::Io;
use crate::oidc::SPIFFE_ID_TEMPLATE;
use crate::report;
use crate::services::Services;
use crate::spire;

/// The namespace of the identity service.
pub const NAMESPACE: &str = "identity";
/// The name of its service account, Deployment, Service, ClusterSPIFFEID and OpenBao role and
/// policy.
pub const NAME: &str = "grauwolf-identity";
/// The label that marks the namespace as the identity service's.
pub const COMPONENT_LABEL: &str = "grauwolf.io/component";
pub const COMPONENT: &str = "identity";
/// The ConfigMap with the policy (which agents may request Txn-Tokens for people).
pub const POLICY_MAP: &str = "grauwolf-identity-policy";
pub const POLICY_KEY: &str = "policy.json";
/// Mounted as a directory: the service re-reads the file when the ConfigMap changes.
pub const POLICY_DIR: &str = "/etc/grauwolf/identity";
/// The NetworkPolicy of the incoming side.
pub const INGRESS_POLICY: &str = "grauwolf-identity-ingress";
/// The Transit mount (the service's default `GRAUWOLF_IDENTITY_TRANSIT_MOUNT`).
pub const TRANSIT_MOUNT: &str = "transit";
pub const DEFAULT_TRANSIT_KEY: &str = "grauwolf-identity";
pub const DEFAULT_SCOPE: &str = "access_as_user";
pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_MTLS_PORT: u16 = 8443;
const KEY_TYPE: &str = "ecdsa-p256";
/// The KV folders of the grants (the service's `GRANTS_PATH` and `GRANT_TOKENS_PATH`).
pub const GRANTS_FOLDER: &str = "identity/grants";
pub const GRANT_TOKENS_FOLDER: &str = "identity/grant-tokens";
const TRANSIT_KEY_VARIABLE: &str = "GRAUWOLF_IDENTITY_TRANSIT_KEY";
/// What agents of `agent deploy --identity` get.
pub const URL_VARIABLE: &str = "GRAUWOLF_IDENTITY_URL";
pub const SPIFFE_ID_VARIABLE: &str = "GRAUWOLF_IDENTITY_SPIFFE_ID";
const NAMESPACE_NAME_LABEL: &str = "kubernetes.io/metadata.name";

/// The arguments of `identity install`.
pub struct InstallOptions {
    pub tenant: String,
    pub issuer: String,
    /// The accepted audiences of people's tokens (at least one).
    pub audience: Vec<String>,
    pub jwks_url: Option<String>,
    pub scope: String,
    pub public_url: Option<String>,
    pub policy: Option<std::path::PathBuf>,
    pub image: String,
    pub pull_secret: Option<String>,
    pub transit_key: String,
    pub port: u16,
    pub mtls_port: u16,
    pub egress: EgressOptions,
    pub timeout: u64,
    /// The public client whose refresh tokens grants hold (`--grant-client-id`); without it the
    /// service has no grants.
    pub grant_client_id: Option<String>,
}

pub async fn run(
    services: &Services,
    io: &mut Io<'_>,
    command: IdentityCommand,
) -> Result<i32, String> {
    match command {
        IdentityCommand::Install {
            tenant,
            issuer,
            audience,
            jwks_url,
            scope,
            public_url,
            policy,
            image,
            pull_secret,
            transit_key,
            port,
            mtls_port,
            cluster_cidr,
            egress_dns_namespace,
            egress_dns_selector,
            timeout,
            grant_client_id,
        } => {
            let options = InstallOptions {
                tenant,
                issuer,
                audience,
                jwks_url,
                scope,
                public_url,
                policy,
                image,
                pull_secret,
                transit_key,
                port,
                mtls_port,
                egress: EgressOptions {
                    strict: false,
                    cluster_cidrs: if cluster_cidr.is_empty() {
                        egress::DEFAULT_CLUSTER_CIDRS
                            .iter()
                            .map(|cidr| (*cidr).to_owned())
                            .collect()
                    } else {
                        cluster_cidr
                    },
                    dns_namespace: egress_dns_namespace,
                    dns_selector: egress_dns_selector,
                },
                timeout,
                grant_client_id,
            };
            install(services, io, options).await
        }
        IdentityCommand::Status { json } => status(services, io, json).await.map(|()| 0),
        IdentityCommand::Uninstall { delete_key, yes } => {
            uninstall(services, io, delete_key, yes).await.map(|()| 0)
        }
        IdentityCommand::Grants { revoke, json } => {
            grants(services, io, revoke, json).await.map(|()| 0)
        }
        IdentityCommand::Allow {
            agent,
            grants,
            trigger,
            purposes,
        } => allow(
            services,
            io,
            policy_target(agent, grants, trigger)?,
            purposes,
        )
        .await
        .map(|()| 0),
        IdentityCommand::Deny {
            agent,
            grants,
            trigger,
        } => deny(services, io, policy_target(agent, grants, trigger)?)
            .await
            .map(|()| 0),
    }
}

/// `spiffe://<trust domain>/ns/identity/sa/grauwolf-identity`.
pub fn spiffe_id(trust_domain: &str) -> String {
    spire::spiffe_id(trust_domain, NAMESPACE, NAME)
}

/// The URL of the mTLS port inside the cluster (`GRAUWOLF_IDENTITY_URL` of the agents).
pub fn mtls_url(mtls_port: u16) -> String {
    format!("https://{NAME}.{NAMESPACE}.svc:{mtls_port}")
}

fn http_url(port: u16) -> String {
    format!("http://{NAME}.{NAMESPACE}.svc:{port}")
}

/// `(scheme, host, port)` of an `http(s)://` URL; the port defaults to the scheme's.
pub fn url_parts(url: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains(['@', ' ']) {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(']') || port.parse::<u16>().is_ok() => {
            (host, port.parse::<u16>().ok()?)
        }
        _ => (authority, default),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then(|| (scheme.to_owned(), host.to_lowercase(), port))
}

fn check_url(option: &str, url: &str) -> Result<(), String> {
    url_parts(url)
        .map(|_| ())
        .ok_or_else(|| format!("{option} '{url}' is not an http(s) URL"))
}

/// Entries of a policy section: SPIFFE ID (or `<prefix>/*`) to its purposes (`None`: any).
pub type Entries = BTreeMap<String, Option<Vec<String>>>;

/// The identity service's policy file:
/// `{"callers": {"<spiffe id>[/*]": {"purposes"?: [..]}},
///   "grants": {"agents": {"<spiffe id>[/*]": {"purposes"?: [..]}}, "triggers": ["<spiffe id>[/*]"]}}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// Agents that may request Txn-Tokens for people (entry with a person's token).
    pub callers: Entries,
    /// Entry agents that may run on grants (ADR-0111 stage 3).
    pub grant_agents: Entries,
    /// Workloads that may start grant runs.
    pub triggers: BTreeSet<String>,
}

fn check_id(id: &str) -> Result<(), String> {
    if !id.starts_with("spiffe://") {
        return Err(format!("'{id}' is not a SPIFFE ID (spiffe://…)"));
    }
    Ok(())
}

/// The entries of one section (`{"<id>": {"purposes"?: [..]}}`).
fn parse_entries(section: &str, value: &Value) -> Result<Entries, String> {
    let object = value.as_object().ok_or_else(|| {
        format!("needs an object '{section}' (SPIFFE ID to {{ \"purposes\"?: [...] }})")
    })?;
    let mut entries = Entries::new();
    for (id, entry) in object {
        check_id(id)?;
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("the entry of {id} is not an object"))?;
        if let Some(other) = entry.keys().find(|key| *key != "purposes") {
            return Err(format!(
                "the entry of {id} has the unknown field '{other}' (only 'purposes')"
            ));
        }
        let purposes = match entry.get("purposes") {
            None | Some(Value::Null) => None,
            Some(Value::Array(list)) => Some(
                list.iter()
                    .map(|purpose| {
                        purpose
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("the purposes of {id} must be a list of texts"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Some(_) => return Err(format!("the purposes of {id} must be a list of texts")),
        };
        entries.insert(id.clone(), purposes);
    }
    Ok(entries)
}

/// Parses and checks a policy file (the same rules as the service, without its SPIFFE parser).
pub fn parse_policy(text: &str) -> Result<Policy, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("is not valid JSON: {error}"))?;
    let object = value.as_object().ok_or("is not a JSON object")?;
    if let Some(other) = object
        .keys()
        .find(|key| *key != "callers" && *key != "grants")
    {
        return Err(format!(
            "has the unknown field '{other}' (only 'callers' and 'grants')"
        ));
    }
    let mut policy = Policy {
        callers: match object.get("callers") {
            Some(callers) => parse_entries("callers", callers)?,
            None => Entries::new(),
        },
        ..Policy::default()
    };
    if let Some(grants) = object.get("grants") {
        let grants = grants
            .as_object()
            .ok_or("needs an object 'grants' ({\"agents\": {..}, \"triggers\": [..]})")?;
        if let Some(other) = grants
            .keys()
            .find(|key| *key != "agents" && *key != "triggers")
        {
            return Err(format!(
                "has the unknown field 'grants.{other}' (only 'agents' and 'triggers')"
            ));
        }
        if let Some(agents) = grants.get("agents") {
            policy.grant_agents = parse_entries("grants.agents", agents)?;
        }
        if let Some(triggers) = grants.get("triggers") {
            for trigger in triggers
                .as_array()
                .ok_or("grants.triggers must be a list of SPIFFE IDs")?
            {
                let trigger = trigger
                    .as_str()
                    .ok_or("grants.triggers must be a list of SPIFFE IDs")?;
                check_id(trigger)?;
                policy.triggers.insert(trigger.to_owned());
            }
        }
    }
    Ok(policy)
}

fn entries_value(entries: &Entries) -> Value {
    entries
        .iter()
        .map(|(id, purposes)| {
            let entry = match purposes {
                Some(list) => json!({ "purposes": list }),
                None => json!({}),
            };
            (id.clone(), entry)
        })
        .collect::<Map<String, Value>>()
        .into()
}

pub fn policy_text(policy: &Policy) -> String {
    let mut file = json!({ "callers": entries_value(&policy.callers) });
    if !policy.grant_agents.is_empty() || !policy.triggers.is_empty() {
        file["grants"] = json!({
            "agents": entries_value(&policy.grant_agents),
            "triggers": policy.triggers,
        });
    }
    let mut text =
        serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{\"callers\": {}}".to_owned());
    text.push('\n');
    text
}

fn policy_object(policy: &Policy) -> Value {
    json!({"apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": POLICY_MAP, "namespace": NAMESPACE, "labels": {MANAGED_BY: MANAGER}},
        "data": {POLICY_KEY: policy_text(policy)}})
}

/// The policy in the cluster; `None` without the ConfigMap, an error if it does not parse.
pub async fn read_policy(cluster: &Cluster) -> Result<Option<Policy>, String> {
    let Some(object) = cluster
        .get(&kinds::CONFIG_MAP, Some(NAMESPACE), POLICY_MAP)
        .await
        .map_err(|error| format!("ConfigMap {NAMESPACE}/{POLICY_MAP}: {error}"))?
    else {
        return Ok(None);
    };
    let text = object["data"][POLICY_KEY]
        .as_str()
        .ok_or_else(|| format!("ConfigMap {NAMESPACE}/{POLICY_MAP} has no key {POLICY_KEY}"))?;
    parse_policy(text)
        .map(Some)
        .map_err(|error| format!("ConfigMap {NAMESPACE}/{POLICY_MAP}, {POLICY_KEY} {error}"))
}

fn namespace_object() -> Value {
    json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE, "labels": {
        COMPONENT_LABEL: COMPONENT,
        MANAGED_BY: MANAGER,
        "pod-security.kubernetes.io/enforce": "restricted",
    }}})
}

fn pod_labels() -> Value {
    json!({"app.kubernetes.io/name": "grauwolf", AGENT_LABEL: NAME})
}

fn object_labels() -> Value {
    json!({"app.kubernetes.io/name": "grauwolf", AGENT_LABEL: NAME, COMPONENT_LABEL: COMPONENT,
        MANAGED_BY: MANAGER})
}

fn cluster_spiffe_id_object() -> Value {
    json!({"apiVersion": "spire.spiffe.io/v1alpha1", "kind": "ClusterSPIFFEID",
        "metadata": {"name": NAME, "labels": {MANAGED_BY: MANAGER}},
        "spec": {"spiffeIDTemplate": SPIFFE_ID_TEMPLATE,
            "podSelector": {"matchLabels": {AGENT_LABEL: NAME}},
            "namespaceSelector": {"matchLabels": {NAMESPACE_NAME_LABEL: NAMESPACE}}}})
}

/// An agent (with `--grants` for the grants section) or `--trigger`, never both.
fn policy_target(
    agent: Option<String>,
    grants: bool,
    trigger: Option<String>,
) -> Result<PolicyTarget, String> {
    match (agent, trigger) {
        (Some(name), None) => Ok(PolicyTarget::Agent { name, grants }),
        (None, Some(trigger)) if !grants => Ok(PolicyTarget::Trigger(trigger)),
        (None, Some(_)) => Err("--grants belongs to an agent, not to --trigger".into()),
        (Some(_), Some(_)) => Err("name an agent or --trigger, not both".into()),
        (None, None) => Err("name an agent or --trigger".into()),
    }
}

fn deployment_object(services: &Services, options: &InstallOptions, public_url: &str) -> Value {
    let settings = &services.settings;
    let mut env = vec![
        (
            "SPIFFE_ENDPOINT_SOCKET",
            "unix:///spiffe-workload-api/agent.sock".to_owned(),
        ),
        // The identity may be issued a while after the pod starts (ADR-0120).
        ("SPIFFE_FETCH_TIMEOUT", "2m".to_owned()),
        ("GRAUWOLF_IDENTITY_TENANT", options.tenant.clone()),
        ("GRAUWOLF_IDENTITY_ISSUER", options.issuer.clone()),
        ("GRAUWOLF_IDENTITY_AUDIENCE", options.audience.join(",")),
        ("GRAUWOLF_IDENTITY_SCOPE", options.scope.clone()),
        ("GRAUWOLF_IDENTITY_PUBLIC_URL", public_url.to_owned()),
        (
            "GRAUWOLF_IDENTITY_POLICY",
            format!("{POLICY_DIR}/{POLICY_KEY}"),
        ),
        (TRANSIT_KEY_VARIABLE, options.transit_key.clone()),
        ("GRAUWOLF_IDENTITY_TRANSIT_MOUNT", TRANSIT_MOUNT.to_owned()),
        ("GRAUWOLF_IDENTITY_OPENBAO_ADDR", settings.agent_address()),
        ("GRAUWOLF_IDENTITY_OPENBAO_ROLE", NAME.to_owned()),
        (
            "GRAUWOLF_IDENTITY_OPENBAO_AUDIENCE",
            settings.audience.clone(),
        ),
        (
            "GRAUWOLF_IDENTITY_OPENBAO_MOUNT",
            settings.jwt_mount.clone(),
        ),
    ];
    if let Some(url) = &options.jwks_url {
        env.push(("GRAUWOLF_IDENTITY_JWKS_URL", url.clone()));
    }
    if let Some(client_id) = &options.grant_client_id {
        env.push(("GRAUWOLF_IDENTITY_GRANT_CLIENT_ID", client_id.clone()));
        env.push((
            "GRAUWOLF_IDENTITY_GRANT_KV_MOUNT",
            settings.kv_mount.clone(),
        ));
    }
    let env: Vec<Value> = env
        .into_iter()
        .map(|(name, value)| json!({"name": name, "value": value}))
        .collect();
    let probe = |period: u64, failures: u64| {
        json!({"httpGet": {"path": "/health", "port": "http"},
            "periodSeconds": period, "failureThreshold": failures, "timeoutSeconds": 3})
    };
    let pod = json!({
        "serviceAccountName": NAME,
        "automountServiceAccountToken": false,
        "securityContext": {"runAsNonRoot": true, "runAsUser": 65532, "runAsGroup": 65532,
            "fsGroup": 65532, "seccompProfile": {"type": "RuntimeDefault"}},
        "containers": [{
            "name": "grauwolf-identity",
            "image": options.image,
            "command": ["grauwolf", "identity", "serve",
                "--port", options.port.to_string(), "--mtls-port", options.mtls_port.to_string()],
            "ports": [{"name": "http", "containerPort": options.port},
                {"name": "mtls", "containerPort": options.mtls_port}],
            "securityContext": {"allowPrivilegeEscalation": false,
                "capabilities": {"drop": ["ALL"]}},
            "env": env,
            "volumeMounts": [
                {"name": "spiffe-workload-api", "mountPath": "/spiffe-workload-api", "readOnly": true},
                // A directory, never `subPath`: the service re-reads the policy when it changes.
                {"name": "policy", "mountPath": POLICY_DIR, "readOnly": true}],
            "startupProbe": probe(5, 60),
            "readinessProbe": probe(5, 3),
            "livenessProbe": probe(10, 6),
        }],
        "volumes": [
            {"name": "spiffe-workload-api", "csi": {"driver": "csi.spiffe.io", "readOnly": true}},
            {"name": "policy", "configMap": {"name": POLICY_MAP}}],
    });
    // Not the policy: the service reads it again by itself.
    let hash = hash_of(&pod);
    json!({"apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": {"name": NAME, "namespace": NAMESPACE, "labels": object_labels()},
        "spec": {"replicas": 1, "strategy": {"type": "Recreate"},
            "selector": {"matchLabels": pod_labels()},
            "template": {"metadata": {"labels": pod_labels(),
                    "annotations": {"grauwolf.io/config-hash": hash}},
                "spec": pod}}})
}

fn service_object(port: u16, mtls_port: u16) -> Value {
    json!({"apiVersion": "v1", "kind": "Service",
        "metadata": {"name": NAME, "namespace": NAMESPACE, "labels": object_labels()},
        "spec": {"type": "ClusterIP", "selector": pod_labels(),
            "ports": [{"name": "http", "port": port, "targetPort": "http"},
                {"name": "mtls", "port": mtls_port, "targetPort": "mtls"}]}})
}

/// The HTTP port (health, keys, configuration; JWT-SVID callers) from anywhere, like an agent's;
/// the mTLS port from the namespaces of agents (`grauwolf.io/agent`). Who gets a token for a
/// person is the policy's decision; renewals come from any agent of the chain.
pub fn ingress_policy_object(port: u16, mtls_port: u16) -> Value {
    json!({"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
        "metadata": {"name": INGRESS_POLICY, "namespace": NAMESPACE,
            "labels": {MANAGED_BY: MANAGER, AGENT_LABEL: NAME}},
        "spec": {"podSelector": {"matchLabels": {AGENT_LABEL: NAME}},
            "policyTypes": ["Ingress"],
            "ingress": [
                {"ports": [{"protocol": "TCP", "port": port}]},
                {"from": [{"namespaceSelector": {"matchExpressions": [
                    {"key": AGENT_LABEL, "operator": "Exists"}]}}],
                 "ports": [{"protocol": "TCP", "port": mtls_port}]}]}})
}

fn destination(url: &str, purpose: &str, pointer: &str) -> Option<Destination> {
    let (scheme, host, port) = url_parts(url)?;
    Some(Destination {
        scheme: Some(scheme),
        host: Some(host),
        port: Some(port),
        any_port: false,
        purpose: purpose.to_owned(),
        pointer: pointer.to_owned(),
        phase: Phase::Run,
        from_env: None,
    })
}

/// The identity service as a destination of an agent's egress policy.
pub fn destination_of(url: &str) -> Option<Destination> {
    destination(url, "the identity service (delegation)", "--identity")
}

/// What the service reaches: OpenBao and the keys of the people's tokens (the JWKS URL, else the
/// issuer's OpenID discovery). DNS is always allowed; the SPIRE agent is a socket of the CSI
/// volume, not a network destination.
fn destinations(services: &Services, options: &InstallOptions) -> Vec<Destination> {
    let keys = options.jwks_url.as_deref().unwrap_or(&options.issuer);
    [
        destination(
            &services.settings.agent_address(),
            "OpenBao (Transit signing, login)",
            "--openbao-service",
        ),
        destination(
            keys,
            "the keys of people's tokens",
            if options.jwks_url.is_some() {
                "--jwks-url"
            } else {
                "--issuer"
            },
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The Transit key named in the Deployment, else the default.
async fn installed_transit_key(cluster: &Cluster) -> Option<String> {
    let deployment = cluster
        .get(&kinds::DEPLOYMENT, Some(NAMESPACE), NAME)
        .await
        .ok()??;
    env_value(&deployment, TRANSIT_KEY_VARIABLE)
}

fn env_value(deployment: &Value, name: &str) -> Option<String> {
    deployment["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == name)?["value"]
        .as_str()
        .map(str::to_owned)
}

/// The OpenBao policy of the service: sign with the key, read its public keys, and keep the
/// grants (ADR-0111 stage 3) – exactly the two KV folders `identity/grants/` and
/// `identity/grant-tokens/` of `kv_mount`, nothing else.
pub fn openbao_policy(transit_key: &str, kv_mount: &str) -> String {
    let kv = kv_mount.trim_matches('/');
    let mut policy = format!(
        "path \"{TRANSIT_MOUNT}/sign/{transit_key}\" {{\n  capabilities = [\"update\"]\n}}\n\npath \"{TRANSIT_MOUNT}/keys/{transit_key}\" {{\n  capabilities = [\"read\"]\n}}\n"
    );
    for folder in [GRANTS_FOLDER, GRANT_TOKENS_FOLDER] {
        policy.push_str(&format!(
            "\npath \"{kv}/data/{folder}/*\" {{\n  capabilities = [\"create\", \"read\", \"update\"]\n}}\n\npath \"{kv}/metadata/{folder}/*\" {{\n  capabilities = [\"read\", \"list\", \"delete\"]\n}}\n"
        ));
    }
    policy.push_str(&format!(
        "\npath \"{kv}/metadata/{GRANTS_FOLDER}\" {{\n  capabilities = [\"list\"]\n}}\n"
    ));
    policy
}

fn check_name(option: &str, value: &str) -> Result<(), String> {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(format!(
            "{option} '{value}' may hold only letters, digits, '-' and '_'"
        ))
    }
}

async fn install(
    services: &Services,
    io: &mut Io<'_>,
    options: InstallOptions,
) -> Result<i32, String> {
    let cluster = &services.cluster;
    let settings = &services.settings;
    let bao = services.admin()?;

    // Refuse before anything is written.
    if options.image.trim().is_empty() {
        return Err("--image must not be empty".to_owned());
    }
    for (option, value) in [("--tenant", &options.tenant), ("--scope", &options.scope)] {
        if value.trim().is_empty() || value.contains(char::is_whitespace) {
            return Err(format!("{option} must be one word, not '{value}'"));
        }
    }
    for audience in &options.audience {
        if audience.trim().is_empty() || audience.contains(char::is_whitespace) {
            return Err(format!("--audience must be one word, not '{audience}'"));
        }
    }
    check_url("--issuer", &options.issuer)?;
    if let Some(url) = &options.jwks_url {
        check_url("--jwks-url", url)?;
    }
    if let Some(url) = &options.public_url {
        check_url("--public-url", url)?;
    }
    check_name("--transit-key", &options.transit_key)?;
    if options.port == 0 || options.mtls_port == 0 || options.port == options.mtls_port {
        return Err("--port and --mtls-port must be two different ports, not 0".to_owned());
    }
    let new_policy = match &options.policy {
        Some(file) => Some(read_policy_file(file)?),
        None => None,
    };
    match bao
        .auth_methods()
        .await
        .map_err(|error| error.to_string())?
        .get(settings.jwt_mount.as_str())
    {
        Some(kind) if kind == "jwt" => {}
        _ => {
            return Err(format!(
                "the JWT auth method {}/ is not enabled in OpenBao: run `grauwolf-k8-ops openbao setup` first",
                settings.jwt_mount
            ));
        }
    }
    if let Some(existing) = cluster
        .get(&kinds::NAMESPACE, None, NAMESPACE)
        .await
        .map_err(|error| error.to_string())?
        && existing["metadata"]["labels"][COMPONENT_LABEL].as_str() != Some(COMPONENT)
    {
        return Err(format!(
            "namespace {NAMESPACE} exists and was not made by `identity install` (label {COMPONENT_LABEL}={COMPONENT}); remove or rename it first"
        ));
    }
    let trust_domain = spire::trust_domain(cluster, settings).await?;
    let subject = spiffe_id(&trust_domain);
    let public_url = options
        .public_url
        .clone()
        .unwrap_or_else(|| mtls_url(options.mtls_port));
    io.line(&format!(
        "identity service: trust domain {trust_domain}; identity {subject}; image {}",
        options.image
    ));
    io.line(&format!(
        "people's tokens: tenant {}, issuer {}, audience {}, scope {}",
        options.tenant,
        options.issuer,
        options.audience.join(","),
        options.scope
    ));

    // OpenBao first, so the pod can log in as soon as it starts.
    let outcome = bao
        .ensure_transit(TRANSIT_MOUNT)
        .await
        .map_err(|error| format!("Transit mount {TRANSIT_MOUNT}: {error}"))?;
    io.line(&format!(
        "OpenBao Transit mount {TRANSIT_MOUNT}/: {}",
        report::bao(outcome)
    ));
    let outcome = bao
        .ensure_transit_key(TRANSIT_MOUNT, &options.transit_key, KEY_TYPE)
        .await
        .map_err(|error| format!("Transit key {}: {error}", options.transit_key))?;
    io.line(&format!(
        "OpenBao Transit key {TRANSIT_MOUNT}/keys/{} ({KEY_TYPE}): {}",
        options.transit_key,
        report::bao(outcome)
    ));
    let outcome = bao
        .ensure_policy(
            NAME,
            &openbao_policy(&options.transit_key, &services.settings.kv_mount),
        )
        .await
        .map_err(|error| format!("OpenBao policy {NAME}: {error}"))?;
    io.line(&format!(
        "OpenBao policy {NAME} (signs with {TRANSIT_MOUNT}/sign/{}, reads its public keys): {}",
        options.transit_key,
        report::bao(outcome)
    ));
    let outcome = bao
        .ensure_jwt_role(
            &settings.jwt_mount,
            NAME,
            &jwt_role(&settings.audience, &subject, NAME),
        )
        .await
        .map_err(|error| format!("OpenBao role {NAME}: {error}"))?;
    io.line(&format!(
        "OpenBao role auth/{}/role/{NAME} (subject {subject}): {}",
        settings.jwt_mount,
        report::bao(outcome)
    ));

    let outcome = cluster
        .ensure(&kinds::NAMESPACE, None, &namespace_object())
        .await
        .map_err(|error| format!("namespace {NAMESPACE}: {error}"))?;
    io.line(&format!(
        "namespace {NAMESPACE} (restricted Pod Security Standard): {}",
        report::k8s(outcome)
    ));
    let pull_name = match &options.pull_secret {
        Some(reference) => Some(agent::copy_pull_secret(cluster, io, reference, NAMESPACE).await?),
        None => None,
    };
    let account = agent::service_account_object(NAME, NAMESPACE, pull_name.as_deref());
    let outcome = cluster
        .ensure(&kinds::SERVICE_ACCOUNT, Some(NAMESPACE), &account)
        .await
        .map_err(|error| format!("service account {NAME}: {error}"))?;
    io.line(&format!(
        "service account {NAMESPACE}/{NAME}: {}",
        report::k8s(outcome)
    ));
    let outcome = cluster
        .ensure(&kinds::CLUSTER_SPIFFE_ID, None, &cluster_spiffe_id_object())
        .await
        .map_err(|error| format!("ClusterSPIFFEID {NAME}: {error}"))?;
    io.line(&format!("ClusterSPIFFEID {NAME}: {}", report::k8s(outcome)));

    // The policy: replaced by --policy, else made empty if missing and never replaced
    // (`identity allow` writes it).
    let policy = match (new_policy, read_policy(cluster).await) {
        (Some(policy), _) => policy,
        (None, Ok(Some(policy))) => {
            io.line(&format!(
                "ConfigMap {NAMESPACE}/{POLICY_MAP}: kept ({} caller(s); --policy replaces it)",
                policy.callers.len()
            ));
            policy
        }
        (None, Ok(None)) => Policy::default(),
        (None, Err(error)) => return Err(error),
    };
    if options.policy.is_some()
        || cluster
            .get(&kinds::CONFIG_MAP, Some(NAMESPACE), POLICY_MAP)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
    {
        let outcome = cluster
            .ensure(&kinds::CONFIG_MAP, Some(NAMESPACE), &policy_object(&policy))
            .await
            .map_err(|error| format!("ConfigMap {POLICY_MAP}: {error}"))?;
        io.line(&format!(
            "ConfigMap {NAMESPACE}/{POLICY_MAP} ({} caller(s)): {}",
            policy.callers.len(),
            report::k8s(outcome)
        ));
    }

    let outcome = cluster
        .ensure(
            &kinds::DEPLOYMENT,
            Some(NAMESPACE),
            &deployment_object(services, &options, &public_url),
        )
        .await
        .map_err(|error| format!("Deployment {NAME}: {error}"))?;
    io.line(&format!(
        "Deployment {NAMESPACE}/{NAME}: {}",
        report::k8s(outcome)
    ));
    let outcome = cluster
        .ensure(
            &kinds::SERVICE,
            Some(NAMESPACE),
            &service_object(options.port, options.mtls_port),
        )
        .await
        .map_err(|error| format!("Service {NAME}: {error}"))?;
    io.line(&format!(
        "Service {NAMESPACE}/{NAME} (ports {}, {} mTLS): {}",
        options.port,
        options.mtls_port,
        report::k8s(outcome)
    ));
    ensure_policy_object(
        cluster,
        io,
        &ingress_policy_object(options.port, options.mtls_port),
        &format!(
            "port {} from the agents' namespaces; port {} from anywhere",
            options.mtls_port, options.port
        ),
    )
    .await?;
    let plan = egress::plan(
        cluster,
        &services.resolver,
        NAME,
        NAMESPACE,
        &destinations(services, &options),
        &options.egress,
    )
    .await;
    egress::sync(cluster, io, NAMESPACE, &plan).await?;

    io.line(&format!(
        "waiting for the rollout and /health (at most {} s)",
        options.timeout
    ));
    match deploy::wait_ready(
        cluster,
        NAMESPACE,
        NAME,
        options.port,
        Duration::from_secs(options.timeout),
    )
    .await?
    {
        deploy::Waited::Ready => {
            io.line("identity service: ready, /health answers ok");
            io.line(&format!(
                "mTLS endpoint {} (agents: `grauwolf-k8-ops agent deploy <agent> --identity`)",
                mtls_url(options.mtls_port)
            ));
            io.line(&format!(
                "keys {}/api/identity/jwks.json",
                http_url(options.port)
            ));
            if policy.callers.is_empty() && policy.grant_agents.is_empty() {
                io.line(
                    "no agent may act on behalf of people yet: `grauwolf-k8-ops identity allow <agent>`",
                );
            }
            Ok(0)
        }
        deploy::Waited::Failed(reason) => {
            deploy::report_failure_with(
                cluster,
                io,
                NAMESPACE,
                NAME,
                &reason,
                "next: `grauwolf-k8-ops identity status`, `kubectl -n identity logs deploy/grauwolf-identity`",
            )
            .await;
            Ok(1)
        }
    }
}

fn read_policy_file(file: &Path) -> Result<Policy, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| format!("--policy {}: {error}", file.display()))?;
    parse_policy(&text).map_err(|error| format!("--policy {} {error}", file.display()))
}

/// Ensures a NetworkPolicy and patches it back when it was changed by hand.
async fn ensure_policy_object(
    cluster: &Cluster,
    io: &mut Io<'_>,
    desired: &Value,
    what: &str,
) -> Result<(), String> {
    let name = desired["metadata"]["name"].as_str().unwrap_or_default();
    let fail = |error: grauwolf_k8s::K8sError| format!("NetworkPolicy {name}: {error}");
    let mut outcome = cluster
        .ensure(&kinds::NETWORK_POLICY, Some(NAMESPACE), desired)
        .await
        .map_err(fail)?;
    if outcome == grauwolf_k8s::Outcome::Unchanged
        && cluster
            .get(&kinds::NETWORK_POLICY, Some(NAMESPACE), name)
            .await
            .map_err(fail)?
            .is_some_and(|existing| existing["spec"] != desired["spec"])
    {
        cluster
            .patch(
                &kinds::NETWORK_POLICY,
                Some(NAMESPACE),
                name,
                &json!({"spec": desired["spec"]}),
            )
            .await
            .map_err(fail)?;
        outcome = grauwolf_k8s::Outcome::Updated;
    }
    io.line(&format!(
        "NetworkPolicy {NAMESPACE}/{name} ({what}): {}",
        report::k8s(outcome)
    ));
    Ok(())
}

/// What `agent deploy --identity` needs: `(GRAUWOLF_IDENTITY_URL, GRAUWOLF_IDENTITY_SPIFFE_ID)`,
/// or what to do first.
pub async fn for_agents(services: &Services) -> Result<(String, String), String> {
    let cluster = &services.cluster;
    let service = cluster
        .get(&kinds::SERVICE, Some(NAMESPACE), NAME)
        .await
        .map_err(|error| format!("Service {NAMESPACE}/{NAME}: {error}"))?
        .ok_or_else(|| {
            "--identity needs the identity service: run `grauwolf-k8-ops identity install` first"
                .to_owned()
        })?;
    let mtls_port = service["spec"]["ports"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|port| port["name"] == "mtls")
        .and_then(|port| port["port"].as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or(DEFAULT_MTLS_PORT);
    let trust_domain = spire::trust_domain(cluster, &services.settings).await?;
    Ok((mtls_url(mtls_port), spiffe_id(&trust_domain)))
}

/// Whether the identity service is installed and ready: `None` if there is no Deployment, else
/// `(ready, image, why not)`.
pub async fn readiness(cluster: &Cluster) -> Result<Option<(bool, String, String)>, String> {
    Ok(cluster
        .get(&kinds::DEPLOYMENT, Some(NAMESPACE), NAME)
        .await
        .map_err(|error| format!("Deployment {NAMESPACE}/{NAME}: {error}"))?
        .map(|deployment| deploy::readiness(&deployment)))
}

/// The identity URL a deployed agent's pod is given, if any.
pub fn url_of(deployment: &Value) -> Option<String> {
    env_value(deployment, URL_VARIABLE)
}

async fn status(services: &Services, io: &mut Io<'_>, json: bool) -> Result<(), String> {
    let cluster = &services.cluster;
    let settings = &services.settings;
    let deployment = cluster
        .get(&kinds::DEPLOYMENT, Some(NAMESPACE), NAME)
        .await
        .map_err(|error| format!("Deployment {NAME}: {error}"))?;
    let Some(deployment) = deployment else {
        if json {
            io.line(&json!({"installed": false}).to_string());
        } else {
            io.line(
                "identity service: not installed (`grauwolf-k8-ops identity install --tenant … --issuer … --audience … --image …`)",
            );
        }
        return Ok(());
    };
    let (_, image, _) = deploy::readiness(&deployment);
    let pod = deploy::current_pod(cluster, NAMESPACE, NAME)
        .await?
        .map(|pod| deploy::pod_info(&pod));
    let ready = deploy::rolled_out(&deployment) && pod.as_ref().is_some_and(|pod| pod.ready);
    let service = cluster
        .get(&kinds::SERVICE, Some(NAMESPACE), NAME)
        .await
        .map_err(|error| format!("Service {NAME}: {error}"))?;
    let port_of = |name: &str| {
        service.as_ref().and_then(|service| {
            service["spec"]["ports"]
                .as_array()?
                .iter()
                .find(|port| port["name"] == name)?["port"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
        })
    };
    let endpoint = port_of("http").map(http_url);
    let mtls_endpoint = port_of("mtls").map(mtls_url);
    let trust_domain = spire::trust_domain(cluster, settings).await.ok();
    let id = trust_domain.as_deref().map(spiffe_id);
    let config: BTreeMap<&str, Option<String>> = [
        ("tenant", "GRAUWOLF_IDENTITY_TENANT"),
        ("issuer", "GRAUWOLF_IDENTITY_ISSUER"),
        ("audience", "GRAUWOLF_IDENTITY_AUDIENCE"),
        ("scope", "GRAUWOLF_IDENTITY_SCOPE"),
        ("jwks_url", "GRAUWOLF_IDENTITY_JWKS_URL"),
        ("public_url", "GRAUWOLF_IDENTITY_PUBLIC_URL"),
        ("transit_key", TRANSIT_KEY_VARIABLE),
    ]
    .into_iter()
    .map(|(key, variable)| (key, env_value(&deployment, variable)))
    .collect();
    let policy = read_policy(cluster).await?.unwrap_or_default();
    let known = agent::agents(cluster).await?;
    let callers: Vec<Value> = policy
        .callers
        .iter()
        .map(|(caller, purposes)| {
            let name = caller.rsplit_once("/sa/").map(|(_, sa)| sa).filter(|sa| {
                known.iter().any(|(agent, namespace)| {
                    agent == sa && caller.contains(&format!("/ns/{namespace}/sa/"))
                })
            });
            json!({"id": caller, "purposes": purposes, "agent": name})
        })
        .collect();
    let key = config["transit_key"]
        .clone()
        .unwrap_or_else(|| DEFAULT_TRANSIT_KEY.to_owned());
    let openbao = match services.admin() {
        Ok(bao) => {
            let role = bao
                .read_jwt_role(&settings.jwt_mount, NAME)
                .await
                .ok()
                .map(|role| role.is_some());
            let bao_policy = bao.read_policy(NAME).await.ok().map(|p| p.is_some());
            let key_type = bao.transit_key_type(TRANSIT_MOUNT, &key).await.ok();
            Some(json!({"role": role, "policy": bao_policy,
                "transit_key": key_type.map(|kind| kind.unwrap_or_else(|| "missing".to_owned()))}))
        }
        Err(_) => None,
    };
    if json {
        io.line(
            &json!({"installed": true, "ready": ready, "namespace": NAMESPACE, "image": image,
                "pod": pod.as_ref().map(|pod| json!({"name": pod.name, "phase": pod.phase,
                    "ready": pod.ready, "restarts": pod.restarts, "waiting_reason": pod.waiting})),
                "spiffe_id": id, "endpoint": endpoint, "mtls_endpoint": mtls_endpoint,
                "config": config, "callers": callers, "openbao": openbao,
                "grants": {"agents": entries_value(&policy.grant_agents),
                    "triggers": policy.triggers}})
            .to_string(),
        );
        return Ok(());
    }
    io.line(&format!(
        "identity service (namespace {NAMESPACE}): installed, {}",
        if ready { "ready" } else { "not ready" }
    ));
    match &pod {
        Some(pod) => io.line(&format!(
            "pod {}: {}, {}ready, {} restart(s){}",
            pod.name,
            pod.phase,
            if pod.ready { "" } else { "not " },
            pod.restarts,
            pod.waiting
                .as_ref()
                .map(|reason| format!(", waiting: {reason}"))
                .unwrap_or_default()
        )),
        None => io.line("pod: none"),
    }
    io.line(&format!("image {image}"));
    if let Some(id) = &id {
        io.line(&format!("SPIFFE ID {id}"));
    }
    if let Some(endpoint) = &mtls_endpoint {
        io.line(&format!(
            "mTLS endpoint {endpoint} (GRAUWOLF_IDENTITY_URL of the agents)"
        ));
    }
    if let Some(endpoint) = &endpoint {
        io.line(&format!("keys {endpoint}/api/identity/jwks.json"));
    }
    for (key, value) in &config {
        if let Some(value) = value {
            io.line(&format!("{key} {value}"));
        }
    }
    io.line(&format!(
        "policy: {} caller(s) may act on behalf of people",
        callers.len()
    ));
    for caller in &callers {
        let purposes = match caller["purposes"].as_array() {
            Some(list) => list
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            None => "any".to_owned(),
        };
        io.line(&format!(
            "  {}  purposes: {purposes}  {}",
            caller["id"].as_str().unwrap_or(""),
            match caller["agent"].as_str() {
                Some(name) => format!("agent {name}"),
                None => "no such agent".to_owned(),
            }
        ));
    }
    io.line(&format!(
        "grants: {} agent(s) may run on grants, {} trigger(s) may start them",
        policy.grant_agents.len(),
        policy.triggers.len()
    ));
    for (id, purposes) in &policy.grant_agents {
        io.line(&format!(
            "  agent {id}  purposes: {}",
            purposes
                .as_ref()
                .map(|list| list.join(", "))
                .unwrap_or_else(|| "any".to_owned())
        ));
    }
    for trigger in &policy.triggers {
        io.line(&format!("  trigger {trigger}"));
    }
    match &openbao {
        Some(state) => {
            let word = |value: &Value| match value.as_bool() {
                Some(true) => "exists",
                Some(false) => "missing",
                None => "unknown",
            };
            io.line(&format!(
                "OpenBao role {NAME}: {}, policy {NAME}: {}, Transit key {TRANSIT_MOUNT}/keys/{key}: {}",
                word(&state["role"]),
                word(&state["policy"]),
                state["transit_key"].as_str().unwrap_or("unknown")
            ));
        }
        None => io.line("OpenBao: not checked (no token)"),
    }
    Ok(())
}

async fn uninstall(
    services: &Services,
    io: &mut Io<'_>,
    delete_key: bool,
    yes: bool,
) -> Result<(), String> {
    let cluster = &services.cluster;
    let settings = &services.settings;
    let bao = services.admin()?;
    let namespace = cluster
        .get(&kinds::NAMESPACE, None, NAMESPACE)
        .await
        .map_err(|error| error.to_string())?;
    let ours = namespace.as_ref().is_some_and(|namespace| {
        namespace["metadata"]["labels"][COMPONENT_LABEL].as_str() == Some(COMPONENT)
    });
    let key = installed_transit_key(cluster)
        .await
        .unwrap_or_else(|| DEFAULT_TRANSIT_KEY.to_owned());
    let spiffe = cluster
        .get(&kinds::CLUSTER_SPIFFE_ID, None, NAME)
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    let role = bao
        .read_jwt_role(&settings.jwt_mount, NAME)
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    let policy = bao
        .read_policy(NAME)
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    let key_exists = bao
        .transit_key_type(TRANSIT_MOUNT, &key)
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    if !ours && !spiffe && !role && !policy && !(delete_key && key_exists) {
        return Err("the identity service is not installed".to_owned());
    }
    // Agents that were deployed with --identity lose their identity service.
    let mut users = Vec::new();
    for (name, agent_ns) in agent::agents(cluster).await? {
        if let Ok(Some(deployment)) = cluster
            .get(&kinds::DEPLOYMENT, Some(&agent_ns), &name)
            .await
            && url_of(&deployment).is_some()
        {
            users.push(name);
        }
    }
    io.line("the identity service will be removed:");
    if namespace.is_some() && !ours {
        io.line(&format!(
            "  not the namespace {NAMESPACE}: it was not made by `identity install`"
        ));
    } else if ours {
        io.line(&format!(
            "  namespace {NAMESPACE} with everything in it (Deployment, Service, policy, NetworkPolicies)"
        ));
    }
    if spiffe {
        io.line(&format!("  ClusterSPIFFEID {NAME}"));
    }
    if role {
        io.line(&format!(
            "  OpenBao role auth/{}/role/{NAME}",
            settings.jwt_mount
        ));
    }
    if policy {
        io.line(&format!("  OpenBao policy {NAME}"));
    }
    if key_exists {
        if delete_key {
            io.line(&format!(
                "  OpenBao Transit key {TRANSIT_MOUNT}/keys/{key} (Txn-Tokens it signed can no longer be verified)"
            ));
        } else {
            io.line(&format!(
                "  kept: OpenBao Transit key {TRANSIT_MOUNT}/keys/{key} (use --delete-key to delete it)"
            ));
        }
    }
    if !users.is_empty() {
        io.error(&format!(
            "warning: deployed with --identity, these agents can no longer accept people's tokens: {} (deploy them again without --identity)",
            users.join(", ")
        ));
    }
    if !yes && !io.confirm() {
        return Err("cancelled, nothing was removed".to_owned());
    }
    if ours {
        cluster
            .delete(&kinds::NAMESPACE, None, NAMESPACE)
            .await
            .map_err(|error| format!("namespace {NAMESPACE}: {error}"))?;
        io.line(&format!("namespace {NAMESPACE}: deleted"));
    }
    if spiffe {
        cluster
            .delete(&kinds::CLUSTER_SPIFFE_ID, None, NAME)
            .await
            .map_err(|error| format!("ClusterSPIFFEID {NAME}: {error}"))?;
        io.line(&format!("ClusterSPIFFEID {NAME}: deleted"));
    }
    if role {
        bao.delete_jwt_role(&settings.jwt_mount, NAME)
            .await
            .map_err(|error| format!("OpenBao role {NAME}: {error}"))?;
        io.line(&format!("OpenBao role {NAME}: deleted"));
    }
    if policy {
        bao.delete_policy(NAME)
            .await
            .map_err(|error| format!("OpenBao policy {NAME}: {error}"))?;
        io.line(&format!("OpenBao policy {NAME}: deleted"));
    }
    if delete_key && key_exists {
        bao.delete_transit_key(TRANSIT_MOUNT, &key)
            .await
            .map_err(|error| format!("Transit key {key}: {error}"))?;
        io.line(&format!(
            "OpenBao Transit key {TRANSIT_MOUNT}/keys/{key}: deleted"
        ));
    }
    Ok(())
}

async fn installed_policy(cluster: &Cluster) -> Result<Policy, String> {
    read_policy(cluster).await?.ok_or_else(|| {
        "the identity service is not installed: run `grauwolf-k8-ops identity install` first"
            .to_owned()
    })
}

async fn write_policy(cluster: &Cluster, io: &mut Io<'_>, policy: &Policy) -> Result<(), String> {
    let outcome = cluster
        .ensure(&kinds::CONFIG_MAP, Some(NAMESPACE), &policy_object(policy))
        .await
        .map_err(|error| format!("ConfigMap {NAMESPACE}/{POLICY_MAP}: {error}"))?;
    io.line(&format!(
        "ConfigMap {NAMESPACE}/{POLICY_MAP}: {} (Kubernetes passes it to the pod within about a minute or two; the service then reads it again)",
        report::k8s(outcome)
    ));
    Ok(())
}

/// `grt_` and 32 lowercase hex digits.
fn is_grant_id(id: &str) -> bool {
    id.strip_prefix("grt_").is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// `2026-12-16 09:30 UTC` for Unix seconds.
fn date(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Days to civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// `identity grants [--revoke <id>] [--json]`: every person's grants from OpenBao (metadata only,
/// never a refresh token), or one revoked – metadata and refresh token deleted.
async fn grants(
    services: &Services,
    io: &mut Io<'_>,
    revoke: Option<String>,
    as_json: bool,
) -> Result<(), String> {
    let bao = services.admin()?;
    let kv = services.settings.kv_mount.as_str();
    if let Some(id) = revoke {
        if !is_grant_id(&id) {
            return Err(format!("'{id}' is not a grant ID (grt_ and 32 hex digits)"));
        }
        let (grant, token) = (
            format!("{GRANTS_FOLDER}/{id}"),
            format!("{GRANT_TOKENS_FOLDER}/{id}"),
        );
        let exists = bao
            .kv_current_version(kv, &grant)
            .await
            .map_err(|error| format!("OpenBao {kv}/{grant}: {error}"))?
            .is_some()
            || bao
                .kv_current_version(kv, &token)
                .await
                .map_err(|error| format!("OpenBao {kv}/{token}: {error}"))?
                .is_some();
        if !exists {
            return Err(format!("no grant {id} (`grauwolf-k8-ops identity grants`)"));
        }
        // The token first: a grant without a token can no longer run.
        for path in [&token, &grant] {
            bao.kv_delete_all(kv, path)
                .await
                .map_err(|error| format!("OpenBao {kv}/{path}: {error}"))?;
        }
        io.line(&format!(
            "grant {id}: revoked ({kv}/{grant} and the refresh token deleted)"
        ));
        return Ok(());
    }
    let mut grants = Vec::new();
    for id in bao
        .kv_list(kv, GRANTS_FOLDER)
        .await
        .map_err(|error| format!("OpenBao {kv}/{GRANTS_FOLDER}: {error}"))?
    {
        if !is_grant_id(&id) {
            continue;
        }
        if let Some(grant) = bao
            .kv_read(kv, &format!("{GRANTS_FOLDER}/{id}"))
            .await
            .map_err(|error| format!("OpenBao {kv}/{GRANTS_FOLDER}/{id}: {error}"))?
        {
            grants.push(grant);
        }
    }
    if as_json {
        io.line(&json!({ "grants": grants }).to_string());
        return Ok(());
    }
    if grants.is_empty() {
        io.line("no grants");
    }
    let text = |value: &Value| value.as_str().unwrap_or("-").to_owned();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    for grant in &grants {
        let owner = &grant["owner"];
        let who = match (owner["name"].as_str(), owner["upn"].as_str()) {
            (Some(name), Some(upn)) => format!("{name} <{upn}>"),
            (Some(name), None) => name.to_owned(),
            (None, Some(upn)) => upn.to_owned(),
            (None, None) => "oid".to_owned(),
        };
        let purposes = grant["purposes"]
            .as_array()
            .filter(|list| !list.is_empty())
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "-".to_owned());
        let valid_until = grant["valid_until"].as_u64().unwrap_or(0);
        io.line(&format!(
            "{}  owner {who} ({})  agent {}  trigger {}  purposes {purposes}  valid to {}{}  last used {}",
            text(&grant["id"]),
            text(&owner["oid"]),
            text(&grant["agent"]),
            text(&grant["trigger"]),
            date(valid_until),
            if valid_until <= now { " (expired)" } else { "" },
            grant["last_used"]
                .as_u64()
                .map(date)
                .unwrap_or_else(|| "never".to_owned())
        ));
    }
    Ok(())
}

/// What `identity allow|deny` changes.
pub enum PolicyTarget {
    /// An agent: in `callers`, or with `--grants` in `grants.agents`.
    Agent { name: String, grants: bool },
    /// A trigger of grant runs (`--trigger`): a schedule's name or a SPIFFE ID (`…/*` a prefix).
    Trigger(String),
}

/// The SPIFFE ID of a trigger: an ID as it is, a name as the schedule of that name.
async fn trigger_id(services: &Services, trigger: &str) -> Result<String, String> {
    if trigger.starts_with("spiffe://") {
        return Ok(trigger.to_owned());
    }
    agent::validate_name(trigger)?;
    let domain = spire::trust_domain(&services.cluster, &services.settings).await?;
    Ok(crate::schedule::spiffe_id(&domain, trigger))
}

/// `identity allow <agent> [--grants] [--purpose …]` / `identity allow --trigger <name or id>`:
/// the agent may request Txn-Tokens on behalf of people (with `--grants`: may run on grants) for
/// these purposes (without any, for every purpose); a trigger may start grant runs.
async fn allow(
    services: &Services,
    io: &mut Io<'_>,
    target: PolicyTarget,
    purposes: Vec<String>,
) -> Result<(), String> {
    let cluster = &services.cluster;
    let (name, grants) = match target {
        PolicyTarget::Trigger(trigger) => {
            if !purposes.is_empty() {
                return Err("--purpose belongs to an agent, not to --trigger".into());
            }
            let id = trigger_id(services, &trigger).await?;
            let mut policy = installed_policy(cluster).await?;
            let entry = if policy.triggers.insert(id.clone()) {
                "created"
            } else {
                "unchanged"
            };
            io.line(&format!(
                "identity service lets {id} trigger grant runs: {entry}"
            ));
            return write_policy(cluster, io, &policy).await;
        }
        PolicyTarget::Agent { name, grants } => (name, grants),
    };
    agent::validate_name(&name)?;
    let namespace = agent::find_namespace(cluster, &name)
        .await?
        .ok_or_else(|| {
            format!("agent {name} not found (create it with `grauwolf-k8-ops agent add {name}`)")
        })?;
    if let Some(bad) = purposes
        .iter()
        .find(|purpose| purpose.is_empty() || purpose.contains(char::is_whitespace))
    {
        return Err(format!("--purpose '{bad}' must be one word"));
    }
    let mut policy = installed_policy(cluster).await?;
    let domain = spire::trust_domain(cluster, &services.settings).await?;
    let id = spire::spiffe_id(&domain, &namespace, &name);
    let mut purposes = purposes;
    purposes.sort();
    purposes.dedup();
    let wanted = (!purposes.is_empty()).then_some(purposes);
    let section = match grants {
        true => &mut policy.grant_agents,
        false => &mut policy.callers,
    };
    let entry = match section.insert(id.clone(), wanted.clone()) {
        None => "created",
        Some(old) if old != wanted => "updated",
        Some(_) => "unchanged",
    };
    let what = match grants {
        true => "to run on grants of people",
        false => "to act on behalf of people",
    };
    io.line(&format!(
        "identity service allows {id} {what} (purposes: {}): {entry}",
        wanted
            .as_ref()
            .map(|list| list.join(", "))
            .unwrap_or_else(|| "any".to_owned())
    ));
    write_policy(cluster, io, &policy).await
}

/// `identity deny <agent> [--grants]` / `identity deny --trigger <name or id>`: takes the
/// permission back (also for an agent that was removed).
async fn deny(services: &Services, io: &mut Io<'_>, target: PolicyTarget) -> Result<(), String> {
    let cluster = &services.cluster;
    let (name, grants) = match target {
        PolicyTarget::Trigger(trigger) => {
            let id = trigger_id(services, &trigger).await?;
            let mut policy = installed_policy(cluster).await?;
            let entry = if policy.triggers.remove(&id) {
                "removed"
            } else {
                "unchanged"
            };
            io.line(&format!(
                "identity service no longer lets {id} trigger grant runs: {entry}"
            ));
            return write_policy(cluster, io, &policy).await;
        }
        PolicyTarget::Agent { name, grants } => (name, grants),
    };
    agent::validate_name(&name)?;
    let mut policy = installed_policy(cluster).await?;
    let section = match grants {
        true => &mut policy.grant_agents,
        false => &mut policy.callers,
    };
    let ids: Vec<String> = match agent::find_namespace(cluster, &name).await? {
        Some(namespace) => {
            let domain = spire::trust_domain(cluster, &services.settings).await?;
            vec![spire::spiffe_id(&domain, &namespace, &name)]
        }
        None => section
            .keys()
            .filter(|id| id.rsplit_once("/sa/").is_some_and(|(_, sa)| sa == name))
            .cloned()
            .collect(),
    };
    let what = match grants {
        true => " to run on grants",
        false => "",
    };
    let mut removed = false;
    for id in &ids {
        if section.remove(id).is_some() {
            removed = true;
            io.line(&format!(
                "identity service no longer allows {id}{what}: removed"
            ));
        }
    }
    if !removed {
        io.line(&format!(
            "identity service does not allow {name}{what}: unchanged"
        ));
    }
    write_policy(cluster, io, &policy).await
}

/// A hint for an agent deployed with `--identity` whose identity service is missing or not
/// ready (`agent check`); `None` when all is well or the agent does not use it.
pub async fn hint_for(cluster: &Cluster, namespace: &str, agent: &str) -> Option<String> {
    let deployment = cluster
        .get(&kinds::DEPLOYMENT, Some(namespace), agent)
        .await
        .ok()??;
    url_of(&deployment)?;
    match readiness(cluster).await {
        Ok(Some((true, ..))) => None,
        Ok(Some((false, _, why))) => Some(format!(
            "agent {agent} is deployed with --identity and the identity service is not ready ({why}): `grauwolf-k8-ops identity status`"
        )),
        Ok(None) => Some(format!(
            "agent {agent} is deployed with --identity, but the identity service is not installed: people's tokens get 503 until `grauwolf-k8-ops identity install`"
        )),
        Err(error) => Some(format!("the identity service cannot be read: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_split_into_scheme_host_and_port() {
        assert_eq!(
            url_parts("https://entra.example.com/t/discovery/v2.0/keys"),
            Some(("https".into(), "entra.example.com".into(), 443))
        );
        assert_eq!(
            url_parts("http://openbao.openbao.svc:8200"),
            Some(("http".into(), "openbao.openbao.svc".into(), 8200))
        );
        assert_eq!(url_parts("ftp://x"), None);
        assert_eq!(url_parts("https://"), None);
    }

    #[test]
    fn a_policy_round_trips_and_unknown_fields_are_refused() {
        let text = r#"{"callers": {"spiffe://d/ns/a/sa/a": {"purposes": ["review"]}, "spiffe://d/ns/b/*": {}}}"#;
        let policy = parse_policy(text).unwrap();
        assert_eq!(parse_policy(&policy_text(&policy)).unwrap(), policy);
        assert!(parse_policy(r#"{"callers": {"x": {}}}"#).is_err());
        assert!(parse_policy(r#"{"callers": {}, "extra": 1}"#).is_err());
        assert!(parse_policy(r#"{"callers": {"spiffe://d/x": {"purpose": []}}}"#).is_err());
    }
}
