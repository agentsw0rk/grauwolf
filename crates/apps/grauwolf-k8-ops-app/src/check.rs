//! `agent check`: the credentials check of ADR-0110 as a Job in the agent's namespace.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant};

use grauwolf_k8s::{Cluster, kinds};
use serde_json::{Value, json};

use crate::agent::{self, CONFIG_KEY, CONFIG_MAP, MANAGED_BY, MANAGER};
use crate::io::Io;
use crate::services::Services;

const POLL: Duration = Duration::from_millis(250);

/// `grauwolf-check-<random>`.
fn job_name() -> String {
    let random = RandomState::new().build_hasher().finish();
    format!("grauwolf-check-{:06x}", random & 0xff_ffff)
}

/// The Job of the example `30-grauwolf-job.yaml`, for the restricted Pod Security Standard.
pub fn job_object(
    name: &str,
    namespace: &str,
    agent: &str,
    image: &str,
    variables: &[String],
    timeout: u64,
) -> Value {
    let mut command = vec![
        "grauwolf".to_owned(),
        "credentials".to_owned(),
        "check".to_owned(),
    ];
    command.extend(variables.iter().cloned());
    command.extend(["--config".to_owned(), format!("/etc/grauwolf/{CONFIG_KEY}")]);
    json!({"apiVersion": "batch/v1", "kind": "Job",
    "metadata": {"name": name, "namespace": namespace, "labels": {MANAGED_BY: MANAGER}},
    "spec": {"backoffLimit": 0, "ttlSecondsAfterFinished": 600, "activeDeadlineSeconds": timeout,
        "template": {
            "metadata": {"labels": {"app.kubernetes.io/name": "grauwolf"}},
            "spec": {
                "restartPolicy": "Never",
                "serviceAccountName": agent,
                // The user is numeric: the image's user `nonroot` is a name, which
                // runAsNonRoot cannot check.
                "securityContext": {"runAsNonRoot": true, "runAsUser": 65532, "runAsGroup": 65532,
                    "seccompProfile": {"type": "RuntimeDefault"}},
                "containers": [{
                    "name": "grauwolf",
                    "image": image,
                    "command": command,
                    "securityContext": {"allowPrivilegeEscalation": false,
                        "capabilities": {"drop": ["ALL"]}},
                    "env": [{"name": "SPIFFE_ENDPOINT_SOCKET",
                        "value": "unix:///spiffe-workload-api/agent.sock"}],
                    "volumeMounts": [
                        {"name": "config", "mountPath": "/etc/grauwolf", "readOnly": true},
                        {"name": "spiffe-workload-api", "mountPath": "/spiffe-workload-api", "readOnly": true}],
                }],
                "volumes": [
                    {"name": "config", "configMap": {"name": CONFIG_MAP}},
                    {"name": "spiffe-workload-api", "csi": {"driver": "csi.spiffe.io", "readOnly": true}}],
            }}}})
}

enum Finished {
    Succeeded,
    Failed,
    TimedOut,
}

