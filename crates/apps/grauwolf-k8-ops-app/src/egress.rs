//! `agent deploy --egress`: the NetworkPolicy `grauwolf-egress` derived from the agent's spec
//! (ADR-0110 stage 6). Plain NetworkPolicies match addresses, not names, so:
//! - DNS is always allowed (UDP and TCP 53 to the cluster's DNS pods);
//! - a destination in the cluster (`<service>.<namespace>.svc…`) is allowed by namespace and the
//!   port of the **pod** behind the Service (read from the Service);
//! - an IP literal is allowed as `/32` (`/128`) and port;
//! - an external host name cannot be matched: by default all addresses outside the cluster's
//!   ranges are allowed on the ports of those names (and `deploy` warns which names are only
//!   port-restricted); `--egress-strict` leaves them out, so they are blocked;
//! - an external host name that resolves to the LoadBalancer address (or an external IP) of a
//!   Service of the cluster (split DNS: the cluster's own ingress) is also allowed to that
//!   Service's pods, on the pod ports behind the destination port: the traffic never leaves the
//!   cluster, because kube-proxy sends it straight to the pods;
//! - destinations of the build phase are not needed at run time and not allowed.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use grauwolf_egress::{Destination, Phase};
use grauwolf_k8s::{Cluster, kinds};
use serde_json::{Value, json};

use crate::agent::{AGENT_LABEL, MANAGED_BY, MANAGER};

/// The NetworkPolicy of the egress rules (the callers' policy is `grauwolf-mtls-callers`).
pub const EGRESS_POLICY: &str = "grauwolf-egress";
/// How many destinations the policy was made from (status and doctor read it back).
pub const ANNOTATION_DESTINATIONS: &str = "grauwolf.io/egress-destinations";
/// The external names that are only port-restricted, comma separated.
pub const ANNOTATION_PORT_ONLY: &str = "grauwolf.io/egress-port-restricted";
/// `true` when external names were left out (`--egress-strict`).
pub const ANNOTATION_STRICT: &str = "grauwolf.io/egress-strict";
const NAMESPACE_NAME_LABEL: &str = "kubernetes.io/metadata.name";

/// The pod and service ranges of a k3s cluster.
pub const DEFAULT_CLUSTER_CIDRS: [&str; 2] = ["10.42.0.0/16", "10.43.0.0/16"];
/// Where k3s runs its DNS: pods labelled `k8s-app=kube-dns` in `kube-system`.
pub const DEFAULT_DNS_NAMESPACE: &str = "kube-system";
pub const DEFAULT_DNS_SELECTOR: &str = "k8s-app=kube-dns";

/// `--egress` and its settings.
#[derive(Debug, Clone)]
pub struct EgressOptions {
    /// `--egress-strict`: leave out the external names.
    pub strict: bool,
    /// `--cluster-cidr`: excluded from "everywhere" for the external names.
    pub cluster_cidrs: Vec<String>,
    /// `--egress-dns-namespace`.
    pub dns_namespace: String,
    /// `--egress-dns-selector` as `key=value`.
    pub dns_selector: String,
}

/// How host names are resolved to addresses (for the egress to the cluster's own ingress).
#[derive(Debug, Clone, Default)]
pub enum Resolver {
    /// The resolver of the machine `grauwolf-k8-ops` runs on.
    #[default]
    System,
    /// A fixed table (tests); unknown names do not resolve.
    Fixed(BTreeMap<String, Vec<IpAddr>>),
}

impl Resolver {
    /// The addresses of `host`; empty when it does not resolve.
    pub async fn resolve(&self, host: &str) -> Vec<IpAddr> {
        match self {
            Self::Fixed(table) => table.get(host).cloned().unwrap_or_default(),
            Self::System => {
                let target = format!("{host}:0");
                tokio::task::spawn_blocking(move || {
                    std::net::ToSocketAddrs::to_socket_addrs(&target)
                        .map(|found| found.map(|address| address.ip()).collect())
                        .unwrap_or_default()
                })
                .await
                .unwrap_or_default()
            }
        }
    }
}

/// A Service of the cluster whose LoadBalancer address or external IP a host name resolves to.
struct Balanced {
    namespace: String,
    service: String,
    /// The Service's selector: the pods behind it.
    selector: serde_json::Map<String, Value>,
    /// The pod ports behind the destination port.
    port: u16,
}

