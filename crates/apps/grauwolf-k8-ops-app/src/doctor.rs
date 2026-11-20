//! `doctor` (read-only): first the prerequisites that grauwolf-k8-ops does not install, then
//! the setup that it does. One line per check: ok, warn or FAIL, with the reason and, where one
//! exists, the next step. Exit code 1 when a check fails.

use std::collections::BTreeMap;

use grauwolf_k8s::{Cluster, kinds};
use grauwolf_openbao::{jwt_role, kv_read_policy};
use serde_json::{Value, json};

use crate::agent::{self, CLUSTER_SPIFFE_ID, CONFIG_MAP};
use crate::agent_files::AGENT_MAP;
use crate::caller_spec;
use crate::callers::{self, CALLERS_MAP, NETWORK_POLICY, PolicyState};
use crate::cli::Settings;
use crate::deploy;
use crate::egress;
use crate::identity;
use crate::io::Io;
use crate::oidc::service_host;
use crate::openbao_setup;
use crate::services::Services;
use crate::spire::{self, OIDC_NAME};

const NO_SPIRE: &str = "SPIRE is not installed - grauwolf-k8-ops does not install SPIRE";
const PREREQUISITES: &str = "prerequisites";
const SETUP: &str = "setup";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

pub struct Check {
    pub section: &'static str,
    pub check: String,
    pub status: Status,
    pub reason: String,
    pub hint: Option<String>,
}

#[derive(Default)]
struct Report {
    checks: Vec<Check>,
}

impl Report {
    fn add(
        &mut self,
        section: &'static str,
        check: &str,
        status: Status,
        reason: impl Into<String>,
        hint: Option<&str>,
    ) {
        self.checks.push(Check {
            section,
            check: check.to_owned(),
            status,
            reason: reason.into(),
            hint: hint.map(str::to_owned),
        });
    }

    fn ok(&mut self, section: &'static str, check: &str, reason: impl Into<String>) {
        self.add(section, check, Status::Ok, reason, None);
    }

    fn warn(&mut self, section: &'static str, check: &str, reason: impl Into<String>, hint: &str) {
        self.add(section, check, Status::Warn, reason, Some(hint));
    }

    fn fail(&mut self, section: &'static str, check: &str, reason: impl Into<String>, hint: &str) {
        self.add(section, check, Status::Fail, reason, Some(hint));
    }

    fn failed(&self) -> bool {
        self.checks.iter().any(|check| check.status == Status::Fail)
    }
}

pub async fn run(services: &Services, io: &mut Io<'_>, json: bool) -> Result<i32, String> {
    let report = collect(services).await;
    if json {
        let rows: Vec<Value> = report
            .checks
            .iter()
            .map(|check| {
                json!({"section": check.section, "check": check.check, "status": check.status.word(),
                    "reason": check.reason, "hint": check.hint})
            })
            .collect();
        io.line(&Value::Array(rows).to_string());
    } else {
        // Which cluster this is about, before anything else.
        io.line(&format!("Cluster {}", services.cluster.describe()));
        for (section, title) in [
            (
                PREREQUISITES,
                "Prerequisites (not installed by grauwolf-k8-ops)",
            ),
            (SETUP, "Setup (installed by grauwolf-k8-ops)"),
        ] {
            io.line(title);
            for check in report
                .checks
                .iter()
                .filter(|check| check.section == section)
            {
                io.line(&format!(
                    "  {:<5} {}: {}",
                    check.status.word(),
                    check.check,
                    check.reason
                ));
                if let Some(hint) = &check.hint {
                    io.line(&format!("        next: {hint}"));
                }
            }
        }
    }
    Ok(if report.failed() { 1 } else { 0 })
}

async fn collect(services: &Services) -> Report {
    let mut report = Report::default();
    if prerequisites(services, &mut report).await {
        setup(services, &mut report).await;
    }
    report
}

/// A resource that grauwolf-k8-ops writes, with the verbs it needs.
struct Right {
    group: &'static str,
    resource: &'static str,
    verbs: &'static [&'static str],
    why: &'static str,
}

const RIGHTS: &[Right] = &[
    Right {
        group: "",
        resource: "namespaces",
        verbs: &["get", "list", "create", "delete"],
        why: "agent add/remove",
    },
    Right {
        group: "",
        resource: "serviceaccounts",
        verbs: &["get", "create", "patch"],
        why: "agent add",
    },
    Right {
        group: "",
        resource: "configmaps",
        verbs: &["get", "create", "patch"],
        why: "agent add, agent allow/deny, secret set --variable",
    },
    Right {
        group: "networking.k8s.io",
        resource: "networkpolicies",
        verbs: &["get", "create", "patch"],
        why: "agent allow/deny",
    },
    Right {
        group: "",
        resource: "services",
        verbs: &["get", "create", "patch"],
        why: "oidc install",
    },
    Right {
        group: "apps",
        resource: "deployments",
        verbs: &["get", "create", "patch"],
        why: "oidc install",
    },
    Right {
        group: "batch",
        resource: "jobs",
        verbs: &["get", "create", "delete"],
        why: "agent check",
    },
    Right {
        group: "",
        resource: "pods",
        verbs: &["list"],
        why: "agent check",
    },
    Right {
        group: "",
        resource: "pods/log",
        verbs: &["get"],
        why: "agent check",
    },
    Right {
        group: "spire.spiffe.io",
        resource: "clusterspiffeids",
        verbs: &["get", "list", "create", "patch"],
        why: "agent add, oidc install",
    },
];

