//! Credential sources behind the central configuration (ADR-0110 section 3): the environment of
//! a run is the process environment with the variables of `[credentials]` answered from files
//! or OpenBao. The sources live in `grauwolf-credentials` (feature `credentials`); without the
//! feature a configuration that asks for them is an error, not silently ignored.

use std::path::Path;
use std::sync::Arc;

use crate::config::Env;

/// A variable's name and the result of its check (`resolved <VAR> from …` or the error).
#[cfg(feature = "credentials")]
pub use grauwolf_credentials::VariableCheck;
/// A variable's name and the result of its check (`resolved <VAR> from …` or the error).
#[cfg(not(feature = "credentials"))]
pub type VariableCheck = (String, Result<String, String>);

/// The environment for the spec `file` (`None`: no spec): `base` itself without a `[credentials]`
/// section (nothing changes), else `base` with the mapped variables from their sources. A source
/// that cannot be read, a mapped variable that is also set in `base` and an invalid section are
/// errors that name the variable and the source, never a value.
#[cfg(feature = "credentials")]
pub fn env_for(base: Arc<dyn Env>, file: Option<&Path>) -> Result<Arc<dyn Env>, String> {
    grauwolf_credentials::CredentialEnv::for_spec(base, file)
}

/// Resolves `variable` for the spec `file` and reports `resolved <VAR> from <origin> (N bytes)`
/// (never the value), to check a deployment.
#[cfg(feature = "credentials")]
pub fn check(base: Arc<dyn Env>, file: Option<&Path>, variable: &str) -> Result<String, String> {
    use grauwolf_credentials::CredentialEnv;
    let identity = grauwolf_spiffe::WorkloadApiClient::from_env(&*base)
        .map_err(|error| error.to_string())?
        .map(|client| Arc::new(client) as Arc<dyn grauwolf_spiffe::WorkloadIdentity>);
    CredentialEnv::check(base, file, variable, identity)
}

/// Like [`check`] for several variables (all mapped ones if `variables` is empty): one result
/// per variable, so one failing source does not hide the others.
#[cfg(feature = "credentials")]
pub fn check_many(
    base: Arc<dyn Env>,
    file: Option<&Path>,
    variables: &[String],
) -> Result<Vec<VariableCheck>, String> {
    use grauwolf_credentials::CredentialEnv;
    let identity = grauwolf_spiffe::WorkloadApiClient::from_env(&*base)
        .map_err(|error| error.to_string())?
        .map(|client| Arc::new(client) as Arc<dyn grauwolf_spiffe::WorkloadIdentity>);
    CredentialEnv::check_many(base, file, variables, identity)
}

/// Without the feature `credentials`: always an error.
#[cfg(not(feature = "credentials"))]
pub fn check_many(
    _: Arc<dyn Env>,
    _: Option<&Path>,
    _: &[String],
) -> Result<Vec<VariableCheck>, String> {
    Err(
        "this build has no credential sources (Cargo feature `credentials` of grauwolf-host)"
            .to_owned(),
    )
}

/// Without the feature `credentials`: always an error.
#[cfg(not(feature = "credentials"))]
pub fn check(_: Arc<dyn Env>, _: Option<&Path>, _: &str) -> Result<String, String> {
    Err(
        "this build has no credential sources (Cargo feature `credentials` of grauwolf-host)"
            .to_owned(),
    )
}

/// Without the feature `credentials`: `base`, unless a credentials file is named or a
/// `grauwolf.toml` next to the spec has a `[credentials` table; then an error.
#[cfg(not(feature = "credentials"))]
pub fn env_for(base: Arc<dyn Env>, file: Option<&Path>) -> Result<Arc<dyn Env>, String> {
    let named = base
        .var("GRAUWOLF_CREDENTIALS")
        .is_some_and(|value| !value.trim().is_empty());
    let dir = file
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."));
    let in_toml = std::fs::read_to_string(dir.join("grauwolf.toml"))
        .is_ok_and(|text| text.contains("[credentials"));
    if named || in_toml {
        return Err(
            "this build has no credential sources (Cargo feature `credentials` of grauwolf-host)"
                .to_owned(),
        );
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapEnv;
    use grauwolf_test::TestDir;

    #[test]
    fn without_a_credentials_section_the_environment_is_unchanged() {
        let dir = TestDir::new("host-credentials-none");
        let base: Arc<dyn Env> = Arc::new(MapEnv::new([("GIT_TOKEN", "t")]));
        let env = env_for(Arc::clone(&base), Some(&dir.join("agent.json"))).unwrap();
        assert_eq!(env.var("GIT_TOKEN").as_deref(), Some("t"));
    }

    #[cfg(feature = "credentials")]
    #[test]
    fn a_mapped_file_credential_is_seen_through_the_environment() {
        let dir = TestDir::new("host-credentials-file");
        std::fs::write(dir.join("key"), "sk-from-file\n").unwrap();
        std::fs::write(
            dir.join("grauwolf.toml"),
            "[credentials.variables]\nOPENAI_API_KEY = \"file:key\"\n",
        )
        .unwrap();
        let base: Arc<dyn Env> = Arc::new(MapEnv::default());
        let env = env_for(base, Some(&dir.join("agent.json"))).unwrap();
        assert_eq!(env.var("OPENAI_API_KEY").as_deref(), Some("sk-from-file"));
    }

    #[cfg(not(feature = "credentials"))]
    #[test]
    fn a_build_without_the_feature_refuses_a_credentials_section() {
        let dir = TestDir::new("host-credentials-off");
        std::fs::write(
            dir.join("grauwolf.toml"),
            "[credentials.variables]\nOPENAI_API_KEY = \"file:key\"\n",
        )
        .unwrap();
        let base: Arc<dyn Env> = Arc::new(MapEnv::default());
        let error = env_for(base, Some(&dir.join("agent.json"))).err().unwrap();
        assert!(error.contains("no credential sources"), "{error}");
    }
}
