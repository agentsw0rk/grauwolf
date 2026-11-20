//! General OCI distribution client (ADR-0107, ADR-0108): distribution API v2 over reqwest with
//! token authentication, digest verification, push of blobs and manifests, credentials
//! (`GRAUWOLF_REGISTRY_AUTH`, Docker configuration and credential helpers) and the OCI image
//! layout on disk. Nothing in here knows what the artifacts mean; media types and the content of
//! manifests belong to the callers.

mod client;
pub mod cosign;
mod docker;
mod error;
mod image;
mod listing;
mod push;
mod signatures;
#[cfg(feature = "test-registry")]
pub mod testing;

pub use client::{FetchedManifest, REGISTRY_AUTH_ENV, RegistryAuth, RegistryClient, base_url};
pub use docker::{DockerConfig, default_config_path};
pub use error::OciError;
pub use image::{IMAGE_INDEX, IMAGE_MANIFEST, OciImage};

use sha2::{Digest, Sha256};

/// `sha256:<hex>` of `bytes`.
pub fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
