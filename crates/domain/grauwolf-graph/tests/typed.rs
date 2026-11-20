//! Typed graph builder (ADR-0002) on top of the value engine (ADR-0017).

use futures_executor::block_on;
use grauwolf_graph::TypedGraph;

#[test]
fn runs_a_typed_graph_with_typed_input_and_output() {
    let graph = TypedGraph::<(), String, usize>::build(|g| {
        let length = g.node("length", |_, text: String| {
            Box::pin(async move { Ok(text.len()) })
        });
        g.edge(&g.start(), &length);
        g.edge(&length, &g.finish());
    })
    .unwrap();

    let output = block_on(graph.run(&mut (), "hello".to_string())).unwrap();

    assert_eq!(output, 5);
}

#[test]
fn typed_conditions_filter_and_convert_the_edge_value() {
    let graph = TypedGraph::<(), String, usize>::build(|g| {
        let fallback = g.node("fallback", |_, _: String| Box::pin(async { Ok(0_usize) }));
        g.edge_when(&g.start(), &g.finish(), |_, text: String| {
            text.parse::<usize>().ok()
        });
        g.edge(&g.start(), &fallback);
        g.edge(&fallback, &g.finish());
    })
    .unwrap();

    assert_eq!(block_on(graph.run(&mut (), "42".to_string())).unwrap(), 42);
    assert_eq!(block_on(graph.run(&mut (), "abc".to_string())).unwrap(), 0);
}
