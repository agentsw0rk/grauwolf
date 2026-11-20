//! Finding the knowledge context of a tool by name (ADR-0116).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_tools::{
    DynTool, KnowledgeContext, KnowledgeHit, ToolDescriptor, ToolError, knowledge_context_of,
};
use serde_json::{Value, json};

struct Nothing;

impl KnowledgeContext for Nothing {
    fn retrieve<'a>(
        &'a self,
        _query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<KnowledgeHit>, ToolError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// A tool named `name` that is the search tool of a knowledge base if `knowledge`.
struct Search {
    name: &'static str,
    knowledge: bool,
}

impl DynTool for Search {
    fn name(&self) -> &str {
        self.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.name.into(),
            description: String::new(),
            parameters: json!({}),
        }
    }

    fn call_json(&self, _args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async { Ok(String::new()) })
    }

    fn knowledge_context(&self) -> Option<Arc<dyn KnowledgeContext>> {
        self.knowledge
            .then(|| Arc::new(Nothing) as Arc<dyn KnowledgeContext>)
    }
}

fn tools() -> Vec<Arc<dyn DynTool>> {
    vec![
        Arc::new(Search {
            name: "docs",
            knowledge: true,
        }),
        Arc::new(Search {
            name: "web",
            knowledge: false,
        }),
    ]
}

#[test]
fn the_knowledge_context_is_found_by_the_name_of_its_tool() {
    assert!(knowledge_context_of(&tools(), "docs").is_some());
}

#[test]
fn an_unknown_tool_or_a_tool_without_a_knowledge_base_has_no_context() {
    assert!(knowledge_context_of(&tools(), "missing").is_none());
    assert!(knowledge_context_of(&tools(), "web").is_none());
}
