//! Generator: spec → Rust source code (F13.6).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;

use grauwolf_generator::{CompanionKind, GrauwolfDependency};
use grauwolf_spec::AgentSpec;
use grauwolf_tools::ToolArtifact;

fn example(name: &str) -> AgentSpec {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn generates_a_typed_graph_library_for_the_echo_spec() {
    let source = grauwolf_generator::generate_lib(&example("echo.json")).unwrap();

    insta::assert_snapshot!(source);
}

#[test]
fn generates_a_standalone_crate_with_manifest_and_library() {
    let generated = grauwolf_generator::generate_crate(
        &example("echo.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
    )
    .unwrap();

    assert_eq!(
        generated
            .files
            .keys()
            .map(|path| path.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["Cargo.toml", "src/lib.rs"]
    );
    insta::assert_snapshot!(
        "echo_cargo_toml",
        generated.files[&PathBuf::from("Cargo.toml")]
    );
}

#[test]
fn embeds_tool_artifacts_and_runs_them_in_the_sandbox() {
    let artifact = ToolArtifact {
        name: "add".into(),
        bytes: vec![0, 97, 115, 109],
        hash: "abc".into(),
    };

    let generated = grauwolf_generator::generate_crate(
        &example("calculator.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        std::slice::from_ref(&artifact),
    )
    .unwrap();

    assert_eq!(
        generated.binary_files[&PathBuf::from("tools/add.wasm")],
        artifact.bytes
    );
    assert!(generated.files[&PathBuf::from("Cargo.toml")].contains("grauwolf-sandbox"));
    insta::assert_snapshot!(
        "calculator_lib",
        generated.files[&PathBuf::from("src/lib.rs")]
    );
}

#[test]
fn generates_the_temperature_and_think_of_the_models() {
    let source = grauwolf_generator::generate_lib(&example("sampling_echo.json")).unwrap();

    insta::assert_snapshot!("sampling_echo_lib", source);
}

#[test]
fn generates_structs_for_named_types_and_a_typed_run_function() {
    let source = grauwolf_generator::generate_lib(&example("relay.json")).unwrap();

    assert!(source.contains("pub struct Ticket"), "{source}");
    insta::assert_snapshot!("relay_lib", source);
}

fn with_ticket_type(ticket: serde_json::Value) -> AgentSpec {
    AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "relay",
            "model": { "provider": "ollama", "id": "m" },
            "types": { "Ticket": ticket },
            "input": "Ticket", "output": "Ticket",
            "graph": { "edges": [ { "from": "start", "to": "finish" } ] }
        })
        .to_string(),
    )
    .unwrap()
}

/// Pointer of the unsupported schema. Since ADR-0080, the spec check already reports it
/// (`UnsupportedSchema`), before the generator runs – at the same place.
fn unsupported_path(spec: &AgentSpec) -> String {
    match grauwolf_generator::generate_lib(spec) {
        Err(grauwolf_generator::GeneratorError::Invalid(errors)) => match errors.as_slice() {
            [error]
                if matches!(
                    error.kind,
                    grauwolf_spec::ValidationErrorKind::UnsupportedSchema { .. }
                ) =>
            {
                error.path.clone()
            }
            other => panic!("exactly one UnsupportedSchema expected: {other:?}"),
        },
        other => panic!("invalid spec expected: {other:?}"),
    }
}

#[test]
fn reports_field_schemas_without_a_rust_mapping() {
    let spec = with_ticket_type(serde_json::json!({
        "type": "object",
        "properties": { "tags": { "type": "array", "items": { "type": "null" } } }
    }));

    assert_eq!(
        unsupported_path(&spec),
        "/types/Ticket/properties/tags/items"
    );
}

#[test]
fn reports_field_names_that_are_not_identifiers() {
    let spec = with_ticket_type(serde_json::json!({
        "type": "object",
        "properties": { "due date": { "type": "string" } }
    }));

    assert_eq!(unsupported_path(&spec), "/types/Ticket/properties/due date");
}

#[test]
fn escapes_field_names_in_error_pointers() {
    let spec = with_ticket_type(serde_json::json!({
        "type": "object",
        "properties": { "a/b~c": { "type": "string" } }
    }));

    assert_eq!(unsupported_path(&spec), "/types/Ticket/properties/a~1b~0c");
}

#[test]
fn escapes_rust_keywords_in_field_names() {
    let spec = with_ticket_type(serde_json::json!({
        "type": "object",
        "properties": { "type": { "type": "string" } },
        "required": ["type"]
    }));

    let source = grauwolf_generator::generate_lib(&spec).unwrap();

    assert!(source.contains("pub r#type: String"), "{source}");
}

#[test]
fn reports_field_names_that_cannot_be_raw_identifiers() {
    let spec = with_ticket_type(serde_json::json!({
        "type": "object",
        "properties": { "self": { "type": "string" } }
    }));

    assert_eq!(unsupported_path(&spec), "/types/Ticket/properties/self");
}

#[test]
fn generates_structured_requests_with_the_schema_from_the_spec() {
    let source = grauwolf_generator::generate_lib(&example("triage.json")).unwrap();

    assert!(
        source.contains("impl grauwolf_agents::Structured for Ticket"),
        "{source}"
    );
    insta::assert_snapshot!("triage_lib", source);
}

#[test]
fn generates_typed_subgraph_functions_with_their_tool_subset() {
    let source = grauwolf_generator::generate_lib(&example("subgraph_calculator.json")).unwrap();

    assert!(source.contains("pub fn subgraph_solve()"), "{source}");
    insta::assert_snapshot!("subgraph_calculator_lib", source);
}

#[test]
fn generates_plan_nodes_as_llm_planners_over_their_step_subgraph() {
    let source = grauwolf_generator::generate_lib(&example("planner.json")).unwrap();

    assert!(source.contains("grauwolf_planner::plan_node"), "{source}");
    insta::assert_snapshot!("planner_lib", source);
}

/// Addendum to ADR-0070: actions with an `agent` need no step graph; the sub-agents come from the
/// loader like for `agent` nodes.
#[test]
fn generates_goap_nodes_with_agent_actions_and_the_loader_for_the_sub_agents() {
    let source = grauwolf_generator::generate_lib(&example("review_loop.json")).unwrap();

    assert!(
        source.contains("grauwolf_planner::goap::fact_planner"),
        "{source}"
    );
    assert!(source.contains("steps_editor.insert(\"save\""), "{source}");
    assert!(
        !source.contains("steps_editor.insert(\"draft_writer\""),
        "{source}"
    );
    assert!(source.contains("AgentLoader::for_provisioning"), "{source}");
    insta::assert_snapshot!("review_loop_lib", source);
}

#[test]
fn generates_external_tools_as_a_checked_parameter_of_run_with_tools() {
    let mut spec = example("calculator.json");
    spec.builtin_tools = Some(grauwolf_spec::BuiltinToolsSpec {
        tools: vec!["read_file".into()],
        root: Some(".".into()),
        ..Default::default()
    });
    spec.mcp_servers.insert(
        "files".into(),
        grauwolf_spec::McpServerSpec {
            command: Some("npx".into()),
            args: vec![],
            url: None,
            transport: None,
            headers: Default::default(),
            resources: false,
            tools: vec!["list_files".into()],
            auth: None,
            workload_api: false,
        },
    );

    let source = grauwolf_generator::generate_lib(&spec).unwrap();

    assert!(
        source.contains(r#"pub const EXTERNAL_TOOLS: &[&str] = &["list_files", "read_file"];"#),
        "{source}"
    );
    insta::assert_snapshot!("external_tools_lib", source);
}

/// A tools directory (ADR-0074) only exists in the interpreter; the generator says so.
#[test]
fn refuses_a_tools_directory() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "toolbox",
            "model": { "provider": "ollama", "id": "m" },
            "tools_dir": "my-tools",
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let error = grauwolf_generator::generate_lib(&spec).unwrap_err();

    assert!(
        matches!(&error, grauwolf_generator::GeneratorError::Unsupported { feature } if feature == "tools_dir"),
        "{error}"
    );
}

#[test]
fn refuses_self_made_tools() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "toolmaker",
            "model": { "provider": "ollama", "id": "m" },
            "self_tools": {},
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let error = grauwolf_generator::generate_lib(&spec).unwrap_err();

    assert!(
        matches!(&error, grauwolf_generator::GeneratorError::Unsupported { feature } if feature == "self_tools"),
        "{error}"
    );
}

