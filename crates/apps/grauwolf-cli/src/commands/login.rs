//! `grauwolf login | logout` (ADR-0111): a person signs in with Entra (or Entra Local) by the
//! device code flow; the tokens are cached in `<config dir>/login.json` (mode 0600) for calls
//! of agents. `login --print-token` prints the access token (refreshed when needed) for `curl`.
//! No token is printed anywhere else.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use grauwolf_host::identity::{DeviceEndpoints, DeviceLogin, OAuthTokens, peek_claims};
use serde_json::{Value, json};

use crate::cli::{LoginArgs, LogoutArgs};
use crate::output::{Io, outln, report};

/// Entra ID's authority when neither `--authority` nor `GRAUWOLF_LOGIN_AUTHORITY` names one.
pub const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com";
/// The file of the cached tokens, in the config directory.
pub const CACHE_FILE: &str = "login.json";
/// An access token with less left than this is refreshed before it is handed out.
const REFRESH_BEFORE_SECS: u64 = 60;

/// Where the cache lives: `--config-dir`, `GRAUWOLF_CONFIG_DIR`, `$XDG_CONFIG_HOME/grauwolf`,
/// `~/.config/grauwolf`.
fn config_dir(flag: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(dir) = flag {
        return Ok(dir);
    }
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    if let Some(dir) = var("GRAUWOLF_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("grauwolf"));
    }
    var("HOME")
        .map(|home| PathBuf::from(home).join(".config").join("grauwolf"))
        .ok_or_else(|| "no config directory: set --config-dir or GRAUWOLF_CONFIG_DIR".to_owned())
}

/// What a login is made against; stored with the tokens, so later calls need no flags.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    authority: String,
    tenant: String,
    client_id: String,
    scope: String,
}

impl Settings {
    fn device_login(&self) -> Result<DeviceLogin, String> {
        DeviceLogin::new(
            DeviceEndpoints::entra(&self.authority, &self.tenant),
            &self.client_id,
            &request_scope(&self.scope),
        )
    }
}

/// The API scope plus `offline_access`, so that a refresh token comes back.
fn request_scope(scope: &str) -> String {
    if scope
        .split_whitespace()
        .any(|part| part == "offline_access")
    {
        scope.to_owned()
    } else {
        format!("{scope} offline_access")
    }
}

/// The cache file's content.
struct Cache {
    settings: Settings,
    /// The identity service's public URL (`--identity-url`), for `grauwolf identity grant`.
    identity_url: Option<String>,
    access_token: String,
    refresh_token: Option<String>,
    /// Unix seconds.
    expires_at: u64,
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

impl Cache {
    fn new(settings: Settings, identity_url: Option<String>, tokens: OAuthTokens) -> Self {
        Self {
            settings,
            identity_url,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: now() + tokens.expires_in,
        }
    }

    fn to_json(&self) -> Value {
        json!({"authority": self.settings.authority, "tenant": self.settings.tenant,
            "client_id": self.settings.client_id, "scope": self.settings.scope,
            "identity_url": self.identity_url,
            "access_token": self.access_token, "refresh_token": self.refresh_token,
            "expires_at": self.expires_at})
    }

    fn from_json(value: &Value) -> Option<Self> {
        let text = |name: &str| value[name].as_str().map(str::to_owned);
        Some(Self {
            settings: Settings {
                authority: text("authority")?,
                tenant: text("tenant")?,
                client_id: text("client_id")?,
                scope: text("scope")?,
            },
            identity_url: text("identity_url"),
            access_token: text("access_token")?,
            refresh_token: text("refresh_token"),
            expires_at: value["expires_at"].as_u64()?,
        })
    }

