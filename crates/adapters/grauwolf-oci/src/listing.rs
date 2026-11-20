//! Listing what a registry holds (ADR-0124): the tags of a repository (`/v2/<name>/tags/list`) and
//! the repositories of the registry (`/v2/_catalog`, which not every registry offers), with the
//! `Link` pagination of the distribution API.

use grauwolf_spec::OciReference;
use reqwest::header::ACCEPT;
use reqwest::{Method, StatusCode};
use serde::Deserialize;

use crate::client::{Request, Scope, base_url, not_found, unexpected};
use crate::{OciError, RegistryClient};

/// Largest page of a listing that is read.
const MAX_LISTING_BYTES: usize = 8 * 1024 * 1024;
/// At most this many pages are followed.
const MAX_PAGES: usize = 100;

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Repositories {
    #[serde(default)]
    repositories: Option<Vec<String>>,
}

impl RegistryClient {
    /// The tags of the repository of `reference` (its tag and digest are ignored); none when the
    /// repository does not exist.
    pub async fn tags(&self, reference: &OciReference) -> Result<Vec<String>, OciError> {
        let first = format!(
            "{}/v2/{}/tags/list",
            base_url(&reference.registry),
            reference.repository
        );
        let mut tags = Vec::new();
        for page in self.pages(reference, &first, Scope::Pull).await? {
            let page: Tags = serde_json::from_slice(&page).map_err(|error| {
                OciError::Invalid(format!("the tag list is not valid JSON: {error}"))
            })?;
            tags.extend(page.tags.unwrap_or_default());
        }
        Ok(tags)
    }

    /// The repositories of `registry` from `/v2/_catalog`; `None` when the registry does not
    /// offer the listing (404, 405, or it refuses it).
    pub async fn repositories(&self, registry: &str) -> Result<Option<Vec<String>>, OciError> {
        let reference = OciReference {
            registry: registry.to_owned(),
            repository: String::new(),
            tag: None,
            digest: None,
        };
        let first = format!("{}/v2/_catalog", base_url(registry));
        let pages = match self.pages(&reference, &first, Scope::Catalog).await {
            Ok(pages) => pages,
            Err(OciError::NotFound { .. } | OciError::Unauthorized { .. }) => return Ok(None),
            Err(OciError::Registry { reason, .. }) if reason.contains("405") => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut repositories = Vec::new();
        for page in pages {
            let page: Repositories = serde_json::from_slice(&page).map_err(|error| {
                OciError::Invalid(format!("the repository list is not valid JSON: {error}"))
            })?;
            repositories.extend(page.repositories.unwrap_or_default());
        }
        Ok(Some(repositories))
    }

    /// The bodies of `first` and the pages its `Link: <…>; rel="next"` headers lead to. A
    /// missing repository is no page at all.
    async fn pages(
        &self,
        reference: &OciReference,
        first: &str,
        scope: Scope,
    ) -> Result<Vec<Vec<u8>>, OciError> {
        let base = base_url(&reference.registry);
        let mut pages = Vec::new();
        let mut next = Some(first.to_owned());
        while let Some(url) = next.take() {
            if pages.len() == MAX_PAGES {
                return Err(OciError::Registry {
                    registry: reference.registry.clone(),
                    reason: format!("more than {MAX_PAGES} pages in a listing"),
                });
            }
            let request = Request::new(Method::GET, &url, scope).header(ACCEPT, "application/json");
            let reply = self.send(reference, request, MAX_LISTING_BYTES).await?;
            match reply.status {
                status if status.is_success() => {}
                StatusCode::NOT_FOUND if scope == Scope::Pull => return Ok(pages),
                StatusCode::NOT_FOUND => return Err(not_found(reference, &url)),
                status => return Err(unexpected(reference, &url, status)),
            }
            next = reply
                .header("link")
                .and_then(|link| next_link(&link))
                .map(|link| {
                    if link.starts_with('/') {
                        format!("{base}{link}")
                    } else {
                        link
                    }
                })
                // Credentials stay on the registry.
                .filter(|link| link.starts_with(&format!("{base}/")));
            pages.push(reply.body);
        }
        Ok(pages)
    }
}

/// The target of `rel="next"` in a `Link` header.
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        params.contains("rel=\"next\"").then(|| {
            target
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_owned()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_link_is_found() {
        assert_eq!(
            next_link("</v2/_catalog?last=b&n=2>; rel=\"next\"").as_deref(),
            Some("/v2/_catalog?last=b&n=2")
        );
        assert_eq!(next_link("</x>; rel=\"prev\""), None);
    }
}