/// ADR-0119: a cost limit is no longer refused; the run takes it as part of its limits and the
/// price table comes with the provisioned agent.
#[test]
fn generates_a_cost_limit_into_the_limits_of_the_run() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "frugal",
            "model": { "provider": "ollama", "id": "m" },
            "limits": { "max_cost": 0.01 },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let source = grauwolf_generator::generate_lib(&spec).unwrap();

    assert!(source.contains("max_cost: Some(0.01)"), "{source}");
    assert!(source.contains("provisioned.prices()"), "{source}");
}

/// ADR-0104: Jira, Confluence and git tools are external tools like the other built-in ones;
/// the facade provides them (feature `builtin`), the credentials are checked before the run.
#[test]
fn generates_jira_confluence_and_git_tools_as_external_tools() {
    let mut spec = example("echo.json");
    spec.builtin_tools = Some(
        serde_json::from_value(serde_json::json!({
            "tools": ["jira_get_issue", "confluence_get_page", "git_checkout"],
            "jira": { "base_url": "https://example.atlassian.net" },
            "confluence": { "base_url": "https://example.atlassian.net" },
            "git": { "repo_url": "https://github.com/org/repo.git" }
        }))
        .unwrap(),
    );

    let source = grauwolf_generator::generate_lib(&spec).unwrap();
    let krate = grauwolf_generator::generate_crate(
        &spec,
        &grauwolf_generator::GrauwolfDependency::Path("crates".into()),
        &[],
    )
    .unwrap();

    for tool in ["jira_get_issue", "confluence_get_page", "git_checkout"] {
        assert!(source.contains(&format!("\"{tool}\",")), "{tool}: {source}");
    }
    assert!(source.contains("provisioned.tools_for("), "{source}");
    let manifest = &krate.files[std::path::Path::new("Cargo.toml")];
    // The groups come with their own facade features (ADR-0108); fetch is not used here.
    for feature in ["\"builtin\"", "\"git\"", "\"atlassian\""] {
        assert!(manifest.contains(feature), "{feature}: {manifest}");
    }
    assert!(!manifest.contains("\"fetch\""), "{manifest}");
}

