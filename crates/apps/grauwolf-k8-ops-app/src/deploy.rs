//! `agent deploy | status | logs | undeploy`: running an agent in its namespace (ADR-0121).
//!
//! One Deployment (one replica, strategy `Recreate`), a ClusterIP Service, optionally an Ingress,
//! the agent's files in a ConfigMap (with `--spec`) and a PVC (with `--data-size`). The pod follows
//! ADR-0120: the restricted Pod Security Standard, the SPIFFE CSI volume, the agent's
//! `grauwolf.toml`. A hash of everything the pod reads is an annotation of the pod template, so a
//! repeated deploy without changes writes nothing and restarts nothing.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use grauwolf_k8s::{Cluster, hash_of, kinds};
use serde_json::{Value, json};

use crate::agent::{
    self, AGENT_LABEL, CONFIG_KEY, CONFIG_MAP, MANAGED_BY, MANAGER, config_map_object,
    current_config, render_config, variables_of,
};
use crate::agent_files::{AGENT_MAP, AgentFiles, DATA_DIR, Writable};
use crate::caller_spec;
use crate::callers::{self, CALLERS_DIR, CALLERS_KEY, CALLERS_MAP};
use crate::egress;
use crate::identity;
use crate::io::Io;
use crate::report;
use crate::services::Services;

/// The variable the service reads its token from; `deploy` maps it to the server token.
const TOKEN_VARIABLE: &str = "GRAUWOLF_SERVER_TOKEN";
/// The server token at `<kv>/agents/<agent>/server-token`.
const TOKEN_KEY: &str = "server-token";
const TOKEN_FIELD: &str = "value";
/// Set by `deploy` itself: `--env` may not set them.
const RESERVED: &[&str] = &[
    "SPIFFE_ENDPOINT_SOCKET",
    "SPIFFE_FETCH_TIMEOUT",
    "GRAUWOLF_CREDENTIALS",
    "GRAUWOLF_DATA_DIR",
    "GRAUWOLF_CONVERSATIONS_DIR",
    "SERVER_PORT",
    "GRAUWOLF_SPIFFE",
    "GRAUWOLF_SPIFFE_PORT",
    "GRAUWOLF_SPIFFE_USERS",
    "GRAUWOLF_SPIFFE_PUBLIC_URL",
    TOKEN_VARIABLE,
];
/// Set by `deploy` itself with `--otel`: then `--env` may not set them.
const RESERVED_OTEL: &[&str] = &[
    OTEL_ENDPOINT,
    OTEL_SERVICE_NAME,
    OTEL_SWITCH,
    OTEL_CONTENT_SWITCH,
];
/// The switches of a generated service (ADR-0123); `spec serve` has the flags instead.
const OTEL_SWITCH: &str = "GRAUWOLF_OTEL";
const OTEL_CONTENT_SWITCH: &str = "GRAUWOLF_OTEL_CONTENT";
const OTEL_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const OTEL_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
/// The switch of a generated service for `--events` (`spec serve` has the flag).
const EVENTS_SWITCH: &str = "GRAUWOLF_EVENTS";
/// The endpoint of `--otel` without `--otel-endpoint`: the OTLP/HTTP port of a Grafana LGTM
/// stack (Service `lgtm` in namespace `lgtm`). It is shown in `--help`.
pub const DEFAULT_OTEL_ENDPOINT: &str = "http://lgtm.lgtm.svc:4318";
/// The waiting reasons that will not go away by waiting.
const FATAL: &[&str] = &[
    "CrashLoopBackOff",
    "ImagePullBackOff",
    "ErrImagePull",
    "InvalidImageName",
    "CreateContainerConfigError",
    "CreateContainerError",
];
const POLL: Duration = Duration::from_millis(250);
const LOG_LINES: u32 = 20;

/// The arguments of `agent deploy`.
pub struct DeployOptions {
    pub name: String,
    pub image: String,
    pub spec: Option<PathBuf>,
    pub env: Vec<String>,
    pub host: Option<String>,
    pub port: u16,
    pub mtls_port: u16,
    pub data_size: Option<String>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub pull_secret: Option<String>,
    /// `--otel`: send the traces of the agent (ADR-0049).
    pub otel: Option<OtelOptions>,
    /// `--events`: the events of every run as JSON lines on stderr.
    pub events: bool,
    /// `--identity`: the agent uses the identity service (ADR-0111).
    pub identity: bool,
    /// `--egress` and its settings.
    pub egress: Option<crate::egress::EgressOptions>,
    pub timeout: u64,
}

/// The service's port without `--port` (`SERVER_PORT` of a generated service, ADR-0093).
pub const DEFAULT_PORT: u16 = 8080;

/// `--otel`, `--otel-endpoint` and `--otel-content`.
pub struct OtelOptions {
    pub endpoint: String,
    pub content: bool,
}

/// An OTLP/HTTP base URL: `http://` or `https://`, a host, no blanks (the SDK adds `/v1/traces`).
fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    let host = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .unwrap_or("");
    if host.is_empty() || host.starts_with(['/', ':']) || endpoint.contains(char::is_whitespace) {
        return Err(format!(
            "--otel-endpoint '{endpoint}' is not a URL (example: {DEFAULT_OTEL_ENDPOINT})"
        ));
    }
    Ok(())
}

/// What a deployed agent exports: the endpoint and whether contents are recorded.
pub struct Tracing {
    pub endpoint: String,
    pub content: bool,
}

/// The tracing settings in the pod template of `deployment`; `None` when it is off.
pub fn tracing_of(deployment: &Value) -> Option<Tracing> {
    let container = &deployment["spec"]["template"]["spec"]["containers"][0];
    let has_flag = |flag: &str| {
        container["command"]
            .as_array()
            .is_some_and(|command| command.iter().any(|part| part == flag))
    };
    // `spec serve` takes flags, the service of an image takes the switches (ADR-0123).
    let switched = |name: &str| {
        container["env"].as_array().is_some_and(|env| {
            env.iter()
                .any(|entry| entry["name"] == name && entry["value"] == "1")
        })
    };
    if !has_flag("--otel") && !switched(OTEL_SWITCH) {
        return None;
    }
    let endpoint = container["env"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == OTEL_ENDPOINT)?["value"]
        .as_str()?
        .to_owned();
    Some(Tracing {
        endpoint,
        content: has_flag("--otel-content") || switched(OTEL_CONTENT_SWITCH),
    })
}

