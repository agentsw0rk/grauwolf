//! Embeddings and vector search (F08.1, F08.5, F08.6).

use futures_executor::block_on;
use grauwolf_rag::{Chunk, Embedding, InMemoryVectorStore, VectorStore};

fn chunk(id: &str) -> Chunk {
    Chunk {
        id: id.into(),
        source: "notes.md".into(),
        text: format!("Text von {id}"),
    }
}

#[test]
fn measures_cosine_similarity() {
    let a = Embedding(vec![1.0, 0.0]);

    assert!((a.cosine(&Embedding(vec![2.0, 0.0])) - 1.0).abs() < 1e-6);
    assert!(a.cosine(&Embedding(vec![0.0, 3.0])).abs() < 1e-6);
    assert_eq!(a.cosine(&Embedding(vec![0.0, 0.0])), 0.0);
}

#[test]
fn finds_the_most_similar_chunks_first() {
    let store = InMemoryVectorStore::default();
    block_on(store.add(chunk("rust"), Embedding(vec![1.0, 0.1]))).unwrap();
    block_on(store.add(chunk("katzen"), Embedding(vec![0.0, 1.0]))).unwrap();
    block_on(store.add(chunk("cargo"), Embedding(vec![0.8, 0.3]))).unwrap();

    let found = block_on(store.search(&Embedding(vec![1.0, 0.0]), 2)).unwrap();

    let ids: Vec<&str> = found.iter().map(|hit| hit.chunk.id.as_str()).collect();
    assert_eq!(ids, ["rust", "cargo"]);
    assert!(found[0].score > found[1].score);
}
