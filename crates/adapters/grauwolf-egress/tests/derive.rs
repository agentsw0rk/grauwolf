//! The derivation per kind of destination, on specs in memory.

use grauwolf_egress::{Destination, Options, Phase, derive_spec, resolve_all};
use grauwolf_spec::AgentSpec;
use serde_json::{Value, json};

fn spec(extra: Value) -> AgentSpec {
    let mut base = json!({"spec_version": 1, "name": "a",
        "model": {"provider": "anthropic", "id": "m"}, "graph": {"nodes": [], "edges": []}});
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    AgentSpec::parse(&base.to_string()).unwrap()
}

fn derive(extra: Value) -> Vec<Destination> {
    derive_spec(&spec(extra), "", &Options::default())
}

fn find<'a>(all: &'a [Destination], host: &str) -> &'a Destination {
    all.iter()
        .find(|destination| destination.host.as_deref() == Some(host))
        .unwrap_or_else(|| panic!("no {host} in {all:#?}"))
}

#[test]
fn the_main_model_has_the_default_host_of_its_provider() {
    let all = derive(json!({}));
    assert_eq!(all.len(), 1);
    let destination = &all[0];
    assert_eq!(destination.target(), "https://api.anthropic.com:443");
    assert_eq!(destination.purpose, "model (anthropic)");
    assert_eq!(destination.pointer, "/model/provider");
    assert_eq!(destination.phase, Phase::Run);
    assert_eq!(destination.from_env, None);
}

#[test]
fn ollama_names_its_variable_and_keeps_the_default() {
    let all = derive(json!({"model": {"provider": "ollama", "id": "m"}}));
    assert_eq!(all[0].target(), "http://localhost:11434");
    assert_eq!(all[0].from_env.as_deref(), Some("OLLAMA_HOST"));
}

#[test]
fn bedrock_has_the_default_region_and_names_aws_region() {
    let all = derive(json!({"model": {"provider": "bedrock", "id": "m"}}));
    assert_eq!(
        all[0].target(),
        "https://bedrock-runtime.us-west-2.amazonaws.com:443"
    );
    assert_eq!(all[0].from_env.as_deref(), Some("AWS_REGION"));
}

#[test]
fn compatible_presets_have_their_host_and_base_url_variable() {
    let all = derive(json!({"model": {"provider": "groq", "id": "m"}}));
    assert_eq!(all[0].host.as_deref(), Some("api.groq.com"));
    assert_eq!(all[0].from_env.as_deref(), Some("GROQ_BASE_URL"));
}

#[test]
fn every_place_that_names_a_model_is_a_destination() {
    let all = derive(json!({
        "model": {"provider": "anthropic", "id": "m",
            "fallbacks": [{"provider": "openai", "id": "f"}]},
        "subgraphs": {"s": {"input": "string", "output": "string",
            "model": {"provider": "gemini", "id": "g"}, "graph": {"nodes": [], "edges": []}}},
        "graph": {"nodes": [
            {"id": "m", "kind": "set_model", "params": {"provider": "mistral", "id": "x"}},
            {"id": "d", "kind": "decide", "params": {"model": "groq/llama", "decider": "llm"}}],
            "edges": []},
        "knowledge": {"sources": [], "embedding_model": {"provider": "ollama", "id": "e"}},
        "long_term_memory": {"embedding_model": {"provider": "deepseek", "id": "e"}}
    }));
    for host in [
        "api.anthropic.com",
        "api.openai.com",
        "generativelanguage.googleapis.com",
        "api.mistral.ai",
        "api.groq.com",
        "localhost",
        "api.deepseek.com",
    ] {
        find(&all, host);
    }
    assert_eq!(find(&all, "localhost").purpose, "embeddings (ollama)");
    assert_eq!(
        find(&all, "api.openai.com").pointer,
        "/model/fallbacks/0/provider"
    );
}

