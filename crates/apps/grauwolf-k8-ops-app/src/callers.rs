//! `agent allow | deny | callers`: which agents may call an agent over the mTLS port (ADR-0109
//! Q7-Q9). The callers are the ConfigMap `grauwolf-callers` (key `callers.json`, SPIFFE ID to user)
//! in the target's namespace, mounted into the pod as a directory so that the service reads it again
//! when it changes; the NetworkPolicy `grauwolf-mtls-callers` lets only the namespaces of the
//! callers reach the mTLS port.

use std::collections::{BTreeMap, BTreeSet};

use grauwolf_k8s::{Cluster, kinds};
use serde_json::{Value, json};

use crate::agent::{self, AGENT_LABEL, MANAGED_BY, MANAGER};
use crate::io::Io;
use crate::report;
use crate::services::Services;
use crate::spire;

/// The ConfigMap with the caller mapping.
pub const CALLERS_MAP: &str = "grauwolf-callers";
pub const CALLERS_KEY: &str = "callers.json";
/// Where the ConfigMap is mounted (a directory, never `subPath`: only a directory sees the
/// `..data` swap that makes the service read the file again).
pub const CALLERS_DIR: &str = "/etc/grauwolf/callers";
pub const NETWORK_POLICY: &str = "grauwolf-mtls-callers";
pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_MTLS_PORT: u16 = 8443;
const NAMESPACE_NAME_LABEL: &str = "kubernetes.io/metadata.name";

/// SPIFFE ID (or `…/*` prefix) to user.
pub type Callers = BTreeMap<String, String>;

pub fn callers_text(callers: &Callers) -> String {
    let mut text = serde_json::to_string_pretty(callers).unwrap_or_else(|_| "{}".to_owned());
    text.push('\n');
    text
}

pub fn callers_object(namespace: &str, callers: &Callers) -> Value {
    json!({"apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": CALLERS_MAP, "namespace": namespace, "labels": {MANAGED_BY: MANAGER}},
        "data": {CALLERS_KEY: callers_text(callers)}})
}

/// The mapping of the file's text: a JSON object of text to text.
pub fn parse_callers(text: &str) -> Result<Callers, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("is not valid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or("is not a JSON object (SPIFFE ID to user)")?;
    object
        .iter()
        .map(|(id, user)| {
            user.as_str()
                .map(|user| (id.clone(), user.to_owned()))
                .ok_or_else(|| format!("maps {id} to something that is not a user name"))
        })
        .collect()
}

/// The mapping in `namespace`; `None` if there is no ConfigMap, an error if it does not parse.
pub async fn read_callers(cluster: &Cluster, namespace: &str) -> Result<Option<Callers>, String> {
    let Some(object) = cluster
        .get(&kinds::CONFIG_MAP, Some(namespace), CALLERS_MAP)
        .await
        .map_err(|error| format!("ConfigMap {namespace}/{CALLERS_MAP}: {error}"))?
    else {
        return Ok(None);
    };
    let text = object["data"][CALLERS_KEY]
        .as_str()
        .ok_or_else(|| format!("ConfigMap {namespace}/{CALLERS_MAP} has no key {CALLERS_KEY}"))?;
    parse_callers(text)
        .map(Some)
        .map_err(|error| format!("ConfigMap {namespace}/{CALLERS_MAP}, {CALLERS_KEY} {error}"))
}

/// Writes the mapping; the outcome says whether anything changed.
async fn write_callers(
    cluster: &Cluster,
    namespace: &str,
    callers: &Callers,
) -> Result<grauwolf_k8s::Outcome, String> {
    cluster
        .ensure(
            &kinds::CONFIG_MAP,
            Some(namespace),
            &callers_object(namespace, callers),
        )
        .await
        .map_err(|error| format!("ConfigMap {namespace}/{CALLERS_MAP}: {error}"))
}

