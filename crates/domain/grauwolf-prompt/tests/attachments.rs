//! Attachments on user messages (ADR-0081).

use grauwolf_prompt::{Attachment, AttachmentData, Message};
use serde_json::json;

#[test]
fn keeps_attachments_of_user_messages_and_reads_older_histories() {
    let message = Message::user_with(
        "What is in the picture?",
        vec![Attachment {
            media_type: "image/png".into(),
            data: AttachmentData::Base64("iVBORw0KGgo=".into()),
        }],
    );

    let json = serde_json::to_value(&message).unwrap();
    let old: Message =
        serde_json::from_value(json!({ "role": "user", "content": "Hello" })).unwrap();

    assert_eq!(
        json,
        json!({
            "role": "user", "content": "What is in the picture?",
            "attachments": [ { "media_type": "image/png", "data": { "base64": "iVBORw0KGgo=" } } ]
        })
    );
    assert_eq!(serde_json::from_value::<Message>(json).unwrap(), message);
    assert_eq!(old, Message::user("Hello"));
    assert_eq!(
        serde_json::to_value(Message::user("Hello")).unwrap(),
        json!({ "role": "user", "content": "Hello" })
    );
}
