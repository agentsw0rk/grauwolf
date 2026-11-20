//! `CredentialEnv`: the environment with the mapped variables answered from their sources.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, RwLock};

use grauwolf_config::{Env, non_blank};
use grauwolf_spiffe::{WorkloadApiClient, WorkloadIdentity};

use crate::config::{CredentialsConfig, Source};
use crate::openbao::OpenBaoClient;

/// The environment variable that names the credentials file; it wins over a `grauwolf.toml`
/// next to the spec (like `GRAUWOLF_TOOL_POLICY`).
pub const CREDENTIALS_VARIABLE: &str = "GRAUWOLF_CREDENTIALS";

/// The name of the file next to a spec.
pub const CREDENTIALS_FILE: &str = "grauwolf.toml";

/// A credential that could not be resolved. Names the variable and the source, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialError {
    /// The variable.
    pub variable: String,
    /// The source (`file:/run/x`), if the error belongs to one.
    pub source: Option<String>,
    /// What went wrong.
    pub reason: String,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "credentials: {}", self.variable)?;
        if let Some(source) = &self.source {
            write!(f, " from {source}")?;
        }
        write!(f, ": {}", self.reason)
    }
}

impl std::error::Error for CredentialError {}

/// The result of checking one variable: its name and `resolved <VAR> from <origin> (N bytes)`
/// or the error.
pub type VariableCheck = (String, Result<String, String>);

/// An [`Env`] that answers the mapped variables from their sources and every other variable
/// from the environment underneath. Values are read again after the configured interval; if a
/// refresh fails the previous values stay and the error goes to stderr (it names variable and
/// source, never a value).
#[derive(Clone)]
pub struct CredentialEnv {
    base: Arc<dyn Env>,
    values: Arc<RwLock<BTreeMap<String, String>>>,
}

