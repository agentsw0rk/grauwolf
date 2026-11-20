//! Own smart-HTTP transport for libgit2 (ADR-0104, addendum 2026-11-11), shared by the git tools and the tool packages (ADR-0107).
//!
//! libgit2's HTTP code crashes when a redirect `Location` ends in `/info/refs` without a query
//! (`remove_service_suffix`), and any server the tools talk to could send one. So `http://` and
//! `https://` URLs are served by this module instead: a [`SmartSubtransport`] on reqwest's
//! blocking client that speaks the smart protocol (`GET …/info/refs?service=…`, `POST
//! …/git-upload-pack` and `…/git-receive-pack`) and follows **no** redirect, so libgit2's HTTP
//! code never runs.
//!
//! The transport is registered once for the process but needs per-call settings (token, host,
//! timeout). They travel in a thread-local that [`with_settings`] sets around one blocking
//! libgit2 call: libgit2 creates the transport and calls it on the thread that made the call.

use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::sync::{Arc, Once};

use grauwolf_spiffe::ClientAuth;
use std::time::Duration;

use git2::Error;
use git2::transport::{Service, SmartSubtransport, SmartSubtransportStream, Transport};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE, PRAGMA};
use reqwest::redirect::Policy;
use reqwest::{StatusCode, Url};

/// Sent as `User-Agent`.
const USER_AGENT: &str = "git/2.0 (grauwolf)";
/// Added to the timeout of the call: the call's own limit (in `git.rs`) fires first and gives
/// its message, this only lets an abandoned call end by itself (`spawn_blocking` cannot be
/// cancelled and the directory stays locked until the call returns).
const SAFETY_NET: Duration = Duration::from_secs(1);

/// What one libgit2 call needs to know that the process-wide transport cannot.
#[derive(Clone)]
pub struct Settings {
    /// Token for `Authorization: Basic` (user `x-access-token`), if the repository needs one.
    pub token: Option<String>,
    /// Scheme and host of the configured repository URL: the only place the token goes to.
    pub origin: Option<(String, String)>,
    /// Time limit of the call.
    pub timeout: Duration,
    /// Workload identity instead of a token (ADR-0110): mTLS with the X.509-SVID, the server's
    /// SPIFFE ID checked. The repository URL must be `https://`.
    pub auth: Option<Arc<ClientAuth>>,
    /// A fresh JWT-SVID for `Authorization: Bearer` (jwt mode; fetched by the caller before the
    /// blocking call, because fetching is async). Goes only to the origin like the token.
    pub bearer: Option<String>,
}

thread_local! {
    static SETTINGS: RefCell<Option<Settings>> = const { RefCell::new(None) };
}

/// Runs `call` (one blocking libgit2 call) with `settings` for this thread's HTTP requests.
pub fn with_settings<T>(settings: Settings, call: impl FnOnce() -> T) -> T {
    struct Restore(Option<Settings>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SETTINGS.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(SETTINGS.with(|cell| cell.replace(Some(settings))));
    call()
}

/// Registers the transport for `http` and `https`, once per process.
pub fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for scheme in ["http", "https"] {
            // SAFETY: `register` must not run concurrently with the creation of other
            // transports, because libgit2 keeps the registered transports in a vector without a
            // lock. `Once` serializes our own registration, and it runs in `GitTools::new`,
            // before this crate starts a network operation; the vector gets two entries and is
            // never changed again. Nothing else in the process uses libgit2's network code
            // concurrently with that first call.
            let result = unsafe {
                git2::transport::register(scheme, |remote| {
                    Transport::smart(remote, true, HttpSubtransport::new()?)
                })
            };
            // Registering twice is the only error that can happen, and cannot (`Once`).
            debug_assert!(result.is_ok(), "{result:?}");
        }
    });
}

/// Scheme and host (with port) of a URL in lower case; `None` for paths and other forms.
pub fn origin_of(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split('/').next()?.rsplit('@').next()?;
    Some((scheme.to_ascii_lowercase(), authority.to_ascii_lowercase()))
}

struct HttpSubtransport {
    client: Client,
    settings: Settings,
}

impl HttpSubtransport {
    fn new() -> Result<Self, Error> {
        let settings = SETTINGS
            .with(|cell| cell.borrow().clone())
            .unwrap_or(Settings {
                token: None,
                origin: None,
                timeout: Duration::from_secs(120),
                auth: None,
                bearer: None,
            });
        let mut builder = Client::builder()
            .redirect(Policy::none())
            .user_agent(USER_AGENT)
            .connect_timeout(settings.timeout)
            .timeout(settings.timeout + SAFETY_NET);
        if let Some(auth) = &settings.auth
            && let Some(tls) = auth
                .tls_config()
                .map_err(|error| Error::from_str(&format!("workload identity: {error}")))?
        {
            builder = builder.use_preconfigured_tls(tls);
        }
        let client = builder
            .build()
            .map_err(|error| Error::from_str(&format!("HTTP client: {}", describe(error))))?;
        Ok(Self { client, settings })
    }
}

