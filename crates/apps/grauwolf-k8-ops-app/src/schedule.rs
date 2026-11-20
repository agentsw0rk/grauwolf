//! `schedule add | list | remove | run-now` (ADR-0111 stage 3, Q7): a CronJob in namespace
//! `schedules` that calls an entry agent on a person's grant. Each schedule has its own service
//! account and SPIFFE ID (`spiffe://<td>/ns/schedules/sa/<name>`, a `ClusterSPIFFEID` of its own),
//! runs `grauwolf call` of the CLI image over mTLS with the SPIFFE CSI volume like an agent pod,
//! may reach only DNS and the agent's mTLS port, and is a caller of the agent (callers file and
//! NetworkPolicy, as `agent allow` does it). The grant itself is made by the person
//! (`grauwolf identity grant create`); the identity service's policy must let the schedule
//! trigger it (`identity allow --trigger <name>`).

use grauwolf_k8s::{Cluster, kinds};
use serde_json::{Value, json};

use crate::agent::{self, AGENT_LABEL, MANAGED_BY, MANAGER};
use crate::callers;
use crate::cli::ScheduleCommand;
use crate::io::Io;
use crate::oidc::SPIFFE_ID_TEMPLATE;
use crate::report;
use crate::services::Services;
use crate::spire;

/// The namespace of every schedule.
pub const NAMESPACE: &str = "schedules";
/// The label that names a schedule on its objects and pods.
pub const SCHEDULE_LABEL: &str = "grauwolf.io/schedule";
const GRANT_ANNOTATION: &str = "grauwolf.io/grant";
const NAMESPACE_NAME_LABEL: &str = "kubernetes.io/metadata.name";

/// The SPIFFE ID of the schedule `name`.
pub fn spiffe_id(trust_domain: &str, name: &str) -> String {
    spire::spiffe_id(trust_domain, NAMESPACE, name)
}

/// The arguments of `schedule add`.
pub struct AddOptions {
    pub name: String,
    pub agent: String,
    pub grant: String,
    pub cron: String,
    pub input: String,
    pub image: String,
    pub pull_secret: Option<String>,
}

pub async fn run(
    services: &Services,
    io: &mut Io<'_>,
    command: ScheduleCommand,
) -> Result<i32, String> {
    match command {
        ScheduleCommand::Add {
            name,
            agent,
            grant,
            cron,
            input,
            image,
            pull_secret,
        } => add(
            services,
            io,
            AddOptions {
                name,
                agent,
                grant,
                cron,
                input,
                image,
                pull_secret,
            },
        )
        .await
        .map(|()| 0),
        ScheduleCommand::List { json } => list(services, io, json).await.map(|()| 0),
        ScheduleCommand::Remove { name } => remove(services, io, &name).await.map(|()| 0),
        ScheduleCommand::RunNow { name } => run_now(services, io, &name).await.map(|()| 0),
    }
}

/// `grt_` and 32 lowercase hex digits (the identity service's grant IDs).
fn check_grant(grant: &str) -> Result<(), String> {
    let well_formed = grant.strip_prefix("grt_").is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if well_formed {
        Ok(())
    } else {
        Err(format!(
            "--grant '{grant}' is not a grant ID (grt_ and 32 hex digits, from `grauwolf identity grant create`)"
        ))
    }
}

/// Five fields of a cron line, or a macro such as `@hourly`; the API server checks the rest.
fn check_cron(cron: &str) -> Result<(), String> {
    let fields = cron.split_whitespace().count();
    if fields == 5 || (cron.starts_with('@') && fields == 1) {
        Ok(())
    } else {
        Err(format!(
            "--cron '{cron}' is not a cron schedule (five fields, e.g. \"0 7 * * 1-5\", or @hourly, @daily …)"
        ))
    }
}

fn namespace_object() -> Value {
    json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE, "labels": {
        MANAGED_BY: MANAGER,
        "grauwolf.io/component": "schedules",
        "pod-security.kubernetes.io/enforce": "restricted",
    }}})
}

