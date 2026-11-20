//! Sections, embedder and retriever (F08.2, F08.4, F08.5).

use std::sync::Arc;

use futures_executor::block_on;
use futures_util::future::BoxFuture;
use grauwolf_rag::{Embedder, Embedding, InMemoryVectorStore, RagError, Retriever, chunk_text};

/// Counts words of a fixed vocabulary – enough to test similarity.
struct WordCounter;

const VOCABULARY: [&str; 4] = ["rust", "cat", "cargo", "milk"];

impl Embedder for WordCounter {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>> {
        let embeddings = texts
            .iter()
            .map(|text| {
                let text = text.to_lowercase();
                Embedding(
                    VOCABULARY
                        .iter()
                        .map(|word| text.matches(word).count() as f32)
                        .collect(),
                )
            })
            .collect();
        Box::pin(async move { Ok(embeddings) })
    }
}

#[test]
fn splits_text_into_chunks_along_paragraphs() {
    let text = "First paragraph about Rust.\n\nSecond paragraph about Cargo.\n\nThird paragraph about cats, who like to drink milk.";

    let chunks = chunk_text("notes.md", text, 60);

    let texts: Vec<&str> = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "First paragraph about Rust.\n\nSecond paragraph about Cargo.",
            "Third paragraph about cats, who like to drink milk.",
        ]
    );
    assert_eq!(chunks[1].id, "notes.md#1");
    assert_eq!(chunks[1].source, "notes.md");
}

#[test]
fn splits_paragraphs_longer_than_the_limit_at_whitespace() {
    let chunks = chunk_text("a.txt", "one two three four five", 10);

    let texts: Vec<&str> = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
    assert_eq!(texts, ["one two", "three four", "five"]);
}

#[test]
fn indexes_documents_and_finds_the_relevant_chunks() {
    let retriever = Retriever::new(
        Arc::new(WordCounter),
        Arc::new(InMemoryVectorStore::default()),
    );
    let mut chunks = chunk_text("animals.md", "The cat drinks milk.", 100);
    chunks.extend(chunk_text("code.md", "Rust builds with Cargo.", 100));
    block_on(retriever.index(chunks)).unwrap();

    let hits = block_on(retriever.search("What does the cat drink?", 1)).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk.source, "animals.md");
}

#[test]
fn offers_the_search_as_a_tool_that_returns_the_hits_with_their_source() {
    let retriever = Arc::new(Retriever::new(
        Arc::new(WordCounter),
        Arc::new(InMemoryVectorStore::default()),
    ));
    block_on(
        retriever.index(
            [
                chunk_text("animals.md", "The cat drinks milk.", 100),
                chunk_text("code.md", "Rust builds with Cargo.", 100),
            ]
            .concat(),
        ),
    )
    .unwrap();
    let tool = grauwolf_rag::search_tool(retriever, "search_docs", 1);

    let descriptor = tool.descriptor();
    let answer =
        block_on(tool.call_json(serde_json::json!({ "query": "What does the cat drink?" })))
            .unwrap();

    assert_eq!(descriptor.name, "search_docs");
    assert_eq!(
        descriptor.parameters["required"],
        serde_json::json!(["query"])
    );
    assert_eq!(answer, "[animals.md] The cat drinks milk.");
}

#[test]
fn remembers_facts_that_recall_finds_later() {
    let retriever = Arc::new(Retriever::new(
        Arc::new(WordCounter),
        Arc::new(InMemoryVectorStore::default()),
    ));
    let remember = grauwolf_rag::remember_tool(Arc::clone(&retriever), "remember");
    let recall = grauwolf_rag::recall_tool(retriever, "recall", 1);

    let stored =
        block_on(remember.call_json(
            serde_json::json!({ "fact": "The user's cat is named Mia and drinks milk." }),
        ))
        .unwrap();
    block_on(remember.call_json(serde_json::json!({ "fact": "The user writes Rust with Cargo." })))
        .unwrap();
    let answer =
        block_on(recall.call_json(serde_json::json!({ "query": "What is the cat called?" })))
            .unwrap();

    assert_eq!(stored, "Remembered.");
    assert_eq!(
        remember.descriptor().parameters["required"],
        serde_json::json!(["fact"])
    );
    assert_eq!(recall.descriptor().name, "recall");
    assert_eq!(answer, "The user's cat is named Mia and drinks milk.");
}

#[test]
fn drops_hits_below_the_minimum_similarity_and_keeps_the_order() {
    let search = |minimum: f32| {
        let retriever = Retriever::new(
            Arc::new(WordCounter),
            Arc::new(InMemoryVectorStore::default()),
        )
        .with_min_similarity(Some(minimum));
        block_on(retriever.index(chunk_text("a.md", "Rust cargo.\n\nRust.\n\nMilk.", 11))).unwrap();
        let hits = block_on(retriever.search("rust", 5)).unwrap();
        hits.into_iter()
            .map(|hit| hit.chunk.text)
            .collect::<Vec<_>>()
    };

    assert_eq!(search(0.5), ["Rust.", "Rust cargo."]);
    assert_eq!(search(0.8), ["Rust."]);
}
