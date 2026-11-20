//! Policy for tool packages (ADR-0107 stage 4): which registries and git hosts are allowed
//! and whether signatures are required. Parsing the policy file is pure (text in, value out);
//! reading key files and checking signatures is the job of the adapters.

use serde::Deserialize;

/// The settings of `[tool_packages]` in `grauwolf.toml` (or the file named by
/// `GRAUWOLF_TOOL_POLICY`). Paths are as written; the caller resolves them relative to the
/// policy file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicy {
    /// OCI registries (`ghcr.io`) or registry/repository prefixes (`ghcr.io/acme`) that may be
    /// used. Missing: any; empty: none.
    #[serde(default)]
    pub allowed_registries: Option<Vec<String>>,
    /// Git hosts (`github.com`) or host/path prefixes (`github.com/acme`) that may be used.
    /// Missing: any; empty: none.
    #[serde(default)]
    pub allowed_git_hosts: Option<Vec<String>>,
    /// Every package must carry a valid signature of one of the keys below.
    #[serde(default)]
    pub require_signatures: bool,
    /// Public keys (PEM, SPKI, P-256) that verify the cosign signatures of OCI packages.
    #[serde(default)]
    pub oci_keys: Vec<String>,
    /// File in the format of git's `gpg.ssh.allowedSignersFile` for signed commits and tags.
    #[serde(default)]
    pub git_allowed_signers: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default)]
    tool_packages: ToolPolicy,
    /// The `[credentials]` table belongs to `grauwolf-credentials` (ADR-0110), which checks it.
    #[serde(default)]
    #[allow(dead_code)]
    credentials: Option<serde::de::IgnoredAny>,
}

/// Why a policy file is unusable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// Not valid TOML, an unknown key or a value of the wrong type.
    #[error("invalid tool policy: {0}")]
    Invalid(String),
    /// `require_signatures` without a key to verify with.
    #[error("invalid tool policy: `require_signatures` needs `oci_keys` or `git_allowed_signers`")]
    NoKeys,
}

/// A package source the policy does not allow; the text names the source and the rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PolicyViolation(pub String);

impl ToolPolicy {
    /// Parses the text of a policy file; a file without `[tool_packages]` allows everything.
    pub fn parse(text: &str) -> Result<Self, PolicyError> {
        let file: PolicyFile =
            toml::from_str(text).map_err(|error| PolicyError::Invalid(error.to_string()))?;
        let policy = file.tool_packages;
        if policy.require_signatures
            && policy.oci_keys.is_empty()
            && policy.git_allowed_signers.is_none()
        {
            return Err(PolicyError::NoKeys);
        }
        Ok(policy)
    }

    /// Checks the OCI package `registry/repository` against `allowed_registries`.
    pub fn check_oci(&self, registry: &str, repository: &str) -> Result<(), PolicyViolation> {
        let target = format!("{registry}/{repository}");
        match &self.allowed_registries {
            Some(allowed) if !allowed.iter().any(|prefix| has_prefix(&target, prefix)) => {
                Err(PolicyViolation(format!(
                    "`{target}` is not allowed by the tool policy: `allowed_registries` lists {}",
                    listing(allowed)
                )))
            }
            _ => Ok(()),
        }
    }

    /// Checks the git repository `url` (`https://host/path`) against `allowed_git_hosts`.
    pub fn check_git(&self, url: &str) -> Result<(), PolicyViolation> {
        let target = git_target(url);
        match &self.allowed_git_hosts {
            Some(allowed) if !allowed.iter().any(|prefix| has_prefix(&target, prefix)) => {
                Err(PolicyViolation(format!(
                    "the git repository `{target}` is not allowed by the tool policy: \
                     `allowed_git_hosts` lists {}",
                    listing(allowed)
                )))
            }
            _ => Ok(()),
        }
    }
}

fn listing(allowed: &[String]) -> String {
    if allowed.is_empty() {
        "nothing".to_owned()
    } else {
        allowed
            .iter()
            .map(|entry| format!("`{entry}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `host/path` of a repository URL without scheme and a trailing `.git`.
fn git_target(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.trim_end_matches('/');
    rest.strip_suffix(".git").unwrap_or(rest).to_owned()
}

/// `prefix` matches `target` as a whole or up to a `/` (the host part ignoring case).
fn has_prefix(target: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    let prefix = prefix.split_once("://").map_or(prefix, |(_, rest)| rest);
    let prefix = prefix.strip_suffix(".git").unwrap_or(prefix);
    if prefix.is_empty() || target.len() < prefix.len() || !target.is_char_boundary(prefix.len()) {
        return false;
    }
    let (head, tail) = target.split_at(prefix.len());
    // Registry and host names are case-insensitive, paths are not.
    let host_len = prefix.find('/').unwrap_or(prefix.len());
    head[..host_len].eq_ignore_ascii_case(&prefix[..host_len])
        && head[host_len..] == prefix[host_len..]
        && (tail.is_empty() || tail.starts_with('/'))
}