/// `false` if the cluster cannot be reached at all (the rest cannot be checked).
async fn prerequisites(services: &Services, report: &mut Report) -> bool {
    let cluster = &services.cluster;
    let settings = &services.settings;
    let spire_ns = settings.spire_namespace.as_str();

    let version = match cluster.version().await {
        Ok(version) => {
            report.ok(
                PREREQUISITES,
                "cluster",
                format!("reachable, Kubernetes {version}, {}", cluster.describe()),
            );
            version
        }
        Err(error) => {
            report.fail(
                PREREQUISITES,
                "cluster",
                format!("not reachable: {error}"),
                "check the kubeconfig (--kubeconfig, KUBECONFIG) and the network",
            );
            return false;
        }
    };

    for right in RIGHTS {
        let mut missing = Vec::new();
        let mut failure = None;
        for verb in right.verbs {
            match cluster.can_i(verb, right.group, right.resource, None).await {
                Ok(true) => {}
                Ok(false) => missing.push(*verb),
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            }
        }
        let check = format!("rights on {}", right.resource);
        match (failure, missing.is_empty()) {
            (Some(error), _) => report.fail(
                PREREQUISITES,
                &check,
                format!("cannot ask the API server: {error}"),
                "check the kubeconfig's rights to create SelfSubjectAccessReviews",
            ),
            (None, true) => report.ok(
                PREREQUISITES,
                &check,
                format!("{} allowed ({})", right.verbs.join(", "), right.why),
            ),
            (None, false) => report.fail(
                PREREQUISITES,
                &check,
                format!(
                    "not allowed: {} (needed for {})",
                    missing.join(", "),
                    right.why
                ),
                "ask the cluster administrator to grant these rights to the kubeconfig's user",
            ),
        }
    }
    if services.settings.openbao_addr.is_none() {
        let namespace = settings.openbao_namespace.as_str();
        let mut missing = Vec::new();
        for verb in ["get", "create", "update", "delete"] {
            if !cluster
                .can_i(verb, "", "services/proxy", Some(namespace))
                .await
                .unwrap_or(false)
            {
                missing.push(verb);
            }
        }
        if missing.is_empty() {
            report.ok(PREREQUISITES, "rights on services/proxy", format!("get, create, update, delete allowed in {namespace} (OpenBao through the API server)"));
        } else {
            report.fail(
                PREREQUISITES,
                "rights on services/proxy",
                format!("not allowed in {namespace}: {}", missing.join(", ")),
                "grant these rights, or reach OpenBao directly with --openbao-addr",
            );
        }
    }

    match cluster
        .get(&kinds::STATEFUL_SET, Some(spire_ns), "spire-server")
        .await
    {
        Ok(Some(server)) => {
            let (wanted, ready) = spire::replicas(&server);
            if wanted > 0 && ready == wanted {
                report.ok(
                    PREREQUISITES,
                    "SPIRE server",
                    format!("StatefulSet {spire_ns}/spire-server ready ({ready}/{wanted})"),
                );
            } else {
                report.fail(
                    PREREQUISITES,
                    "SPIRE server",
                    format!("StatefulSet {spire_ns}/spire-server not ready ({ready}/{wanted})"),
                    &format!("look at the pods in {spire_ns}"),
                );
            }
        }
        Ok(None) => report.fail(
            PREREQUISITES,
            "SPIRE server",
            format!("StatefulSet {spire_ns}/spire-server not found"),
            NO_SPIRE,
        ),
        Err(error) => report.fail(
            PREREQUISITES,
            "SPIRE server",
            error.to_string(),
            "check the rights on statefulsets",
        ),
    }

    let nodes = cluster
        .list(&kinds::NODE, None, None)
        .await
        .map(|nodes| nodes.len());
    match cluster
        .get(&kinds::DAEMON_SET, Some(spire_ns), "spire-agent")
        .await
    {
        Ok(Some(agents)) => {
            let (wanted, ready) = spire::replicas(&agents);
            let nodes = nodes.unwrap_or(wanted as usize) as u64;
            if wanted > 0 && wanted == ready && ready == nodes {
                report.ok(
                    PREREQUISITES,
                    "SPIRE agents",
                    format!("DaemonSet {spire_ns}/spire-agent ready on all {nodes} node(s)"),
                );
            } else {
                report.fail(PREREQUISITES, "SPIRE agents", format!("DaemonSet {spire_ns}/spire-agent: {ready} ready of {wanted} wanted, {nodes} node(s)"), "every node needs a ready SPIRE agent: look at the DaemonSet's pods");
            }
        }
        Ok(None) => report.fail(
            PREREQUISITES,
            "SPIRE agents",
            format!("DaemonSet {spire_ns}/spire-agent not found"),
            NO_SPIRE,
        ),
        Err(error) => report.fail(
            PREREQUISITES,
            "SPIRE agents",
            error.to_string(),
            "check the rights on daemonsets",
        ),
    }

    match cluster.get(&kinds::CSI_DRIVER, None, "csi.spiffe.io").await {
        Ok(Some(_)) => report.ok(PREREQUISITES, "SPIFFE CSI driver", "CSIDriver csi.spiffe.io exists"),
        Ok(None) => report.fail(PREREQUISITES, "SPIFFE CSI driver", "CSIDriver csi.spiffe.io not found", "install the SPIFFE CSI driver (pods get the Workload API socket through it); grauwolf-k8-ops does not install it"),
        Err(error) => report.fail(PREREQUISITES, "SPIFFE CSI driver", error.to_string(), "check the rights on csidrivers"),
    }

    match cluster
        .get(&kinds::CRD, None, "clusterspiffeids.spire.spiffe.io")
        .await
    {
        Ok(Some(_)) => report.ok(
            PREREQUISITES,
            "ClusterSPIFFEID CRD",
            "clusterspiffeids.spire.spiffe.io is installed",
        ),
        Ok(None) => report.fail(
            PREREQUISITES,
            "ClusterSPIFFEID CRD",
            "clusterspiffeids.spire.spiffe.io not found",
            NO_SPIRE,
        ),
        Err(error) => report.fail(
            PREREQUISITES,
            "ClusterSPIFFEID CRD",
            error.to_string(),
            "check the rights on customresourcedefinitions",
        ),
    }

    controller_manager(cluster, spire_ns, report).await;

    openbao_prerequisites(services, report).await;

    let minor = version
        .trim_start_matches('v')
        .split('.')
        .nth(1)
        .and_then(|minor| minor.parse::<u32>().ok());
    match minor {
        Some(minor) if minor >= 25 => report.ok(PREREQUISITES, "Pod Security Admission", format!("built into Kubernetes {version} (since 1.25); `agent check` shows whether pods that break the restricted standard are refused")),
        _ => report.warn(PREREQUISITES, "Pod Security Admission", format!("Kubernetes {version}: cannot tell whether the restricted standard is enforced"), "enable the PodSecurity admission plugin; otherwise the label on the agent namespaces has no effect"),
    }
    true
}

