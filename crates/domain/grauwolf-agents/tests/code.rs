//! Code in graphs: node, condition and transformation on a code module (ADR-0102).

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_agents::AgentContext;
use grauwolf_agents::code::{
    CodeModule, code_condition, code_node, code_node_typed, code_transform,
};
use grauwolf_graph::{FINISH, Graph, START, TypedGraph};
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{LlModel, Prompt, ToolDescriptor};
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// A module whose "code" is a Rust closure on the JSON value.
struct Fake(fn(Value) -> Result<String, String>);

impl DynTool for Fake {
    fn name(&self) -> &str {
        "fake"
    }

    fn descriptor(&self) -> ToolDescriptor {
        unreachable!("not offered to a model")
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        let result = (self.0)(args).map_err(ToolError::Execution);
        Box::pin(async move { result })
    }
}

fn module(code: fn(Value) -> Result<String, String>) -> CodeModule {
    CodeModule::new(Arc::new(Fake(code)))
}

fn context() -> AgentContext {
    AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").build(),
    )
}

#[test]
fn a_code_node_returns_the_json_result_of_the_code() {
    let upper = module(|input| Ok(json!(input.as_str().unwrap().to_uppercase()).to_string()));
    let graph = Graph::<AgentContext>::builder()
        .node("upper", code_node(upper))
        .edge(START, "upper")
        .edge("upper", FINISH)
        .build()
        .unwrap();

    let output = block_on(graph.run(&mut context(), json!("hi"))).unwrap();

    assert_eq!(output, json!("HI"));
}

#[test]
fn an_error_of_the_code_fails_the_node_with_its_message() {
    let failing = module(|_| Err("no good".into()));
    let graph = Graph::<AgentContext>::builder()
        .node("check", code_node(failing))
        .edge(START, "check")
        .edge("check", FINISH)
        .build()
        .unwrap();

    let error = block_on(graph.run(&mut context(), json!("x"))).unwrap_err();

    assert_eq!(error.to_string(), "node 'check' failed: no good");
}

#[test]
fn a_typed_code_node_converts_its_input_and_output() {
    let length = module(|input| Ok(json!(input.as_str().unwrap().len()).to_string()));
    let graph = TypedGraph::<AgentContext, String, usize>::build(|g| {
        let node = g.node("length", code_node_typed::<String, usize>(length.clone()));
        g.edge(&g.start(), &node);
        g.edge(&node, &g.finish());
    })
    .unwrap();

    assert_eq!(
        block_on(graph.run(&mut context(), "four".to_owned())).unwrap(),
        4
    );
}

#[test]
fn a_condition_passes_the_value_through_when_the_code_says_true() {
    let long = module(|input| Ok((input.as_str().unwrap().len() > 3).to_string()));
    let graph = Graph::<AgentContext>::builder()
        .edge_when(START, FINISH, code_condition(long))
        .edge_transformed(START, FINISH, |_, _| Box::pin(async { Ok(json!("short")) }))
        .build()
        .unwrap();

    assert_eq!(
        block_on(graph.run(&mut context(), json!("long text"))).unwrap(),
        json!("long text")
    );
    assert_eq!(
        block_on(graph.run(&mut context(), json!("no"))).unwrap(),
        json!("short")
    );
}

#[test]
fn a_failing_condition_is_an_error_of_the_run_and_not_false() {
    let trap = module(|_| Err("wasm trap: unreachable".into()));
    let graph = Graph::<AgentContext>::builder()
        .edge_when(START, FINISH, code_condition(trap))
        .edge(START, FINISH)
        .build()
        .unwrap();

    let error = block_on(graph.run(&mut context(), json!("x"))).unwrap_err();

    assert!(error.to_string().contains("wasm trap"), "{error}");
}

#[test]
fn a_condition_must_answer_with_a_boolean() {
    let odd = module(|_| Ok("\"yes\"".into()));
    let graph = Graph::<AgentContext>::builder()
        .edge_when(START, FINISH, code_condition(odd))
        .build()
        .unwrap();

    let error = block_on(graph.run(&mut context(), json!("x"))).unwrap_err();

    assert!(error.to_string().contains("true or false"), "{error}");
}

#[test]
fn a_transformation_gives_the_target_the_result_of_the_code() {
    let wrap = module(|input| Ok(json!({ "wrapped": input }).to_string()));
    let graph = Graph::<AgentContext>::builder()
        .edge_transformed(START, FINISH, code_transform(wrap))
        .build()
        .unwrap();

    assert_eq!(
        block_on(graph.run(&mut context(), json!("x"))).unwrap(),
        json!({ "wrapped": "x" })
    );
}
