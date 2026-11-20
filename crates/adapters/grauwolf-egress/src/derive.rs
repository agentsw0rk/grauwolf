//! What a spec names: every place that holds a destination.

use grauwolf_agent_tree::AgentTree;
use grauwolf_spec::{AgentSpec, escape_pointer_token};

use crate::collect::Collector;
use crate::providers::{self, BEDROCK_DEFAULT_REGION};
use crate::target;
use crate::{Destination, Phase};

/// The OpenTelemetry variable that holds the OTLP endpoint.
pub const OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
/// The variable of the System One (RKEV) service.
pub const RKEV_BASE_URL: &str = "RKEV_BASE_URL";

/// What the derivation takes besides the specs.
#[derive(Debug, Clone, Default)]
pub struct Options<'a> {
    /// The text of a `grauwolf.toml`: its `[credentials.openbao] address` is a destination.
    pub credentials_toml: Option<&'a str>,
    /// The agent exports OpenTelemetry spans (`--otel`).
    pub otel: bool,
}

/// The text of the `grauwolf.toml` next to the root spec of `tree`, if there is one.
pub fn credentials_file(tree: &AgentTree) -> Option<String> {
    std::fs::read_to_string(tree.root().base_dir().join("grauwolf.toml")).ok()
}

/// The destinations of every spec of `tree`, sorted, merged and lower case.
pub fn derive_tree(tree: &AgentTree, options: &Options) -> Vec<Destination> {
    let mut collector = Collector::default();
    for (index, node) in tree.nodes().iter().enumerate() {
        let origin = if index == 0 {
            String::new()
        } else {
            node.relative().to_string_lossy().replace('\\', "/")
        };
        collect_spec(node.spec(), &origin, &mut collector);
    }
    collect_extras(options, &mut collector);
    collector.finish()
}

/// The destinations of one spec (not its sub-agents), `origin` the spec's file in the tree
/// (empty for the root).
pub fn derive_spec(spec: &AgentSpec, origin: &str, options: &Options) -> Vec<Destination> {
    let mut collector = Collector::default();
    collect_spec(spec, origin, &mut collector);
    collect_extras(options, &mut collector);
    collector.finish()
}

fn collect_extras(options: &Options, collector: &mut Collector) {
    if let Some(address) = options
        .credentials_toml
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| {
            table
                .get("credentials")?
                .get("openbao")?
                .get("address")?
                .as_str()
                .map(str::to_owned)
        })
    {
        collector.url(
            Phase::Run,
            &address,
            "OpenBao (credentials)",
            "grauwolf.toml:/credentials/openbao/address",
            None,
        );
    }
    if options.otel {
        collector.by_variable(
            None,
            OTLP_ENDPOINT,
            "OpenTelemetry (OTLP)",
            "environment:OTEL_EXPORTER_OTLP_ENDPOINT",
        );
    }
}

