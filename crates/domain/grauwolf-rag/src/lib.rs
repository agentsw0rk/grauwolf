//! Retrieval-Augmented Generation (E08, ADR-0038): embeddings, vector store, search.

use std::sync::{Arc, Mutex};

use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, KnowledgeContext, KnowledgeHit, ToolError};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// Error in embeddings, storage or search.
#[derive(Debug, thiserror::Error)]
pub enum RagError {
    /// The embedder failed.
    #[error("embedding failed: {0}")]
    Embedding(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The vector store failed.
    #[error("vector store failed: {0}")]
    Store(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Vector of a text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Embedding(pub Vec<f32>);

impl Embedding {
    /// Cosine similarity in `[-1, 1]`; 0 for zero vectors (F08.1).
    pub fn cosine(&self, other: &Embedding) -> f32 {
        let dot: f32 = self.0.iter().zip(&other.0).map(|(a, b)| a * b).sum();
        let norm = |values: &[f32]| values.iter().map(|value| value * value).sum::<f32>().sqrt();
        let norms = norm(&self.0) * norm(&other.0);
        if norms == 0.0 { 0.0 } else { dot / norms }
    }
}

/// A section of a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// Unique ID, e.g. `<source>#<number>`.
    pub id: String,
    /// Origin, e.g. the file path.
    pub source: String,
    /// Text of the section.
    pub text: String,
}

/// A hit of the search with its similarity.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredChunk {
    /// The section found.
    pub chunk: Chunk,
    /// Similarity to the query (higher = more similar).
    pub score: f32,
}

/// Port for vector stores (F08.5, F08.6).
pub trait VectorStore: Send + Sync {
    /// Stores `chunk` with its `embedding`.
    fn add<'a>(&'a self, chunk: Chunk, embedding: Embedding)
    -> BoxFuture<'a, Result<(), RagError>>;

    /// The `limit` most similar sections to `query`, most similar first.
    fn search<'a>(
        &'a self,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, RagError>>;
}

/// In-memory vector store; searches linearly.
#[derive(Debug, Default)]
pub struct InMemoryVectorStore(Mutex<Vec<(Chunk, Embedding)>>);

impl VectorStore for InMemoryVectorStore {
    fn add<'a>(
        &'a self,
        chunk: Chunk,
        embedding: Embedding,
    ) -> BoxFuture<'a, Result<(), RagError>> {
        self.0
            .lock()
            .expect("vector store")
            .push((chunk, embedding));
        Box::pin(async { Ok(()) })
    }

    fn search<'a>(
        &'a self,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, RagError>> {
        let mut hits: Vec<ScoredChunk> = self
            .0
            .lock()
            .expect("vector store")
            .iter()
            .map(|(chunk, embedding)| ScoredChunk {
                chunk: chunk.clone(),
                score: query.cosine(embedding),
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Box::pin(async move { Ok(hits) })
    }
}

/// Port for embedding models (F08.2); adapters such as Ollama or OpenAI implement it.
pub trait Embedder: Send + Sync {
    /// One embedding per text, in the same order.
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Embedding>, RagError>>;
}

/// Splits `text` into sections of at most `max_chars` characters: along the paragraphs
/// (blank lines), paragraphs that are too long at whitespace. Short paragraphs are combined.
pub fn chunk_text(source: &str, text: &str, max_chars: usize) -> Vec<Chunk> {
    let length = |text: &str| text.chars().count();
    let mut pieces = Vec::new();
    for paragraph in text
        .split("\n\n")
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if length(paragraph) <= max_chars {
            pieces.push(paragraph.to_owned());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() && length(&current) + 1 + length(word) > max_chars {
                pieces.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        if !current.is_empty() {
            pieces.push(current);
        }
    }
    let mut texts: Vec<String> = Vec::new();
    for piece in pieces {
        match texts.last_mut() {
            Some(last) if length(last) + 2 + length(&piece) <= max_chars => {
                last.push_str("\n\n");
                last.push_str(&piece);
            }
            _ => texts.push(piece),
        }
    }
    texts
        .into_iter()
        .enumerate()
        .map(|(index, text)| Chunk {
            id: format!("{source}#{index}"),
            source: source.to_owned(),
            text,
        })
        .collect()
}

/// Indexes sections and finds those that best match a query (F08.5).
pub struct Retriever {
    embedder: Arc<dyn Embedder>,
    store: Arc<dyn VectorStore>,
    min_similarity: Option<f32>,
}

/// Sections per embedding request.
const BATCH: usize = 16;

impl Retriever {
    /// Retriever with `embedder` and `store`.
    pub fn new(embedder: Arc<dyn Embedder>, store: Arc<dyn VectorStore>) -> Self {
        Self {
            embedder,
            store,
            min_similarity: None,
        }
    }

    /// Drops hits whose similarity is below `min_similarity` (ADR-0113); order and limit stay.
    #[must_use]
    pub fn with_min_similarity(mut self, min_similarity: Option<f32>) -> Self {
        self.min_similarity = min_similarity;
        self
    }

    /// Embeds `chunks` and stores them in memory.
    pub async fn index(&self, chunks: Vec<Chunk>) -> Result<(), RagError> {
        for batch in chunks.chunks(BATCH) {
            let texts: Vec<String> = batch.iter().map(|chunk| chunk.text.clone()).collect();
            let embeddings = self.embedder.embed(&texts).await?;
            for (chunk, embedding) in batch.iter().cloned().zip(embeddings) {
                self.store.add(chunk, embedding).await?;
            }
        }
        Ok(())
    }

    /// The `limit` best matching sections for `query`.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<ScoredChunk>, RagError> {
        let mut embeddings = self.embedder.embed(&[query.to_owned()]).await?;
        let query = embeddings
            .pop()
            .ok_or_else(|| RagError::Embedding("no embedding for the query".into()))?;
        let mut hits = self.store.search(&query, limit).await?;
        if let Some(minimum) = self.min_similarity {
            hits.retain(|hit| hit.score >= minimum);
        }
        Ok(hits)
    }
}

