//! Conversations in SQL (F07.3, ADR-0048), tested against SQLite.

use std::path::PathBuf;

use grauwolf_agents::memory::ChatHistoryStore;
use grauwolf_prompt::Message;
use grauwolf_store_sql::SqlChatHistoryStore;

/// URL of a fresh SQLite file under `target/store-sql-tests`.
fn database(name: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/store-sql-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{name}.db"));
    let _ = std::fs::remove_file(&file);
    format!("sqlite://{}", file.display())
}

#[tokio::test]
async fn saves_replaces_and_loads_conversations_across_connections() {
    let url = database("roundtrip");
    let store = SqlChatHistoryStore::connect(&url).await.unwrap();
    let first = [
        Message::user("My name is Arek."),
        Message::assistant("Noted."),
    ];

    store.save("arek", &first).await.unwrap();
    let mut longer = first.to_vec();
    longer.push(Message::user("What is my name?"));
    store.save("arek", &longer).await.unwrap();
    store
        .save("other", &[Message::user("Hello")])
        .await
        .unwrap();
    drop(store);
    let reopened = SqlChatHistoryStore::connect(&url).await.unwrap();

    assert_eq!(reopened.load("arek").await.unwrap(), longer);
    assert_eq!(
        reopened.load("other").await.unwrap(),
        [Message::user("Hello")]
    );
    assert_eq!(
        reopened.load("unknown").await.unwrap(),
        Vec::<Message>::new()
    );
}