fn collect_spec(spec: &AgentSpec, origin: &str, collector: &mut Collector) {
    let at = |pointer: &str| {
        if origin.is_empty() {
            pointer.to_owned()
        } else {
            format!("{origin}:{pointer}")
        }
    };
    models(spec, &at, collector);
    for (name, server) in &spec.mcp_servers {
        if let Some(url) = &server.url {
            let pointer = at(&format!("/mcp_servers/{}/url", escape_pointer_token(name)));
            collector.url(
                Phase::Run,
                url,
                &format!("MCP server {name}"),
                &pointer,
                None,
            );
        }
    }
    for (name, agent) in &spec.agent_tools {
        if let Some(url) = &agent.url {
            let pointer = at(&format!("/agent_tools/{}/url", escape_pointer_token(name)));
            collector.url(
                Phase::Run,
                url,
                &format!("agent {name} (A2A)"),
                &pointer,
                None,
            );
        }
    }
    if let Some(builtin) = &spec.builtin_tools {
        if let Some(fetch) = &builtin.fetch {
            for (index, host) in fetch.allowed_hosts.iter().enumerate() {
                let pointer = at(&format!("/builtin_tools/fetch/allowed_hosts/{index}"));
                // A Service of the cluster has the ports its Service has: not fixed by the spec.
                let cluster = target::is_cluster_host(host);
                let mut add = |scheme: &str, port: u16| {
                    if cluster {
                        collector.host_any_port(Phase::Run, scheme, host, "fetch tool", &pointer);
                    } else {
                        collector.host(Phase::Run, scheme, host, port, "fetch tool", &pointer);
                    }
                };
                add("https", 443);
                if fetch.allow_http {
                    add("http", 80);
                }
            }
        }
        for (section, label) in [(&builtin.jira, "Jira"), (&builtin.confluence, "Confluence")] {
            if let Some(atlassian) = section {
                let field = label.to_ascii_lowercase();
                collector.url(
                    Phase::Run,
                    &atlassian.base_url,
                    label,
                    &at(&format!("/builtin_tools/{field}/base_url")),
                    None,
                );
            }
        }
        if let Some(git) = &builtin.git {
            collector.url(
                Phase::Run,
                &git.repo_url,
                "git tools",
                &at("/builtin_tools/git/repo_url"),
                None,
            );
        }
    }
    system_one(spec, &at, collector);
    if let Some(knowledge) = &spec.knowledge {
        for (index, url) in knowledge.urls.iter().enumerate() {
            collector.url(
                Phase::Run,
                url,
                "knowledge urls",
                &at(&format!("/knowledge/urls/{index}")),
                None,
            );
        }
        if let Some(variable) = knowledge.store.as_ref().and_then(|s| s.url_env.as_deref()) {
            collector.by_variable(
                Some("postgres"),
                variable,
                "pgvector (knowledge)",
                &at("/knowledge/store/url_env"),
            );
        }
    }
    if let Some(store) = spec
        .long_term_memory
        .as_ref()
        .and_then(|memory| memory.vector_store.as_ref())
    {
        collector.by_variable(
            Some("postgres"),
            &store.url_env,
            "pgvector (long-term memory)",
            &at("/long_term_memory/vector_store/url_env"),
        );
    }
    sources(spec, &at, collector);
}

fn models(spec: &AgentSpec, at: &dyn Fn(&str) -> String, collector: &mut Collector) {
    for (pointer, provider) in spec.provider_uses() {
        let Some(row) = providers::address_of(provider) else {
            continue;
        };
        let purpose = if pointer.contains("embedding_model") {
            format!("embeddings ({provider})")
        } else {
            format!("model ({provider})")
        };
        let pointer = at(&pointer);
        let default_url = if provider == "bedrock" {
            providers::bedrock_url(BEDROCK_DEFAULT_REGION)
        } else {
            row.default_url.to_owned()
        };
        collector.url(Phase::Run, &default_url, &purpose, &pointer, row.from_env);
    }
}

fn system_one(spec: &AgentSpec, at: &dyn Fn(&str) -> String, collector: &mut Collector) {
    let graphs = std::iter::once(("/graph".to_owned(), &spec.graph)).chain(
        spec.subgraphs.iter().map(|(name, subgraph)| {
            (
                format!("/subgraphs/{}/graph", escape_pointer_token(name)),
                &subgraph.graph,
            )
        }),
    );
    for (base, graph) in graphs {
        for (index, node) in graph.nodes.iter().enumerate() {
            let decider = node
                .params
                .as_ref()
                .and_then(|params| params.get("decider"))
                .and_then(serde_json::Value::as_str);
            if node.kind == "decide" && decider == Some("system_one") {
                collector.by_variable(
                    None,
                    RKEV_BASE_URL,
                    "System One (RKEV)",
                    &at(&format!("{base}/nodes/{index}/params/decider")),
                );
            }
        }
    }
}

/// Tool packages and skills from outside: needed while the image is built.
fn sources(spec: &AgentSpec, at: &dyn Fn(&str) -> String, collector: &mut Collector) {
    for (index, package) in spec.tool_packages.iter().enumerate() {
        if let Some(reference) = &package.oci
            && let Ok(oci) = grauwolf_spec::OciReference::parse(reference)
        {
            collector.url(
                Phase::Build,
                &format!("https://{}", oci.registry),
                "tool package registry",
                &at(&format!("/tool_packages/{index}/oci")),
                None,
            );
        }
        if let Some(url) = &package.git {
            collector.url(
                Phase::Build,
                url,
                "tool package git source",
                &at(&format!("/tool_packages/{index}/git")),
                None,
            );
        }
    }
    for (index, skill) in spec.skills.iter().enumerate() {
        if let Some(url) = &skill.git {
            collector.url(
                Phase::Build,
                url,
                "skills git source",
                &at(&format!("/skills/{index}/git")),
                None,
            );
        }
    }
}
