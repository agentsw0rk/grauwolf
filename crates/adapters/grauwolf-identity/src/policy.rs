//! The identity service's policy (ADR-0111 stage 1): which workloads may request Txn-Tokens on
//! behalf of people, and for which purposes. The file is read at start and again when it changes.
//!
//! ```json
//! { "callers": {
//!     "spiffe://agents.local/ns/agents/sa/coordinator": { "purposes": ["review"] },
//!     "spiffe://agents.local/ns/frontends/*": {}
//! } }
//! ```
//! An entry without `purposes` allows any purpose (and none); with a list, a requested purpose
//! must be in it. The exact entry wins over prefixes, the longest prefix over shorter ones.
//!
//! Grants (stage 3) have a section of their own: which entry agents may run on a grant, with
//! which purposes, and which workloads may trigger such runs. It is checked when a grant is
//! created and again at every run.
//!
//! ```json
//! { "callers": {},
//!   "grants": {
//!     "agents": { "spiffe://agents.local/ns/coordinator/sa/coordinator": { "purposes": ["review"] } },
//!     "triggers": ["spiffe://agents.local/ns/schedules/*"]
//! } }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use grauwolf_spiffe::SpiffeId;
use serde::Deserialize;
use tokio::task::JoinHandle;

/// How often the policy file is checked for changes.
pub const POLICY_RELOAD_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    #[serde(default)]
    purposes: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantsFile {
    #[serde(default)]
    agents: HashMap<String, Entry>,
    #[serde(default)]
    triggers: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    callers: HashMap<String, Entry>,
    #[serde(default)]
    grants: GrantsFile,
}

/// SPIFFE IDs and prefixes (`<id>/*`) with their entries.
#[derive(Debug, Default)]
struct Rules {
    exact: HashMap<String, Entry>,
    prefixes: Vec<(String, Entry)>,
}

/// The whole policy: callers (stage 1) and the grants section.
#[derive(Debug, Default)]
struct Policy {
    callers: Rules,
    grant_agents: Rules,
    triggers: Rules,
}

impl Policy {
    fn from_json(text: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|error| format!("identity policy: {error}"))?;
        if !value.is_object() {
            return Err("identity policy: the file must be a JSON object".into());
        }
        let file = File::deserialize(value).map_err(|error| format!("identity policy: {error}"))?;
        Ok(Self {
            callers: Rules::from_entries(file.callers)?,
            grant_agents: Rules::from_entries(file.grants.agents)?,
            triggers: Rules::from_entries(
                file.grants
                    .triggers
                    .into_iter()
                    .map(|id| (id, Entry::default())),
            )?,
        })
    }
}

impl Rules {
    fn from_entries(entries: impl IntoIterator<Item = (String, Entry)>) -> Result<Self, String> {
        let mut rules = Self::default();
        for (key, entry) in entries {
            let check =
                |id: &str| SpiffeId::parse(id).map_err(|error| format!("identity policy: {error}"));
            match key.strip_suffix("/*") {
                Some(prefix) => {
                    check(prefix)?;
                    rules.prefixes.push((prefix.to_owned(), entry));
                }
                None => {
                    check(&key)?;
                    rules.exact.insert(key, entry);
                }
            }
        }
        Ok(rules)
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        if let Some(entry) = self.exact.get(id) {
            return Some(entry);
        }
        self.prefixes
            .iter()
            .filter(|(prefix, _)| {
                id.strip_prefix(prefix.as_str())
                    .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, entry)| entry)
    }
}

/// The policy, possibly kept up to date from its file.
pub struct IdentityPolicy {
    rules: Arc<RwLock<Policy>>,
    task: Option<JoinHandle<()>>,
}

impl IdentityPolicy {
    /// A fixed policy from JSON (tests, no reload).
    pub fn from_json(text: &str) -> Result<Self, String> {
        Ok(Self {
            rules: Arc::new(RwLock::new(Policy::from_json(text)?)),
            task: None,
        })
    }

    /// Reads `path` now (an error stops the start) and every `interval` again when it changed; a
    /// changed file that is invalid is reported and the old policy stays.
    pub fn watch(path: &Path, interval: Duration) -> Result<Self, String> {
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .map_err(|error| format!("{}: {error}", path.display()))
                .and_then(|text| {
                    Policy::from_json(&text).map_err(|error| format!("{}: {error}", path.display()))
                })
        };
        let rules = Arc::new(RwLock::new(read(path)?));
        let (shared, path) = (Arc::clone(&rules), path.to_owned());
        let task = tokio::spawn(async move {
            let mut seen = fingerprint(&path);
            loop {
                tokio::time::sleep(interval).await;
                let now = fingerprint(&path);
                if now == seen {
                    continue;
                }
                seen = now;
                match read(&path) {
                    Ok(fresh) => {
                        *shared.write().unwrap() = fresh;
                        eprintln!("grauwolf identity: policy {} reloaded", path.display());
                    }
                    Err(error) => eprintln!(
                        "grauwolf identity: policy not reloaded, keeping the old one: {error}"
                    ),
                }
            }
        });
        Ok(Self {
            rules,
            task: Some(task),
        })
    }

    /// `Ok` if `caller` may request a Txn-Token on behalf of a person with `purpose`; else the
    /// reason.
    pub fn allows(&self, caller: &SpiffeId, purpose: Option<&str>) -> Result<(), String> {
        let rules = self.rules.read().unwrap();
        let entry = rules
            .callers
            .entry(caller.as_str())
            .ok_or_else(|| format!("{caller} may not request tokens on behalf of people"))?;
        match (&entry.purposes, purpose) {
            (Some(allowed), Some(purpose)) if !allowed.iter().any(|p| p == purpose) => Err(
                format!("{caller} may not request tokens for the purpose '{purpose}'"),
            ),
            _ => Ok(()),
        }
    }
}

