//! Fetching a web page as knowledge text (ADR-0113): conditional requests, same-origin
//! redirects only, a size limit, and text, Markdown and HTML as the accepted formats.

use std::time::Duration;

use reqwest::{StatusCode, Url, header, redirect::Policy};

use crate::html::html_to_text;

/// At most this many bytes of a page are accepted.
pub const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
/// At most this many redirects are followed (within the same origin).
const MAX_REDIRECTS: usize = 5;
/// Time limit of one request.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Cache validators of a page, sent back as `If-None-Match`/`If-Modified-Since`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    /// `ETag` of the last response.
    pub etag: Option<String>,
    /// `Last-Modified` of the last response.
    pub last_modified: Option<String>,
}

/// Result of [`fetch_page`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// The server answered `304`: the content is unchanged.
    NotModified,
    /// The page as text, with the validators of the response.
    Fetched {
        /// Readable text.
        text: String,
        /// Validators to send next time.
        validators: Validators,
    },
}

/// Fetches `url` (sending `validators` if they are not empty). Redirects are followed only
/// within the same origin. Errors name the reason and never the credentials (the spec forbids
/// credentials in the URL).
pub async fn fetch_page(url: &str, validators: &Validators) -> Result<Page, String> {
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(TIMEOUT)
        .user_agent("grauwolf")
        .build()
        .map_err(|error| error.to_string())?;
    let origin = Url::parse(url).map_err(|error| format!("invalid URL: {error}"))?;
    let mut current = origin.clone();
    for _ in 0..=MAX_REDIRECTS {
        let mut request = client.get(current.clone());
        if let Some(etag) = &validators.etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &validators.last_modified {
            request = request.header(header::IF_MODIFIED_SINCE, modified);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| format!("request failed: {}", describe(error)))?;
        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(Page::NotModified);
        }
        if status.is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| format!("HTTP {status} without a Location"))?;
            let next = current
                .join(location)
                .map_err(|error| format!("invalid redirect: {error}"))?;
            if next.origin() != origin.origin() {
                return Err(format!(
                    "redirects to another origin ({}); list the final URL instead",
                    next.host_str().unwrap_or("?")
                ));
            }
            current = next;
            continue;
        }
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        let header_text = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let content_type = header_text(header::CONTENT_TYPE);
        let fetched = Validators {
            etag: header_text(header::ETAG),
            last_modified: header_text(header::LAST_MODIFIED),
        };
        let too_big = || format!("larger than {} MiB", MAX_PAGE_BYTES / (1024 * 1024));
        if response
            .content_length()
            .is_some_and(|length| length > MAX_PAGE_BYTES as u64)
        {
            return Err(too_big());
        }
        let format = Format::of(content_type.as_deref(), &current)?;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("reading the body failed: {}", describe(error)))?
        {
            if body.len() + chunk.len() > MAX_PAGE_BYTES {
                return Err(too_big());
            }
            body.extend_from_slice(&chunk);
        }
        let charset = content_type.as_deref().and_then(charset_of);
        let text = decode(&body, charset.as_deref())?;
        let text = match format {
            Format::Plain => text,
            Format::Html => html_to_text(&text),
        };
        return Ok(Page::Fetched {
            text,
            validators: fetched,
        });
    }
    Err(format!("more than {MAX_REDIRECTS} redirects"))
}

enum Format {
    Plain,
    Html,
}

impl Format {
    /// By `Content-Type`; without a useful one, by the extension of the path.
    fn of(content_type: Option<&str>, url: &Url) -> Result<Self, String> {
        let media = content_type
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty() && value != "application/octet-stream");
        let unsupported = |what: &str| {
            format!("unsupported content type `{what}`; only text, Markdown and HTML are indexed")
        };
        match media.as_deref() {
            Some("text/plain" | "text/markdown" | "text/x-markdown") => Ok(Self::Plain),
            Some("text/html" | "application/xhtml+xml") => Ok(Self::Html),
            Some(other) => Err(unsupported(other)),
            None => {
                let path = url.path().to_ascii_lowercase();
                match path.rsplit_once('.').map(|(_, extension)| extension) {
                    Some("txt" | "md" | "markdown") => Ok(Self::Plain),
                    Some("html" | "htm" | "xhtml") => Ok(Self::Html),
                    _ => Err(unsupported("unknown")),
                }
            }
        }
    }
}

fn charset_of(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_ascii_lowercase())
    })
}

/// UTF-8 (default) or Latin-1 family; other charsets are refused.
fn decode(body: &[u8], charset: Option<&str>) -> Result<String, String> {
    match charset {
        None | Some("utf-8" | "utf8" | "us-ascii") => {
            Ok(String::from_utf8_lossy(body).into_owned())
        }
        Some("iso-8859-1" | "latin1" | "windows-1252") => {
            Ok(body.iter().map(|&byte| char::from(byte)).collect())
        }
        Some(other) => Err(format!("unsupported charset `{other}`")),
    }
}

/// The error with its causes (e.g. `connection refused`), without the URL.
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    parts.join(": ")
}
