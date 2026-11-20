//! `agent add | list | remove | check` (and, in `deploy`, `deploy | status | logs | undeploy`): one agent, one namespace, one OpenBao role and policy
//! (ADR-0120 decision 2).

use std::collections::BTreeMap;

use grauwolf_k8s::{Cluster, kinds};
use grauwolf_openbao::{jwt_role, kv_read_policy};
use serde_json::{Value, json};

use crate::callers;
use crate::check;
use crate::cli::{AgentCommand, Settings};
use crate::deploy::{self, DeployOptions};
use crate::io::Io;
use crate::oidc::SPIFFE_ID_TEMPLATE;
use crate::report;
use crate::services::Services;
use crate::spire;

pub const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
pub const MANAGER: &str = "grauwolf-k8-ops";
pub const AGENT_LABEL: &str = "grauwolf.io/agent";
/// The one `ClusterSPIFFEID` for all agent namespaces.
pub const CLUSTER_SPIFFE_ID: &str = "grauwolf-agents";
/// The ConfigMap with the agent's `grauwolf.toml`.
pub const CONFIG_MAP: &str = "grauwolf-config";
pub const CONFIG_KEY: &str = "grauwolf.toml";

pub async fn run(
    services: &Services,
    io: &mut Io<'_>,
    command: AgentCommand,
) -> Result<i32, String> {
    match command {
        AgentCommand::Add {
            name,
            namespace,
            pull_secret,
        } => add(services, io, &name, namespace, pull_secret)
            .await
            .map(|()| 0),
        AgentCommand::List { json } => list(services, io, json).await.map(|()| 0),
        AgentCommand::Remove {
            name,
            purge_secrets,
            yes,
        } => remove(services, io, &name, purge_secrets, yes)
            .await
            .map(|()| 0),
        AgentCommand::Allow { target, from, user } => {
            callers::allow(services, io, &target, &from, user)
                .await
                .map(|()| 0)
        }
        AgentCommand::Deny { target, from } => callers::deny(services, io, &target, &from)
            .await
            .map(|()| 0),
        AgentCommand::Callers { target, json } => {
            callers::list(services, io, &target, json).await.map(|()| 0)
        }
        AgentCommand::Check {
            name,
            image,
            variables,
            timeout,
        } => check::run(services, io, &name, &image, variables, timeout).await,
        AgentCommand::Build {
            harness,
            project,
            namespace,
            kaniko_image,
            registry_secret,
            git_secret,
            timeout,
        } => {
            let options = crate::build::BuildOptions {
                harness,
                project,
                namespace,
                kaniko_image,
                registry_secret,
                git_secret,
                timeout,
            };
            crate::build::run(services, io, options).await
        }
        AgentCommand::Deploy {
            name,
            harness,
            image,
            spec,
            env,
            host,
            port,
            mtls_port,
            data_size,
            cpu,
            memory,
            pull_secret,
            otel,
            otel_endpoint,
            otel_content,
            events,
            identity,
            egress,
            egress_strict,
            cluster_cidr,
            egress_dns_namespace,
            egress_dns_selector,
            timeout,
        } => {
            let harness = harness
                .map(|file| crate::harness::read(&file).map(|read| (file, read)))
                .transpose()?;
            let (name, image, port, env) = match (name, harness) {
                (Some(name), _) => (name, image, port, env),
                (None, Some((file, harness))) => {
                    // Without an image, the default of the workspace's registry (ADR-0128 H7).
                    let image = match one_source("--image", "image", image, harness.image.clone())?
                    {
                        Some(image) => Some(image),
                        None => grauwolf_agent_tree::harness_registry(&file)?.map(|registry| {
                            harness.default_image(&registry, env!("CARGO_PKG_VERSION"))
                        }),
                    };
                    // The harness's values go through the same checks as `--env`.
                    let in_harness = (!harness.env.is_empty()).then(|| {
                        harness
                            .env
                            .iter()
                            .map(|(name, value)| format!("{name}={value}"))
                            .collect()
                    });
                    (
                        harness.name,
                        image,
                        one_source("--port", "port", port, harness.port)?,
                        one_source("--env", "env", (!env.is_empty()).then_some(env), in_harness)?
                            .unwrap_or_default(),
                    )
                }
                (None, None) => unreachable!("clap requires a name or --harness"),
            };
            let port = port.unwrap_or(deploy::DEFAULT_PORT);
            let image = image.ok_or(
                "no image: give --image, set `image` in the harness file or `[harness] registry` in grauwolf.toml",
            )?;
            let options = DeployOptions {
                name,
                image,
                spec,
                env,
                host,
                port,
                mtls_port,
                data_size,
                cpu,
                memory,
                pull_secret,
                otel: otel.then_some(deploy::OtelOptions {
                    endpoint: otel_endpoint,
                    content: otel_content,
                }),
                events,
                identity,
                egress: egress.then(|| crate::egress::EgressOptions {
                    strict: egress_strict,
                    cluster_cidrs: if cluster_cidr.is_empty() {
                        crate::egress::DEFAULT_CLUSTER_CIDRS
                            .iter()
                            .map(|cidr| (*cidr).to_owned())
                            .collect()
                    } else {
                        cluster_cidr
                    },
                    dns_namespace: egress_dns_namespace,
                    dns_selector: egress_dns_selector,
                }),
                timeout,
            };
            deploy::deploy(services, io, options).await
        }
        AgentCommand::Status { name, json } => {
            deploy::status(services, io, &name, json).await.map(|()| 0)
        }
        AgentCommand::Logs {
            name,
            follow,
            tail,
            events_only,
        } => deploy::logs(services, io, &name, follow, tail, events_only)
            .await
            .map(|()| 0),
        AgentCommand::Undeploy {
            name,
            purge_data,
            yes,
        } => deploy::undeploy(services, io, &name, purge_data, yes)
            .await
            .map(|()| 0),
    }
}

