//! The `grauwolf` command line, tested from the outside.

use grauwolf_spec::AgentSpecExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn grauwolf(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(args)
        .current_dir(workspace())
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn validate_accepts_a_valid_spec() {
    let output = grauwolf(&["spec", "validate", "examples/specs/echo.json"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "examples/specs/echo.json: valid\n");
}

/// ADR-0080: Typos in the provider and errors in sub-agents are already caught during validation;
/// a cycle between sub-agents is only traversed once.
#[test]
fn validate_checks_providers_and_sub_agents() {
    let dir = workspace().join("target/cli-tests/validate-tree");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("main.json"),
        r#"{ "spec_version": 1, "name": "main", "model": { "provider": "olama", "id": "m" },
             "agent_tools": { "helper": { "spec": "helper.json", "description": "Helps" } },
             "strategy": { "kind": "single_run" } }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("helper.json"),
        r#"{ "spec_version": 1, "name": "helper", "model": { "provider": "ollama", "id": "m" },
             "agent_tools": { "back": { "spec": "main.json", "description": "Back" } },
             "graph": { "edges": [ { "from": "start", "to": "nirgends" } ] } }"#,
    )
    .unwrap();

    let output = grauwolf(&["spec", "validate", dir.join("main.json").to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("main.json:/model/provider: unknown provider 'olama'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("helper.json:/graph/edges/0/to: unknown node 'nirgends'"),
        "{stderr}"
    );
}

/// A spec with the old single-call tool loop: valid, but with a warning.
fn single_call_spec(name: &str) -> PathBuf {
    let file = workspace().join(format!("target/cli-tests/{name}.json"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
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
    )
    .unwrap();
    file
}

#[test]
fn validate_prints_warnings_and_still_succeeds() {
    let file = single_call_spec("warns-validate");

    let output = grauwolf(&["spec", "validate", file.to_str().unwrap()]);

    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).ends_with("warns-validate.json: valid\n"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("warning: ")
            && stderr
                .contains("warns-validate.json:/graph/nodes/0: answers with several tool calls"),
        "{stderr}"
    );
}

#[test]
fn commands_that_load_a_spec_print_each_warning_once() {
    let file = single_call_spec("warns-load");

    let output = grauwolf(&[
        "spec",
        "generate",
        file.to_str().unwrap(),
        "--out",
        "target/cli-tests/warns-load-out",
    ]);

    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("warning: ").count(), 1, "{stderr}");
}

#[test]
fn validate_reports_every_error_with_its_json_pointer() {
    let file = workspace().join("target/cli-tests/broken.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        r#"{ "spec_version": 1, "name": "broken", "model": { "provider": "ollama", "id": "m" },
             "graph": { "nodes": [ { "id": "start", "kind": "llm_request" } ],
                        "edges": [ { "from": "start", "to": "nowhere" } ] } }"#,
    )
    .unwrap();

    let output = grauwolf(&["spec", "validate", file.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("broken.json:/graph/nodes/0/id: 'start' is reserved"),
        "{stderr}"
    );
    assert!(
        stderr.contains("broken.json:/graph/edges/0/to: unknown node 'nowhere'"),
        "{stderr}"
    );
}

#[test]
fn build_compiles_the_spec_tools_into_the_cache() {
    let output = grauwolf(&[
        "spec",
        "build",
        "examples/specs/calculator.json",
        "--cache",
        "target/toolbuild-tests",
    ]);

    assert!(output.status.success(), "{output:?}");
    let stdout = stdout(&output);
    let hash = stdout
        .strip_prefix("add: ")
        .and_then(|rest| rest.strip_suffix('\n'));
    let hash = hash.unwrap_or_else(|| panic!("unexpected output: {stdout}"));
    assert!(
        workspace()
            .join("target/toolbuild-tests/artifacts")
            .join(format!("{hash}.wasm"))
            .exists()
    );
}

#[test]
fn generate_writes_a_complete_crate_with_tool_artifacts() {
    let out = workspace().join("target/cli-tests/calculator");
    let _ = std::fs::remove_dir_all(&out);

    let output = grauwolf(&[
        "spec",
        "generate",
        "examples/specs/calculator.json",
        "--out",
        out.to_str().unwrap(),
        "--cache",
        "target/toolbuild-tests",
    ]);

    assert!(output.status.success(), "{output:?}");
    for file in ["Cargo.toml", "src/lib.rs", "tools/add.wasm"] {
        assert!(out.join(file).exists(), "{file} is missing");
    }
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains(
            &workspace()
                .join("crates/domain/grauwolf-agents")
                .display()
                .to_string()
        )
    );
}

#[test]
fn schema_prints_the_json_schema_of_the_spec_format() {
    let output = grauwolf(&["spec", "schema"]);

    assert!(output.status.success(), "{output:?}");
    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(schema["properties"]["graph"].is_object(), "{schema}");
}

/// ADR-0080: The schema knows the file references that `load_spec` resolves (ADR-0073).
#[test]
fn schema_knows_tool_files_and_code_files() {
    let output = grauwolf(&["spec", "schema"]);

    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tool = &schema["$defs"]["ToolSpec"];
    assert_eq!(
        schema["properties"]["tool_files"]["type"], "array",
        "{schema}"
    );
    assert_eq!(tool["properties"]["code_file"]["type"], "string", "{tool}");
    let required: Vec<&str> = tool["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(!required.contains(&"code"), "{tool}");
    assert_eq!(tool["oneOf"].as_array().map(Vec::len), Some(2), "{tool}");
}

#[test]
#[ignore = "slow: starts the binary, which builds tools or generated crates with cargo"]
fn build_does_not_pass_the_callers_environment_to_cargo() {
    let cache = workspace().join("target/cli-tests/hermetic-cache");
    let _ = std::fs::remove_dir_all(&cache);

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "build", "examples/specs/calculator.json", "--cache"])
        .arg(&cache)
        .current_dir(workspace())
        .env("RUSTFLAGS", "--this-option-does-not-exist")
        .env("GRAUWOLF_SECRET", "must-not-leak")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Environment was passed on to cargo: {output:?}"
    );
}

/// Blocking process calls need a multi-threaded runtime so the mock server keeps running.
#[tokio::test(flavor = "multi_thread")]
async fn run_executes_a_spec_against_the_provider_from_the_environment() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/api/chat"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": { "role": "assistant", "content": "Hello world" }, "done": true
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run", "examples/specs/echo.json", "Hello world"])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Hello world\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_streams_text_live_with_the_stream_flag() {
    let body = [
        serde_json::json!({ "message": { "role": "assistant", "content": "Hello " }, "done": false }),
        serde_json::json!({ "message": { "role": "assistant", "content": "world" }, "done": false }),
        serde_json::json!({ "message": { "role": "assistant", "content": "" }, "done": true, "done_reason": "stop" }),
    ]
    .map(|line| line.to_string())
    .join("\n");
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/api/chat"))
        .and(wiremock::matchers::body_partial_json(
            serde_json::json!({ "stream": true }),
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .expect(1)
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "examples/specs/streaming_echo.json",
            "Hello world",
            "--stream",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Hello world\n");
}

/// Proof for slice 4: abort in the main graph after the subgraph, then resume without
/// repeating the subgraph.
#[tokio::test(flavor = "multi_thread")]
async fn resume_continues_a_crashed_run_without_repeating_finished_work() {
    let runs = workspace().join("target/cli-tests/runs");
    let _ = std::fs::remove_dir_all(&runs);
    crash_and_resume(&["--runs-dir".into(), runs.display().to_string()]).await;
}

/// Like above, but with checkpoints in a SQLite database (F07.15, ADR-0048).
#[tokio::test(flavor = "multi_thread")]
async fn resume_continues_a_crashed_run_from_a_sql_database() {
    let database = workspace().join("target/cli-tests/runs.db");
    let _ = std::fs::remove_file(&database);
    crash_and_resume(&[
        "--runs-db".into(),
        format!("sqlite://{}", database.display()),
    ])
    .await;
    assert!(database.exists());
}

/// Aborts `draft_and_summarize.json` after the draft and continues with `store`.
async fn crash_and_resume(store: &[String]) {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    let spec = "examples/specs/draft_and_summarize.json";

    let failing = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(answer("Draft"))
        .up_to_n_times(1)
        .mount(&failing)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(400).set_body_string("broken"))
        .mount(&failing)
        .await;
    let crashed = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run", spec, "Topic", "--run-id", "cli-1"])
        .args(store)
        .current_dir(workspace())
        .env("OLLAMA_HOST", failing.uri())
        .output()
        .unwrap();
    assert!(!crashed.status.success(), "{crashed:?}");

    let working = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(answer("Summary"))
        .expect(1)
        .mount(&working)
        .await;
    let resumed = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "resume", spec, "--run-id", "cli-1"])
        .args(store)
        .current_dir(workspace())
        .env("OLLAMA_HOST", working.uri())
        .output()
        .unwrap();

    assert!(resumed.status.success(), "{resumed:?}");
    assert_eq!(stdout(&resumed), "Summary\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_writes_events_as_json_lines_to_stderr() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/api/chat"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": { "role": "assistant", "content": "Hello" }, "done": true
            })),
        )
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "examples/specs/echo.json",
            "Hello",
            "--events",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Hello\n");
    let events: Vec<String> = String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(|line| {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            event["event"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(events.first().map(String::as_str), Some("agent_starting"));
    assert_eq!(events.last().map(String::as_str), Some("agent_completed"));
    assert!(
        events.iter().any(|event| event == "llm_call_completed"),
        "{events:?}"
    );
}

#[test]
#[ignore = "slow: starts the binary, which builds tools or generated crates with cargo"]
fn build_points_compile_errors_to_the_tool_code_in_the_spec() {
    let file = workspace().join("target/cli-tests/broken_tool.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut spec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(workspace().join("examples/specs/calculator.json")).unwrap(),
    )
    .unwrap();
    spec["tools"][0]["code"] = serde_json::json!(
        "let a = args[\"a\"].as_f64().unwrap_or(0.0);\nOk((a + unknown_value).to_string())"
    );
    std::fs::write(&file, spec.to_string()).unwrap();

    let output = grauwolf(&[
        "spec",
        "build",
        file.to_str().unwrap(),
        "--cache",
        "target/toolbuild-tests",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "broken_tool.json:/tools/0/code:2:9: error: cannot find value `unknown_value`"
        ),
        "{stderr}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn create_writes_a_repaired_spec_from_a_requirement() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let echo = std::fs::read_to_string(workspace().join("examples/specs/echo.json")).unwrap();
    let answer = |text: String| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    let server = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("unknown node kind"))
        .respond_with(answer(echo.clone()))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(answer(echo.replace("\"llm_request\"", "\"ask_llm\"")))
        .with_priority(2)
        .mount(&server)
        .await;
    let out = workspace().join("target/cli-tests/created.json");
    let _ = std::fs::remove_file(&out);

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "create",
            "An agent that repeats every message.",
            "--model",
            "ollama:qwen3.5:9b",
            "--out",
        ])
        .arg(&out)
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("1 repair"), "{output:?}");
    let created =
        grauwolf_spec::AgentSpec::from_json(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(created.validate(), Ok(()));
}

