//! `openbao setup`: the JWT auth method with the provider's `jwks_url`, the KV v2 mount and the
//! policy `grauwolf-k8-ops` that lets the tool work without the root token; `openbao token`
//! makes a token with only that policy.

use crate::cli::Settings;
use crate::identity::{NAME as IDENTITY, TRANSIT_MOUNT as TRANSIT};
use crate::io::Io;
use crate::oidc::service_host;
use crate::report;
use crate::services::Services;

/// The policy that holds exactly what this tool needs (ADR-0120).
pub const POLICY: &str = "grauwolf-k8-ops";

/// The token of `openbao token` is valid this long unless `--ttl` says otherwise.
pub const DEFAULT_TTL: &str = "8h";

/// One path of the policy and its capabilities. A path that ends in `*` is a prefix.
pub struct Rule {
    pub path: String,
    pub capabilities: &'static [&'static str],
}

/// What the tool does in OpenBao, derived from its calls (`grauwolf-openbao`'s admin calls and
/// what `doctor` reads), nothing else: no `sys/*` wildcard, no other secrets, no token creation.
/// Every new call needs a rule here, and `openbao setup` must run again to write it.
pub fn rules(settings: &Settings) -> Vec<Rule> {
    let (jwt, kv) = (
        settings.jwt_mount.trim_matches('/'),
        settings.kv_mount.trim_matches('/'),
    );
    let rule = |path: String, capabilities: &'static [&'static str]| Rule { path, capabilities };
    vec![
        // The checks of `doctor` and the token in use.
        rule("sys/health".into(), &["read"]),
        rule("sys/capabilities-self".into(), &["update"]),
        rule("auth/token/lookup-self".into(), &["read"]),
        // `openbao setup`: the auth method (enabling needs `sudo`) and its configuration.
        rule("sys/auth".into(), &["read"]),
        rule(format!("sys/auth/{jwt}"), &["create", "update", "sudo"]),
        rule(format!("auth/{jwt}/config"), &["create", "read", "update"]),
        // The KV mount.
        rule("sys/mounts".into(), &["read"]),
        rule(format!("sys/mounts/{kv}"), &["create", "update"]),
        // `agent add|remove`: the policies and roles `agent-<name>` (the own policy only read).
        rule(
            "sys/policies/acl/agent-*".into(),
            &["create", "read", "update", "delete"],
        ),
        rule(format!("sys/policies/acl/{POLICY}"), &["read"]),
        rule(
            format!("auth/{jwt}/role/agent-*"),
            &["create", "read", "update", "delete"],
        ),
        // `doctor` warns about the shared role of the example.
        rule(format!("auth/{jwt}/role/grauwolf-agents"), &["read"]),
        // `identity install|uninstall` (ADR-0111): the Transit mount and keys, the service's
        // policy and role.
        rule(format!("sys/mounts/{TRANSIT}"), &["create", "update"]),
        rule(
            format!("{TRANSIT}/keys/*"),
            &["create", "read", "update", "delete"],
        ),
        rule(
            format!("sys/policies/acl/{IDENTITY}"),
            &["create", "read", "update", "delete"],
        ),
        rule(
            format!("auth/{jwt}/role/{IDENTITY}"),
            &["create", "read", "update", "delete"],
        ),
        // `identity grants [--revoke]` (ADR-0111 stage 3): the grants' metadata, and deleting a
        // grant's refresh token without reading it.
        rule(format!("{kv}/data/identity/grants/*"), &["read"]),
        rule(format!("{kv}/metadata/identity/grants"), &["list"]),
        rule(
            format!("{kv}/metadata/identity/grants/*"),
            &["read", "list", "delete"],
        ),
        rule(
            format!("{kv}/metadata/identity/grant-tokens/*"),
            &["read", "delete"],
        ),
        // `secret ...` and the server token of `agent deploy`.
        rule(
            format!("{kv}/data/agents/*"),
            &["create", "read", "update", "delete"],
        ),
        rule(
            format!("{kv}/metadata/agents/*"),
            &["read", "list", "delete"],
        ),
    ]
}

/// The policy as HCL.
pub fn policy(settings: &Settings) -> String {
    rules(settings)
        .iter()
        .map(|rule| {
            let capabilities: Vec<String> = rule
                .capabilities
                .iter()
                .map(|capability| format!("\"{capability}\""))
                .collect();
            format!(
                "path \"{}\" {{\n  capabilities = [{}]\n}}\n",
                rule.path,
                capabilities.join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn setup(
    services: &Services,
    io: &mut Io<'_>,
    jwks_url: Option<String>,
    bound_issuer: Option<String>,
) -> Result<(), String> {
    let settings = &services.settings;
    let bao = services.admin()?;
    let jwks_url = jwks_url
        .unwrap_or_else(|| format!("http://{}/keys", service_host(&settings.oidc_namespace)));
    let auth = bao
        .ensure_jwt_auth(&settings.jwt_mount, &jwks_url, bound_issuer.as_deref())
        .await
        .map_err(|error| format!("auth method {}: {error}", settings.jwt_mount))?;
    io.line(&format!(
        "auth method {}/ (jwks_url {jwks_url}): {}",
        settings.jwt_mount,
        report::bao(auth)
    ));
    let kv = bao
        .ensure_kv2(&settings.kv_mount)
        .await
        .map_err(|error| format!("KV mount {}: {error}", settings.kv_mount))?;
    io.line(&format!(
        "KV v2 mount {}/: {}",
        settings.kv_mount,
        report::bao(kv)
    ));
    let own = bao
        .ensure_policy(POLICY, &policy(settings))
        .await
        .map_err(|error| format!("OpenBao policy {POLICY}: {error}"))?;
    io.line(&format!(
        "OpenBao policy {POLICY} (what this tool needs, for `openbao token`): {}",
        report::bao(own)
    ));
    Ok(())
}

/// `openbao token`: a token with only the policy `grauwolf-k8-ops`. Its value is the only thing
/// on stdout (so it can be captured); the rest goes to stderr.
pub async fn token(
    services: &Services,
    io: &mut Io<'_>,
    ttl: &str,
    renewable: bool,
) -> Result<(), String> {
    check_ttl(ttl)?;
    let bao = services.admin()?;
    if bao
        .read_policy(POLICY)
        .await
        .map_err(|error| format!("OpenBao policy {POLICY}: {error}"))?
        .is_none()
    {
        return Err(format!(
            "the policy {POLICY} does not exist: run `grauwolf-k8-ops openbao setup` first (with an admin token)"
        ));
    }
    let token = bao
        .create_token(POLICY, ttl, renewable)
        .await
        .map_err(|error| {
            format!("cannot create the token (it needs the root token or an admin token): {error}")
        })?;
    io.error(&format!(
        "token with the policy {POLICY}, valid for {ttl}{}; use it with `export OPENBAO_TOKEN=...` or --token-file",
        if renewable { ", renewable" } else { "" }
    ));
    io.value(&token);
    Ok(())
}

/// A duration as OpenBao reads it: seconds, or a number with `s`, `m`, `h` or `d`.
fn check_ttl(ttl: &str) -> Result<(), String> {
    let digits = ttl.trim_end_matches(['s', 'm', 'h', 'd']);
    if !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && ttl.len() - digits.len() <= 1
    {
        Ok(())
    } else {
        Err(format!(
            "--ttl {ttl}: not a duration (examples: 30m, 8h, 7d)"
        ))
    }
}