pub async fn run(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    image: &str,
    variables: Vec<String>,
    timeout: u64,
) -> Result<i32, String> {
    agent::validate_name(agent)?;
    let cluster = &services.cluster;
    let namespace = agent::find_namespace(cluster, agent)
        .await?
        .ok_or_else(|| {
            format!("agent {agent} not found (create it with `grauwolf-k8-ops agent add {agent}`)")
        })?;
    let config = agent::current_config(cluster, &namespace)
        .await?
        .ok_or_else(|| format!("ConfigMap {namespace}/{CONFIG_MAP} not found: run `grauwolf-k8-ops agent add {agent}`"))?;
    if let Some(hint) = crate::identity::hint_for(cluster, &namespace, agent).await {
        io.error(&format!("hint: {hint}"));
    }
    let mapped = agent::variables_of(&config);
    for name in &variables {
        if !mapped.contains_key(name) {
            return Err(format!(
                "{name} is not mapped in the configuration of agent {agent}"
            ));
        }
    }
    // Without `--variable` every mapped variable is checked.
    let variables = if variables.is_empty() {
        mapped.keys().cloned().collect()
    } else {
        let mut variables = variables;
        variables.sort();
        variables.dedup();
        variables
    };
    if variables.is_empty() {
        return Err(format!(
            "agent {agent} maps no variable: `grauwolf-k8-ops secret set {agent} <key> --variable <NAME>`"
        ));
    }

    let name = job_name();
    let job = job_object(&name, &namespace, agent, image, &variables, timeout);
    io.line(&format!(
        "running Job {namespace}/{name} (check of {}, image {image})",
        variables.join(", ")
    ));
    cluster
        .ensure(&kinds::JOB, Some(&namespace), &job)
        .await
        .map_err(|error| format!("Job {name}: {error}"))?;

    // The Job is deleted whatever happens while waiting.
    let finished = wait(cluster, &namespace, &name, Duration::from_secs(timeout)).await;
    let log = job_log(cluster, &namespace, &name).await;
    let deleted = cluster.delete(&kinds::JOB, Some(&namespace), &name).await;
    let finished = finished?;
    deleted.map_err(|error| format!("cannot delete Job {name}: {error}"))?;
    io.line(&format!("Job {name}: deleted"));

    if log.is_empty() {
        io.line(&format!("agent {agent}: (no log)"));
    }
    for line in &log {
        io.line(&format!("agent {agent}: {line}"));
        if let Some(hint) = permission_hint(services, agent, &mapped, line) {
            io.line(&format!("agent {agent}: hint: {hint}"));
        }
    }
    match finished {
        Finished::Succeeded => Ok(0),
        Finished::Failed => {
            io.line(&format!("agent {agent}: the check failed"));
            Ok(1)
        }
        Finished::TimedOut => {
            io.line(&format!(
                "agent {agent}: the check did not finish within {timeout} s"
            ));
            Ok(1)
        }
    }
}

/// For a line of the log that says `permission denied` when reading a mapped variable's secret
/// (not for a refused login): what the agent's policy allows and the path that was tried.
fn permission_hint(
    services: &Services,
    agent: &str,
    mapped: &BTreeMap<String, String>,
    line: &str,
) -> Option<String> {
    if !line.contains("permission denied") || line.contains("login") {
        return None;
    }
    let (variable, source) = mapped
        .iter()
        .find(|(variable, _)| line.contains(&format!("credentials: {variable} from ")))?;
    let location = source
        .strip_prefix("openbao:")
        .or_else(|| source.strip_prefix("vault:"))?
        .split('#')
        .next()?;
    let (mount, path) = location.split_once('/')?;
    Some(format!(
        "the policy {} of the agent only reads {}/data/{}/*; {variable} maps to {source}, which reads {mount}/data/{path} - store the secret below that prefix (`secret set {agent} <key> --variable {variable}`)",
        agent::role_name(agent),
        services.settings.kv_mount,
        agent::kv_prefix(agent),
    ))
}

async fn wait(
    cluster: &Cluster,
    namespace: &str,
    name: &str,
    timeout: Duration,
) -> Result<Finished, String> {
    let started = Instant::now();
    loop {
        let job = cluster
            .get(&kinds::JOB, Some(namespace), name)
            .await
            .map_err(|error| format!("Job {name}: {error}"))?
            .ok_or_else(|| format!("Job {name} disappeared"))?;
        if job["status"]["succeeded"].as_u64().unwrap_or(0) > 0 {
            return Ok(Finished::Succeeded);
        }
        if job["status"]["failed"].as_u64().unwrap_or(0) > 0 {
            return Ok(Finished::Failed);
        }
        if started.elapsed() >= timeout {
            return Ok(Finished::TimedOut);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The non-empty lines of the log of the Job's pod.
async fn job_log(cluster: &Cluster, namespace: &str, job: &str) -> Vec<String> {
    let Ok(pods) = cluster
        .list(
            &kinds::POD,
            Some(namespace),
            Some(&format!("job-name={job}")),
        )
        .await
    else {
        return Vec::new();
    };
    let Some(pod) = pods
        .first()
        .and_then(|pod| pod["metadata"]["name"].as_str())
    else {
        return Vec::new();
    };
    let Ok(log) = cluster.pod_log(namespace, pod, 200).await else {
        return Vec::new();
    };
    log.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}
