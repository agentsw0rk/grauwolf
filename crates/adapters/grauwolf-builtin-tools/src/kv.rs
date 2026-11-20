//! Key-value store tools (ADR-0098): `store_value`, `retrieve_value`, `list_keys` over one JSON
//! file. Distinct from the semantic long-term memory (`remember`/`recall`, ADR-0058).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use serde_json::{Value, json};

/// Names of the key-value tools.
pub const KV_TOOLS: &[&str] = &["store_value", "retrieve_value", "list_keys"];

/// Longest key in bytes.
pub const MAX_KEY_BYTES: usize = 256;
/// Longest value in bytes.
pub const MAX_VALUE_BYTES: usize = 64 * 1024;

type Locks = Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>;

/// One lock per file: calls on one file run one after another, whatever instance they use.
fn locks() -> &'static Locks {
    static LOCKS: OnceLock<Locks> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

/// The key-value tools over the file `path` (usually `kv.json` below the root).
#[derive(Debug, Clone)]
pub struct KvTools {
    path: PathBuf,
}

impl KvTools {
    /// Tools that keep their values in `path`; the file and its directories are created on the
    /// first write.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The tools with the names `names`; unknown names are an error.
    pub fn tools<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<Arc<dyn DynTool>>, String> {
        names
            .into_iter()
            .map(|name| {
                let kind = match name {
                    "store_value" => Kind::Store,
                    "retrieve_value" => Kind::Retrieve,
                    "list_keys" => Kind::List,
                    other => return Err(format!("unknown built-in tool '{other}'")),
                };
                Ok(Arc::new(KvTool {
                    path: self.path.clone(),
                    kind,
                }) as Arc<dyn DynTool>)
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Store,
    Retrieve,
    List,
}

struct KvTool {
    path: PathBuf,
    kind: Kind,
}

fn failure(message: impl Into<String>) -> ToolError {
    ToolError::Execution(message.into())
}

async fn read(path: &Path) -> Result<BTreeMap<String, String>, ToolError> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => serde_json::from_str(&text).map_err(|error| {
            failure(format!(
                "{} is not a key-value file: {error}",
                path.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(failure(format!("{}: {error}", path.display()))),
    }
}

/// Writes through a temporary file and a rename, so readers never see half a file.
async fn write(path: &Path, values: &BTreeMap<String, String>) -> Result<(), ToolError> {
    let failed = |error: std::io::Error| failure(format!("{}: {error}", path.display()));
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    }
    let temporary = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(values).map_err(|error| failure(error.to_string()))?;
    tokio::fs::write(&temporary, text).await.map_err(failed)?;
    tokio::fs::rename(&temporary, path).await.map_err(failed)
}

impl KvTool {
    fn text<'a>(&self, args: &'a Value, field: &str) -> Result<&'a str, ToolError> {
        args[field].as_str().ok_or_else(|| ToolError::InvalidArgs {
            tool: self.name().to_owned(),
            reason: format!("missing field '{field}' (text)"),
        })
    }

    fn key<'a>(&self, args: &'a Value) -> Result<&'a str, ToolError> {
        let key = self.text(args, "key")?;
        if key.is_empty() || key.len() > MAX_KEY_BYTES {
            return Err(failure(format!(
                "the key must have 1 to {MAX_KEY_BYTES} bytes, this one has {}",
                key.len()
            )));
        }
        Ok(key)
    }

    async fn run(&self, args: Value) -> Result<String, ToolError> {
        let lock = Arc::clone(
            locks()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(self.path.clone())
                .or_default(),
        );
        let _guard = lock.lock().await;
        match self.kind {
            Kind::Store => {
                let key = self.key(&args)?;
                let value = self.text(&args, "value")?;
                if value.len() > MAX_VALUE_BYTES {
                    return Err(failure(format!(
                        "the value must have at most {MAX_VALUE_BYTES} bytes, this one has {}",
                        value.len()
                    )));
                }
                let mut values = read(&self.path).await?;
                values.insert(key.to_owned(), value.to_owned());
                write(&self.path, &values).await?;
                Ok(format!("Stored key: {key}"))
            }
            Kind::Retrieve => {
                let key = self.key(&args)?;
                Ok(read(&self.path)
                    .await?
                    .remove(key)
                    .unwrap_or_else(|| format!("Key not found: {key}")))
            }
            Kind::List => {
                let values = read(&self.path).await?;
                Ok(if values.is_empty() {
                    "No keys stored".to_owned()
                } else {
                    values.into_keys().collect::<Vec<_>>().join("\n")
                })
            }
        }
    }
}

impl DynTool for KvTool {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Store => "store_value",
            Kind::Retrieve => "retrieve_value",
            Kind::List => "list_keys",
        }
    }

    fn descriptor(&self) -> ToolDescriptor {
        let (description, parameters) = match self.kind {
            Kind::Store => (
                "Stores a text value under a key in the persistent key-value store \
                 (overwrites an existing key; keys up to 256 bytes, values up to 64 KiB).",
                json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "description": "The key" },
                        "value": { "type": "string", "description": "The value to store" }
                    },
                    "required": ["key", "value"]
                }),
            ),
            Kind::Retrieve => (
                "Retrieves the value stored under a key.",
                json!({
                    "type": "object",
                    "properties": { "key": { "type": "string", "description": "The key to look up" } },
                    "required": ["key"]
                }),
            ),
            Kind::List => (
                "Lists all keys of the key-value store.",
                json!({ "type": "object", "properties": {} }),
            ),
        };
        ToolDescriptor {
            name: self.name().to_owned(),
            description: description.to_owned(),
            parameters,
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(self.run(args))
    }
}
