//! User input/output as tools (F03.13, ADR-0046).

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use grauwolf_builtin_tools::{UserIo, UserTools};
use serde_json::json;

/// Records outputs and answers questions with fixed answers.
#[derive(Default)]
struct Scripted {
    said: Mutex<Vec<String>>,
}

impl UserIo for Scripted {
    fn say(&self, message: &str) {
        self.said.lock().unwrap().push(message.to_owned());
    }

    fn ask<'a>(&'a self, question: &'a str) -> BoxFuture<'a, Result<String, String>> {
        self.said.lock().unwrap().push(format!("? {question}"));
        Box::pin(async { Ok("Blue".to_owned()) })
    }
}

#[tokio::test]
async fn talks_to_the_user_through_the_port() {
    let io = Arc::new(Scripted::default());
    let tools = UserTools::new(io.clone())
        .tools(["say_to_user", "ask_user"])
        .unwrap();
    let (say, ask) = (&tools[0], &tools[1]);

    let said = say
        .call_json(json!({ "message": "Still working on it." }))
        .await
        .unwrap();
    let answer = ask
        .call_json(json!({ "question": "Which color?" }))
        .await
        .unwrap();

    assert_eq!(said, "shown");
    assert_eq!(answer, "Blue");
    assert_eq!(
        *io.said.lock().unwrap(),
        ["Still working on it.", "? Which color?"]
    );
    assert!(UserTools::new(io).tools(["shout"]).is_err());
}
