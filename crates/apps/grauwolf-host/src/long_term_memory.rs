//! Long-term memory of a spec (ADR-0058, ADR-0059): facts are stored with their embeddings and
//! optionally their user in a JSON file and are offered via the tools `remember` and `recall`
//! – separately per user.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use futures_util::future::BoxFuture;
use grauwolf_agents::memory::{FactMemory, MemoryError};
use grauwolf_rag::{Chunk, Embedder, Embedding, RagError, Retriever, ScoredChunk, VectorStore};
use grauwolf_spec::LongTermMemorySpec;
use grauwolf_tools::DynTool;
use serde::{Deserialize, Serialize};

/// An opened memory; returns the tools per user.
pub struct LongTermMemory {
    backend: Backend,
    embedder: Arc<dyn Embedder>,
    remember_tool: String,
    recall_tool: String,
    top_k: usize,
}

/// Where the facts live.
enum Backend {
    /// JSON file (ADR-0058).
    File(Arc<MemoryStore>),
    /// PostgreSQL with pgvector (ADR-0106).
    #[cfg(feature = "pgvector")]
    Postgres {
        db: Arc<dyn grauwolf_store_pgvector::MemoryDb>,
        agent: String,
        policy: grauwolf_store_pgvector::MemoryPolicy,
    },
}

impl Backend {
    /// The facts of `user` as a vector store.
    fn area(&self, user: Option<&str>) -> Arc<dyn VectorStore> {
        match self {
            Backend::File(file) => Arc::new(Area {
                file: Arc::clone(file),
                user: user.map(str::to_owned),
            }),
            #[cfg(feature = "pgvector")]
            Backend::Postgres { db, agent, policy } => {
                Arc::new(grauwolf_store_pgvector::PgMemoryStore::new(
                    Arc::clone(db),
                    agent,
                    user,
                    *policy,
                    grauwolf_store_pgvector::system_now(),
                ))
            }
        }
    }
}

impl LongTermMemory {
    /// Opens `memory.store` relative to `base_dir`. If another embedding model wrote the file,
    /// that is an error: the embeddings would not be comparable. If the file is already open in
    /// the process, both share the same store (ADR-0059).
    pub async fn open(
        memory: &LongTermMemorySpec,
        base_dir: &Path,
        embedder: Arc<dyn Embedder>,
    ) -> Result<Arc<Self>, String> {
        let model = &memory.embedding_model;
        let model = format!("{}:{}", model.provider, model.id);
        let Some(store) = &memory.store else {
            return Err(
                "long_term_memory.vector_store needs the spec name and the environment: use \
                 `LongTermMemory::open_in`"
                    .into(),
            );
        };
        let file = MemoryStore::shared(base_dir.join(store), model).await?;
        Ok(Self::with_backend(memory, Backend::File(file), embedder))
    }

    /// Like [`open`](Self::open), and the `vector_store` (PostgreSQL, ADR-0106) works: the URL
    /// comes from the variable `url_env` read from `env`, the facts belong to the agent
    /// `agent_name`.
    pub async fn open_in(
        agent_name: &str,
        memory: &LongTermMemorySpec,
        base_dir: &Path,
        embedder: Arc<dyn Embedder>,
        env: &dyn crate::config::Env,
    ) -> Result<Arc<Self>, String> {
        let Some(vector) = &memory.vector_store else {
            return Self::open(memory, base_dir, embedder).await;
        };
        #[cfg(feature = "pgvector")]
        {
            let db = crate::pgvector::connect(&vector.url_env, env).await?;
            Self::open_on(agent_name, memory, embedder, db).await
        }
        #[cfg(not(feature = "pgvector"))]
        {
            let _ = (agent_name, vector, env);
            Err("this build has no PostgreSQL store (feature `pgvector`)".into())
        }
    }