fn object_labels(name: &str) -> Value {
    json!({MANAGED_BY: MANAGER, SCHEDULE_LABEL: name})
}

fn cluster_spiffe_id_name(name: &str) -> String {
    format!("grauwolf-schedule-{name}")
}

/// The schedule's own identity: pods with its label in namespace `schedules`.
fn cluster_spiffe_id_object(name: &str) -> Value {
    json!({"apiVersion": "spire.spiffe.io/v1alpha1", "kind": "ClusterSPIFFEID",
        "metadata": {"name": cluster_spiffe_id_name(name), "labels": object_labels(name)},
        "spec": {"spiffeIDTemplate": SPIFFE_ID_TEMPLATE,
            "namespaceSelector": {"matchLabels": {NAMESPACE_NAME_LABEL: NAMESPACE}},
            "podSelector": {"matchLabels": {SCHEDULE_LABEL: name}}}})
}

fn network_policy_name(name: &str) -> String {
    format!("grauwolf-schedule-{name}")
}

/// Egress of the schedule's pods: DNS and the agent's mTLS port, nothing else; no ingress.
fn network_policy_object(
    name: &str,
    agent: &str,
    agent_namespace: &str,
    mtls_port: u16,
    dns_namespace: &str,
    dns_selector: &str,
) -> Value {
    let (dns_key, dns_value) = dns_selector.split_once('=').unwrap_or((dns_selector, ""));
    json!({"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
        "metadata": {"name": network_policy_name(name), "namespace": NAMESPACE,
            "labels": object_labels(name)},
        "spec": {"podSelector": {"matchLabels": {SCHEDULE_LABEL: name}},
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [],
            "egress": [
                {"to": [{
                    "namespaceSelector": {"matchLabels": {NAMESPACE_NAME_LABEL: dns_namespace}},
                    "podSelector": {"matchLabels": {dns_key: dns_value}}}],
                 "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]},
                {"to": [{
                    "namespaceSelector": {"matchLabels": {NAMESPACE_NAME_LABEL: agent_namespace}},
                    "podSelector": {"matchLabels": {AGENT_LABEL: agent}}}],
                 "ports": [{"protocol": "TCP", "port": mtls_port}]}]}})
}

/// The CronJob: `grauwolf call` with the schedule's workload identity, one run at a time.
fn cron_job_object(options: &AddOptions, url: &str, agent_id: &str) -> Value {
    let name = &options.name;
    let command = vec![
        "grauwolf".to_owned(),
        "call".to_owned(),
        url.to_owned(),
        "--spiffe-id".to_owned(),
        agent_id.to_owned(),
        "--grant".to_owned(),
        options.grant.clone(),
        "--input".to_owned(),
        options.input.clone(),
    ];
    let pod = json!({
    "metadata": {"labels": {SCHEDULE_LABEL: name}},
    "spec": {
        "restartPolicy": "Never",
        "serviceAccountName": name,
        "automountServiceAccountToken": false,
        "securityContext": {"runAsNonRoot": true, "runAsUser": 65532, "runAsGroup": 65532,
            "seccompProfile": {"type": "RuntimeDefault"}},
        "containers": [{
            "name": "call",
            "image": options.image,
            "command": command,
            "securityContext": {"allowPrivilegeEscalation": false,
                "capabilities": {"drop": ["ALL"]}},
            "env": [
                {"name": "SPIFFE_ENDPOINT_SOCKET",
                    "value": "unix:///spiffe-workload-api/agent.sock"},
                // The identity may be issued a while after the pod starts (ADR-0120).
                {"name": "SPIFFE_FETCH_TIMEOUT", "value": "2m"}],
            "volumeMounts": [{"name": "spiffe-workload-api",
                "mountPath": "/spiffe-workload-api", "readOnly": true}],
        }],
        "volumes": [{"name": "spiffe-workload-api",
            "csi": {"driver": "csi.spiffe.io", "readOnly": true}}],
    }});
    json!({"apiVersion": "batch/v1", "kind": "CronJob",
        "metadata": {"name": name, "namespace": NAMESPACE,
            "labels": {MANAGED_BY: MANAGER, SCHEDULE_LABEL: name, AGENT_LABEL: options.agent},
            "annotations": {GRANT_ANNOTATION: options.grant}},
        "spec": {"schedule": options.cron, "concurrencyPolicy": "Forbid",
            "successfulJobsHistoryLimit": 3, "failedJobsHistoryLimit": 3,
            "jobTemplate": {"metadata": {"labels": {MANAGED_BY: MANAGER, SCHEDULE_LABEL: name}},
                "spec": {"backoffLimit": 0, "ttlSecondsAfterFinished": 86_400,
                    "template": pod}}}})
}

/// The user a schedule is in its agent's callers file (the grant's person replaces it in runs).
fn caller_user(name: &str) -> String {
    format!("schedule-{name}")
}

async fn add(services: &Services, io: &mut Io<'_>, options: AddOptions) -> Result<(), String> {
    let cluster = &services.cluster;
    // Refuse before anything is written.
    agent::validate_name(&options.name)?;
    check_grant(&options.grant)?;
    check_cron(&options.cron)?;
    if options.input.is_empty() {
        return Err("--input is empty".into());
    }
    let agent_ns = agent::find_namespace(cluster, &options.agent)
        .await?
        .ok_or_else(|| format!("agent {} not found", options.agent))?;
    let domain = spire::trust_domain(cluster, &services.settings).await?;
    if let Some(existing) = cluster
        .get(&kinds::NAMESPACE, None, NAMESPACE)
        .await
        .map_err(|error| format!("namespace {NAMESPACE}: {error}"))?
        && existing["metadata"]["labels"][MANAGED_BY] != MANAGER
    {
        return Err(format!(
            "namespace {NAMESPACE} exists and is not managed by {MANAGER}"
        ));
    }
    let name = options.name.as_str();
    let (_, mtls_port) = callers::ports_of(cluster, &agent_ns, &options.agent).await;
    let agent_id = spire::spiffe_id(&domain, &agent_ns, &options.agent);
    let own_id = spiffe_id(&domain, name);
    let url = format!("https://{}.{agent_ns}.svc:{mtls_port}/run", options.agent);

    let outcome = cluster
        .ensure(&kinds::NAMESPACE, None, &namespace_object())
        .await
        .map_err(|error| format!("namespace {NAMESPACE}: {error}"))?;
    io.line(&format!(
        "namespace {NAMESPACE} (restricted): {}",
        report::k8s(outcome)
    ));
    let pull_secret = match &options.pull_secret {
        Some(reference) => Some(agent::copy_pull_secret(cluster, io, reference, NAMESPACE).await?),
        None => None,
    };
    let mut account = agent::service_account_object(name, NAMESPACE, pull_secret.as_deref());
    account["metadata"]["labels"][SCHEDULE_LABEL] = json!(name);
    let outcome = cluster
        .ensure(&kinds::SERVICE_ACCOUNT, Some(NAMESPACE), &account)
        .await
        .map_err(|error| format!("ServiceAccount {NAMESPACE}/{name}: {error}"))?;
    io.line(&format!(
        "ServiceAccount {NAMESPACE}/{name}: {}",
        report::k8s(outcome)
    ));
    let outcome = cluster
        .ensure(
            &kinds::CLUSTER_SPIFFE_ID,
            None,
            &cluster_spiffe_id_object(name),
        )
        .await
        .map_err(|error| format!("ClusterSPIFFEID {}: {error}", cluster_spiffe_id_name(name)))?;
    io.line(&format!(
        "ClusterSPIFFEID {} ({own_id}): {}",
        cluster_spiffe_id_name(name),
        report::k8s(outcome)
    ));
    let outcome = cluster
        .ensure(
            &kinds::NETWORK_POLICY,
            Some(NAMESPACE),
            &network_policy_object(
                name,
                &options.agent,
                &agent_ns,
                mtls_port,
                crate::egress::DEFAULT_DNS_NAMESPACE,
                crate::egress::DEFAULT_DNS_SELECTOR,
            ),
        )
        .await
        .map_err(|error| format!("NetworkPolicy {}: {error}", network_policy_name(name)))?;
    io.line(&format!(
        "NetworkPolicy {NAMESPACE}/{} (egress: DNS and {}.{agent_ns} port {mtls_port}): {}",
        network_policy_name(name),
        options.agent,
        report::k8s(outcome)
    ));
    let outcome = cluster
        .ensure(
            &kinds::CRON_JOB,
            Some(NAMESPACE),
            &cron_job_object(&options, &url, &agent_id),
        )
        .await
        .map_err(|error| format!("CronJob {NAMESPACE}/{name}: {error}"))?;
    io.line(&format!(
        "CronJob {NAMESPACE}/{name} (\"{}\", grauwolf call {url} --grant {}): {}",
        options.cron,
        options.grant,
        report::k8s(outcome)
    ));
    // The agent admits the schedule as a caller.
    callers::admit(
        cluster,
        io,
        &options.agent,
        &agent_ns,
        &own_id,
        &caller_user(name),
    )
    .await?;
    hint_policy(cluster, io, &agent_id, &own_id).await;
    Ok(())
}

/// Says what the identity service's policy still lacks for runs of this schedule.
async fn hint_policy(cluster: &Cluster, io: &mut Io<'_>, agent_id: &str, own_id: &str) {
    let Ok(Some(policy)) = crate::identity::read_policy(cluster).await else {
        io.line("the identity service is not installed: grant runs get 503 until `grauwolf-k8-ops identity install --grant-client-id …`");
        return;
    };
    let covered = |id: &str, keys: Vec<&String>| {
        keys.into_iter().any(|key| {
            key == id
                || key.strip_suffix("/*").is_some_and(|prefix| {
                    id.strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('/'))
                })
        })
    };
    if !covered(agent_id, policy.grant_agents.keys().collect()) {
        io.line(&format!(
            "the identity service's policy does not let {agent_id} run on grants yet: `grauwolf-k8-ops identity allow <agent> --grants`"
        ));
    }
    if !covered(own_id, policy.triggers.iter().collect()) {
        io.line(&format!(
            "the identity service's policy does not let {own_id} trigger grants yet: `grauwolf-k8-ops identity allow --trigger <name>`"
        ));
    }
}

