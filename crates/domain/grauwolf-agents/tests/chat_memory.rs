//! Chat memory: histories per conversation ID (F07.1, F07.2, ADR-0037).

use futures_executor::block_on;
use grauwolf_agents::memory::{ChatHistoryStore, MemoryChatHistoryStore};
use grauwolf_prompt::Message;

#[test]
fn keeps_the_latest_history_per_conversation() {
    let store = MemoryChatHistoryStore::default();

    block_on(store.save("anna", &[Message::user("Hello"), Message::assistant("Hi")])).unwrap();
    block_on(store.save("ben", &[Message::user("Hey")])).unwrap();

    assert_eq!(
        block_on(store.load("anna")).unwrap(),
        [Message::user("Hello"), Message::assistant("Hi")]
    );
    assert_eq!(block_on(store.load("nobody")).unwrap(), []);
}

#[test]
fn builds_conversation_keys_that_keep_users_apart() {
    use grauwolf_agents::memory::conversation_key;

    assert_eq!(conversation_key(None, "c-1").unwrap(), "c-1");
    assert_eq!(
        conversation_key(Some("anna"), "c-1").unwrap(),
        "users/616e6e61/c-1"
    );
    assert_eq!(
        conversation_key(Some("a@b.de"), "c_1").unwrap(),
        "users/6140622e6465/c_1"
    );
    for id in ["", "users/616e6e61/c-1", "a.b", "ä"] {
        assert!(conversation_key(None, id).is_err(), "{id:?}");
    }
    assert!(conversation_key(Some(""), "c-1").is_err());
    assert!(conversation_key(Some(&"x".repeat(129)), "c-1").is_err());
}
