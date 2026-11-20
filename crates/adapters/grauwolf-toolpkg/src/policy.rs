//! The tool policy at work (ADR-0107 stage 4): the rules of `grauwolf.toml` plus the public keys
//! they name, loaded from disk, checked whenever a package is resolved.

use std::path::Path;

use grauwolf_git::AllowedSigners;
use grauwolf_oci::cosign::PublicKey;
use grauwolf_spec::OciReference;
use grauwolf_tools::ToolPolicy;

use crate::PackageFetchError;

/// The rules of a policy file and the keys they refer to.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    rules: ToolPolicy,
    oci_keys: Vec<PublicKey>,
    git_signers: Option<AllowedSigners>,
}

impl Policy {
    /// A policy from parsed `rules` and already loaded keys.
    pub fn new(rules: ToolPolicy, oci_keys: Vec<PublicKey>) -> Self {
        Self {
            rules,
            oci_keys,
            git_signers: None,
        }
    }

    /// The same policy with the signers that may sign git commits and tags.
    pub fn with_git_signers(mut self, signers: AllowedSigners) -> Self {
        self.git_signers = Some(signers);
        self
    }

    /// Reads the policy file `path`; the key files it names are read relative to it. The errors
    /// name the file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("tool policy {}: {error}", path.display()))?;
        let rules =
            ToolPolicy::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        let mut oci_keys = Vec::new();
        for key_path in &rules.oci_keys {
            let key_file = base.join(key_path);
            let pem = std::fs::read_to_string(&key_file)
                .map_err(|error| format!("tool policy key {}: {error}", key_file.display()))?;
            let before = oci_keys.len();
            for block in pem_blocks(&pem) {
                oci_keys.push(
                    PublicKey::from_pem(&block).map_err(|error| {
                        format!("tool policy key {}: {error}", key_file.display())
                    })?,
                );
            }
            if oci_keys.len() == before {
                return Err(format!(
                    "tool policy key {}: no PEM public key found",
                    key_file.display()
                ));
            }
        }
        let git_signers = match &rules.git_allowed_signers {
            Some(file) => {
                let file = base.join(file);
                let text = std::fs::read_to_string(&file).map_err(|error| {
                    format!("tool policy allowed signers {}: {error}", file.display())
                })?;
                Some(AllowedSigners::parse(&text).map_err(|error| {
                    format!("tool policy allowed signers {}: {error}", file.display())
                })?)
            }
            None => None,
        };
        Ok(Self {
            rules,
            oci_keys,
            git_signers,
        })
    }

    /// The parsed rules.
    pub fn rules(&self) -> &ToolPolicy {
        &self.rules
    }

    /// The public keys that verify the signatures of OCI packages.
    pub fn oci_keys(&self) -> &[PublicKey] {
        &self.oci_keys
    }

    /// The signers that may sign git commits and tags (`git_allowed_signers`), if the policy
    /// names a file.
    pub fn git_signers(&self) -> Option<&AllowedSigners> {
        self.git_signers.as_ref()
    }

    /// May `reference` be used?
    pub fn check_oci(&self, reference: &OciReference) -> Result<(), PackageFetchError> {
        self.rules
            .check_oci(&reference.registry, &reference.repository)
            .map_err(|violation| PackageFetchError::Policy(violation.to_string()))
    }

    /// May the git repository `url` be used?
    pub fn check_git(&self, url: &str) -> Result<(), PackageFetchError> {
        self.rules
            .check_git(url)
            .map_err(|violation| PackageFetchError::Policy(violation.to_string()))
    }
}

/// The PEM blocks (`-----BEGIN … -----END …-----`) of `text`, each with its markers.
fn pem_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("-----BEGIN ") {
            current = Some(String::new());
        }
        if let Some(block) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
            if line.starts_with("-----END ") {
                blocks.push(current.take().expect("inside a block"));
            }
        }
    }
    blocks
}