impl fmt::Debug for CredentialEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Names only: values stay out of Debug output.
        let values = self.values.read().unwrap();
        f.debug_struct("CredentialEnv")
            .field("variables", &values.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl CredentialEnv {
    /// Resolves every mapped variable once, then keeps them fresh in a background thread.
    /// `identity` is the agent's workload identity, needed by `openbao:` sources. A mapped
    /// variable that is also set in `base` is an error, as is a source that cannot be read: the
    /// start fails (no fallback). Blocks until the first resolution is done.
    pub fn start(
        base: Arc<dyn Env>,
        config: &CredentialsConfig,
        identity: Option<Arc<dyn WorkloadIdentity>>,
    ) -> Result<Self, CredentialError> {
        for name in config.variables.keys() {
            if base.var(name).is_some() {
                return Err(CredentialError {
                    variable: name.clone(),
                    source: None,
                    reason: "is mapped to a credential source and also set in the environment; \
                             remove one of them"
                        .to_owned(),
                });
            }
        }
        let client = match (
            &config.openbao,
            config.variables.values().any(Source::is_openbao),
        ) {
            (Some(openbao), true) => {
                let identity = identity.ok_or_else(|| CredentialError {
                    variable: grauwolf_spiffe::ENDPOINT_VARIABLE.to_owned(),
                    source: None,
                    reason: "is not set: the openbao source logs in with the agent's JWT-SVID"
                        .to_owned(),
                })?;
                Some(OpenBaoClient::new(openbao.clone(), identity))
            }
            _ => None,
        };
        let values = Arc::new(RwLock::new(BTreeMap::new()));
        let (sender, receiver) = std::sync::mpsc::channel();
        let (weak, variables, refresh) = (
            Arc::downgrade(&values),
            config.variables.clone(),
            config.refresh,
        );
        let spawned = std::thread::Builder::new()
            .name("grauwolf-credentials".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = sender.send(Err(CredentialError {
                            variable: String::new(),
                            source: None,
                            reason: format!("no runtime for the credential sources: {error}"),
                        }));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let first = resolve_all(&variables, client.as_ref()).await;
                    match (first, weak.upgrade()) {
                        (Ok(resolved), Some(values)) => *values.write().unwrap() = resolved,
                        (Err(error), _) => {
                            let _ = sender.send(Err(error));
                            return;
                        }
                        _ => return,
                    }
                    let _ = sender.send(Ok(()));
                    loop {
                        tokio::time::sleep(refresh).await;
                        let Some(values) = weak.upgrade() else { return };
                        match resolve_all(&variables, client.as_ref()).await {
                            Ok(resolved) => *values.write().unwrap() = resolved,
                            Err(error) => eprintln!("grauwolf: refreshing {error}"),
                        }
                    }
                });
            });
        spawned.map_err(|error| CredentialError {
            variable: String::new(),
            source: None,
            reason: format!("cannot start the credential thread: {error}"),
        })?;
        receiver.recv().map_err(|_| CredentialError {
            variable: String::new(),
            source: None,
            reason: "the credential thread ended unexpectedly".to_owned(),
        })??;
        Ok(Self { base, values })
    }

    /// The environment for the spec `file` (`None`: no spec): `base` itself if there is no
    /// `[credentials]` section, else a `CredentialEnv`. The section is in the file named by
    /// `GRAUWOLF_CREDENTIALS`, else in the `grauwolf.toml` next to the spec (in the working
    /// directory without a spec). A named file that is missing or invalid is an error. The
    /// identity for `openbao:` sources comes from `SPIFFE_ENDPOINT_SOCKET`.
    pub fn for_spec(base: Arc<dyn Env>, file: Option<&Path>) -> Result<Arc<dyn Env>, String> {
        let identity = WorkloadApiClient::from_env(&*base)
            .map_err(|error| error.to_string())?
            .map(|client| Arc::new(client) as Arc<dyn WorkloadIdentity>);
        Self::for_spec_with(base, file, identity)
    }

    /// Like [`for_spec`](Self::for_spec) with the identity given (tests).
    pub fn for_spec_with(
        base: Arc<dyn Env>,
        file: Option<&Path>,
        identity: Option<Arc<dyn WorkloadIdentity>>,
    ) -> Result<Arc<dyn Env>, String> {
        let Some(config) = load_config(&*base, file)? else {
            return Ok(base);
        };
        Ok(Arc::new(
            Self::start(base, &config, identity).map_err(|error| error.to_string())?,
        ))
    }

    /// Resolves `variable` the way a run would and reports where it came from and how long it
    /// is: `resolved ATLASSIAN_API_TOKEN from openbao (42 bytes)`. Never the value. For checking
    /// a deployment (`grauwolf credentials check`).
    pub fn check(
        base: Arc<dyn Env>,
        file: Option<&Path>,
        variable: &str,
        identity: Option<Arc<dyn WorkloadIdentity>>,
    ) -> Result<String, String> {
        let mut results = Self::check_many(base, file, &[variable.to_owned()], identity)?;
        results.remove(0).1
    }

    /// Checks each of `variables` on its own (all variables mapped in the `[credentials]`
    /// section if `variables` is empty, in the order of their names): one entry per variable
    /// with `resolved <VAR> from <origin> (N bytes)` or the error that names the variable and
    /// the source, never a value. One variable failing does not hide the others; the OpenBao
    /// login is shared. The outer error is for what stops the whole check (unreadable
    /// section, no variable mapped for "all", no identity for an `openbao:` source).
    pub fn check_many(
        base: Arc<dyn Env>,
        file: Option<&Path>,
        variables: &[String],
        identity: Option<Arc<dyn WorkloadIdentity>>,
    ) -> Result<Vec<VariableCheck>, String> {
        let config = load_config(&*base, file)?;
        let mapped = config
            .as_ref()
            .map(|config| config.variables.clone())
            .unwrap_or_default();
        let names: Vec<String> = if variables.is_empty() {
            if mapped.is_empty() {
                return Err("no variable is mapped in a [credentials] section".to_owned());
            }
            mapped.keys().cloned().collect()
        } else {
            variables.to_vec()
        };
        let client = match (
            config.as_ref().and_then(|config| config.openbao.as_ref()),
            names
                .iter()
                .any(|name| mapped.get(name).is_some_and(Source::is_openbao)),
        ) {
            (Some(openbao), true) => {
                let identity = identity.ok_or_else(|| {
                    format!(
                        "{} is not set: the openbao source logs in with the agent's JWT-SVID",
                        grauwolf_spiffe::ENDPOINT_VARIABLE
                    )
                })?;
                Some(OpenBaoClient::new(openbao.clone(), identity))
            }
            _ => None,
        };
        // Its own thread and runtime: the caller may already be inside one.
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| {
                            format!("no runtime for the credential sources: {error}")
                        })?;
                    Ok(runtime.block_on(async {
                        let mut results = Vec::new();
                        for name in &names {
                            results.push((
                                name.clone(),
                                check_one(&*base, name, mapped.get(name), client.as_ref()).await,
                            ));
                        }
                        results
                    }))
                })
                .join()
                .unwrap_or_else(|_| Err("the credential check panicked".to_owned()))
        })
    }
}