/// A DNS-1123 label.
pub fn validate_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{name}' is not a valid agent name: use lowercase letters, digits and '-' (at most 63, a DNS-1123 label)"
        ))
    }
}

pub fn role_name(agent: &str) -> String {
    format!("agent-{agent}")
}

pub fn kv_prefix(agent: &str) -> String {
    format!("agents/{agent}")
}

/// The namespace of an agent created by this tool.
pub async fn find_namespace(cluster: &Cluster, agent: &str) -> Result<Option<String>, String> {
    let selector = format!("{AGENT_LABEL}={agent},{MANAGED_BY}={MANAGER}");
    let found = cluster
        .list(&kinds::NAMESPACE, None, Some(&selector))
        .await
        .map_err(|error| error.to_string())?;
    Ok(found
        .first()
        .and_then(|namespace| namespace["metadata"]["name"].as_str())
        .map(str::to_owned))
}

/// The agents' namespaces: `(agent, namespace)`.
pub async fn agents(cluster: &Cluster) -> Result<Vec<(String, String)>, String> {
    let selector = format!("{AGENT_LABEL},{MANAGED_BY}={MANAGER}");
    let found = cluster
        .list(&kinds::NAMESPACE, None, Some(&selector))
        .await
        .map_err(|error| error.to_string())?;
    let mut agents: Vec<(String, String)> = found
        .iter()
        // A namespace that is being deleted belongs to an agent that was removed.
        .filter(|namespace| namespace["metadata"].get("deletionTimestamp").is_none())
        .filter_map(|namespace| {
            Some((
                namespace["metadata"]["labels"][AGENT_LABEL]
                    .as_str()?
                    .to_owned(),
                namespace["metadata"]["name"].as_str()?.to_owned(),
            ))
        })
        .collect();
    agents.sort();
    Ok(agents)
}

pub fn namespace_object(agent: &str, namespace: &str) -> Value {
    json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": namespace, "labels": {
        AGENT_LABEL: agent,
        MANAGED_BY: MANAGER,
        "pod-security.kubernetes.io/enforce": "restricted",
    }}})
}

pub fn service_account_object(agent: &str, namespace: &str, pull_secret: Option<&str>) -> Value {
    let mut account = json!({"apiVersion": "v1", "kind": "ServiceAccount",
        "metadata": {"name": agent, "namespace": namespace, "labels": {MANAGED_BY: MANAGER}}});
    if let Some(secret) = pull_secret {
        account["imagePullSecrets"] = json!([{ "name": secret }]);
    }
    account
}

pub fn cluster_spiffe_id_object() -> Value {
    json!({"apiVersion": "spire.spiffe.io/v1alpha1", "kind": "ClusterSPIFFEID",
        "metadata": {"name": CLUSTER_SPIFFE_ID, "labels": {MANAGED_BY: MANAGER}},
        "spec": {"spiffeIDTemplate": SPIFFE_ID_TEMPLATE,
            "namespaceSelector": {"matchExpressions": [
                {"key": AGENT_LABEL, "operator": "Exists"}]}}})
}