impl IdentityPolicy {
    /// `Ok` if `agent` may run on a grant with every one of `purposes`, started by `trigger`
    /// (the `grants` section); else the reason.
    pub fn allows_grant(
        &self,
        agent: &str,
        trigger: &str,
        purposes: &[String],
    ) -> Result<(), String> {
        let rules = self.rules.read().unwrap();
        let entry = rules
            .grant_agents
            .entry(agent)
            .ok_or_else(|| format!("{agent} may not run on grants (policy: grants.agents)"))?;
        if let Some(allowed) = &entry.purposes
            && let Some(purpose) = purposes
                .iter()
                .find(|purpose| !allowed.iter().any(|p| p == *purpose))
        {
            return Err(format!(
                "{agent} may not run on grants for the purpose '{purpose}'"
            ));
        }
        if rules.triggers.entry(trigger).is_none() {
            return Err(format!(
                "{trigger} may not trigger grants (policy: grants.triggers)"
            ));
        }
        Ok(())
    }
}

impl Drop for IdentityPolicy {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// What identifies the file's content: resolved path (a ConfigMap update swaps a symlink),
/// modification time and size.
fn fingerprint(path: &Path) -> Option<(PathBuf, SystemTime, u64)> {
    let resolved = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::metadata(&resolved).ok()?;
    Some((resolved, meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> SpiffeId {
        SpiffeId::parse(text).unwrap()
    }

    #[test]
    fn exact_entries_and_prefixes_allow_callers_and_purposes() {
        let policy = IdentityPolicy::from_json(
            r#"{ "callers": {
                "spiffe://d.de/ns/a/sa/coordinator": { "purposes": ["review"] },
                "spiffe://d.de/ns/web/*": {}
            } }"#,
        )
        .unwrap();
        let coordinator = id("spiffe://d.de/ns/a/sa/coordinator");
        assert!(policy.allows(&coordinator, Some("review")).is_ok());
        assert!(policy.allows(&coordinator, None).is_ok());
        assert!(policy.allows(&coordinator, Some("deploy")).is_err());
        assert!(
            policy
                .allows(&id("spiffe://d.de/ns/web/sa/ui"), Some("x"))
                .is_ok()
        );
        assert!(
            policy
                .allows(&id("spiffe://d.de/ns/webby/sa/ui"), None)
                .is_err()
        );
        assert!(
            policy
                .allows(&id("spiffe://d.de/ns/a/sa/other"), None)
                .is_err()
        );
    }

    #[test]
    fn the_grants_section_names_agents_with_purposes_and_triggers() {
        let policy = IdentityPolicy::from_json(
            r#"{ "grants": {
                "agents": {
                    "spiffe://d.de/ns/c/sa/coordinator": { "purposes": ["review", "report"] },
                    "spiffe://d.de/ns/free/*": {}
                },
                "triggers": ["spiffe://d.de/ns/schedules/*", "spiffe://d.de/ns/ci/sa/runner"]
            } }"#,
        )
        .unwrap();
        let purposes = |list: &[&str]| list.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let coordinator = "spiffe://d.de/ns/c/sa/coordinator";
        let morning = "spiffe://d.de/ns/schedules/sa/morning";
        assert!(
            policy
                .allows_grant(coordinator, morning, &purposes(&["review"]))
                .is_ok()
        );
        assert!(policy.allows_grant(coordinator, morning, &[]).is_ok());
        assert!(
            policy
                .allows_grant(coordinator, morning, &purposes(&["review", "deploy"]))
                .unwrap_err()
                .contains("'deploy'")
        );
        assert!(
            policy
                .allows_grant(
                    "spiffe://d.de/ns/free/sa/x",
                    "spiffe://d.de/ns/ci/sa/runner",
                    &purposes(&["any"])
                )
                .is_ok()
        );
        assert!(
            policy
                .allows_grant("spiffe://d.de/ns/c/sa/other", morning, &[])
                .unwrap_err()
                .contains("grants.agents")
        );
        assert!(
            policy
                .allows_grant(coordinator, "spiffe://d.de/ns/ci/sa/other", &[])
                .unwrap_err()
                .contains("grants.triggers")
        );
        // The callers of stage 1 and the grant agents are separate lists.
        assert!(policy.allows(&id(coordinator), None).is_err());
    }

    #[test]
    fn invalid_policies_are_refused() {
        for bad in [
            r#"{ "callers": { "https://x/y": {} } }"#,
            r#"{ "callers": { "spiffe://d.de/a": { "purpose": [] } } }"#,
            r#"{ "grants": { "triggers": ["schedules/*"] } }"#,
            r#"{ "grants": { "agents": {}, "trigger": [] } }"#,
            "[]",
        ] {
            assert!(IdentityPolicy::from_json(bad).is_err(), "{bad}");
        }
    }
}