    /// Opens the memory of `agent_name` on `db`: learns the embedding dimension with one
    /// request, creates the table if needed and checks the model.
    #[cfg(feature = "pgvector")]
    pub async fn open_on(
        agent_name: &str,
        memory: &LongTermMemorySpec,
        embedder: Arc<dyn Embedder>,
        db: Arc<dyn grauwolf_store_pgvector::MemoryDb>,
    ) -> Result<Arc<Self>, String> {
        let vector = memory
            .vector_store
            .as_ref()
            .ok_or("long_term_memory has no vector_store")?;
        let model = &memory.embedding_model;
        let model = format!("{}:{}", model.provider, model.id);
        let probe = embedder
            .embed(&["dimension probe".to_owned()])
            .await
            .map_err(|error| error.to_string())?;
        let dimension = probe.first().map_or(0, |embedding| embedding.0.len());
        db.prepare(agent_name, &model, dimension)
            .await
            .map_err(|error| error.to_string())?;
        let policy = grauwolf_store_pgvector::MemoryPolicy {
            ttl_days: vector.ttl_days,
            min_similarity: vector.min_similarity,
        };
        let backend = Backend::Postgres {
            db,
            agent: agent_name.to_owned(),
            policy,
        };
        Ok(Self::with_backend(memory, backend, embedder))
    }

    fn with_backend(
        memory: &LongTermMemorySpec,
        backend: Backend,
        embedder: Arc<dyn Embedder>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            embedder,
            remember_tool: memory.remember_tool.clone(),
            recall_tool: memory.recall_tool.clone(),
            top_k: memory.top_k,
        })
    }

    /// Tools `[remember, recall]` for `user`; without a user for the shared area.
    pub fn tools(&self, user: Option<&str>) -> [Arc<dyn DynTool>; 2] {
        let retriever = Arc::new(Retriever::new(
            Arc::clone(&self.embedder),
            self.backend.area(user),
        ));
        [
            grauwolf_rag::remember_tool(Arc::clone(&retriever), &self.remember_tool),
            grauwolf_rag::recall_tool(retriever, &self.recall_tool, self.top_k),
        ]
    }

    /// Facts of `user` (without a user: shared area) for automatic recall and
    /// storing (ADR-0069); storing goes through the same `remember` as the tool.
    pub fn facts(&self, user: Option<&str>) -> Arc<dyn FactMemory> {
        let retriever = Arc::new(Retriever::new(
            Arc::clone(&self.embedder),
            self.backend.area(user),
        ));
        Arc::new(AreaFacts {
            remember: grauwolf_rag::remember_tool(Arc::clone(&retriever), &self.remember_tool),
            retriever,
            top_k: self.top_k,
        })
    }
}

/// Facts of an area as [`FactMemory`].
struct AreaFacts {
    retriever: Arc<Retriever>,
    remember: Arc<dyn DynTool>,
    top_k: usize,
}

impl FactMemory for AreaFacts {
    fn recall<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Result<Vec<String>, MemoryError>> {
        Box::pin(async move {
            let found = self.retriever.search(query, self.top_k).await?;
            Ok(found.into_iter().map(|scored| scored.chunk.text).collect())
        })
    }

    fn remember<'a>(&'a self, facts: &'a [String]) -> BoxFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            for fact in facts {
                self.remember
                    .call_json(serde_json::json!({ "fact": fact }))
                    .await?;
            }
            Ok(())
        })
    }
}