/// Whether the pod template of `deployment` has the events switched on: `--events` of
/// `spec serve`, or `GRAUWOLF_EVENTS=1` of a generated service.
pub fn events_of(deployment: &Value) -> bool {
    let container = &deployment["spec"]["template"]["spec"]["containers"][0];
    container["command"]
        .as_array()
        .is_some_and(|command| command.iter().any(|part| part == "--events"))
        || container["env"].as_array().is_some_and(|env| {
            env.iter()
                .any(|entry| entry["name"] == EVENTS_SWITCH && entry["value"] == "1")
        })
}

/// `(service, namespace)` of an endpoint `http://<service>.<namespace>.svc[.…][:port][/…]`.
pub fn collector_service(endpoint: &str) -> Option<(String, String)> {
    let rest = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    let host = authority.split(':').next()?;
    let mut parts = host.split('.');
    let (service, namespace, svc) = (parts.next()?, parts.next()?, parts.next()?);
    (svc == "svc" && !service.is_empty() && !namespace.is_empty())
        .then(|| (service.to_owned(), namespace.to_owned()))
}

fn pod_labels(agent: &str) -> Value {
    json!({"app.kubernetes.io/name": "grauwolf", AGENT_LABEL: agent})
}

fn selector(agent: &str) -> String {
    format!("app.kubernetes.io/name=grauwolf,{AGENT_LABEL}={agent}")
}

fn object_labels(agent: &str) -> Value {
    json!({"app.kubernetes.io/name": "grauwolf", AGENT_LABEL: agent, MANAGED_BY: MANAGER})
}

fn data_claim(agent: &str) -> String {
    format!("{agent}-data")
}

/// `1Gi`, `500m`, `0.5`: what Kubernetes calls a quantity (a simple check; the API server has the
/// last word).
fn validate_quantity(option: &str, value: &str) -> Result<(), String> {
    let digits = value.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = &value[digits.len()..];
    let number = !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && digits.matches('.').count() <= 1
        && !digits.starts_with('.')
        && !digits.ends_with('.');
    let unit = matches!(
        suffix,
        "" | "m" | "k" | "M" | "G" | "T" | "P" | "E" | "Ki" | "Mi" | "Gi" | "Ti" | "Pi" | "Ei"
    );
    if number && unit {
        Ok(())
    } else {
        Err(format!(
            "{option} '{value}' is not a quantity (examples: 500m, 512Mi, 1Gi)"
        ))
    }
}

/// The `(name, value)` pairs of `--env` and the warnings about them.
type ParsedEnv = (Vec<(String, String)>, Vec<String>);

/// `--env NAME=VALUE` split and checked against the variables mapped to OpenBao and the ones
/// `deploy` sets itself. The second part holds warnings.
fn parse_env(
    env: &[String],
    mapped: &BTreeMap<String, String>,
    otel: bool,
    events: bool,
) -> Result<ParsedEnv, String> {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut warnings = Vec::new();
    for entry in env {
        let (name, value) = entry
            .split_once('=')
            .ok_or_else(|| format!("--env takes NAME=VALUE, not '{entry}'"))?;
        let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!(
                "'{name}' is not an environment variable name (letters, digits and '_')"
            ));
        }
        if mapped.contains_key(name) {
            return Err(format!(
                "{name} is mapped to a secret in OpenBao in the agent's configuration: it cannot also be set with --env"
            ));
        }
        if [identity::URL_VARIABLE, identity::SPIFFE_ID_VARIABLE].contains(&name) {
            return Err(format!(
                "{name} is set by `agent deploy --identity`; use --identity instead of --env"
            ));
        }
        if RESERVED.contains(&name) {
            return Err(format!(
                "{name} is set by `agent deploy`; --env cannot set it"
            ));
        }
        if otel && RESERVED_OTEL.contains(&name) {
            return Err(format!(
                "{name} is set by `agent deploy --otel`; use --otel-endpoint instead of --env"
            ));
        }
        if events && name == EVENTS_SWITCH {
            return Err(format!(
                "{name} is set by `agent deploy --events`; remove one of them"
            ));
        }
        if values.insert(name.to_owned(), value.to_owned()).is_some() {
            return Err(format!("--env {name} is given twice"));
        }
        if !grauwolf_config::VARIABLES
            .iter()
            .any(|known| known.name == name)
        {
            warnings.push(format!(
                "warning: {name} is not a configuration variable of grauwolf; it is set in the pod anyway"
            ));
        }
    }
    Ok((values.into_iter().collect(), warnings))
}

/// What the pod is made of.
struct Plan<'a> {
    agent: &'a str,
    namespace: &'a str,
    options: &'a DeployOptions,
    command: Option<Vec<String>>,
    env: Vec<(String, String)>,
    mounts: Vec<Value>,
    /// The writable folders of the spec tree: directories of the data volume.
    writable: Vec<Writable>,
    has_agent_map: bool,
    /// `(GRAUWOLF_IDENTITY_URL, GRAUWOLF_IDENTITY_SPIFFE_ID)` with `--identity`.
    identity: Option<(String, String)>,
}