#[test]
#[ignore = "slow: starts the binary, which builds tools or generated crates with cargo"]
fn generate_checks_the_generated_crate_with_cargo_on_request() {
    let out = workspace().join("target/cli-tests/echo-checked");
    let _ = std::fs::remove_dir_all(&out);

    let output = grauwolf(&[
        "spec",
        "generate",
        "examples/specs/echo.json",
        "--out",
        out.to_str().unwrap(),
        "--check",
    ]);

    assert!(output.status.success(), "{output:?}");
    // `run_app` behind the feature `app` is checked too (ADR-0084 addendum).
    assert!(
        stdout(&output).contains("cargo check (with feature app): no errors"),
        "{}",
        stdout(&output)
    );
}

#[test]
#[ignore = "slow: starts the binary, which builds tools or generated crates with cargo"]
fn build_runs_cargo_under_the_configured_wrapper() {
    let cache = "target/cli-tests/wrapper-cache";
    let _ = std::fs::remove_dir_all(workspace().join(cache));
    let calculator = workspace().join("examples/specs/calculator.json");
    // New content, so that it is not built from a cache.
    let spec = std::fs::read_to_string(&calculator)
        .unwrap()
        .replace("a missing", "a missing (wrapper)");
    let file = workspace().join("target/cli-tests/wrapped.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, spec).unwrap();
    let build = |wrapper: &[&str]| {
        let mut args = vec!["spec", "build", file.to_str().unwrap(), "--cache", cache];
        for part in wrapper {
            args.extend(["--build-wrapper", part]);
        }
        grauwolf(&args)
    };

    let refused = build(&["false"]);
    let wrapped = build(&["env", "GRAUWOLF_WRAPPED=1"]);

    assert!(!refused.status.success(), "{refused:?}");
    assert!(wrapped.status.success(), "{wrapped:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_starts_the_mcp_servers_of_the_spec() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: python3 is missing");
        return;
    }
    let server_script =
        workspace().join("crates/adapters/grauwolf-mcp/tests/fixtures/fake_mcp_server.py");
    let spec = serde_json::json!({
        "spec_version": 1, "name": "shouter",
        "model": { "provider": "ollama", "id": "m" },
        "mcp_servers": { "text": { "command": "python3", "args": [server_script], "tools": ["upper"] } },
        "graph": {
            "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                       { "id": "execute_tool", "kind": "execute_tool" },
                       { "id": "send_result", "kind": "llm_send_tool_result" } ],
            "edges": [ { "from": "start", "to": "call_llm" },
                       { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                       { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                       { "from": "execute_tool", "to": "send_result" },
                       { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
        }
    });
    let file = workspace().join("target/cli-tests/shouter.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, spec.to_string()).unwrap();

    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("HALLO"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Laut: HALLO" }, "done": true
        })))
        .with_priority(1)
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "",
                "tool_calls": [ { "function": { "name": "upper", "arguments": { "text": "hallo" } } } ] },
            "done": true
        })))
        .with_priority(2)
        .mount(&llm)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run", file.to_str().unwrap(), "Sag es laut"])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Laut: HALLO\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_continues_a_conversation_across_runs() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    let llm = MockServer::start().await;
    // Only in the second run does the answer from the first exchange appear in the history.
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Noted."))
        .respond_with(answer("You said 4711."))
        .with_priority(1)
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(answer("Noted."))
        .with_priority(2)
        .mount(&llm)
        .await;
    let conversations = workspace().join("target/cli-tests/conversations");
    let _ = std::fs::remove_dir_all(&conversations);
    let run = |input: &str| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args([
                "spec",
                "run",
                "examples/specs/echo.json",
                input,
                "--conversation",
                "anna",
            ])
            .arg("--conversations-dir")
            .arg(&conversations)
            .current_dir(workspace())
            .env("OLLAMA_HOST", llm.uri())
            .output()
            .unwrap()
    };

    let first = run("Remember 4711.");
    let second = run("What did I say?");

    assert_eq!(stdout(&first), "Noted.\n", "{first:?}");
    assert_eq!(stdout(&second), "You said 4711.\n", "{second:?}");

    // The same with a SQLite database instead of the directory (ADR-0048).
    let database = workspace().join("target/cli-tests/conversations.db");
    let _ = std::fs::remove_file(&database);
    let url = format!("sqlite://{}", database.display());
    let run_with_db = |input: &str| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args(["spec", "run", "examples/specs/echo.json", input])
            .args(["--conversation", "anna", "--conversations-db", &url])
            .current_dir(workspace())
            .env("OLLAMA_HOST", llm.uri())
            .output()
            .unwrap()
    };

    let first = run_with_db("Remember 4711.");
    let second = run_with_db("What did I say?");

    assert_eq!(stdout(&first), "Noted.\n", "{first:?}");
    assert_eq!(stdout(&second), "You said 4711.\n", "{second:?}");
    assert!(database.exists());
}

/// RAG (ADR-0038): index sources relative to the spec, offer the search tool, use the hits.
#[tokio::test(flavor = "multi_thread")]
async fn run_indexes_the_knowledge_of_a_spec_and_offers_the_search_tool() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = workspace().join("target/cli-tests/knowledge");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("notes")).unwrap();
    std::fs::write(dir.join("notes/cat.md"), "The cat is called Minka.").unwrap();
    std::fs::write(
        dir.join("librarian.json"),
        serde_json::json!({
            "spec_version": 1, "name": "librarian",
            "model": { "provider": "ollama", "id": "m" },
            "knowledge": { "sources": ["notes"], "embedding_model": { "provider": "ollama", "id": "e" } },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                           { "id": "execute_tool", "kind": "execute_tool" },
                           { "id": "send_result", "kind": "llm_send_tool_result" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                           { "from": "execute_tool", "to": "send_result" },
                           { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [[1.0, 0.0]] })),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains(
            "[notes/cat.md] The cat is called Minka.",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Her name is Minka." }, "done": true
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("search_knowledge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "search_knowledge", "arguments": { "query": "name of the cat" } } }
            ] },
            "done": true
        })))
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/knowledge/librarian.json",
            "What is the cat's name?",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Her name is Minka.\n");
}

/// Agent as tool (ADR-0040): the coordinator delegates to a sub-agent from another
/// spec file.
#[tokio::test(flavor = "multi_thread")]
async fn run_offers_sub_agents_from_other_spec_files_as_tools() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = workspace().join("target/cli-tests/agent-tools");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("team")).unwrap();
    std::fs::copy(
        workspace().join("examples/specs/echo.json"),
        dir.join("team/translator.json"),
    )
    .unwrap();
    std::fs::write(
        dir.join("coordinator.json"),
        serde_json::json!({
            "spec_version": 1, "name": "coordinator",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "translator": { "spec": "team/translator.json", "description": "Translates into English" } },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                           { "id": "execute_tool", "kind": "execute_tool" },
                           { "id": "send_result", "kind": "llm_send_tool_result" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                           { "from": "execute_tool", "to": "send_result" },
                           { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    let server = MockServer::start().await;
    // The sub-agent's result goes back to the coordinator.
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"role\":\"tool\""))
        .respond_with(answer("In English: Hello"))
        .mount(&server)
        .await;
    // The coordinator offers the tool and calls it.
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Translate into English"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "translator", "arguments": { "request": "Hello" } } }
            ] },
            "done": true
        })))
        .mount(&server)
        .await;
    // The sub-agent (without tools) translates.
    Mock::given(path("/api/chat"))
        .respond_with(answer("Hello"))
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/agent-tools/coordinator.json",
            "Translate into English: Hello",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "In English: Hello\n");
}

