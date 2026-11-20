//! Builder agent: requirement → valid AgentSpec (F13.12, ADR-0035).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::Arc;

use futures_executor::block_on;
use grauwolf_builder_agent::Builder;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::LlModel;
use grauwolf_test::MockLlmClient;

fn example(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/specs")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn builder(mock: MockLlmClient) -> Builder {
    let executor = PromptExecutor::builder()
        .client("mock", Arc::new(mock))
        .build();
    Builder::new(Arc::new(executor), LlModel::new("mock", "m"))
}

#[test]
fn returns_the_spec_when_the_first_answer_is_valid() {
    let answer = format!("```json\n{}\n```", example("echo.json"));
    let builder = builder(MockLlmClient::new("?").answer_when_contains("repeats", answer));

    let built = block_on(builder.build("An agent that repeats every message.")).unwrap();

    assert_eq!(built.spec.name, "echo");
    assert_eq!(built.repairs, 0);
    let reparsed = grauwolf_spec::AgentSpec::from_json(&built.json).unwrap();
    assert_eq!(reparsed, built.spec);
}

#[test]
fn repairs_the_spec_with_the_reported_issues() {
    let broken = example("echo.json").replace("\"llm_request\"", "\"ask_llm\"");
    let mock = MockLlmClient::new("?")
        // The feedback names path and error; only then does the corrected spec come.
        .answer_when_contains(
            "/graph/nodes/0/kind: unknown node kind 'ask_llm'",
            example("echo.json"),
        )
        .answer_when_contains("repeats", broken);
    let builder = builder(mock);

    let built = block_on(builder.build("An agent that repeats every message.")).unwrap();

    assert_eq!(built.spec.name, "echo");
    assert_eq!(built.repairs, 1);
}

#[test]
fn gives_up_after_the_allowed_repairs_with_the_last_issues() {
    let mock = MockLlmClient::new("this is not JSON");
    let builder = builder(mock).with_max_repairs(2);

    let error = block_on(builder.build("Some agent")).unwrap_err();

    let grauwolf_builder_agent::BuilderError::GaveUp { repairs, issues } = &error else {
        panic!("expected giving up: {error}");
    };
    assert_eq!(*repairs, 2);
    assert!(
        issues[0].message.contains("invalid spec JSON"),
        "{issues:?}"
    );
}

/// Checker that rejects tool code with `unknown_value` like a compiler.
struct FakeCompiler;

impl grauwolf_builder_agent::SpecChecker for FakeCompiler {
    fn check<'a>(
        &'a self,
        spec: &'a grauwolf_spec::AgentSpec,
    ) -> futures_util::future::BoxFuture<'a, Vec<grauwolf_builder_agent::SpecIssue>> {
        let issues = spec
            .tools
            .iter()
            .enumerate()
            .filter(|(_, tool)| tool.code.contains("unknown_value"))
            .map(|(index, _)| grauwolf_builder_agent::SpecIssue {
                path: format!("/tools/{index}/code"),
                message: "line 2, column 9: error: cannot find value `unknown_value`".into(),
            })
            .collect();
        Box::pin(async move { issues })
    }
}

#[test]
fn repairs_issues_found_by_the_spec_checker() {
    let valid = example("calculator.json");
    let broken = valid.replace("(a + b)", "(a + unknown_value)");
    assert_ne!(broken, valid);
    let mock = MockLlmClient::new("?")
        .answer_when_contains("/tools/0/code: line 2", valid)
        .answer_when_contains("Calculator", broken);
    let builder = builder(mock).with_checker(Arc::new(FakeCompiler));

    let built = block_on(builder.build("Calculator with an add tool")).unwrap();

    assert_eq!(built.repairs, 1);
    assert!(!built.spec.tools[0].code.contains("unknown_value"));
}

/// Client that records the system prompt of the first request.
#[derive(Default)]
struct SystemPromptRecorder(std::sync::Mutex<Option<String>>);

impl grauwolf_llm::LlmClient for SystemPromptRecorder {
    fn execute<'a>(
        &'a self,
        prompt: &'a grauwolf_prompt::Prompt,
        _model: &'a LlModel,
        _tools: &'a [grauwolf_prompt::ToolDescriptor],
    ) -> futures_util::future::BoxFuture<'a, Result<grauwolf_prompt::Message, grauwolf_llm::LlmError>>
    {
        if let Some(grauwolf_prompt::Message::System { content }) = prompt.messages.first() {
            self.0.lock().unwrap().get_or_insert(content.clone());
        }
        let answer = example("echo.json");
        Box::pin(async move { Ok(grauwolf_prompt::Message::assistant(answer)) })
    }
}

