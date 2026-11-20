//! Knowledge URLs (ADR-0113): one page per URL, fetched with a conditional request at start.

use serde::{Deserialize, Serialize};

/// Cache validators of a fetched page, kept with the index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlValidators {
    /// `ETag` of the last response.
    pub etag: Option<String>,
    /// `Last-Modified` of the last response.
    pub last_modified: Option<String>,
}

/// What loading a URL produced.
#[derive(Debug, PartialEq, Eq)]
pub enum UrlLoad {
    /// The indexed content stays (`304`, or a failure with content already indexed).
    Unchanged,
    /// The page text and the validators to keep.
    Fetched {
        /// Readable text of the page.
        text: String,
        /// Validators of the response.
        validators: UrlValidators,
    },
}

/// Loads `url`. `known` are the validators of the indexed content (sent as a conditional
/// request); `indexed` says that content for this URL exists in the index. A failure with
/// indexed content keeps it and warns on stderr; without, it is an error naming URL and reason.
#[cfg(feature = "fetch")]
pub async fn load_url(
    url: &str,
    known: Option<&UrlValidators>,
    indexed: bool,
) -> Result<UrlLoad, String> {
    use grauwolf_tools_web::{Page, Validators, fetch_page};
    let sent = known.map_or_else(Validators::default, |known| Validators {
        etag: known.etag.clone(),
        last_modified: known.last_modified.clone(),
    });
    let reason = match fetch_page(url, &sent).await {
        Ok(Page::NotModified) if indexed => return Ok(UrlLoad::Unchanged),
        Ok(Page::NotModified) => "the server answered 304 without a conditional request".to_owned(),
        Ok(Page::Fetched { text, validators }) => {
            return Ok(UrlLoad::Fetched {
                text,
                validators: UrlValidators {
                    etag: validators.etag,
                    last_modified: validators.last_modified,
                },
            });
        }
        Err(reason) => reason,
    };
    if indexed {
        eprintln!("warning: knowledge URL {url}: {reason}; using the last indexed content");
        return Ok(UrlLoad::Unchanged);
    }
    Err(format!("knowledge URL {url}: {reason}"))
}

/// Without the feature `fetch` there is no HTTP client.
#[cfg(not(feature = "fetch"))]
pub async fn load_url(
    _url: &str,
    _known: Option<&UrlValidators>,
    _indexed: bool,
) -> Result<UrlLoad, String> {
    Err("this build has no HTTP client for knowledge URLs (feature `fetch`)".into())
}
