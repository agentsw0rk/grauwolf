//! Attachments on the input of a run (ADR-0081).

use std::sync::Arc;

use grauwolf_agents::AgentContext;
use grauwolf_llm::PromptExecutor;
use grauwolf_prompt::{Attachment, AttachmentData, LlModel, Message, Prompt};

fn picture() -> Attachment {
    Attachment {
        media_type: "image/png".into(),
        data: AttachmentData::Base64("iVBORw0KGgo=".into()),
    }
}

#[test]
fn attaches_to_the_next_user_message_only() {
    let mut ctx = AgentContext::new(
        Arc::new(PromptExecutor::builder().build()),
        LlModel::new("mock", "m"),
        Prompt::builder("p").system("Be brief.").build(),
    );
    ctx.attach_to_input(vec![picture()]);

    ctx.append(Message::system("Another instruction."));
    ctx.append(Message::user("What is in the picture?"));
    ctx.append(Message::user("And now?"));

    assert_eq!(
        ctx.prompt().messages[2..],
        [
            Message::user_with("What is in the picture?", vec![picture()]),
            Message::user("And now?"),
        ]
    );
}
