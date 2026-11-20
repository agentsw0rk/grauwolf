//! The `[credentials]` section: which variables come from which source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use grauwolf_config::VARIABLES;
use serde::Deserialize;

/// Default interval after which mapped values are read again.
pub const DEFAULT_REFRESH: Duration = Duration::from_secs(5 * 60);

/// Variables that are read from the process environment directly (by the OpenTelemetry SDK,
/// the shell tools, the credential sources themselves) and so cannot be mapped.
const NOT_THROUGH_ENV: &[&str] = &[
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_HEADERS",
    "CARGO",
    "RUSTC",
    "HOME",
    "PATH",
    "DOCKER_CONFIG",
    "GRAUWOLF_CREDENTIALS",
    "SPIFFE_ENDPOINT_SOCKET",
    "SPIFFE_FETCH_TIMEOUT",
];

/// Where the value of one variable comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A file (`file:/run/secrets/openai/api-key`); a trailing newline is removed.
    File(PathBuf),
    /// A KV v2 secret of OpenBao (`openbao:kv/agents/reviewer#jira`).
    OpenBao {
        /// The KV v2 mount (`kv`).
        mount: String,
        /// The path of the secret below the mount (`agents/reviewer`).
        path: String,
        /// The field of the secret (`jira`).
        field: String,
    },
}

impl Source {
    /// Whether this is an OpenBao source.
    pub fn is_openbao(&self) -> bool {
        matches!(self, Self::OpenBao { .. })
    }

    /// A description for errors: scheme and location, never a value.
    pub fn describe(&self) -> String {
        match self {
            Self::File(path) => format!("file:{}", path.display()),
            Self::OpenBao { mount, path, field } => format!("openbao:{mount}/{path}#{field}"),
        }
    }
}

/// How the agent logs in to OpenBao: with its JWT-SVID (ADR-0109).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiffeLogin {
    /// The audience of the JWT-SVID (the role's `bound_audiences`).
    pub audience: String,
    /// The role of the JWT auth method.
    pub role: String,
    /// The mount of the JWT auth method (default `jwt`).
    pub mount: String,
}

/// The OpenBao server and the login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenBaoConfig {
    /// Base address, e.g. `https://openbao.internal:8200`.
    pub address: String,
    /// The login.
    pub login: SpiffeLogin,
}

/// The parsed `[credentials]` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsConfig {
    /// The OpenBao server, needed by `openbao:` sources.
    pub openbao: Option<OpenBaoConfig>,
    /// Interval after which values are read again.
    pub refresh: Duration,
    /// The mapped variables.
    pub variables: BTreeMap<String, Source>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLogin {
    audience: String,
    role: String,
    #[serde(default = "default_mount")]
    mount: String,
}

