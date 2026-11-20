//! MockLlmClient (F12.1).

use futures_executor::block_on;
use grauwolf_llm::LlmClient;
use grauwolf_prompt::{LlModel, Message, Prompt};
use grauwolf_test::MockLlmClient;

fn ask(client: &MockLlmClient, text: &str) -> Message {
    let prompt = Prompt::builder("p")
        .system("Hello as system")
        .user(text)
        .build();
    block_on(client.execute(&prompt, &LlModel::new("mock", "m"), &[])).unwrap()
}

#[test]
fn answers_by_the_first_rule_matching_the_last_user_message() {
    let client = MockLlmClient::new("Standard")
        .answer_when_contains("weather", "Sunny")
        .answer_when_contains("weather tomorrow", "Rain")
        .answer_when_contains("Hello", "Hi");

    assert_eq!(
        ask(&client, "What will the weather be tomorrow?"),
        Message::assistant("Sunny")
    );
    assert_eq!(ask(&client, "Hello!"), Message::assistant("Hi"));
    assert_eq!(
        ask(&client, "Something else"),
        Message::assistant("Standard")
    );
}

#[test]
fn calls_tools_and_reacts_to_their_results() {
    let client = MockLlmClient::new("?")
        .call_tool_when_contains("2+3", "add", serde_json::json!({ "a": 2, "b": 3 }))
        .answer_when_contains("5.0", "The result is 5.");
    let model = LlModel::new("mock", "m");
    let mut prompt = Prompt::builder("p").user("What is 2+3?").build();

    let call = block_on(client.execute(&prompt, &model, &[])).unwrap();
    assert_eq!(
        call,
        Message::tool_call("add", "add", serde_json::json!({ "a": 2, "b": 3 }))
    );

    prompt.messages.push(call);
    prompt
        .messages
        .push(Message::tool_result("add", "add", "5.0"));
    let answer = block_on(client.execute(&prompt, &model, &[])).unwrap();
    assert_eq!(answer, Message::assistant("The result is 5."));
}

#[test]
fn calls_several_tools_in_one_answer_with_numbered_ids() {
    let mock = MockLlmClient::new("?").call_tools_when_contains(
        "both",
        [
            ("add", serde_json::json!({ "a": 2, "b": 3 })),
            ("add", serde_json::json!({ "a": 4, "b": 5 })),
        ],
    );
    let prompt = grauwolf_prompt::Prompt::builder("p")
        .user("Calculate both")
        .build();

    let answer = futures_executor::block_on(grauwolf_llm::LlmClient::execute(
        &mock,
        &prompt,
        &grauwolf_prompt::LlModel::new("mock", "m"),
        &[],
    ))
    .unwrap();

    let grauwolf_prompt::Message::Assistant(answer) = answer else {
        panic!("answer expected: {answer:?}");
    };
    let ids: Vec<&str> = answer.tool_calls().map(|call| call.id.as_str()).collect();
    assert_eq!(ids, ["add_1", "add_2"]);
}

#[test]
fn matches_everything_since_the_last_answer_or_the_answer_itself() {
    let client = MockLlmClient::new("Standard")
        .answer_when_contains("I add", "Tool call")
        .answer_when_contains("2+3?\nThink about it", "I add 2 and 3");
    let model = LlModel::new("mock", "m");
    let mut prompt = Prompt::builder("p")
        .user("What is 2+3?")
        .user("Think about it.")
        .build();

    let thought = block_on(client.execute(&prompt, &model, &[])).unwrap();
    prompt.messages.push(thought.clone());
    let action = block_on(client.execute(&prompt, &model, &[])).unwrap();

    assert_eq!(thought, Message::assistant("I add 2 and 3"));
    assert_eq!(action, Message::assistant("Tool call"));
}
