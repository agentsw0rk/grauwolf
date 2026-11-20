//! Reads the knowledge sources of a spec, indexes them and offers them as a search tool (ADR-0038).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use grauwolf_rag::{
    Chunk, Embedder, Embedding, InMemoryVectorStore, Retriever, VectorStore, chunk_text,
    search_tool as tool,
};
use grauwolf_spec::KnowledgeSpec;
use grauwolf_tools::DynTool;

use crate::urls::{UrlLoad, UrlValidators, load_url};
use serde::{Deserialize, Serialize};

/// Reads all sources from `knowledge` (files, directories recursively; relative to `base_dir`),
/// embeds their chunks with `embedder` and returns the search tool. Chunks name the path
/// relative to `base_dir` as source. With `knowledge.index` only new or changed files are
/// embedded, the rest comes from the index (ADR-0057).
pub async fn search_tool(
    knowledge: &KnowledgeSpec,
    base_dir: &Path,
    embedder: Arc<dyn Embedder>,
) -> Result<Arc<dyn DynTool>, String> {
    if uses_postgres(knowledge) {
        return Err(
            "knowledge.store kind `postgres` needs the spec name and the environment: use \
             `knowledge::search_tool_in`"
                .into(),
        );
    }
    let model = format!(
        "{}:{}",
        knowledge.embedding_model.provider, knowledge.embedding_model.id
    );
    let index_file = knowledge.index.as_ref().map(|index| base_dir.join(index));
    let previous = match &index_file {
        Some(file) => read_index(file, &model, knowledge.chunk_size).await,
        None => IndexFile::default(),
    };
    let mut current = IndexFile {
        model,
        chunk_size: knowledge.chunk_size,
        files: BTreeMap::new(),
    };
    let mut pending: Vec<(String, Vec<Chunk>)> = Vec::new();
    for source in &knowledge.sources {
        for file in files(&base_dir.join(source)).await? {
            let text = tokio::fs::read_to_string(&file)
                .await
                .map_err(|error| format!("{}: {error}", file.display()))?;
            let name = relative_name(&file, base_dir);
            stage(
                &previous,
                &mut current,
                &mut pending,
                (name, text, None),
                knowledge.chunk_size,
            );
        }
    }
    for url in &knowledge.urls {
        let indexed = previous.files.get(url);
        let known = indexed.and_then(|file| file.validators.as_ref());
        match load_url(url, known, indexed.is_some()).await? {
            UrlLoad::Unchanged => {
                if let Some(file) = indexed {
                    current.files.insert(url.clone(), file.clone());
                }
            }
            UrlLoad::Fetched { text, validators } => stage(
                &previous,
                &mut current,
                &mut pending,
                (url.clone(), text, Some(validators)),
                knowledge.chunk_size,
            ),
        }
    }
    // Embed only new or changed files, in batches like the retriever.
    for (name, chunks) in pending {
        let mut indexed = Vec::with_capacity(chunks.len());
        for batch in chunks.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch.iter().map(|chunk| chunk.text.clone()).collect();
            let embeddings = embedder
                .embed(&texts)
                .await
                .map_err(|error| error.to_string())?;
            indexed.extend(
                batch
                    .iter()
                    .cloned()
                    .zip(embeddings)
                    .map(|(chunk, embedding)| IndexedChunk { chunk, embedding }),
            );
        }
        if let Some(file) = current.files.get_mut(&name) {
            file.chunks = indexed;
        }
    }
    if let Some(file) = &index_file {
        write_index(file, &current).await?;
    }
    let store = Arc::new(InMemoryVectorStore::default());
    for indexed in current.files.into_values().flat_map(|file| file.chunks) {
        store
            .add(indexed.chunk, indexed.embedding)
            .await
            .map_err(|error| error.to_string())?;
    }
    let retriever = Retriever::new(embedder, store).with_min_similarity(knowledge.min_similarity);
    Ok(tool(Arc::new(retriever), &knowledge.tool, knowledge.top_k))
}

