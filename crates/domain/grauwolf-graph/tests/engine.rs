//! Graph engine on `serde_json::Value` (ADR-0017).

use futures_executor::block_on;
use grauwolf_graph::{EngineError, FINISH, Graph, GraphError, START};
use serde_json::json;

#[test]
fn passes_input_through_when_start_leads_to_finish() {
    let graph = Graph::<()>::builder().edge(START, FINISH).build().unwrap();

    let output = block_on(graph.run(&mut (), json!("hello"))).unwrap();

    assert_eq!(output, json!("hello"));
}

#[test]
fn fails_when_no_edge_leaves_the_current_node() {
    let graph = Graph::<()>::builder().build().unwrap();

    let error = block_on(graph.run(&mut (), json!("hello"))).unwrap_err();

    assert!(
        matches!(&error, EngineError::NoMatchingEdge { node } if node == START),
        "unexpected error: {error}"
    );
}

#[test]
fn runs_nodes_and_passes_their_output_along() {
    let graph = Graph::<()>::builder()
        .node("upper", |_, input| {
            Box::pin(async move { Ok(json!(input.as_str().unwrap_or_default().to_uppercase())) })
        })
        .edge(START, "upper")
        .edge("upper", FINISH)
        .build()
        .unwrap();

    let output = block_on(graph.run(&mut (), json!("hello"))).unwrap();

    assert_eq!(output, json!("HELLO"));
}

#[test]
fn takes_the_first_edge_whose_condition_matches_and_uses_its_value() {
    let graph = Graph::<()>::builder()
        .edge_when(START, FINISH, |_, _| Box::pin(async { Ok(None) }))
        .edge_when(START, FINISH, |_, input| {
            Box::pin(async move {
                Ok(Some(json!(format!(
                    "second: {}",
                    input.as_str().unwrap_or_default()
                ))))
            })
        })
        .edge(START, FINISH)
        .build()
        .unwrap();

    let output = block_on(graph.run(&mut (), json!("hello"))).unwrap();

    assert_eq!(output, json!("second: hello"));
}

#[derive(Default)]
struct Counter {
    visits: u32,
}

#[test]
fn stops_after_the_maximum_number_of_node_executions() {
    let graph = Graph::<Counter>::builder()
        .node("again", |ctx, input| {
            ctx.visits += 1;
            Box::pin(async move { Ok(input) })
        })
        .edge(START, "again")
        .edge("again", "again")
        .max_steps(5)
        .build()
        .unwrap();
    let mut ctx = Counter::default();

    let error = block_on(graph.run(&mut ctx, json!(null))).unwrap_err();

    assert!(
        matches!(error, EngineError::MaxStepsExceeded { limit: 5 }),
        "{error}"
    );
    assert_eq!(ctx.visits, 5);
}

#[test]
fn refuses_to_build_a_graph_with_edges_to_unregistered_nodes() {
    let result = Graph::<()>::builder()
        .edge(START, "ghost")
        .edge("ghost", FINISH)
        .build();

    assert!(
        matches!(&result, Err(GraphError::UnknownNode { node }) if node == "ghost"),
        "unexpected result: {:?}",
        result.err()
    );
}