/// The namespace a SPIFFE ID (or `…/*` prefix) names: `spiffe://<domain>/ns/<namespace>/…`.
pub fn namespace_of_id(id: &str) -> Option<&str> {
    let path = id.strip_prefix("spiffe://")?.split_once('/')?.1;
    let mut parts = path.split('/');
    (parts.next()? == "ns")
        .then(|| parts.next())
        .flatten()
        .filter(|namespace| !namespace.is_empty() && *namespace != "*")
}

/// The ID is in the mapping, as the service reads it: exactly, or by a prefix ending in `/*`
/// (whole segments only).
pub fn is_allowed(callers: &Callers, id: &str) -> bool {
    callers.contains_key(id)
        || callers.keys().any(|key| {
            key.strip_suffix("/*").is_some_and(|prefix| {
                id.strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
        })
}

/// `(http port, mTLS port)` of the agent's pod: from the Deployment, else the defaults.
pub async fn ports_of(cluster: &Cluster, namespace: &str, agent: &str) -> (u16, u16) {
    let deployment = cluster
        .get(&kinds::DEPLOYMENT, Some(namespace), agent)
        .await
        .ok()
        .flatten();
    let find = |name: &str, default: u16| {
        deployment
            .as_ref()
            .and_then(|deployment| {
                deployment["spec"]["template"]["spec"]["containers"][0]["ports"]
                    .as_array()?
                    .iter()
                    .find(|port| port["name"] == name)?["containerPort"]
                    .as_u64()
            })
            .and_then(|port| u16::try_from(port).ok())
            .unwrap_or(default)
    };
    (find("http", DEFAULT_PORT), find("mtls", DEFAULT_MTLS_PORT))
}

/// The namespaces of the callers, sorted.
pub fn caller_namespaces(callers: &Callers) -> BTreeSet<String> {
    callers
        .keys()
        .filter_map(|id| namespace_of_id(id))
        .map(str::to_owned)
        .collect()
}

/// The HTTP port stays open from anywhere (ingress, probes, people with tokens); the mTLS port is
/// open to the namespaces of the callers only. Without callers there is no rule for it: an ingress
/// rule without `from` would admit everybody, no rule admits nobody.
pub fn policy_object(
    agent: &str,
    namespace: &str,
    http_port: u16,
    mtls_port: u16,
    callers: &Callers,
) -> Value {
    let mut ingress = vec![json!({"ports": [{"protocol": "TCP", "port": http_port}]})];
    let namespaces = caller_namespaces(callers);
    if !namespaces.is_empty() {
        let from: Vec<Value> = namespaces
            .iter()
            .map(|name| json!({"namespaceSelector": {"matchLabels": {NAMESPACE_NAME_LABEL: name}}}))
            .collect();
        ingress.push(json!({"from": from, "ports": [{"protocol": "TCP", "port": mtls_port}]}));
    }
    json!({"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
        "metadata": {"name": NETWORK_POLICY, "namespace": namespace,
            "labels": {MANAGED_BY: MANAGER, AGENT_LABEL: agent}},
        "spec": {"podSelector": {"matchLabels": {AGENT_LABEL: agent}},
            "policyTypes": ["Ingress"], "ingress": ingress}})
}

/// Makes the NetworkPolicy of `agent` fit `callers` and the ports of its pod.
pub async fn sync_policy(
    cluster: &Cluster,
    io: &mut Io<'_>,
    agent: &str,
    namespace: &str,
    callers: &Callers,
) -> Result<(), String> {
    let ports = ports_of(cluster, namespace, agent).await;
    sync_policy_with(cluster, io, agent, namespace, callers, ports).await
}

/// Like [`sync_policy`] for ports that are not in the cluster yet (`agent deploy`).
pub async fn sync_policy_with(
    cluster: &Cluster,
    io: &mut Io<'_>,
    agent: &str,
    namespace: &str,
    callers: &Callers,
    (http, mtls): (u16, u16),
) -> Result<(), String> {
    let desired = policy_object(agent, namespace, http, mtls, callers);
    let mut outcome = cluster
        .ensure(&kinds::NETWORK_POLICY, Some(namespace), &desired)
        .await
        .map_err(|error| format!("NetworkPolicy {NETWORK_POLICY}: {error}"))?;
    // `ensure` trusts its hash annotation, which survives an edit by hand: the rules of a policy
    // matter too much to leave a deviation in place.
    if outcome == grauwolf_k8s::Outcome::Unchanged
        && policy_state(cluster, namespace, &desired).await? == PolicyState::Differs
    {
        cluster
            .patch(
                &kinds::NETWORK_POLICY,
                Some(namespace),
                NETWORK_POLICY,
                &json!({"spec": desired["spec"]}),
            )
            .await
            .map_err(|error| format!("NetworkPolicy {NETWORK_POLICY}: {error}"))?;
        outcome = grauwolf_k8s::Outcome::Updated;
    }
    io.line(&format!(
        "NetworkPolicy {namespace}/{NETWORK_POLICY} (port {mtls} from {}; port {http} from anywhere): {}",
        describe_namespaces(callers),
        report::k8s(outcome)
    ));
    Ok(())
}

fn describe_namespaces(callers: &Callers) -> String {
    let namespaces = caller_namespaces(callers);
    if namespaces.is_empty() {
        "nobody".to_owned()
    } else {
        format!(
            "namespace {}",
            namespaces.into_iter().collect::<Vec<_>>().join(", ")
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyState {
    Matches,
    Differs,
    Missing,
}

impl PolicyState {
    pub fn word(self) -> &'static str {
        match self {
            PolicyState::Matches => "matches",
            PolicyState::Differs => "differs",
            PolicyState::Missing => "missing",
        }
    }
}

/// The policy in the cluster against the one asked for (`expected`, from [`policy_object`]).
pub async fn policy_state(
    cluster: &Cluster,
    namespace: &str,
    expected: &Value,
) -> Result<PolicyState, String> {
    Ok(match policy_in_cluster(cluster, namespace).await? {
        None => PolicyState::Missing,
        Some(existing) if existing["spec"] == expected["spec"] => PolicyState::Matches,
        Some(_) => PolicyState::Differs,
    })
}

async fn policy_in_cluster(cluster: &Cluster, namespace: &str) -> Result<Option<Value>, String> {
    cluster
        .get(&kinds::NETWORK_POLICY, Some(namespace), NETWORK_POLICY)
        .await
        .map_err(|error| format!("NetworkPolicy {namespace}/{NETWORK_POLICY}: {error}"))
}

/// The namespaces the mTLS rule of the policy admits.
fn admitted_namespaces(policy: &Value) -> BTreeSet<String> {
    policy["spec"]["ingress"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|rule| rule["from"].as_array().into_iter().flatten())
        .filter_map(|peer| {
            peer["namespaceSelector"]["matchLabels"][NAMESPACE_NAME_LABEL]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

async fn existing_agent(cluster: &Cluster, name: &str) -> Result<String, String> {
    agent::validate_name(name)?;
    agent::find_namespace(cluster, name).await?.ok_or_else(|| {
        format!("agent {name} not found (create it with `grauwolf-k8-ops agent add {name}`)")
    })
}

fn validate_user(user: &str) -> Result<(), String> {
    if user.is_empty() || user.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Err(format!(
            "'{user}' is not a user name: it must not be empty or contain whitespace"
        ))
    } else {
        Ok(())
    }
}

/// Adds `id` as `user` to the callers of `agent` (namespace `namespace`) and makes its
/// NetworkPolicy fit – for workloads that are not agents, such as schedules.
pub async fn admit(
    cluster: &Cluster,
    io: &mut Io<'_>,
    agent: &str,
    namespace: &str,
    id: &str,
    user: &str,
) -> Result<(), String> {
    let mut callers = read_callers(cluster, namespace).await?.unwrap_or_default();
    let entry = match callers.insert(id.to_owned(), user.to_owned()) {
        None => "created",
        Some(old) if old != user => "updated",
        Some(_) => "unchanged",
    };
    io.line(&format!(
        "agent {agent} (namespace {namespace}) allows {id} as user {user}: {entry}"
    ));
    let outcome = write_callers(cluster, namespace, &callers).await?;
    io.line(&format!(
        "ConfigMap {namespace}/{CALLERS_MAP}: {}",
        report::k8s(outcome)
    ));
    sync_policy(cluster, io, agent, namespace, &callers).await
}

/// Removes `id` from the callers of `agent`, if it is there, and makes the NetworkPolicy fit.
pub async fn release(
    cluster: &Cluster,
    io: &mut Io<'_>,
    agent: &str,
    namespace: &str,
    id: &str,
) -> Result<(), String> {
    let Some(mut callers) = read_callers(cluster, namespace).await? else {
        return Ok(());
    };
    if callers.remove(id).is_none() {
        io.line(&format!(
            "agent {agent} does not list {id} as a caller: unchanged"
        ));
        return Ok(());
    }
    io.line(&format!(
        "agent {agent} (namespace {namespace}) no longer allows {id}: removed"
    ));
    let outcome = write_callers(cluster, namespace, &callers).await?;
    io.line(&format!(
        "ConfigMap {namespace}/{CALLERS_MAP}: {}",
        report::k8s(outcome)
    ));
    sync_policy(cluster, io, agent, namespace, &callers).await
}

pub async fn allow(
    services: &Services,
    io: &mut Io<'_>,
    target: &str,
    caller: &str,
    user: Option<String>,
) -> Result<(), String> {
    let cluster = &services.cluster;
    // Refuse before anything is written.
    let target_ns = existing_agent(cluster, target).await?;
    let caller_ns = existing_agent(cluster, caller).await?;
    let user = user.unwrap_or_else(|| caller.to_owned());
    validate_user(&user)?;
    let domain = spire::trust_domain(cluster, &services.settings).await?;
    let id = spire::spiffe_id(&domain, &caller_ns, caller);
    let mut callers = read_callers(cluster, &target_ns).await?.unwrap_or_default();

    let entry = match callers.insert(id.clone(), user.clone()) {
        None => "created",
        Some(old) if old != user => "updated",
        Some(_) => "unchanged",
    };
    io.line(&format!(
        "agent {target} (namespace {target_ns}) allows {id} as user {user}: {entry}"
    ));
    let outcome = write_callers(cluster, &target_ns, &callers).await?;
    io.line(&format!(
        "ConfigMap {target_ns}/{CALLERS_MAP}: {}",
        report::k8s(outcome)
    ));
    sync_policy(cluster, io, target, &target_ns, &callers).await
}

pub async fn deny(
    services: &Services,
    io: &mut Io<'_>,
    target: &str,
    caller: &str,
) -> Result<(), String> {
    let cluster = &services.cluster;
    let target_ns = existing_agent(cluster, target).await?;
    agent::validate_name(caller)?;
    let Some(mut callers) = read_callers(cluster, &target_ns).await? else {
        io.line(&format!(
            "agent {target} (namespace {target_ns}) has no callers: unchanged"
        ));
        return Ok(());
    };
    // The caller's ID if it exists; one that was removed leaves entries that end in its name.
    let ids: Vec<String> = match agent::find_namespace(cluster, caller).await? {
        Some(caller_ns) => {
            let domain = spire::trust_domain(cluster, &services.settings).await?;
            vec![spire::spiffe_id(&domain, &caller_ns, caller)]
        }
        None => callers
            .keys()
            .filter(|id| id.rsplit_once("/sa/").is_some_and(|(_, sa)| sa == caller))
            .cloned()
            .collect(),
    };
    let mut removed = false;
    for id in &ids {
        if callers.remove(id).is_some() {
            removed = true;
            io.line(&format!(
                "agent {target} (namespace {target_ns}) no longer allows {id}: removed"
            ));
        }
    }
    if !removed {
        io.line(&format!(
            "agent {target} (namespace {target_ns}) does not allow {caller}: unchanged"
        ));
    }
    let outcome = write_callers(cluster, &target_ns, &callers).await?;
    io.line(&format!(
        "ConfigMap {target_ns}/{CALLERS_MAP}: {}",
        report::k8s(outcome)
    ));
    sync_policy(cluster, io, target, &target_ns, &callers).await
}

pub async fn list(
    services: &Services,
    io: &mut Io<'_>,
    target: &str,
    json: bool,
) -> Result<(), String> {
    let cluster = &services.cluster;
    let target_ns = existing_agent(cluster, target).await?;
    let callers = read_callers(cluster, &target_ns).await?;
    let present = callers.is_some();
    let callers = callers.unwrap_or_default();
    let (http, mtls) = ports_of(cluster, &target_ns, target).await;
    let expected = policy_object(target, &target_ns, http, mtls, &callers);
    let state = policy_state(cluster, &target_ns, &expected).await?;
    let admitted = policy_in_cluster(cluster, &target_ns)
        .await?
        .map(|policy| admitted_namespaces(&policy))
        .unwrap_or_default();
    let known = agent::agents(cluster).await?;
    let rows: Vec<Value> = callers
        .iter()
        .map(|(id, user)| {
            let namespace = namespace_of_id(id);
            let name = id.rsplit_once("/sa/").map(|(_, sa)| sa).filter(|sa| {
                known
                    .iter()
                    .any(|(a, ns)| a == sa && Some(ns.as_str()) == namespace)
            });
            json!({"id": id, "user": user, "agent": name,
                "network_policy": namespace.is_some_and(|ns| admitted.contains(ns))})
        })
        .collect();
    if json {
        io.line(
            &json!({"agent": target, "namespace": target_ns, "callers": rows,
                "network_policy": state.word()})
            .to_string(),
        );
        return Ok(());
    }
    if !present {
        io.line(&format!(
            "agent {target} (namespace {target_ns}): no callers (ConfigMap {CALLERS_MAP} not made yet; `grauwolf-k8-ops agent allow {target} --from <agent>`)"
        ));
    } else {
        io.line(&format!(
            "agent {target} (namespace {target_ns}): {} caller(s)",
            rows.len()
        ));
    }
    for row in &rows {
        io.line(&format!(
            "  {}  user {}  {}  network policy: {}",
            row["id"].as_str().unwrap_or(""),
            row["user"].as_str().unwrap_or(""),
            match row["agent"].as_str() {
                Some(name) => format!("agent {name}"),
                None => "no such agent".to_owned(),
            },
            if row["network_policy"] == true {
                "admitted"
            } else {
                "not admitted"
            }
        ));
    }
    io.line(&format!(
        "NetworkPolicy {target_ns}/{NETWORK_POLICY}: {}",
        state.word()
    ));
    Ok(())
}

/// The other agents that allow `removed` (`agent remove` cleans them up): `(agent, namespace)`.
pub async fn allowing(
    cluster: &Cluster,
    removed: &str,
    removed_ns: &str,
) -> Result<Vec<(String, String)>, String> {
    let suffix = format!("/ns/{removed_ns}/sa/{removed}");
    let mut found = Vec::new();
    for (name, namespace) in agent::agents(cluster).await? {
        if name != removed
            && let Some(callers) = read_callers(cluster, &namespace).await?
            && callers.keys().any(|id| id.ends_with(&suffix))
        {
            found.push((name, namespace));
        }
    }
    Ok(found)
}

/// Drops the entries of the removed agent from the callers of `agent` and fits its policy.
pub async fn forget(
    cluster: &Cluster,
    io: &mut Io<'_>,
    removed: (&str, &str),
    (agent, namespace): (&str, &str),
) -> Result<(), String> {
    let suffix = format!("/ns/{}/sa/{}", removed.1, removed.0);
    let Some(mut callers) = read_callers(cluster, namespace).await? else {
        return Ok(());
    };
    callers.retain(|id, _| !id.ends_with(&suffix));
    write_callers(cluster, namespace, &callers).await?;
    io.line(&format!(
        "ConfigMap {namespace}/{CALLERS_MAP}: {} removed from the callers of {agent}",
        removed.0
    ));
    sync_policy(cluster, io, agent, namespace, &callers).await
}
