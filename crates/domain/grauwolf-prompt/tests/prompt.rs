//! Prompt and message model (E01).

use grauwolf_prompt::{Message, Prompt};

#[test]
fn builder_collects_messages_in_order() {
    let prompt = Prompt::builder("echo")
        .system("Be brief.")
        .user("Hello")
        .build();

    assert_eq!(prompt.id, "echo");
    assert_eq!(
        prompt.messages,
        vec![Message::system("Be brief."), Message::user("Hello")]
    );
}

#[test]
fn messages_have_a_role_tagged_json_form() {
    let message = Message::assistant("Hello back");

    let json = serde_json::to_value(&message).unwrap();

    assert_eq!(
        json,
        serde_json::json!({ "role": "assistant", "parts": [ { "type": "text", "text": "Hello back" } ] })
    );
    assert_eq!(serde_json::from_value::<Message>(json).unwrap(), message);
}

#[test]
fn tool_calls_and_results_have_a_stable_json_form() {
    let call = Message::tool_call("call_1", "add", serde_json::json!({ "a": 2, "b": 3 }));
    let result = Message::tool_result("call_1", "add", "5");

    assert_eq!(
        serde_json::to_value(&call).unwrap(),
        serde_json::json!({ "role": "assistant", "parts": [
            { "type": "tool_call", "id": "call_1", "tool": "add", "args": { "a": 2, "b": 3 } }
        ] })
    );
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::json!({ "role": "tool_result", "id": "call_1", "tool": "add", "content": "5" })
    );
}

#[test]
fn prompts_carry_a_tool_choice_that_defaults_to_auto_and_survives_serialization() {
    use grauwolf_prompt::{LlmParams, ToolChoice};

    let fresh = Prompt::builder("p").user("Hello").build();
    let stored: Prompt = serde_json::from_str(r#"{ "id": "p", "messages": [] }"#).unwrap();
    let mut forced = fresh.clone();
    forced.params = LlmParams {
        tool_choice: ToolChoice::Tool("add".into()),
    };
    let roundtrip: Prompt = serde_json::from_str(&serde_json::to_string(&forced).unwrap()).unwrap();

    assert_eq!(fresh.params.tool_choice, ToolChoice::Auto);
    assert_eq!(stored.params, LlmParams::default());
    assert_eq!(roundtrip, forced);
}