async fn controller_manager(cluster: &Cluster, spire_ns: &str, report: &mut Report) {
    // The Helm chart runs it as a container of the SPIRE server's pod; it can also be a
    // Deployment of its own.
    let deployment = cluster
        .get(
            &kinds::DEPLOYMENT,
            Some(spire_ns),
            "spire-controller-manager",
        )
        .await;
    if let Ok(Some(deployment)) = &deployment {
        let (wanted, ready) = spire::replicas(deployment);
        if wanted > 0 && ready == wanted {
            report.ok(
                PREREQUISITES,
                "spire-controller-manager",
                format!("Deployment {spire_ns}/spire-controller-manager ready"),
            );
        } else {
            report.fail(
                PREREQUISITES,
                "spire-controller-manager",
                format!(
                    "Deployment {spire_ns}/spire-controller-manager not ready ({ready}/{wanted})"
                ),
                &format!("look at its pods in {spire_ns}"),
            );
        }
        return;
    }
    if let Ok(Some(server)) = cluster
        .get(&kinds::STATEFUL_SET, Some(spire_ns), "spire-server")
        .await
    {
        let sidecar = server["spec"]["template"]["spec"]["containers"]
            .as_array()
            .is_some_and(|containers| {
                containers.iter().any(|c| {
                    c["name"]
                        .as_str()
                        .is_some_and(|name| name.contains("controller-manager"))
                })
            });
        let (wanted, ready) = spire::replicas(&server);
        if sidecar && wanted > 0 && ready == wanted {
            report.ok(
                PREREQUISITES,
                "spire-controller-manager",
                "runs as a container of the spire-server pod",
            );
            return;
        }
    }
    report.fail(
        PREREQUISITES,
        "spire-controller-manager",
        "not running (no ready Deployment, no container in the spire-server pod)",
        "SPIRE is not complete - grauwolf-k8-ops does not install spire-controller-manager",
    );
}

/// One required capability group: any of the names.
type Group = &'static [&'static str];

async fn openbao_prerequisites(services: &Services, report: &mut Report) {
    let settings = &services.settings;
    let reach_hint = "check --openbao-namespace and --openbao-service, or pass --openbao-addr";
    match services.bao.health().await {
        Ok(health) if health.sealed => report.fail(
            PREREQUISITES,
            "OpenBao",
            "reachable but sealed",
            "unseal OpenBao (`bao operator unseal`)",
        ),
        Ok(health) if !health.initialized => report.fail(
            PREREQUISITES,
            "OpenBao",
            "reachable but not initialized",
            "initialize OpenBao (`bao operator init`); grauwolf-k8-ops does not install it",
        ),
        Ok(health) => report.ok(
            PREREQUISITES,
            "OpenBao",
            format!(
                "reachable, initialized, unsealed (version {})",
                health.version.as_deref().unwrap_or("unknown")
            ),
        ),
        Err(error) => {
            report.fail(
                PREREQUISITES,
                "OpenBao",
                format!("not reachable: {error}"),
                reach_hint,
            );
            return;
        }
    }
    if !services.has_token {
        report.warn(
            PREREQUISITES,
            "OpenBao token",
            "no token given, the OpenBao checks that need one are skipped",
            "set OPENBAO_TOKEN or pass --token-file",
        );
        return;
    }
    let jwt = settings.jwt_mount.as_str();
    let kv = settings.kv_mount.as_str();
    match services.bao.token_policies().await {
        Ok(policies) if policies.iter().any(|policy| policy == "root") => report.warn(
            PREREQUISITES,
            "OpenBao token policy",
            "working with the root token",
            "create a restricted one with `grauwolf-k8-ops openbao token`",
        ),
        Ok(policies)
            if policies
                .iter()
                .any(|policy| policy == openbao_setup::POLICY) =>
        {
            report.ok(
                PREREQUISITES,
                "OpenBao token policy",
                format!("the token has the policy {}", openbao_setup::POLICY),
            )
        }
        Ok(policies) => report.ok(
            PREREQUISITES,
            "OpenBao token policy",
            format!("the token has the policies {}", policies.join(", ")),
        ),
        // An invalid token is reported by the capabilities check below.
        Err(_) => {}
    }
    let paths: Vec<(String, Vec<Group>)> = vec![
        (
            format!("sys/auth/{jwt}"),
            vec![&["sudo"], &["create", "update"]],
        ),
        (format!("sys/mounts/{kv}"), vec![&["create", "update"]]),
        (
            "sys/policies/acl/agent-doctor".to_owned(),
            vec![&["create", "update"]],
        ),
        (
            format!("auth/{jwt}/role/agent-doctor"),
            vec![&["create", "update"]],
        ),
        (
            format!("{kv}/data/agents/doctor"),
            vec![&["create", "update"]],
        ),
    ];
    let names: Vec<&str> = paths.iter().map(|(path, _)| path.as_str()).collect();
    let capabilities = match services.bao.capabilities_self(&names).await {
        Ok(capabilities) => capabilities,
        Err(error) => {
            report.fail(PREREQUISITES, "OpenBao token", format!("not valid: {error}"), "use a valid token (OPENBAO_TOKEN or --token-file) with the policies the setup needs");
            return;
        }
    };
    let mut lacking = Vec::new();
    for (path, groups) in &paths {
        let have = &capabilities[path];
        let root = have.iter().any(|capability| capability == "root");
        for group in groups {
            if !root
                && !group
                    .iter()
                    .any(|needed| have.iter().any(|capability| capability == needed))
            {
                lacking.push(format!("{path} ({})", group.join(" or ")));
            }
        }
    }
    if lacking.is_empty() {
        report.ok(
            PREREQUISITES,
            "OpenBao token",
            "valid; may configure the auth method, mount, policies, roles and agent secrets",
        );
    } else {
        report.fail(
            PREREQUISITES,
            "OpenBao token",
            format!("valid, but lacks: {}", lacking.join(", ")),
            "use a token whose policies grant these capabilities (an admin token for the setup)",
        );
    }
}