/// Registers one document in `current`: unchanged content (same hash) keeps its chunks from the
/// index, new or changed content is chunked and queued for embedding.
fn stage(
    previous: &IndexFile,
    current: &mut IndexFile,
    pending: &mut Vec<(String, Vec<Chunk>)>,
    (name, text, validators): (String, String, Option<UrlValidators>),
    chunk_size: usize,
) {
    let hash = format!("{:016x}", fnv1a(text.as_bytes()));
    match previous.files.get(&name) {
        Some(indexed) if indexed.hash == hash => {
            current.files.insert(
                name,
                IndexedFile {
                    validators,
                    ..indexed.clone()
                },
            );
        }
        _ => {
            let chunks = chunk_text(&name, &text, chunk_size);
            current.files.insert(
                name.clone(),
                IndexedFile {
                    hash,
                    validators,
                    chunks: Vec::new(),
                },
            );
            pending.push((name, chunks));
        }
    }
}

/// Does the knowledge live in PostgreSQL (ADR-0106)?
fn uses_postgres(knowledge: &KnowledgeSpec) -> bool {
    knowledge
        .store
        .as_ref()
        .is_some_and(|store| store.kind == grauwolf_spec::KnowledgeStoreKind::Postgres)
}

/// Like [`search_tool`], and the store kind `postgres` (ADR-0106) works: the URL comes from the
/// environment variable `knowledge.store.url_env` read from `env`, the collection is
/// `<agent_name>/<knowledge tool>`.
pub async fn search_tool_in(
    agent_name: &str,
    knowledge: &KnowledgeSpec,
    base_dir: &Path,
    embedder: Arc<dyn Embedder>,
    env: &dyn crate::config::Env,
) -> Result<Arc<dyn DynTool>, String> {
    if !uses_postgres(knowledge) {
        return search_tool(knowledge, base_dir, embedder).await;
    }
    #[cfg(feature = "pgvector")]
    {
        let variable = knowledge
            .store
            .as_ref()
            .and_then(|store| store.url_env.as_deref())
            .ok_or("knowledge.store kind `postgres` needs `url_env`")?;
        let db = crate::pgvector::connect(variable, env).await?;
        search_tool_on(agent_name, knowledge, base_dir, embedder, db).await
    }
    #[cfg(not(feature = "pgvector"))]
    {
        let _ = (agent_name, env, embedder, base_dir);
        Err("this build has no PostgreSQL store (feature `pgvector`)".into())
    }
}

/// Indexes the sources into the collection `<agent_name>/<knowledge tool>` of `db` (only new or
/// changed chunks are embedded, ADR-0057) and returns the search tool on that collection.
#[cfg(feature = "pgvector")]
pub async fn search_tool_on(
    agent_name: &str,
    knowledge: &KnowledgeSpec,
    base_dir: &Path,
    embedder: Arc<dyn Embedder>,
    db: Arc<dyn grauwolf_store_pgvector::ChunkDb>,
) -> Result<Arc<dyn DynTool>, String> {
    let model = format!(
        "{}:{}",
        knowledge.embedding_model.provider, knowledge.embedding_model.id
    );
    let collection = format!("{agent_name}/{}", knowledge.tool);
    let mut sources = Vec::new();
    for source in &knowledge.sources {
        for file in files(&base_dir.join(source)).await? {
            let text = tokio::fs::read_to_string(&file)
                .await
                .map_err(|error| format!("{}: {error}", file.display()))?;
            let name = relative_name(&file, base_dir);
            sources.push(grauwolf_store_pgvector::SourceChunks {
                chunks: chunk_text(&name, &text, knowledge.chunk_size),
                source: name,
            });
        }
    }
    let known = db
        .url_validators(&collection)
        .await
        .map_err(|error| error.to_string())?;
    let mut kept = Vec::new();
    let mut validators = grauwolf_store_pgvector::UrlValidatorMap::new();
    for url in &knowledge.urls {
        let indexed = known.get(url).cloned();
        let sent = indexed.as_ref().map(|found| UrlValidators {
            etag: found.etag.clone(),
            last_modified: found.last_modified.clone(),
        });
        match load_url(url, sent.as_ref(), indexed.is_some()).await? {
            UrlLoad::Unchanged => {
                kept.push(url.clone());
                validators.extend(indexed.map(|found| (url.clone(), found)));
            }
            UrlLoad::Fetched {
                text,
                validators: fetched,
            } => {
                sources.push(grauwolf_store_pgvector::SourceChunks {
                    chunks: chunk_text(url, &text, knowledge.chunk_size),
                    source: url.clone(),
                });
                validators.insert(
                    url.clone(),
                    grauwolf_store_pgvector::UrlValidators {
                        etag: fetched.etag,
                        last_modified: fetched.last_modified,
                    },
                );
            }
        }
    }
    grauwolf_store_pgvector::sync_collection_with(
        db.as_ref(),
        embedder.as_ref(),
        &collection,
        &model,
        &sources,
        &kept,
        &validators,
    )
    .await
    .map_err(|error| error.to_string())?;
    let store = Arc::new(grauwolf_store_pgvector::PgChunkStore::new(db, collection));
    let retriever = Retriever::new(embedder, store).with_min_similarity(knowledge.min_similarity);
    Ok(tool(Arc::new(retriever), &knowledge.tool, knowledge.top_k))
}

