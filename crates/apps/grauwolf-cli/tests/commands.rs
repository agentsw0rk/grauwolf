//! The command logic of the `grauwolf` CLI, run in-process through the library (no binary is
//! started); the process tests in `cli.rs` cover the real binary.

use std::path::PathBuf;
use std::process::ExitCode;

use grauwolf_cli::{Captured, Io, run};

fn workspace_file(relative: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(relative)
        .canonicalize()
        .unwrap()
        .display()
        .to_string()
}

fn grauwolf(args: &[&str]) -> (ExitCode, Captured) {
    let (io, captured) = Io::captured();
    let code = run(std::iter::once("grauwolf").chain(args.iter().copied()), &io);
    (code, captured)
}

#[test]
fn a_missing_subcommand_is_a_usage_error_on_stderr() {
    let (code, output) = grauwolf(&["spec", "frobnicate"]);

    assert_eq!(code, ExitCode::from(2));
    assert_eq!(output.stdout(), "");
    assert!(
        output.stderr().contains("frobnicate"),
        "{}",
        output.stderr()
    );
}

#[test]
fn version_goes_to_stdout() {
    let (code, output) = grauwolf(&["--version"]);

    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(
        output.stdout(),
        format!("grauwolf {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn version_json_names_the_version_and_the_builder_image_of_this_cli() {
    let (code, output) = grauwolf(&["version", "--json"]);

    assert_eq!(code, ExitCode::SUCCESS);
    let report: serde_json::Value = serde_json::from_str(&output.stdout()).unwrap();
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        report,
        serde_json::json!({
            "version": version,
            "builder_image": format!("ghcr.io/agentsw0rk/grauwolf-builder:{version}"),
        })
    );
}

#[test]
fn version_without_json_prints_the_same_line_as_the_flag() {
    let (_, flag) = grauwolf(&["--version"]);
    let (code, command) = grauwolf(&["version"]);

    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(command.stdout(), flag.stdout());
}

#[test]
fn token_prints_the_token_and_the_line_for_the_users_file() {
    let (code, output) = grauwolf(&["token", "alice"]);

    assert_eq!(code, ExitCode::SUCCESS);
    let stdout = output.stdout();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(lines[1].starts_with("\"alice\": \"sha256:"), "{stdout}");
    assert!(
        !lines[1].contains(lines[0]),
        "the token itself stays out of the users line"
    );
}

#[test]
fn tools_lock_needs_tool_packages_in_the_spec() {
    let file = workspace_file("examples/specs/echo.json");

    let (code, output) = grauwolf(&["tools", "lock", &file]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(output.stdout(), "");
    assert_eq!(
        output.stderr(),
        format!("{file}: the spec has no `tool_packages` and no git `skills`\n")
    );
}

#[test]
fn tools_pull_rejects_a_malformed_reference_before_any_network_access() {
    let (code, output) = grauwolf(&["tools", "pull", "not a reference"]);

    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output.stderr().starts_with("not a reference: "),
        "{}",
        output.stderr()
    );
}

/// A fresh, empty directory of its own below `target/cli-tests/lib`.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/cli-tests/lib")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Writes `json` as a spec file in a scratch directory.
fn spec_file(name: &str, json: &str) -> String {
    let file = scratch_dir(name).join("spec.json");
    std::fs::write(&file, json).unwrap();
    file.display().to_string()
}

#[test]
fn validate_accepts_a_valid_spec() {
    let file = workspace_file("examples/specs/echo.json");

    let (code, output) = grauwolf(&["spec", "validate", &file]);

    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(output.stdout(), format!("{file}: valid\n"));
    assert_eq!(output.stderr(), "");
}

#[test]
fn validate_prints_warnings_on_stderr_and_still_succeeds() {
    let file = spec_file(
        "validate-warns",
        r#"{ "spec_version": 1, "name": "old_loop", "model": { "provider": "ollama", "id": "m" },
             "graph": {
               "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                          { "id": "execute_tool", "kind": "execute_tool" },
                          { "id": "send_result", "kind": "llm_send_tool_result" } ],
               "edges": [
                 { "from": "start", "to": "call_llm" },
                 { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                 { "from": "execute_tool", "to": "send_result" },
                 { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } },
                 { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
               ] } }"#,
    );

    let (code, output) = grauwolf(&["spec", "validate", &file]);

    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(output.stdout(), format!("{file}: valid\n"));
    assert!(
        output.stderr().contains(&format!(
            "warning: {file}:/graph/nodes/0: answers with several tool calls"
        )),
        "{}",
        output.stderr()
    );
}