/// The agent's `grauwolf.toml` (ADR-0110): the OpenBao source for its role and the mapped
/// variables (`NAME -> openbao:kv/agents/<agent>/<key>#value`).
pub fn render_config(
    settings: &Settings,
    agent: &str,
    variables: &BTreeMap<String, String>,
) -> String {
    let mut text = format!(
        "[credentials.openbao]\naddress = \"{}\"\nauth = {{ spiffe = {{ audience = \"{}\", role = \"{}\", mount = \"{}\" }} }}\nrefresh = \"5m\"\n\n[credentials.variables]\n",
        settings.agent_address(),
        settings.audience,
        role_name(agent),
        settings.jwt_mount
    );
    for (name, source) in variables {
        text.push_str(&format!("{name} = \"{source}\"\n"));
    }
    text
}

/// The mapped variables of an agent's `grauwolf.toml`.
pub fn variables_of(config: &str) -> BTreeMap<String, String> {
    config
        .parse::<toml::Table>()
        .ok()
        .and_then(|table| {
            table
                .get("credentials")?
                .get("variables")?
                .as_table()
                .map(|variables| {
                    variables
                        .iter()
                        .filter_map(|(name, value)| {
                            Some((name.clone(), value.as_str()?.to_owned()))
                        })
                        .collect()
                })
        })
        .unwrap_or_default()
}

pub fn config_map_object(namespace: &str, config: &str) -> Value {
    json!({"apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": CONFIG_MAP, "namespace": namespace, "labels": {MANAGED_BY: MANAGER}},
        "data": {CONFIG_KEY: config}})
}

/// The text of the agent's `grauwolf.toml` as it is in the cluster.
pub async fn current_config(cluster: &Cluster, namespace: &str) -> Result<Option<String>, String> {
    Ok(cluster
        .get(&kinds::CONFIG_MAP, Some(namespace), CONFIG_MAP)
        .await
        .map_err(|error| error.to_string())?
        .and_then(|object| object["data"][CONFIG_KEY].as_str().map(str::to_owned)))
}

async fn add(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    namespace: Option<String>,
    pull_secret: Option<String>,
) -> Result<(), String> {
    validate_name(agent)?;
    let namespace = namespace.unwrap_or_else(|| agent.to_owned());
    validate_name(&namespace)?;
    let settings = &services.settings;
    let cluster = &services.cluster;
    let bao = services.admin()?;

    let trust_domain = spire::trust_domain(cluster, settings).await?;
    let subject = spire::spiffe_id(&trust_domain, &namespace, agent);
    io.line(&format!("trust domain {trust_domain}; identity {subject}"));

    // Refuse before changing anything.
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
        .get(&kinds::NAMESPACE, None, &namespace)
        .await
        .map_err(|error| error.to_string())?
        && existing["metadata"]["labels"][AGENT_LABEL].as_str() != Some(agent)
    {
        return Err(format!(
            "namespace {namespace} exists and does not belong to agent {agent} (label {AGENT_LABEL}); choose another with --namespace"
        ));
    }
    if let Some(other) = find_namespace(cluster, agent).await?
        && other != namespace
    {
        return Err(format!("agent {agent} already lives in namespace {other}"));
    }

    let outcome = cluster
        .ensure(
            &kinds::NAMESPACE,
            None,
            &namespace_object(agent, &namespace),
        )
        .await
        .map_err(|error| format!("namespace {namespace}: {error}"))?;
    io.line(&format!("namespace {namespace}: {}", report::k8s(outcome)));

    let pull_name = match &pull_secret {
        Some(reference) => Some(copy_pull_secret(cluster, io, reference, &namespace).await?),
        None => None,
    };

    let account = service_account_object(agent, &namespace, pull_name.as_deref());
    let outcome = cluster
        .ensure(&kinds::SERVICE_ACCOUNT, Some(&namespace), &account)
        .await
        .map_err(|error| format!("service account {agent}: {error}"))?;
    io.line(&format!(
        "service account {namespace}/{agent}: {}",
        report::k8s(outcome)
    ));

    let outcome = cluster
        .ensure(&kinds::CLUSTER_SPIFFE_ID, None, &cluster_spiffe_id_object())
        .await
        .map_err(|error| format!("ClusterSPIFFEID {CLUSTER_SPIFFE_ID}: {error}"))?;
    io.line(&format!(
        "ClusterSPIFFEID {CLUSTER_SPIFFE_ID}: {}",
        report::k8s(outcome)
    ));

    // Keep the variables that `secret set --variable` mapped.
    let variables = current_config(cluster, &namespace)
        .await?
        .map(|text| variables_of(&text))
        .unwrap_or_default();
    let config = config_map_object(&namespace, &render_config(settings, agent, &variables));
    let outcome = cluster
        .ensure(&kinds::CONFIG_MAP, Some(&namespace), &config)
        .await
        .map_err(|error| format!("ConfigMap {CONFIG_MAP}: {error}"))?;
    io.line(&format!(
        "ConfigMap {namespace}/{CONFIG_MAP}: {}",
        report::k8s(outcome)
    ));

    let role = role_name(agent);
    let policy = kv_read_policy(&settings.kv_mount, &kv_prefix(agent));
    let outcome = bao
        .ensure_policy(&role, &policy)
        .await
        .map_err(|error| format!("OpenBao policy {role}: {error}"))?;
    io.line(&format!(
        "OpenBao policy {role} (reads {}/data/{}/*): {}",
        settings.kv_mount,
        kv_prefix(agent),
        report::bao(outcome)
    ));
    let body = jwt_role(&settings.audience, &subject, &role);
    let outcome = bao
        .ensure_jwt_role(&settings.jwt_mount, &role, &body)
        .await
        .map_err(|error| format!("OpenBao role {role}: {error}"))?;
    io.line(&format!(
        "OpenBao role auth/{}/role/{role} (subject {subject}): {}",
        settings.jwt_mount,
        report::bao(outcome)
    ));
    Ok(())
}