fn pod_spec(plan: &Plan) -> Value {
    let options = plan.options;
    let mut env = vec![
        json!({"name": "SPIFFE_ENDPOINT_SOCKET", "value": "unix:///spiffe-workload-api/agent.sock"}),
        // The identity may be issued a while after the pod starts (ADR-0120).
        json!({"name": "SPIFFE_FETCH_TIMEOUT", "value": "2m"}),
        json!({"name": "GRAUWOLF_CREDENTIALS", "value": format!("/etc/grauwolf/{CONFIG_KEY}")}),
        // Callers by SPIFFE ID (ADR-0109): the mTLS port, the map of the callers and the URL that
        // the agent card names.
        json!({"name": "GRAUWOLF_SPIFFE", "value": "1"}),
        json!({"name": "GRAUWOLF_SPIFFE_PORT", "value": options.mtls_port.to_string()}),
        json!({"name": "GRAUWOLF_SPIFFE_USERS", "value": format!("{CALLERS_DIR}/{CALLERS_KEY}")}),
        json!({"name": "GRAUWOLF_SPIFFE_PUBLIC_URL",
            "value": format!("https://{}.{}.svc:{}", plan.agent, plan.namespace, options.mtls_port)}),
    ];
    if options.spec.is_none() {
        // A generated service reads its port from the environment and keeps its files in /app.
        env.push(json!({"name": "SERVER_PORT", "value": options.port.to_string()}));
        if options.data_size.is_some() {
            env.push(json!({"name": "GRAUWOLF_CONVERSATIONS_DIR", "value": format!("{DATA_DIR}/conversations")}));
        }
    }
    if let Some(otel) = &options.otel {
        // The OpenTelemetry SDK reads both from the environment (ADR-0049).
        env.push(json!({"name": OTEL_ENDPOINT, "value": otel.endpoint}));
        env.push(json!({"name": OTEL_SERVICE_NAME, "value": plan.agent}));
        if options.spec.is_none() {
            // The service of an image has no flags (ADR-0123).
            env.push(json!({"name": OTEL_SWITCH, "value": "1"}));
            if otel.content {
                env.push(json!({"name": OTEL_CONTENT_SWITCH, "value": "1"}));
            }
        }
    }
    if let Some((url, id)) = &plan.identity {
        // Delegation (ADR-0111): people's tokens are exchanged, Txn-Tokens verified.
        env.push(json!({"name": identity::URL_VARIABLE, "value": url}));
        env.push(json!({"name": identity::SPIFFE_ID_VARIABLE, "value": id}));
    }
    if options.events && options.spec.is_none() {
        // The service of an image has no flags.
        env.push(json!({"name": EVENTS_SWITCH, "value": "1"}));
    }
    env.extend(
        plan.env
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value})),
    );
    let mut mounts = vec![
        json!({"name": "config", "mountPath": "/etc/grauwolf", "readOnly": true}),
        json!({"name": "spiffe-workload-api", "mountPath": "/spiffe-workload-api", "readOnly": true}),
        json!({"name": "data", "mountPath": DATA_DIR}),
        // The whole directory, never one file by `subPath`: only a directory follows an update of
        // the ConfigMap, and the service reads the file again when it changes.
        json!({"name": "callers", "mountPath": CALLERS_DIR, "readOnly": true}),
    ];
    mounts.extend(plan.mounts.iter().cloned());
    // The folders the agent writes: directories of the data volume at their place in the tree.
    mounts.extend(plan.writable.iter().map(
        |dir| json!({"name": "data", "mountPath": dir.mount_path(), "subPath": dir.sub_path()}),
    ));
    let probe = |period: u64, failures: u64| {
        json!({"httpGet": {"path": "/health", "port": "http"},
            "periodSeconds": period, "failureThreshold": failures, "timeoutSeconds": 3})
    };
    let mut container = json!({
        "name": "grauwolf",
        "image": options.image,
        "ports": [{"name": "http", "containerPort": options.port},
            {"name": "mtls", "containerPort": options.mtls_port}],
        "securityContext": {"allowPrivilegeEscalation": false, "capabilities": {"drop": ["ALL"]}},
        "env": env,
        "volumeMounts": mounts,
        "workingDir": DATA_DIR,
        // Up to five minutes to start (the identity, the first connections), then the probes.
        "startupProbe": probe(5, 60),
        "readinessProbe": probe(5, 3),
        "livenessProbe": probe(10, 6),
    });
    if let Some(command) = &plan.command {
        container["command"] = json!(command);
    }
    let mut resources = serde_json::Map::new();
    for (name, value) in [("cpu", &options.cpu), ("memory", &options.memory)] {
        if let Some(value) = value {
            resources.insert(name.to_owned(), json!(value));
        }
    }
    if !resources.is_empty() {
        container["resources"] = json!({"requests": resources.clone(), "limits": resources});
    }
    // The directories exist before the agent starts, made by the agent's own user: kubelet would
    // make a missing `subPath` directory itself, owned by root and closed to the group, so that the
    // agent could not write into it. The image needs `mkdir` (the grauwolf image has it).
    let init = (!plan.writable.is_empty()).then(|| {
        let mut command = vec!["mkdir".to_owned(), "-p".to_owned()];
        command.extend(
            plan.writable
                .iter()
                .map(|dir| format!("{DATA_DIR}/{}", dir.sub_path())),
        );
        let mut init = json!({
            "name": "data-dirs",
            "image": options.image,
            "command": command,
            "securityContext": container["securityContext"],
            "volumeMounts": [{"name": "data", "mountPath": DATA_DIR}],
        });
        if let Some(resources) = container.get("resources") {
            init["resources"] = resources.clone();
        }
        init
    });
    let data = match &options.data_size {
        Some(_) => {
            json!({"name": "data", "persistentVolumeClaim": {"claimName": data_claim(plan.agent)}})
        }
        None => json!({"name": "data", "emptyDir": {}}),
    };
    let mut volumes = vec![
        json!({"name": "config", "configMap": {"name": CONFIG_MAP}}),
        json!({"name": "spiffe-workload-api", "csi": {"driver": "csi.spiffe.io", "readOnly": true}}),
        json!({"name": "callers", "configMap": {"name": CALLERS_MAP}}),
        data,
    ];
    if plan.has_agent_map {
        volumes.push(json!({"name": "agent", "configMap": {"name": AGENT_MAP}}));
    }
    let mut pod = json!({
        "serviceAccountName": plan.agent,
        "automountServiceAccountToken": false,
        // The user is numeric: runAsNonRoot cannot check the name `nonroot` (ADR-0120).
        "securityContext": {"runAsNonRoot": true, "runAsUser": 65532, "runAsGroup": 65532,
            "fsGroup": 65532, "seccompProfile": {"type": "RuntimeDefault"}},
        "containers": [container],
        "volumes": volumes,
    });
    if let Some(init) = init {
        pod["initContainers"] = json!([init]);
    }
    pod
}

fn deployment_object(plan: &Plan, config: &str, agent_map: Option<&Value>) -> Value {
    let pod = pod_spec(plan);
    // Everything the pod reads without the Deployment changing: a new hash restarts it. Not the
    // callers (`grauwolf-callers`): the service reads that file again by itself.
    let hash = hash_of(&json!({"pod": pod, "config": config, "agent": agent_map}));
    json!({"apiVersion": "apps/v1", "kind": "Deployment",
    "metadata": {"name": plan.agent, "namespace": plan.namespace,
        "labels": object_labels(plan.agent)},
    "spec": {
        "replicas": 1,
        // One replica and a volume that only one pod can use: the old pod goes first.
        "strategy": {"type": "Recreate"},
        "selector": {"matchLabels": pod_labels(plan.agent)},
        "template": {
            "metadata": {"labels": pod_labels(plan.agent),
                "annotations": {"grauwolf.io/config-hash": hash}},
            "spec": pod,
        }}})
}

