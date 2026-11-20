//! Task list tools (ADR-0098): `add_task`, `complete_task`, `remove_task`, `list_tasks`.
//!
//! The list lives in the agent storage of the run (ADR-0044) under [`TODO_STATE_KEY`]: the agent
//! context hands the stored value to [`DynTool::call_with_state`], so the list is part of
//! checkpoints and survives a resume. Task IDs are integers that are never reused.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Names of the todo tools.
pub const TODO_TOOLS: &[&str] = &["add_task", "complete_task", "remove_task", "list_tasks"];

/// Key of the task list in the agent storage.
pub const TODO_STATE_KEY: &str = "grauwolf.todo";

#[derive(Debug, Default, Serialize, Deserialize)]
struct TaskList {
    next_id: u64,
    tasks: Vec<Task>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Task {
    id: u64,
    title: String,
    completed: bool,
}

/// The todo tools with the names `names`; unknown names are an error.
pub fn todo_tools<'n>(
    names: impl IntoIterator<Item = &'n str>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    names
        .into_iter()
        .map(|name| {
            let kind = match name {
                "add_task" => Kind::Add,
                "complete_task" => Kind::Complete,
                "remove_task" => Kind::Remove,
                "list_tasks" => Kind::List,
                other => return Err(format!("unknown built-in tool '{other}'")),
            };
            Ok(Arc::new(TodoTool { kind }) as Arc<dyn DynTool>)
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Kind {
    Add,
    Complete,
    Remove,
    List,
}

struct TodoTool {
    kind: Kind,
}

impl TodoTool {
    fn id(&self, args: &Value) -> Result<u64, ToolError> {
        args["id"].as_u64().ok_or_else(|| ToolError::InvalidArgs {
            tool: self.name().to_owned(),
            reason: "missing field 'id' (integer)".to_owned(),
        })
    }
}

impl DynTool for TodoTool {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Add => "add_task",
            Kind::Complete => "complete_task",
            Kind::Remove => "remove_task",
            Kind::List => "list_tasks",
        }
    }

    fn descriptor(&self) -> ToolDescriptor {
        let id = json!({ "type": "integer", "description": "Task ID" });
        let (description, parameters) = match self.kind {
            Kind::Add => (
                "Adds a new task to the task list.",
                json!({
                    "type": "object",
                    "properties": { "title": { "type": "string", "description": "Task description" } },
                    "required": ["title"]
                }),
            ),
            Kind::Complete => (
                "Marks a task as completed.",
                json!({ "type": "object", "properties": { "id": id }, "required": ["id"] }),
            ),
            Kind::Remove => (
                "Removes a task from the task list.",
                json!({ "type": "object", "properties": { "id": id }, "required": ["id"] }),
            ),
            Kind::List => (
                "Lists all tasks with their IDs and whether they are completed.",
                json!({ "type": "object", "properties": {} }),
            ),
        };
        ToolDescriptor {
            name: self.name().to_owned(),
            description: description.to_owned(),
            parameters,
        }
    }

    /// Without an agent context the list lives only for this call.
    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move { self.call_with_state(args, &mut Value::Null) })
    }

    fn state_key(&self) -> Option<&str> {
        Some(TODO_STATE_KEY)
    }

    fn call_with_state(&self, args: Value, state: &mut Value) -> Result<String, ToolError> {
        let mut list: TaskList = if state.is_null() {
            TaskList::default()
        } else {
            serde_json::from_value(state.clone()).map_err(|error| {
                ToolError::Execution(format!("the task list is damaged: {error}"))
            })?
        };
        let result = match self.kind {
            Kind::Add => {
                let title = args["title"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArgs {
                        tool: self.name().to_owned(),
                        reason: "missing field 'title' (text)".to_owned(),
                    })?;
                list.next_id += 1;
                let id = list.next_id;
                list.tasks.push(Task {
                    id,
                    title: title.to_owned(),
                    completed: false,
                });
                format!("Added task #{id}: {title}")
            }
            Kind::Complete => {
                let id = self.id(&args)?;
                match list.tasks.iter_mut().find(|task| task.id == id) {
                    Some(task) => {
                        task.completed = true;
                        format!("Completed task #{id}: {}", task.title)
                    }
                    None => format!("Task #{id} not found"),
                }
            }
            Kind::Remove => {
                let id = self.id(&args)?;
                let before = list.tasks.len();
                list.tasks.retain(|task| task.id != id);
                if list.tasks.len() < before {
                    format!("Removed task #{id}")
                } else {
                    format!("Task #{id} not found")
                }
            }
            Kind::List if list.tasks.is_empty() => "No tasks".to_owned(),
            Kind::List => list
                .tasks
                .iter()
                .map(|task| {
                    format!(
                        "#{} [{}] {}",
                        task.id,
                        if task.completed { "x" } else { " " },
                        task.title
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        *state =
            serde_json::to_value(&list).map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(result)
    }
}