#[test]
fn run_reports_sub_agents_that_reference_each_other() {
    let dir = workspace().join("target/cli-tests/agent-cycle");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let spec = |name: &str, other: &str| {
        serde_json::json!({
            "spec_version": 1, "name": name,
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { other: { "spec": format!("{other}.json"), "description": "Helps" } },
            "graph": {
                "nodes": [ { "id": "call_llm", "kind": "llm_request" },
                           { "id": "execute_tool", "kind": "execute_tool" },
                           { "id": "send_result", "kind": "llm_send_tool_result" } ],
                "edges": [ { "from": "start", "to": "call_llm" },
                           { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
                           { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
                           { "from": "execute_tool", "to": "send_result" },
                           { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string()
    };
    std::fs::write(dir.join("ping.json"), spec("ping", "pong")).unwrap();
    std::fs::write(dir.join("pong.json"), spec("pong", "ping")).unwrap();

    let output = grauwolf(&[
        "spec",
        "run",
        "target/cli-tests/agent-cycle/ping.json",
        "Hello",
    ]);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sub-agents form a cycle"), "{stderr}");
    assert!(
        stderr.contains("ping.json → ") && stderr.contains("pong.json → "),
        "{stderr}"
    );
}

/// Model per subgraph (ADR-0043): the draft comes from OpenAI, the summary from Ollama.
#[tokio::test(flavor = "multi_thread")]
async fn run_uses_the_providers_of_all_subgraph_models() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = workspace().join("target/cli-tests/two-providers");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut spec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(workspace().join("examples/specs/two_models.json")).unwrap(),
    )
    .unwrap();
    spec["subgraphs"]["draft"]["model"] =
        serde_json::json!({ "provider": "openai", "id": "gpt-4o-mini" });
    std::fs::write(dir.join("spec.json"), spec.to_string()).unwrap();
    let openai = MockServer::start().await;
    Mock::given(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [ { "index": 0, "message": { "role": "assistant", "content": "Draft from OpenAI" } } ]
        })))
        .expect(1)
        .mount(&openai)
        .await;
    let ollama = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Summary from Ollama" }, "done": true
        })))
        .expect(1)
        .mount(&ollama)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/two-providers/spec.json",
            "Topic",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", ollama.uri())
        .env("OPENAI_API_KEY", "sk-test")
        .env("OPENAI_BASE_URL", openai.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Summary from Ollama\n");
}

/// Built-in tools (ADR-0046): the agent reads a file below the root of the spec.
#[tokio::test(flavor = "multi_thread")]
async fn run_offers_builtin_file_tools_below_the_spec_root() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = workspace().join("target/cli-tests/builtin-tools");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    std::fs::write(dir.join("files/todo.md"), "Buy milk").unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "filer",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": { "tools": ["read_file", "say_to_user"], "root": "files" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Buy milk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "On the list: Buy milk" }, "done": true
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"name\":\"say_to_user\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "read_file", "arguments": { "path": "todo.md" } } }
            ] },
            "done": true
        })))
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/builtin-tools/spec.json",
            "What is on the agenda?",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "On the list: Buy milk\n");
}

/// Interceptor of the CLI (ADR-0047): `--confirm-tools` asks; "n" refuses the call.
#[tokio::test(flavor = "multi_thread")]
async fn run_asks_before_confirmed_tools_and_reports_a_refusal_to_the_llm() {
    use std::io::Write;
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = workspace().join("target/cli-tests/confirm-tools");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("files")).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "writer",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": { "tools": ["write_file"], "root": "files" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("refused"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Not written." }, "done": true
        })))
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "write_file", "arguments": { "path": "a.txt", "content": "x" } } }
            ] },
            "done": true
        })))
        .mount(&server)
        .await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/confirm-tools/spec.json",
            "Write a.txt",
            "--confirm-tools",
            "write_file",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Not written.\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Run tool write_file with"));
    assert!(!dir.join("files/a.txt").exists());
}

/// OpenTelemetry (ADR-0049): `--otel` sends the spans of the run to the OTLP endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn run_exports_spans_with_the_otel_flag() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello" }, "done": true
        })))
        .mount(&llm)
        .await;
    let collector = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&collector)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run", "examples/specs/echo.json", "Hello", "--otel"])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", collector.uri())
        .env("OTEL_SERVICE_NAME", "cli-test")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let bodies: Vec<String> = collector
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let all = bodies.join("");
    assert!(all.contains("invoke_agent echo"), "{bodies:?}");
    assert!(all.contains("chat llama3.2"), "{bodies:?}");
    assert!(all.contains("cli-test"), "{bodies:?}");
}

/// MCP over Streamable HTTP (ADR-0051): remote server, credentials from the environment.
#[tokio::test(flavor = "multi_thread")]
async fn run_uses_remote_mcp_servers_with_headers_from_the_environment() {
    use wiremock::matchers::{body_partial_json, body_string_contains, header, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let rpc = |id: u64, result: serde_json::Value| {
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    };
    let mcp = MockServer::start().await;
    Mock::given(path("/mcp"))
        .and(header("authorization", "Bearer token-from-the-environment"))
        .and(body_partial_json(serde_json::json!({ "method": "initialize" })))
        .respond_with(rpc(1, serde_json::json!({
            "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "docs" }
        })))
        .mount(&mcp)
        .await;
    Mock::given(path("/mcp"))
        .and(body_partial_json(
            serde_json::json!({ "method": "notifications/initialized" }),
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(&mcp)
        .await;
    Mock::given(path("/mcp"))
        .and(body_partial_json(
            serde_json::json!({ "method": "tools/list" }),
        ))
        .respond_with(rpc(
            2,
            serde_json::json!({ "tools": [ {
            "name": "search_docs", "description": "Searches", "inputSchema": { "type": "object" }
        } ] }),
        ))
        .mount(&mcp)
        .await;
    Mock::given(path("/mcp"))
        .and(body_partial_json(
            serde_json::json!({ "method": "tools/call" }),
        ))
        .respond_with(rpc(
            3,
            serde_json::json!({ "content": [ { "type": "text", "text": "grauwolf uses MCP" } ] }),
        ))
        .mount(&mcp)
        .await;
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("grauwolf uses MCP"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "According to the docs: grauwolf uses MCP" }, "done": true
        })))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "search_docs", "arguments": { "query": "MCP" } } }
            ] },
            "done": true
        })))
        .mount(&llm)
        .await;
    let dir = workspace().join("target/cli-tests/remote-mcp");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "docs_agent",
            "model": { "provider": "ollama", "id": "m" },
            "mcp_servers": { "docs": {
                "url": format!("{}/mcp", mcp.uri()),
                "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" },
                "tools": ["search_docs"]
            } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/remote-mcp/spec.json",
            "What does grauwolf use?",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .env("DOCS_TOKEN", "token-from-the-environment")
        .output()
        .unwrap();
    let missing_token = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/remote-mcp/spec.json",
            "Hello",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .env_remove("DOCS_TOKEN")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "According to the docs: grauwolf uses MCP\n"
    );
    assert!(!missing_token.status.success());
    assert!(
        String::from_utf8_lossy(&missing_token.stderr)
            .contains("missing environment variable DOCS_TOKEN")
    );
}

#[test]
fn graph_prints_the_flow_of_a_spec_as_mermaid() {
    let output = grauwolf(&["spec", "graph", "examples/specs/react_calculator.json"]);

    assert!(output.status.success(), "{output:?}");
    let diagram = stdout(&output);
    assert!(diagram.starts_with("flowchart TD\n"), "{diagram}");
    assert!(
        diagram.contains("    execute_tool --> reflect\n"),
        "{diagram}"
    );
}

