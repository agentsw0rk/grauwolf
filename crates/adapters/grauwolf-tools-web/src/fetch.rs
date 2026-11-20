//! The `fetch` tool (ADR-0098): HTTP requests to hosts the spec names.
//!
//! Guards against requests into the own network (SSRF): the host must be on the allow list
//! (exact or `*.domain`), HTTPS is required unless `allow_http` is set, the host name is
//! resolved first and the connection goes to the checked address, addresses that are private,
//! loopback or link-local are refused unless the host is listed exactly, and every redirect
//! is checked again.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_spiffe::ClientAuth;
use grauwolf_tools::{DynTool, ToolError};
use reqwest::{Method, Url, header::HeaderMap, redirect::Policy};
use serde_json::{Value, json};

/// Names of the fetch tool.
pub const FETCH_TOOLS: &[&str] = &["fetch"];

/// The body is cut off after this many bytes.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// At most this many redirects are followed.
const MAX_REDIRECTS: usize = 5;
/// Time limit of a whole request.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Settings of the fetch tool (`builtin_tools.fetch`).
#[derive(Debug, Clone, Default)]
pub struct FetchConfig {
    /// Hosts that may be called: an exact host name or IP, or `*.domain` for all subdomains.
    pub allowed_hosts: Vec<String>,
    /// Also allow plain `http://` (default: HTTPS only).
    pub allow_http: bool,
    /// Workload identity per host (ADR-0110): `(host pattern, authentication)`; the first
    /// pattern that matches the host of a request decides. Such hosts are only called over
    /// `https://`, with mTLS (the server's SPIFFE ID is checked) or a fresh JWT-SVID as
    /// `Authorization: Bearer`. Hosts without an entry get no identity.
    pub auth: Vec<(String, Arc<ClientAuth>)>,
}

/// Port: resolves a host name to addresses (replaceable in tests).
pub trait Resolver: Send + Sync {
    /// The addresses of `host`.
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>, String>>;
}

/// The system resolver.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>, String>> {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host, 0))
                .await
                .map_err(|error| error.to_string())?;
            Ok(addresses.map(|address| address.ip()).collect())
        })
    }
}

/// `fetch { url, method?, headers?, body? }` under `config`.
pub fn fetch_tool(config: FetchConfig) -> Arc<dyn DynTool> {
    fetch_tool_with(config, Arc::new(SystemResolver))
}

/// Like [`fetch_tool`] with its own [`Resolver`].
pub fn fetch_tool_with(config: FetchConfig, resolver: Arc<dyn Resolver>) -> Arc<dyn DynTool> {
    Arc::new(FetchTool { config, resolver })
}

struct FetchTool {
    config: FetchConfig,
    resolver: Arc<dyn Resolver>,
}

/// Whether `host` is matched by `pattern`: exactly, or `*.domain` for subdomains.
fn host_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    match pattern.strip_prefix("*.") {
        Some(domain) => host.len() > domain.len() + 1 && host.ends_with(&format!(".{domain}")),
        None => pattern == host,
    }
}

/// `true` for addresses inside a network: loopback, private, link-local, unspecified, shared.
fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                // Shared address space 100.64.0.0/10 (carrier-grade NAT).
                || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_internal(IpAddr::V4(mapped));
            }
            ip.is_loopback()
                || ip.is_unspecified()
                // Unique local fc00::/7 and link-local fe80::/10.
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

impl FetchTool {
    fn failure(message: impl Into<String>) -> ToolError {
        ToolError::Execution(message.into())
    }

    /// `true` if `host` is on the list; `exact` tells whether by an exact entry.
    fn listed(&self, host: &str) -> Option<bool> {
        let host = host.to_ascii_lowercase();
        let mut found = None;
        for entry in &self.config.allowed_hosts {
            let entry = entry.trim().to_ascii_lowercase();
            if entry == host {
                return Some(true);
            }
            if let Some(domain) = entry.strip_prefix("*.")
                && host.len() > domain.len() + 1
                && host.ends_with(&format!(".{domain}"))
            {
                found = Some(false);
            }
        }
        found
    }