#[test]
fn http_mcp_servers_and_remote_agents_are_destinations_but_stdio_is_not() {
    let all = derive(json!({
        "mcp_servers": {
            "docs": {"url": "https://Docs.Example.com/mcp", "tools": []},
            "files": {"command": "npx", "args": [], "tools": []}},
        "agent_tools": {"peer": {"url": "http://peer.team.svc:8080/a2a"}}
    }));
    let docs = find(&all, "docs.example.com");
    assert_eq!(docs.port, Some(443));
    assert_eq!(docs.purpose, "MCP server docs");
    assert_eq!(docs.pointer, "/mcp_servers/docs/url");
    let peer = find(&all, "peer.team.svc");
    assert_eq!(
        (peer.scheme.as_deref(), peer.port),
        (Some("http"), Some(8080))
    );
    assert_eq!(peer.purpose, "agent peer (A2A)");
    assert_eq!(all.len(), 3);
}

#[test]
fn fetch_hosts_are_listed_as_given_with_443_and_80_when_http_is_allowed() {
    let all = derive(json!({"builtin_tools": {"tools": ["fetch"], "fetch": {
        "allowed_hosts": ["*.Example.com", "10.0.0.5"], "allow_http": true}}}));
    let pattern: Vec<_> = all
        .iter()
        .filter(|d| d.host.as_deref() == Some("*.example.com"))
        .map(|d| d.port)
        .collect();
    assert_eq!(pattern, [Some(80), Some(443)]);
    assert_eq!(
        find(&all, "10.0.0.5").pointer,
        "/builtin_tools/fetch/allowed_hosts/1"
    );
}

#[test]
fn a_fetch_host_inside_the_cluster_has_no_fixed_port() {
    let all = derive(json!({"builtin_tools": {"tools": ["fetch"], "fetch": {
        "allowed_hosts": ["target.target.svc", "*.team.svc", "api.example.com"],
        "allow_http": true}}}));
    for host in ["target.target.svc", "*.team.svc"] {
        let entries: Vec<_> = all
            .iter()
            .filter(|d| d.host.as_deref() == Some(host))
            .collect();
        assert_eq!(entries.len(), 2, "{host}: https and http");
        for entry in entries {
            assert_eq!(entry.port, None, "{host}");
            assert!(entry.any_port, "{host}");
        }
    }
    assert_eq!(
        find(&all, "target.target.svc").target(),
        "http://target.target.svc:*"
    );
    // An external host keeps its ports and says nothing about "any".
    for entry in all
        .iter()
        .filter(|d| d.host.as_deref() == Some("api.example.com"))
    {
        assert!(!entry.any_port);
        assert!(entry.port.is_some());
    }
    assert_eq!(
        find(&all, "target.target.svc").to_json()["any_port"],
        json!(true)
    );
    assert_eq!(
        find(&all, "api.example.com").to_json()["any_port"],
        json!(false)
    );
}

#[test]
fn a_cluster_host_survives_resolving_and_shows_in_the_text_format() {
    let all = derive(json!({"builtin_tools": {"tools": ["fetch"], "fetch": {
        "allowed_hosts": ["target.target.svc"]}}}));
    let resolved = resolve_all(&all, &|_| None);
    assert_eq!(resolved, all);
    let line = grauwolf_egress::to_text(&resolved);
    assert!(line.contains("https://target.target.svc:*"), "{line}");
}

#[test]
fn jira_confluence_and_git_are_destinations() {
    let all = derive(json!({"builtin_tools": {"tools": [],
        "jira": {"base_url": "https://acme.atlassian.net"},
        "confluence": {"base_url": "https://wiki.acme.net:8443/wiki"},
        "git": {"repo_url": "https://git.acme.net/team/repo.git"}}}));
    assert_eq!(find(&all, "acme.atlassian.net").purpose, "Jira");
    assert_eq!(find(&all, "wiki.acme.net").port, Some(8443));
    assert_eq!(find(&all, "git.acme.net").purpose, "git tools");
}

