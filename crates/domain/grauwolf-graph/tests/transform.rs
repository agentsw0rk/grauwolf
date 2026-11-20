//! Edge transformations (ADR-0102): applied after the condition, before the target node.

use std::sync::Mutex;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_graph::{
    Checkpoint, CheckpointObserver, EngineError, FINISH, Graph, NodeError, START, TypedGraph,
    erase_transform,
};
use serde_json::{Value, json};

#[test]
fn the_target_receives_the_transformed_value() {
    let graph = Graph::<()>::builder()
        .node("shout", |_, input| {
            Box::pin(async move { Ok(json!(input.as_str().unwrap().to_uppercase())) })
        })
        .edge_transformed(START, "shout", |_, input| {
            Box::pin(async move { Ok(json!(format!("{}!", input.as_str().unwrap()))) })
        })
        .edge("shout", FINISH)
        .build()
        .unwrap();

    let output = block_on(graph.run(&mut (), json!("hi"))).unwrap();

    assert_eq!(output, json!("HI!"));
}

#[test]
fn the_transformation_runs_after_the_condition_and_only_on_the_taken_edge() {
    let graph = Graph::<Vec<String>>::builder()
        .edge_when_transformed(
            START,
            FINISH,
            |log, _| {
                assert!(log.is_empty(), "the condition runs first");
                Box::pin(async { Ok(None) })
            },
            |_, _| Box::pin(async { Ok(json!("never")) }),
        )
        .edge_when_transformed(
            START,
            FINISH,
            |_, input| Box::pin(async move { Ok(Some(json!([input]))) }),
            |_, input| Box::pin(async move { Ok(json!({ "wrapped": input })) }),
        )
        .build()
        .unwrap();

    let output = block_on(graph.run(&mut Vec::new(), json!("x"))).unwrap();

    assert_eq!(output, json!({ "wrapped": ["x"] }));
}

#[test]
fn a_failing_transformation_fails_the_run_and_names_the_source_node() {
    let graph = Graph::<()>::builder()
        .edge_transformed(START, FINISH, |_, _| {
            Box::pin(async { Err::<Value, NodeError>("cannot reshape".into()) })
        })
        .build()
        .unwrap();

    let error = block_on(graph.run(&mut (), json!("x"))).unwrap_err();

    assert!(
        matches!(&error, EngineError::Transform { node, .. } if node == START),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("cannot reshape"), "{error}");
}

#[test]
fn typed_graphs_transform_between_different_types() {
    let graph = TypedGraph::<(), String, usize>::build(|g| {
        let double = g.node("double", |_, number: usize| {
            Box::pin(async move { Ok(number * 2) })
        });
        g.edge_with(
            &g.start(),
            &double,
            None,
            Some(erase_transform(|_, text: String| {
                text.parse::<usize>().map_err(NodeError::from)
            })),
        );
        g.edge(&double, &g.finish());
    })
    .unwrap();

    assert_eq!(block_on(graph.run(&mut (), "21".to_string())).unwrap(), 42);
}

/// Observer that collects all checkpoints.
#[derive(Default)]
struct Recorder(Mutex<Vec<Checkpoint>>);

impl CheckpointObserver<()> for Recorder {
    fn checkpoint<'a>(
        &'a self,
        _ctx: &'a (),
        checkpoint: &'a Checkpoint,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        self.0.lock().unwrap().push(checkpoint.clone());
        Box::pin(async { Ok(()) })
    }
}

fn pipeline() -> Graph<()> {
    Graph::<()>::builder()
        .node("count", |_, input| {
            Box::pin(async move { Ok(json!(input["items"].as_array().unwrap().len())) })
        })
        .edge_transformed(START, "count", |_, input| {
            Box::pin(async move { Ok(json!({ "items": input })) })
        })
        .edge("count", FINISH)
        .build()
        .unwrap()
}

#[test]
fn checkpoints_hold_the_transformed_value_and_a_resume_does_not_transform_again() {
    let recorder = Recorder::default();
    let output = block_on(pipeline().run_observed(&mut (), json!([1, 2, 3]), &recorder)).unwrap();
    assert_eq!(output, json!(3));
    let checkpoint = recorder.0.lock().unwrap()[0].clone();
    assert_eq!(checkpoint.path, ["count"]);
    assert_eq!(checkpoint.input, json!({ "items": [1, 2, 3] }));

    let resumed = block_on(pipeline().resume(&mut (), checkpoint, &Recorder::default())).unwrap();

    assert_eq!(resumed, json!(3));
}