async fn setup(services: &Services, report: &mut Report) {
    let settings = &services.settings;
    let cluster = &services.cluster;
    let install_oidc = "run `grauwolf-k8-ops oidc install`";
    let run_setup = "run `grauwolf-k8-ops openbao setup`";
    let oidc_ns = settings.oidc_namespace.as_str();

    match cluster
        .get(&kinds::DEPLOYMENT, Some(oidc_ns), OIDC_NAME)
        .await
    {
        Ok(Some(provider)) => {
            let (wanted, ready) = spire::replicas(&provider);
            if wanted > 0 && ready == wanted {
                report.ok(
                    SETUP,
                    "OIDC discovery provider",
                    format!("Deployment {oidc_ns}/{OIDC_NAME} ready"),
                );
            } else {
                report.fail(SETUP, "OIDC discovery provider", format!("Deployment {oidc_ns}/{OIDC_NAME} not ready ({ready}/{wanted})"), &format!("look at its pods in {oidc_ns}; the pod needs a SPIFFE ID, so {oidc_ns} must not be in ignoreNamespaces of spire-controller-manager"));
            }
        }
        Ok(None) => report.fail(
            SETUP,
            "OIDC discovery provider",
            format!("Deployment {oidc_ns}/{OIDC_NAME} not found"),
            install_oidc,
        ),
        Err(error) => report.fail(
            SETUP,
            "OIDC discovery provider",
            error.to_string(),
            "check the rights on deployments",
        ),
    }
    let keys_path = format!("/api/v1/namespaces/{oidc_ns}/services/http:{OIDC_NAME}:80/proxy/keys");
    match cluster.raw("GET", &keys_path, &[], None).await {
        Ok((200, body)) => {
            let keys = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|body| body["keys"].as_array().map(Vec::len))
                .unwrap_or(0);
            if keys > 0 {
                report.ok(
                    SETUP,
                    "OIDC provider keys",
                    format!("{keys} signing key(s) at /keys"),
                );
            } else {
                report.fail(SETUP, "OIDC provider keys", "the provider serves no keys", "the provider has no SPIFFE identity yet: check its ClusterSPIFFEID and the SPIRE agents");
            }
        }
        Ok((status, _)) => report.fail(
            SETUP,
            "OIDC provider keys",
            format!("/keys answers HTTP {status}"),
            install_oidc,
        ),
        Err(error) => report.fail(SETUP, "OIDC provider keys", error.to_string(), install_oidc),
    }
    match cluster
        .get(&kinds::CLUSTER_SPIFFE_ID, None, OIDC_NAME)
        .await
    {
        Ok(Some(_)) => report.ok(
            SETUP,
            "OIDC provider ClusterSPIFFEID",
            format!("{OIDC_NAME} exists"),
        ),
        Ok(None) => report.fail(
            SETUP,
            "OIDC provider ClusterSPIFFEID",
            format!("{OIDC_NAME} not found"),
            install_oidc,
        ),
        Err(error) => report.fail(
            SETUP,
            "OIDC provider ClusterSPIFFEID",
            error.to_string(),
            "check the rights on clusterspiffeids",
        ),
    }

    let agents = match agent::agents(cluster).await {
        Ok(agents) => agents,
        Err(error) => {
            report.fail(SETUP, "agents", error, "check the rights on namespaces");
            Vec::new()
        }
    };
    match cluster
        .get(&kinds::CLUSTER_SPIFFE_ID, None, CLUSTER_SPIFFE_ID)
        .await
    {
        Ok(Some(_)) => report.ok(
            SETUP,
            "agent ClusterSPIFFEID",
            format!("{CLUSTER_SPIFFE_ID} exists"),
        ),
        Ok(None) if agents.is_empty() => report.warn(
            SETUP,
            "agent ClusterSPIFFEID",
            format!("{CLUSTER_SPIFFE_ID} not found; no agent exists yet"),
            "`grauwolf-k8-ops agent add <name>` creates it",
        ),
        Ok(None) => report.fail(
            SETUP,
            "agent ClusterSPIFFEID",
            format!("{CLUSTER_SPIFFE_ID} not found, but agents exist"),
            "run `grauwolf-k8-ops agent add <name>` for an agent again",
        ),
        Err(error) => report.fail(
            SETUP,
            "agent ClusterSPIFFEID",
            error.to_string(),
            "check the rights on clusterspiffeids",
        ),
    }

    identity_line(cluster, report, &agents).await;

    // OpenBao side: needs a token.
    let token_hint = "set OPENBAO_TOKEN or pass --token-file";
    let mut bao_ready = services.has_token;
    let jwt = settings.jwt_mount.as_str();
    let kv = settings.kv_mount.as_str();
    if bao_ready {
        let expected = format!("http://{}/keys", service_host(oidc_ns));
        match services.bao.auth_methods().await {
            Ok(methods) if methods.get(jwt).map(String::as_str) == Some("jwt") => {
                match services.bao.jwt_config(jwt).await {
                    Ok(Some(config)) if config["jwks_url"].as_str() == Some(expected.as_str()) => {
                        report.ok(
                            SETUP,
                            "JWT auth method",
                            format!("auth/{jwt}/ trusts {expected}"),
                        )
                    }
                    Ok(config) => {
                        let current = config
                            .as_ref()
                            .and_then(|config| config["jwks_url"].as_str())
                            .unwrap_or("none")
                            .to_owned();
                        report.fail(
                            SETUP,
                            "JWT auth method",
                            format!("auth/{jwt}/ has jwks_url {current}, expected {expected}"),
                            run_setup,
                        );
                    }
                    Err(error) => {
                        report.fail(SETUP, "JWT auth method", error.to_string(), token_hint)
                    }
                }
            }
            Ok(_) => report.fail(
                SETUP,
                "JWT auth method",
                format!("auth/{jwt}/ is not enabled"),
                run_setup,
            ),
            Err(error) => {
                report.fail(SETUP, "JWT auth method", error.to_string(), token_hint);
                bao_ready = false;
            }
        }
        match services.bao.mounts().await {
            Ok(mounts) => match mounts.get(kv) {
                Some((kind, version)) if kind == "kv" && version.as_deref() == Some("2") => {
                    report.ok(SETUP, "KV v2 mount", format!("{kv}/ is a KV v2 mount"))
                }
                Some((kind, _)) => report.fail(
                    SETUP,
                    "KV v2 mount",
                    format!("{kv}/ exists as '{kind}', not as KV v2"),
                    "use another mount with --kv-mount",
                ),
                None => report.fail(
                    SETUP,
                    "KV v2 mount",
                    format!("{kv}/ is not mounted"),
                    run_setup,
                ),
            },
            Err(error) => report.fail(SETUP, "KV v2 mount", error.to_string(), token_hint),
        }
    } else {
        report.warn(
            SETUP,
            "JWT auth method",
            "no token, not checked",
            token_hint,
        );
        report.warn(SETUP, "KV v2 mount", "no token, not checked", token_hint);
    }

    // Trust domain, everywhere it appears.
    let mut domains: Vec<(String, String)> = Vec::new();
    let mut unread = Vec::new();
    for source in spire::trust_domain_sources(cluster, settings).await {
        match source.domain {
            Ok(domain) => domains.push((source.label.to_owned(), domain)),
            Err(error) => unread.push(format!("{}: {error}", source.label)),
        }
    }

    // Per agent.
    let trust_domain = domains
        .first()
        .filter(|(label, _)| label == "SPIRE server")
        .map(|(_, domain)| domain.clone());
    let mut policies_seen = false;
    for (name, namespace) in &agents {
        let fix = format!("run `grauwolf-k8-ops agent add {name}`");
        let check = |what: &str| format!("agent {name}: {what}");
        match cluster.get(&kinds::NAMESPACE, None, namespace).await {
            Ok(Some(object))
                if object["metadata"]["labels"]["pod-security.kubernetes.io/enforce"]
                    == "restricted" =>
            {
                report.ok(
                    SETUP,
                    &check("namespace"),
                    format!("{namespace}, restricted Pod Security Standard enforced"),
                )
            }
            Ok(Some(_)) => report.fail(
                SETUP,
                &check("namespace"),
                format!("{namespace} does not enforce the restricted Pod Security Standard"),
                &fix,
            ),
            Ok(None) => report.fail(
                SETUP,
                &check("namespace"),
                format!("{namespace} not found"),
                &fix,
            ),
            Err(error) => report.fail(
                SETUP,
                &check("namespace"),
                error.to_string(),
                "check the rights on namespaces",
            ),
        }
        match cluster
            .get(&kinds::SERVICE_ACCOUNT, Some(namespace), name)
            .await
        {
            Ok(Some(_)) => report.ok(
                SETUP,
                &check("service account"),
                format!("{namespace}/{name} exists"),
            ),
            Ok(None) => report.fail(
                SETUP,
                &check("service account"),
                format!("{namespace}/{name} not found"),
                &fix,
            ),
            Err(error) => report.fail(
                SETUP,
                &check("service account"),
                error.to_string(),
                "check the rights on serviceaccounts",
            ),
        }
        match cluster
            .get(&kinds::CONFIG_MAP, Some(namespace), CONFIG_MAP)
            .await
        {
            Ok(Some(_)) => report.ok(
                SETUP,
                &check("ConfigMap"),
                format!("{namespace}/{CONFIG_MAP} exists"),
            ),
            Ok(None) => report.fail(
                SETUP,
                &check("ConfigMap"),
                format!("{namespace}/{CONFIG_MAP} not found"),
                &fix,
            ),
            Err(error) => report.fail(
                SETUP,
                &check("ConfigMap"),
                error.to_string(),
                "check the rights on configmaps",
            ),
        }
        // A deployed agent (ADR-0121); an agent that is not deployed has no line.
        match cluster.get(&kinds::DEPLOYMENT, Some(namespace), name).await {
            Ok(Some(deployment)) => {
                let (ready, image, why) = deploy::readiness(&deployment);
                if let Some(tracing) = deploy::tracing_of(&deployment) {
                    tracing_line(cluster, report, &check("tracing"), &tracing.endpoint).await;
                }
                if ready {
                    report.ok(
                        SETUP,
                        &check("deployment"),
                        format!("{namespace}/{name} ready, image {image}"),
                    );
                } else {
                    report.fail(
                        SETUP,
                        &check("deployment"),
                        format!("{namespace}/{name} not ready ({why}), image {image}"),
                        &format!(
                            "run `grauwolf-k8-ops agent status {name}` and `agent logs {name}`"
                        ),
                    );
                }
            }
            Ok(None) => {}
            Err(error) => report.fail(
                SETUP,
                &check("deployment"),
                error.to_string(),
                "check the rights on deployments",
            ),
        }
        policies_seen |= agent_callers(
            cluster,
            settings,
            report,
            (name, namespace),
            &agents,
            trust_domain.as_deref(),
        )
        .await;
        if !bao_ready {
            report.warn(
                SETUP,
                &check("OpenBao role and policy"),
                "no token or OpenBao not readable, not checked",
                token_hint,
            );
            continue;
        }
        let role = agent::role_name(name);
        match services.bao.read_jwt_role(jwt, &role).await {
            Ok(Some(data)) => {
                let subject = data["bound_subject"].as_str().unwrap_or("").to_owned();
                if let Some(domain) = spire::domain_of_id(&subject) {
                    domains.push((format!("OpenBao role {role}"), domain.to_owned()));
                }
                match &trust_domain {
                    Some(domain) => {
                        let expected = jwt_role(&settings.audience, &spire::spiffe_id(domain, namespace, name), &role);
                        let matches = ["role_type", "user_claim", "bound_audiences", "bound_subject", "token_policies"].iter().all(|key| data[*key] == expected[*key]);
                        if matches {
                            report.ok(SETUP, &check("OpenBao role"), format!("{role} is bound to exactly {subject}"));
                        } else {
                            report.fail(SETUP, &check("OpenBao role"), format!("{role} is bound to '{subject}', expected '{}'", expected["bound_subject"].as_str().unwrap_or("")), &fix);
                        }
                    }
                    None => report.warn(SETUP, &check("OpenBao role"), format!("{role} exists (bound to {subject}); the trust domain is unknown, not compared"), "fix the SPIRE server configuration first"),
                }
            }
            Ok(None) => report.fail(
                SETUP,
                &check("OpenBao role"),
                format!("{role} not found"),
                &fix,
            ),
            Err(error) => report.fail(SETUP, &check("OpenBao role"), error.to_string(), token_hint),
        }
        let expected = kv_read_policy(kv, &agent::kv_prefix(name));
        match services.bao.read_policy(&role).await {
            Ok(Some(policy)) if policy.trim() == expected.trim() => report.ok(
                SETUP,
                &check("OpenBao policy"),
                format!("{role} reads only {kv}/data/{}/*", agent::kv_prefix(name)),
            ),
            Ok(Some(_)) => report.fail(
                SETUP,
                &check("OpenBao policy"),
                format!(
                    "{role} differs from the policy limited to {kv}/data/{}/*",
                    agent::kv_prefix(name)
                ),
                &fix,
            ),
            Ok(None) => report.fail(
                SETUP,
                &check("OpenBao policy"),
                format!("{role} not found"),
                &fix,
            ),
            Err(error) => report.fail(
                SETUP,
                &check("OpenBao policy"),
                error.to_string(),
                token_hint,
            ),
        }
    }

    if policies_seen {
        // The API cannot tell: a cluster whose network plugin ignores NetworkPolicies accepts them
        // without a word.
        report.warn(
            SETUP,
            "NetworkPolicy enforcement",
            format!("the API does not say whether the cluster enforces NetworkPolicies, so it is not known that {NETWORK_POLICY} keeps anybody out of the mTLS ports"),
            "check it from outside: run the live check, or start a pod in a namespace that is not allowed and try an mTLS port (`nc -zv -w 3 <agent>.<namespace>.svc 8443` must time out)",
        );
    }

    // The trust domain: one answer for all places.
    let distinct: BTreeMap<&str, Vec<&str>> =
        domains
            .iter()
            .fold(BTreeMap::new(), |mut map, (label, domain)| {
                map.entry(domain.as_str()).or_default().push(label.as_str());
                map
            });
    match (distinct.len(), trust_domain.is_some()) {
        (_, false) => report.fail(
            SETUP,
            "trust domain",
            unread
                .first()
                .cloned()
                .unwrap_or_else(|| "the SPIRE server's trust domain cannot be read".to_owned()),
            "fix the SPIRE server configuration (ConfigMap spire-server, key server.conf)",
        ),
        (1, true) => {
            let domain = distinct.keys().next().copied().unwrap_or("");
            let places = domains.len();
            if unread.is_empty() {
                report.ok(
                    SETUP,
                    "trust domain",
                    format!("{domain} in all {places} place(s) read"),
                );
            } else {
                report.warn(
                    SETUP,
                    "trust domain",
                    format!(
                        "{domain} in {places} place(s); not read: {}",
                        unread.join("; ")
                    ),
                    "install the missing piece, or ignore if it is not used",
                );
            }
        }
        _ => {
            let listing = distinct
                .iter()
                .map(|(domain, labels)| format!("{domain} ({})", labels.join(", ")))
                .collect::<Vec<_>>()
                .join(" vs ");
            report.fail(SETUP, "trust domain", format!("not the same everywhere: {listing}"), "an interrupted change is finished by running `grauwolf-k8-ops trust-domain set <the domain you want>` again (plan first, then --apply); otherwise correct the others by hand");
        }
    }

    // The shared role of the example (ADR-0120): every agent could read every agent's secrets.
    if bao_ready && let Ok(Some(_)) = services.bao.read_jwt_role(jwt, "grauwolf-agents").await {
        report.warn(SETUP, "legacy shared role", format!("auth/{jwt}/role/grauwolf-agents (from the example) lets its agents read all of {kv}/data/agents/*"), "per-agent roles replace it: remove the role and policy `grauwolf-agents` once no agent uses them");
    }
}