/// Content of the memory file.
#[derive(Serialize, Deserialize)]
struct MemoryFile {
    /// Embedding model as `<provider>:<id>`.
    model: String,
    facts: Vec<StoredFact>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredFact {
    /// User the fact belongs to; if missing, it belongs to the shared area.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    chunk: Chunk,
    embedding: Embedding,
}

/// The file with all facts; every new entry is written atomically right away, searching happens
/// in memory.
struct MemoryStore {
    file: PathBuf,
    /// Embedding model of the file as `<provider>:<id>`.
    model: String,
    /// Holds the entries and serializes the writing.
    saved: tokio::sync::Mutex<MemoryFile>,
}

impl MemoryStore {
    /// The store already open in the process for `file` or a newly loaded one.
    async fn shared(file: PathBuf, model: String) -> Result<Arc<Self>, String> {
        static OPEN: Mutex<Option<HashMap<PathBuf, Weak<MemoryStore>>>> = Mutex::new(None);
        let key = normalize(&file);
        let find = || -> Option<Arc<MemoryStore>> {
            let open = OPEN.lock().expect("memory registry");
            open.as_ref()?.get(&key)?.upgrade()
        };
        let store = match find() {
            Some(store) => store,
            None => {
                let loaded = Arc::new(Self::open(file, model.clone()).await?);
                let mut open = OPEN.lock().expect("memory registry");
                let open = open.get_or_insert_with(HashMap::new);
                open.retain(|_, store| store.strong_count() > 0);
                // If another call has opened the file in the meantime, its store applies.
                match open.get(&key).and_then(Weak::upgrade) {
                    Some(store) => store,
                    None => {
                        open.insert(key, Arc::downgrade(&loaded));
                        loaded
                    }
                }
            }
        };
        if store.model != model {
            return Err(format!(
                "{}: memory comes from the embedding model {}, the spec names {model}; restore the model \
                 or remove the file",
                store.file.display(),
                store.model
            ));
        }
        Ok(store)
    }

    /// Loads `file` if it exists.
    async fn open(file: PathBuf, model: String) -> Result<Self, String> {
        let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", file.display());
        let saved: MemoryFile = match tokio::fs::read(&file).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| failed(&error))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => MemoryFile {
                model,
                facts: Vec::new(),
            },
            Err(error) => return Err(failed(&error)),
        };
        Ok(Self {
            file,
            model: saved.model.clone(),
            saved: tokio::sync::Mutex::new(saved),
        })
    }

    async fn write(&self, memory: &MemoryFile) -> std::io::Result<()> {
        if let Some(dir) = self.file.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let partial = self.file.with_extension("json.partial");
        tokio::fs::write(&partial, serde_json::to_vec(memory)?).await?;
        tokio::fs::rename(&partial, &self.file).await
    }
}

/// Area of a user (or the shared one) as a vector store. Entries whose text already
/// exists in the same area are skipped.
struct Area {
    file: Arc<MemoryStore>,
    user: Option<String>,
}

impl VectorStore for Area {
    fn add<'a>(
        &'a self,
        chunk: Chunk,
        embedding: Embedding,
    ) -> BoxFuture<'a, Result<(), RagError>> {
        Box::pin(async move {
            let mut saved = self.file.saved.lock().await;
            if saved
                .facts
                .iter()
                .any(|fact| fact.user == self.user && fact.chunk.text == chunk.text)
            {
                return Ok(());
            }
            saved.facts.push(StoredFact {
                user: self.user.clone(),
                chunk,
                embedding,
            });
            self.file
                .write(&saved)
                .await
                .map_err(|error| RagError::Store(Box::new(error)))
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ScoredChunk>, RagError>> {
        Box::pin(async move {
            let saved = self.file.saved.lock().await;
            let mut hits: Vec<ScoredChunk> = saved
                .facts
                .iter()
                .filter(|fact| fact.user == self.user)
                .map(|fact| ScoredChunk {
                    chunk: fact.chunk.clone(),
                    score: query.cosine(&fact.embedding),
                })
                .collect();
            hits.sort_by(|a, b| b.score.total_cmp(&a.score));
            hits.truncate(limit);
            Ok(hits)
        })
    }
}

/// Absolute path without `.` and `..`, so that different spellings of the same file lead to
/// the same store (symlinks are not resolved).
fn normalize(file: &Path) -> PathBuf {
    let absolute = std::path::absolute(file).unwrap_or_else(|_| file.to_owned());
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}
