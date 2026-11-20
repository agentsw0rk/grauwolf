//! Pushing a image: blob existence check, monolithic blob upload and manifest `PUT`
//! (distribution API v2). Needs a token with the `push,pull` scope.

use grauwolf_spec::OciReference;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, IF_MATCH, IF_NONE_MATCH, LOCATION};
use reqwest::{Method, StatusCode};

use crate::IMAGE_MANIFEST;
use crate::client::{Request, Scope, base_url, not_found, unexpected};
use crate::{OciError, OciImage, RegistryClient};

/// Largest answer that is read for the small answers of the upload endpoints.
const MAX_ANSWER_BYTES: usize = 64 * 1024;

impl RegistryClient {
    /// Uploads the blobs of `image` that the repository of `reference` does not have yet, then
    /// the manifest under the tag (or digest) of `reference`; returns the manifest digest.
    pub async fn push(
        &self,
        reference: &OciReference,
        image: &OciImage,
    ) -> Result<String, OciError> {
        self.push_if(reference, image, None).await
    }

    /// Like [`push`](Self::push), but the manifest `PUT` is conditional (ADR-0124): with
    /// `Some(Some(digest))` it carries `If-Match: "<digest>"`, with `Some(None)` (nothing there
    /// yet) `If-None-Match: *`. A registry that honours the condition answers `412` when the tag
    /// moved, which is [`OciError::Conflict`]; one that ignores it simply writes, so callers
    /// check afterwards.
    pub async fn push_if(
        &self,
        reference: &OciReference,
        image: &OciImage,
        expected: Option<Option<&str>>,
    ) -> Result<String, OciError> {
        if let Some(digest) = &reference.digest
            && *digest != image.digest
        {
            return Err(OciError::Invalid(format!(
                "the reference names {digest}, the package is {}",
                image.digest
            )));
        }
        let base = format!(
            "{}/v2/{}",
            base_url(&reference.registry),
            reference.repository
        );
        for (digest, bytes) in &image.blobs {
            if !self.has_blob(reference, &base, digest).await? {
                self.upload_blob(reference, &base, digest, bytes).await?;
            }
        }
        let url = format!("{base}/manifests/{}", reference.reference());
        let mut request = Request::new(Method::PUT, &url, Scope::Push)
            .header(CONTENT_TYPE, IMAGE_MANIFEST)
            .body(&image.manifest);
        match expected {
            Some(Some(digest)) => request = request.header(IF_MATCH, format!("\"{digest}\"")),
            Some(None) => request = request.header(IF_NONE_MATCH, "*"),
            None => {}
        }
        let reply = self.send(reference, request, MAX_ANSWER_BYTES).await?;
        if reply.status == StatusCode::PRECONDITION_FAILED {
            return Err(OciError::Conflict {
                registry: reference.registry.clone(),
                what: format!("{}:{}", reference.repository, reference.reference()),
            });
        }
        if !matches!(reply.status, StatusCode::CREATED | StatusCode::OK) {
            return Err(unexpected(reference, &url, reply.status));
        }
        match reply.digest() {
            Some(announced) if announced != image.digest => Err(OciError::DigestMismatch {
                what: format!(
                    "pushed manifest {}/{}",
                    reference.registry, reference.repository
                ),
                expected: image.digest.clone(),
                actual: announced,
            }),
            _ => Ok(image.digest.clone()),
        }
    }

    async fn has_blob(
        &self,
        reference: &OciReference,
        base: &str,
        digest: &str,
    ) -> Result<bool, OciError> {
        let url = format!("{base}/blobs/{digest}");
        let request = Request::new(Method::HEAD, &url, Scope::Push);
        let reply = self.send(reference, request, MAX_ANSWER_BYTES).await?;
        match reply.status {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(unexpected(reference, &url, status)),
        }
    }

    /// Monolithic upload: `POST …/blobs/uploads/`, then `PUT <location>?digest=…` with the bytes.
    async fn upload_blob(
        &self,
        reference: &OciReference,
        base: &str,
        digest: &str,
        bytes: &[u8],
    ) -> Result<(), OciError> {
        let start_url = format!("{base}/blobs/uploads/");
        let start = Request::new(Method::POST, &start_url, Scope::Push).header(CONTENT_LENGTH, "0");
        let started = self.send(reference, start, MAX_ANSWER_BYTES).await?;
        if started.status == StatusCode::NOT_FOUND {
            return Err(not_found(reference, &start_url));
        }
        if started.status != StatusCode::ACCEPTED {
            return Err(unexpected(reference, &start_url, started.status));
        }
        let location = started
            .header(LOCATION.as_str())
            .ok_or_else(|| OciError::Registry {
                registry: reference.registry.clone(),
                reason: "the upload answer has no Location".into(),
            })?;
        let url = upload_url(reference, &location, digest)?;
        let put = Request::new(Method::PUT, &url, Scope::Push)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(bytes);
        let done = self.send(reference, put, MAX_ANSWER_BYTES).await?;
        if done.status != StatusCode::CREATED {
            return Err(unexpected(reference, &url, done.status));
        }
        Ok(())
    }
}

/// The `Location` of an upload session with `digest` added; it must stay on the registry, so
/// that credentials never go anywhere else.
fn upload_url(reference: &OciReference, location: &str, digest: &str) -> Result<String, OciError> {
    let base = base_url(&reference.registry);
    let absolute = if location.starts_with('/') {
        format!("{base}{location}")
    } else {
        location.to_owned()
    };
    if absolute != base && !absolute.starts_with(&format!("{base}/")) {
        return Err(OciError::Registry {
            registry: reference.registry.clone(),
            reason: "the upload location points to another host".into(),
        });
    }
    let separator = if absolute.contains('?') { '&' } else { '?' };
    Ok(format!("{absolute}{separator}digest={digest}"))
}
