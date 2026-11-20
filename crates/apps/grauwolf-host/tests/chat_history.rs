//! File store for conversation histories (ADR-0037, ADR-0060).

use grauwolf_agents::memory::{ChatHistoryStore, conversation_key};
use grauwolf_host::memory::FileChatHistoryStore;
use grauwolf_prompt::Message;

#[tokio::test]
async fn keeps_conversations_of_users_in_their_own_directory() {
    let dir = grauwolf_test::TestDir::new("chat-history");
    let store = FileChatHistoryStore::new(&dir);
    let anna = conversation_key(Some("anna"), "c-1").unwrap();

    store
        .save(&anna, &[Message::user("I am Anna.")])
        .await
        .unwrap();

    assert_eq!(
        store.load(&anna).await.unwrap(),
        [Message::user("I am Anna.")]
    );
    assert!(dir.join("users/616e6e61/c-1.json").is_file());
    assert!(store.load("c-1").await.unwrap().is_empty());
    for key in [
        "users/../c-1",
        "users/ZZ/c-1",
        "users/616e6e61/a.b",
        "users/616e6e61",
    ] {
        assert!(store.load(key).await.is_err(), "{key}");
    }
}