/// HTTP service (ADR-0053): `spec serve` reports its address and answers `/run`.
#[tokio::test(flavor = "multi_thread")]
async fn serve_answers_runs_over_http() {
    use std::io::{BufRead, BufReader};
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello from the service" }, "done": true
        })))
        .mount(&llm)
        .await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "serve",
            "examples/specs/echo.json",
            "--addr",
            "127.0.0.1:0",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let base = line.trim().rsplit(' ').next().unwrap().to_owned();

    let answer: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/run"))
        .json(&serde_json::json!({ "input": "Hello" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let card: serde_json::Value = reqwest::get(format!("{base}/.well-known/agent-card.json"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mcp = grauwolf_host::mcp::McpClient::connect_http(&format!("{base}/mcp"), &[])
        .await
        .unwrap();
    let mcp_answer = mcp
        .call_tool("echo", serde_json::json!({ "request": "Hello" }))
        .await
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(card["supportedInterfaces"][0]["url"], format!("{base}/a2a"));
    assert_eq!(mcp_answer, "Hello from the service");

    assert!(
        line.starts_with("grauwolf service for echo on http://127.0.0.1:"),
        "{line}"
    );
    assert_eq!(
        answer,
        serde_json::json!({ "output": "Hello from the service" })
    );
}

/// Access (ADR-0053): token from the environment; without a token, no address reachable from outside.
#[tokio::test(flavor = "multi_thread")]
async fn serve_requires_the_token_from_the_environment_and_refuses_open_addresses_without_one() {
    use std::io::{BufRead, BufReader};

    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "serve",
            "examples/specs/echo.json",
            "--addr",
            "127.0.0.1:0",
        ])
        .current_dir(workspace())
        .env("GRAUWOLF_SERVER_TOKEN", "secret-123")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let base = line.trim().rsplit(' ').next().unwrap().to_owned();
    let client = reqwest::Client::new();
    let without = client
        .post(format!("{base}/run"))
        .json(&serde_json::json!({ "input": "Hello" }))
        .send()
        .await
        .unwrap()
        .status();
    child.kill().unwrap();
    child.wait().unwrap();
    let open = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "serve",
            "examples/specs/echo.json",
            "--addr",
            "0.0.0.0:0",
        ])
        .current_dir(workspace())
        .env_remove("GRAUWOLF_SERVER_TOKEN")
        .output()
        .unwrap();

    assert_eq!(without, 401);
    assert!(!open.status.success());
    assert!(String::from_utf8_lossy(&open.stderr).contains("set a token in GRAUWOLF_SERVER_TOKEN"));
}

/// A2A in both directions (ADR-0054, ADR-0055): the coordinator commissions the translator, which
/// runs as its own service with `spec serve`, via its agent card.
#[tokio::test(flavor = "multi_thread")]
async fn run_delegates_to_a_remote_agent_over_a2a() {
    use std::io::{BufRead, BufReader};
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    let llm = MockServer::start().await;
    // Coordinator after the tool result.
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"role\":\"tool\""))
        .respond_with(answer("In English: The grey wolf"))
        .with_priority(1)
        .mount(&llm)
        .await;
    // Coordinator offering the tool `translator`.
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"name\":\"translator\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "translator", "arguments": { "request": "The grey wolf" } } }
            ] },
            "done": true
        })))
        .with_priority(2)
        .mount(&llm)
        .await;
    // Translator in the remote service.
    Mock::given(path("/api/chat"))
        .respond_with(answer("The grey wolf"))
        .with_priority(3)
        .mount(&llm)
        .await;
    let mut translator = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "serve",
            "examples/specs/translator.json",
            "--addr",
            "127.0.0.1:0",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .env("TRANSLATOR_TOKEN", "a2a-secret")
        .env("GRAUWOLF_SERVER_TOKEN", "a2a-secret")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(translator.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let base = line.trim().rsplit(' ').next().unwrap().to_owned();
    let dir = workspace().join("target/cli-tests/a2a-client");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("coordinator.json"),
        serde_json::json!({
            "spec_version": 1, "name": "coordinator",
            "model": { "provider": "ollama", "id": "m" },
            "agent_tools": { "translator": {
                "url": base,
                "headers": { "Authorization": "Bearer ${TRANSLATOR_TOKEN}" }
            } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "target/cli-tests/a2a-client/coordinator.json",
            "Translate: The grey wolf",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .env("TRANSLATOR_TOKEN", "a2a-secret")
        .output()
        .unwrap();
    translator.kill().unwrap();
    translator.wait().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "In English: The grey wolf\n");
}

/// MCP server (ADR-0056): an MCP client starts `spec mcp` as a process and calls the agent.
#[tokio::test(flavor = "multi_thread")]
async fn mcp_offers_the_agent_as_a_tool_over_stdio() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello" }, "done": true
        })))
        .mount(&llm)
        .await;
    let client = grauwolf_host::mcp::McpClient::spawn(
        "env",
        &[
            format!("OLLAMA_HOST={}", llm.uri()),
            env!("CARGO_BIN_EXE_grauwolf").to_owned(),
            "spec".into(),
            "mcp".into(),
            workspace()
                .join("examples/specs/translator.json")
                .display()
                .to_string(),
        ],
    )
    .await
    .unwrap();

    let tools = client.list_tools().await.unwrap();
    let answer = client
        .call_tool("translator", serde_json::json!({ "request": "Hello" }))
        .await
        .unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "translator");
    assert!(
        tools[0].description.contains("into English"),
        "{}",
        tools[0].description
    );
    assert_eq!(answer, "Hello");
}

/// Long-term memory per user (ADR-0059): `--user` selects the user's area.
#[tokio::test(flavor = "multi_thread")]
async fn run_recalls_only_the_facts_of_the_given_user() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-memory");
    let fact = |user: &str, text: &str| {
        serde_json::json!({
            "user": user,
            "chunk": { "id": "memory", "source": "memory", "text": text },
            "embedding": [1.0, 0.0]
        })
    };
    std::fs::write(
        dir.join("memory.json"),
        serde_json::json!({
            "model": "ollama:e",
            "facts": [fact("bob", "Bob likes coffee."), fact("anna", "Anna likes tea.")]
        })
        .to_string(),
    )
    .unwrap();
    let spec = dir.join("assistant.json");
    std::fs::write(
        &spec,
        serde_json::json!({
            "spec_version": 1, "name": "assistant",
            "model": { "provider": "ollama", "id": "m" },
            "long_term_memory": { "store": "memory.json", "embedding_model": { "provider": "ollama", "id": "e" } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [[1.0, 0.0]] })),
        )
        .mount(&server)
        .await;
    let say = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Bob likes coffee."))
        .respond_with(say("Wrong user"))
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Anna likes tea."))
        .respond_with(say("You like tea."))
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "recall", "arguments": { "query": "What do I like?" } } }
            ] },
            "done": true
        })))
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(&spec)
        .args(["What do I like?", "--user", "anna"])
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "You like tea.\n");
}

/// Spec with long-term memory in `dir`, holding one fact each from Bob and Anna.
fn memory_spec_of_anna_and_bob(dir: &Path) -> PathBuf {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let fact = |user: &str, text: &str| {
        serde_json::json!({
            "user": user,
            "chunk": { "id": "memory", "source": "memory", "text": text },
            "embedding": [1.0, 0.0]
        })
    };
    std::fs::write(
        dir.join("memory.json"),
        serde_json::json!({
            "model": "ollama:e",
            "facts": [fact("bob", "Bob likes coffee."), fact("anna", "Anna likes tea.")]
        })
        .to_string(),
    )
    .unwrap();
    let spec = dir.join("assistant.json");
    std::fs::write(
        &spec,
        serde_json::json!({
            "spec_version": 1, "name": "assistant",
            "model": { "provider": "ollama", "id": "m" },
            "long_term_memory": { "store": "memory.json", "embedding_model": { "provider": "ollama", "id": "e" } },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    spec
}

/// Ollama, which first calls `recall` and then answers with the fact found: Anna's fact
/// yields "You like tea.", Bob's "Wrong user".
async fn recalling_ollama() -> wiremock::MockServer {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [[1.0, 0.0]] })),
        )
        .mount(&server)
        .await;
    let say = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Bob likes coffee."))
        .respond_with(say("Wrong user"))
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Anna likes tea."))
        .respond_with(say("You like tea."))
        .mount(&server)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "recall", "arguments": { "query": "What do I like?" } } }
            ] },
            "done": true
        })))
        .mount(&server)
        .await;
    server
}

/// Checkpoint runs with `--user` (ADR-0077 addendum) use that user's memory.
#[tokio::test(flavor = "multi_thread")]
async fn checkpointed_runs_use_the_memory_of_the_user() {
    let dir = grauwolf_test::TestDir::new("cli-memory-checkpointed");
    let spec = memory_spec_of_anna_and_bob(&dir);
    let server = recalling_ollama().await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(&spec)
        .args(["What do I like?", "--user", "anna", "--run-id", "run-anna"])
        .arg("--runs-dir")
        .arg(dir.join("runs"))
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "You like tea.\n");
}

/// `spec resume --user` (ADR-0077 addendum): the user is not stored in the checkpoint; when
/// resuming, the run uses the memory of the given user.
#[tokio::test(flavor = "multi_thread")]
async fn resume_uses_the_memory_of_the_given_user() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-memory-resumed");
    let spec = memory_spec_of_anna_and_bob(&dir);
    let failing = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(400).set_body_string("broken"))
        .mount(&failing)
        .await;
    let run = |command: &str, server: &str| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args(["spec", command])
            .arg(&spec)
            .args(if command == "run" {
                vec!["What do I like?"]
            } else {
                vec![]
            })
            .args(["--user", "anna", "--run-id", "run-anna", "--runs-dir"])
            .arg(dir.join("runs"))
            .current_dir(workspace())
            .env("OLLAMA_HOST", server)
            .output()
            .unwrap()
    };
    let crashed = run("run", &failing.uri());
    assert!(!crashed.status.success(), "{crashed:?}");
    let working = recalling_ollama().await;

    let resumed = run("resume", &working.uri());

    assert!(resumed.status.success(), "{resumed:?}");
    assert_eq!(stdout(&resumed), "You like tea.\n");
}