#[test]
fn system_one_pgvector_and_otel_are_known_only_by_their_variable() {
    let spec = spec(json!({
        "graph": {"nodes": [{"id": "d", "kind": "decide", "params": {"decider": "system_one"}}],
            "edges": []},
        "knowledge": {"sources": [], "embedding_model": {"provider": "anthropic", "id": "e"},
            "store": {"kind": "postgres", "url_env": "PGVECTOR_URL"}}}));
    let all = derive_spec(
        &spec,
        "",
        &Options {
            otel: true,
            ..Options::default()
        },
    );
    let by_env = |name: &str| {
        all.iter()
            .find(|d| d.from_env.as_deref() == Some(name))
            .unwrap()
    };
    for name in [
        "RKEV_BASE_URL",
        "PGVECTOR_URL",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
    ] {
        assert_eq!(by_env(name).host, None, "{name}");
        assert_eq!(by_env(name).target(), "unknown");
    }
    assert_eq!(by_env("PGVECTOR_URL").scheme.as_deref(), Some("postgres"));
    assert_eq!(
        by_env("RKEV_BASE_URL").pointer,
        "/graph/nodes/0/params/decider"
    );
}

#[test]
fn knowledge_urls_are_destinations() {
    let all = derive(
        json!({"knowledge": {"sources": [], "urls": ["https://docs.rs/x"],
        "embedding_model": {"provider": "anthropic", "id": "e"}}}),
    );
    assert_eq!(find(&all, "docs.rs").purpose, "knowledge urls");
}

#[test]
fn openbao_comes_from_the_credentials_file() {
    let toml = "[credentials.openbao]\naddress = \"http://openbao.openbao.svc:8200\"\n";
    let all = derive_spec(
        &spec(json!({})),
        "",
        &Options {
            credentials_toml: Some(toml),
            ..Options::default()
        },
    );
    let bao = find(&all, "openbao.openbao.svc");
    assert_eq!(bao.port, Some(8200));
    assert_eq!(bao.purpose, "OpenBao (credentials)");
}

#[test]
fn package_registries_and_git_sources_are_build_destinations() {
    let all = derive(json!({
        "tool_packages": [{"oci": "ghcr.io/acme/tools/math:1.0"},
            {"git": "https://github.com/acme/tools.git", "tag": "v1"}],
        "skills": [{"git": "https://git.acme.net/skills.git", "tag": "v1"}]}));
    for host in ["ghcr.io", "github.com", "git.acme.net"] {
        assert_eq!(find(&all, host).phase, Phase::Build, "{host}");
    }
    // Run-phase entries come first.
    assert_eq!(all[0].phase, Phase::Run);
}

#[test]
fn duplicates_are_merged_and_their_purposes_and_pointers_joined() {
    let all = derive(json!({
        "model": {"provider": "anthropic", "id": "m", "fallbacks": [{"provider": "anthropic", "id": "n"}]},
        "mcp_servers": {"x": {"url": "https://api.anthropic.com/mcp", "tools": []}}}));
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].purpose, "MCP server x; model (anthropic)");
    assert_eq!(
        all[0].pointer,
        "/mcp_servers/x/url, /model/fallbacks/0/provider, /model/provider"
    );
}

#[test]
fn a_sub_agent_names_its_spec_file_in_the_pointer() {
    let all = derive_spec(&spec(json!({})), "agents/w.json", &Options::default());
    assert_eq!(all[0].pointer, "agents/w.json:/model/provider");
}

#[test]
fn from_env_is_resolved_with_the_values_the_deployment_knows() {
    let spec = spec(json!({
        "model": {"provider": "ollama", "id": "m",
            "fallbacks": [{"provider": "bedrock", "id": "b"}]},
        "graph": {"nodes": [{"id": "d", "kind": "decide", "params": {"decider": "system_one"}}],
            "edges": []}}));
    let all = derive_spec(&spec, "", &Options::default());
    let known = |name: &str| match name {
        "OLLAMA_HOST" => Some("10.1.2.3:11434".to_owned()),
        "AWS_REGION" => Some("eu-central-1".to_owned()),
        "RKEV_BASE_URL" => Some("https://rkev.example.com".to_owned()),
        _ => None,
    };
    let resolved = resolve_all(&all, &known);
    assert_eq!(
        resolved.iter().map(Destination::target).collect::<Vec<_>>(),
        [
            "http://10.1.2.3:11434",
            "https://bedrock-runtime.eu-central-1.amazonaws.com:443",
            "https://rkev.example.com:443"
        ]
    );
    // Without values nothing changes.
    assert_eq!(resolve_all(&all, &|_| None), all);
}
