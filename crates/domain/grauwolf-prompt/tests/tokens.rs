//! Estimate tokens before the request (F06.13, ADR-0087).

use grauwolf_prompt::{
    Attachment, AttachmentData, EstimatingTokenizer, Message, Prompt, Tokenizer, ToolResult,
    count_prompt,
};
use serde_json::json;

#[test]
fn estimates_a_token_per_four_characters_and_a_little_per_message() {
    let tokenizer = EstimatingTokenizer;
    let mut prompt = Prompt::builder("p")
        .system("abcdefgh") // 8 characters → 2
        .user("äöü") // 3 characters (not bytes) → 1
        .build();
    prompt
        .messages
        .push(Message::tool_call("c1", "add", json!({ "a": 1 }))); // add → 1, {"a":1} → 2
    prompt.messages.push(Message::ToolResult(ToolResult {
        id: "c1".into(),
        tool: "add".into(),
        content: "12345".into(), // → 2
    }));
    prompt.messages.push(Message::user_with(
        "",
        vec![Attachment {
            media_type: "image/png".into(),
            data: AttachmentData::Base64("iVBORw==".into()),
        }],
    )); // attachment → 1000

    assert_eq!(tokenizer.count(""), 0);
    assert_eq!(tokenizer.count("abcde"), 2);
    assert_eq!(
        count_prompt(&tokenizer, &prompt),
        2 + 1 + 3 + 2 + 1000 + 5 * 4
    );
}