/// A spec in its own directory with the sub-agents `subs` (name → file); returns its path.
fn tree_spec(dir: &std::path::Path, file: &str, subs: &[(&str, &str)]) -> String {
    let tools: serde_json::Map<String, serde_json::Value> = subs
        .iter()
        .map(|(name, sub)| {
            (
                (*name).to_owned(),
                serde_json::json!({ "spec": sub, "description": "d" }),
            )
        })
        .collect();
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::json!({
            "spec_version": 1, "name": "n", "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }, "agent_tools": tools
        })
        .to_string(),
    )
    .unwrap();
    path.display().to_string()
}

#[test]
fn validate_refuses_cycles_and_sub_specs_outside_the_directory_like_run_does() {
    let dir = grauwolf_test::TestDir::new("validate-tree");
    let a = tree_spec(&dir, "a.json", &[("b", "b.json")]);
    tree_spec(&dir, "b.json", &[("a", "a.json")]);
    tree_spec(&dir, "elsewhere/o.json", &[]);
    let outer = tree_spec(&dir, "app/root.json", &[("o", "../elsewhere/o.json")]);

    let (cycle, cycle_output) = grauwolf(&["spec", "validate", &a]);
    let (outside, outside_output) = grauwolf(&["spec", "validate", &outer]);

    assert_eq!(cycle, ExitCode::FAILURE);
    assert!(
        cycle_output
            .stderr()
            .contains("sub-agents form a cycle: a.json → b.json → a.json"),
        "{}",
        cycle_output.stderr()
    );
    assert_eq!(outside, ExitCode::FAILURE);
    assert!(
        outside_output
            .stderr()
            .contains("sub-agent spec outside the agent's directory"),
        "{}",
        outside_output.stderr()
    );
}

#[test]
fn validate_reports_unknown_providers_with_their_json_pointer() {
    let file = spec_file(
        "validate-provider",
        r#"{ "spec_version": 1, "name": "typo", "model": { "provider": "olama", "id": "m" },
             "strategy": { "kind": "single_run" } }"#,
    );

    let (code, output) = grauwolf(&["spec", "validate", &file]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(output.stdout(), "");
    assert!(
        output.stderr().contains(&format!(
            "{file}:/model/provider: unknown provider 'olama' (built-in: "
        )),
        "{}",
        output.stderr()
    );
}

#[test]
fn validate_accepts_the_bedrock_provider() {
    let file = spec_file(
        "validate-bedrock",
        r#"{ "spec_version": 1, "name": "b",
             "model": { "provider": "bedrock", "id": "eu.anthropic.claude-3-5-sonnet-20240620-v1:0" },
             "strategy": { "kind": "single_run" } }"#,
    );

    let (code, output) = grauwolf(&["spec", "validate", &file]);

    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
}

#[test]
fn validate_reports_a_missing_file() {
    let (code, output) = grauwolf(&["spec", "validate", "/nonexistent/spec.json"]);

    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output.stderr().contains("/nonexistent/spec.json"),
        "{}",
        output.stderr()
    );
}

#[test]
fn graph_prints_a_mermaid_diagram() {
    let file = workspace_file("examples/specs/echo.json");

    let (code, output) = grauwolf(&["spec", "graph", &file]);

    assert_eq!(code, ExitCode::SUCCESS);
    assert!(
        output.stdout().starts_with("flowchart TD\n"),
        "{}",
        output.stdout()
    );
    assert!(output.stdout().contains("start --> call_llm"));
}

