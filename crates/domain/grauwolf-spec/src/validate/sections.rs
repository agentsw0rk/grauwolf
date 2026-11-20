//! Top-level sections: MCP servers, sub-agents, knowledge, long-term memory, self tools and limits
//! (ADR-0036, ADR-0040, ADR-0038, ADR-0058, ADR-0075, ADR-0078).

use std::collections::BTreeMap;

use grauwolf_spec_model::{AuthSpec, KnowledgeStoreKind, SpiffeMode, ToolOrigin};

use super::Rules;
use super::{ValidationError, ValidationErrorKind, escape_pointer_token, is_snake_case_identifier};

impl Rules<'_> {
    /// An error if `agent` is not an entry of `agent_tools` (ADR-0101).
    pub(super) fn check_agent_name(
        &self,
        params: &serde_json::Value,
        path: &str,
    ) -> Option<ValidationError> {
        let name = params["agent"].as_str()?;
        (!self.agent_tools.contains_key(name)).then(|| ValidationError {
            path: format!("{path}/agent"),
            kind: ValidationErrorKind::UnknownAgent { name: name.into() },
        })
    }

    /// Server names are snake_case; their tools share the namespace with the own
    /// tools (ADR-0025), so names must not appear twice.
    pub(super) fn check_mcp_servers(&self, errors: &mut Vec<ValidationError>) {
        for (name, server) in &self.mcp_servers {
            let path = format!("/mcp_servers/{}", escape_pointer_token(name));
            if !is_snake_case_identifier(name) {
                errors.push(ValidationError {
                    path: path.clone(),
                    kind: ValidationErrorKind::InvalidId { id: name.clone() },
                });
            }
            if server.command.is_some() == server.url.is_some() {
                errors.push(ValidationError {
                    path: path.clone(),
                    kind: ValidationErrorKind::McpTransport,
                });
            }
            check_auth(
                &path,
                server.auth.as_ref(),
                server.url.is_some(),
                &server.headers,
                errors,
            );
            if server.workload_api && server.command.is_none() {
                errors.push(ValidationError {
                    path: format!("{path}/workload_api"),
                    kind: ValidationErrorKind::WorkloadApiWithoutStdio,
                });
            }
            if server.transport.is_some() && server.url.is_none() {
                errors.push(ValidationError {
                    path: format!("{path}/transport"),
                    kind: ValidationErrorKind::McpTransportWithoutUrl,
                });
            }
        }
        self.check_tool_collisions(
            &self.tool_namespace(),
            &[ToolOrigin::Mcp, ToolOrigin::McpResource],
            errors,
        );
    }

    /// Sub-agents are named like tools (snake_case) and must not take a tool name twice.
    pub(super) fn check_agent_tools(&self, errors: &mut Vec<ValidationError>) {
        for (name, agent_tool) in &self.agent_tools {
            let path = format!("/agent_tools/{}", escape_pointer_token(name));
            if agent_tool.spec.is_some() == agent_tool.url.is_some() {
                errors.push(ValidationError {
                    path: path.clone(),
                    kind: ValidationErrorKind::AgentToolSource,
                });
            } else if agent_tool.spec.is_some() && agent_tool.description.is_none() {
                errors.push(ValidationError {
                    path: format!("{path}/description"),
                    kind: ValidationErrorKind::MissingAgentDescription,
                });
            }
            check_auth(
                &path,
                agent_tool.auth.as_ref(),
                agent_tool.url.is_some(),
                &agent_tool.headers,
                errors,
            );
            if !is_snake_case_identifier(name) {
                errors.push(ValidationError {
                    path,
                    kind: ValidationErrorKind::InvalidId { id: name.clone() },
                });
            }
        }
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::SubAgent], errors);
    }

    /// The search tool needs a free snake_case name and at least one source.
    pub(super) fn check_knowledge(&self, errors: &mut Vec<ValidationError>) {
        let Some(knowledge) = &self.knowledge else {
            return;
        };
        self.check_extra_tool_name("/knowledge/tool", &knowledge.tool, errors);
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::Knowledge], errors);
        if knowledge.sources.is_empty() && knowledge.urls.is_empty() {
            errors.push(ValidationError {
                path: "/knowledge/sources".into(),
                kind: ValidationErrorKind::NoKnowledgeSources,
            });
        }
        if knowledge
            .min_similarity
            .is_some_and(|value| !(0.0..=1.0).contains(&value))
        {
            errors.push(ValidationError {
                path: "/knowledge/min_similarity".into(),
                kind: ValidationErrorKind::InvalidLimit {
                    reason: "`min_similarity` must be between 0 and 1".into(),
                },
            });
        }
        if !knowledge.tool_enabled && !knowledge.inject {
            errors.push(ValidationError {
                path: "/knowledge/tool_enabled".into(),
                kind: ValidationErrorKind::UnusedKnowledge,
            });
        }
        let mut seen_urls = std::collections::HashSet::new();
        for (index, url) in knowledge.urls.iter().enumerate() {
            let problem = match url_problem(url) {
                Some(reason) => Some(reason),
                None if !seen_urls.insert(url.as_str()) => Some("is listed twice".to_owned()),
                None => None,
            };
            if let Some(reason) = problem {
                errors.push(ValidationError {
                    path: format!("/knowledge/urls/{index}"),
                    kind: ValidationErrorKind::InvalidUrl { reason },
                });
            }
        }
        if let Some(store) = &knowledge.store {
            let mut invalid = |path: &str, reason: &str| {
                errors.push(ValidationError {
                    path: path.into(),
                    kind: ValidationErrorKind::InvalidStore {
                        reason: reason.into(),
                    },
                });
            };
            match (store.kind, &store.url_env) {
                (KnowledgeStoreKind::Postgres, None) => invalid(
                    "/knowledge/store/url_env",
                    "kind `postgres` needs `url_env`, the name of the environment variable with the connection URL",
                ),
                (KnowledgeStoreKind::Postgres, Some(name)) if !is_env_name(name) => invalid(
                    "/knowledge/store/url_env",
                    "`url_env` must be the name of an environment variable (letters, digits, `_`), not the URL itself",
                ),
                (KnowledgeStoreKind::File, Some(_)) => {
                    invalid("/knowledge/store/url_env", "kind `file` takes no `url_env`");
                }
                _ => {}
            }
            if store.kind == KnowledgeStoreKind::Postgres && knowledge.index.is_some() {
                invalid(
                    "/knowledge/index",
                    "`index` applies to the store kind `file`; PostgreSQL keeps the index itself",
                );
            }
        }
    }

    /// The memory tools need free snake_case names (ADR-0058).
    pub(super) fn check_long_term_memory(&self, errors: &mut Vec<ValidationError>) {
        let Some(memory) = &self.long_term_memory else {
            return;
        };
        self.check_extra_tool_name(
            "/long_term_memory/remember_tool",
            &memory.remember_tool,
            errors,
        );
        self.check_extra_tool_name("/long_term_memory/recall_tool", &memory.recall_tool, errors);
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::Memory], errors);
        self.check_memory_store(memory, errors);
        if !memory.tools
            && !memory.auto_recall
            && !memory.auto_remember
            && !memory.remember_compressed
        {
            errors.push(ValidationError {
                path: "/long_term_memory/tools".into(),
                kind: ValidationErrorKind::UnusedMemory,
            });
        }
    }

    /// Exactly one of `store` and `vector_store`; the vector store needs a variable name, a TTL
    /// of at least 1 day and a minimum similarity in `[0, 1]` (ADR-0106).
    fn check_memory_store(
        &self,
        memory: &grauwolf_spec_model::LongTermMemorySpec,
        errors: &mut Vec<ValidationError>,
    ) {
        let mut invalid = |path: &str, reason: &str| {
            errors.push(ValidationError {
                path: path.into(),
                kind: ValidationErrorKind::InvalidStore {
                    reason: reason.into(),
                },
            });
        };
        if memory.store.is_some() == memory.vector_store.is_some() {
            invalid(
                "/long_term_memory",
                "give exactly one of `store` (JSON file) and `vector_store` (PostgreSQL with pgvector)",
            );
        }
        let Some(vector) = &memory.vector_store else {
            return;
        };
        if !is_env_name(&vector.url_env) {
            invalid(
                "/long_term_memory/vector_store/url_env",
                "`url_env` must be the name of an environment variable (letters, digits, `_`), not the URL itself",
            );
        }
        if vector.ttl_days == Some(0) {
            invalid(
                "/long_term_memory/vector_store/ttl_days",
                "`ttl_days` must be at least 1; leave it out for facts that never expire",
            );
        }
        if vector
            .min_similarity
            .is_some_and(|value| !(0.0..=1.0).contains(&value))
        {
            invalid(
                "/long_term_memory/vector_store/min_similarity",
                "`min_similarity` must be between 0 and 1",
            );
        }
    }

    /// Steps and tokens at least 1, cost above 0 (ADR-0078).
    pub(super) fn check_limits(&self, errors: &mut Vec<ValidationError>) {
        let Some(limits) = &self.limits else {
            return;
        };
        let mut invalid = |field: &str, reason: &str| {
            errors.push(ValidationError {
                path: format!("/limits/{field}"),
                kind: ValidationErrorKind::InvalidLimit {
                    reason: reason.into(),
                },
            });
        };
        if limits.max_steps == Some(0) {
            invalid("max_steps", "at least 1 step");
        }
        if limits.max_tokens == Some(0) {
            invalid("max_tokens", "at least 1 token");
        }
        if limits.max_prompt_tokens == Some(0) {
            invalid("max_prompt_tokens", "at least 1 token");
        }
        if limits
            .max_cost
            .is_some_and(|cost| cost.is_nan() || cost <= 0.0)
        {
            invalid("max_cost", "cost must be greater than 0");
        }
    }

    /// The tool for creating needs a free name; `save` needs a `tools_dir` (ADR-0075).
    pub(super) fn check_self_tools(&self, errors: &mut Vec<ValidationError>) {
        let Some(self_tools) = &self.self_tools else {
            for (name, subgraph) in &self.subgraphs {
                if subgraph.self_tools.is_some() {
                    errors.push(ValidationError {
                        path: format!("/subgraphs/{}/self_tools", escape_pointer_token(name)),
                        kind: ValidationErrorKind::SelfToolsNotConfigured,
                    });
                }
            }
            return;
        };
        self.check_extra_tool_name("/self_tools/tool", &self_tools.tool, errors);
        self.check_tool_collisions(&self.tool_namespace(), &[ToolOrigin::SelfBuild], errors);
        if self_tools.save && self.tools_dir.is_none() {
            errors.push(ValidationError {
                path: "/self_tools/save".into(),
                kind: ValidationErrorKind::SaveWithoutToolsDir,
            });
        }
    }
}