    /// `None` without a file; an error names the file, never its content.
    fn read(dir: &Path) -> Result<Option<Self>, String> {
        let file = dir.join(CACHE_FILE);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", file.display())),
        };
        serde_json::from_str::<Value>(&text)
            .ok()
            .as_ref()
            .and_then(Self::from_json)
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "{} is not a login cache of grauwolf: `grauwolf logout`, then `grauwolf login`",
                    file.display()
                )
            })
    }

    /// Written to a new file with mode 0600, then renamed over the old one.
    fn write(&self, dir: &Path) -> Result<PathBuf, String> {
        create_private_dir(dir)?;
        let file = dir.join(CACHE_FILE);
        let partial = dir.join(format!("{CACHE_FILE}.new"));
        let _ = std::fs::remove_file(&partial);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let fail = |error: std::io::Error| format!("{}: {error}", file.display());
        {
            use std::io::Write;
            let mut handle = options.open(&partial).map_err(fail)?;
            handle
                .write_all(self.to_json().to_string().as_bytes())
                .map_err(fail)?;
        }
        std::fs::rename(&partial, &file).map_err(fail)?;
        Ok(file)
    }

    fn seconds_left(&self) -> i64 {
        self.expires_at as i64 - now() as i64
    }
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))
}

/// Flag, then `GRAUWOLF_LOGIN_<NAME>`, then the last login.
fn pick(
    flag: Option<String>,
    variable: &str,
    last: Option<&str>,
    option: &str,
) -> Result<String, String> {
    flag.or_else(|| std::env::var(variable).ok())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| last.map(str::to_owned))
        .ok_or_else(|| format!("grauwolf login needs {option} (or {variable})"))
}

fn settings(args: &LoginArgs, last: Option<&Settings>) -> Result<Settings, String> {
    Ok(Settings {
        authority: pick(
            args.authority.clone(),
            "GRAUWOLF_LOGIN_AUTHORITY",
            last.map(|s| s.authority.as_str()),
            "--authority",
        )
        .unwrap_or_else(|_| DEFAULT_AUTHORITY.to_owned()),
        tenant: pick(
            args.tenant.clone(),
            "GRAUWOLF_LOGIN_TENANT",
            last.map(|s| s.tenant.as_str()),
            "--tenant",
        )?,
        client_id: pick(
            args.client_id.clone(),
            "GRAUWOLF_LOGIN_CLIENT_ID",
            last.map(|s| s.client_id.as_str()),
            "--client-id",
        )?,
        scope: pick(
            args.scope.clone(),
            "GRAUWOLF_LOGIN_SCOPE",
            last.map(|s| s.scope.as_str()),
            "--scope",
        )?,
    })
}

/// `grauwolf login …`.
pub fn run(io: &Io, args: LoginArgs) -> ExitCode {
    if args.status {
        return status(io, args.config_dir);
    }
    if args.print_token {
        return report(io, print_token(io, args.config_dir));
    }
    report(io, login(io, args))
}

/// `grauwolf logout`: removes the cache.
pub fn logout(io: &Io, args: LogoutArgs) -> ExitCode {
    report(
        io,
        config_dir(args.config_dir).and_then(|dir| {
            let file = dir.join(CACHE_FILE);
            match std::fs::remove_file(&file) {
                Ok(()) => {
                    outln!(io, "logged out (removed {})", file.display());
                    Ok(())
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    outln!(io, "not logged in");
                    Ok(())
                }
                Err(error) => Err(format!("{}: {error}", file.display())),
            }
        }),
    )
}

fn login(io: &Io, args: LoginArgs) -> Result<(), String> {
    let dir = config_dir(args.config_dir.clone())?;
    // A broken cache does not stop a new login.
    let last = Cache::read(&dir).ok().flatten();
    let identity_url = args
        .identity_url
        .clone()
        .or_else(|| std::env::var("GRAUWOLF_LOGIN_IDENTITY_URL").ok())
        .filter(|url| !url.trim().is_empty())
        .or_else(|| last.as_ref().and_then(|cache| cache.identity_url.clone()));
    let last = last.map(|cache| cache.settings);
    let settings = settings(&args, last.as_ref())?;
    let login = settings.device_login()?;
    let runtime = crate::errors::runtime()?;
    let tokens = runtime.block_on(async {
        let code = login.start().await?;
        match &code.message {
            Some(message) => outln!(io, "{message}"),
            None => outln!(
                io,
                "To sign in, open {} in a browser and enter the code {}.",
                code.verification_uri,
                code.user_code
            ),
        }
        outln!(
            io,
            "waiting for the sign-in (at most {} s) ...",
            code.expires_in
        );
        login.wait(&code).await
    })?;
    let cache = Cache::new(settings, identity_url, tokens);
    let file = cache.write(&dir)?;
    outln!(
        io,
        "logged in as {} at tenant {}; tokens cached in {}",
        who(&cache.access_token),
        cache.settings.tenant,
        file.display()
    );
    Ok(())
}