/// ADR-0099, ADR-0119: the command tool brings its facade feature; in shell mode with an
/// approval per call, generated code gates `run_command` like the interpreter.
#[test]
fn generates_the_command_tool_with_its_feature_and_gates_the_shell_with_approval() {
    let spec_with = |command: serde_json::Value| {
        let mut spec = example("echo.json");
        spec.builtin_tools = Some(
            serde_json::from_value(serde_json::json!({
                "tools": ["run_command"], "root": "work", "command": command
            }))
            .unwrap(),
        );
        spec
    };
    let shell = serde_json::json!({ "allowed": ["*"], "shell": true, "i_understand_shell": true });
    let mut unattended = shell.clone();
    unattended["approve_each_call"] = serde_json::json!(false);
    let dependency = grauwolf_generator::GrauwolfDependency::Path("crates".into());

    let krate = grauwolf_generator::generate_crate(
        &spec_with(serde_json::json!({ "allowed": ["ls"] })),
        &dependency,
        &[],
    )
    .unwrap();
    let manifest = &krate.files[std::path::Path::new("Cargo.toml")];
    assert!(manifest.contains("\"command\""), "{manifest}");
    let source_of = |spec: &AgentSpec| grauwolf_generator::generate_lib(spec).unwrap();
    assert!(source_of(&spec_with(unattended)).contains("&[]"));
    let gated = source_of(&spec_with(shell));
    assert!(gated.contains("&[\"run_command\"]"), "{gated}");
}

/// ADR-0108: the `fetch` tool needs the facade feature `fetch`, and only it.
#[test]
fn generates_the_fetch_tool_with_the_facade_feature_fetch() {
    let mut spec = example("echo.json");
    spec.builtin_tools = Some(
        serde_json::from_value(serde_json::json!({
            "tools": ["fetch"],
            "fetch": { "allowed_hosts": ["example.org"] }
        }))
        .unwrap(),
    );

    let krate = grauwolf_generator::generate_crate(
        &spec,
        &grauwolf_generator::GrauwolfDependency::Path("crates".into()),
        &[],
    )
    .unwrap();

    let manifest = &krate.files[std::path::Path::new("Cargo.toml")];
    assert!(manifest.contains("\"fetch\""), "{manifest}");
    assert!(
        !manifest.contains("\"git\"") && !manifest.contains("\"atlassian\""),
        "{manifest}"
    );
}

