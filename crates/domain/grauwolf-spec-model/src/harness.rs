//! The harness file `<name>.harness.json`: how an agent tree is packaged as a generated,
//! deployable project (ADR-0128).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::SpecError;

/// Version of the harness format that this crate reads.
pub const SUPPORTED_HARNESS_VERSION: u64 = 1;

/// A harness: the boundary of one generated project around a root spec and its local sub-agents.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    /// Version of the harness format.
    pub version: u64,
    /// DNS label naming the project, image, namespace and service.
    pub name: String,
    /// The root spec, relative to the harness file.
    pub spec: String,
    /// The image that runs the agent (`grauwolf-k8-ops agent deploy --harness`).
    #[serde(default)]
    pub image: Option<String>,
    /// The port of the service in the container and of the Kubernetes Service.
    #[serde(default)]
    pub port: Option<u16>,
    /// Values for the pod's environment that are not secret (`NAME` → value).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl Harness {
    /// The image of this harness when its file names none (ADR-0128 H7): `<registry>/<name>:<version>`,
    /// `version` being grauwolf's.
    pub fn default_image(&self, registry: &str, version: &str) -> String {
        format!("{}/{}:{version}", registry.trim_end_matches('/'), self.name)
    }

    /// Parses the harness file `name` (its path, for the messages); errors read
    /// `<name>:<pointer>: …`, or `<name>: invalid harness JSON: …`.
    pub fn parse_named(name: &str, json: &str) -> Result<Self, String> {
        Self::parse(json).map_err(|error| match error {
            SpecError::Json(error) => format!("{name}: invalid harness JSON: {error}"),
            // The others start with their JSON pointer.
            error => format!("{name}:{error}"),
        })
    }

    /// Parses a harness file; format errors carry a JSON pointer like those of a spec (ADR-0080).
    pub fn parse(json: &str) -> Result<Self, SpecError> {
        let document: serde_json::Value = serde_json::from_str(json)?;
        let harness: Self =
            serde_path_to_error::deserialize(document).map_err(crate::format_error)?;
        if harness.version != SUPPORTED_HARNESS_VERSION {
            return Err(SpecError::Format {
                pointer: "/version".into(),
                message: format!(
                    "harness version {} is not supported (supported: {SUPPORTED_HARNESS_VERSION})",
                    harness.version
                ),
            });
        }
        if !is_dns_label(&harness.name) {
            return Err(SpecError::Format {
                pointer: "/name".into(),
                message: "must be a DNS label (a-z, 0-9 and '-', starting with a letter, not \
                          ending with '-', at most 63 characters): it names the project, image, \
                          namespace and service"
                    .into(),
            });
        }
        Ok(harness)
    }
}

/// An RFC 1123 label that also starts with a letter, as Kubernetes namespaces and services need.
fn is_dns_label(name: &str) -> bool {
    name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_harness_names_the_project_and_its_root_spec() {
        let harness = Harness::parse(
            r#"{ "version": 1, "name": "support-bot", "spec": "../agents/support.json" }"#,
        )
        .unwrap();

        assert_eq!(harness.name, "support-bot");
        assert_eq!(harness.spec, "../agents/support.json");
    }

    #[test]
    fn the_default_image_is_the_name_in_the_registry_tagged_with_the_version() {
        let harness =
            Harness::parse(r#"{ "version": 1, "name": "bot", "spec": "bot.json" }"#).unwrap();

        assert_eq!(
            harness.default_image("ghcr.io/agentsw0rk", "0.1.0"),
            "ghcr.io/agentsw0rk/bot:0.1.0"
        );
        assert_eq!(
            harness.default_image("registry.example.com/team/", "0.1.0"),
            "registry.example.com/team/bot:0.1.0"
        );
    }

    #[test]
    fn errors_of_a_named_file_start_with_its_name() {
        let no_json = Harness::parse_named("a.harness.json", "{").unwrap_err();
        let bad_name = Harness::parse_named(
            "a.harness.json",
            r#"{ "version": 1, "name": "A", "spec": "a.json" }"#,
        )
        .unwrap_err();

        assert!(
            no_json.starts_with("a.harness.json: invalid harness JSON: "),
            "{no_json}"
        );
        assert!(
            bad_name.starts_with("a.harness.json:/name: must be a DNS label"),
            "{bad_name}"
        );
    }

    #[test]
    fn an_unknown_format_version_is_refused_at_the_version() {
        let error =
            Harness::parse(r#"{ "version": 2, "name": "support-bot", "spec": "support.json" }"#)
                .unwrap_err();

        assert_eq!(
            error.to_string(),
            "/version: harness version 2 is not supported (supported: 1)"
        );
    }

    #[test]
    fn a_name_that_is_no_dns_label_is_refused_at_the_name() {
        let error =
            Harness::parse(r#"{ "version": 1, "name": "Support_Bot", "spec": "support.json" }"#)
                .unwrap_err();

        assert_eq!(
            error.to_string(),
            "/name: must be a DNS label (a-z, 0-9 and '-', starting with a letter, not ending \
             with '-', at most 63 characters): it names the project, image, namespace and service"
        );
    }
}
