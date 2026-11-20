//! Task list tools (ADR-0098); the state comes from the agent context.

use std::sync::Arc;

use grauwolf_builtin_tools::{TODO_STATE_KEY, TODO_TOOLS, todo_tools};
use grauwolf_tools::DynTool;
use serde_json::{Value, json};

struct List {
    tools: Vec<Arc<dyn DynTool>>,
    state: Value,
}

impl List {
    fn new() -> Self {
        Self {
            tools: todo_tools(TODO_TOOLS.iter().copied()).unwrap(),
            state: Value::Null,
        }
    }

    fn call(&mut self, name: &str, args: Value) -> String {
        let tool = self.tools.iter().find(|tool| tool.name() == name).unwrap();
        assert_eq!(tool.state_key(), Some(TODO_STATE_KEY));
        match tool.call_with_state(args, &mut self.state) {
            Ok(text) => text,
            Err(error) => error.to_string(),
        }
    }
}

#[test]
fn adds_completes_lists_and_removes_tasks() {
    let mut list = List::new();

    assert_eq!(list.call("list_tasks", json!({})), "No tasks");
    assert_eq!(
        list.call("add_task", json!({ "title": "Write tests" })),
        "Added task #1: Write tests"
    );
    assert_eq!(
        list.call("add_task", json!({ "title": "Ship" })),
        "Added task #2: Ship"
    );
    assert_eq!(
        list.call("complete_task", json!({ "id": 1 })),
        "Completed task #1: Write tests"
    );
    assert_eq!(
        list.call("list_tasks", json!({})),
        "#1 [x] Write tests\n#2 [ ] Ship"
    );
    assert_eq!(
        list.call("remove_task", json!({ "id": 1 })),
        "Removed task #1"
    );
    assert_eq!(list.call("list_tasks", json!({})), "#2 [ ] Ship");
}

#[test]
fn ids_are_stable_and_never_reused() {
    let mut list = List::new();
    list.call("add_task", json!({ "title": "a" }));
    list.call("remove_task", json!({ "id": 1 }));

    assert_eq!(
        list.call("add_task", json!({ "title": "b" })),
        "Added task #2: b"
    );
}

#[test]
fn reports_unknown_ids() {
    let mut list = List::new();

    assert_eq!(
        list.call("complete_task", json!({ "id": 7 })),
        "Task #7 not found"
    );
    assert_eq!(
        list.call("remove_task", json!({ "id": 7 })),
        "Task #7 not found"
    );
    assert!(
        list.call("complete_task", json!({}))
            .contains("missing field 'id'")
    );
}
