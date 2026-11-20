//! Registry credentials from the Docker configuration (`config.json`): static `auths` entries,
//! identity tokens and credential helpers (`docker-credential-<name> get`). Secrets never appear
//! in error messages or logs; a helper's output is parsed and dropped.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;

use crate::client::Credential;

/// How long a credential helper may take.
const HELPER_TIMEOUT: Duration = Duration::from_secs(20);
/// Most output that is read from a helper.
const MAX_HELPER_OUTPUT: u64 = 1024 * 1024;
/// Key under which Docker stores the credentials of Docker Hub.
const HUB_KEY: &str = "https://index.docker.io/v1/";

/// Where the Docker configuration lives: `$DOCKER_CONFIG/config.json`, else
/// `~/.docker/config.json`.
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DOCKER_CONFIG").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("config.json"));
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join(".docker").join("config.json"))
}

/// The credential related parts of a Docker `config.json`.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct DockerConfig {
    auths: BTreeMap<String, AuthEntry>,
    #[serde(rename = "credHelpers")]
    cred_helpers: BTreeMap<String, String>,
    #[serde(rename = "credsStore")]
    creds_store: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct AuthEntry {
    auth: Option<String>,
    username: Option<String>,
    password: Option<String>,
    identitytoken: Option<String>,
}

impl std::fmt::Debug for DockerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DockerConfig(..)")
    }
}

impl DockerConfig {
    /// Reads `path`; a missing file is an empty configuration. The error names the file but
    /// never repeats its content.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        serde_json::from_slice(&bytes).map_err(|_| {
            format!(
                "{}: not a valid Docker configuration (JSON object expected)",
                path.display()
            )
        })
    }

    /// The credential for `registry` (as in an OCI reference). Like Docker: the helper of
    /// `credHelpers` for that registry, else `credsStore`; where the helper has no entry the
    /// static `auths` entry is used. `search` is where helper programs are looked up.
    pub(crate) async fn credential(
        &self,
        registry: &str,
        search: &[PathBuf],
    ) -> Result<Option<Credential>, String> {
        let wanted = normalize(registry);
        let helper = self
            .cred_helpers
            .iter()
            .find(|(name, _)| normalize(name) == wanted)
            .map(|(_, helper)| helper)
            .or(self.creds_store.as_ref());
        if let Some(helper) = helper
            && let Some(credential) = run_helper(helper, &server_url(&wanted), search).await?
        {
            return Ok(Some(credential));
        }
        match self
            .auths
            .iter()
            .find(|(name, _)| normalize(name) == wanted)
        {
            Some((_, entry)) => entry.credential(),
            None => Ok(None),
        }
    }
}

impl AuthEntry {
    fn credential(&self) -> Result<Option<Credential>, String> {
        if let Some(token) = self.identitytoken.as_ref().filter(|t| !t.is_empty()) {
            return Ok(Some(Credential::Bearer(token.clone())));
        }
        if let Some(auth) = self.auth.as_ref().filter(|a| !a.is_empty()) {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(auth.trim())
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|text| {
                    text.split_once(':')
                        .map(|(user, secret)| (user.to_owned(), secret.to_owned()))
                })
                .ok_or("an `auth` entry of the Docker configuration is not base64 user:password")?;
            return Ok(Some(Credential::Basic {
                user: decoded.0,
                secret: decoded.1,
            }));
        }
        Ok(match (&self.username, &self.password) {
            (Some(user), Some(secret)) if !user.is_empty() => Some(Credential::Basic {
                user: user.clone(),
                secret: secret.clone(),
            }),
            _ => None,
        })
    }
}

/// Registry name as Docker compares it: no scheme, no path, Docker Hub aliases unified, ports
/// kept.
pub(crate) fn normalize(name: &str) -> String {
    let rest = name
        .strip_prefix("https://")
        .or_else(|| name.strip_prefix("http://"))
        .unwrap_or(name);
    let host = rest.split('/').next().unwrap_or(rest).to_ascii_lowercase();
    match host.as_str() {
        "docker.io" | "index.docker.io" | "registry-1.docker.io" | "registry.hub.docker.com" => {
            "docker.io".to_owned()
        }
        _ => host,
    }
}

/// What a helper is asked about: Docker Hub under its historic URL, others by host.
fn server_url(normalized: &str) -> String {
    if normalized == "docker.io" {
        HUB_KEY.to_owned()
    } else {
        normalized.to_owned()
    }
}

/// Directories of `PATH`, where helpers are found by default.
pub(crate) fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct HelperAnswer {
    #[serde(rename = "Username", default)]
    username: String,
    #[serde(rename = "Secret", default)]
    secret: String,
}

/// Runs `docker-credential-<name> get` off the async workers.
async fn run_helper(
    name: &str,
    server: &str,
    search: &[PathBuf],
) -> Result<Option<Credential>, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("the Docker configuration names an invalid credential helper".into());
    }
    let program = format!("docker-credential-{name}");
    let path = search
        .iter()
        .map(|dir| dir.join(&program))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            format!(
                "credential helper {program} (from the Docker configuration) was not found in PATH"
            )
        })?;
    let server = server.to_owned();
    tokio::task::spawn_blocking(move || call_helper(&program, &path, &server))
        .await
        .map_err(|_| "credential helper task failed".to_owned())?
}