/// The callers of one agent (ADR-0109 Q7-Q9): the ConfigMap, the NetworkPolicy and, for a deployed
/// agent, its spec's calls to other agents. `true` if the agent has a NetworkPolicy.
async fn agent_callers(
    cluster: &Cluster,
    settings: &Settings,
    report: &mut Report,
    (name, namespace): (&str, &str),
    agents: &[(String, String)],
    trust_domain: Option<&str>,
) -> bool {
    let check = |what: &str| format!("agent {name}: {what}");
    let deployed = matches!(
        cluster.get(&kinds::DEPLOYMENT, Some(namespace), name).await,
        Ok(Some(_))
    );
    let map = check("callers");
    let callers = match callers::read_callers(cluster, namespace).await {
        Err(error) => {
            report.fail(SETUP, &map, error, "correct the file by hand, or delete the ConfigMap and run `grauwolf-k8-ops agent allow` again");
            return false;
        }
        Ok(None) if !deployed => return false,
        Ok(None) => {
            report.warn(
                SETUP,
                &map,
                format!("{namespace}/{CALLERS_MAP} not found: no caller is mapped, so every caller by SPIFFE ID gets 403"),
                &format!("run `grauwolf-k8-ops agent deploy {name} --image <image>` (it makes an empty one) or `agent allow {name} --from <agent>`"),
            );
            callers::Callers::new()
        }
        Ok(Some(callers)) => {
            report.ok(
                SETUP,
                &map,
                format!(
                    "{namespace}/{CALLERS_MAP} parses, {} caller(s)",
                    callers.len()
                ),
            );
            callers
        }
    };
    for id in callers.keys() {
        if let Some(dangling) = dangling(id, agents, trust_domain) {
            report.warn(
                SETUP,
                &check("callers"),
                format!("{id} {dangling}"),
                &format!("run `grauwolf-k8-ops agent deny {name} --from <agent>`, or allow the agent again"),
            );
        }
    }

    let (http, mtls) = callers::ports_of(cluster, namespace, name).await;
    let expected = callers::policy_object(name, namespace, http, mtls, &callers);
    let state = callers::policy_state(cluster, namespace, &expected).await;
    let policy = check("NetworkPolicy");
    let exists = !matches!(state, Ok(PolicyState::Missing));
    match state {
        Ok(PolicyState::Matches) => report.ok(
            SETUP,
            &policy,
            format!(
                "{namespace}/{NETWORK_POLICY} matches the callers (port {mtls} from the namespaces of {} caller(s), port {http} from anywhere)",
                callers.len()
            ),
        ),
        Ok(state) => report.warn(
            SETUP,
            &policy,
            format!(
                "{namespace}/{NETWORK_POLICY} {}: it does not fit the callers, so the mTLS port {mtls} is not limited to them",
                if state == PolicyState::Missing { "not found" } else { "differs" }
            ),
            &format!("run `grauwolf-k8-ops agent deploy {name} --image <image>`, or `agent allow`/`deny {name} --from <agent>` again"),
        ),
        Err(error) => report.warn(SETUP, &policy, error, "check the rights on networkpolicies"),
    }

    egress_line(cluster, report, name, namespace).await;
    if deployed {
        caller_spec_line(cluster, settings, report, (name, namespace), agents).await;
    }
    exists
}

