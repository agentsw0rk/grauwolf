//! Reading and pushing cosign signatures of an image (ADR-0107 stage 4): they live in the same
//! repository as a small image under the tag `sha256-<hex>.sig`, one layer per signature.

use std::collections::BTreeMap;

use grauwolf_spec::OciReference;
use serde_json::{Value, json};

use crate::cosign::{SIGNATURE_ANNOTATION, SIMPLE_SIGNING_TYPE, Signature, SigningKey};
use crate::cosign::{payload, signature_tag};
use crate::{IMAGE_MANIFEST, OciError, OciImage, RegistryClient, sha256_digest};

/// Media type of the (empty) config of a signature image.
const CONFIG_TYPE: &str = "application/vnd.oci.image.config.v1+json";
/// The config blob cosign writes for its signature images.
const CONFIG: &str = r#"{"architecture":"","config":{},"created":"0001-01-01T00:00:00Z","history":[{"created":"0001-01-01T00:00:00Z"}],"os":"","rootfs":{"diff_ids":[""],"type":"layers"}}"#;

/// A signature layer as it is in the signature image: its descriptor (kept as it is, with all
/// annotations) and the signature.
struct Layer {
    descriptor: Value,
    signature: Signature,
}

impl RegistryClient {
    /// The signatures stored for the image with manifest `digest` in the repository of
    /// `reference`; none when there is no signature image. Layers without signature annotation
    /// or of another media type are ignored.
    pub async fn signatures(
        &self,
        reference: &OciReference,
        digest: &str,
    ) -> Result<Vec<Signature>, OciError> {
        Ok(self
            .signature_layers(reference, digest)
            .await?
            .into_iter()
            .map(|layer| layer.signature)
            .collect())
    }

    /// Signs the image with manifest `digest` with `key` and pushes the signature next to it
    /// (identity `registry/repository` of `reference`); signatures already there are kept.
    pub async fn push_signature(
        &self,
        reference: &OciReference,
        digest: &str,
        key: &SigningKey,
    ) -> Result<(), OciError> {
        let identity = format!("{}/{}", reference.registry, reference.repository);
        let payload = payload(&identity, digest);
        let signature = Signature {
            signature: key.sign(&payload),
            payload,
        };
        let mut layers = self.signature_layers(reference, digest).await?;
        if layers.iter().any(|layer| layer.signature == signature) {
            return Ok(());
        }
        layers.push(Layer {
            descriptor: layer_descriptor(&signature),
            signature,
        });
        let image = signature_image(&layers);
        self.push(&signature_reference(reference, digest), &image)
            .await
            .map(|_| ())
    }

    async fn signature_layers(
        &self,
        reference: &OciReference,
        digest: &str,
    ) -> Result<Vec<Layer>, OciError> {
        let target = signature_reference(reference, digest);
        let manifest = match self.manifest(&target).await {
            Ok(manifest) => manifest,
            Err(OciError::NotFound { .. }) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let value: Value = serde_json::from_slice(&manifest.bytes).map_err(|error| {
            OciError::Invalid(format!("the signature manifest is not valid JSON: {error}"))
        })?;
        let mut layers = Vec::new();
        for descriptor in value["layers"].as_array().into_iter().flatten() {
            let (Some(media_type), Some(blob), Some(signature)) = (
                descriptor["mediaType"].as_str(),
                descriptor["digest"].as_str(),
                descriptor["annotations"][SIGNATURE_ANNOTATION].as_str(),
            ) else {
                continue;
            };
            if media_type != SIMPLE_SIGNING_TYPE {
                continue;
            }
            let payload = self.blob(&target, blob).await?;
            layers.push(Layer {
                descriptor: descriptor.clone(),
                signature: Signature {
                    payload,
                    signature: signature.to_owned(),
                },
            });
        }
        Ok(layers)
    }
}

fn signature_reference(reference: &OciReference, digest: &str) -> OciReference {
    OciReference {
        registry: reference.registry.clone(),
        repository: reference.repository.clone(),
        tag: Some(signature_tag(digest)),
        digest: None,
    }
}

fn layer_descriptor(signature: &Signature) -> Value {
    json!({
        "mediaType": SIMPLE_SIGNING_TYPE,
        "digest": sha256_digest(&signature.payload),
        "size": signature.payload.len(),
        "annotations": { SIGNATURE_ANNOTATION: signature.signature },
    })
}

fn signature_image(layers: &[Layer]) -> OciImage {
    let config = CONFIG.as_bytes().to_vec();
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": IMAGE_MANIFEST,
        "config": {
            "mediaType": CONFIG_TYPE,
            "digest": sha256_digest(&config),
            "size": config.len(),
        },
        "layers": layers.iter().map(|layer| &layer.descriptor).collect::<Vec<_>>(),
    })
    .to_string()
    .into_bytes();
    let mut blobs: BTreeMap<String, Vec<u8>> = layers
        .iter()
        .map(|layer| {
            (
                sha256_digest(&layer.signature.payload),
                layer.signature.payload.clone(),
            )
        })
        .collect();
    blobs.insert(sha256_digest(&config), config);
    OciImage {
        digest: sha256_digest(&manifest),
        manifest,
        blobs,
    }
}
