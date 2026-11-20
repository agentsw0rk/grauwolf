//! Attachments on the input of a run (ADR-0081).

use grauwolf_spec::AgentSpecExt;
use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_interpreter::{Interpreter, RunExtras};
use grauwolf_llm::{LlmClient, LlmError, PromptExecutor};
use grauwolf_prompt::{Attachment, AttachmentData, LlModel, Message, Prompt, ToolDescriptor};
use grauwolf_spec::AgentSpec;
use serde_json::json;

/// Records the prompts and answers with a fixed reply.
#[derive(Default)]
struct Recorder(Mutex<Vec<Prompt>>);

impl LlmClient for Recorder {
    fn execute<'a>(
        &'a self,
        prompt: &'a Prompt,
        _model: &'a LlModel,
        _tools: &'a [ToolDescriptor],
    ) -> BoxFuture<'a, Result<Message, LlmError>> {
        self.0.lock().unwrap().push(prompt.clone());
        Box::pin(async { Ok(Message::assistant("A wolf.")) })
    }
}

#[test]
fn sends_the_attachments_of_a_run_with_its_input() {
    let spec = AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "seher",
            "model": { "provider": "mock", "id": "m" },
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap();
    let client = Arc::new(Recorder::default());
    let agent = Interpreter::new(Arc::new(
        PromptExecutor::builder()
            .client("mock", client.clone())
            .build(),
    ))
    .load(&spec)
    .unwrap();
    let picture = Attachment {
        media_type: "image/png".into(),
        data: AttachmentData::Base64("iVBORw0KGgo=".into()),
    };

    let output = block_on(agent.run_with(
        json!("What is in the picture?"),
        RunExtras {
            attachments: vec![picture.clone()],
            ..RunExtras::default()
        },
    ))
    .unwrap();

    assert_eq!(output, json!("A wolf."));
    let prompts = client.0.lock().unwrap();
    assert_eq!(
        prompts[0].messages.last().unwrap(),
        &Message::user_with("What is in the picture?", vec![picture])
    );
}