/// The egress policy of an agent (`agent deploy --egress`), when it has one: which destinations
/// are only restricted by port.
async fn egress_line(cluster: &Cluster, report: &mut Report, name: &str, namespace: &str) {
    let check = format!("agent {name}: egress policy");
    match egress::read(cluster, namespace).await {
        Ok(None) => {}
        Ok(Some(policy)) => {
            let (destinations, port_only, strict) = egress::summary_of(&policy);
            let policy_name = egress::EGRESS_POLICY;
            if port_only.is_empty() {
                report.ok(
                    SETUP,
                    &check,
                    format!(
                        "{namespace}/{policy_name}: {destinations} destinations{}",
                        if strict {
                            ", external names left out (strict)"
                        } else {
                            ""
                        }
                    ),
                );
            } else {
                report.warn(
                    SETUP,
                    &check,
                    format!(
                        "{namespace}/{policy_name}: {destinations} destinations, {} only port-restricted: {}",
                        port_only.len(),
                        port_only.join(", ")
                    ),
                    "NetworkPolicies cannot match names: `agent deploy --egress-strict` leaves such hosts out, or use a policy engine that matches names",
                );
            }
        }
        Err(error) => report.warn(SETUP, &check, error, "check the rights on networkpolicies"),
    }
}

/// Why an entry of the callers does not name an agent of the cluster, if it does not.
fn dangling(id: &str, agents: &[(String, String)], trust_domain: Option<&str>) -> Option<String> {
    let path = id.strip_prefix("spiffe://")?;
    let (domain, path) = path.split_once('/')?;
    let mut parts = path.split('/');
    let (Some("ns"), Some(namespace), Some("sa"), Some(account), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    if trust_domain.is_some_and(|current| current != domain) {
        return Some(format!(
            "is in trust domain {domain}, not {}",
            trust_domain.unwrap_or("")
        ));
    }
    (!agents
        .iter()
        .any(|(agent, ns)| agent == account && ns == namespace))
    .then(|| "names an agent that no longer exists (a dangling entry)".to_owned())
}

/// The calls of the deployed agent's spec to other agents of the cluster.
async fn caller_spec_line(
    cluster: &Cluster,
    settings: &Settings,
    report: &mut Report,
    (name, namespace): (&str, &str),
    agents: &[(String, String)],
) {
    let check = format!("agent {name}: caller spec");
    let map = cluster
        .get(&kinds::CONFIG_MAP, Some(namespace), AGENT_MAP)
        .await
        .ok()
        .flatten();
    let Some(map) = map else {
        report.ok(
            SETUP,
            &check,
            "not checked: the agent was deployed from an image without --spec, so its calls are not known here",
        );
        return;
    };
    let specs = caller_spec::spec_documents(
        map["data"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(_, text)| text.as_str()),
    );
    let findings = caller_spec::check(cluster, settings, name, namespace, agents, &specs).await;
    for problem in &findings.problems {
        report.warn(SETUP, &check, &problem.message, &problem.hint);
    }
    if findings.problems.is_empty() {
        report.ok(
            SETUP,
            &check,
            if findings.targets.is_empty() {
                "calls no other agent of the cluster".to_owned()
            } else {
                format!(
                    "calls {}: auth.spiffe, mTLS port and permission fit",
                    findings.targets.join(", ")
                )
            },
        );
    }
}

/// The line of an agent that exports traces: the Service of an endpoint in the cluster
/// (`http://<service>.<namespace>.svc…`) must exist. The collector itself is not contacted.
async fn tracing_line(cluster: &Cluster, report: &mut Report, name: &str, endpoint: &str) {
    let Some((service, namespace)) = deploy::collector_service(endpoint) else {
        report.ok(
            SETUP,
            name,
            format!("exports to {endpoint} (not a service of the cluster, not checked)"),
        );
        return;
    };
    match cluster
        .get(&kinds::SERVICE, Some(&namespace), &service)
        .await
    {
        Ok(Some(_)) => report.ok(
            SETUP,
            name,
            format!("exports to {endpoint}, Service {namespace}/{service} exists"),
        ),
        Ok(None) => report.warn(
            SETUP,
            name,
            format!("exports to {endpoint}, but Service {namespace}/{service} not found: the traces are lost"),
            "install the collector or deploy again with `--otel-endpoint <url>`",
        ),
        Err(error) => report.warn(
            SETUP,
            name,
            format!("exports to {endpoint}; Service {namespace}/{service}: {error}"),
            "check the rights on services",
        ),
    }
}

/// The identity service (ADR-0111): optional, so its absence is fine unless an agent was deployed
/// with `--identity`.
async fn identity_line(cluster: &Cluster, report: &mut Report, agents: &[(String, String)]) {
    let check = "identity service";
    let mut users = Vec::new();
    for (name, namespace) in agents {
        if let Ok(Some(deployment)) = cluster.get(&kinds::DEPLOYMENT, Some(namespace), name).await
            && identity::url_of(&deployment).is_some()
        {
            users.push(name.clone());
        }
    }
    match identity::readiness(cluster).await {
        Ok(Some((true, image, _))) => report.ok(
            SETUP,
            check,
            format!(
                "installed in namespace {}, ready, image {image}",
                identity::NAMESPACE
            ),
        ),
        Ok(Some((false, image, why))) => report.fail(
            SETUP,
            check,
            format!(
                "installed in namespace {}, not ready ({why}), image {image}",
                identity::NAMESPACE
            ),
            "run `grauwolf-k8-ops identity status`",
        ),
        Ok(None) if users.is_empty() => report.ok(
            SETUP,
            check,
            "not installed (needed only for acting on behalf of people, ADR-0111)",
        ),
        Ok(None) => report.fail(
            SETUP,
            check,
            format!(
                "not installed, but deployed with --identity: {}",
                users.join(", ")
            ),
            "run `grauwolf-k8-ops identity install`",
        ),
        Err(error) => report.fail(SETUP, check, error, "check the rights on deployments"),
    }
}