#[test]
fn schema_prints_the_json_schema_of_the_spec() {
    let (code, output) = grauwolf(&["spec", "schema"]);

    assert_eq!(code, ExitCode::SUCCESS);
    let schema: serde_json::Value = serde_json::from_str(&output.stdout()).unwrap();
    assert!(schema["properties"]["graph"].is_object(), "{schema}");
}

#[test]
fn build_of_a_spec_without_tools_succeeds_silently() {
    let file = workspace_file("examples/specs/echo.json");
    let cache = scratch_dir("build-cache").display().to_string();

    let (code, output) = grauwolf(&["spec", "build", &file, "--cache", &cache]);

    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    assert_eq!(output.stdout(), "");
}

#[test]
fn create_expects_provider_and_model_in_one_argument() {
    let (code, output) = grauwolf(&[
        "spec",
        "create",
        "a greeter",
        "--model",
        "nocolon",
        "--out",
        "/nonexistent/spec.json",
    ]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stderr(),
        "--model expects <provider>:<model>, e.g. ollama:qwen3.5:9b, got: nocolon\n"
    );
}

#[test]
fn generate_writes_a_crate_and_reports_where() {
    let file = workspace_file("examples/specs/echo.json");
    let out = scratch_dir("generate-out").display().to_string();

    let cache = scratch_dir("generate-cache").display().to_string();

    let (code, output) = grauwolf(&["spec", "generate", &file, "--out", &out, "--cache", &cache]);

    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    assert_eq!(output.stdout(), format!("{file}: crate written to {out}\n"));
    assert!(PathBuf::from(&out).join("Cargo.toml").is_file());
}

#[test]
fn generate_rejects_a_missing_crates_directory() {
    let file = workspace_file("examples/specs/echo.json");

    let (code, output) = grauwolf(&[
        "spec",
        "generate",
        &file,
        "--out",
        "/nonexistent/out",
        "--grauwolf-crates",
        "/nonexistent/crates",
    ]);

    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output.stderr().starts_with("/nonexistent/crates: "),
        "{}",
        output.stderr()
    );
}

#[test]
fn run_options_without_effect_abort_before_the_spec_is_loaded() {
    let (code, output) = grauwolf(&[
        "spec",
        "run",
        "/nonexistent.json",
        "hi",
        "--runs-dir",
        "runs",
    ]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stderr(),
        "no effect with spec run: --runs-dir (only with --run-id)\n"
    );
    assert_eq!(output.stdout(), "");
}

#[test]
fn resume_rejects_a_conversation() {
    let (code, output) = grauwolf(&[
        "spec",
        "resume",
        "/nonexistent.json",
        "--run-id",
        "r1",
        "--conversation",
        "c",
    ]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stderr(),
        "no effect with spec resume: --conversation\n"
    );
}

#[test]
fn run_needs_prices_for_a_cost_limit() {
    let file = workspace_file("examples/specs/echo.json");

    let (code, output) = grauwolf(&["spec", "run", &file, "hi", "--max-cost", "1.5"]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stderr(),
        "--max-cost needs --prices: without a price table the cost is unknown\n"
    );
}

#[test]
fn run_with_a_user_needs_memory_or_a_conversation() {
    let file = workspace_file("examples/specs/echo.json");

    let (code, output) = grauwolf(&["spec", "run", &file, "hi", "--user", "bob"]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stderr(),
        "--user needs a long_term_memory in the spec or --conversation\n"
    );
}

#[test]
fn chat_rejects_streaming() {
    let (code, output) = grauwolf(&["spec", "chat", "/nonexistent.json", "--stream"]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stderr(),
        "--stream and --otel are not yet available in spec chat\n"
    );
}

#[test]
fn mcp_rejects_confirm_tools_because_stdin_carries_the_protocol() {
    let (code, output) = grauwolf(&["spec", "mcp", "/nonexistent.json", "--confirm-tools", "t"]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stderr(),
        "--confirm-tools reads stdin and does not fit the stdio transport of MCP\n"
    );
}

#[test]
fn serve_and_mcp_reject_streaming() {
    for command in ["serve", "mcp"] {
        let (code, output) = grauwolf(&["spec", command, "/nonexistent.json", "--stream"]);

        assert_eq!(code, ExitCode::FAILURE);
        assert_eq!(
            output.stderr(),
            format!("no effect with spec {command}: --stream\n")
        );
    }
}