#[test]
fn generates_a_service_binary_behind_the_feature_service() {
    let generated = grauwolf_generator::generate_crate_with(
        &example("echo.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        &grauwolf_generator::GenerateOptions {
            service: true,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        generated
            .files
            .keys()
            .map(|path| path.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["Cargo.toml", "src/lib.rs", "src/main.rs"]
    );
    insta::assert_snapshot!(
        "echo_service_cargo_toml",
        generated.files[&PathBuf::from("Cargo.toml")]
    );
    insta::assert_snapshot!(
        "echo_service_main",
        generated.files[&PathBuf::from("src/main.rs")]
    );
}

#[test]
fn full_facade_depends_on_every_facade_feature_whatever_the_spec_needs() {
    let manifest = |name: &str| {
        let generated = grauwolf_generator::generate_crate_with(
            &example(name),
            &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
            &[],
            &grauwolf_generator::GenerateOptions {
                service: true,
                full_facade: true,
                ..Default::default()
            },
        )
        .unwrap();
        generated.files[&PathBuf::from("Cargo.toml")].clone()
    };

    let echo = manifest("echo.json");
    let facade = |manifest: &str| {
        manifest
            .lines()
            .find(|line| line.starts_with("grauwolf = "))
            .unwrap()
            .to_owned()
    };
    // Same facade line for a spec with other needs (here: built-in tools).
    assert_eq!(facade(&echo), facade(&manifest("notes_reader.json")));
    insta::assert_snapshot!("echo_full_facade_cargo_toml", echo);
}

fn dockerfile_files(spec: &AgentSpec) -> std::collections::BTreeMap<PathBuf, String> {
    grauwolf_generator::generate_crate_with(
        spec,
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        &grauwolf_generator::GenerateOptions {
            service: true,
            full_facade: true,
            dockerfile: Some(grauwolf_generator::DockerfileOptions {
                builder_image: "ghcr.io/agentsw0rk/grauwolf-builder:1.2.3".into(),
                expected_builder_version: Some("1.2.3".into()),
            }),
            ..Default::default()
        },
    )
    .unwrap()
    .files
}

#[test]
fn generates_a_dockerfile_that_builds_offline_in_the_builder_image() {
    let files = dockerfile_files(&example("echo.json"));

    insta::assert_snapshot!("echo_dockerfile", files[&PathBuf::from("Dockerfile")]);
    insta::assert_snapshot!("echo_dockerignore", files[&PathBuf::from(".dockerignore")]);
}

#[test]
fn the_dockerfile_refuses_a_builder_of_another_version_in_its_first_step() {
    let files = dockerfile_files(&example("echo.json"));
    let dockerfile = &files[&PathBuf::from("Dockerfile")];

    let lines: Vec<&str> = dockerfile.lines().collect();
    let from = lines.iter().position(|l| l.starts_with("FROM ")).unwrap();
    let first_step = lines[from + 1..]
        .iter()
        .find(|l| !l.starts_with('#'))
        .unwrap();
    assert!(
        first_step.starts_with("RUN test \"$GRAUWOLF_VERSION\" = \"1.2.3\""),
        "{dockerfile}"
    );
    assert!(dockerfile.contains("crate generated by grauwolf 1.2.3, builder is $GRAUWOLF_VERSION"));
}

#[test]
fn the_dockerfile_has_no_guard_when_the_mismatch_is_allowed() {
    let files = grauwolf_generator::generate_crate_with(
        &example("echo.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        &grauwolf_generator::GenerateOptions {
            service: true,
            full_facade: true,
            dockerfile: Some(grauwolf_generator::DockerfileOptions {
                builder_image: "my/builder:9".into(),
                expected_builder_version: None,
            }),
            ..Default::default()
        },
    )
    .unwrap()
    .files;

    assert!(!files[&PathBuf::from("Dockerfile")].contains("GRAUWOLF_VERSION"));
}

/// The git tools are built into the binary (libgit2, ADR-0104): the runtime image is the
/// distroless one like for every other spec.
#[test]
fn a_spec_with_git_tools_gets_the_distroless_runtime_image_without_git() {
    let mut spec = example("echo.json");
    spec.builtin_tools = Some(
        serde_json::from_value(serde_json::json!({
            "tools": ["git_checkout"],
            "git": { "repo_url": "https://github.com/org/repo.git" }
        }))
        .unwrap(),
    );

    let files = dockerfile_files(&spec);

    let dockerfile = &files[&PathBuf::from("Dockerfile")];
    assert!(dockerfile.contains("FROM gcr.io/distroless/cc-debian12:nonroot"));
    assert!(!dockerfile.contains("git"), "{dockerfile}");
    assert!(!dockerfile.contains("apt-get"), "{dockerfile}");
}

/// A spec that imports a tool package and the tool as resolved from the registry (ADR-0107).
fn packaged() -> (AgentSpec, grauwolf_tools::ResolvedTool) {
    let mut spec = example("echo.json");
    spec.tool_packages = vec![
        serde_json::from_value(serde_json::json!({ "oci": "registry.example/test/ping:1" }))
            .unwrap(),
    ];
    let tool = grauwolf_tools::ResolvedTool {
        name: "ping".into(),
        description: "Answers pong".into(),
        args: serde_json::json!({ "type": "object" }),
        artifact: ToolArtifact {
            name: "ping".into(),
            bytes: vec![0, 97, 115, 109],
            hash: "abc".into(),
        },
        limits: Default::default(),
    };
    (spec, tool)
}

#[test]
fn embeds_the_modules_of_tool_packages_like_local_tools() {
    let (spec, tool) = packaged();

    let generated = grauwolf_generator::generate_crate_with(
        &spec,
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        &grauwolf_generator::GenerateOptions {
            package_tools: vec![tool.clone()],
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        generated.binary_files[&PathBuf::from("tools/ping.wasm")],
        tool.artifact.bytes
    );
    let lib = &generated.files[&PathBuf::from("src/lib.rs")];
    assert!(
        lib.contains("include_bytes!(\"../tools/ping.wasm\")"),
        "{lib}"
    );
    assert!(lib.contains("Answers pong"), "{lib}");
    assert!(generated.files[&PathBuf::from("Cargo.toml")].contains("grauwolf-sandbox"));
    // The crate carries no reference to the registry: the image build stays offline.
    assert!(!lib.contains("registry.example"), "{lib}");
}

#[test]
fn embeds_package_modules_for_the_dockerfile_too() {
    let (spec, tool) = packaged();

    let generated = grauwolf_generator::generate_crate_with(
        &spec,
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        &grauwolf_generator::GenerateOptions {
            package_tools: vec![tool],
            dockerfile: Some(grauwolf_generator::DockerfileOptions {
                builder_image: "ghcr.io/agentsw0rk/grauwolf-builder:1.2.3".into(),
                expected_builder_version: Some("1.2.3".into()),
            }),
            ..Default::default()
        },
    )
    .unwrap();

    assert!(
        generated
            .binary_files
            .contains_key(&PathBuf::from("tools/ping.wasm"))
    );
    assert!(generated.files.contains_key(&PathBuf::from("Dockerfile")));
}

#[test]
fn refuses_unresolved_tool_packages() {
    let (spec, _) = packaged();

    let error = grauwolf_generator::generate_crate(
        &spec,
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
    )
    .unwrap_err();

    assert!(
        matches!(
            error,
            grauwolf_generator::GeneratorError::UnresolvedPackages
        ),
        "{error}"
    );
}

fn source_crate(name: &str) -> grauwolf_tools::ToolSourceCrate {
    grauwolf_tools::ToolSourceCrate {
        name: name.into(),
        kind: grauwolf_tools::ToolSourceKind::Tool,
        manifest: format!("[package]\nname = \"gw_tool_{name}\"\n"),
        source: format!("// source of {name}\n"),
        hash: format!("sha256:{name}"),
    }
}

fn tool_sources_crate(
    options: &grauwolf_generator::GenerateOptions,
) -> Result<grauwolf_generator::GeneratedCrate, grauwolf_generator::GeneratorError> {
    grauwolf_generator::generate_crate_with(
        &example("calculator.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        options,
    )
}

#[test]
fn writes_tool_sources_and_their_manifest_instead_of_modules() {
    let generated = tool_sources_crate(&grauwolf_generator::GenerateOptions {
        tool_sources: Some(vec![source_crate("add")]),
        ..Default::default()
    })
    .unwrap();

    assert!(generated.binary_files.is_empty());
    let files = &generated.files;
    assert_eq!(
        files[&PathBuf::from("tools/add/src/lib.rs")],
        "// source of add\n"
    );
    assert!(files.contains_key(&PathBuf::from("tools/add/Cargo.toml")));
    insta::assert_snapshot!(
        "tool_sources_manifest",
        files[&PathBuf::from("tools/tools.json")]
    );
    // The library still embeds the module that `grauwolf tools build` creates.
    assert!(files[&PathBuf::from("src/lib.rs")].contains("include_bytes!(\"../tools/add.wasm\")"));
}

#[test]
fn a_tool_without_source_is_an_error() {
    let error = tool_sources_crate(&grauwolf_generator::GenerateOptions {
        tool_sources: Some(Vec::new()),
        ..Default::default()
    })
    .unwrap_err();

    assert!(error.to_string().contains("'add'"), "{error}");
}

#[test]
fn a_build_script_names_the_command_when_modules_are_missing() {
    let generated = tool_sources_crate(&grauwolf_generator::GenerateOptions {
        tool_sources: Some(vec![source_crate("add")]),
        ..Default::default()
    })
    .unwrap();

    insta::assert_snapshot!(
        "tool_sources_build_rs",
        generated.files[&PathBuf::from("build.rs")]
    );
    assert!(!generated.files[&PathBuf::from("Cargo.toml")].contains("[build-dependencies]"));
}

#[test]
fn the_dockerfile_builds_the_tools_in_a_layer_of_their_own() {
    let generated = tool_sources_crate(&grauwolf_generator::GenerateOptions {
        tool_sources: Some(vec![source_crate("add")]),
        dockerfile: Some(grauwolf_generator::DockerfileOptions {
            builder_image: "builder:1".into(),
            expected_builder_version: Some("1".into()),
        }),
        ..Default::default()
    })
    .unwrap();

    insta::assert_snapshot!(
        "tool_sources_dockerfile",
        generated.files[&PathBuf::from("Dockerfile")]
    );
}

#[test]
fn without_tool_sources_there_is_no_manifest_and_no_build_script() {
    let generated = grauwolf_generator::generate_crate(
        &example("calculator.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[ToolArtifact {
            name: "add".into(),
            bytes: vec![0],
            hash: String::new(),
        }],
    )
    .unwrap();

    assert!(!generated.files.contains_key(&PathBuf::from("build.rs")));
    assert!(
        !generated
            .files
            .contains_key(&PathBuf::from("tools/tools.json"))
    );
}

#[test]
fn skills_extend_the_system_prompt_from_the_tools_and_are_external_tools() {
    let source = grauwolf_generator::generate_lib(&example("skill_writer.json")).unwrap();

    assert!(
        source.contains("EXTERNAL_TOOLS: &[&str] = &[\"load_skill\", \"read_skill_file\"]"),
        "{source}"
    );
    assert!(source.contains("registry.prompt_addition()"), "{source}");
    assert!(
        source.contains("prompt().extend_system(&addition)"),
        "{source}"
    );
    // Specs without skills keep the plain prompt.
    let plain = grauwolf_generator::generate_lib(&example("notes_reader.json")).unwrap();
    assert!(!plain.contains("prompt_addition"), "{plain}");
}

#[test]
fn the_embedded_spec_of_a_crate_with_skills_names_the_skills_directory() {
    let generated = grauwolf_generator::generate_crate(
        &example("skill_writer.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
    )
    .unwrap();
    let lib = &generated.files[&PathBuf::from("src/lib.rs")];
    assert!(lib.contains("SPEC_JSON"), "{lib}");
    assert!(lib.contains("skills"), "{lib}");
}

#[test]
fn the_local_skill_directories_are_companion_paths_of_the_image() {
    assert_eq!(
        grauwolf_generator::companion_paths(&example("skill_writer.json")),
        ["skills"]
    );
}

#[test]
fn every_companion_path_comes_with_the_pointer_of_its_field() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "librarian",
            "model": { "provider": "ollama", "id": "m" },
            "knowledge": {
                "sources": ["docs", "faq.md"], "index": "index.json",
                "embedding_model": { "provider": "ollama", "id": "e" }
            },
            "long_term_memory": {
                "store": "facts.json", "embedding_model": { "provider": "ollama", "id": "e" }
            },
            "builtin_tools": { "tools": ["read_file"], "root": "work" },
            "skills": [{ "git": "https://example.com/skills.git" }, { "dir": "skills" }],
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let companions: Vec<(String, String, CompanionKind)> = grauwolf_generator::companions(&spec)
        .into_iter()
        .map(|companion| (companion.pointer, companion.path, companion.kind))
        .collect();

    let expected = [
        (
            "/knowledge/sources/0",
            "docs",
            CompanionKind::KnowledgeSource,
        ),
        (
            "/knowledge/sources/1",
            "faq.md",
            CompanionKind::KnowledgeSource,
        ),
        (
            "/knowledge/index",
            "index.json",
            CompanionKind::KnowledgeIndex,
        ),
        (
            "/long_term_memory/store",
            "facts.json",
            CompanionKind::MemoryStore,
        ),
        ("/builtin_tools/root", "work", CompanionKind::FilesRoot),
        ("/skills/1/dir", "skills", CompanionKind::SkillDir),
    ]
    .map(|(pointer, path, kind)| (pointer.to_owned(), path.to_owned(), kind));
    assert_eq!(companions, expected);
    assert_eq!(
        grauwolf_generator::companion_paths(&spec),
        [
            "docs",
            "faq.md",
            "index.json",
            "facts.json",
            "work",
            "skills"
        ]
    );
}

#[test]
fn generates_agent_and_for_each_nodes_of_a_flow() {
    let source = grauwolf_generator::generate_lib(&example("topic_report.json")).unwrap();

    assert!(
        source.contains("grauwolf_agents::nodes::agent_step::<String, String>(\"researcher\")"),
        "{source}"
    );
    assert!(
        source.contains("grauwolf_agents::for_each::for_each::<"),
        "{source}"
    );
    insta::assert_snapshot!("topic_report_lib", source);
}

#[test]
fn generates_for_each_nodes_with_a_subgraph_body_and_its_scope() {
    let spec = AgentSpec::from_json(
        &serde_json::json!({
            "spec_version": 1, "name": "each_flow",
            "model": { "provider": "ollama", "id": "m" },
            "subgraphs": {
                "shout": {
                    "input": "string", "output": "string", "tools": [],
                    "graph": {
                        "nodes": [ { "id": "ask", "kind": "llm_request" } ],
                        "edges": [
                            { "from": "start", "to": "ask" },
                            { "from": "ask", "to": "finish", "when": { "kind": "on_assistant_message" } }
                        ]
                    }
                }
            },
            "graph": {
                "nodes": [ { "id": "each", "kind": "for_each", "params": {
                    "input": "string", "output": "string", "body": { "subgraph": "shout" } } } ],
                "edges": [ { "from": "start", "to": "each" }, { "from": "each", "to": "finish" } ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let source = grauwolf_generator::generate_lib(&spec).unwrap();

    assert!(source.contains("for_each::Body::Graph"), "{source}");
    assert!(
        source.contains("subgraph_shout()?.into_graph()"),
        "{source}"
    );
    assert!(source.contains("ToolSubset::new"), "{source}");
}

fn review_router_artifact(name: &str) -> ToolArtifact {
    ToolArtifact {
        name: name.into(),
        bytes: vec![0, 97, 115, 109],
        hash: String::new(),
    }
}

#[test]
fn generates_code_nodes_conditions_and_transformations_as_embedded_modules() {
    let spec = example("review_router.json");
    let artifacts: Vec<_> = spec
        .code_pieces()
        .iter()
        .map(|piece| review_router_artifact(&piece.name))
        .collect();

    let generated = grauwolf_generator::generate_crate(
        &spec,
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &artifacts,
    )
    .unwrap();

    for name in [
        "code_main_score",
        "condition_main_score_finish",
        "transform_main_score_finish",
        "transform_main_score_finish_2",
    ] {
        assert!(
            generated
                .binary_files
                .contains_key(&PathBuf::from(format!("tools/{name}.wasm"))),
            "{name}: {:?}",
            generated.binary_files.keys().collect::<Vec<_>>()
        );
    }
    assert!(generated.files[&PathBuf::from("Cargo.toml")].contains("grauwolf-sandbox"));
    insta::assert_snapshot!(
        "review_router_lib",
        generated.files[&PathBuf::from("src/lib.rs")]
    );
}

#[test]
fn code_in_a_graph_needs_its_built_module() {
    let error = grauwolf_generator::generate_crate(
        &example("review_router.json"),
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
    )
    .unwrap_err();

    assert!(error.to_string().contains("'code_main_score'"), "{error}");
}

#[test]
fn code_pieces_are_written_as_tool_sources_with_their_kind() {
    let spec = example("review_router.json");
    let sources: Vec<_> = spec
        .code_pieces()
        .iter()
        .map(|piece| {
            let mut source = source_crate(&piece.name);
            source.kind = match piece.kind {
                grauwolf_spec::CodePieceKind::Node => grauwolf_tools::ToolSourceKind::Code,
                grauwolf_spec::CodePieceKind::Condition => {
                    grauwolf_tools::ToolSourceKind::Condition
                }
                grauwolf_spec::CodePieceKind::Transform => {
                    grauwolf_tools::ToolSourceKind::Transform
                }
            };
            source
        })
        .collect();

    let generated = grauwolf_generator::generate_crate_with(
        &spec,
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        &grauwolf_generator::GenerateOptions {
            tool_sources: Some(sources),
            ..Default::default()
        },
    )
    .unwrap();

    assert!(generated.binary_files.is_empty());
    assert!(generated.files.contains_key(&PathBuf::from(
        "tools/transform_main_score_finish/src/lib.rs"
    )));
    insta::assert_snapshot!(
        "review_router_tool_sources_manifest",
        generated.files[&PathBuf::from("tools/tools.json")]
    );
}

#[test]
fn enables_the_bedrock_feature_of_the_facade_only_for_specs_that_use_it() {
    let manifest = |spec: &AgentSpec| {
        let generated = grauwolf_generator::generate_crate(
            spec,
            &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
            &[],
        )
        .unwrap();
        generated.files[&PathBuf::from("Cargo.toml")].clone()
    };
    let echo = example("echo.json");
    let mut bedrock = echo.clone();
    bedrock.model.provider = "bedrock".into();
    bedrock.model.id = "eu.anthropic.claude-3-5-sonnet-20240620-v1:0".into();

    let with = manifest(&bedrock);

    assert!(!manifest(&echo).contains("bedrock"));
    insta::assert_snapshot!("bedrock_cargo_toml", with);
}

#[test]
fn enables_the_pgvector_feature_of_the_facade_only_for_specs_with_a_postgres_store() {
    let manifest = |spec: &AgentSpec| {
        let generated = grauwolf_generator::generate_crate(
            spec,
            &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
            &[],
        )
        .unwrap();
        generated.files[&PathBuf::from("Cargo.toml")].clone()
    };
    let echo = example("echo.json");
    let mut knowledge = echo.clone();
    knowledge.knowledge = Some(
        serde_json::from_value(serde_json::json!({
            "sources": ["docs"],
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
            "store": { "kind": "postgres", "url_env": "PGVECTOR_URL" }
        }))
        .unwrap(),
    );
    let mut memory = echo.clone();
    memory.long_term_memory = Some(
        serde_json::from_value(serde_json::json!({
            "vector_store": { "kind": "postgres", "url_env": "PGVECTOR_URL" },
            "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
        }))
        .unwrap(),
    );
    let mut file_knowledge = knowledge.clone();
    file_knowledge.knowledge.as_mut().unwrap().store = None;

    assert!(!manifest(&echo).contains("pgvector"));
    assert!(!manifest(&file_knowledge).contains("pgvector"));
    insta::assert_snapshot!("pgvector_knowledge_cargo_toml", manifest(&knowledge));
    assert!(manifest(&memory).contains("\"pgvector\""));
}

#[test]
fn knowledge_as_context_only_still_takes_external_tools_and_injects_before_the_run() {
    let json = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/specs/knowledge_context.json"),
    )
    .unwrap();
    let spec = AgentSpec::from_json(&json).unwrap();

    let source = grauwolf_generator::generate_lib(&spec).unwrap();

    assert!(
        source.contains("pub const EXTERNAL_TOOLS: &[&str] = &[];"),
        "{source}"
    );
    assert!(source.contains("run_with_tools"), "{source}");
    assert!(source.contains("run.with_knowledge(knowledge)"), "{source}");
    assert!(
        source.contains("knowledge_context_of(&external, \"search_knowledge\")")
            && source.contains(
                "tool 'search_knowledge' was not provided (MCP server, knowledge, sub-agent or built-in tool)"
            ),
        "the knowledge source is found by the name of its tool: {source}"
    );
    assert!(source.contains("provisioned.tools_for("), "{source}");
}

/// ADR-0079, ADR-0116: the durable facts of a history compression reach the memory through the
/// run lifecycle – the same call as in the interpreter (whose lifecycle test checks the facts).
#[test]
fn asks_the_run_lifecycle_to_remember_compressed_facts_only_when_the_spec_does() {
    let source = |remember_compressed: bool| {
        let mut spec = example("echo.json");
        spec.long_term_memory = Some(
            serde_json::from_value(serde_json::json!({
                "store": "memory.json",
                "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
                "remember_compressed": remember_compressed
            }))
            .unwrap(),
        );
        grauwolf_generator::generate_lib(&spec).unwrap()
    };

    let asked = source(true);
    assert!(
        asked.contains("run.with_memory(Arc::clone(memory), false, false)")
            && asked.contains("let run = run.remember_compressed();"),
        "{asked}"
    );
    assert!(!source(false).contains("remember_compressed()"));
}

/// The import of `AgentEvent` must not leave an unused-import warning in generated crates:
/// it is there exactly when the code below the imports names the type.
#[test]
fn imports_agent_event_exactly_when_the_generated_code_uses_it() {
    for name in ["echo.json", "remembering_assistant.json", "calculator.json"] {
        let source = grauwolf_generator::generate_lib(&example(name)).unwrap();
        let imported = source
            .lines()
            .filter(|line| line.starts_with("use "))
            .any(|line| line.contains("AgentEvent"));
        let used = source
            .lines()
            .filter(|line| !line.starts_with("use "))
            .any(|line| line.contains("AgentEvent"));
        assert_eq!(
            imported, used,
            "{name}: import and use of AgentEvent differ"
        );
    }
}

#[test]
fn tools_of_packages_with_limits_are_loaded_with_them() {
    let (spec, mut tool) = packaged();
    tool.limits = grauwolf_tools::ToolLimits {
        fuel: Some(1000),
        time_ms: Some(50),
        ..Default::default()
    };

    let generated = grauwolf_generator::generate_crate_with(
        &spec,
        &GrauwolfDependency::Path("/opt/grauwolf/crates".into()),
        &[],
        &grauwolf_generator::GenerateOptions {
            package_tools: vec![tool],
            ..Default::default()
        },
    )
    .unwrap();

    let lib = &generated.files[&PathBuf::from("src/lib.rs")];
    let flat: String = lib.split_whitespace().collect();
    assert!(
        flat.contains(".instantiate_limited(")
            && flat.contains(
                "&grauwolf_tools::ToolLimits{memory_mib:None,fuel:Some(1000),time_ms:Some(50),}"
            ),
        "{lib}"
    );
}