/// Conversations per user (ADR-0060): `--user` with `--conversation` stores the history with
/// the user.
#[tokio::test(flavor = "multi_thread")]
async fn run_keeps_the_conversation_of_a_user_with_that_user() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-conversations");
    let _ = std::fs::remove_dir_all(&dir);
    let server = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello Anna" }, "done": true
        })))
        .mount(&server)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "examples/specs/echo.json",
            "Hello",
            "--conversation",
            "c1",
        ])
        .args(["--user", "anna", "--conversations-dir"])
        .arg(&dir)
        .current_dir(workspace())
        .env("OLLAMA_HOST", server.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(dir.join("users/616e6e61/c1.json").is_file());
    assert!(!dir.join("c1.json").exists());
}

/// Approval in the service (ADR-0061): `spec serve --confirm-tools` refuses via `/run` and asks
/// via `/run/stream`.
#[tokio::test(flavor = "multi_thread")]
async fn serve_asks_for_approval_of_confirmed_tools_in_the_stream() {
    use std::io::{BufRead, BufReader};
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-approval");
    std::fs::create_dir_all(dir.join("files")).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "writer",
            "model": { "provider": "ollama", "id": "m" },
            "builtin_tools": { "tools": ["write_file"], "root": "files" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let llm = MockServer::start().await;
    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    Mock::given(path("/api/chat"))
        .and(body_string_contains("approval"))
        .respond_with(answer("Not written."))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"role\":\"tool\""))
        .respond_with(answer("Written."))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "write_file", "arguments": { "path": "a.txt", "content": "x" } } }
            ] },
            "done": true
        })))
        .mount(&llm)
        .await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "serve"])
        .arg(dir.join("spec.json"))
        .args(["--addr", "127.0.0.1:0", "--confirm-tools", "write_file"])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let base = line.trim().rsplit(' ').next().unwrap().to_owned();
    let client = reqwest::Client::new();

    let denied: serde_json::Value = client
        .post(format!("{base}/run"))
        .json(&serde_json::json!({ "input": "Write a.txt" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let written_without_approval = dir.join("files/a.txt").exists();
    let mut stream = client
        .post(format!("{base}/run/stream"))
        .json(&serde_json::json!({ "input": "Write a.txt" }))
        .send()
        .await
        .unwrap();
    let mut events = String::new();
    while !events.contains("event: output") {
        let chunk = stream.chunk().await.unwrap().expect("stream ended early");
        events.push_str(&String::from_utf8_lossy(&chunk));
        if let Some(start) = events.find("event: approval\ndata: ") {
            let data = &events[start + 22..];
            let request: serde_json::Value =
                serde_json::from_str(&data[..data.find('\n').unwrap()]).unwrap();
            client
                .post(format!(
                    "{base}/approvals/{}",
                    request["id"].as_str().unwrap()
                ))
                .json(&serde_json::json!({ "approved": true }))
                .send()
                .await
                .unwrap();
            events = events.replacen("event: approval", "event: approved", 1);
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(denied, serde_json::json!({ "output": "Not written." }));
    assert!(!written_without_approval);
    assert!(events.contains("Written."), "{events}");
    assert_eq!(
        std::fs::read_to_string(dir.join("files/a.txt")).unwrap(),
        "x"
    );
}

/// Chat (ADR-0064): `spec chat` runs every line from stdin as a run in the same conversation.
#[tokio::test(flavor = "multi_thread")]
async fn chat_answers_each_line_within_one_conversation() {
    use std::io::Write;
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let llm = MockServer::start().await;
    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Hello Anna."))
        .respond_with(answer("Your name is Anna."))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(answer("Hello Anna."))
        .mount(&llm)
        .await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "chat", "examples/specs/echo.json"])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("My name is Anna.\n\nWhat is my name?\n".as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Hello Anna.\nYour name is Anna.\n");
}

/// User tokens (ADR-0068): `grauwolf token <user>` creates a token and the line for
/// the users file; every token is new.
#[test]
fn token_creates_a_fresh_user_token_and_its_entry_for_the_users_file() {
    let create = || {
        let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args(["token", "anna"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        stdout(&output)
    };

    let first = create();
    let second = create();

    let mut lines = first.lines();
    let token = lines.next().unwrap();
    let entry = lines.next().unwrap();
    assert!(token.starts_with("gw_") && token.len() == 35, "{token}");
    assert_eq!(
        entry,
        format!(
            "\"anna\": \"sha256:{}\"",
            grauwolf_server::token_hash(token)
        )
    );
    assert_ne!(first, second);
}

/// User tokens (ADR-0068): `spec serve --users` admits users with their own token and
/// binds their runs to them.
#[tokio::test(flavor = "multi_thread")]
async fn serve_admits_users_with_their_own_token() {
    use std::io::{BufRead, BufReader};
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let token = "gw_0123456789abcdef0123456789abcdef";
    let dir = grauwolf_test::TestDir::new("cli-users");
    std::fs::write(
        dir.join("users.json"),
        format!(
            "{{ \"anna\": \"sha256:{}\" }}",
            grauwolf_server::token_hash(token)
        ),
    )
    .unwrap();
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello Anna" }, "done": true
        })))
        .mount(&llm)
        .await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "serve",
            "examples/specs/echo.json",
            "--addr",
            "127.0.0.1:0",
        ])
        .arg("--users")
        .arg(dir.join("users.json"))
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .env_remove("GRAUWOLF_SERVER_TOKEN")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let base = line.trim().rsplit(' ').next().unwrap().to_owned();
    let client = reqwest::Client::new();
    let run = |token: Option<&str>, body: serde_json::Value| {
        let request = client.post(format!("{base}/run")).json(&body);
        match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
        .send()
    };

    let anna = run(Some(token), serde_json::json!({ "input": "Hello" }))
        .await
        .unwrap();
    let as_bob = run(
        Some(token),
        serde_json::json!({ "input": "Hello", "user_id": "bob" }),
    )
    .await
    .unwrap();
    let anonymous = run(None, serde_json::json!({ "input": "Hello" }))
        .await
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(anna.status(), 200);
    assert_eq!(as_bob.status(), 403);
    assert_eq!(anonymous.status(), 401);
}

/// Automatic memory (ADR-0069): run 1 stores a fact, run 2 sees it before the
/// input – without the model calling a tool.
#[tokio::test(flavor = "multi_thread")]
async fn run_recalls_what_an_earlier_run_remembered_automatically() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-auto-memory");
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "assistant",
            "model": { "provider": "ollama", "id": "m" },
            "long_term_memory": {
                "store": "memory.json",
                "embedding_model": { "provider": "ollama", "id": "embed" },
                "auto_recall": true, "auto_remember": true
            },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let llm = MockServer::start().await;
    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    Mock::given(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [[1.0, 0.0]] })),
        )
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("durable facts"))
        .and(body_string_contains("green tea"))
        .respond_with(answer(r#"{ "facts": ["Anna likes green tea."] }"#))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("durable facts"))
        .respond_with(answer(r#"{ "facts": [] }"#))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("Anna likes green tea."))
        .respond_with(answer("You like green tea."))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(answer("Noted."))
        .mount(&llm)
        .await;
    let run = |input: &str| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args(["spec", "run"])
            .arg(dir.join("spec.json"))
            .arg(input)
            .current_dir(workspace())
            .env("OLLAMA_HOST", llm.uri())
            .output()
            .unwrap()
    };

    let first = run("I am Anna and like green tea.");
    let second = run("What do I like to drink?");

    assert!(first.status.success(), "{first:?}");
    assert_eq!(stdout(&first), "Noted.\n");
    assert!(second.status.success(), "{second:?}");
    assert_eq!(stdout(&second), "You like green tea.\n");
}

/// Memory without tools (ADR-0069): even with `--user`, the model is offered no
/// memory tools.
#[tokio::test(flavor = "multi_thread")]
async fn run_offers_no_memory_tools_when_the_spec_leaves_them_out() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-no-memory-tools");
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "assistant",
            "model": { "provider": "ollama", "id": "m" },
            "long_term_memory": {
                "store": "memory.json",
                "embedding_model": { "provider": "ollama", "id": "embed" },
                "auto_recall": true, "tools": false
            },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let llm = MockServer::start().await;
    let answer = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    Mock::given(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [[1.0, 0.0]] })),
        )
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"remember\""))
        .respond_with(answer("Tools offered."))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(answer("Without tools."))
        .mount(&llm)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(dir.join("spec.json"))
        .args(["Hello", "--user", "anna"])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Without tools.\n");
}