/// `Name <upn> (oid …)` from the unverified claims, for display only; `oid …` when the token names
/// nobody (Entra Local).
fn who(token: &str) -> String {
    let Some(claims) = peek_claims(token) else {
        return "(the access token is not a JWT)".to_owned();
    };
    let text = |name: &str| claims[name].as_str().map(str::to_owned);
    let upn = text("upn").or_else(|| text("preferred_username"));
    let mut parts = Vec::new();
    if let Some(name) = text("name") {
        parts.push(name);
    }
    if let Some(upn) = upn {
        parts.push(format!("<{upn}>"));
    }
    match (text("oid"), parts.is_empty()) {
        (Some(oid), true) => return format!("oid {oid}"),
        (Some(oid), false) => parts.push(format!("(oid {oid})")),
        (None, _) => {}
    }
    if parts.is_empty() {
        "(unnamed)".to_owned()
    } else {
        parts.join(" ")
    }
}

fn duration(seconds: i64) -> String {
    let seconds = seconds.unsigned_abs();
    if seconds >= 3600 {
        format!("{} h {} min", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

/// Exit code 0 when logged in (also with an expired access token that can be refreshed), else 1.
fn status(io: &Io, dir: Option<PathBuf>) -> ExitCode {
    let cache = match config_dir(dir).and_then(|dir| Cache::read(&dir)) {
        Ok(Some(cache)) => cache,
        Ok(None) => {
            outln!(io, "not logged in (`grauwolf login`)");
            return ExitCode::from(1);
        }
        Err(error) => return report(io, Err(error)),
    };
    let left = cache.seconds_left();
    outln!(io, "logged in as {}", who(&cache.access_token));
    outln!(
        io,
        "tenant {}, authority {}, client {}, scope {}",
        cache.settings.tenant,
        cache.settings.authority,
        cache.settings.client_id,
        cache.settings.scope
    );
    if left > 0 {
        outln!(io, "access token valid for {}", duration(left));
    } else {
        outln!(io, "access token expired {} ago", duration(left));
    }
    let refresh = cache.refresh_token.is_some();
    outln!(
        io,
        "refresh token: {}",
        if refresh {
            "yes"
        } else {
            "no (sign in again when the access token expires)"
        }
    );
    if left > 0 || refresh {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// The access token on stdout, nothing else; refreshed first when it has less than a minute left.
fn print_token(io: &Io, dir: Option<PathBuf>) -> Result<(), String> {
    outln!(io, "{}", current(dir)?.access_token);
    Ok(())
}

/// The cached login of a person, its access token valid for at least a minute.
pub(crate) struct Login {
    pub access_token: String,
    pub identity_url: Option<String>,
    settings: Settings,
}

impl Login {
    /// A new device code sign-in of the same client and scope with `offline_access` (for a
    /// grant's own token family; nothing of it is cached).
    pub fn grant_login(&self) -> Result<DeviceLogin, String> {
        self.settings.device_login()
    }
}

/// The cached login (`dir`: `--config-dir`), refreshed first when its access token has less than
/// a minute left.
pub(crate) fn current(dir: Option<PathBuf>) -> Result<Login, String> {
    let dir = config_dir(dir)?;
    let cache = Cache::read(&dir)?.ok_or("not logged in: run `grauwolf login` first")?;
    let cache = if cache.seconds_left() > REFRESH_BEFORE_SECS as i64 {
        cache
    } else {
        let refresh = cache
            .refresh_token
            .clone()
            .ok_or("the login has expired and has no refresh token: run `grauwolf login` again")?;
        let login = cache.settings.device_login()?;
        let tokens = crate::errors::runtime()?
            .block_on(login.refresh(&refresh))
            .map_err(|error| format!("{error}; run `grauwolf login` again"))?;
        let fresh = Cache::new(cache.settings, cache.identity_url, tokens);
        fresh.write(&dir)?;
        fresh
    };
    Ok(Login {
        access_token: cache.access_token,
        identity_url: cache.identity_url,
        settings: cache.settings,
    })
}