fn default_mount() -> String {
    "jwt".to_owned()
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RawAuth {
    Spiffe(RawLogin),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOpenBao {
    address: String,
    auth: RawAuth,
    refresh: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawCredentials {
    openbao: Option<RawOpenBao>,
    #[serde(default)]
    variables: BTreeMap<String, String>,
}

/// The file as a whole: other tables (`tool_packages`) belong to other readers.
#[derive(Deserialize)]
struct RawFile {
    credentials: Option<RawCredentials>,
}

impl CredentialsConfig {
    /// Parses the text of a `grauwolf.toml`; `None` if it has no `[credentials]` table. Relative
    /// file paths are resolved against `base`, the directory of the file.
    pub fn parse(text: &str, base: &Path) -> Result<Option<Self>, String> {
        let file: RawFile = toml::from_str(text).map_err(|error| error.to_string())?;
        let Some(raw) = file.credentials else {
            return Ok(None);
        };
        let refresh = match raw.openbao.as_ref().and_then(|o| o.refresh.as_deref()) {
            Some(text) => parse_duration(text)?,
            None => DEFAULT_REFRESH,
        };
        let mut variables = BTreeMap::new();
        for (name, source) in &raw.variables {
            if !VARIABLES.iter().any(|variable| variable.name == name) {
                return Err(format!(
                    "credentials.variables: {name} is not a variable grauwolf reads"
                ));
            }
            if NOT_THROUGH_ENV.contains(&name.as_str()) {
                return Err(format!(
                    "credentials.variables: {name} is read from the process environment directly \
                     and cannot come from a credential source"
                ));
            }
            let source = parse_source(source, base)
                .map_err(|reason| format!("credentials.variables.{name}: {reason}"))?;
            variables.insert(name.clone(), source);
        }
        let openbao = raw.openbao.map(|raw| {
            let RawAuth::Spiffe(login) = raw.auth;
            OpenBaoConfig {
                address: raw.address.trim_end_matches('/').to_owned(),
                login: SpiffeLogin {
                    audience: login.audience,
                    role: login.role,
                    mount: login.mount,
                },
            }
        });
        if openbao.is_none() {
            if let Some((name, _)) = variables
                .iter()
                .find(|(_, source)| matches!(source, Source::OpenBao { .. }))
            {
                return Err(format!(
                    "credentials.variables.{name}: an openbao source needs a [credentials.openbao] table"
                ));
            }
        }
        Ok(Some(Self {
            openbao,
            refresh,
            variables,
        }))
    }
}

fn parse_source(text: &str, base: &Path) -> Result<Source, String> {
    if let Some(path) = text.strip_prefix("file:") {
        if path.is_empty() {
            return Err("file: needs a path".to_owned());
        }
        return Ok(Source::File(base.join(path)));
    }
    let broker = text
        .strip_prefix("openbao:")
        .or_else(|| text.strip_prefix("vault:"));
    let Some(location) = broker else {
        return Err("a source starts with file:, openbao: or vault:".to_owned());
    };
    let (secret, field) = location
        .split_once('#')
        .filter(|(_, field)| !field.is_empty())
        .ok_or("an openbao source needs a field: openbao:<mount>/<path>#<field>")?;
    let (mount, path) = secret
        .split_once('/')
        .filter(|(mount, path)| !mount.is_empty() && !path.is_empty())
        .ok_or("an openbao source needs a mount and a path: openbao:<mount>/<path>#<field>")?;
    Ok(Source::OpenBao {
        mount: mount.to_owned(),
        path: path.trim_matches('/').to_owned(),
        field: field.to_owned(),
    })
}

/// `30s`, `5m`, `1h` (a number and one unit).
fn parse_duration(text: &str) -> Result<Duration, String> {
    let invalid =
        || format!("credentials.openbao.refresh: '{text}' is not a duration like 30s, 5m or 1h");
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let number: u64 = number.parse().map_err(|_| invalid())?;
    let seconds = match unit {
        "s" => number,
        "m" => number * 60,
        "h" => number * 3600,
        _ => return Err(invalid()),
    };
    if seconds == 0 {
        return Err(invalid());
    }
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Option<CredentialsConfig>, String> {
        CredentialsConfig::parse(text, Path::new("/etc/grauwolf"))
    }

    #[test]
    fn a_file_without_credentials_has_none() {
        assert_eq!(
            parse("[tool_packages]\nrequire_signatures = false\n"),
            Ok(None)
        );
    }

    #[test]
    fn parses_the_documented_example() {
        let config = parse(
            r#"
[credentials.openbao]
address = "https://openbao.internal:8200/"
auth = { spiffe = { audience = "openbao", role = "grauwolf-agents", mount = "jwt" } }
refresh = "10m"
[credentials.variables]
ATLASSIAN_API_TOKEN = "openbao:kv/agents/reviewer#jira"
OPENAI_API_KEY      = "file:/run/secrets/openai/api-key"
GIT_TOKEN           = "vault:github/token/reviewer#token"
"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.refresh, Duration::from_secs(600));
        let openbao = config.openbao.unwrap();
        assert_eq!(openbao.address, "https://openbao.internal:8200");
        assert_eq!(openbao.login.role, "grauwolf-agents");
        assert_eq!(
            config.variables["ATLASSIAN_API_TOKEN"],
            Source::OpenBao {
                mount: "kv".into(),
                path: "agents/reviewer".into(),
                field: "jira".into()
            }
        );
        assert_eq!(
            config.variables["OPENAI_API_KEY"],
            Source::File("/run/secrets/openai/api-key".into())
        );
        assert!(matches!(
            config.variables["GIT_TOKEN"],
            Source::OpenBao { .. }
        ));
    }

    #[test]
    fn the_defaults_are_five_minutes_and_the_jwt_mount() {
        let config = parse(
            "[credentials.openbao]\naddress = \"http://b:8200\"\n\
             auth = { spiffe = { audience = \"a\", role = \"r\" } }\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.refresh, DEFAULT_REFRESH);
        assert_eq!(config.openbao.unwrap().login.mount, "jwt");
    }

    #[test]
    fn relative_file_paths_are_relative_to_the_config_file() {
        let config = parse("[credentials.variables]\nOPENAI_API_KEY = \"file:keys/openai\"\n")
            .unwrap()
            .unwrap();
        assert_eq!(
            config.variables["OPENAI_API_KEY"],
            Source::File("/etc/grauwolf/keys/openai".into())
        );
    }

    #[test]
    fn rejects_variables_grauwolf_does_not_read() {
        let error = parse("[credentials.variables]\nMY_SECRET = \"file:/x\"\n").unwrap_err();
        assert!(error.contains("MY_SECRET"), "{error}");
    }

    #[test]
    fn rejects_variables_that_are_not_read_through_the_environment() {
        for name in [
            "OTEL_EXPORTER_OTLP_HEADERS",
            "PATH",
            "SPIFFE_ENDPOINT_SOCKET",
        ] {
            let text = format!("[credentials.variables]\n{name} = \"file:/x\"\n");
            let error = parse(&text).unwrap_err();
            assert!(error.contains("cannot come from"), "{error}");
        }
    }

    #[test]
    fn rejects_bad_sources() {
        for source in [
            "env:X",
            "file:",
            "openbao:kv/agents/reviewer",
            "openbao:kv#field",
            "openbao:kv/agents/reviewer#",
        ] {
            let text = format!("[credentials.variables]\nGIT_TOKEN = \"{source}\"\n");
            assert!(parse(&text).is_err(), "{source} should be refused");
        }
    }

    #[test]
    fn an_openbao_source_needs_the_openbao_table() {
        let error = parse("[credentials.variables]\nGIT_TOKEN = \"openbao:kv/a#t\"\n").unwrap_err();
        assert!(error.contains("[credentials.openbao]"), "{error}");
    }

    #[test]
    fn rejects_unknown_keys_and_bad_refresh() {
        assert!(parse("[credentials]\nfoo = 1\n").is_err());
        for refresh in ["5", "0s", "m", "5d", ""] {
            let text = format!(
                "[credentials.openbao]\naddress = \"http://b\"\nrefresh = \"{refresh}\"\n\
                 auth = {{ spiffe = {{ audience = \"a\", role = \"r\" }} }}\n"
            );
            assert!(
                parse(&text).is_err(),
                "refresh {refresh:?} should be refused"
            );
        }
    }
}