/// Costs (ADR-0071): `--prices` converts token usage into money.
#[tokio::test(flavor = "multi_thread")]
async fn run_reports_costs_with_a_price_table() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-prices");
    std::fs::write(
        dir.join("prices.json"),
        r#"{ "currency": "EUR", "models": { "ollama:llama3.2": { "input": 1.0, "output": 4.0 } } }"#,
    )
    .unwrap();
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello" }, "done": true,
            "prompt_eval_count": 1000, "eval_count": 500
        })))
        .mount(&llm)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            "examples/specs/echo.json",
            "Hello",
            "--events",
            "--prices",
        ])
        .arg(dir.join("prices.json"))
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let completed: serde_json::Value = String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| event["event"] == "agent_completed")
        .expect("agent_completed is missing");
    // 1000 × 1 + 500 × 4 per million tokens.
    let cost = completed["cost"].as_f64().unwrap();
    assert!((cost - 0.003).abs() < 1e-12, "{completed}");
}

/// Budget (ADR-0072): `--max-tokens` ends the run before the next call; `--max-cost`
/// needs a price table.
#[tokio::test(flavor = "multi_thread")]
async fn run_stops_when_the_budget_is_used_up() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-budget");
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "twice",
            "model": { "provider": "ollama", "id": "m" },
            "graph": {
                "nodes": [ { "id": "first", "kind": "llm_request" },
                           { "id": "again", "kind": "llm_continue" } ],
                "edges": [ { "from": "start", "to": "first" },
                           { "from": "first", "to": "again", "when": { "kind": "on_assistant_message" } },
                           { "from": "again", "to": "finish", "when": { "kind": "on_assistant_message" } } ]
            }
        })
        .to_string(),
    )
    .unwrap();
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello" }, "done": true,
            "prompt_eval_count": 10, "eval_count": 2
        })))
        .expect(1)
        .mount(&llm)
        .await;
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args(["spec", "run"])
            .arg(dir.join("spec.json"))
            .arg("Hello")
            .args(extra)
            .current_dir(workspace())
            .env("OLLAMA_HOST", llm.uri())
            .output()
            .unwrap()
    };

    let limited = run(&["--max-tokens", "1"]);
    let without_prices = run(&["--max-cost", "0.01"]);

    assert!(!limited.status.success());
    assert!(
        String::from_utf8_lossy(&limited.stderr).contains("budget"),
        "{limited:?}"
    );
    assert!(!without_prices.status.success());
    assert!(
        String::from_utf8_lossy(&without_prices.stderr).contains("--prices"),
        "{without_prices:?}"
    );
}

/// Tools directory (ADR-0074): a tool file in the directory is available as a tool on the next run.
#[tokio::test(flavor = "multi_thread")]
async fn run_offers_the_tools_of_the_tools_directory() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-tools-dir");
    std::fs::create_dir_all(dir.join("my-tools")).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "toolbox",
            "model": { "provider": "ollama", "id": "m" },
            "tools_dir": "my-tools",
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("my-tools/upper.rs"),
        "Ok(args[\"text\"].as_str().unwrap_or(\"\").to_uppercase())",
    )
    .unwrap();
    std::fs::write(
        dir.join("my-tools/upper.json"),
        serde_json::json!({
            "name": "upper", "description": "Writes text in uppercase",
            "args": { "type": "object", "properties": { "text": { "type": "string" } } },
            "code_file": "upper.rs"
        })
        .to_string(),
    )
    .unwrap();
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("WOLF"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Result: WOLF" }, "done": true
        })))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"upper\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "upper", "arguments": { "text": "wolf" } } }
            ] },
            "done": true
        })))
        .mount(&llm)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(dir.join("spec.json"))
        .args([
            "Write wolf in uppercase.",
            "--cache",
            "target/toolbuild-tests",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Result: WOLF\n");
}

/// Checkpoint runs (ADR-0077) get the tools of the tools directory, like ordinary runs.
#[tokio::test(flavor = "multi_thread")]
async fn checkpointed_runs_offer_the_tools_of_the_tools_directory() {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-tools-dir-checkpointed");
    std::fs::create_dir_all(dir.join("my-tools")).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "toolbox",
            "model": { "provider": "ollama", "id": "m" },
            "tools_dir": "my-tools",
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("my-tools/upper.rs"),
        "Ok(args[\"text\"].as_str().unwrap_or(\"\").to_uppercase())",
    )
    .unwrap();
    std::fs::write(
        dir.join("my-tools/upper.json"),
        serde_json::json!({
            "name": "upper", "description": "Writes text in uppercase",
            "args": { "type": "object", "properties": { "text": { "type": "string" } } },
            "code_file": "upper.rs"
        })
        .to_string(),
    )
    .unwrap();
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("WOLF"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Result: WOLF" }, "done": true
        })))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("\"upper\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": "upper", "arguments": { "text": "wolf" } } }
            ] },
            "done": true
        })))
        .mount(&llm)
        .await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(dir.join("spec.json"))
        .args([
            "Write wolf in uppercase.",
            "--cache",
            "target/toolbuild-tests",
            "--run-id",
            "run-1",
        ])
        .arg("--runs-dir")
        .arg(dir.join("runs"))
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Result: WOLF\n");
}

/// Self-built tools (ADR-0075): the model builds itself a tool and calls it in the same run.
#[tokio::test(flavor = "multi_thread")]
async fn run_lets_the_agent_build_and_use_its_own_tool() {
    let (dir, llm) = self_tools_setup("unconfirmed", serde_json::json!({ "confirm": false })).await;

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(dir.join("spec.json"))
        .args([
            "Build yourself a tool.",
            "--cache",
            "target/toolbuild-tests",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Result: WOLFRUDEL\n");
}

/// With `confirm` (default), `spec run` asks on stderr before the build and shows the code.
#[tokio::test(flavor = "multi_thread")]
async fn run_asks_before_building_a_tool_of_the_agent() {
    use std::io::Write as _;

    let (dir, llm) = self_tools_setup("confirmed", serde_json::json!({})).await;

    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(dir.join("spec.json"))
        .args([
            "Build yourself a tool.",
            "--cache",
            "target/toolbuild-tests",
        ])
        .current_dir(workspace())
        .env("OLLAMA_HOST", llm.uri())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"j\n").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Result: WOLFRUDEL\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("to_uppercase()"), "{stderr}");
}

/// Spec with `self_tools` and a mock LLM that builds `shout`, calls it and names the result.
async fn self_tools_setup(
    name: &str,
    self_tools: serde_json::Value,
) -> (grauwolf_test::TestDir, wiremock::MockServer) {
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new(&format!("cli-self-tools-{name}"));
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "toolmaker",
            "model": { "provider": "ollama", "id": "m" },
            "self_tools": self_tools,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let call = |name: &str, arguments: serde_json::Value| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "", "tool_calls": [
                { "function": { "name": name, "arguments": arguments } }
            ] },
            "done": true
        }))
    };
    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("WOLFRUDEL"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Result: WOLFRUDEL" }, "done": true
        })))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains("available from now on"))
        .respond_with(call("shout", serde_json::json!({ "text": "wolfrudel" })))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(call(
            "create_tool",
            serde_json::json!({
                "name": "shout", "description": "Writes text in uppercase",
                "args": { "type": "object", "properties": { "text": { "type": "string" } } },
                "code": "Ok(args[\"text\"].as_str().unwrap_or(\"\").to_uppercase())"
            }),
        ))
        .mount(&llm)
        .await;

    (dir, llm)
}

/// Attachments (ADR-0081): `spec run --attach` and `/attach` in `spec chat` send the picture along.
#[tokio::test(flavor = "multi_thread")]
async fn run_and_chat_send_attached_files_with_the_input() {
    use std::io::Write as _;
    use wiremock::matchers::{body_string_contains, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-attach");
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "seer",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(dir.join("wolf.png"), [0x89, b'P', b'N', b'G']).unwrap();
    let llm = MockServer::start().await;
    let say = |text: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": text }, "done": true
        }))
    };
    // If the picture were also attached to the second line, it would appear twice in the history.
    Mock::given(path("/api/chat"))
        .and(body_string_contains(r#""content":"And now?","images""#))
        .respond_with(say("Attached twice."))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .and(body_string_contains(r#""images":["iVBORw=="]"#))
        .respond_with(say("A wolf."))
        .mount(&llm)
        .await;
    Mock::given(path("/api/chat"))
        .respond_with(say("No picture."))
        .mount(&llm)
        .await;

    let run = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "run"])
        .arg(dir.join("spec.json"))
        .args(["What is in the picture?", "--attach"])
        .arg(dir.join("wolf.png"))
        .env("OLLAMA_HOST", llm.uri())
        .output()
        .unwrap();
    let mut chat = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args(["spec", "chat"])
        .arg(dir.join("spec.json"))
        .env("OLLAMA_HOST", llm.uri())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let lines = format!(
        "/attach {}\nWhat is this?\nAnd now?\n",
        dir.join("wolf.png").display()
    );
    chat.stdin
        .take()
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
    let chat = chat.wait_with_output().unwrap();

    assert!(run.status.success(), "{run:?}");
    assert_eq!(stdout(&run), "A wolf.\n");
    assert!(chat.status.success(), "{chat:?}");
    // The picture stays part of the conversation (on the first line); the second line has none.
    assert_eq!(stdout(&chat), "A wolf.\nA wolf.\n");
}

/// Options that a command does not use are reported instead of silently ignored – before every
/// load and every request.
#[test]
fn commands_reject_options_without_effect() {
    let spec = "examples/specs/echo.json";
    for (args, message) in [
        (
            vec!["run", spec, "Hello", "--runs-dir", "x"],
            "no effect with spec run: --runs-dir (only with --run-id)",
        ),
        (
            vec!["run", spec, "Hello", "--conversations-db", "sqlite://x.db"],
            "no effect with spec run: --conversations-db (only with --conversation)",
        ),
        (
            vec!["resume", spec, "--run-id", "r1", "--conversation", "c1"],
            "no effect with spec resume: --conversation",
        ),
        (
            vec!["chat", spec, "--runs-db", "sqlite://x.db"],
            "no effect with spec chat: --runs-db",
        ),
        (
            vec!["serve", spec, "--stream", "--conversation", "c1"],
            "no effect with spec serve: --stream, --conversation",
        ),
        (
            vec!["mcp", spec, "--conversations-dir", "x", "--runs-dir", "y"],
            "no effect with spec mcp: --conversations-dir, --runs-dir",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .arg("spec")
            .args(&args)
            .current_dir(workspace())
            .env("OLLAMA_HOST", "http://127.0.0.1:9")
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `--llm-cache <dir>` (ADR-0086): the second identical request comes from the cache, the LLM
/// is asked only once – also across processes.
#[tokio::test(flavor = "multi_thread")]
async fn run_answers_repeated_requests_from_the_llm_cache() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = grauwolf_test::TestDir::new("cli-llm-cache");
    let server = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": "Hello back" }, "done": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args([
                "spec",
                "run",
                "examples/specs/echo.json",
                "Hello",
                "--llm-cache",
            ])
            .arg(&dir)
            .current_dir(workspace())
            .env("OLLAMA_HOST", server.uri())
            .output()
            .unwrap()
    };

    let first = run();
    let second = run();

    assert!(second.status.success(), "{second:?}");
    assert_eq!(stdout(&first), "Hello back\n");
    assert_eq!(stdout(&second), "Hello back\n");
}

