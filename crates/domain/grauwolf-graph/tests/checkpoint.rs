//! Checkpoints and resuming (ADR-0020, ADR-0034).

use std::sync::Mutex;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_graph::{Checkpoint, CheckpointObserver, FINISH, Graph, NodeError, START, Scope};
use serde_json::{Value, json};

type Log = Vec<String>;

fn step(
    name: &'static str,
) -> impl for<'a> Fn(&'a mut Log, Value) -> BoxFuture<'a, Result<Value, NodeError>> + Send + Sync + 'static
{
    move |log: &mut Log, value: Value| {
        log.push(name.to_owned());
        Box::pin(async move { Ok(json!(format!("{}+{name}", value.as_str().unwrap()))) })
    }
}

struct Named(&'static str);

impl Scope<Log> for Named {
    fn enter(&self, log: &mut Log) {
        log.push(format!("enter {}", self.0));
    }

    fn exit(&self, log: &mut Log) {
        log.push(format!("exit {}", self.0));
    }
}

/// Observer that collects all checkpoints.
#[derive(Default)]
struct Recorder(Mutex<Vec<Checkpoint>>);

impl CheckpointObserver<Log> for Recorder {
    fn checkpoint<'a>(
        &'a self,
        _ctx: &'a Log,
        checkpoint: &'a Checkpoint,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        self.0.lock().unwrap().push(checkpoint.clone());
        Box::pin(async { Ok(()) })
    }
}

/// before → sub(inner) → after
fn nested() -> Graph<Log> {
    let inner = Graph::<Log>::builder()
        .node("inner", step("inner"))
        .edge(START, "inner")
        .edge("inner", FINISH)
        .build()
        .unwrap();
    Graph::<Log>::builder()
        .node("before", step("before"))
        .subgraph("sub", inner, Some(Box::new(Named("sub"))))
        .node("after", step("after"))
        .edge(START, "before")
        .edge("before", "sub")
        .edge("sub", "after")
        .edge("after", FINISH)
        .build()
        .unwrap()
}

fn checkpoint(path: &[&str], input: &str) -> Checkpoint {
    Checkpoint {
        path: path.iter().map(|id| id.to_string()).collect(),
        input: json!(input),
    }
}

#[test]
fn reports_a_checkpoint_before_every_node_with_its_path() {
    let recorder = Recorder::default();
    let mut log = Log::new();

    block_on(nested().run_observed(&mut log, json!("x"), &recorder)).unwrap();

    assert_eq!(
        *recorder.0.lock().unwrap(),
        [
            checkpoint(&["before"], "x"),
            checkpoint(&["sub"], "x+before"),
            checkpoint(&["sub", "inner"], "x+before"),
            checkpoint(&["after"], "x+before+inner"),
        ]
    );
}

#[test]
fn resumes_inside_a_subgraph_without_repeating_earlier_nodes() {
    let recorder = Recorder::default();
    let mut log = Log::new();

    let output = block_on(nested().resume(
        &mut log,
        checkpoint(&["sub", "inner"], "x+before"),
        &recorder,
    ))
    .unwrap();

    assert_eq!(output, json!("x+before+inner+after"));
    assert_eq!(log, ["enter sub", "inner", "exit sub", "after"]);
    assert_eq!(
        *recorder.0.lock().unwrap(),
        [
            checkpoint(&["sub", "inner"], "x+before"),
            checkpoint(&["after"], "x+before+inner"),
        ]
    );
}

#[test]
fn rejects_checkpoints_whose_path_does_not_exist() {
    let error = block_on(nested().resume(
        &mut Log::new(),
        checkpoint(&["before", "inner"], "x"),
        &Recorder::default(),
    ))
    .unwrap_err();

    assert!(
        matches!(&error, grauwolf_graph::EngineError::InvalidCheckpoint { path } if path == &["before", "inner"]),
        "{error}"
    );
}

#[test]
fn typed_graphs_report_checkpoints_and_resume() {
    use grauwolf_graph::TypedGraph;

    let graph = || {
        TypedGraph::<Log, String, usize>::build(|g| {
            let count = g.node("count", |log: &mut Log, text: String| {
                log.push("count".into());
                Box::pin(async move { Ok(text.len()) })
            });
            g.edge(&g.start(), &count);
            g.edge(&count, &g.finish());
        })
        .unwrap()
    };
    let recorder = Recorder::default();

    let output =
        block_on(graph().run_observed(&mut Log::new(), "hello".into(), &recorder)).unwrap();
    let saved = recorder.0.lock().unwrap()[0].clone();
    let resumed = block_on(graph().resume(&mut Log::new(), saved.clone(), &recorder)).unwrap();

    assert_eq!((output, resumed), (5, 5));
    assert_eq!(saved, checkpoint(&["count"], "hello"));
}

/// Observer that logs the start and end of every node.
#[derive(Default)]
struct Lifecycle(Mutex<Vec<String>>);

impl CheckpointObserver<Log> for Lifecycle {
    fn checkpoint<'a>(
        &'a self,
        _ctx: &'a Log,
        checkpoint: &'a Checkpoint,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        self.0
            .lock()
            .unwrap()
            .push(format!("start {}", checkpoint.path.join("/")));
        Box::pin(async { Ok(()) })
    }

    fn node_completed<'a>(
        &'a self,
        _ctx: &'a Log,
        path: &'a [String],
        output: &'a Value,
    ) -> BoxFuture<'a, Result<(), NodeError>> {
        self.0
            .lock()
            .unwrap()
            .push(format!("done {} = {output}", path.join("/")));
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn reports_the_output_of_every_completed_node() {
    let lifecycle = Lifecycle::default();

    block_on(nested().run_observed(&mut Log::new(), json!("x"), &lifecycle)).unwrap();

    assert_eq!(
        *lifecycle.0.lock().unwrap(),
        [
            "start before",
            "done before = \"x+before\"",
            "start sub",
            "start sub/inner",
            "done sub/inner = \"x+before+inner\"",
            "done sub = \"x+before+inner\"",
            "start after",
            "done after = \"x+before+inner+after\"",
        ]
    );
}