/// Offers the search as the tool `name` (F08.8): argument `query`, result the `limit`
/// best sections as `[<source>] <text>`, separated by blank lines.
pub fn search_tool(retriever: Arc<Retriever>, name: &str, limit: usize) -> Arc<dyn DynTool> {
    Arc::new(SearchTool {
        retriever,
        name: name.to_owned(),
        limit,
        description: "Searches the stored documents and returns the best-matching \
                      sections with their source.",
        with_source: true,
    })
}

/// Source of the facts in long-term memory.
const MEMORY_SOURCE: &str = "memory";

/// Offers remembering as the tool `name` (ADR-0058): argument `fact`; the fact is
/// embedded and stored.
pub fn remember_tool(retriever: Arc<Retriever>, name: &str) -> Arc<dyn DynTool> {
    Arc::new(RememberTool {
        retriever,
        name: name.to_owned(),
    })
}

/// Offers recalling as the tool `name` (ADR-0058): argument `query`, result the `limit`
/// best matching facts, one per line.
pub fn recall_tool(retriever: Arc<Retriever>, name: &str, limit: usize) -> Arc<dyn DynTool> {
    Arc::new(SearchTool {
        retriever,
        name: name.to_owned(),
        limit,
        description: "Recalls remembered facts from earlier conversations that match the query.",
        with_source: false,
    })
}

struct RememberTool {
    retriever: Arc<Retriever>,
    name: String,
}

impl DynTool for RememberTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.name.clone(),
            description: "Remembers a fact permanently, e.g. a preference or detail of the \
                          user, so it can be recalled in later conversations."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "fact": { "type": "string", "description": "The fact as a complete sentence" }
                },
                "required": ["fact"]
            }),
        }
    }

    fn call_json(&self, args: serde_json::Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let fact = args["fact"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArgs {
                    tool: self.name.clone(),
                    reason: "missing field 'fact' (text)".into(),
                })?;
            let chunk = Chunk {
                id: MEMORY_SOURCE.into(),
                source: MEMORY_SOURCE.into(),
                text: fact.trim().to_owned(),
            };
            self.retriever
                .index(vec![chunk])
                .await
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            Ok("Remembered.".into())
        })
    }
}

struct SearchTool {
    retriever: Arc<Retriever>,
    name: String,
    limit: usize,
    description: &'static str,
    /// Hits as `[<source>] <text>` instead of just `<text>`.
    with_source: bool,
}

/// The search of a retriever as the context of an agent (ADR-0113).
struct RetrieverContext {
    retriever: Arc<Retriever>,
    limit: usize,
}

impl KnowledgeContext for RetrieverContext {
    fn retrieve<'a>(
        &'a self,
        query: &'a str,
    ) -> BoxFuture<'a, Result<Vec<KnowledgeHit>, ToolError>> {
        Box::pin(async move {
            let hits = self
                .retriever
                .search(query, self.limit)
                .await
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            Ok(hits
                .into_iter()
                .map(|hit| KnowledgeHit {
                    source: hit.chunk.source,
                    text: hit.chunk.text,
                })
                .collect())
        })
    }
}

impl DynTool for SearchTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn knowledge_context(&self) -> Option<Arc<dyn KnowledgeContext>> {
        self.with_source.then(|| {
            Arc::new(RetrieverContext {
                retriever: Arc::clone(&self.retriever),
                limit: self.limit,
            }) as Arc<dyn KnowledgeContext>
        })
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.name.clone(),
            description: self.description.into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for" }
                },
                "required": ["query"]
            }),
        }
    }

    fn call_json(&self, args: serde_json::Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move {
            let query = args["query"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArgs {
                    tool: self.name.clone(),
                    reason: "missing field 'query' (text)".into(),
                })?;
            let hits = self
                .retriever
                .search(query, self.limit)
                .await
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            if hits.is_empty() {
                return Ok("No matches.".into());
            }
            if !self.with_source {
                return Ok(hits
                    .iter()
                    .map(|hit| hit.chunk.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"));
            }
            Ok(hits
                .iter()
                .map(|hit| format!("[{}] {}", hit.chunk.source, hit.chunk.text))
                .collect::<Vec<_>>()
                .join("\n\n"))
        })
    }
}