/// `spec validate` reports errors in fallback models and unknown providers together (ADR-0088).
#[test]
fn validate_reports_fallback_errors_together_with_unknown_providers() {
    let dir = workspace().join("target/cli-tests/fallbacks");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("broken.json");
    std::fs::write(
        &file,
        serde_json::json!({
            "spec_version": 1, "name": "broken",
            "model": {
                "provider": "anthropic", "id": "claude-haiku-5-5",
                "fallbacks": [
                    { "provider": "anthropic", "id": "claude-haiku-5-5" },
                    { "provider": "openia", "id": "gpt-4o-mini" }
                ]
            },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();

    let output = grauwolf(&["spec", "validate", file.to_str().unwrap()]);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "/model/fallbacks/0: fallback model 'anthropic:claude-haiku-5-5' is already listed"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("/model/fallbacks/1/provider: unknown provider 'openia'"),
        "{stderr}"
    );
}

/// ADR-0100: a decision node with the `system_one` decider asks the service from
/// `RKEV_BASE_URL`; without it the run fails before it starts.
#[tokio::test]
async fn run_asks_the_system_one_service_for_decisions() {
    let dir = workspace().join("target/cli-tests/system-one");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut spec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(workspace().join("examples/specs/ticket_router.json")).unwrap(),
    )
    .unwrap();
    spec["graph"]["nodes"][0]["params"]["decider"] = "system_one".into();
    let file = dir.join("router.json");
    std::fs::write(&file, spec.to_string()).unwrap();
    let run = |url: &str| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args([
                "spec",
                "run",
                file.to_str().unwrap(),
                "Great product, thanks!",
            ])
            .current_dir(workspace())
            .env("RKEV_BASE_URL", url)
            .output()
            .unwrap()
    };

    let without = run("");
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/v1/systemone"))
        .and(wiremock::matchers::header("authorization", "Bearer secret"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "kev-1",
                "answers": { "decision": { "type": "choice", "choice": "other" } }
            })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let with = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "run",
            file.to_str().unwrap(),
            "Great product, thanks!",
        ])
        .current_dir(workspace())
        .env("RKEV_BASE_URL", server.uri())
        .env("RKEV_API_KEY", "secret")
        .output()
        .unwrap();

    assert_eq!(without.status.code(), Some(1), "{without:?}");
    let stderr = String::from_utf8_lossy(&without.stderr);
    assert!(stderr.contains("RKEV_BASE_URL"), "{stderr}");
    assert!(with.status.success(), "{with:?}");
    assert_eq!(stdout(&with), "Great product, thanks!\n");
}

/// ADR-0104: Jira tools need the Atlassian credentials and git tools with `use_token` need
/// `GIT_TOKEN`; without them the run fails before it starts, naming the variable.
#[test]
fn run_fails_before_the_run_without_credentials_for_jira_and_git_tools() {
    let dir = workspace().join("target/cli-tests/atlassian-git");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, builtin: serde_json::Value| {
        let mut spec: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(workspace().join("examples/specs/notes_reader.json")).unwrap(),
        )
        .unwrap();
        spec["builtin_tools"] = builtin;
        let file = dir.join(name);
        std::fs::write(&file, spec.to_string()).unwrap();
        file
    };
    let run = |file: &Path| {
        Command::new(env!("CARGO_BIN_EXE_grauwolf"))
            .args(["spec", "run", file.to_str().unwrap(), "hello"])
            .current_dir(workspace())
            .env_remove("ATLASSIAN_EMAIL")
            .env_remove("ATLASSIAN_API_TOKEN")
            .env_remove("GIT_TOKEN")
            .output()
            .unwrap()
    };
    let jira = write(
        "jira.json",
        serde_json::json!({
            "tools": ["jira_get_issue"],
            "jira": { "base_url": "https://example.atlassian.net" }
        }),
    );
    let git = write(
        "git.json",
        serde_json::json!({
            "tools": ["git_checkout"],
            "git": { "repo_url": "https://github.com/org/repo.git", "use_token": true }
        }),
    );

    let jira = run(&jira);
    let git = run(&git);

    assert_eq!(jira.status.code(), Some(1), "{jira:?}");
    assert!(
        String::from_utf8_lossy(&jira.stderr).contains("ATLASSIAN_EMAIL"),
        "{jira:?}"
    );
    assert_eq!(git.status.code(), Some(1), "{git:?}");
    assert!(
        String::from_utf8_lossy(&git.stderr).contains("GIT_TOKEN"),
        "{git:?}"
    );
}

