//! Long-term memory of a spec across runs (ADR-0058).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_rag::{Embedder, Embedding, RagError};
use grauwolf_spec::{LongTermMemorySpec, ModelSpec};
use serde_json::json;

use crate::long_term_memory::LongTermMemory;

/// Counts the words of a fixed vocabulary – enough to test similarity.
struct WordCounter;

impl Embedder for WordCounter {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        let embeddings = texts
            .iter()
            .map(|text| {
                let text = text.to_lowercase();
                Embedding(
                    ["cat", "rust", "holiday"]
                        .iter()
                        .map(|word| text.matches(word).count() as f32)
                        .collect(),
                )
            })
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }
}

/// Tools `[remember, recall]` for the shared area of `memory`.
async fn shared_tools(
    memory: &LongTermMemorySpec,
    base_dir: &std::path::Path,
    embedder: Arc<dyn Embedder>,
) -> Result<[Arc<dyn grauwolf_tools::DynTool>; 2], String> {
    Ok(LongTermMemory::open(memory, base_dir, embedder)
        .await?
        .tools(None))
}

fn memory() -> LongTermMemorySpec {
    LongTermMemorySpec {
        store: Some("state/memory.json".into()),
        vector_store: None,
        embedding_model: ModelSpec {
            provider: "ollama".into(),
            id: "nomic-embed-text".into(),
            fallbacks: Vec::new(),
            temperature: None,
            think: None,
        },
        remember_tool: "remember".into(),
        recall_tool: "recall".into(),
        top_k: 1,
        auto_recall: false,
        auto_remember: false,
        tools: true,
        remember_compressed: false,
    }
}

/// Own directory in the temporary directory of the container (not in the mounted
/// workspace, where freshly written files are sometimes not visible right away).
fn base(name: &str) -> grauwolf_test::TestDir {
    grauwolf_test::TestDir::new(name)
}

#[tokio::test]
async fn recalls_facts_that_an_earlier_run_remembered() {
    let base = base("long-term-memory");
    let [remember, _] = shared_tools(&memory(), &base, Arc::new(WordCounter))
        .await
        .unwrap();
    remember
        .call_json(json!({ "fact": "The user's cat is named Mia." }))
        .await
        .unwrap();
    remember
        .call_json(json!({ "fact": "The next holiday goes to Cornwall." }))
        .await
        .unwrap();
    drop(remember);

    let [_, recall] = shared_tools(&memory(), &base, Arc::new(WordCounter))
        .await
        .unwrap();
    let answer = recall
        .call_json(json!({ "query": "What is the cat called?" }))
        .await
        .unwrap();

    assert_eq!(answer, "The user's cat is named Mia.");
    assert!(base.join("state/memory.json").is_file());
}

#[tokio::test]
async fn keeps_a_fact_only_once_when_it_is_remembered_again() {
    let base = base("long-term-memory-duplicates");
    let memory = LongTermMemorySpec {
        top_k: 3,
        ..memory()
    };
    let [remember, recall] = shared_tools(&memory, &base, Arc::new(WordCounter))
        .await
        .unwrap();
    for fact in [
        "The cat is named Mia.",
        " The cat is named Mia. ",
        "Rust is fast.",
    ] {
        remember.call_json(json!({ "fact": fact })).await.unwrap();
    }

    let answer = recall.call_json(json!({ "query": "cat" })).await.unwrap();

    assert_eq!(answer, "The cat is named Mia.\nRust is fast.");
}

#[tokio::test]
async fn refuses_a_memory_that_another_embedding_model_wrote() {
    let base = base("long-term-memory-model");
    let [remember, _] = shared_tools(&memory(), &base, Arc::new(WordCounter))
        .await
        .unwrap();
    remember
        .call_json(json!({ "fact": "The cat is named Mia." }))
        .await
        .unwrap();
    let other = LongTermMemorySpec {
        embedding_model: ModelSpec {
            provider: "openai".into(),
            id: "text-embedding-3-small".into(),
            fallbacks: Vec::new(),
            temperature: None,
            think: None,
        },
        ..memory()
    };

    let error = shared_tools(&other, &base, Arc::new(WordCounter))
        .await
        .err()
        .unwrap();

    assert!(error.contains("ollama:nomic-embed-text"), "{error}");
    assert!(error.contains("openai:text-embedding-3-small"), "{error}");
}

#[tokio::test]
async fn keeps_the_facts_of_each_user_apart() {
    let base = base("long-term-memory-users");
    let opened = LongTermMemory::open(&memory(), &base, Arc::new(WordCounter))
        .await
        .unwrap();
    let [anna_remember, anna_recall] = opened.tools(Some("anna"));
    let [_, bob_recall] = opened.tools(Some("bob"));
    let [_, shared_recall] = opened.tools(None);
    anna_remember
        .call_json(json!({ "fact": "Anna's cat is named Mia." }))
        .await
        .unwrap();

    let query = json!({ "query": "What is the cat called?" });
    let anna = anna_recall.call_json(query.clone()).await.unwrap();
    let bob = bob_recall.call_json(query.clone()).await.unwrap();
    let shared = shared_recall.call_json(query.clone()).await.unwrap();
    let [_, reopened] = LongTermMemory::open(&memory(), &base, Arc::new(WordCounter))
        .await
        .unwrap()
        .tools(Some("anna"));

    assert_eq!(anna, "Anna's cat is named Mia.");
    assert_eq!(bob, "No matches.");
    assert_eq!(shared, "No matches.");
    assert_eq!(
        reopened.call_json(query).await.unwrap(),
        "Anna's cat is named Mia."
    );
}

#[tokio::test]
async fn shares_one_memory_between_everyone_who_opens_the_same_file() {
    let base = base("long-term-memory-shared");
    let spec = memory();
    let open = || LongTermMemory::open(&spec, &base, Arc::new(WordCounter));
    let first = open().await.unwrap();
    let second = open().await.unwrap();

    let [remember, _] = first.tools(Some("anna"));
    remember
        .call_json(json!({ "fact": "Anna's cat is named Mia." }))
        .await
        .unwrap();
    let [_, recall] = second.tools(Some("anna"));
    let answer = recall.call_json(json!({ "query": "cat" })).await.unwrap();

    assert_eq!(answer, "Anna's cat is named Mia.");
}

/// Automatic memory (ADR-0069): facts per area via the port `FactMemory`.
#[tokio::test]
async fn offers_the_facts_of_one_user_for_automatic_memory() {
    let base = base("fact-memory");
    let memory = LongTermMemory::open(&memory(), &base, Arc::new(WordCounter))
        .await
        .unwrap();
    let anna = memory.facts(Some("anna"));

    anna.remember(&["Anna's cat is named Mia.".to_owned()])
        .await
        .unwrap();
    let recalled = anna.recall("What is the cat called?").await.unwrap();
    let for_bob = memory
        .facts(Some("bob"))
        .recall("What is the cat called?")
        .await
        .unwrap();

    assert_eq!(recalled, ["Anna's cat is named Mia."]);
    assert!(for_bob.is_empty(), "{for_bob:?}");
}