/// The Services (with a selector) that own one of the addresses `host` resolves to.
async fn balanced_by(
    cluster: &Cluster,
    resolver: &Resolver,
    host: &str,
    port: u16,
    warnings: &mut Vec<String>,
) -> Vec<Balanced> {
    let addresses = resolver.resolve(host).await;
    if addresses.is_empty() {
        return Vec::new();
    }
    let Ok(services) = cluster.list(&kinds::SERVICE, None, None).await else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for object in &services {
        let owned = object["status"]["loadBalancer"]["ingress"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["ip"].as_str())
            .chain(
                object["spec"]["externalIPs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            )
            .filter_map(|text| text.parse::<IpAddr>().ok());
        let (Some(namespace), Some(service)) = (
            object["metadata"]["namespace"].as_str(),
            object["metadata"]["name"].as_str(),
        ) else {
            continue;
        };
        let Some(selector) = object["spec"]["selector"]
            .as_object()
            .filter(|selector| !selector.is_empty())
        else {
            continue;
        };
        if !owned.into_iter().any(|ip| addresses.contains(&ip)) {
            continue;
        }
        let pod_port = match target_port(cluster, namespace, service, port).await {
            Ok(target) => target.unwrap_or(port),
            Err(reason) => {
                warnings.push(format!(
                    "egress: {reason}: port {port} is allowed on the pods of {namespace}/{service} as it is"
                ));
                port
            }
        };
        found.push(Balanced {
            namespace: namespace.to_owned(),
            service: service.to_owned(),
            selector: selector.clone(),
            port: pod_port,
        });
    }
    found
}

/// The policy and what to tell the user about it.
pub struct Plan {
    pub policy: Value,
    /// Destinations that have a rule (or are allowed by port), for the summary.
    pub destinations: usize,
    /// External names allowed by port only (empty in strict mode).
    pub port_only: Vec<String>,
    /// External names that are blocked (strict mode).
    pub blocked: Vec<String>,
    /// Things that deserve a warning (a Service that is not there, a name that is not known).
    pub warnings: Vec<String>,
    /// What was resolved that the user may want to know (service port to pod port).
    pub notes: Vec<String>,
}

/// Where a destination is, as far as the policy is concerned.
#[derive(Debug, PartialEq, Eq)]
enum Place {
    /// Inside the pod: no rule.
    Loopback,
    Ip(IpAddr),
    /// A Service: `service` is `None` for a pattern, `namespace` `None` for any namespace.
    Cluster {
        service: Option<String>,
        namespace: Option<String>,
    },
    External,
}

fn place_of(host: &str, own_namespace: &str) -> Place {
    if host == "localhost" {
        return Place::Loopback;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if ip.is_loopback() {
            Place::Loopback
        } else {
            Place::Ip(ip)
        };
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() == 1 {
        // A bare name is a Service of the agent's own namespace (the DNS search path).
        return Place::Cluster {
            service: Some(host.to_owned()),
            namespace: Some(own_namespace.to_owned()),
        };
    }
    let Some(at) = labels.iter().position(|label| *label == "svc") else {
        return Place::External;
    };
    let named = |index: usize| {
        labels
            .get(index)
            .filter(|label| **label != "*" && !label.is_empty())
            .map(|label| (*label).to_owned())
    };
    Place::Cluster {
        service: at.checked_sub(2).and_then(named),
        namespace: at.checked_sub(1).and_then(named),
    }
}

fn is_cluster_pattern(host: &str) -> bool {
    host.starts_with("*.")
}

fn port_entry(port: u16) -> Value {
    json!({"protocol": "TCP", "port": port})
}

fn ports(set: &BTreeSet<u16>) -> Vec<Value> {
    set.iter().copied().map(port_entry).collect()
}

/// The number of the container port named `name` in the pods Service `object` selects: looked up
/// in the pod templates of the Deployments of `namespace` whose labels match the Service's
/// selector (pods may not run yet when an agent is deployed). `None` if nothing names it.
async fn named_port(cluster: &Cluster, namespace: &str, object: &Value, name: &str) -> Option<u16> {
    let selector = object["spec"]["selector"].as_object()?;
    if selector.is_empty() {
        return None;
    }
    let deployments = cluster
        .list(&kinds::DEPLOYMENT, Some(namespace), None)
        .await
        .ok()?;
    deployments
        .iter()
        .map(|deployment| &deployment["spec"]["template"])
        .filter(|template| {
            selector
                .iter()
                .all(|(key, value)| &template["metadata"]["labels"][key] == value)
        })
        .flat_map(|template| {
            template["spec"]["containers"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .flat_map(|container| container["ports"].as_array().into_iter().flatten())
        .find(|port| port["name"].as_str() == Some(name))
        .and_then(|port| port["containerPort"].as_u64())
        .and_then(|port| u16::try_from(port).ok())
}

/// The pod port behind `port` of Service `namespace/service`: `Ok(Some(target))` if it differs,
/// `Ok(None)` if it is the same, `Err` with a reason when the Service cannot say.
async fn target_port(
    cluster: &Cluster,
    namespace: &str,
    service: &str,
    port: u16,
) -> Result<Option<u16>, String> {
    let object = cluster
        .get(&kinds::SERVICE, Some(namespace), service)
        .await
        .map_err(|error| format!("Service {namespace}/{service}: {error}"))?
        .ok_or_else(|| format!("Service {namespace}/{service} not found"))?;
    let entry = object["spec"]["ports"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["port"].as_u64() == Some(u64::from(port)))
        .ok_or_else(|| format!("Service {namespace}/{service} has no port {port}"))?;
    match &entry["targetPort"] {
        Value::Null => Ok(None),
        Value::Number(number) => Ok(number
            .as_u64()
            .and_then(|target| u16::try_from(target).ok())
            .filter(|target| *target != port)),
        Value::String(name) => match named_port(cluster, namespace, &object, name).await {
            Some(target) => Ok(Some(target).filter(|target| *target != port)),
            None => Err(format!(
                "Service {namespace}/{service} names its target port ({name:?}) and no Deployment it selects has a container port of that name"
            )),
        },
        _ => Err(format!(
            "Service {namespace}/{service} has an unknown target port ({})",
            entry["targetPort"]
        )),
    }
}

/// The pod ports behind every port of Service `namespace/service`; `Err` with a reason when the
/// Service cannot say (not there, no ports, a named target port).
async fn service_pod_ports(
    cluster: &Cluster,
    namespace: &str,
    service: &str,
) -> Result<BTreeSet<u16>, String> {
    let object = cluster
        .get(&kinds::SERVICE, Some(namespace), service)
        .await
        .map_err(|error| format!("Service {namespace}/{service}: {error}"))?
        .ok_or_else(|| format!("Service {namespace}/{service} not found"))?;
    let mut found = BTreeSet::new();
    for entry in object["spec"]["ports"].as_array().into_iter().flatten() {
        let port = entry["port"]
            .as_u64()
            .and_then(|port| u16::try_from(port).ok());
        let pod_port = match &entry["targetPort"] {
            Value::Null => port,
            Value::Number(number) => number.as_u64().and_then(|port| u16::try_from(port).ok()),
            Value::String(name) => match named_port(cluster, namespace, &object, name).await {
                Some(target) => Some(target),
                None => {
                    return Err(format!(
                        "Service {namespace}/{service} names its target port ({name:?}) and no Deployment it selects has a container port of that name"
                    ));
                }
            },
            _ => {
                return Err(format!(
                    "Service {namespace}/{service} has an unknown target port ({})",
                    entry["targetPort"]
                ));
            }
        };
        found.extend(pod_port);
    }
    if found.is_empty() {
        return Err(format!("Service {namespace}/{service} has no ports"));
    }
    Ok(found)
}

/// Adds `ports` (`None`: every port) to what the policy allows in `namespace`; every port wins.
fn allow_namespace(
    by_namespace: &mut BTreeMap<Option<String>, Option<BTreeSet<u16>>>,
    namespace: Option<String>,
    ports: Option<BTreeSet<u16>>,
) {
    match (by_namespace.entry(namespace), ports) {
        (Entry::Vacant(slot), ports) => {
            slot.insert(ports);
        }
        (Entry::Occupied(mut slot), Some(new)) => {
            if let Some(set) = slot.get_mut() {
                set.extend(new);
            }
        }
        (Entry::Occupied(mut slot), None) => {
            slot.insert(None);
        }
    }
}

/// The policy for the run-phase `destinations` (already resolved against what `deploy` knows).
pub async fn plan(
    cluster: &Cluster,
    resolver: &Resolver,
    agent: &str,
    namespace: &str,
    destinations: &[Destination],
    options: &EgressOptions,
) -> Plan {
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    let mut by_namespace: BTreeMap<Option<String>, Option<BTreeSet<u16>>> = BTreeMap::new();
    let mut by_ip: BTreeMap<IpAddr, BTreeSet<u16>> = BTreeMap::new();
    // The pods of a Service whose LoadBalancer address an external name resolves to, by namespace
    // and selector.
    let mut by_pods: BTreeMap<(String, String), (Value, BTreeSet<u16>)> = BTreeMap::new();
    let mut external_ports = BTreeSet::new();
    let mut port_only = BTreeSet::new();
    let mut blocked = BTreeSet::new();
    let mut counted = 0;
    for destination in destinations.iter().filter(|d| d.phase == Phase::Run) {
        let (Some(host), Some(port)) = (destination.host.as_deref(), destination.port) else {
            if let (Some(host), true) = (destination.host.as_deref(), destination.any_port)
                && let Place::Cluster {
                    service,
                    namespace: target_namespace,
                } = place_of(host, namespace)
            {
                counted += 1;
                // `None`: the namespace without a port restriction.
                let mut allowed = None;
                if let (Some(service), Some(target_namespace)) = (&service, &target_namespace) {
                    match service_pod_ports(cluster, target_namespace, service).await {
                        Ok(found) => {
                            notes.push(format!(
                                "egress: {service}.{target_namespace} is allowed on any port of the Service: the policy allows the pod ports {}",
                                found.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
                            ));
                            allowed = Some(found);
                        }
                        Err(reason) => warnings.push(format!(
                            "egress: {reason}: {host} is allowed on any port of namespace {target_namespace} until the Service is there (deploy again then)"
                        )),
                    }
                }
                // A pattern (`*.team.svc`) has no single Service: the whole namespace.
                allow_namespace(&mut by_namespace, target_namespace, allowed);
                continue;
            }
            let variable = destination
                .from_env
                .as_deref()
                .map(|name| format!("; set {name} with --env"))
                .unwrap_or_default();
            warnings.push(format!(
                "egress: {} ({}) is not known to deploy, so no rule allows it{variable}",
                destination.purpose, destination.pointer
            ));
            continue;
        };
        match place_of(host, namespace) {
            Place::Loopback => {}
            Place::Ip(ip) => {
                counted += 1;
                by_ip.entry(ip).or_default().insert(port);
            }
            Place::Cluster {
                service,
                namespace: target_namespace,
            } => {
                counted += 1;
                let mut allowed = port;
                if let (Some(service), Some(target_namespace)) = (&service, &target_namespace) {
                    match target_port(cluster, target_namespace, service, port).await {
                        Ok(Some(target)) => {
                            notes.push(format!(
                                "egress: {service}.{target_namespace} port {port} is port {target} of its pods: the policy allows {target}"
                            ));
                            allowed = target;
                        }
                        Ok(None) => {}
                        Err(reason) => warnings.push(format!(
                            "egress: {reason}: port {port} is allowed as it is (a Service port that differs from its pod port needs the pod port)"
                        )),
                    }
                } else if !is_cluster_pattern(host) {
                    warnings.push(format!(
                        "egress: {host} names no service: port {port} is allowed as it is"
                    ));
                }
                allow_namespace(
                    &mut by_namespace,
                    target_namespace,
                    Some(BTreeSet::from([allowed])),
                );
            }
            Place::External => {
                let balanced = balanced_by(cluster, resolver, host, port, &mut warnings).await;
                for found in &balanced {
                    notes.push(format!(
                        "egress: {host} resolves to the LoadBalancer of {}/{}; allowing its pods (port {})",
                        found.namespace, found.service, found.port
                    ));
                    let key = (
                        found.namespace.clone(),
                        Value::Object(found.selector.clone()).to_string(),
                    );
                    by_pods
                        .entry(key)
                        .or_insert_with(|| (Value::Object(found.selector.clone()), BTreeSet::new()))
                        .1
                        .insert(found.port);
                }
                if options.strict {
                    if balanced.is_empty() {
                        blocked.insert(format!("{host}:{port}"));
                    } else {
                        counted += 1;
                    }
                } else {
                    counted += 1;
                    external_ports.insert(port);
                    port_only.insert(format!("{host}:{port}"));
                }
            }
        }
    }

    let (dns_key, dns_value) = options
        .dns_selector
        .split_once('=')
        .unwrap_or((&options.dns_selector, ""));
    let mut rules = vec![json!({
        "to": [{
            "namespaceSelector": {"matchLabels": {NAMESPACE_NAME_LABEL: options.dns_namespace}},
            "podSelector": {"matchLabels": {dns_key: dns_value}}}],
        "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]})];
    for (target_namespace, set) in &by_namespace {
        let selector = match target_namespace {
            Some(name) => json!({"matchLabels": {NAMESPACE_NAME_LABEL: name}}),
            None => json!({}),
        };
        rules.push(match set {
            Some(set) => json!({"to": [{"namespaceSelector": selector}], "ports": ports(set)}),
            None => json!({"to": [{"namespaceSelector": selector}]}),
        });
    }
    for ((target_namespace, _), (selector, set)) in &by_pods {
        rules.push(json!({
            "to": [{
                "namespaceSelector": {"matchLabels": {NAMESPACE_NAME_LABEL: target_namespace}},
                "podSelector": {"matchLabels": selector}}],
            "ports": ports(set)}));
    }
    for (ip, set) in &by_ip {
        let cidr = match ip {
            IpAddr::V4(_) => format!("{ip}/32"),
            IpAddr::V6(_) => format!("{ip}/128"),
        };
        rules.push(json!({"to": [{"ipBlock": {"cidr": cidr}}], "ports": ports(set)}));
    }
    if !external_ports.is_empty() {
        rules.push(json!({
            "to": [{"ipBlock": {"cidr": "0.0.0.0/0", "except": options.cluster_cidrs}}],
            "ports": ports(&external_ports)}));
    }

    let port_only: Vec<String> = port_only.into_iter().collect();
    let blocked: Vec<String> = blocked.into_iter().collect();
    let mut annotations = serde_json::Map::new();
    annotations.insert(
        ANNOTATION_DESTINATIONS.to_owned(),
        json!(counted.to_string()),
    );
    annotations.insert(ANNOTATION_PORT_ONLY.to_owned(), json!(port_only.join(",")));
    if options.strict {
        annotations.insert(ANNOTATION_STRICT.to_owned(), json!("true"));
    }
    let policy = json!({"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
        "metadata": {"name": EGRESS_POLICY, "namespace": namespace, "annotations": annotations,
            "labels": {MANAGED_BY: MANAGER, AGENT_LABEL: agent}},
        "spec": {"podSelector": {"matchLabels": {AGENT_LABEL: agent}},
            "policyTypes": ["Egress"], "egress": rules}});
    Plan {
        policy,
        destinations: counted,
        port_only,
        blocked,
        warnings,
        notes,
    }
}

/// What a policy in the cluster says about itself: `(destinations, port-restricted names, strict)`.
pub fn summary_of(policy: &Value) -> (usize, Vec<String>, bool) {
    let annotation = |name: &str| policy["metadata"]["annotations"][name].as_str();
    let destinations = annotation(ANNOTATION_DESTINATIONS)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0);
    let port_only = annotation(ANNOTATION_PORT_ONLY)
        .unwrap_or("")
        .split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    (
        destinations,
        port_only,
        annotation(ANNOTATION_STRICT) == Some("true"),
    )
}

/// The egress policy of `namespace`, if it exists.
pub async fn read(cluster: &Cluster, namespace: &str) -> Result<Option<Value>, String> {
    cluster
        .get(&kinds::NETWORK_POLICY, Some(namespace), EGRESS_POLICY)
        .await
        .map_err(|error| format!("NetworkPolicy {namespace}/{EGRESS_POLICY}: {error}"))
}

/// The destinations of the agent tree at `spec`, resolved with what `deploy` knows: the `--env`
/// values, the OpenBao address of the agent's `config` (`grauwolf.toml`) and the endpoint of
/// `--otel`. A variable that OpenBao supplies is not known here; its entry stays as it is and
/// a warning says so.
pub fn derive(
    spec: &std::path::Path,
    config: &str,
    otel_endpoint: Option<&str>,
    env: &[(String, String)],
    mapped: &BTreeMap<String, String>,
) -> Result<(Vec<Destination>, Vec<String>), String> {
    let tree =
        grauwolf_agent_tree::AgentTree::read(spec, &grauwolf_agent_tree::ReadOptions::default())
            .map_err(|errors| errors.errors.join("\n"))?;
    let all = grauwolf_egress::derive_tree(
        &tree,
        &grauwolf_egress::Options {
            credentials_toml: Some(config),
            otel: otel_endpoint.is_some(),
        },
    );
    let lookup = |name: &str| {
        if name == grauwolf_egress::OTLP_ENDPOINT
            && let Some(endpoint) = otel_endpoint
        {
            return Some(endpoint.to_owned());
        }
        env.iter()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.clone())
    };
    let mut warnings = Vec::new();
    for destination in &all {
        if let Some(name) = destination.from_env.as_deref()
            && mapped.contains_key(name)
        {
            warnings.push(format!(
                "egress: {name} comes from OpenBao, so its address is not known to deploy ({})",
                destination.purpose
            ));
        }
    }
    let resolved = grauwolf_egress::resolve_all(&all, &lookup)
        .into_iter()
        .filter(|destination| destination.phase == Phase::Run)
        .collect();
    Ok((resolved, warnings))
}

/// Makes the policy in the cluster the one of `plan` and says what it does.
pub async fn sync(
    cluster: &Cluster,
    io: &mut crate::io::Io<'_>,
    namespace: &str,
    plan: &Plan,
) -> Result<(), String> {
    let fail = |error: grauwolf_k8s::K8sError| format!("NetworkPolicy {EGRESS_POLICY}: {error}");
    let mut outcome = cluster
        .ensure(&kinds::NETWORK_POLICY, Some(namespace), &plan.policy)
        .await
        .map_err(fail)?;
    // `ensure` trusts its hash annotation, which survives an edit by hand: the rules of a policy
    // matter too much to leave a deviation in place.
    if outcome == grauwolf_k8s::Outcome::Unchanged
        && read(cluster, namespace)
            .await?
            .is_some_and(|existing| existing["spec"] != plan.policy["spec"])
    {
        cluster
            .patch(
                &kinds::NETWORK_POLICY,
                Some(namespace),
                EGRESS_POLICY,
                &json!({"spec": plan.policy["spec"]}),
            )
            .await
            .map_err(fail)?;
        outcome = grauwolf_k8s::Outcome::Updated;
    }
    io.line(&format!(
        "NetworkPolicy {namespace}/{EGRESS_POLICY} ({} destinations, {} only port-restricted): {}",
        plan.destinations,
        plan.port_only.len(),
        crate::report::k8s(outcome)
    ));
    for note in &plan.notes {
        io.line(&format!("note: {note}"));
    }
    for warning in &plan.warnings {
        io.error(&format!("warning: {warning}"));
    }
    if !plan.port_only.is_empty() {
        io.error(&format!(
            "warning: egress: {} external destination(s) can only be restricted by port, not by name (NetworkPolicies match addresses): {}; `--egress-strict` leaves them out",
            plan.port_only.len(),
            plan.port_only.join(", ")
        ));
    }
    if !plan.blocked.is_empty() {
        io.line(&format!(
            "note: egress strict: not allowed, so blocked: {}",
            plan.blocked.join(", ")
        ));
    }
    Ok(())
}

/// Deletes the policy (a deploy without `--egress`, `undeploy`); says so if there was one.
pub async fn remove(
    cluster: &Cluster,
    io: &mut crate::io::Io<'_>,
    namespace: &str,
    why: &str,
) -> Result<(), String> {
    if cluster
        .delete(&kinds::NETWORK_POLICY, Some(namespace), EGRESS_POLICY)
        .await
        .map_err(|error| format!("NetworkPolicy {EGRESS_POLICY}: {error}"))?
    {
        io.line(&format!(
            "NetworkPolicy {namespace}/{EGRESS_POLICY}: deleted ({why})"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster(service: Option<&str>, namespace: Option<&str>) -> Place {
        Place::Cluster {
            service: service.map(str::to_owned),
            namespace: namespace.map(str::to_owned),
        }
    }

    #[test]
    fn names_inside_the_cluster_are_recognised_by_their_svc_label() {
        assert_eq!(
            place_of("peer.team.svc", "own"),
            cluster(Some("peer"), Some("team"))
        );
        assert_eq!(
            place_of("peer.team.svc.cluster.local", "own"),
            cluster(Some("peer"), Some("team"))
        );
        assert_eq!(place_of("*.team.svc", "own"), cluster(None, Some("team")));
        assert_eq!(place_of("*.svc", "own"), cluster(None, None));
        assert_eq!(
            place_of("ollama", "own"),
            cluster(Some("ollama"), Some("own"))
        );
    }

    #[test]
    fn literals_loopback_and_external_names_are_told_apart() {
        assert_eq!(place_of("localhost", "own"), Place::Loopback);
        assert_eq!(place_of("127.0.0.1", "own"), Place::Loopback);
        assert_eq!(
            place_of("10.1.2.3", "own"),
            Place::Ip("10.1.2.3".parse().unwrap())
        );
        assert_eq!(
            place_of("fd00::1", "own"),
            Place::Ip("fd00::1".parse().unwrap())
        );
        assert_eq!(place_of("api.example.com", "own"), Place::External);
        assert_eq!(place_of("*.example.com", "own"), Place::External);
    }
}
