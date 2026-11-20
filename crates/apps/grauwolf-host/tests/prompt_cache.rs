//! Response cache as files (ADR-0086).

use grauwolf_host::cache::FilePromptCache;
use grauwolf_llm::PromptCache;
use grauwolf_prompt::Message;

#[tokio::test]
async fn keeps_answers_in_files_across_instances() {
    let dir = grauwolf_test::TestDir::new("prompt-cache");
    let answer = Message::assistant("Paris");

    FilePromptCache::new(&dir)
        .put("{\"question\":\"Capital?\"}", &answer)
        .await;
    let reopened = FilePromptCache::new(&dir);
    let found = reopened.get("{\"question\":\"Capital?\"}").await;
    let other = reopened.get("{\"question\":\"River?\"}").await;
    // A broken file counts as a miss, not as an error.
    for entry in std::fs::read_dir(&dir).unwrap() {
        std::fs::write(entry.unwrap().path(), "broken").unwrap();
    }
    let broken = reopened.get("{\"question\":\"Capital?\"}").await;

    assert_eq!(found, Some(answer));
    assert_eq!(other, None);
    assert_eq!(broken, None);
}
