//! Subgraphs in the engine (F05.6, ADR-0034).

use futures_executor::block_on;
use grauwolf_graph::{FINISH, Graph, START, Scope};
use serde_json::{Value, json};

/// Context that logs what happens.
type Log = Vec<String>;

fn step(
    name: &'static str,
) -> impl for<'a> Fn(
    &'a mut Log,
    Value,
) -> futures_util::future::BoxFuture<'a, Result<Value, grauwolf_graph::NodeError>>
+ Send
+ Sync
+ 'static {
    move |log: &mut Log, value: Value| {
        log.push(name.to_owned());
        Box::pin(async move { Ok(json!(format!("{}+{name}", value.as_str().unwrap()))) })
    }
}

/// Scope that logs entering and leaving.
struct Named(&'static str);

impl Scope<Log> for Named {
    fn enter(&self, log: &mut Log) {
        log.push(format!("enter {}", self.0));
    }

    fn exit(&self, log: &mut Log) {
        log.push(format!("exit {}", self.0));
    }
}

#[test]
fn runs_a_subgraph_as_a_node_inside_its_scope() {
    let inner = Graph::<Log>::builder()
        .node("inner", step("inner"))
        .edge(START, "inner")
        .edge("inner", FINISH)
        .build()
        .unwrap();
    let outer = Graph::<Log>::builder()
        .node("before", step("before"))
        .subgraph("sub", inner, Some(Box::new(Named("sub"))))
        .node("after", step("after"))
        .edge(START, "before")
        .edge("before", "sub")
        .edge("sub", "after")
        .edge("after", FINISH)
        .build()
        .unwrap();
    let mut log = Log::new();

    let output = block_on(outer.run(&mut log, json!("x"))).unwrap();

    assert_eq!(output, json!("x+before+inner+after"));
    assert_eq!(log, ["before", "enter sub", "inner", "exit sub", "after"]);
}

#[test]
fn typed_subgraphs_keep_their_input_and_output_types() {
    use grauwolf_graph::TypedGraph;

    let inner = TypedGraph::<Log, String, usize>::build(|g| {
        let count = g.node("count", |log: &mut Log, text: String| {
            log.push("count".into());
            Box::pin(async move { Ok(text.len()) })
        });
        g.edge(&g.start(), &count);
        g.edge(&count, &g.finish());
    })
    .unwrap();
    let outer = TypedGraph::<Log, String, usize>::build(|g| {
        let sub = g.subgraph("sub", inner, Some(Box::new(Named("sub"))));
        g.edge(&g.start(), &sub);
        g.edge(&sub, &g.finish());
    })
    .unwrap();
    let mut log = Log::new();

    let output = block_on(outer.run(&mut log, "hello".to_string())).unwrap();

    assert_eq!(output, 5);
    assert_eq!(log, ["enter sub", "count", "exit sub"]);
}