fn service_object(agent: &str, namespace: &str, port: u16, mtls_port: u16) -> Value {
    json!({"apiVersion": "v1", "kind": "Service",
        "metadata": {"name": agent, "namespace": namespace, "labels": object_labels(agent)},
        "spec": {"type": "ClusterIP", "selector": pod_labels(agent),
            "ports": [{"name": "http", "port": port, "targetPort": "http"},
                {"name": "mtls", "port": mtls_port, "targetPort": "mtls"}]}})
}

fn ingress_object(agent: &str, namespace: &str, host: &str, port: u16) -> Value {
    // No TLS block and no class: the cluster's defaults add them.
    json!({"apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
        "metadata": {"name": agent, "namespace": namespace, "labels": object_labels(agent)},
        "spec": {"rules": [{"host": host, "http": {"paths": [{
            "path": "/", "pathType": "Prefix",
            "backend": {"service": {"name": agent, "port": {"number": port}}}}]}}]}})
}

fn claim_object(agent: &str, namespace: &str, size: &str) -> Value {
    json!({"apiVersion": "v1", "kind": "PersistentVolumeClaim",
        "metadata": {"name": data_claim(agent), "namespace": namespace,
            "labels": object_labels(agent)},
        "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": size}}}})
}

/// A random token of 32 bytes, hex encoded.
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("cannot get random bytes for the server token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Maps `GRAUWOLF_SERVER_TOKEN` to the server token and creates the token if there is none; the
/// secret comes first, so that the configuration never points at a secret that is missing.
/// Returns the text of the configuration.
async fn ensure_token(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    namespace: &str,
    config: &str,
) -> Result<String, String> {
    let settings = &services.settings;
    let bao = services.admin()?;
    let mut variables = variables_of(config);
    let path = format!("{}/{TOKEN_KEY}", agent::kv_prefix(agent));
    let source = format!("openbao:{}/{path}#{TOKEN_FIELD}", settings.kv_mount);
    match variables.get(TOKEN_VARIABLE) {
        Some(other) if *other != source => {
            io.line(&format!(
                "server token: {TOKEN_VARIABLE} is mapped to {other} in the configuration, kept as it is"
            ));
            return Ok(config.to_owned());
        }
        _ => {}
    }
    let exists = bao
        .kv_current_version(&settings.kv_mount, &path)
        .await
        .map_err(|error| format!("server token of {agent}: {error}"))?
        .is_some();
    if exists {
        io.line(&format!("server token {}/{path}: kept", settings.kv_mount));
    } else {
        bao.kv_put_field(&settings.kv_mount, &path, TOKEN_FIELD, &random_token()?)
            .await
            .map_err(|error| format!("server token of {agent}: {error}"))?;
        io.line(&format!(
            "server token {}/{path}: created (read it with `grauwolf-k8-ops secret get {agent} {TOKEN_KEY} --reveal`)",
            settings.kv_mount
        ));
    }
    if variables.contains_key(TOKEN_VARIABLE) {
        return Ok(config.to_owned());
    }
    variables.insert(TOKEN_VARIABLE.to_owned(), source);
    let text = render_config(settings, agent, &variables);
    let outcome = services
        .cluster
        .ensure(
            &kinds::CONFIG_MAP,
            Some(namespace),
            &config_map_object(namespace, &text),
        )
        .await
        .map_err(|error| format!("ConfigMap {namespace}/{CONFIG_MAP}: {error}"))?;
    io.line(&format!(
        "ConfigMap {namespace}/{CONFIG_MAP} ({TOKEN_VARIABLE} -> the server token): {}",
        report::k8s(outcome)
    ));
    Ok(text)
}

/// The agent's namespace and the text of its configuration, or what to do first.
async fn agent_state(cluster: &Cluster, agent: &str) -> Result<(String, String), String> {
    agent::validate_name(agent)?;
    let namespace = agent::find_namespace(cluster, agent)
        .await?
        .ok_or_else(|| {
            format!("agent {agent} not found (create it with `grauwolf-k8-ops agent add {agent}`)")
        })?;
    let config = current_config(cluster, &namespace).await?.ok_or_else(|| {
        format!(
            "ConfigMap {namespace}/{CONFIG_MAP} not found: run `grauwolf-k8-ops agent add {agent}`"
        )
    })?;
    Ok((namespace, config))
}

pub async fn deploy(
    services: &Services,
    io: &mut Io<'_>,
    options: DeployOptions,
) -> Result<i32, String> {
    let cluster = &services.cluster;
    let agent = options.name.as_str();
    let (namespace, config) = agent_state(cluster, agent).await?;
    let namespace = namespace.as_str();
    services.admin()?;

    // Refuse before anything is written.
    if options.image.trim().is_empty() {
        return Err("--image must not be empty".to_owned());
    }
    if options.port == 0 {
        return Err("--port must not be 0".to_owned());
    }
    if options.mtls_port == 0 {
        return Err("--mtls-port must not be 0".to_owned());
    }
    if options.mtls_port == options.port {
        return Err("--mtls-port must differ from --port".to_owned());
    }
    // A map that does not parse is refused here, before anything is written.
    let callers = callers::read_callers(cluster, namespace).await?;
    for (option, value) in [
        ("--data-size", &options.data_size),
        ("--cpu", &options.cpu),
        ("--memory", &options.memory),
    ] {
        if let Some(value) = value {
            validate_quantity(option, value)?;
        }
    }
    if let Some(host) = &options.host
        && (host.is_empty() || host.contains(['/', ':', ' ']))
    {
        return Err(format!("--host takes a host name, not '{host}'"));
    }
    if let Some(otel) = &options.otel {
        validate_endpoint(&otel.endpoint)?;
    }
    let (env, warnings) = parse_env(
        &options.env,
        &variables_of(&config),
        options.otel.is_some(),
        options.events,
    )?;
    let files = options.spec.as_deref().map(AgentFiles::read).transpose()?;
    let delegation = if options.identity {
        Some(identity::for_agents(services).await?)
    } else {
        None
    };
    // The destinations of the spec tree, before anything is written.
    let egress_destinations = match (&options.egress, &options.spec) {
        (Some(_), None) => {
            return Err(
                "--egress needs --spec: the destinations are derived from the spec tree".to_owned(),
            );
        }
        (Some(_), Some(spec)) => {
            let (mut destinations, warnings) = egress::derive(
                spec,
                &config,
                options.otel.as_ref().map(|otel| otel.endpoint.as_str()),
                &env,
                &variables_of(&config),
            )?;
            if let Some(destination) = delegation
                .as_ref()
                .and_then(|(url, _)| identity::destination_of(url))
            {
                destinations.push(destination);
            }
            Some((destinations, warnings))
        }
        (None, _) => None,
    };
    let agent_map = files
        .as_ref()
        .map(|files| files.config_map(namespace))
        .transpose()?;

    let mode = match &options.spec {
        Some(spec) => format!("grauwolf spec serve {}", spec.display()),
        None => "the service of the image".to_owned(),
    };
    io.line(&format!(
        "agent {agent} (namespace {namespace}): image {}, {mode}",
        options.image
    ));
    if let Some(otel) = &options.otel {
        io.line(&format!(
            "tracing: on, OTLP/HTTP to {}{}",
            otel.endpoint,
            if otel.content { " (with contents)" } else { "" }
        ));
    }
    if options.events {
        io.line("events: on, JSON lines on stderr (the pod log)");
    }
    if let Some((url, _)) = &delegation {
        io.line(&format!(
            "delegation: on, identity service {url} (people's tokens and Txn-Tokens)"
        ));
    }
    for warning in warnings {
        io.error(&warning);
    }
    if let Some(files) = &files {
        for dir in &files.writable {
            io.line(&format!(
                "writable: {} ({}; the data volume, {})",
                dir.mount_path(),
                dir.reason,
                dir.sub_path()
            ));
        }
        for warning in &files.warnings {
            io.error(warning);
        }
        // The spec's calls to other agents of the cluster (ADR-0109 Q8): warnings, not errors.
        let known = agent::agents(cluster).await?;
        let specs = caller_spec::spec_documents(
            files
                .files
                .values()
                .filter_map(|bytes| std::str::from_utf8(bytes).ok()),
        );
        let findings = caller_spec::check(
            cluster,
            &services.settings,
            agent,
            namespace,
            &known,
            &specs,
        )
        .await;
        for problem in findings.problems {
            io.error(&format!("warning: {} ({})", problem.message, problem.hint));
        }
    }

    if let Some(reference) = &options.pull_secret {
        let name = agent::copy_pull_secret(cluster, io, reference, namespace).await?;
        let account = agent::service_account_object(agent, namespace, Some(&name));
        let outcome = cluster
            .ensure(&kinds::SERVICE_ACCOUNT, Some(namespace), &account)
            .await
            .map_err(|error| format!("service account {agent}: {error}"))?;
        io.line(&format!(
            "service account {namespace}/{agent}: {}",
            report::k8s(outcome)
        ));
    }
    let config = ensure_token(services, io, agent, namespace, &config).await?;

    // The callers: made empty if missing, never replaced (`agent allow` writes them).
    let callers = match callers {
        Some(callers) => callers,
        None => {
            let callers = callers::Callers::new();
            let outcome = cluster
                .ensure(
                    &kinds::CONFIG_MAP,
                    Some(namespace),
                    &callers::callers_object(namespace, &callers),
                )
                .await
                .map_err(|error| format!("ConfigMap {CALLERS_MAP}: {error}"))?;
            io.line(&format!(
                "ConfigMap {namespace}/{CALLERS_MAP} (no callers yet): {}",
                report::k8s(outcome)
            ));
            callers
        }
    };

    let (command, mounts, map_object) = match (&files, &agent_map) {
        (Some(files), Some((object, mounts, size))) => {
            let outcome = cluster
                .ensure(&kinds::CONFIG_MAP, Some(namespace), object)
                .await
                .map_err(|error| format!("ConfigMap {AGENT_MAP}: {error}"))?;
            io.line(&format!(
                "ConfigMap {namespace}/{AGENT_MAP} ({} file(s), {} KiB): {}",
                files.files.len(),
                size.div_ceil(1024),
                report::k8s(outcome)
            ));
            let mut command = vec![
                "grauwolf".to_owned(),
                "spec".to_owned(),
                "serve".to_owned(),
                files.root_path(),
                "--addr".to_owned(),
                format!("0.0.0.0:{}", options.port),
            ];
            if let Some(otel) = &options.otel {
                command.push("--otel".to_owned());
                if otel.content {
                    command.push("--otel-content".to_owned());
                }
            }
            if options.events {
                command.push("--events".to_owned());
            }
            (Some(command), mounts.clone(), Some(object))
        }
        _ => {
            if cluster
                .delete(&kinds::CONFIG_MAP, Some(namespace), AGENT_MAP)
                .await
                .map_err(|error| format!("ConfigMap {AGENT_MAP}: {error}"))?
            {
                io.line(&format!(
                    "ConfigMap {namespace}/{AGENT_MAP}: deleted (no --spec)"
                ));
            }
            (None, Vec::new(), None)
        }
    };

    if let Some(size) = &options.data_size {
        let outcome = cluster
            .ensure(
                &kinds::PERSISTENT_VOLUME_CLAIM,
                Some(namespace),
                &claim_object(agent, namespace, size),
            )
            .await
            .map_err(|error| format!("PersistentVolumeClaim {}: {error}", data_claim(agent)))?;
        io.line(&format!(
            "PersistentVolumeClaim {namespace}/{} ({size}): {}",
            data_claim(agent),
            report::k8s(outcome)
        ));
    } else if cluster
        .get(
            &kinds::PERSISTENT_VOLUME_CLAIM,
            Some(namespace),
            &data_claim(agent),
        )
        .await
        .map_err(|error| error.to_string())?
        .is_some()
    {
        io.line(&format!(
            "note: PersistentVolumeClaim {namespace}/{} is not used without --data-size (`agent undeploy --purge-data` deletes it)",
            data_claim(agent)
        ));
    }

    let plan = Plan {
        agent,
        namespace,
        options: &options,
        command,
        env,
        mounts,
        writable: files.map(|files| files.writable).unwrap_or_default(),
        has_agent_map: map_object.is_some(),
        identity: delegation,
    };
    let deployment = deployment_object(&plan, &config, map_object);
    let outcome = cluster
        .ensure(&kinds::DEPLOYMENT, Some(namespace), &deployment)
        .await
        .map_err(|error| format!("Deployment {agent}: {error}"))?;
    io.line(&format!(
        "Deployment {namespace}/{agent}: {}",
        report::k8s(outcome)
    ));
    let outcome = cluster
        .ensure(
            &kinds::SERVICE,
            Some(namespace),
            &service_object(agent, namespace, options.port, options.mtls_port),
        )
        .await
        .map_err(|error| format!("Service {agent}: {error}"))?;
    io.line(&format!(
        "Service {namespace}/{agent} (ports {}, {} mTLS): {}",
        options.port,
        options.mtls_port,
        report::k8s(outcome)
    ));
    match &options.host {
        Some(host) => {
            let outcome = cluster
                .ensure(
                    &kinds::INGRESS,
                    Some(namespace),
                    &ingress_object(agent, namespace, host, options.port),
                )
                .await
                .map_err(|error| format!("Ingress {agent}: {error}"))?;
            io.line(&format!(
                "Ingress {namespace}/{agent} (host {host}): {}",
                report::k8s(outcome)
            ));
        }
        None => {
            if cluster
                .delete(&kinds::INGRESS, Some(namespace), agent)
                .await
                .map_err(|error| format!("Ingress {agent}: {error}"))?
            {
                io.line(&format!("Ingress {namespace}/{agent}: deleted (no --host)"));
            }
        }
    }

    callers::sync_policy_with(
        cluster,
        io,
        agent,
        namespace,
        &callers,
        (options.port, options.mtls_port),
    )
    .await?;

    match (&options.egress, egress_destinations) {
        (Some(egress_options), Some((destinations, warnings))) => {
            let mut plan = egress::plan(
                cluster,
                &services.resolver,
                agent,
                namespace,
                &destinations,
                egress_options,
            )
            .await;
            plan.warnings.splice(0..0, warnings);
            egress::sync(cluster, io, namespace, &plan).await?;
        }
        _ => egress::remove(cluster, io, namespace, "no --egress").await?,
    }

    io.line(&format!(
        "waiting for the rollout and /health (at most {} s)",
        options.timeout
    ));
    let waited = wait_ready(
        cluster,
        namespace,
        agent,
        options.port,
        Duration::from_secs(options.timeout),
    )
    .await?;
    match waited {
        Waited::Ready => {
            io.line(&format!("agent {agent}: ready, /health answers ok"));
            io.line(&format!(
                "endpoint http://{agent}.{namespace}.svc:{}",
                options.port
            ));
            io.line(&format!(
                "mTLS endpoint https://{agent}.{namespace}.svc:{}",
                options.mtls_port
            ));
            if let Some(host) = &options.host {
                io.line(&format!("host {host}"));
            }
            Ok(0)
        }
        Waited::Failed(reason) => {
            report_failure(cluster, io, namespace, agent, &reason).await;
            Ok(1)
        }
    }
}

pub(crate) enum Waited {
    Ready,
    Failed(String),
}

/// The pod of the Deployment that was made last.
pub(crate) async fn current_pod(
    cluster: &Cluster,
    namespace: &str,
    agent: &str,
) -> Result<Option<Value>, String> {
    let mut pods = cluster
        .list(&kinds::POD, Some(namespace), Some(&selector(agent)))
        .await
        .map_err(|error| error.to_string())?;
    pods.sort_by_key(|pod| {
        pod["metadata"]["creationTimestamp"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    Ok(pods.pop())
}

pub(crate) struct PodInfo {
    pub name: String,
    pub phase: String,
    pub ready: bool,
    pub restarts: u64,
    pub waiting: Option<String>,
}

pub(crate) fn pod_info(pod: &Value) -> PodInfo {
    let status = &pod["status"]["containerStatuses"][0];
    PodInfo {
        name: pod["metadata"]["name"].as_str().unwrap_or("").to_owned(),
        phase: pod["status"]["phase"]
            .as_str()
            .unwrap_or("Unknown")
            .to_owned(),
        ready: status["ready"].as_bool().unwrap_or(false),
        restarts: status["restartCount"].as_u64().unwrap_or(0),
        waiting: status["state"]["waiting"]["reason"]
            .as_str()
            .map(str::to_owned),
    }
}

/// The Deployment has rolled out its generation and its pod is ready.
pub(crate) fn rolled_out(deployment: &Value) -> bool {
    let status = &deployment["status"];
    let wanted = deployment["spec"]["replicas"].as_u64().unwrap_or(1);
    status["observedGeneration"].as_u64().unwrap_or(0)
        >= deployment["metadata"]["generation"].as_u64().unwrap_or(0)
        && status["updatedReplicas"].as_u64().unwrap_or(0) >= wanted
        && status["readyReplicas"].as_u64().unwrap_or(0) >= wanted
        && status["replicas"].as_u64().unwrap_or(0)
            == status["updatedReplicas"].as_u64().unwrap_or(0)
}

/// Waits for the rollout, then for `/health` through the API server's service proxy.
pub(crate) async fn wait_ready(
    cluster: &Cluster,
    namespace: &str,
    agent: &str,
    port: u16,
    timeout: Duration,
) -> Result<Waited, String> {
    let started = Instant::now();
    let health =
        format!("/api/v1/namespaces/{namespace}/services/http:{agent}:{port}/proxy/health");
    let mut state = "no pod yet".to_owned();
    loop {
        let deployment = cluster
            .get(&kinds::DEPLOYMENT, Some(namespace), agent)
            .await
            .map_err(|error| format!("Deployment {agent}: {error}"))?
            .ok_or_else(|| format!("Deployment {agent} disappeared"))?;
        if let Some(pod) = current_pod(cluster, namespace, agent).await? {
            let info = pod_info(&pod);
            if let Some(reason) = info.waiting.as_deref().filter(|r| FATAL.contains(r)) {
                return Ok(Waited::Failed(format!(
                    "pod {}: {reason} ({} restart(s))",
                    info.name, info.restarts
                )));
            }
            state = format!(
                "pod {}: {}, {}ready, {} restart(s){}",
                info.name,
                info.phase,
                if info.ready { "" } else { "not " },
                info.restarts,
                info.waiting
                    .map(|reason| format!(", waiting: {reason}"))
                    .unwrap_or_default()
            );
        }
        if rolled_out(&deployment) {
            match cluster.raw("GET", &health, &[], None).await {
                Ok((200, _)) => return Ok(Waited::Ready),
                Ok((status, _)) => state = format!("/health answers HTTP {status}"),
                Err(error) => state = format!("/health is not reachable: {error}"),
            }
        }
        if started.elapsed() >= timeout {
            return Ok(Waited::Failed(format!(
                "not ready after {} s ({state})",
                timeout.as_secs()
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// What went wrong and the last lines of the pod's log.
async fn report_failure(
    cluster: &Cluster,
    io: &mut Io<'_>,
    namespace: &str,
    agent: &str,
    reason: &str,
) {
    report_failure_with(
        cluster,
        io,
        namespace,
        agent,
        reason,
        &format!(
            "next: `grauwolf-k8-ops agent status {agent}`, `grauwolf-k8-ops agent logs {agent}`"
        ),
    )
    .await;
}

/// Like `report_failure`, with the last line `next`.
pub(crate) async fn report_failure_with(
    cluster: &Cluster,
    io: &mut Io<'_>,
    namespace: &str,
    agent: &str,
    reason: &str,
    next: &str,
) {
    io.error(&format!("agent {agent}: {reason}"));
    if let Ok(Some(pod)) = current_pod(cluster, namespace, agent).await {
        let name = pod_info(&pod).name;
        let mut log = cluster
            .pod_log(namespace, &name, LOG_LINES)
            .await
            .unwrap_or_default();
        if log.trim().is_empty() {
            log = cluster
                .pod_log_previous(namespace, &name, LOG_LINES)
                .await
                .unwrap_or_default();
        }
        if log.trim().is_empty() {
            io.error("(the pod has no log)");
        } else {
            io.error(&format!("last lines of the log of pod {name}:"));
            for line in log.lines() {
                io.error(&format!("  {line}"));
            }
        }
    }
    io.error(next);
}

pub async fn status(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    json: bool,
) -> Result<(), String> {
    let cluster = &services.cluster;
    agent::validate_name(agent)?;
    let namespace = agent::find_namespace(cluster, agent)
        .await?
        .ok_or_else(|| format!("agent {agent} not found"))?;
    let deployment = cluster
        .get(&kinds::DEPLOYMENT, Some(&namespace), agent)
        .await
        .map_err(|error| format!("Deployment {agent}: {error}"))?;
    let token = server_token_exists(services, agent).await;
    let Some(deployment) = deployment else {
        if json {
            io.line(
                &json!({"agent": agent, "namespace": namespace, "deployed": false,
                "server_token": token})
                .to_string(),
            );
        } else {
            io.line(&format!(
                "agent {agent}: not deployed (`grauwolf-k8-ops agent deploy {agent} --image <image>`)"
            ));
        }
        return Ok(());
    };
    let image = deployment["spec"]["template"]["spec"]["containers"][0]["image"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let pod = current_pod(cluster, &namespace, agent)
        .await?
        .map(|pod| pod_info(&pod));
    let service = cluster
        .get(&kinds::SERVICE, Some(&namespace), agent)
        .await
        .map_err(|error| format!("Service {agent}: {error}"))?;
    let endpoint = service.as_ref().and_then(|service| {
        service["spec"]["ports"][0]["port"]
            .as_u64()
            .map(|port| format!("http://{agent}.{namespace}.svc:{port}"))
    });
    let mtls_endpoint = service.as_ref().and_then(|service| {
        service["spec"]["ports"]
            .as_array()?
            .iter()
            .find(|port| port["name"] == "mtls")?["port"]
            .as_u64()
            .map(|port| format!("https://{agent}.{namespace}.svc:{port}"))
    });
    let host = cluster
        .get(&kinds::INGRESS, Some(&namespace), agent)
        .await
        .map_err(|error| format!("Ingress {agent}: {error}"))?
        .and_then(|ingress| {
            ingress["spec"]["rules"][0]["host"]
                .as_str()
                .map(str::to_owned)
        });
    let ready = rolled_out(&deployment) && pod.as_ref().is_some_and(|pod| pod.ready);
    let tracing = tracing_of(&deployment);
    let events = events_of(&deployment);
    let delegation = identity::url_of(&deployment);
    let writable = writable_places_of(&deployment);
    let egress_summary = egress::read(cluster, &namespace)
        .await?
        .map(|policy| egress::summary_of(&policy));
    if json {
        let pod_json = pod.as_ref().map(|pod| {
            json!({"name": pod.name, "phase": pod.phase, "ready": pod.ready,
                "restarts": pod.restarts, "waiting_reason": pod.waiting})
        });
        io.line(
            &json!({"agent": agent, "namespace": namespace, "deployed": true, "ready": ready,
                "image": image, "pod": pod_json, "endpoint": endpoint,
                "mtls_endpoint": mtls_endpoint, "host": host,
                "tracing": tracing.as_ref().map(|tracing| json!({
                    "endpoint": tracing.endpoint, "content": tracing.content})),
                "events": events, "identity": delegation, "writable": writable, "server_token": token,
                "egress": egress_summary.as_ref().map(|(destinations, port_only, strict)| json!({
                    "destinations": destinations, "port_restricted": port_only,
                    "strict": strict}))})
            .to_string(),
        );
        return Ok(());
    }
    io.line(&format!(
        "agent {agent} (namespace {namespace}): deployed, {}",
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
    if let Some(endpoint) = &endpoint {
        io.line(&format!("endpoint {endpoint}"));
    }
    if let Some(endpoint) = &mtls_endpoint {
        io.line(&format!("mTLS endpoint {endpoint}"));
    }
    if let Some(host) = &host {
        io.line(&format!("host {host}"));
    }
    io.line(&match &tracing {
        Some(tracing) => format!(
            "tracing: on ({}{})",
            tracing.endpoint,
            if tracing.content {
                ", with contents"
            } else {
                ""
            }
        ),
        None => "tracing: off".to_owned(),
    });
    io.line(if events { "events: on" } else { "events: off" });
    io.line(&match &delegation {
        Some(url) => format!("identity: on ({url})"),
        None => "identity: off".to_owned(),
    });
    io.line(&match &egress_summary {
        Some((destinations, port_only, strict)) => format!(
            "egress: {destinations} destinations, {} only port-restricted{}",
            port_only.len(),
            if *strict { " (strict)" } else { "" }
        ),
        None => "egress: off".to_owned(),
    });
    for place in &writable {
        io.line(&format!(
            "writable {place} (a directory of the data volume)"
        ));
    }
    io.line(&format!(
        "server token: {}",
        match token {
            Some(true) => "exists",
            Some(false) => "missing",
            None => "unknown (no OpenBao token)",
        }
    ));
    Ok(())
}

/// The folders of the spec tree that the pod of `deployment` may write: the data volume mounted
/// below the spec folder.
fn writable_places_of(deployment: &Value) -> Vec<String> {
    let prefix = format!("{}/", crate::agent_files::SPEC_DIR);
    deployment["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|mount| mount["name"] == "data")
        .filter_map(|mount| mount["mountPath"].as_str())
        .filter(|path| path.starts_with(&prefix))
        .map(str::to_owned)
        .collect()
}

/// Whether the server token exists; `None` if OpenBao cannot be asked.
async fn server_token_exists(services: &Services, agent: &str) -> Option<bool> {
    let bao = services.admin().ok()?;
    let path = format!("{}/{TOKEN_KEY}", agent::kv_prefix(agent));
    bao.kv_current_version(&services.settings.kv_mount, &path)
        .await
        .ok()
        .map(|version| version.is_some())
}

pub async fn logs(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    follow: bool,
    tail: u32,
    events_only: bool,
) -> Result<(), String> {
    let cluster = &services.cluster;
    agent::validate_name(agent)?;
    let namespace = agent::find_namespace(cluster, agent)
        .await?
        .ok_or_else(|| format!("agent {agent} not found"))?;
    let pod = current_pod(cluster, &namespace, agent)
        .await?
        .ok_or_else(|| {
            format!(
                "agent {agent} has no pod (is it deployed? `grauwolf-k8-ops agent status {agent}`)"
            )
        })?;
    let name = pod_info(&pod).name;
    cluster
        .pod_log_stream(&namespace, &name, tail, follow, |chunk| {
            if events_only {
                // A chunk is complete lines; an event is a JSON object with an `event` field.
                for line in chunk.lines().filter(|line| line.starts_with("{\"event\":")) {
                    io.value(line);
                }
            } else {
                io.text(chunk);
            }
        })
        .await
        .map_err(|error| error.to_string())
}

pub async fn undeploy(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    purge_data: bool,
    yes: bool,
) -> Result<(), String> {
    let cluster = &services.cluster;
    agent::validate_name(agent)?;
    let namespace = agent::find_namespace(cluster, agent)
        .await?
        .ok_or_else(|| format!("agent {agent} not found"))?;
    let claim = data_claim(agent);
    let mut found: Vec<(&str, &grauwolf_k8s::Kind, String)> = Vec::new();
    for (label, kind, name) in [
        ("Deployment", &kinds::DEPLOYMENT, agent.to_owned()),
        ("Service", &kinds::SERVICE, agent.to_owned()),
        ("Ingress", &kinds::INGRESS, agent.to_owned()),
        ("ConfigMap", &kinds::CONFIG_MAP, AGENT_MAP.to_owned()),
        (
            "NetworkPolicy",
            &kinds::NETWORK_POLICY,
            egress::EGRESS_POLICY.to_owned(),
        ),
        (
            "PersistentVolumeClaim",
            &kinds::PERSISTENT_VOLUME_CLAIM,
            claim.clone(),
        ),
    ] {
        if cluster
            .get(kind, Some(&namespace), &name)
            .await
            .map_err(|error| format!("{label} {name}: {error}"))?
            .is_some()
        {
            found.push((label, kind, name));
        }
    }
    let kept_claim = found
        .iter()
        .any(|(label, ..)| *label == "PersistentVolumeClaim")
        && !purge_data;
    found.retain(|(label, ..)| *label != "PersistentVolumeClaim" || purge_data);
    if found.is_empty() && !kept_claim {
        return Err(format!("agent {agent} is not deployed"));
    }
    io.line(&format!(
        "agent {agent} will be undeployed from namespace {namespace}:"
    ));
    for (label, _, name) in &found {
        io.line(&format!("  {label} {name}"));
    }
    if kept_claim {
        io.line(&format!(
            "  kept: PersistentVolumeClaim {claim} (use --purge-data to delete the data)"
        ));
    }
    io.line(&format!(
        "  kept: identity, role, policy, secrets and the callers (ConfigMap {CALLERS_MAP}, NetworkPolicy {})",
        callers::NETWORK_POLICY
    ));
    if purge_data
        && !yes
        && found
            .iter()
            .any(|(label, ..)| *label == "PersistentVolumeClaim")
        && !io.confirm()
    {
        return Err("cancelled, nothing was removed".to_owned());
    }
    for (label, kind, name) in found {
        cluster
            .delete(kind, Some(&namespace), &name)
            .await
            .map_err(|error| format!("{label} {name}: {error}"))?;
        io.line(&format!("{label} {namespace}/{name}: deleted"));
    }
    Ok(())
}

/// Whether the Deployment of `agent` exists, for `agent remove`'s list.
pub async fn is_deployed(cluster: &Cluster, namespace: &str, agent: &str) -> bool {
    matches!(
        cluster
            .get(&kinds::DEPLOYMENT, Some(namespace), agent)
            .await,
        Ok(Some(_))
    )
}

/// The Deployment is rolled out and ready (for `doctor`): `(ready, image, why not)`.
pub fn readiness(deployment: &Value) -> (bool, String, String) {
    let image = deployment["spec"]["template"]["spec"]["containers"][0]["image"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let status = &deployment["status"];
    let ready = rolled_out(deployment);
    let why = format!(
        "{} of {} replica(s) ready",
        status["readyReplicas"].as_u64().unwrap_or(0),
        deployment["spec"]["replicas"].as_u64().unwrap_or(1)
    );
    (ready, image, why)
}