/// An environment variable name: letters, digits and `_`, not starting with a digit.
pub(super) fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why `url` cannot be a knowledge URL (ADR-0113): `https://` (plain `http://` only for
/// `localhost`/`127.0.0.1`), a host, no credentials, no fragment. Never repeats the URL.
fn url_problem(url: &str) -> Option<String> {
    let (secure, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (None, Some(rest)) => (false, rest),
        _ => {
            return Some(
                "must start with https:// (http:// only for localhost and 127.0.0.1)".into(),
            );
        }
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Some("must not contain credentials (user:password@)".into());
    }
    if authority.is_empty() {
        return Some("has no host".into());
    }
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    if !secure && !matches!(host, "localhost" | "127.0.0.1") {
        return Some("must start with https:// (http:// only for localhost and 127.0.0.1)".into());
    }
    if url.contains('#') {
        return Some("must not contain a fragment (#...)".into());
    }
    None
}

/// A trust domain: `a-z 0-9 . - _`, at most 255 characters.
fn trust_domain_problem(domain: &str) -> Option<String> {
    if domain.is_empty() || domain.len() > 255 {
        return Some(format!("the trust domain '{domain}' is empty or too long"));
    }
    domain
        .chars()
        .find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '.' | '-' | '_'))
        .map(|bad| {
            format!(
                "the trust domain '{domain}' has the character '{bad}' (allowed: a-z 0-9 . - _)"
            )
        })
}

