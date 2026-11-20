//! The check of a spec that calls other agents of the cluster (ADR-0109 Q8): an `agent_tools` or
//! `mcp_servers` entry whose URL is `<name>.<namespace>.svc` names an agent set up with this tool.
//! The call only works if the entry has `auth.spiffe` for that agent's SPIFFE ID, goes to its mTLS
//! port over https, and the agent has allowed the caller (`agent allow`). `agent deploy` warns about
//! what it finds, `doctor` reports it per deployed agent.

use grauwolf_k8s::Cluster;
use serde_json::Value;

use crate::callers::{self, is_allowed};
use crate::cli::Settings;
use crate::spire;

/// What is wrong with one entry, and what to do about it.
pub struct Problem {
    pub message: String,
    pub hint: String,
}

/// The result of the check.
#[derive(Default)]
pub struct Findings {
    /// The agents of the cluster that the spec calls (`name`), without repeats.
    pub targets: Vec<String>,
    pub problems: Vec<Problem>,
}

/// The specs among `texts`: JSON objects with a `spec_version` (a tree holds prompts and other
/// files too).
pub fn spec_documents<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<Value> {
    texts
        .into_iter()
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .filter(|document| document.get("spec_version").is_some())
        .collect()
}

/// `(scheme, host, port)` of a URL; the port defaults to the scheme's.
fn parse_url(url: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => {
            (host, port.parse().ok()?)
        }
        _ => (
            authority,
            if scheme.eq_ignore_ascii_case("https") {
                443
            } else {
                80
            },
        ),
    };
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase(), port))
}

/// The agent a host names: `<name>.<namespace>.svc` (with or without `.cluster.local`).
fn agent_of_host<'a>(host: &str, agents: &'a [(String, String)]) -> Option<&'a (String, String)> {
    let mut parts = host.split('.');
    let (name, namespace, svc) = (parts.next()?, parts.next()?, parts.next()?);
    (svc == "svc")
        .then(|| {
            agents
                .iter()
                .find(|(agent, ns)| agent == name && ns == namespace)
        })
        .flatten()
}

/// The entries of the specs that call an agent of the cluster: `(spec name, "agent_tools.x",
/// the entry, the agent, its namespace)`.
fn entries<'a>(
    specs: &'a [Value],
    agents: &'a [(String, String)],
) -> Vec<(String, String, &'a Value, &'a (String, String))> {
    let mut found = Vec::new();
    for spec in specs {
        let name = spec["name"].as_str().unwrap_or("spec");
        for section in ["agent_tools", "mcp_servers"] {
            let Some(map) = spec[section].as_object() else {
                continue;
            };
            for (key, entry) in map {
                let target = entry["url"]
                    .as_str()
                    .and_then(parse_url)
                    .and_then(|(_, host, _)| agent_of_host(&host, agents));
                if let Some(target) = target {
                    found.push((name.to_owned(), format!("{section}.{key}"), entry, target));
                }
            }
        }
    }
    found
}

/// Checks the specs of agent `caller` (in `caller_ns`) against the agents of the cluster.
pub async fn check(
    cluster: &Cluster,
    settings: &Settings,
    caller: &str,
    caller_ns: &str,
    agents: &[(String, String)],
    specs: &[Value],
) -> Findings {
    let mut findings = Findings::default();
    let found = entries(specs, agents);
    if found.is_empty() {
        return findings;
    }
    let domain = match spire::trust_domain(cluster, settings).await {
        Ok(domain) => domain,
        Err(error) => {
            findings.problems.push(Problem {
                message: format!(
                    "the calls of {caller} to other agents are not checked: the trust domain cannot be read ({error})"
                ),
                hint: "fix the SPIRE server configuration first".to_owned(),
            });
            return findings;
        }
    };
    let caller_id = spire::spiffe_id(&domain, caller_ns, caller);
    for (spec, label, entry, (target, target_ns)) in found {
        if !findings.targets.contains(target) {
            findings.targets.push(target.clone());
        }
        let target_id = spire::spiffe_id(&domain, target_ns, target);
        let at = format!("{spec}: {label} (agent {target})");
        let mut problem = |message: String, hint: String| {
            findings.problems.push(Problem {
                message: format!("{at} {message}"),
                hint,
            });
        };
        let url = entry["url"].as_str().unwrap_or("");
        let spiffe = &entry["auth"]["spiffe"];
        let uses_auth = spiffe.is_object();
        if !uses_auth {
            problem(
                "has no auth.spiffe: the call carries no workload identity".to_owned(),
                format!("add `\"auth\": {{ \"spiffe\": {{ \"id\": \"{target_id}\" }} }}`"),
            );
        } else {
            let id = spiffe["id"].as_str();
            let ids: Vec<&str> = spiffe["ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let trust_domain = spiffe["trust_domain"].as_str();
            let wrong = match (id, ids.is_empty(), trust_domain) {
                (Some(id), ..) if id != target_id => Some(format!("is {id}")),
                (None, false, _) if !ids.contains(&target_id.as_str()) => {
                    Some(format!("lists {}", ids.join(", ")))
                }
                (None, true, Some(td)) if td != domain => Some(format!("is trust domain {td}")),
                _ => None,
            };
            if let Some(wrong) = wrong {
                problem(
                    format!(
                        "has an auth.spiffe that {wrong}, but agent {target} is {target_id}: the TLS handshake would fail"
                    ),
                    format!("set `auth.spiffe.id` to {target_id}"),
                );
            }
            // The JWT mode goes to the HTTP port; mTLS needs https on the mTLS port.
            if spiffe["mode"].as_str().unwrap_or("mtls") == "mtls"
                && let Some((scheme, host, port)) = parse_url(url)
            {
                let (_, mtls) = callers::ports_of(cluster, target_ns, target).await;
                if scheme != "https" || port != mtls {
                    problem(
                        format!(
                            "calls {url}, which is not the mTLS port of agent {target} (https, port {mtls})"
                        ),
                        format!("use `https://{host}:{mtls}`"),
                    );
                }
            }
        }
        // Only a call that carries the identity needs the permission.
        if uses_auth {
            let allowed = match callers::read_callers(cluster, target_ns).await {
                Ok(Some(map)) => is_allowed(&map, &caller_id),
                _ => false,
            };
            if !allowed {
                problem(
                    format!("calls agent {target}, which has not allowed {caller} ({caller_id})"),
                    format!("run `grauwolf-k8-ops agent allow {target} --from {caller}`"),
                );
            }
        }
    }
    findings
}