/// The schedules: name, cron, agent, grant, last run.
async fn list(services: &Services, io: &mut Io<'_>, as_json: bool) -> Result<(), String> {
    let jobs = services
        .cluster
        .list(
            &kinds::CRON_JOB,
            Some(NAMESPACE),
            Some(&format!("{MANAGED_BY}={MANAGER}")),
        )
        .await
        .map_err(|error| format!("CronJobs in {NAMESPACE}: {error}"))?;
    let rows: Vec<Value> = jobs
        .iter()
        .map(|job| {
            json!({
                "name": job["metadata"]["name"],
                "cron": job["spec"]["schedule"],
                "agent": job["metadata"]["labels"][AGENT_LABEL],
                "grant": job["metadata"]["annotations"][GRANT_ANNOTATION],
                "suspended": job["spec"]["suspend"].as_bool().unwrap_or(false),
                "last_schedule": job["status"]["lastScheduleTime"],
                "last_success": job["status"]["lastSuccessfulTime"],
            })
        })
        .collect();
    if as_json {
        io.line(&json!({ "schedules": rows }).to_string());
        return Ok(());
    }
    if rows.is_empty() {
        io.line("no schedules");
    }
    let text = |value: &Value| value.as_str().unwrap_or("-").to_owned();
    for row in &rows {
        io.line(&format!(
            "{}  \"{}\"  agent {}  grant {}  last run {}  last success {}{}",
            text(&row["name"]),
            text(&row["cron"]),
            text(&row["agent"]),
            text(&row["grant"]),
            text(&row["last_schedule"]),
            text(&row["last_success"]),
            if row["suspended"] == true {
                "  (suspended)"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

async fn existing(cluster: &Cluster, name: &str) -> Result<Value, String> {
    agent::validate_name(name)?;
    cluster
        .get(&kinds::CRON_JOB, Some(NAMESPACE), name)
        .await
        .map_err(|error| format!("CronJob {NAMESPACE}/{name}: {error}"))?
        .filter(|job| job["metadata"]["labels"][MANAGED_BY] == MANAGER)
        .ok_or_else(|| format!("schedule {name} not found (`grauwolf-k8-ops schedule list`)"))
}

/// Removes the CronJob, its NetworkPolicy, identity and service account, and the schedule's
/// entry in its agent's callers.
async fn remove(services: &Services, io: &mut Io<'_>, name: &str) -> Result<(), String> {
    let cluster = &services.cluster;
    let job = existing(cluster, name).await?;
    let agent = job["metadata"]["labels"][AGENT_LABEL]
        .as_str()
        .map(str::to_owned);
    for (kind, namespace, object) in [
        (&kinds::CRON_JOB, Some(NAMESPACE), name.to_owned()),
        (
            &kinds::NETWORK_POLICY,
            Some(NAMESPACE),
            network_policy_name(name),
        ),
        (
            &kinds::CLUSTER_SPIFFE_ID,
            None,
            cluster_spiffe_id_name(name),
        ),
        (&kinds::SERVICE_ACCOUNT, Some(NAMESPACE), name.to_owned()),
    ] {
        let deleted = cluster
            .delete(kind, namespace, &object)
            .await
            .map_err(|error| format!("{} {object}: {error}", kind.kind))?;
        io.line(&format!(
            "{} {object}: {}",
            kind.kind,
            if deleted { "deleted" } else { "not there" }
        ));
    }
    if let Some(agent) = agent
        && let Some(agent_ns) = agent::find_namespace(cluster, &agent).await?
    {
        let domain = spire::trust_domain(cluster, &services.settings).await?;
        callers::release(cluster, io, &agent, &agent_ns, &spiffe_id(&domain, name)).await?;
    }
    Ok(())
}

/// Starts one run now: a Job from the CronJob's template.
async fn run_now(services: &Services, io: &mut Io<'_>, name: &str) -> Result<(), String> {
    let cluster = &services.cluster;
    let job = existing(cluster, name).await?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let job_name = format!("{name}-manual-{stamp}");
    let template = &job["spec"]["jobTemplate"];
    let object = json!({"apiVersion": "batch/v1", "kind": "Job",
        "metadata": {"name": job_name, "namespace": NAMESPACE,
            "labels": template["metadata"]["labels"],
            "annotations": {"cronjob.kubernetes.io/instantiate": "manual"}},
        "spec": template["spec"]});
    cluster
        .ensure(&kinds::JOB, Some(NAMESPACE), &object)
        .await
        .map_err(|error| format!("Job {NAMESPACE}/{job_name}: {error}"))?;
    io.line(&format!(
        "Job {NAMESPACE}/{job_name}: created (logs: kubectl -n {NAMESPACE} logs job/{job_name})"
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_and_cron_lines_are_checked() {
        assert!(check_grant("grt_0123456789abcdef0123456789abcdef").is_ok());
        assert!(check_grant("grt_x").is_err());
        assert!(check_grant("0123456789abcdef0123456789abcdef").is_err());
        assert!(check_cron("*/5 * * * *").is_ok());
        assert!(check_cron("@hourly").is_ok());
        assert!(check_cron("* * *").is_err());
        assert!(check_cron("@ hourly").is_err());
    }
}
