//! An OCI image: the manifest and all blobs it refers to, written to and read from an OCI image
//! layout directory (`oci-layout`, `index.json`, `blobs/sha256/<hex>`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{OciError, sha256_digest};

/// Media type of an OCI image manifest.
pub const IMAGE_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
/// Media type of an OCI image index.
pub const IMAGE_INDEX: &str = "application/vnd.oci.image.index.v1+json";
const LAYOUT_VERSION: &str = "1.0.0";

/// The manifest of an image and the blobs it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciImage {
    /// Digest of the manifest, `sha256:<hex>`.
    pub digest: String,
    /// The OCI image manifest.
    pub manifest: Vec<u8>,
    /// Digest → content of the config and the layers.
    pub blobs: BTreeMap<String, Vec<u8>>,
}

impl OciImage {
    /// Writes the OCI image layout into `dir` (created if needed; existing blobs are replaced).
    /// The index entry carries `artifact_type`.
    pub fn write_layout(&self, dir: &Path, artifact_type: &str) -> Result<(), OciError> {
        let blob_dir = dir.join("blobs").join("sha256");
        std::fs::create_dir_all(&blob_dir).map_err(|error| io(&blob_dir, error))?;
        write(
            &dir.join("oci-layout"),
            json!({ "imageLayoutVersion": LAYOUT_VERSION })
                .to_string()
                .as_bytes(),
        )?;
        for (digest, bytes) in &self.blobs {
            write(&blob_path(dir, digest)?, bytes)?;
        }
        write(&blob_path(dir, &self.digest)?, &self.manifest)?;
        let index = json!({
            "schemaVersion": 2,
            "mediaType": IMAGE_INDEX,
            "manifests": [{
                "mediaType": IMAGE_MANIFEST,
                "digest": self.digest,
                "size": self.manifest.len(),
                "artifactType": artifact_type,
            }],
        });
        write(&dir.join("index.json"), index.to_string().as_bytes())
    }

    /// Reads an OCI image layout written by [`Self::write_layout`] and verifies every blob
    /// against its digest. Reads the config and the layers (or `blobs`) the manifest names.
    pub fn read_layout(dir: &Path) -> Result<Self, OciError> {
        let invalid = |reason: String| OciError::Invalid(reason);
        let marker: Value = read_json(&dir.join("oci-layout"))?;
        if marker["imageLayoutVersion"] != LAYOUT_VERSION {
            return Err(invalid(format!(
                "{}: unsupported image layout version",
                dir.display()
            )));
        }
        let index: Value = read_json(&dir.join("index.json"))?;
        let manifests = index["manifests"].as_array().map_or(&[][..], Vec::as_slice);
        let [entry] = manifests else {
            return Err(invalid(format!(
                "{}: the index must list exactly one manifest, it lists {}",
                dir.display(),
                manifests.len()
            )));
        };
        let digest = entry["digest"]
            .as_str()
            .ok_or_else(|| invalid("the index entry has no digest".into()))?
            .to_owned();
        let manifest = read_blob(dir, &digest)?;
        let mut blobs = BTreeMap::new();
        for blob in referenced_blobs(&manifest)? {
            let bytes = read_blob(dir, &blob)?;
            blobs.insert(blob, bytes);
        }
        Ok(Self {
            digest,
            manifest,
            blobs,
        })
    }
}

/// Digests of the config and of the layers (the deprecated artifact manifest calls them
/// `blobs`) that `manifest` names.
fn referenced_blobs(manifest: &[u8]) -> Result<Vec<String>, OciError> {
    let value: Value = serde_json::from_slice(manifest)
        .map_err(|error| OciError::Invalid(format!("the manifest is not valid JSON: {error}")))?;
    let digests = std::iter::once(&value["config"])
        .chain(value["layers"].as_array().into_iter().flatten())
        .chain(value["blobs"].as_array().into_iter().flatten())
        .filter_map(|descriptor| descriptor["digest"].as_str())
        .map(str::to_owned)
        .collect();
    Ok(digests)
}

/// `blobs/sha256/<hex>`; the digest is checked so that it cannot name another path.
fn blob_path(dir: &Path, digest: &str) -> Result<PathBuf, OciError> {
    match digest.strip_prefix("sha256:") {
        Some(hex) if hex.len() == 64 && hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) => {
            Ok(dir.join("blobs").join("sha256").join(hex))
        }
        _ => Err(OciError::Invalid(format!("unsupported digest '{digest}'"))),
    }
}

fn read_blob(dir: &Path, digest: &str) -> Result<Vec<u8>, OciError> {
    let path = blob_path(dir, digest)?;
    let bytes = std::fs::read(&path).map_err(|error| io(&path, error))?;
    let actual = sha256_digest(&bytes);
    if actual != digest {
        return Err(OciError::DigestMismatch {
            what: path.display().to_string(),
            expected: digest.to_owned(),
            actual,
        });
    }
    Ok(bytes)
}

fn read_json(path: &Path) -> Result<Value, OciError> {
    let bytes = std::fs::read(path).map_err(|error| io(path, error))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| OciError::Invalid(format!("{}: not valid JSON: {error}", path.display())))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), OciError> {
    std::fs::write(path, bytes).map_err(|error| io(path, error))
}

fn io(path: &Path, error: std::io::Error) -> OciError {
    OciError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> OciImage {
        let config = b"{}".to_vec();
        let layer = b"layer".to_vec();
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": IMAGE_MANIFEST,
            "config": { "mediaType": "x/config", "digest": sha256_digest(&config), "size": 2 },
            "layers": [{ "mediaType": "x/layer", "digest": sha256_digest(&layer), "size": 5 }],
        })
        .to_string()
        .into_bytes();
        OciImage {
            digest: sha256_digest(&manifest),
            manifest,
            blobs: BTreeMap::from([
                (sha256_digest(&config), config),
                (sha256_digest(&layer), layer),
            ]),
        }
    }

    fn temp(name: &str) -> grauwolf_test::TestDir {
        grauwolf_test::TestDir::new(&format!("oci-{name}"))
    }

    #[test]
    fn the_layout_round_trips() {
        let image = image();
        let dir = temp("round-trip");
        image.write_layout(&dir, "x/artifact").unwrap();
        assert!(dir.join("oci-layout").is_file());
        assert!(dir.join("index.json").is_file());
        assert_eq!(OciImage::read_layout(&dir).unwrap(), image);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_blob_is_detected_when_reading() {
        let dir = temp("damaged");
        image().write_layout(&dir, "x/artifact").unwrap();
        std::fs::write(
            blob_path(&dir, &sha256_digest(b"layer")).unwrap(),
            b"tampered",
        )
        .unwrap();
        let error = OciImage::read_layout(&dir).unwrap_err();
        assert!(matches!(error, OciError::DigestMismatch { .. }), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_that_is_no_layout_is_an_error() {
        let dir = temp("empty");
        std::fs::create_dir_all(&dir).unwrap();
        let error = OciImage::read_layout(&dir).unwrap_err();
        assert!(error.to_string().contains("oci-layout"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_digest_cannot_name_another_path() {
        let error = blob_path(Path::new("/x"), "sha256:../../etc/passwd").unwrap_err();
        assert!(error.to_string().contains("unsupported digest"), "{error}");
    }
}