#[test]
#[ignore = "slow: starts the binary, which builds tools or generated crates with cargo"]
fn generate_service_writes_the_binary_and_checks_it_on_request() {
    let out = workspace().join("target/cli-tests/echo-service");
    let _ = std::fs::remove_dir_all(&out);

    let output = grauwolf(&[
        "spec",
        "generate",
        "examples/specs/echo.json",
        "--out",
        out.to_str().unwrap(),
        "--service",
        "--check",
    ]);

    assert!(output.status.success(), "{output:?}");
    assert!(out.join("src/main.rs").exists());
    assert!(
        stdout(&output).contains("cargo check (with feature service): no errors"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn generate_full_facade_depends_on_all_facade_features() {
    let out = workspace().join("target/cli-tests/echo-full-facade");
    let _ = std::fs::remove_dir_all(&out);

    let output = grauwolf(&[
        "spec",
        "generate",
        "examples/specs/echo.json",
        "--out",
        out.to_str().unwrap(),
        "--service",
        "--full-facade",
    ]);

    assert!(output.status.success(), "{output:?}");
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(!manifest.contains("default-features = false"), "{manifest}");
}

/// ADR-0096: `--dockerfile` writes what AgentWeaver builds with kaniko: the service crate, a
/// `Dockerfile` for the builder image and the files the spec reads at run time under `app/`.
#[test]
fn generate_dockerfile_writes_the_service_crate_and_the_files_the_spec_reads() {
    let dir = workspace().join("target/cli-tests/dockerfile-spec");
    let out = workspace().join("target/cli-tests/dockerfile-out");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(dir.join("notes/sub")).unwrap();
    std::fs::write(dir.join("notes/sub/a.txt"), "alpha").unwrap();
    std::fs::write(dir.join("unrelated.txt"), "not needed").unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "reader",
            "model": { "provider": "ollama", "id": "llama3.2" },
            "builtin_tools": { "tools": ["read_file"], "root": "notes" },
            "graph": {
                "nodes": [{ "id": "call_llm", "kind": "llm_request" }],
                "edges": [
                    { "from": "start", "to": "call_llm" },
                    { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let output = grauwolf(&[
        "spec",
        "generate",
        dir.join("spec.json").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dockerfile",
    ]);

    assert!(output.status.success(), "{output:?}");
    let dockerfile = std::fs::read_to_string(out.join("Dockerfile")).unwrap();
    assert!(
        dockerfile.contains(&format!(
            "FROM ghcr.io/agentsw0rk/grauwolf-builder:{} AS build",
            env!("CARGO_PKG_VERSION")
        )),
        "{dockerfile}"
    );
    assert!(out.join(".dockerignore").exists());
    assert!(out.join("src/main.rs").exists());
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(
        manifest.contains("path = \"/opt/grauwolf/crates/apps/grauwolf\""),
        "{manifest}"
    );
    assert!(!manifest.contains("default-features = false"), "{manifest}");
    assert_eq!(
        std::fs::read_to_string(out.join("app/notes/sub/a.txt")).unwrap(),
        "alpha"
    );
    assert!(!out.join("app/unrelated.txt").exists());
}

/// The chat API (ADR-0094) reads the signature and the agent names from the base directory.
#[test]
fn generate_dockerfile_copies_the_signature_and_the_agent_names() {
    let dir = workspace().join("target/cli-tests/dockerfile-signature-spec");
    let out = workspace().join("target/cli-tests/dockerfile-signature-out");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        workspace().join("examples/specs/echo.json"),
        dir.join("spec.json"),
    )
    .unwrap();
    std::fs::write(dir.join("agent-signature.json"), r#"{ "harnessId": "h1" }"#).unwrap();
    std::fs::write(dir.join("agent-names.json"), r#"{ "agents": {} }"#).unwrap();

    let output = grauwolf(&[
        "spec",
        "generate",
        dir.join("spec.json").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dockerfile",
    ]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(out.join("app/agent-signature.json")).unwrap(),
        r#"{ "harnessId": "h1" }"#
    );
    assert_eq!(
        std::fs::read_to_string(out.join("app/agent-names.json")).unwrap(),
        r#"{ "agents": {} }"#
    );
}

/// A sub-agent's spec goes into the image too, with the files it reads relative to itself.
#[test]
fn generate_dockerfile_copies_sub_agent_specs_and_their_files() {
    let dir = workspace().join("target/cli-tests/dockerfile-sub-spec");
    let out = workspace().join("target/cli-tests/dockerfile-sub-out");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(dir.join("helper/data")).unwrap();
    std::fs::write(dir.join("helper/data/b.txt"), "beta").unwrap();
    let graph = serde_json::json!({
        "nodes": [{ "id": "call_llm", "kind": "llm_request" }],
        "edges": [
            { "from": "start", "to": "call_llm" },
            { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } }
        ]
    });
    std::fs::write(
        dir.join("helper/helper.json"),
        serde_json::json!({
            "spec_version": 1, "name": "helper",
            "model": { "provider": "ollama", "id": "llama3.2" },
            "builtin_tools": { "tools": ["read_file"], "root": "data" },
            "graph": graph
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("spec.json"),
        serde_json::json!({
            "spec_version": 1, "name": "boss",
            "model": { "provider": "ollama", "id": "llama3.2" },
            "agent_tools": { "ask_helper": { "spec": "helper/helper.json", "description": "Helps" } },
            "graph": graph
        })
        .to_string(),
    )
    .unwrap();

    let output = grauwolf(&[
        "spec",
        "generate",
        dir.join("spec.json").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--dockerfile",
    ]);

    assert!(output.status.success(), "{output:?}");
    assert!(out.join("app/helper/helper.json").exists());
    assert_eq!(
        std::fs::read_to_string(out.join("app/helper/data/b.txt")).unwrap(),
        "beta"
    );
}

#[test]
fn generate_dockerfile_takes_the_builder_image_from_the_option() {
    let out = workspace().join("target/cli-tests/dockerfile-image");
    let _ = std::fs::remove_dir_all(&out);

    let output = grauwolf(&[
        "spec",
        "generate",
        "examples/specs/echo.json",
        "--out",
        out.to_str().unwrap(),
        "--dockerfile",
        "--builder-image",
        "registry.example/grauwolf-builder:ci",
    ]);

    assert!(output.status.success(), "{output:?}");
    let dockerfile = std::fs::read_to_string(out.join("Dockerfile")).unwrap();
    assert!(
        dockerfile.contains("FROM registry.example/grauwolf-builder:ci AS build"),
        "{dockerfile}"
    );
    assert!(out.join("app").is_dir());
}

/// The container image checks itself with `grauwolf --version` (ADR-0096).
#[test]
fn version_prints_the_name_and_the_version() {
    let output = grauwolf(&["--version"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        format!("grauwolf {}\n", env!("CARGO_PKG_VERSION"))
    );
}

/// Chat API (ADR-0094): `spec serve --chat-api` answers `/api/chat` as a cron job calls it and
/// reports the signature and the model with its digest on `/api/signature`; the Ollama address
/// comes from `OLLAMA_BASE_URL`.
#[tokio::test(flavor = "multi_thread")]
async fn serve_with_the_chat_api_answers_chat_and_reports_the_signature() {
    use std::io::{BufRead, BufReader};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let llm = MockServer::start().await;
    Mock::given(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "m", "message": { "role": "assistant", "content": "Hello from the chat API" },
            "done": true
        })))
        .mount(&llm)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [ { "name": "m:latest", "digest": "0123456789abcdef0123" } ]
        })))
        .mount(&llm)
        .await;
    let dir = workspace().join("target/cli-tests/chat-api");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        workspace().join("examples/specs/echo.json"),
        dir.join("spec.json"),
    )
    .unwrap();
    std::fs::write(dir.join("agent-signature.json"), r#"{ "harnessId": "h1" }"#).unwrap();
    std::fs::write(
        dir.join("agent-names.json"),
        r#"{ "agents": { "echo": "Echo Agent" } }"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "serve",
            "target/cli-tests/chat-api/spec.json",
            "--addr",
            "127.0.0.1:0",
            "--chat-api",
        ])
        .current_dir(workspace())
        .env_remove("OLLAMA_HOST")
        .env("OLLAMA_BASE_URL", llm.uri())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let base = line.trim().rsplit(' ').next().unwrap().to_owned();
    let client = reqwest::Client::new();

    let answer: serde_json::Value = client
        .post(format!("{base}/api/chat"))
        .json(&serde_json::json!({ "message": "Hello" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let signature: serde_json::Value = client
        .get(format!("{base}/api/signature"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    child.kill().unwrap();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        answer,
        serde_json::json!({ "response": "Hello from the chat API" })
    );
    assert_eq!(
        signature["signature"],
        serde_json::json!({ "harnessId": "h1" })
    );
    assert_eq!(
        signature["observations"]["models"]["Echo Agent"][0]["modelId"],
        "m@0123456789ab"
    );
}

/// ADR-0095: `--tool-sources` writes the guest crates and the manifest and needs no Rust
/// toolchain: the binary runs with an empty `PATH`.
#[test]
fn generate_tool_sources_writes_the_guest_crates_without_a_toolchain() {
    let out = workspace().join("target/cli-tests/tool-sources-out");
    let _ = std::fs::remove_dir_all(&out);

    let output = Command::new(env!("CARGO_BIN_EXE_grauwolf"))
        .args([
            "spec",
            "generate",
            "examples/specs/calculator.json",
            "--out",
        ])
        .arg(&out)
        .arg("--tool-sources")
        .env_clear()
        .current_dir(workspace())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(out.join("tools/add/Cargo.toml").is_file());
    let lib = std::fs::read_to_string(out.join("tools/add/src/lib.rs")).unwrap();
    assert!(lib.contains("fn tool(args: Value)"), "{lib}");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("tools/tools.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["tools"][0]["name"], "add");
    assert!(
        manifest["tools"][0]["sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(!out.join("tools/add.wasm").exists());
    assert!(out.join("build.rs").is_file());
}

#[test]
fn generate_tool_sources_with_the_dockerfile_builds_the_tools_in_the_image() {
    let out = workspace().join("target/cli-tests/tool-sources-docker");
    let _ = std::fs::remove_dir_all(&out);

    let output = grauwolf(&[
        "spec",
        "generate",
        "examples/specs/calculator.json",
        "--out",
        out.to_str().unwrap(),
        "--tool-sources",
        "--dockerfile",
    ]);

    assert!(output.status.success(), "{output:?}");
    let dockerfile = std::fs::read_to_string(out.join("Dockerfile")).unwrap();
    assert!(
        dockerfile.contains("RUN grauwolf tools build ."),
        "{dockerfile}"
    );
}

#[test]
fn generate_tool_sources_cannot_be_checked_before_the_modules_exist() {
    let out = workspace().join("target/cli-tests/tool-sources-check");

    let output = grauwolf(&[
        "spec",
        "generate",
        "examples/specs/calculator.json",
        "--out",
        out.to_str().unwrap(),
        "--tool-sources",
        "--check",
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("grauwolf tools build"),
        "{output:?}"
    );
}

#[test]
fn generate_tool_sources_refuses_oci_tool_packages() {
    let dir = workspace().join("target/cli-tests/tool-sources-oci");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("spec.json"),
        r#"{ "spec_version": 1, "name": "pk", "model": { "provider": "ollama", "id": "m" },
             "tool_packages": [ { "oci": "registry.example/acme/math:1.0" } ],
             "strategy": { "kind": "single_run" } }"#,
    )
    .unwrap();

    let output = grauwolf(&[
        "spec",
        "generate",
        dir.join("spec.json").to_str().unwrap(),
        "--out",
        dir.join("out").to_str().unwrap(),
        "--tool-sources",
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("OCI tool packages carry built modules"),
        "{output:?}"
    );
    assert!(!dir.join("out").exists());
}