/// Copies the image pull secret `namespace/name` into `target` under the same name.
pub(crate) async fn copy_pull_secret(
    cluster: &Cluster,
    io: &mut Io<'_>,
    reference: &str,
    target: &str,
) -> Result<String, String> {
    let (source_ns, name) = reference
        .split_once('/')
        .filter(|(namespace, name)| !namespace.is_empty() && !name.is_empty())
        .ok_or_else(|| format!("--pull-secret takes NAMESPACE/NAME, not '{reference}'"))?;
    let source = cluster
        .get(&kinds::SECRET, Some(source_ns), name)
        .await
        .map_err(|error| format!("pull secret {reference}: {error}"))?
        .ok_or_else(|| format!("pull secret {reference} not found"))?;
    // A copy that is already equal (e.g. cloned by Kyverno, which guards its labels) is left alone.
    let existing = cluster
        .get(&kinds::SECRET, Some(target), name)
        .await
        .map_err(|error| format!("pull secret {name}: {error}"))?;
    let outcome = match existing {
        Some(found) if found["type"] == source["type"] && found["data"] == source["data"] => {
            "unchanged"
        }
        _ => {
            let copy = json!({"apiVersion": "v1", "kind": "Secret",
                "metadata": {"name": name, "namespace": target, "labels": {MANAGED_BY: MANAGER}},
                "type": source["type"], "data": source["data"]});
            report::k8s(
                cluster
                    .ensure(&kinds::SECRET, Some(target), &copy)
                    .await
                    .map_err(|error| format!("pull secret {name}: {error}"))?,
            )
        }
    };
    io.line(&format!(
        "pull secret {target}/{name} (copied from {reference}): {outcome}"
    ));
    Ok(name.to_owned())
}

async fn list(services: &Services, io: &mut Io<'_>, json: bool) -> Result<(), String> {
    let found = agents(&services.cluster).await?;
    let trust_domain = spire::trust_domain(&services.cluster, &services.settings)
        .await
        .ok();
    let rows: Vec<Value> = found
        .iter()
        .map(|(agent, namespace)| {
            let id = trust_domain
                .as_deref()
                .map(|domain| spire::spiffe_id(domain, namespace, agent));
            json!({"agent": agent, "namespace": namespace, "spiffe_id": id})
        })
        .collect();
    if json {
        io.line(&Value::Array(rows).to_string());
    } else if rows.is_empty() {
        io.line("no agents (create one with `grauwolf-k8-ops agent add <name>`)");
    } else {
        for row in rows {
            io.line(&format!(
                "{}  namespace {}  {}",
                row["agent"].as_str().unwrap_or(""),
                row["namespace"].as_str().unwrap_or(""),
                row["spiffe_id"]
                    .as_str()
                    .unwrap_or("(trust domain unknown)")
            ));
        }
    }
    Ok(())
}