    /// The authentication of the first entry of `auth` that matches `host`.
    fn auth_for(&self, host: &str) -> Option<&Arc<ClientAuth>> {
        let host = host.to_ascii_lowercase();
        self.config
            .auth
            .iter()
            .find(|(pattern, _)| host_matches(pattern, &host))
            .map(|(_, auth)| auth)
    }

    /// Checks scheme and host of `url` and returns the address to connect to.
    async fn check(&self, url: &Url) -> Result<(String, SocketAddr), ToolError> {
        match url.scheme() {
            "https" => {}
            "http" if self.config.allow_http => {}
            "http" => {
                return Err(Self::failure(
                    "refused: only https:// URLs may be fetched (the spec does not set allow_http)",
                ));
            }
            other => {
                return Err(Self::failure(format!(
                    "refused: scheme '{other}' is not supported"
                )));
            }
        }
        let host = url
            .host_str()
            .ok_or_else(|| Self::failure("refused: the URL has no host"))?
            .trim_matches(['[', ']'])
            .to_owned();
        let exact = self.listed(&host).ok_or_else(|| {
            Self::failure(format!(
                "refused: host '{host}' is not in allowed_hosts ({})",
                self.config.allowed_hosts.join(", ")
            ))
        })?;
        let port = url.port_or_known_default().unwrap_or(443);
        let addresses = match host.parse::<IpAddr>() {
            Ok(ip) => vec![ip],
            Err(_) => self
                .resolver
                .resolve(&host)
                .await
                .map_err(|error| Self::failure(format!("cannot resolve '{host}': {error}")))?,
        };
        let address = addresses
            .iter()
            .copied()
            .find(|ip| exact || !is_internal(*ip))
            .ok_or_else(|| {
                if addresses.is_empty() {
                    Self::failure(format!("cannot resolve '{host}': no address"))
                } else {
                    Self::failure(format!(
                        "refused: '{host}' resolves to a private, loopback or link-local address; \
                         list the host exactly in allowed_hosts to allow it"
                    ))
                }
            })?;
        Ok((host, SocketAddr::new(address, port)))
    }

    async fn run(&self, args: Value) -> Result<String, ToolError> {
        let invalid = |reason: &str| ToolError::InvalidArgs {
            tool: "fetch".to_owned(),
            reason: reason.to_owned(),
        };
        let url = args["url"]
            .as_str()
            .ok_or_else(|| invalid("missing field 'url' (text)"))?;
        let mut url =
            Url::parse(url).map_err(|error| Self::failure(format!("invalid URL: {error}")))?;
        let mut method = Method::from_bytes(
            args["method"]
                .as_str()
                .unwrap_or("GET")
                .to_ascii_uppercase()
                .as_bytes(),
        )
        .map_err(|_| invalid("'method' is not an HTTP method"))?;
        let mut headers = HeaderMap::new();
        if let Some(map) = args["headers"].as_object() {
            for (name, value) in map {
                let value = value
                    .as_str()
                    .ok_or_else(|| invalid("'headers' must map names to text"))?;
                headers.insert(
                    reqwest::header::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| invalid("a header name is not valid"))?,
                    reqwest::header::HeaderValue::from_str(value)
                        .map_err(|_| invalid("a header value is not valid"))?,
                );
            }
        }
        let mut body = args["body"].as_str().map(str::to_owned);
        for _ in 0..=MAX_REDIRECTS {
            let (host, address) = self.check(&url).await?;
            let mut builder = reqwest::Client::builder()
                .redirect(Policy::none())
                .timeout(TIMEOUT)
                .resolve(&host, address);
            let mut bearer = None;
            let mut delegation = None;
            if let Some(auth) = self.auth_for(&host) {
                // Never a silent fall back to plain HTTP: the identity needs TLS.
                if url.scheme() != "https" {
                    return Err(Self::failure(format!(
                        "refused: '{host}' needs a workload identity (`auth`), which only works \
                         over https:// (not {})",
                        url.scheme()
                    )));
                }
                if let Some(tls) = auth.tls_config().map_err(|error| {
                    Self::failure(format!("workload identity for '{host}': {error}"))
                })? {
                    builder = builder.use_preconfigured_tls(tls);
                }
                bearer = auth.bearer().await.map_err(|error| {
                    Self::failure(format!("workload identity for '{host}': {error}"))
                })?;
                // The person the run acts for (ADR-0111): only to hosts with a workload
                // identity, never to external ones.
                delegation = grauwolf_tools::delegation_header()
                    .await
                    .map_err(Self::failure)?;
            }
            let client = builder
                .build()
                .map_err(|error| Self::failure(error.to_string()))?;
            let mut request = client
                .request(method.clone(), url.clone())
                .headers(headers.clone());
            if let Some(token) = bearer {
                request = request.bearer_auth(token);
            }
            if let Some((name, token)) = delegation {
                request = request.header(name, token);
            }
            if let Some(body) = &body {
                request = request.body(body.clone());
            }
            let response = request.send().await.map_err(|error| {
                if let Some(reason) = grauwolf_spiffe::tls_refusal(&error) {
                    return Self::failure(format!(
                        "TLS handshake with {host} failed: {reason}; the request was not sent"
                    ));
                }
                Self::failure(format!("request failed: {}", describe(error)))
            })?;
            if response.status().is_redirection()
                && let Some(location) = response.headers().get(reqwest::header::LOCATION)
            {
                let location = location
                    .to_str()
                    .map_err(|_| Self::failure("the redirect has an unreadable Location"))?;
                url = url
                    .join(location)
                    .map_err(|error| Self::failure(format!("invalid redirect target: {error}")))?;
                // As browsers do: 301/302/303 turn into GET without a body.
                if response.status().as_u16() != 307 && response.status().as_u16() != 308 {
                    method = Method::GET;
                    body = None;
                }
                continue;
            }
            return render(response).await;
        }
        Err(Self::failure(format!(
            "too many redirects (more than {MAX_REDIRECTS})"
        )))
    }
}