fn call_helper(program: &str, path: &Path, server: &str) -> Result<Option<Credential>, String> {
    // Tests write their helper scripts while other tests start helpers; see `tests::SPAWNING`.
    #[cfg(test)]
    let spawning = tests::SPAWNING.read().unwrap_or_else(|e| e.into_inner());
    let mut child = Command::new(path)
        .arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("credential helper {program} could not be started: {error}"))?;
    #[cfg(test)]
    drop(spawning);
    if let Some(mut stdin) = child.stdin.take() {
        // A helper that does not read its input is judged by its exit status.
        let _ = stdin.write_all(server.as_bytes());
    }
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = (&mut stdout).take(MAX_HELPER_OUTPUT).read_to_end(&mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < HELPER_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "credential helper {program} did not answer within {} s",
                    HELPER_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("credential helper {program}: {error}")),
        }
    };
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        // Helpers report "no entry" with an exit status and a message on stdout.
        if String::from_utf8_lossy(&output).contains("credentials not found") {
            return Ok(None);
        }
        return Err(format!("credential helper {program} failed ({status})"));
    }
    let answer: HelperAnswer = serde_json::from_slice(&output)
        .map_err(|_| format!("credential helper {program} gave an unusable answer"))?;
    Ok(Some(if answer.username == "<token>" {
        Credential::Bearer(answer.secret)
    } else {
        Credential::Basic {
            user: answer.username,
            secret: answer.secret,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RegistryAuth;

    /// Cause of the sporadic "Text file busy (os error 26)": `fork` copies every open file
    /// descriptor of the process until the child has called `exec`. When one test writes its
    /// helper script while another test is between `fork` and `exec` of its own helper, that
    /// child still holds the script's write descriptor, and executing the script fails with
    /// ETXTBSY. Writing a script takes the write lock, starting a helper (`call_helper`) the read
    /// lock until `spawn` returned (after `exec`), so no child can hold a descriptor of a script
    /// that is written later.
    pub(super) static SPAWNING: std::sync::RwLock<()> = std::sync::RwLock::new(());

    fn dir(name: &str) -> PathBuf {
        // Below the build directory (next to the test executable), not in a shared temp dir.
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join(format!("grauwolf-docker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(name: &str, json: &str) -> (PathBuf, PathBuf) {
        let dir = dir(name);
        let file = dir.join("config.json");
        std::fs::write(&file, json).unwrap();
        (dir, file)
    }

    fn b64(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    /// A helper script that answers `get` with `answer` (stdout), exiting with `status`.
    fn helper(dir: &Path, name: &str, answer: &str, status: i32) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(format!("docker-credential-{name}"));
        let _writing = SPAWNING.write().unwrap_or_else(|e| e.into_inner());
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ \"$1\" = get ] || exit 2\nread server\n\
                 printf '%s' \"{}\" | sed \"s|@SERVER@|$server|\"\nexit {status}\n",
                answer.replace('"', "\\\"")
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    async fn lookup(
        file: &Path,
        search: &[PathBuf],
        registry: &str,
    ) -> Result<Option<Credential>, String> {
        DockerConfig::load(file)?.credential(registry, search).await
    }

    fn basic(user: &str, secret: &str) -> Option<Credential> {
        Some(Credential::Basic {
            user: user.into(),
            secret: secret.into(),
        })
    }

    #[tokio::test]
    async fn an_auth_entry_gives_basic_credentials_with_colons_in_the_password() {
        let (_, file) = config(
            "auths",
            &format!(
                r#"{{"auths":{{"reg.example:5000":{{"auth":"{}"}}}}}}"#,
                b64("ci:pa:ss")
            ),
        );
        assert_eq!(
            lookup(&file, &[], "reg.example:5000").await.unwrap(),
            basic("ci", "pa:ss")
        );
        assert_eq!(lookup(&file, &[], "reg.example").await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_identity_token_is_a_bearer_credential() {
        let (_, file) = config(
            "identity",
            r#"{"auths":{"reg.example":{"identitytoken":"tok123"}}}"#,
        );
        assert_eq!(
            lookup(&file, &[], "reg.example").await.unwrap(),
            Some(Credential::Bearer("tok123".into()))
        );
    }

    #[tokio::test]
    async fn docker_hub_names_are_unified() {
        let (_, file) = config(
            "hub",
            &format!(
                r#"{{"auths":{{"https://index.docker.io/v1/":{{"auth":"{}"}}}}}}"#,
                b64("u:p")
            ),
        );
        for name in ["docker.io", "index.docker.io", "registry-1.docker.io"] {
            assert_eq!(
                lookup(&file, &[], name).await.unwrap(),
                basic("u", "p"),
                "{name}"
            );
        }
        let (_, file) = config(
            "scheme",
            &format!(
                r#"{{"auths":{{"https://reg.example:5000/path":{{"auth":"{}"}}}}}}"#,
                b64("u:p")
            ),
        );
        assert_eq!(
            lookup(&file, &[], "reg.example:5000").await.unwrap(),
            basic("u", "p")
        );
    }

    #[tokio::test]
    async fn a_missing_file_is_an_empty_configuration() {
        let dir = dir("missing");
        assert_eq!(
            lookup(&dir.join("config.json"), &[], "reg.example")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_malformed_configuration_is_reported_without_its_content() {
        let (_, file) = config("malformed", r#"{"auths": "pa55w0rd-leak""#);
        let error = lookup(&file, &[], "reg.example").await.unwrap_err();
        assert!(
            error.contains("config.json") && !error.contains("pa55w0rd"),
            "{error}"
        );

        let (_, file) = config(
            "badauth",
            r#"{"auths":{"reg.example":{"auth":"!!not-base64!!"}}}"#,
        );
        let error = lookup(&file, &[], "reg.example").await.unwrap_err();
        assert!(!error.contains("not-base64"), "{error}");
    }

    #[tokio::test]
    async fn a_cred_helper_is_asked_with_the_registry_on_stdin() {
        let (dir, file) = config("helper", r#"{"credHelpers":{"reg.example":"vault"}}"#);
        helper(
            &dir,
            "vault",
            r#"{"ServerURL":"@SERVER@","Username":"robot","Secret":"h3lper-s3cret"}"#,
            0,
        );
        assert_eq!(
            lookup(&file, std::slice::from_ref(&dir), "reg.example")
                .await
                .unwrap(),
            basic("robot", "h3lper-s3cret")
        );
        // Other registries are not served by this helper.
        assert_eq!(lookup(&file, &[dir], "other.example").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_creds_store_serves_all_registries_and_token_users_are_bearer() {
        let (dir, file) = config("store", r#"{"credsStore":"keys"}"#);
        helper(
            &dir,
            "keys",
            r#"{"Username":"<token>","Secret":"tok-from-store"}"#,
            0,
        );
        assert_eq!(
            lookup(&file, &[dir], "reg.example").await.unwrap(),
            Some(Credential::Bearer("tok-from-store".into()))
        );
    }

    #[tokio::test]
    async fn a_helper_without_entry_falls_back_to_the_auths_entry() {
        let (dir, file) = config(
            "fallback",
            &format!(
                r#"{{"credsStore":"keys","auths":{{"reg.example":{{"auth":"{}"}}}}}}"#,
                b64("u:p")
            ),
        );
        helper(&dir, "keys", "credentials not found in native keychain", 1);
        assert_eq!(
            lookup(&file, &[dir], "reg.example").await.unwrap(),
            basic("u", "p")
        );
    }

    #[tokio::test]
    async fn a_missing_helper_is_named_clearly() {
        let (dir, file) = config("nohelper", r#"{"credsStore":"nonexistent"}"#);
        let error = lookup(&file, &[dir], "reg.example").await.unwrap_err();
        assert!(error.contains("docker-credential-nonexistent"), "{error}");
        assert!(error.contains("not found"), "{error}");
    }

    #[tokio::test]
    async fn a_failing_helper_never_leaks_its_output() {
        let (dir, file) = config("failing", r#"{"credsStore":"keys"}"#);
        helper(&dir, "keys", "boom: Secret=l34ked", 3);
        let error = lookup(&file, std::slice::from_ref(&dir), "reg.example")
            .await
            .unwrap_err();
        assert!(
            error.contains("docker-credential-keys") && !error.contains("l34ked"),
            "{error}"
        );

        helper(&dir, "keys", "not json l34ked", 0);
        let error = lookup(&file, &[dir], "reg.example").await.unwrap_err();
        assert!(!error.contains("l34ked"), "{error}");
    }

    #[tokio::test]
    async fn helper_names_cannot_escape_the_search_path() {
        let (dir, file) = config("escape", r#"{"credsStore":"../evil"}"#);
        let error = lookup(&file, &[dir], "reg.example").await.unwrap_err();
        assert!(error.contains("invalid credential helper"), "{error}");
    }

    #[tokio::test]
    async fn explicit_credentials_win_over_the_docker_configuration() {
        let (_, file) = config(
            "precedence",
            &format!(
                r#"{{"auths":{{"reg.example":{{"auth":"{}"}},"only.docker":{{"auth":"{}"}}}}}}"#,
                b64("docker:one"),
                b64("docker:two")
            ),
        );
        let auth = RegistryAuth::from_json(r#"{"reg.example":"env:secret"}"#)
            .unwrap()
            .with_docker_config(&file, vec![]);
        assert_eq!(
            auth.credential("reg.example").await.unwrap(),
            basic("env", "secret")
        );
        assert_eq!(
            auth.credential("only.docker").await.unwrap(),
            basic("docker", "two")
        );
        assert_eq!(auth.credential("unknown").await.unwrap(), None);
    }
}