#[test]
fn tells_the_llm_the_schema_catalogs_tool_rules_and_an_example() {
    let recorder = Arc::new(SystemPromptRecorder::default());
    let executor = PromptExecutor::builder()
        .client("mock", recorder.clone())
        .build();

    block_on(Builder::new(Arc::new(executor), LlModel::new("mock", "m")).build("Echo")).unwrap();

    let system = recorder.0.lock().unwrap().clone().unwrap();
    let catalogs = grauwolf_graph::Catalogs::builtin();
    for expected in [
        "\"spec_version\"".to_string(),
        "- llm_request: string → message".to_string(),
        "- llm_request_structured, params {\"type\":\"Example\"}: string → Example".to_string(),
        "- on_tool_call: message → tool_call".to_string(),
        "- subgraph".to_string(),
        "- parallel, params {\"subgraphs\":[\"<a>\",\"<b>\"],\"merge\":\"concat\"}".to_string(),
        "- plan, params {\"subgraph\":\"<name>\",\"critic\":true,\"max_iterations\":10}: string → string".to_string(),
        "fn tool(args: serde_json::Value) -> Result<String, String>".to_string(),
        "unsafe".to_string(),
        "`\\n` with exactly one backslash".to_string(),
        "## Proven patterns".to_string(),
        "call_llm → execute_tool [on_tool_call]".to_string(),
        "call_llm → execute_all [on_multiple_tool_calls]".to_string(),
        "\"strategy\": {\"kind\": \"single_run\"}".to_string(),
        "send_result → finish [on_assistant_message]".to_string(),
        "\"name\": \"calculator\"".to_string(),
    ]
    .into_iter()
    .chain(catalogs.nodes.kinds().map(|kind| format!("- {kind}")))
    .chain(catalogs.conditions.kinds().map(|kind| format!("- {kind}")))
    {
        assert!(
            system.contains(&expected),
            "system prompt without {expected:?}"
        );
    }
}

/// Client that answers only structured requests and records their type names.
#[derive(Default)]
struct StructuredOnly(std::sync::Mutex<Vec<(String, serde_json::Value)>>);

impl grauwolf_llm::LlmClient for StructuredOnly {
    fn execute<'a>(
        &'a self,
        _prompt: &'a grauwolf_prompt::Prompt,
        _model: &'a LlModel,
        _tools: &'a [grauwolf_prompt::ToolDescriptor],
    ) -> futures_util::future::BoxFuture<'a, Result<grauwolf_prompt::Message, grauwolf_llm::LlmError>>
    {
        Box::pin(async { Err(grauwolf_llm::LlmError::Client("structured only".into())) })
    }

    fn execute_structured<'a>(
        &'a self,
        _prompt: &'a grauwolf_prompt::Prompt,
        _model: &'a LlModel,
        output: &'a grauwolf_llm::StructuredOutput,
    ) -> futures_util::future::BoxFuture<'a, Result<grauwolf_prompt::Message, grauwolf_llm::LlmError>>
    {
        self.0
            .lock()
            .unwrap()
            .push((output.name.clone(), output.schema.clone()));
        let answer = example("echo.json");
        Box::pin(async move { Ok(grauwolf_prompt::Message::assistant(answer)) })
    }
}

#[test]
fn requests_the_spec_as_structured_output_with_the_dsl_schema() {
    let client = Arc::new(StructuredOnly::default());
    let executor = PromptExecutor::builder()
        .client("mock", client.clone())
        .build();

    let built = block_on(Builder::new(Arc::new(executor), LlModel::new("mock", "m")).build("Echo"))
        .unwrap();

    assert_eq!(built.spec.name, "echo");
    let requests = client.0.lock().unwrap();
    let (name, schema) = &requests[0];
    assert_eq!(name, "AgentSpec");
    // With enforced output, models omit optional fields: a graph needs nodes and edges. The
    // graph itself stays optional, so that a spec can use a `strategy` instead.
    assert!(
        !schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("graph")),
        "{schema}"
    );
    assert_eq!(
        schema["$defs"]["GraphSpec"]["required"],
        serde_json::json!(["nodes", "edges"])
    );
}

#[test]
fn shows_the_lines_around_a_json_syntax_error() {
    let broken = "{\n  \"spec_version\": 1,\n  \"name\": \"echo\"\n  \"model\": {}\n}";
    let builder = builder(MockLlmClient::new(broken)).with_max_repairs(0);

    let error = block_on(builder.build("Echo")).unwrap_err();

    let grauwolf_builder_agent::BuilderError::GaveUp { issues, .. } = &error else {
        panic!("expected giving up: {error}");
    };
    let message = &issues[0].message;
    assert!(message.contains("line 4 column 3"), "{message}");
    assert!(message.contains("3 |   \"name\": \"echo\""), "{message}");
    assert!(message.contains("4 |   \"model\": {}"), "{message}");
}

#[test]
fn turns_doubly_escaped_line_breaks_in_tool_code_into_real_ones() {
    // Some models write `\\n` in the JSON; the code would then contain a single backslash.
    let doubled = example("calculator.json").replace("\\n", "\\\\n");
    assert!(doubled.contains("\\\\n"));
    let builder = builder(MockLlmClient::new("?").answer_when_contains("Calculator", doubled));

    let built = block_on(builder.build("Calculator")).unwrap();

    let code = &built.spec.tools[0].code;
    assert!(code.contains('\n') && !code.contains("\\n"), "{code:?}");
    assert_eq!(
        grauwolf_spec::AgentSpec::from_json(&built.json).unwrap(),
        built.spec
    );
}

#[test]
fn sets_the_model_the_agent_shall_use() {
    let builder =
        builder(MockLlmClient::new("?").answer_when_contains("repeats", example("echo.json")))
            .with_agent_model(LlModel::new("anthropic", "claude-opus-5-5"));

    let built = block_on(builder.build("An agent that repeats every message.")).unwrap();

    assert_eq!(
        (
            built.spec.model.provider.as_str(),
            built.spec.model.id.as_str()
        ),
        ("anthropic", "claude-opus-5-5")
    );
    assert_eq!(
        grauwolf_spec::AgentSpec::from_json(&built.json).unwrap(),
        built.spec
    );
}