impl SmartSubtransport for HttpSubtransport {
    fn action(
        &self,
        url: &str,
        action: Service,
    ) -> Result<Box<dyn SmartSubtransportStream>, Error> {
        let (service, listing) = match action {
            Service::UploadPackLs => ("git-upload-pack", true),
            Service::UploadPack => ("git-upload-pack", false),
            Service::ReceivePackLs => ("git-receive-pack", true),
            Service::ReceivePack => ("git-receive-pack", false),
        };
        let mut target =
            Url::parse(url).map_err(|error| Error::from_str(&format!("invalid URL: {error}")))?;
        // Credentials in the URL are not used: the token is the only credential.
        let _ = target.set_username("");
        let _ = target.set_password(None);
        let path = format!("{}/", target.path().trim_end_matches('/'));
        target.set_path(&format!(
            "{path}{}",
            if listing { "info/refs" } else { service }
        ));
        if listing {
            target.set_query(Some(&format!("service={service}")));
        }
        let to_origin = self.settings.origin.is_some() && origin_of(url) == self.settings.origin;
        if self.settings.auth.is_some() && !(to_origin && target.scheme() == "https") {
            return Err(Error::from_str(
                "the workload identity is only used for the configured https:// repository",
            ));
        }
        let token = self.settings.token.clone().filter(|_| to_origin);
        let bearer = self.settings.bearer.clone().filter(|_| to_origin);
        Ok(Box::new(Exchange {
            client: self.client.clone(),
            url: target,
            service,
            listing,
            token,
            bearer,
            body: Vec::new(),
            response: None,
        }))
    }

    fn close(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// One request and its response, as a stream: what is written is buffered as the request body,
/// the request is sent at the first read.
struct Exchange {
    client: Client,
    url: Url,
    service: &'static str,
    /// `GET …/info/refs` (otherwise `POST` of the buffered body).
    listing: bool,
    token: Option<String>,
    bearer: Option<String>,
    body: Vec<u8>,
    response: Option<Response>,
}

impl Exchange {
    fn send(&mut self) -> io::Result<Response> {
        let mut request = if self.listing {
            self.client.get(self.url.clone())
        } else {
            self.client
                .post(self.url.clone())
                .header(
                    CONTENT_TYPE,
                    format!("application/x-{}-request", self.service),
                )
                .body(std::mem::take(&mut self.body))
        };
        request = request
            .header(PRAGMA, "no-cache")
            .header(CACHE_CONTROL, "no-cache")
            .header(
                ACCEPT,
                if self.listing {
                    "*/*".to_owned()
                } else {
                    format!("application/x-{}-result", self.service)
                },
            );
        if let Some(token) = &self.token {
            request = request.basic_auth("x-access-token", Some(token));
        }
        if let Some(bearer) = &self.bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request.send().map_err(|error| {
            if let Some(reason) = grauwolf_spiffe::tls_refusal(&error) {
                let host = self.url.host_str().unwrap_or_default();
                return io::Error::other(format!("TLS handshake with {host} failed: {reason}"));
            }
            io::Error::other(describe(error))
        })?;
        let status = response.status();
        if let Some(reason) = refusal(status) {
            return Err(io::Error::other(reason));
        }
        if self.listing {
            let expected = format!("application/x-{}-advertisement", self.service);
            let kind = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.split(';').next().unwrap_or("").trim().to_owned());
            if kind.as_deref() != Some(expected.as_str()) {
                return Err(io::Error::other(format!(
                    "the server does not speak the smart HTTP protocol (Content-Type {}, expected {expected})",
                    kind.as_deref().unwrap_or("missing")
                )));
            }
        }
        Ok(response)
    }
}

impl Read for Exchange {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.response.is_none() {
            self.response = Some(self.send()?);
        }
        let Some(response) = self.response.as_mut() else {
            return Ok(0);
        };
        response
            .read(buffer)
            .map_err(|error| io::Error::other(describe(error)))
    }
}

impl Write for Exchange {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.body.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Why `status` is not an answer to a smart-protocol request; `None` for success.
fn refusal(status: StatusCode) -> Option<String> {
    let code = status.as_u16();
    match code {
        200..=299 => None,
        300..=399 => Some(format!(
            "the server answered with a redirect (HTTP {code}); redirects are not followed"
        )),
        401 => Some("authentication failed (HTTP 401)".to_owned()),
        403 => Some("access denied (HTTP 403)".to_owned()),
        404 => Some("repository not found (HTTP 404)".to_owned()),
        _ => Some(format!("the server answered HTTP {code}")),
    }
}

/// A request error with its causes, without the URL.
fn describe(error: impl std::error::Error + 'static) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_scheme_and_host_identify_where_the_token_may_go() {
        let same = origin_of("https://GitHub.com/a/b.git");
        assert_eq!(same, origin_of("https://x-access-token@github.com/other"));
        assert_ne!(same, origin_of("https://evil.example.com/a/b.git"));
        assert_ne!(same, origin_of("http://github.com/a/b.git"));
        assert_eq!(origin_of("/srv/repo.git"), None);
    }
}
