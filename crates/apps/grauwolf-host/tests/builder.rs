//! Checker for the builder agent, which really compiles tool code (F13.9, ADR-0035).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;

use grauwolf_builder_agent::SpecChecker;
use grauwolf_host::builder::ToolBuildChecker;
use grauwolf_spec::AgentSpec;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn calculator(code_change: (&str, &str)) -> AgentSpec {
    let json = std::fs::read_to_string(workspace().join("examples/specs/calculator.json")).unwrap();
    AgentSpec::from_json(&json.replace(code_change.0, code_change.1)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: builds tool code for WASM with cargo"]
async fn reports_compile_errors_of_tool_code_with_their_position() {
    let checker = ToolBuildChecker::new(workspace().join("target/toolbuild-tests"));

    let issues = checker
        .check(&calculator(("(a + b)", "(a + unknown_value)")))
        .await;

    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].path, "/tools/0/code");
    assert!(
        issues[0]
            .message
            .starts_with("line 3, column 9: error: cannot find value `unknown_value`"),
        "{issues:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn accepts_tool_code_that_compiles() {
    let checker = ToolBuildChecker::new(workspace().join("target/toolbuild-tests"));

    assert!(checker.check(&calculator(("", ""))).await.is_empty());
}