/// The error with its causes, without the URL (it may carry secrets).
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

/// Status line, a few headers and the body as text.
async fn render(mut response: reqwest::Response) -> Result<String, ToolError> {
    let status = response.status();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let (content_type, content_length) = (header("content-type"), header("content-length"));
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        ToolError::Execution(format!("reading the response failed: {}", describe(error)))
    })? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_BODY_BYTES {
            bytes.truncate(MAX_BODY_BYTES);
            truncated = true;
            break;
        }
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if content_type
        .as_deref()
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("html"))
    {
        text = html_to_text(&text);
    }
    let mut out = format!(
        "HTTP {} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
    .trim_end()
    .to_owned();
    if let Some(kind) = content_type {
        out.push_str(&format!("\ncontent-type: {kind}"));
    }
    if let Some(length) = content_length {
        out.push_str(&format!("\ncontent-length: {length}"));
    }
    out.push_str("\n\n");
    out.push_str(&text);
    if truncated {
        out.push_str("\n[truncated after 1 MiB]");
    }
    Ok(out)
}

/// HTML reduced to text: scripts, styles and comments dropped, tags removed, common entities
/// decoded, whitespace collapsed.
fn html_to_text(html: &str) -> String {
    let drop =
        regex::Regex::new(r"(?is)<script\b.*?</script\s*>|<style\b.*?</style\s*>|<!--.*?-->")
            .expect("valid regex");
    let block = regex::Regex::new(
        r"(?i)</?(p|div|br|li|ul|ol|h[1-6]|tr|table|section|article|header|footer)\b[^>]*>",
    )
    .expect("valid regex");
    let tags = regex::Regex::new(r"(?s)<[^>]*>").expect("valid regex");
    let without = drop.replace_all(html, " ");
    let broken = block.replace_all(&without, "\u{1}");
    let text = tags.replace_all(&broken, " ");
    let text = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    text.split('\u{1}')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

impl DynTool for FetchTool {
    fn name(&self) -> &str {
        "fetch"
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "fetch".to_owned(),
            description: format!(
                "Makes an HTTP request and returns the status line, a few headers and the body \
                 as text (HTML is reduced to its text; at most 1 MiB). Only these hosts may be \
                 called: {}.",
                self.config.allowed_hosts.join(", ")
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL to fetch" },
                    "method": { "type": "string", "description": "HTTP method (default GET)" },
                    "headers": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Request headers"
                    },
                    "body": { "type": "string", "description": "Request body" }
                },
                "required": ["url"]
            }),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(self.run(args))
    }
}