async fn remove(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    purge_secrets: bool,
    yes: bool,
) -> Result<(), String> {
    validate_name(agent)?;
    let settings = &services.settings;
    let bao = services.admin()?;
    let namespace = find_namespace(&services.cluster, agent).await?;
    let role = role_name(agent);
    let role_exists = bao
        .read_jwt_role(&settings.jwt_mount, &role)
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    let policy_exists = bao
        .read_policy(&role)
        .await
        .map_err(|error| error.to_string())?
        .is_some();
    let secrets = bao
        .kv_list(&settings.kv_mount, &kv_prefix(agent))
        .await
        .map_err(|error| error.to_string())?;
    // The agents that allow this one to call them keep an entry that would dangle.
    let allowing = match &namespace {
        Some(namespace) => callers::allowing(&services.cluster, agent, namespace).await?,
        None => Vec::new(),
    };
    if namespace.is_none() && !role_exists && !policy_exists && secrets.is_empty() {
        return Err(format!("agent {agent} not found"));
    }

    io.line(&format!("agent {agent} will be removed:"));
    if let Some(namespace) = &namespace {
        io.line(&format!("  namespace {namespace} with everything in it"));
        if deploy::is_deployed(&services.cluster, namespace, agent).await {
            io.line("    including the deployed agent (Deployment, Service, Ingress, data volume)");
        }
        for (other, other_ns) in &allowing {
            io.line(&format!(
                "  its entry in the callers of {other} (namespace {other_ns}) and the NetworkPolicy there"
            ));
        }
    }
    if role_exists {
        io.line(&format!(
            "  OpenBao role auth/{}/role/{role}",
            settings.jwt_mount
        ));
    }
    if policy_exists {
        io.line(&format!("  OpenBao policy {role}"));
    }
    if purge_secrets {
        io.line(&format!(
            "  {} secret(s) under {}/{}/ (all versions)",
            secrets.len(),
            settings.kv_mount,
            kv_prefix(agent)
        ));
    } else if !secrets.is_empty() {
        io.line(&format!(
            "  kept: {} secret(s) under {}/{}/ (use --purge-secrets to delete them)",
            secrets.len(),
            settings.kv_mount,
            kv_prefix(agent)
        ));
    }
    if !yes && !io.confirm() {
        return Err("cancelled, nothing was removed".to_owned());
    }

    if let Some(namespace) = &namespace {
        services
            .cluster
            .delete(&kinds::NAMESPACE, None, namespace)
            .await
            .map_err(|error| format!("namespace {namespace}: {error}"))?;
        io.line(&format!("namespace {namespace}: deleted"));
        for (other, other_ns) in &allowing {
            callers::forget(&services.cluster, io, (agent, namespace), (other, other_ns)).await?;
        }
    }
    if role_exists {
        bao.delete_jwt_role(&settings.jwt_mount, &role)
            .await
            .map_err(|error| format!("OpenBao role {role}: {error}"))?;
        io.line(&format!("OpenBao role {role}: deleted"));
    }
    if policy_exists {
        bao.delete_policy(&role)
            .await
            .map_err(|error| format!("OpenBao policy {role}: {error}"))?;
        io.line(&format!("OpenBao policy {role}: deleted"));
    }
    if purge_secrets {
        let removed = bao
            .kv_delete_tree(&settings.kv_mount, &kv_prefix(agent))
            .await
            .map_err(|error| format!("secrets of {agent}: {error}"))?;
        io.line(&format!("secrets of {agent}: {} deleted", removed.len()));
    }
    Ok(())
}

/// A setting of `agent deploy --harness` comes from the flag or from the harness file, never both
/// (ADR-0128 decision 6).
fn one_source<T>(
    flag: &str,
    field: &str,
    given: Option<T>,
    in_harness: Option<T>,
) -> Result<Option<T>, String> {
    match (given, in_harness) {
        (Some(_), Some(_)) => Err(format!(
            "{flag}: the harness file sets `{field}` already; change it there or leave the flag out"
        )),
        (given, in_harness) => Ok(given.or(in_harness)),
    }
}