#[test]
fn serve_reports_an_unreadable_users_file_before_it_serves() {
    let file = workspace_file("examples/specs/echo.json");

    let (code, output) = grauwolf(&[
        "spec",
        "serve",
        &file,
        "--addr",
        "127.0.0.1:0",
        "--users",
        "/nonexistent/users.json",
    ]);

    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output.stderr().starts_with("/nonexistent/users.json: "),
        "{}",
        output.stderr()
    );
}

#[test]
fn validate_checks_the_types_of_agent_nodes_against_the_sub_agents() {
    let dir = scratch_dir("validate-sub-agent-types");
    std::fs::write(
        dir.join("worker.json"),
        r#"{ "spec_version": 1, "name": "worker", "model": { "provider": "ollama", "id": "m" },
             "strategy": { "kind": "single_run" } }"#,
    )
    .unwrap();
    let flow = dir.join("flow.json");
    std::fs::write(
        &flow,
        r#"{ "spec_version": 1, "name": "flow", "model": { "provider": "ollama", "id": "m" },
             "types": { "Topic": { "type": "object", "properties": { "title": { "type": "string" } } } },
             "agent_tools": { "worker": { "spec": "worker.json", "description": "Works" } },
             "graph": {
               "nodes": [ { "id": "work", "kind": "agent", "params": { "agent": "worker", "output": "Topic" } } ],
               "edges": [ { "from": "start", "to": "work" }, { "from": "work", "to": "finish" } ]
             } }"#,
    )
    .unwrap();

    let (code, output) = grauwolf(&["spec", "validate", &flow.display().to_string()]);

    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output.stderr().contains(
            "/graph/nodes/0/params/output: the agent 'worker' has the output type 'string', but the node declares 'Topic'"
        ),
        "{}",
        output.stderr()
    );
}

#[test]
fn credentials_check_reports_the_source_and_never_the_value() {
    let dir = grauwolf_test::TestDir::new("cli-credentials-check");
    std::fs::write(dir.join("key"), "sk-very-secret\n").unwrap();
    std::fs::write(
        dir.join("creds.toml"),
        "[credentials.variables]\nOPENAI_API_KEY = \"file:key\"\n",
    )
    .unwrap();
    let config = dir.join("creds.toml").display().to_string();

    let (code, output) = grauwolf(&[
        "credentials",
        "check",
        "OPENAI_API_KEY",
        "--config",
        &config,
    ]);

    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(
        output.stdout(),
        "resolved OPENAI_API_KEY from file (14 bytes)\n"
    );
    assert!(!output.stderr().contains("sk-very-secret"));

    let (code, output) = grauwolf(&[
        "credentials",
        "check",
        "GRAUWOLF_NO_SUCH",
        "--config",
        &config,
    ]);
    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output.stderr().contains("GRAUWOLF_NO_SUCH"),
        "{}",
        output.stderr()
    );
}