/// `spiffe://<trust domain>/<path>`; the path segments are not empty, not `.` or `..`.
fn spiffe_id_problem(id: &str) -> Option<String> {
    let Some(rest) = id.strip_prefix("spiffe://") else {
        return Some(format!(
            "'{id}' is not a SPIFFE ID (spiffe://<trust domain>/<path>)"
        ));
    };
    let (domain, path) = match rest.split_once('/') {
        Some((domain, path)) => (domain, Some(path)),
        None => (rest, None),
    };
    if let Some(problem) = trust_domain_problem(domain) {
        return Some(problem);
    }
    path?.split('/').find_map(|segment| {
        let bad = segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        bad.then(|| {
            format!("'{id}': the path has an empty, '.', '..' or invalid segment '{segment}'")
        })
    })
}

/// `auth` of a server or sub-agent (ADR-0109): only with `url`, not with an `Authorization`
/// header, and a complete, valid `spiffe` section.
fn check_auth(
    path: &str,
    auth: Option<&AuthSpec>,
    has_url: bool,
    headers: &BTreeMap<String, String>,
    errors: &mut Vec<ValidationError>,
) {
    let Some(AuthSpec { spiffe }) = auth else {
        return;
    };
    let mut add = |pointer: String, kind: ValidationErrorKind| {
        errors.push(ValidationError {
            path: pointer,
            kind,
        });
    };
    let base = format!("{path}/auth");
    if !has_url {
        add(base.clone(), ValidationErrorKind::AuthWithoutUrl);
    }
    if headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
    {
        add(
            base.clone(),
            ValidationErrorKind::AuthWithAuthorizationHeader,
        );
    }
    check_spiffe(&base, spiffe, errors);
}