/// Path of `file` relative to `base_dir`, with `/` separators.
fn relative_name(file: &Path, base_dir: &Path) -> String {
    file.strip_prefix(base_dir)
        .unwrap_or(file)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Chunks per embedding request.
const EMBED_BATCH: usize = 16;

/// Content of the index file (ADR-0057).
#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    /// `<provider>:<model>` of the embeddings.
    model: String,
    chunk_size: usize,
    /// Source (path relative to the spec) → checksum and chunks.
    files: BTreeMap<String, IndexedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedFile {
    hash: String,
    /// Cache validators of a URL source (ADR-0113).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    validators: Option<UrlValidators>,
    chunks: Vec<IndexedChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedChunk {
    chunk: Chunk,
    embedding: Embedding,
}

/// Index from `file`; if it is missing, unreadable or does not match the model and
/// chunk size, the index starts empty.
async fn read_index(file: &Path, model: &str, chunk_size: usize) -> IndexFile {
    let Ok(bytes) = tokio::fs::read(file).await else {
        return IndexFile::default();
    };
    match serde_json::from_slice::<IndexFile>(&bytes) {
        Ok(index) if index.model == model && index.chunk_size == chunk_size => index,
        _ => IndexFile::default(),
    }
}

/// Writes the index via a temporary file so that an abort leaves no half-written index.
async fn write_index(file: &Path, index: &IndexFile) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{}: {error}", file.display());
    if let Some(dir) = file.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(failed)?;
    }
    let partial = file.with_extension("json.partial");
    let bytes = serde_json::to_vec(index).map_err(|error| error.to_string())?;
    tokio::fs::write(&partial, bytes).await.map_err(failed)?;
    tokio::fs::rename(&partial, file).await.map_err(failed)
}

/// FNV-1a (64 bit): stable checksum across Rust versions.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// `path` itself or all files below it, sorted.
pub(crate) async fn files(path: &Path) -> Result<Vec<PathBuf>, String> {
    let describe = |error: std::io::Error| format!("{}: {error}", path.display());
    if !tokio::fs::metadata(path).await.map_err(describe)?.is_dir() {
        return Ok(vec![path.to_owned()]);
    }
    let mut found = Vec::new();
    let mut pending = vec![path.to_owned()];
    while let Some(dir) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await.map_err(describe)?;
        while let Some(entry) = entries.next_entry().await.map_err(describe)? {
            if entry.file_type().await.map_err(describe)?.is_dir() {
                pending.push(entry.path());
            } else {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    Ok(found)
}