#[test]
fn credentials_check_takes_several_variables_or_all_and_fails_if_one_fails() {
    let dir = grauwolf_test::TestDir::new("cli-credentials-check-many");
    std::fs::write(dir.join("key"), "sk-very-secret\n").unwrap();
    std::fs::write(
        dir.join("creds.toml"),
        "[credentials.variables]\nOPENAI_API_KEY = \"file:key\"\nGIT_TOKEN = \"file:missing\"\n",
    )
    .unwrap();
    let config = dir.join("creds.toml").display().to_string();

    let (code, output) = grauwolf(&["credentials", "check", "--all", "--config", &config]);

    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(
        output.stdout(),
        "resolved OPENAI_API_KEY from file (14 bytes)\n"
    );
    assert!(output.stderr().contains("GIT_TOKEN"), "{}", output.stderr());
    assert!(!output.stderr().contains("sk-very-secret"));

    let (code, output) = grauwolf(&[
        "credentials",
        "check",
        "OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "--config",
        &config,
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(output.stdout().lines().count(), 2);

    let (code, _) = grauwolf(&["credentials", "check", "--config", &config]);
    assert_eq!(code, ExitCode::from(2), "a name or --all is required");
}

#[test]
fn identity_serve_needs_a_workload_identity() {
    if std::env::var_os("SPIFFE_ENDPOINT_SOCKET").is_some() {
        return;
    }
    let (code, output) = grauwolf(&["identity", "serve"]);
    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output.stderr().contains("SPIFFE_ENDPOINT_SOCKET"),
        "{}",
        output.stderr()
    );
}

/// Writes a spec without tools and a harness file under `harnesses/` that names it (ADR-0128).
fn harness_workspace(dir: &grauwolf_test::TestDir) -> String {
    std::fs::create_dir_all(dir.join("harnesses")).unwrap();
    std::fs::write(
        dir.join("support.json"),
        r#"{ "spec_version": 1, "name": "support", "model": { "provider": "ollama", "id": "m" },
             "strategy": { "kind": "single_run" } }"#,
    )
    .unwrap();
    let harness = dir.join("harnesses/support-bot.harness.json");
    std::fs::write(
        &harness,
        r#"{ "version": 1, "name": "support-bot", "spec": "../support.json" }"#,
    )
    .unwrap();
    harness.display().to_string()
}

#[test]
fn harness_generate_writes_the_project_into_a_folder_named_after_the_harness() {
    let dir = grauwolf_test::TestDir::new("harness-generate");
    let harness = harness_workspace(&dir);
    let build = dir.join("build");

    let (code, output) = grauwolf(&[
        "harness",
        "generate",
        &harness,
        "--out-dir",
        build.to_str().unwrap(),
    ]);

    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    let project = build.join("support-bot");
    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    assert!(
        dockerfile.contains(&format!(
            "FROM ghcr.io/agentsw0rk/grauwolf-builder:{} AS build",
            env!("CARGO_PKG_VERSION")
        )),
        "{dockerfile}"
    );
    assert!(project.join("src/main.rs").exists());
    assert!(project.join("app").is_dir());
}

#[test]
fn harness_generate_replaces_the_previous_project_completely() {
    let dir = grauwolf_test::TestDir::new("harness-regenerate");
    let harness = harness_workspace(&dir);
    let build = dir.join("build");
    let stale = build.join("support-bot/src/stale.rs");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "// left over from an older generation").unwrap();

    let (code, output) = grauwolf(&[
        "harness",
        "generate",
        &harness,
        "--out-dir",
        build.to_str().unwrap(),
    ]);

    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    assert!(!stale.exists());
    assert!(build.join("support-bot/Dockerfile").exists());
}

#[test]
fn harness_generate_marks_the_project_as_output_that_is_not_edited_by_hand() {
    let dir = grauwolf_test::TestDir::new("harness-note");
    let harness = harness_workspace(&dir);
    let build = dir.join("build");

    let (code, output) = grauwolf(&[
        "harness",
        "generate",
        &harness,
        "--out-dir",
        build.to_str().unwrap(),
    ]);

    assert_eq!(code, ExitCode::SUCCESS, "{}", output.stderr());
    let note = std::fs::read_to_string(build.join("support-bot/GENERATED.md")).unwrap();
    assert!(note.contains("support-bot.harness.json"), "{note}");
    assert!(
        note.contains("grauwolf harness generate replaces this folder completely"),
        "{note}"
    );
}

#[test]
fn harness_generate_names_the_file_of_a_harness_that_is_no_json() {
    let dir = grauwolf_test::TestDir::new("harness-no-json");
    let harness = dir.join("broken.harness.json");
    std::fs::write(&harness, "{ \"version\": 1, ").unwrap();
    let build = dir.join("build");

    let (code, output) = grauwolf(&[
        "harness",
        "generate",
        harness.to_str().unwrap(),
        "--out-dir",
        build.to_str().unwrap(),
    ]);

    assert_eq!(code, ExitCode::FAILURE);
    assert!(
        output
            .stderr()
            .starts_with(&format!("{}: invalid harness JSON: ", harness.display())),
        "{}",
        output.stderr()
    );
    assert!(!build.exists());
}