/// `resolved <VAR> from <origin> (N bytes)` for one variable, or its error.
async fn check_one(
    base: &dyn Env,
    name: &str,
    source: Option<&Source>,
    client: Option<&OpenBaoClient>,
) -> Result<String, String> {
    let (origin, value) = match source {
        Some(source) => {
            if base.var(name).is_some() {
                return Err(CredentialError {
                    variable: name.to_owned(),
                    source: None,
                    reason: "is mapped to a credential source and also set in the environment; \
                             remove one of them"
                        .to_owned(),
                }
                .to_string());
            }
            let origin = if source.is_openbao() {
                "openbao"
            } else {
                "file"
            };
            let value = resolve_one(name, source, client)
                .await
                .map_err(|error| error.to_string())?;
            (origin, value)
        }
        None => {
            let value = base.var(name).ok_or_else(|| {
                format!("{name} is neither mapped to a credential source nor set")
            })?;
            ("environment", value)
        }
    };
    Ok(format!(
        "resolved {name} from {origin} ({} bytes)",
        value.len()
    ))
}

/// The `[credentials]` section for the spec `file`: in the file named by `GRAUWOLF_CREDENTIALS`,
/// else in the `grauwolf.toml` next to the spec; `None` if there is none.
fn load_config(base: &dyn Env, file: Option<&Path>) -> Result<Option<CredentialsConfig>, String> {
    let path = match non_blank(&|name| base.var(name), CREDENTIALS_VARIABLE) {
        Some(named) => Path::new(&named).to_path_buf(),
        None => {
            let dir = file
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new("."));
            let path = dir.join(CREDENTIALS_FILE);
            if !path.is_file() {
                return Ok(None);
            }
            path
        }
    };
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    CredentialsConfig::parse(&text, dir).map_err(|error| format!("{}: {error}", path.display()))
}

impl Env for CredentialEnv {
    fn var(&self, name: &str) -> Option<String> {
        match self.values.read().unwrap().get(name) {
            Some(value) => Some(value.clone()),
            None => self.base.var(name),
        }
    }
}

async fn resolve_all(
    variables: &BTreeMap<String, Source>,
    client: Option<&OpenBaoClient>,
) -> Result<BTreeMap<String, String>, CredentialError> {
    let mut resolved = BTreeMap::new();
    for (name, source) in variables {
        resolved.insert(name.clone(), resolve_one(name, source, client).await?);
    }
    Ok(resolved)
}

/// One mapped variable from its source; an empty value is an error.
async fn resolve_one(
    name: &str,
    source: &Source,
    client: Option<&OpenBaoClient>,
) -> Result<String, CredentialError> {
    let result = match (source, client) {
        (Source::File(path), _) => read_file(path),
        (Source::OpenBao { mount, path, field }, Some(client)) => {
            client.read_field(mount, path, field).await
        }
        (Source::OpenBao { .. }, None) => Err("no OpenBao client".to_owned()),
    };
    let error = |reason: String| CredentialError {
        variable: name.to_owned(),
        source: Some(source.describe()),
        reason,
    };
    let value = result.map_err(error)?;
    if value.is_empty() {
        return Err(error("the value is empty".to_owned()));
    }
    Ok(value)
}