/// The `spiffe` section under `base` (`…/auth`): one way to name the server, `audience` exactly
/// with `jwt`, valid IDs and trust domain.
pub(super) fn check_spiffe(
    base: &str,
    spiffe: &crate::SpiffeAuthSpec,
    errors: &mut Vec<ValidationError>,
) {
    let mut add = |pointer: String, kind: ValidationErrorKind| {
        errors.push(ValidationError {
            path: pointer,
            kind,
        });
    };
    let spiffe_path = format!("{base}/spiffe");
    let given = usize::from(spiffe.id.is_some())
        + usize::from(!spiffe.ids.is_empty())
        + usize::from(spiffe.trust_domain.is_some());
    match spiffe.mode() {
        SpiffeMode::Mtls => {
            if given != 1 {
                add(spiffe_path.clone(), ValidationErrorKind::SpiffeMatch);
            }
            if spiffe.audience.is_some() {
                add(
                    format!("{spiffe_path}/audience"),
                    ValidationErrorKind::SpiffeAudienceWithMtls,
                );
            }
        }
        SpiffeMode::Jwt => {
            if spiffe.audience.as_deref().is_none_or(str::is_empty) {
                add(spiffe_path.clone(), ValidationErrorKind::SpiffeAudience);
            }
            if given > 1 {
                add(spiffe_path.clone(), ValidationErrorKind::SpiffeMatch);
            }
        }
    }
    let mut invalid = |pointer: String, reason: Option<String>| {
        if let Some(reason) = reason {
            errors.push(ValidationError {
                path: pointer,
                kind: ValidationErrorKind::InvalidSpiffe { reason },
            });
        }
    };
    if let Some(id) = &spiffe.id {
        invalid(format!("{spiffe_path}/id"), spiffe_id_problem(id));
    }
    for (index, id) in spiffe.ids.iter().enumerate() {
        invalid(format!("{spiffe_path}/ids/{index}"), spiffe_id_problem(id));
    }
    if let Some(domain) = &spiffe.trust_domain {
        invalid(
            format!("{spiffe_path}/trust_domain"),
            trust_domain_problem(domain),
        );
    }
}
