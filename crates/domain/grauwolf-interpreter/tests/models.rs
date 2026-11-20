//! Own model per subgraph (F05.6, ADR-0043).

use grauwolf_spec::AgentSpecExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;

/// Answers with the model name and records the models of the requests.
#[derive(Default)]
struct ModelEcho(Mutex<Vec<String>>);

impl LlmClient for ModelEcho {
    fn execute<'a>(
        &'a self,
        _prompt: &'a Prompt,
        model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.lock().unwrap().push(model.id.clone());
        Box::pin(async move { Ok(Message::assistant(format!("from {}", model.id))) })
    }
}

#[test]
fn subgraphs_run_with_their_own_model() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/specs/two_models.json");
    let spec = AgentSpec::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
    let client = Arc::new(ModelEcho::default());
    let executor = PromptExecutor::builder()
        .client("ollama", client.clone())
        .build();
    let agent = Interpreter::new(Arc::new(executor)).load(&spec).unwrap();

    let output = block_on(agent.run("Topic")).unwrap();

    assert_eq!(*client.0.lock().unwrap(), ["qwen3.5:9b", "llama3.2"]);
    assert_eq!(output, "from llama3.2");
}
