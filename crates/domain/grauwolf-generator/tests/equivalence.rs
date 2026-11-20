//! Equivalence interpreter ⇔ generator on the example corpus (ADR-0015, ADR-0019).
//!
//! The tools of each spec are built with grauwolf-toolbuild; both modes run them in the
//! sandbox. The generated crate is built with cargo in its own target directory and run
//! against the same mock as the interpreter.

use grauwolf_spec::AgentSpecExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use grauwolf_generator::GrauwolfDependency;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_sandbox::SandboxRuntime;
use grauwolf_spec::AgentSpec;
use grauwolf_test::MockLlmClient;
use grauwolf_toolbuild::ToolBuilder;
use grauwolf_tools::ToolArtifact;

/// Rule of the mock: if the last input contains `needle`, …
enum Rule {
    /// … answer with text.
    Answer(&'static str, &'static str),
    /// … call a tool with JSON arguments.
    CallTool(&'static str, &'static str, &'static str),
    /// … call several tools in one answer (tool, JSON arguments).
    CallTools(&'static str, &'static [(&'static str, &'static str)]),
}

/// A case: spec, input (as JSON) and rules of the mock.
struct Case {
    spec: &'static str,
    /// Expected output as JSON, if the case takes a specific path through the graph.
    expect: Option<&'static str>,
    input: &'static str,
    mock: &'static [Rule],
}

const CASES: &[Case] = &[
    // Code in graphs (ADR-0102): a `code` node, an `on_code` condition and edge transformations
    // run as embedded WASM modules in the sandbox, in both modes.
    Case {
        spec: "review_router.json",
        expect: Some(r#"{"subject":"Thank you for the praise","body":"A great product"}"#),
        input: r#""The review""#,
        mock: &[Rule::Answer("The review", "A great product")],
    },
    Case {
        spec: "review_router.json",
        expect: Some(r#"{"subject":"We will look into it (4 words)","body":"Not what I hoped"}"#),
        input: r#""The review""#,
        mock: &[Rule::Answer("The review", "Not what I hoped")],
    },
    // A decision node with the model as decider routes to a branch (ADR-0100).
    Case {
        spec: "ticket_router.json",
        expect: Some(r#""We will refund the invoice.""#),
        input: r#""My invoice shows the wrong amount.""#,
        mock: &[
            Rule::Answer("Allowed answers", r#"{ "answer": "billing" }"#),
            Rule::Answer("wrong amount", "We will refund the invoice."),
        ],
    },
    // `temperature` and `think` of a decision apply to its request only (ADR-0100).
    Case {
        spec: "ticket_router_sampling.json",
        expect: Some(r#""We will refund the invoice.""#),
        input: r#""My invoice shows the wrong amount.""#,
        mock: &[
            Rule::Answer("Allowed answers", r#"{ "answer": "billing" }"#),
            Rule::Answer("wrong amount", "We will refund the invoice."),
        ],
    },
    // `set_model` with fallback models via `switch_model` (ADR-0088).
    Case {
        spec: "switching_fallback.json",
        expect: None,
        input: r#""Hello""#,
        mock: &[Rule::Answer("Hello", "Hello")],
    },
    // Temperature and think are part of the generated model and its events (ADR-0097).
    Case {
        spec: "sampling_echo.json",
        expect: None,
        input: r#""Hello""#,
        mock: &[Rule::Answer("Hello", "Hello")],
    },
    // Fallback models are part of the generated model (ADR-0088); only the first one is asked here.
    Case {
        spec: "fallback_echo.json",
        expect: None,
        input: r#""Hello""#,
        mock: &[Rule::Answer("Hello", "Hello")],
    },
    // The system prompt alone has over 4 tokens: the trimming by tokens applies (ADR-0087).
    Case {
        spec: "token_trimmer.json",
        expect: None,
        input: r#""Hello""#,
        mock: &[Rule::Answer("Hello", "Hello back")],
    },
    Case {
        spec: "echo.json",
        expect: None,
        input: r#""Hello world""#,
        mock: &[Rule::Answer("Hello world", "Hello world")],
    },
    Case {
        spec: "streaming_echo.json",
        expect: None,
        input: r#""Hello world""#,
        mock: &[Rule::Answer("Hello world", "Hello world")],
    },
    Case {
        spec: "calculator.json",
        expect: None,
        input: r#""What is 2+3?""#,
        mock: &[
            Rule::CallTool("2+3", "add", r#"{ "a": 2, "b": 3 }"#),
            Rule::Answer("5", "The result is 5."),
        ],
    },
    Case {
        spec: "draft_and_summarize.json",
        expect: None,
        input: r#""Topic""#,
        mock: &[
            Rule::Answer("Topic", "Draft"),
            Rule::Answer("Draft", "Summary"),
        ],
    },
    Case {
        spec: "limited_calculator.json",
        expect: Some(r#""I am not allowed to use more tools.""#),
        input: r#""What is 2+3+4?""#,
        mock: &[
            Rule::Answer("limit of 1", "I am not allowed to use more tools."),
            Rule::CallTool("2+3", "add", r#"{ "a": 2, "b": 3 }"#),
            Rule::CallTool("5", "add", r#"{ "a": 5, "b": 4 }"#),
        ],
    },
    Case {
        spec: "chat_assistant.json",
        expect: Some(r#""Hello! How are you?""#),
        input: r#""Hello""#,
        mock: &[Rule::Answer("Hello", "Hello! How are you?")],
    },
    Case {
        spec: "compact_calculator.json",
        expect: None,
        input: r#""What is 2+3?""#,
        mock: &[
            Rule::CallTool("2+3", "add", r#"{ "a": 2, "b": 3 }"#),
            Rule::Answer("5", "The result is 5."),
        ],
    },
    Case {
        spec: "subgraph_calculator.json",
        expect: None,
        input: r#""What is 2+3?""#,
        mock: &[
            Rule::CallTool("2+3", "add", r#"{ "a": 2, "b": 3 }"#),
            Rule::Answer("5", "The result is 5."),
        ],
    },
    Case {
        spec: "parallel_calculator.json",
        expect: None,
        input: r#""What is 2+3 and 4+5?""#,
        mock: &[
            Rule::CallTools(
                "2+3",
                &[
                    ("add", r#"{ "a": 2, "b": 3 }"#),
                    ("add", r#"{ "a": 4, "b": 5 }"#),
                ],
            ),
            Rule::Answer("9", "5 and 9"),
        ],
    },
    Case {
        spec: "triage.json",
        expect: None,
        input: r#""The Login button does not respond.""#,
        // First answer without the required field `tags`; the error message leads to a valid
        // answer.
        mock: &[
            Rule::Answer(
                "missing required field",
                "```json\n{ \"title\": \"Login button not working\", \"priority\": 1, \"tags\": [\"ui\"], \"estimate\": 2, \"note\": null, \"extra\": true }\n```",
            ),
            Rule::Answer(
                "Login",
                r#"{ "title": "Login button not working", "priority": 1 }"#,
            ),
        ],
    },
    Case {
        spec: "react_calculator.json",
        expect: Some(r#""The result is 5.""#),
        input: r#""What is 2+3?""#,
        mock: &[
            Rule::Answer("result", "The result is 5."),
            Rule::Answer("2+3?", "I add 2 and 3."),
            Rule::CallTool("I add", "add", r#"{ "a": 2, "b": 3 }"#),
            Rule::Answer("5", "The result is fixed: 5."),
        ],
    },
    // First answer wrong, the check rejects it, the feedback leads to the right answer.
    Case {
        spec: "verified_calculator.json",
        expect: Some(r#""It is 5.""#),
        input: r#""What is 2+3?""#,
        mock: &[
            Rule::Answer(
                "Answer:\nIt is 5.",
                r#"{ "correct": true, "feedback": "" }"#,
            ),
            Rule::Answer(
                "Answer:\nIt is 4.",
                r#"{ "correct": false, "feedback": "Recalculate: 2+3 is 5." }"#,
            ),
            Rule::Answer("Recalculate", "It is 5."),
            Rule::CallTool("2+3?", "add", r#"{ "a": 2, "b": 3 }"#),
            Rule::Answer("5", "It is 4."),
        ],
    },
    // Two calls in one answer under the sequential loop run one after another; the tool call
    // limit of 1 applies per call, so the second call gets the limit message.
    Case {
        spec: "limited_calculator.json",
        expect: Some(r#""I am not allowed to use more tools.""#),
        input: r#""What is 2+3 and 4+5?""#,
        mock: &[
            Rule::Answer("limit of 1", "I am not allowed to use more tools."),
            Rule::CallTools(
                "2+3",
                &[
                    ("add", r#"{ "a": 2, "b": 3 }"#),
                    ("add", r#"{ "a": 4, "b": 5 }"#),
                ],
            ),
        ],
    },
    // `react` thinks over all results of an answer with several calls.
    Case {
        spec: "react_calculator.json",
        expect: Some(r#""The result is 5 and 9.""#),
        input: r#""What is 2+3 and 4+5?""#,
        mock: &[
            Rule::Answer("result", "The result is 5 and 9."),
            Rule::Answer("4+5?", "I add both."),
            Rule::CallTools(
                "I add",
                &[
                    ("add", r#"{ "a": 2, "b": 3 }"#),
                    ("add", r#"{ "a": 4, "b": 5 }"#),
                ],
            ),
            Rule::Answer("9", "The result is fixed: 5 and 9."),
        ],
    },
    // `verified` checks the answer that follows several calls in one answer.
    Case {
        spec: "verified_calculator.json",
        expect: Some(r#""It is 5 and 9.""#),
        input: r#""What is 2+3 and 4+5?""#,
        mock: &[
            Rule::Answer(
                "Answer:\nIt is 5 and 9.",
                r#"{ "correct": true, "feedback": "" }"#,
            ),
            Rule::CallTools(
                "2+3",
                &[
                    ("add", r#"{ "a": 2, "b": 3 }"#),
                    ("add", r#"{ "a": 4, "b": 5 }"#),
                ],
            ),
            Rule::Answer("9", "It is 5 and 9."),
        ],
    },
    Case {
        spec: "two_models.json",
        expect: None,
        mock: &[
            Rule::Answer("Topic", "Draft"),
            Rule::Answer("Draft", "Summary"),
        ],
        input: r#""Topic""#,
    },
    Case {
        spec: "remember_task.json",
        expect: Some(r#""Final answer""#),
        mock: &[
            Rule::Answer("Thoughts on the topic\nTopic", "Final answer"),
            Rule::Answer("Topic", "Thoughts on the topic"),
        ],
        input: r#""Topic""#,
    },
    Case {
        spec: "perspectives.json",
        expect: Some(r#""Weighed up: opportunity and risk""#),
        mock: &[
            Rule::Answer(
                "## optimist\nOpportunity\n\n## pessimist\nRisk",
                "Weighed up: opportunity and risk",
            ),
            Rule::Answer("optimistic perspective", "Opportunity"),
            Rule::Answer("pessimistic perspective", "Risk"),
        ],
        input: r#""Is Rust worth it?""#,
    },
    // External tools (ADR-0050): built-in `read_file` under `examples/workspace`.
    Case {
        spec: "notes_reader.json",
        expect: Some(r#""The notes say grauwolf.""#),
        mock: &[
            Rule::CallTool("notes say", "read_file", r#"{ "path": "notes.md" }"#),
            Rule::Answer("grauwolf: agent framework", "The notes say grauwolf."),
        ],
        input: r#""What do the notes say?""#,
    },
    // Skills (ADR-0103): `load_skill` over `examples/specs/skills`, with the `skill_loaded` event.
    Case {
        spec: "skill_writer.json",
        expect: Some(r#""Autumn moon rises\nOver the quiet river\nLeaves drift to the sea""#),
        mock: &[
            Rule::CallTool("autumn", "load_skill", r#"{ "name": "haiku-writer" }"#),
            Rule::Answer(
                "5-7-5",
                "Autumn moon rises\nOver the quiet river\nLeaves drift to the sea",
            ),
        ],
        input: r#""Write something about autumn.""#,
    },
    Case {
        spec: "planner.json",
        expect: Some(r#""Go to Rügen.""#),
        input: r#""Where to go on vacation?""#,
        mock: &[
            Rule::Answer(
                "Create a plan",
                r#"{ "goal": "trip", "steps": ["Choose destination", "Give answer"] }"#,
            ),
            Rule::Answer(
                "Evaluate the plan",
                r#"{ "should_continue": true, "reason": "Fits." }"#,
            ),
            Rule::Answer("step 1 of 2: Choose destination", "Rügen"),
            Rule::Answer("step 2 of 2: Give answer", "Go to Rügen."),
        ],
    },
    Case {
        spec: "goap_baker.json",
        expect: Some(r#""The bread is baked.""#),
        input: r#""Bake a loaf of bread.""#,
        mock: &[
            Rule::Answer("Action: buy_flour", "Flour bought."),
            Rule::Answer("Action: knead_dough", "Dough kneaded."),
            Rule::Answer("Action: bake_bread", "The bread is baked."),
        ],
    },
    Case {
        spec: "relay.json",
        expect: None,
        input: r#"{ "title": "Bug", "priority": 2, "tags": ["ui"], "urgent": true }"#,
        mock: &[],
    },
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn load(name: &str) -> AgentSpec {
    let json = std::fs::read_to_string(workspace_root().join("examples/specs").join(name)).unwrap();
    AgentSpec::from_json(&json).unwrap()
}

fn build_tools(spec: &AgentSpec) -> Vec<ToolArtifact> {
    let builder = ToolBuilder::new(workspace_root().join("target/toolbuild-tests"));
    builder.prepare().unwrap();
    spec.tools
        .iter()
        .map(|tool| builder.build(tool).unwrap())
        .chain(
            spec.code_pieces()
                .iter()
                .map(|piece| builder.build_code(piece).unwrap()),
        )
        .collect()
}

fn mock(rules: &[Rule]) -> MockLlmClient {
    rules
        .iter()
        .fold(MockLlmClient::new("?"), |mock, rule| match rule {
            Rule::Answer(needle, answer) => mock.answer_when_contains(*needle, *answer),
            Rule::CallTool(needle, tool, args) => {
                mock.call_tool_when_contains(*needle, *tool, serde_json::from_str(args).unwrap())
            }
            Rule::CallTools(needle, calls) => mock.call_tools_when_contains(
                *needle,
                calls
                    .iter()
                    .map(|(tool, args)| (*tool, serde_json::from_str(args).unwrap())),
            ),
        })
}

/// The same mock as a Rust expression for the test in the generated crate.
fn mock_source(rules: &[Rule]) -> String {
    let calls: String = rules
        .iter()
        .map(|rule| match rule {
            Rule::Answer(needle, answer) => format!(".answer_when_contains({needle:?}, {answer:?})"),
            Rule::CallTool(needle, tool, args) => format!(
                ".call_tool_when_contains({needle:?}, {tool:?}, serde_json::from_str({args:?}).unwrap())"
            ),
            Rule::CallTools(needle, calls) => {
                let calls: String = calls
                    .iter()
                    .map(|(tool, args)| {
                        format!("({tool:?}, serde_json::from_str::<serde_json::Value>({args:?}).unwrap()),")
                    })
                    .collect();
                format!(".call_tools_when_contains({needle:?}, [{calls}])")
            }
        })
        .collect();
    format!("grauwolf_test::MockLlmClient::new(\"?\"){calls}")
}

fn interpreter_output(
    spec: &AgentSpec,
    case: &Case,
    artifacts: &[ToolArtifact],
) -> serde_json::Value {
    let executor = PromptExecutor::builder()
        .client(spec.model.provider.clone(), Arc::new(mock(case.mock)))
        .build();
    let log = Arc::new(event_log::EventLog::default());
    let agent = Interpreter::new(Arc::new(executor))
        .with_tool_runtime(Arc::new(SandboxRuntime::new().unwrap()))
        .with_tools(external_tools(spec))
        .with_feature(log.clone())
        .load_with_artifacts(spec, artifacts)
        .unwrap();
    // Built-in file tools use tokio::fs and need a Tokio runtime.
    let output = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(agent.run_json(serde_json::from_str(case.input).unwrap()))
        .unwrap();
    serde_json::json!({ "output": output, "events": log.events() })
}

/// Root of the built-in file tools of a spec from `examples/specs`.
fn files_root(spec: &AgentSpec) -> Option<PathBuf> {
    let root = spec.builtin_tools.as_ref()?.root.as_ref()?;
    Some(workspace_root().join("examples/specs").join(root))
}

/// Built-in file tools of the spec, as the CLI provides them (ADR-0046, ADR-0050).
fn file_tools(spec: &AgentSpec) -> Vec<Arc<dyn grauwolf_tools::DynTool>> {
    match (files_root(spec), &spec.builtin_tools) {
        (Some(root), Some(builtin)) => grauwolf_builtin_tools::FileTools::new(root)
            .unwrap()
            .tools(builtin.tools.iter().map(String::as_str))
            .unwrap(),
        _ => Vec::new(),
    }
}

/// The skill roots of the spec from `examples/specs`.
fn skill_roots(spec: &AgentSpec) -> Vec<grauwolf_builtin_tools::SkillRoot> {
    spec.skills
        .iter()
        .map(|source| grauwolf_builtin_tools::SkillRoot {
            dir: workspace_root()
                .join("examples/specs")
                .join(source.dir.as_ref().unwrap()),
            names: source.names.clone(),
        })
        .collect()
}

/// File tools and skill tools of the spec, as the host provides them.
fn external_tools(spec: &AgentSpec) -> Vec<Arc<dyn grauwolf_tools::DynTool>> {
    let mut tools = file_tools(spec);
    if !spec.skills.is_empty() {
        tools.extend(
            grauwolf_builtin_tools::SkillLibrary::load(&skill_roots(spec))
                .unwrap()
                .tools(),
        );
    }
    tools
}

/// The same external tools as source code for the generated crate.
fn file_tools_source(spec: &AgentSpec) -> Option<String> {
    let mut parts = Vec::new();
    if let (Some(root), Some(builtin)) = (files_root(spec), &spec.builtin_tools) {
        parts.push(format!(
            "grauwolf_builtin_tools::FileTools::new({:?}).unwrap().tools({:?}).unwrap()",
            root.display().to_string(),
            builtin.tools
        ));
    }
    if !spec.skills.is_empty() {
        let roots: Vec<String> = skill_roots(spec)
            .iter()
            .map(|root| {
                format!(
                    "grauwolf_builtin_tools::SkillRoot {{ dir: {:?}.into(), names: {:?} }}",
                    root.dir.display().to_string(),
                    root.names
                )
            })
            .collect();
        parts.push(format!(
            "grauwolf_builtin_tools::SkillLibrary::load(&[{}]).unwrap().tools()",
            roots.join(", ")
        ));
    }
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(format!(
            "{{ let mut tools = Vec::new(); {} tools }}",
            parts
                .iter()
                .map(|part| format!("tools.extend({part});"))
                .collect::<String>()
        )),
    }
}

/// Writes the generated crate with the test dependencies of its harness to `dir`.
fn write_crate(spec: &AgentSpec, artifacts: &[ToolArtifact], dir: &Path) {
    write_crate_with(
        spec,
        artifacts,
        dir,
        &grauwolf_generator::GenerateOptions::default(),
    );
}

/// Like [`write_crate`] with `options`, e.g. the service binary.
fn write_crate_with(
    spec: &AgentSpec,
    artifacts: &[ToolArtifact],
    dir: &Path,
    options: &grauwolf_generator::GenerateOptions,
) {
    let crates = workspace_root().join("crates");
    let generated = grauwolf_generator::generate_crate_with(
        spec,
        &GrauwolfDependency::Path(crates.clone()),
        artifacts,
        options,
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let text = generated
        .files
        .iter()
        .map(|(path, content)| (path, content.as_bytes()));
    let binary = generated
        .binary_files
        .iter()
        .map(|(path, content)| (path, content.as_slice()));
    for (path, content) in text.chain(binary) {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }

    let manifest = dir.join("Cargo.toml");
    let mut content = std::fs::read_to_string(&manifest).unwrap();
    content.push_str(&format!(
        "\n[dev-dependencies]\nfutures-util = \"0.3\"\nserde_json = \"1\"\ntokio = {{ version = \"1\", features = [\"rt-multi-thread\"] }}\ngrauwolf-test = {{ path = {:?} }}\ngrauwolf-builtin-tools = {{ path = {:?} }}\n",
        crates.join("support/grauwolf-test").display().to_string(),
        crates.join("adapters/grauwolf-builtin-tools").display().to_string()
    ));
    std::fs::write(manifest, content).unwrap();
}

/// Writes `source` as an integration test of the generated crate in `dir`.
fn write_test(dir: &Path, source: &str) {
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(dir.join("tests/equivalence.rs"), source).unwrap();
}

/// Test of the generated crate that compares output and events with `expected`.
fn equivalence_test(spec: &AgentSpec, case: &Case, expected: &serde_json::Value) -> String {
    let krate = spec.name.replace('-', "_");
    let tools = file_tools_source(spec)
        .map(|tools| format!("{tools}, "))
        .or_else(|| (!spec.external_tools().is_empty()).then(|| "Vec::new(), ".to_owned()))
        .unwrap_or_default();
    format!(
        r#"use std::sync::Arc;

{event_log}

#[test]
fn matches_the_interpreter() {{
    let executor = grauwolf_llm::PromptExecutor::builder()
        .client({provider:?}, Arc::new({mock}))
        .build();
    let log = Arc::new(EventLog::default());
    let input = serde_json::from_str({input:?}).unwrap();
    let output = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on({krate}::run_observed(Arc::new(executor), input, {tools}vec![log.clone()]))
        .unwrap();
    let actual = serde_json::json!({{ "output": output, "events": log.events() }});
    let expected: serde_json::Value = serde_json::from_str({expected:?}).unwrap();
    assert_eq!(actual, expected);
}}
"#,
        event_log = include_str!("support/event_log.rs"),
        provider = spec.model.provider,
        mock = mock_source(case.mock),
        input = case.input,
        expected = expected.to_string(),
    )
}

/// Location of the generated crates: in the cargo build directory if one is set – the crates
/// need over a gigabyte and should not end up in the (possibly mounted) workspace.
fn equivalence_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
        .join("generated-equivalence")
}

/// Runs the tests of the generated crate in `dir`; `name` appears in the error message.
fn run_generated_tests(name: &str, dir: &Path) {
    run_generated_tests_with(name, dir, &[], &[]);
}

/// Like [`run_generated_tests`], with further arguments for `cargo test` and environment variables.
fn run_generated_tests_with(name: &str, dir: &Path, args: &[&str], envs: &[(&str, &str)]) {
    let output = Command::new(env!("CARGO"))
        .args(["test", "--quiet", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .args(args)
        .envs(envs.iter().copied())
        .env("CARGO_TARGET_DIR", equivalence_dir().join("target"))
        // Generated crates belong to no workspace with a profile; without full debug info
        // and incremental data the build directory stays small.
        .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{name}: generated crate deviates from the interpreter or does not build:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
fn generated_crates_behave_like_the_interpreter() {
    // `GRAUWOLF_EQUIVALENCE_ONLY=<spec file>` runs a single case while developing.
    let only = std::env::var("GRAUWOLF_EQUIVALENCE_ONLY").ok();
    for case in CASES {
        if only.as_deref().is_some_and(|only| only != case.spec) {
            continue;
        }
        let spec = load(case.spec);
        let artifacts = build_tools(&spec);
        let expected = interpreter_output(&spec, case, &artifacts);
        if let Some(expect) = case.expect {
            let expect: serde_json::Value = serde_json::from_str(expect).unwrap();
            assert_eq!(
                expected["output"], expect,
                "{}: interpreter does not produce the expected output",
                case.spec
            );
        }
        let dir = equivalence_dir().join(&spec.name);
        write_crate(&spec, &artifacts, &dir);
        write_test(&dir, &equivalence_test(&spec, case, &expected));
        run_generated_tests(case.spec, &dir);
    }
}

/// A decision node with the `system_one` decider (ADR-0100): the generated crate takes the
/// decider as an argument and routes like the interpreter.
#[test]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
fn generated_crates_route_by_the_decider_they_are_given() {
    const DECIDER: &str = r#"
struct Technical;

impl grauwolf_agents::decision::Decider for Technical {
    fn decide<'a>(
        &'a self,
        _decision: &'a grauwolf_agents::decision::Decision,
        _state: &'a serde_json::Value,
    ) -> futures_util::future::BoxFuture<'a, Result<grauwolf_agents::decision::Verdict, grauwolf_agents::decision::DecideError>> {
        Box::pin(std::future::ready(Ok(grauwolf_agents::decision::Verdict::new("technical"))))
    }
}
"#;
    let mut spec: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(workspace_root().join("examples/specs/ticket_router.json"))
            .unwrap(),
    )
    .unwrap();
    spec["name"] = "decided_router".into();
    spec["graph"]["nodes"][0]["params"]["decider"] = "system_one".into();
    let spec = AgentSpec::from_json(&spec.to_string()).unwrap();
    let case = Case {
        spec: "decided_router",
        expect: Some(r#""Restart the printer.""#),
        input: r#""The printer is broken.""#,
        mock: &[Rule::Answer("printer", "Restart the printer.")],
    };

    #[derive(Debug)]
    struct Technical;
    impl grauwolf_agents::decision::Decider for Technical {
        fn decide<'a>(
            &'a self,
            _decision: &'a grauwolf_agents::decision::Decision,
            _state: &'a serde_json::Value,
        ) -> futures_util::future::BoxFuture<
            'a,
            Result<grauwolf_agents::decision::Verdict, grauwolf_agents::decision::DecideError>,
        > {
            Box::pin(std::future::ready(Ok(
                grauwolf_agents::decision::Verdict::new("technical"),
            )))
        }
    }
    let executor = PromptExecutor::builder()
        .client(spec.model.provider.clone(), Arc::new(mock(case.mock)))
        .build();
    let log = Arc::new(event_log::EventLog::default());
    let agent = Interpreter::new(Arc::new(executor))
        .with_decider(Arc::new(Technical))
        .with_feature(log.clone())
        .load(&spec)
        .unwrap();
    let output =
        futures_executor::block_on(agent.run_json(serde_json::from_str(case.input).unwrap()))
            .unwrap();
    assert_eq!(output, serde_json::json!("Restart the printer."));
    let expected = serde_json::json!({ "output": output, "events": log.events() });

    let dir = equivalence_dir().join(&spec.name);
    write_crate(&spec, &[], &dir);
    let test = equivalence_test(&spec, &case, &expected).replace(
        "run_observed(Arc::new(executor), input, vec![log.clone()])",
        "run_observed(Arc::new(executor), input, Some(Arc::new(Technical)), vec![log.clone()])",
    );
    write_test(&dir, &format!("{test}\n{DECIDER}"));
    run_generated_tests(case.spec, &dir);

    // As an app, the crate reads the service from the environment and fails before the run
    // with a clear message when there is none.
    std::fs::write(
        dir.join("tests/app.rs"),
        r#"#[test]
fn the_app_needs_a_service_for_its_system_one_decisions() {
    let error = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(decided_router::run_app(std::path::Path::new("."), "The printer is broken.".to_owned()))
        .unwrap_err();
    assert!(error.to_string().contains("RKEV_BASE_URL"), "{error}");
}
"#,
    )
    .unwrap();
    run_generated_tests_with(
        case.spec,
        &dir,
        &["--features", "app", "--test", "app"],
        &[("RKEV_BASE_URL", "")],
    );
}

#[path = "support/event_log.rs"]
mod event_log;

/// `run_app_with` with feature `app` (ADR-0084): providers from the environment, built-in tools and
/// the long-term memory of the given user relative to `base_dir`, events to the features –
/// without wiring code of its own, against a mocked Ollama.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_apps_provide_providers_tools_and_memory_themselves() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    let reply = |message: serde_json::Value| {
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "message": message, "done": true }))
    };
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [[1.0, 0.0]] })),
        )
        .mount(&ollama)
        .await;
    // Bob's fact must not appear in Anna's prompt.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_string_contains("Bob looks after"))
        .respond_with(reply(
            serde_json::json!({ "role": "assistant", "content": "Wrong user" }),
        ))
        .mount(&ollama)
        .await;
    // The result of read_file is there: answer.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_string_contains("Feed the wolves at eight"))
        .respond_with(reply(serde_json::json!({
            "role": "assistant", "content": "Anna, the wolves get their food at eight."
        })))
        .mount(&ollama)
        .await;
    // The fact from memory is there: read the note.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_string_contains("Anna looks after the wolves"))
        .respond_with(reply(serde_json::json!({
            "role": "assistant", "content": "",
            "tool_calls": [ { "function": { "name": "read_file", "arguments": { "path": "today.txt" } } } ]
        })))
        .mount(&ollama)
        .await;

    let spec = load("app_assistant.json");
    let base = equivalence_dir().join("app_base");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("notes")).unwrap();
    std::fs::write(base.join("notes/today.txt"), "Feed the wolves at eight").unwrap();
    std::fs::write(
        base.join("memory.json"),
        serde_json::json!({
            "model": "ollama:nomic-embed-text",
            "facts": [
                {
                    "user": "bob",
                    "chunk": { "id": "f1", "source": "remember", "text": "Bob looks after the cats" },
                    "embedding": [1.0, 0.0]
                },
                {
                    "user": "anna",
                    "chunk": { "id": "f2", "source": "remember", "text": "Anna looks after the wolves" },
                    "embedding": [1.0, 0.0]
                }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let dir = equivalence_dir().join(&spec.name);
    write_crate(&spec, &[], &dir);
    write_test(
        &dir,
        &format!(
            r#"use std::sync::Arc;

{event_log}

#[test]
fn runs_as_an_app_for_a_user() {{
    let log = Arc::new(EventLog::default());
    let options = grauwolf::app::RunOptions {{
        user: Some("anna".into()),
        features: vec![log.clone()],
        ..Default::default()
    }};
    let output = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on({krate}::run_app_with(std::path::Path::new({base:?}), "What is on today?".to_owned(), grauwolf::provisioning::ProvisionOptions::new(std::sync::Arc::new(grauwolf::config::ProcessEnv)), options))
        .unwrap();
    assert_eq!(output, "Anna, the wolves get their food at eight.");
    let events = log.events();
    let kinds: Vec<&str> = events.as_array().unwrap().iter().map(|event| event["event"].as_str().unwrap()).collect();
    assert_eq!(kinds.first(), Some(&"agent_starting"));
    assert!(kinds.contains(&"memory_recalled"), "{{kinds:?}}");
    assert_eq!(kinds.last(), Some(&"agent_completed"));
}}
"#,
            event_log = include_str!("support/event_log.rs"),
            krate = spec.name,
            base = base.display().to_string(),
        ),
    );

    let uri = ollama.uri();
    tokio::task::spawn_blocking(move || {
        run_generated_tests_with(
            "app_assistant.json",
            &dir,
            &["--features", "app"],
            &[("OLLAMA_HOST", uri.as_str())],
        )
    })
    .await
    .unwrap();
}

/// ADR-0119: a generated app takes what the interpreter's runs have – the price table of
/// `GRAUWOLF_PRICES` (cost on the completion event) and a conversation that remembers the first
/// request – through `provision` once and `run_provisioned` with `RunOptions`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_apps_price_their_runs_and_keep_conversations() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    let reply = |content: &str| {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message": { "role": "assistant", "content": content }, "done": true,
            "prompt_eval_count": 1_000_000, "eval_count": 1_000_000
        }))
    };
    // Only the second request carries the name from the first one.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_string_contains("my name is Ada"))
        .and(body_string_contains("Who am I"))
        .respond_with(reply("You are Ada"))
        .mount(&ollama)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(reply("Nice to meet you"))
        .mount(&ollama)
        .await;

    let spec = load("echo.json");
    let base = equivalence_dir().join("conversation_base");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("prices.json"),
        r#"{ "currency": "USD", "models": { "ollama:llama3.2": { "input": 1.0, "output": 2.0 } } }"#,
    )
    .unwrap();
    let dir = equivalence_dir().join("echo_conversation");
    write_crate(&spec, &[], &dir);
    write_test(
        &dir,
        &format!(
            r#"use std::sync::Arc;

{event_log}

#[test]
fn prices_and_conversations_come_with_the_options() {{
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let log = Arc::new(EventLog::default());
    let store: Arc<dyn grauwolf_agents::memory::ChatHistoryStore> =
        Arc::new(grauwolf_agents::memory::MemoryChatHistoryStore::default());
    let provisioned = runtime
        .block_on({krate}::provision(
            std::path::Path::new({base:?}),
            grauwolf::provisioning::ProvisionOptions::new(Arc::new(grauwolf::config::ProcessEnv)),
        ))
        .unwrap();
    let options = || grauwolf::app::RunOptions {{
        features: vec![log.clone()],
        conversation: Some((store.clone(), "chat-1".to_owned())),
        ..Default::default()
    }};
    let first = runtime
        .block_on({krate}::run_provisioned(&provisioned, "my name is Ada".to_owned(), options()))
        .unwrap();
    let second = runtime
        .block_on({krate}::run_provisioned(&provisioned, "Who am I?".to_owned(), options()))
        .unwrap();
    assert_eq!(first, "Nice to meet you");
    assert_eq!(second, "You are Ada");
    let events = log.events();
    let cost = events
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event["event"] == "agent_completed")
        .map(|event| event["cost"].as_f64());
    assert_eq!(cost, Some(Some(3.0)));
}}
"#,
            event_log = include_str!("support/event_log.rs"),
            krate = spec.name,
            base = base.display().to_string(),
        ),
    );

    let uri = ollama.uri();
    tokio::task::spawn_blocking(move || {
        run_generated_tests_with(
            "echo.json (prices, conversation)",
            &dir,
            &["--features", "app"],
            &[
                ("OLLAMA_HOST", uri.as_str()),
                ("GRAUWOLF_PRICES", "prices.json"),
            ],
        )
    })
    .await
    .unwrap();
}

/// Sub-agents of a generated app run through the loader with the per-agent features of
/// `ProvisionOptions::with_agent_features` (ADR-0094): each reports its own model under its own name.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_apps_install_the_agent_features_in_their_sub_agents() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    let reply = |model: &str, message: serde_json::Value| {
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "model": model, "message": message, "done": true }))
    };
    // The translator (sub-agent) answers.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_string_contains("You translate the text"))
        .respond_with(reply(
            "translator-model",
            serde_json::json!({ "role": "assistant", "content": "Hello world" }),
        ))
        .mount(&ollama)
        .await;
    // The coordinator has the translation: report.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(body_string_contains("Hello world"))
        .respond_with(reply(
            "coordinator-model",
            serde_json::json!({ "role": "assistant", "content": "Done: Hello world" }),
        ))
        .mount(&ollama)
        .await;
    // The coordinator's first request: hand over to the translator.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(reply(
            "coordinator-model",
            serde_json::json!({
                "role": "assistant", "content": "",
                "tool_calls": [ { "function": { "name": "translator", "arguments": { "request": "Hallo Welt" } } } ]
            }),
        ))
        .mount(&ollama)
        .await;

    let spec = load("coordinator.json");
    let base = equivalence_dir().join("coordinator_base");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::copy(
        workspace_root().join("examples/specs/translator.json"),
        base.join("translator.json"),
    )
    .unwrap();
    let dir = equivalence_dir().join(&spec.name);
    write_crate(&spec, &[], &dir);
    write_test(
        &dir,
        &format!(
            r#"use std::sync::{{Arc, Mutex}};

/// Records the models of `llm_call_completed` under the name of the agent it was installed for.
struct Models(String, Arc<Mutex<Vec<(String, Option<String>)>>>);

impl grauwolf_agents::events::Feature for Models {{
    fn on_event(&self, event: &grauwolf_agents::events::AgentEvent) {{
        let json = serde_json::to_value(event).unwrap();
        if json["event"] == "llm_call_completed" {{
            let model = json["model"].as_str().map(str::to_owned);
            self.1.lock().unwrap().push((self.0.clone(), model));
        }}
    }}
}}

#[test]
fn sub_agents_get_the_features_for_their_own_spec() {{
    let seen: Arc<Mutex<Vec<(String, Option<String>)>>> = Arc::default();
    let sink = seen.clone();
    let options = grauwolf::provisioning::ProvisionOptions::new(std::sync::Arc::new(grauwolf::config::ProcessEnv))
        .with_agent_features(Some(Arc::new(move |spec: &grauwolf::spec::AgentSpec| {{
            let feature: Arc<dyn grauwolf_agents::events::Feature> =
                Arc::new(Models(spec.name.clone(), sink.clone()));
            vec![feature]
        }})));
    let output = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on({krate}::run_app_with(std::path::Path::new({base:?}), "Translate: Hallo Welt".to_owned(), options, Default::default()))
        .unwrap();
    assert_eq!(output, "Done: Hello world");
    assert!(
        seen.lock().unwrap().contains(&("translator".to_owned(), Some("translator-model".to_owned()))),
        "{{:?}}",
        seen.lock().unwrap()
    );
}}
"#,
            krate = spec.name,
            base = base.display().to_string(),
        ),
    );

    let uri = ollama.uri();
    tokio::task::spawn_blocking(move || {
        run_generated_tests_with(
            "coordinator.json",
            &dir,
            &["--features", "app"],
            &[("OLLAMA_HOST", uri.as_str())],
        )
    })
    .await
    .unwrap();
}

/// The echo service is built in one directory, so the tests that use it take turns.
static SERVICE_BUILD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Generates the service for `echo.json` and builds its binary `agent`.
async fn build_echo_service() {
    let spec = load("echo.json");
    let dir = equivalence_dir().join("echo_service");
    write_crate_with(
        &spec,
        &[],
        &dir,
        &grauwolf_generator::GenerateOptions {
            service: true,
            ..Default::default()
        },
    );
    let build_dir = dir.clone();
    tokio::task::spawn_blocking(move || {
        let output = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "--features",
                "service",
                "--manifest-path",
            ])
            .arg(build_dir.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", equivalence_dir().join("target"))
            .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
            .env("CARGO_INCREMENTAL", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "the service does not build:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    })
    .await
    .unwrap();
}

/// A generated crate with `--service` (ADR-0093): the binary `agent` builds with the feature
/// `service`, listens on `SERVER_PORT` and answers `/health`, `/run` and the chat API against a
/// mocked Ollama. With `GRAUWOLF_EVENTS=1` it writes the events of the runs as JSON lines to
/// stderr (`agent_starting` among them).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_services_answer_health_and_run() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "echo-model",
            "message": { "role": "assistant", "content": "Hello from the service" },
            "done": true
        })))
        .mount(&ollama)
        .await;
    let _lock = SERVICE_BUILD.lock().await;
    build_echo_service().await;

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let a2a_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut service = Command::new(equivalence_dir().join("target/debug/agent"))
        .env("SERVER_PORT", port.to_string())
        .env("A2A_PORT", a2a_port.to_string())
        .env("A2A_BASE_PATH", "/a2a")
        .env("GRAUWOLF_ALLOW_ANONYMOUS", "true")
        .env("GRAUWOLF_CHAT_API", "1")
        // The events of the runs go to stderr as JSON lines (`GRAUWOLF_EVENTS`).
        .env("GRAUWOLF_EVENTS", "1")
        .stderr(std::process::Stdio::piped())
        .env("OLLAMA_HOST", ollama.uri())
        .spawn()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut health = None;
    for _ in 0..100 {
        if let Ok(response) = client.get(format!("{base}/health")).send().await {
            health = Some(response.text().await.unwrap());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let answer: serde_json::Value = client
        .post(format!("{base}/run"))
        .json(&serde_json::json!({ "input": "Hello" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let chat: serde_json::Value = client
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
    let card: serde_json::Value = client
        .get(format!("{base}/.well-known/agent-card.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: serde_json::Value = client
        .get(format!(
            "http://127.0.0.1:{a2a_port}/a2a/.well-known/agent-card.json"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    service.kill().unwrap();
    let _ = service.wait();
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut service.stderr.take().unwrap(), &mut stderr).unwrap();
    let events: Vec<serde_json::Value> = stderr
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "agent_starting"),
        "no agent_starting event on stderr:\n{stderr}"
    );

    assert_eq!(
        second["supportedInterfaces"][0]["url"],
        format!("http://localhost:{a2a_port}/a2a/a2a")
    );
    assert_eq!(health.as_deref(), Some("ok"));
    assert_eq!(
        answer,
        serde_json::json!({ "output": "Hello from the service" })
    );
    assert_eq!(card["name"], "echo");
    // The chat API (ADR-0094): the same run, and the model the run used is observed.
    assert_eq!(
        chat,
        serde_json::json!({ "response": "Hello from the service" })
    );
    assert!(signature["signature"].is_null());
    assert_eq!(
        signature["observations"]["models"]["echo"][0]["modelId"],
        "echo-model"
    );
}

/// A generated service with `GRAUWOLF_OTEL=1` (ADR-0123) exports its spans over OTLP/HTTP to a
/// stand-in for the collector, continues the `traceparent` of a request, and sends the spans
/// still pending when it gets SIGTERM. The collector is a wiremock that accepts
/// `POST /v1/traces`; the protobuf body is not decoded: ids are `bytes` and names are `string`
/// fields, which protobuf writes unchanged, so they are looked for as byte sequences.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_services_export_spans_and_continue_the_callers_trace() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "echo-model",
            "message": { "role": "assistant", "content": "Hello from the service" },
            "done": true
        })))
        .mount(&ollama)
        .await;
    let collector = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&collector)
        .await;
    let _lock = SERVICE_BUILD.lock().await;
    build_echo_service().await;

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut service = Command::new(equivalence_dir().join("target/debug/agent"))
        .env("SERVER_PORT", port.to_string())
        .env("GRAUWOLF_ALLOW_ANONYMOUS", "true")
        .env("GRAUWOLF_OTEL", "1")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", collector.uri())
        .env("OTEL_SERVICE_NAME", "echo-service")
        .env("OLLAMA_HOST", ollama.uri())
        .spawn()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut up = false;
    for _ in 0..100 {
        if client.get(format!("{base}/health")).send().await.is_ok() {
            up = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(up, "the service does not start");

    let trace_id: [u8; 16] = *b"\x4b\xf9\x2f\x35\x77\xb3\x4d\xa6\xa3\xce\x92\x9d\x0e\x0e\x47\x36";
    let parent_id: [u8; 8] = *b"\x00\xf0\x67\xaa\x0b\xa9\x02\xb7";
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let answer: serde_json::Value = client
        .post(format!("{base}/run"))
        .header(
            "traceparent",
            format!("00-{}-{}-01", hex(&trace_id), hex(&parent_id)),
        )
        .json(&serde_json::json!({ "input": "Hello" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        answer,
        serde_json::json!({ "output": "Hello from the service" })
    );
    // The batch exporter waits seconds for more spans: only the flush on SIGTERM brings them
    // now.
    assert!(
        collector.received_requests().await.unwrap().is_empty(),
        "the spans were not batched"
    );
    let terminated = Command::new("kill")
        .args(["-TERM", &service.id().to_string()])
        .status()
        .unwrap();
    assert!(terminated.success());
    let mut exit = None;
    for _ in 0..100 {
        if let Some(status) = service.try_wait().unwrap() {
            exit = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if exit.is_none() {
        service.kill().unwrap();
        let _ = service.wait();
    }
    assert!(exit.is_some_and(|status| status.success()), "{exit:?}");

    let requests = collector.received_requests().await.unwrap();
    assert!(!requests.is_empty(), "no span arrived at the collector");
    let contains = |haystack: &[u8], needle: &[u8]| {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    };
    let body: Vec<u8> = requests.iter().flat_map(|r| r.body.clone()).collect();
    assert!(
        requests
            .iter()
            .all(|r| r.url.path() == "/v1/traces" && r.method.as_str() == "POST")
    );
    assert!(contains(&body, b"invoke_agent"), "no invoke_agent span");
    assert!(contains(&body, b"echo-service"), "service name missing");
    assert!(
        contains(&body, &trace_id),
        "the span is not in the trace of the caller"
    );
    assert!(
        contains(&body, &parent_id),
        "the span is not a child of the caller's span"
    );
}

/// A generated service with `GRAUWOLF_SPIFFE=1` against the fake Workload API (ADR-0109,
/// ADR-0119): a mapped client reaches the mTLS port, an unmapped one gets `403`, and the run
/// reports the caller in `agent_starting` – the same behavior as `spec serve`. The fake Workload
/// API runs in this process on a Unix socket, the service connects to it like to a SPIRE agent.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_services_authenticate_callers_by_spiffe() {
    use grauwolf_spiffe::testing::{FakeIdentity, FakeWorkloadApi, TestCa};
    use grauwolf_spiffe::{SpiffeId, SpiffeMatch, TlsIdentity};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SERVICE: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";
    const COORDINATOR: &str = "spiffe://agentswork.de/ns/coordinator/sa/coordinator";
    const INTRUDER: &str = "spiffe://agentswork.de/ns/intruder/sa/intruder";

    let ollama = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "echo-model",
            "message": { "role": "assistant", "content": "Hello from the service" },
            "done": true
        })))
        .mount(&ollama)
        .await;
    let _lock = SERVICE_BUILD.lock().await;
    build_echo_service().await;

    let dir = grauwolf_test::TestDir::new("generated-spiffe");
    let ca = TestCa::new("agentswork.de");
    let socket = dir.join("agent.sock");
    let workload_api = FakeWorkloadApi::start_with_ca(&socket, SERVICE, 0, &ca);
    let callers = dir.join("callers.json");
    std::fs::write(
        &callers,
        serde_json::json!({ COORDINATOR: "coordinator" }).to_string(),
    )
    .unwrap();
    let free_port = || {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    let (port, mtls_port) = (free_port(), free_port());
    let mut service = Command::new(equivalence_dir().join("target/debug/agent"))
        .env("SERVER_PORT", port.to_string())
        .env("GRAUWOLF_ALLOW_ANONYMOUS", "true")
        .env("OLLAMA_HOST", ollama.uri())
        .env("GRAUWOLF_SPIFFE", "1")
        .env("GRAUWOLF_SPIFFE_PORT", mtls_port.to_string())
        .env("GRAUWOLF_SPIFFE_USERS", &callers)
        .env("SPIFFE_ENDPOINT_SOCKET", workload_api.endpoint())
        .spawn()
        .unwrap();

    let client_for = |id: &str| {
        let identity = FakeIdentity::with_ca(&ca, id);
        let tls = identity
            .client_tls(&SpiffeMatch::Id(SpiffeId::parse(SERVICE).unwrap()))
            .unwrap();
        reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .build()
            .unwrap()
    };
    let coordinator = client_for(COORDINATOR);
    let mtls = format!("https://localhost:{mtls_port}");
    let mut health = None;
    for _ in 0..100 {
        if let Ok(response) = coordinator.get(format!("{mtls}/health")).send().await {
            health = Some(response.text().await.unwrap());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let run = |client: reqwest::Client, route: &'static str| {
        let url = format!("{mtls}{route}");
        async move {
            let response = client
                .post(url)
                .json(&serde_json::json!({ "input": "Hello" }))
                .send()
                .await
                .unwrap();
            (response.status().as_u16(), response.text().await.unwrap())
        }
    };
    let mapped = run(coordinator.clone(), "/run").await;
    let streamed = run(coordinator, "/run/stream").await;
    let unmapped = run(client_for(INTRUDER), "/run").await;
    // Without a certificate the mTLS port refuses the handshake.
    let bare = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let refused = bare.get(format!("{mtls}/health")).send().await.is_err();
    service.kill().unwrap();
    let _ = service.wait();

    assert_eq!(health.as_deref(), Some("ok"));
    assert_eq!(
        mapped,
        (200, r#"{"output":"Hello from the service"}"#.to_owned())
    );
    assert_eq!(streamed.0, 200);
    let starting = streamed
        .1
        .lines()
        .find(|line| line.contains("\"event\":\"agent_starting\""))
        .unwrap_or_else(|| panic!("no agent_starting in {}", streamed.1));
    let event: serde_json::Value =
        serde_json::from_str(starting.trim_start_matches("data: ")).unwrap();
    assert_eq!(event["caller"], COORDINATOR);
    assert_eq!(unmapped.0, 403, "{}", unmapped.1);
    assert!(
        refused,
        "the mTLS port accepted a client without a certificate"
    );
}

/// Tool sources (ADR-0095): a crate generated with `tool_sources` does not build until the
/// modules exist – and the error names `grauwolf tools build` –; once they are built from the
/// sources it behaves like the interpreter, like the variant with embedded modules.
#[test]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
fn generated_crates_with_tool_sources_build_after_their_modules_are_built() {
    let case = CASES
        .iter()
        .find(|case| case.spec == "calculator.json")
        .unwrap();
    let spec = load(case.spec);
    let expected = interpreter_output(&spec, case, &build_tools(&spec));
    let dir = equivalence_dir().join("calculator_sources");
    let sources = spec
        .tools
        .iter()
        .map(|tool| grauwolf_toolbuild::guest_crate(tool).unwrap())
        .collect();
    write_crate_with(
        &spec,
        &[],
        &dir,
        &grauwolf_generator::GenerateOptions {
            tool_sources: Some(sources),
            ..Default::default()
        },
    );
    write_test(&dir, &equivalence_test(&spec, case, &expected));

    let before = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", equivalence_dir().join("target"))
        .output()
        .unwrap();
    assert!(!before.status.success());
    assert!(
        String::from_utf8_lossy(&before.stderr).contains("grauwolf tools build"),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );

    let builder = ToolBuilder::new(workspace_root().join("target/toolbuild-tests"));
    builder.prepare().unwrap();
    for tool in &spec.tools {
        let guest = grauwolf_toolbuild::guest_crate(tool).unwrap();
        let artifact = builder.build_sources(&guest).unwrap();
        std::fs::write(
            dir.join(format!("tools/{}.wasm", tool.name)),
            artifact.bytes,
        )
        .unwrap();
    }
    run_generated_tests("calculator (tool sources)", &dir);
}

/// A multi-agent flow (ADR-0101): a generated app runs the `agent` and `for_each` nodes with
/// its sub-agents from the loader, like the interpreter does in
/// `grauwolf-interpreter/tests/flow_for_each.rs`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_flows_run_agent_and_for_each_nodes_like_the_interpreter() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    let answer = |marker: &'static str, topic: Option<&'static str>, content: &'static str| {
        let mut mock = Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_string_contains(marker));
        if let Some(topic) = topic {
            mock = mock.and(body_string_contains(topic));
        }
        mock.respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "flow-model",
            "message": { "role": "assistant", "content": content },
            "done": true
        })))
    };
    // The decision of the flow, then the sub-agents.
    answer(
        "coherent summary of several short articles",
        None,
        r#"{ "probability": 0.95 }"#,
    )
    .mount(&ollama)
    .await;
    answer("You pick topics", None, r#"["alpha", "beta", "gamma"]"#)
        .mount(&ollama)
        .await;
    for (topic, article) in [
        ("alpha", "Article alpha"),
        ("beta", "Article beta"),
        ("gamma", "Article gamma"),
    ] {
        answer("You write short texts", Some(topic), article)
            .mount(&ollama)
            .await;
    }
    answer("You summarize", None, "Summary of three articles")
        .mount(&ollama)
        .await;

    let spec = load("topic_report.json");
    let base = equivalence_dir().join("topic_report_base");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    for file in [
        "topic_researcher.json",
        "topic_writer.json",
        "topic_summarizer.json",
    ] {
        std::fs::copy(
            workspace_root().join("examples/specs").join(file),
            base.join(file),
        )
        .unwrap();
    }
    let dir = equivalence_dir().join(&spec.name);
    write_crate(&spec, &[], &dir);
    write_test(
        &dir,
        &format!(
            r#"#[test]
fn flows_like_the_interpreter() {{
    let output = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on({krate}::run_app_with(std::path::Path::new({base:?}), "Three topics about animals".to_owned(), grauwolf::provisioning::ProvisionOptions::new(std::sync::Arc::new(grauwolf::config::ProcessEnv)), Default::default()))
        .unwrap();
    assert_eq!(output, "Summary of three articles");
}}
"#,
            krate = spec.name,
            base = base.display().to_string(),
        ),
    );

    let uri = ollama.uri();
    tokio::task::spawn_blocking(move || {
        run_generated_tests_with(
            "topic_report.json",
            &dir,
            &["--features", "app"],
            &[("OLLAMA_HOST", uri.as_str())],
        )
    })
    .await
    .unwrap();
}

/// GOAP with sub-agents and a subgraph as executors (addendum to ADR-0070): the review loop of
/// `examples/specs/review_loop.json` – a rejected review counts an attempt, the draft is revised,
/// the second review approves – in a generated app, like the interpreter runs it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: compiles generated crates with cargo and runs them"]
async fn generated_goap_runs_the_review_loop_with_agent_and_subgraph_actions() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let ollama = MockServer::start().await;
    let answer = |markers: &[&'static str], content: &'static str| {
        let mut mock = Mock::given(method("POST")).and(path("/api/chat"));
        for marker in markers {
            mock = mock.and(body_string_contains(*marker));
        }
        mock.respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "flow-model",
            "message": { "role": "assistant", "content": content },
            "done": true
        })))
    };
    // The first review rejects, later ones approve.
    answer(
        &["You are a reviewer", "First draft"],
        "REJECTED: add a title",
    )
    .up_to_n_times(1)
    .mount(&ollama)
    .await;
    answer(&["You are a reviewer"], "APPROVED")
        .mount(&ollama)
        .await;
    answer(
        &["You are a writer", "Revise the text"],
        "Title\nSecond draft",
    )
    .mount(&ollama)
    .await;
    answer(&["You are a writer", "Write a short text"], "First draft")
        .mount(&ollama)
        .await;
    answer(&["You confirm that a text was saved"], "Saved")
        .mount(&ollama)
        .await;

    let spec = load("review_loop.json");
    let base = equivalence_dir().join("review_loop_base");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    for file in ["draft_writer.json", "draft_reviewer.json"] {
        std::fs::copy(
            workspace_root().join("examples/specs").join(file),
            base.join(file),
        )
        .unwrap();
    }
    let dir = equivalence_dir().join(&spec.name);
    write_crate(&spec, &[], &dir);
    write_test(
        &dir,
        &format!(
            r#"#[test]
fn reviews_like_the_interpreter() {{
    let output = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on({krate}::run_app_with(std::path::Path::new({base:?}), "Autumn".to_owned(), grauwolf::provisioning::ProvisionOptions::new(std::sync::Arc::new(grauwolf::config::ProcessEnv)), Default::default()))
        .unwrap();
    assert_eq!(output, "Title\nSecond draft");
}}
"#,
            krate = spec.name,
            base = base.display().to_string(),
        ),
    );

    let uri = ollama.uri();
    tokio::task::spawn_blocking(move || {
        run_generated_tests_with(
            "review_loop.json",
            &dir,
            &["--features", "app"],
            &[("OLLAMA_HOST", uri.as_str())],
        )
    })
    .await
    .unwrap();
}