/// The content of a credential file without the trailing newline. Errors do not contain it.
fn read_file(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value = text.trim_end_matches(['\n', '\r']);
    if value.is_empty() {
        return Err("the file is empty".to_owned());
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use grauwolf_config::MapEnv;
    use grauwolf_test::TestDir;

    fn env(pairs: &[(&str, &str)]) -> Arc<dyn Env> {
        Arc::new(MapEnv::new(pairs.iter().copied()))
    }

    fn config(text: &str, dir: &Path) -> CredentialsConfig {
        CredentialsConfig::parse(text, dir).unwrap().unwrap()
    }

    #[test]
    fn a_file_source_answers_the_mapped_variable_and_the_rest_comes_from_the_environment() {
        let dir = TestDir::new("credentials-file");
        std::fs::write(dir.join("key"), "sk-secret\n").unwrap();
        let config = config(
            "[credentials.variables]\nOPENAI_API_KEY = \"file:key\"\n",
            dir.path(),
        );
        let credentials = CredentialEnv::start(env(&[("HOME", "/h")]), &config, None).unwrap();
        assert_eq!(
            credentials.var("OPENAI_API_KEY").as_deref(),
            Some("sk-secret")
        );
        assert_eq!(credentials.var("HOME").as_deref(), Some("/h"));
        assert_eq!(credentials.var("GIT_TOKEN"), None);
    }

    #[test]
    fn a_mapped_variable_that_is_also_set_is_an_error_without_the_values() {
        let dir = TestDir::new("credentials-conflict");
        std::fs::write(dir.join("key"), "from-file").unwrap();
        let config = config(
            "[credentials.variables]\nOPENAI_API_KEY = \"file:key\"\n",
            dir.path(),
        );
        let error = CredentialEnv::start(env(&[("OPENAI_API_KEY", "from-env")]), &config, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("OPENAI_API_KEY"), "{error}");
        assert!(
            !error.contains("from-env") && !error.contains("from-file"),
            "{error}"
        );
    }

    #[test]
    fn an_unreadable_or_empty_file_fails_naming_variable_and_source() {
        let dir = TestDir::new("credentials-missing");
        std::fs::write(dir.join("empty"), "\n").unwrap();
        for file in ["missing", "empty"] {
            let config = config(
                &format!("[credentials.variables]\nGIT_TOKEN = \"file:{file}\"\n"),
                dir.path(),
            );
            let error = CredentialEnv::start(env(&[]), &config, None)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("GIT_TOKEN") && error.contains(file),
                "{error}"
            );
        }
    }

    #[test]
    fn debug_output_names_the_variables_but_not_the_values() {
        let dir = TestDir::new("credentials-debug");
        std::fs::write(dir.join("key"), "sk-secret").unwrap();
        let config = config(
            "[credentials.variables]\nGIT_TOKEN = \"file:key\"\n",
            dir.path(),
        );
        let shown = format!(
            "{:?}",
            CredentialEnv::start(env(&[]), &config, None).unwrap()
        );
        assert!(
            shown.contains("GIT_TOKEN") && !shown.contains("sk-secret"),
            "{shown}"
        );
    }

    #[test]
    fn for_spec_reads_the_grauwolf_toml_next_to_the_spec() {
        let dir = TestDir::new("credentials-spec");
        std::fs::write(dir.join("key"), "tok").unwrap();
        std::fs::write(
            dir.join("grauwolf.toml"),
            "[credentials.variables]\nGIT_TOKEN = \"file:key\"\n",
        )
        .unwrap();
        let credentials = CredentialEnv::for_spec(env(&[]), Some(&dir.join("agent.json"))).unwrap();
        assert_eq!(credentials.var("GIT_TOKEN").as_deref(), Some("tok"));
    }

    #[test]
    fn for_spec_prefers_the_file_named_by_the_variable() {
        let dir = TestDir::new("credentials-named");
        std::fs::write(dir.join("key"), "tok").unwrap();
        std::fs::write(
            dir.join("grauwolf.toml"),
            "[credentials.variables]\nGIT_TOKEN = \"file:nope\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("other.toml"),
            "[credentials.variables]\nGIT_TOKEN = \"file:key\"\n",
        )
        .unwrap();
        let named = dir.join("other.toml");
        let base = env(&[(CREDENTIALS_VARIABLE, named.to_str().unwrap())]);
        let credentials = CredentialEnv::for_spec(base, Some(&dir.join("agent.json"))).unwrap();
        assert_eq!(credentials.var("GIT_TOKEN").as_deref(), Some("tok"));
    }

    #[test]
    fn for_spec_changes_nothing_without_a_credentials_section() {
        let dir = TestDir::new("credentials-none");
        let base = env(&[("GIT_TOKEN", "t")]);
        let same = CredentialEnv::for_spec(Arc::clone(&base), Some(&dir.join("a.json"))).unwrap();
        assert!(Arc::ptr_eq(&base, &same));
        std::fs::write(dir.join("grauwolf.toml"), "[tool_packages]\n").unwrap();
        let same = CredentialEnv::for_spec(Arc::clone(&base), Some(&dir.join("a.json"))).unwrap();
        assert!(Arc::ptr_eq(&base, &same));
    }

    #[test]
    fn check_reports_source_and_length_but_not_the_value() {
        let dir = TestDir::new("credentials-check");
        std::fs::write(dir.join("key"), "sk-secret\n").unwrap();
        std::fs::write(
            dir.join("grauwolf.toml"),
            "[credentials.variables]\nGIT_TOKEN = \"file:key\"\n",
        )
        .unwrap();
        let spec = dir.join("agent.json");
        let message =
            CredentialEnv::check(env(&[("HOME", "/h")]), Some(&spec), "GIT_TOKEN", None).unwrap();
        assert_eq!(message, "resolved GIT_TOKEN from file (9 bytes)");
        let plain =
            CredentialEnv::check(env(&[("HOME", "/h")]), Some(&spec), "HOME", None).unwrap();
        assert_eq!(plain, "resolved HOME from environment (2 bytes)");
        let error =
            CredentialEnv::check(env(&[]), Some(&spec), "OPENAI_API_KEY", None).unwrap_err();
        assert!(error.contains("OPENAI_API_KEY"), "{error}");
    }

    #[test]
    fn a_named_file_that_is_missing_is_an_error() {
        let base = env(&[(CREDENTIALS_VARIABLE, "/nonexistent/creds.toml")]);
        let error = CredentialEnv::for_spec(base, None).err().unwrap();
        assert!(error.contains("/nonexistent/creds.toml"), "{error}");
    }
}
