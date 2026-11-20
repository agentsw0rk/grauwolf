//! Cosign-compatible signatures of OCI images with public keys (ADR-0107 stage 4): the "simple
//! signing" payload, ECDSA P-256 with SHA-256 (DER, base64) and the verification of the
//! signatures found under the tag `sha256-<hex>.sig`. Keyless signing (Fulcio, Rekor) is not
//! part of this.
//!
//! Keys are PEM: private keys PKCS#8 (unencrypted), public keys SPKI (`cosign.pub`).

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature as EcdsaSignature, SigningKey as EcdsaSigningKey, VerifyingKey};
use p256::pkcs8::{
    DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Media type of the layer that holds the signed payload.
pub const SIMPLE_SIGNING_TYPE: &str = "application/vnd.dev.cosign.simplesigning.v1+json";
/// Annotation of the layer with the base64 DER signature of the payload.
pub const SIGNATURE_ANNOTATION: &str = "dev.cosignproject.cosign/signature";
/// The `type` of the payload's `critical` part.
const PAYLOAD_TYPE: &str = "cosign container image signature";

/// The tag under which the signatures of the image with manifest `digest` are stored.
pub fn signature_tag(digest: &str) -> String {
    format!("{}.sig", digest.replace(':', "-"))
}

/// Why a key or a signature is not usable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CosignError {
    /// A key is not valid PEM of the expected kind.
    #[error("invalid key: {0}")]
    Key(String),
    /// The image has no signature at all.
    #[error("unsigned")]
    Unsigned,
    /// There are signatures, but none is valid for the image and one of the keys.
    #[error("signature invalid: {0}")]
    Invalid(String),
}

/// A private signing key (ECDSA P-256).
pub struct SigningKey(EcdsaSigningKey);

impl SigningKey {
    /// A new random key.
    pub fn generate() -> Self {
        Self(EcdsaSigningKey::random(&mut rand_core::OsRng))
    }

    /// Reads an unencrypted PKCS#8 PEM private key (`-----BEGIN PRIVATE KEY-----`).
    pub fn from_pem(pem: &str) -> Result<Self, CosignError> {
        EcdsaSigningKey::from_pkcs8_pem(pem).map(Self).map_err(|_| {
            CosignError::Key(
                "not an unencrypted PKCS#8 PEM private key for P-256 (cosign's encrypted \
                 private keys are not supported)"
                    .into(),
            )
        })
    }

    /// The private key as PKCS#8 PEM, unencrypted.
    pub fn to_pem(&self) -> String {
        self.0
            .to_pkcs8_pem(LineEnding::LF)
            .expect("a P-256 key encodes")
            .to_string()
    }

    /// The matching public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(*self.0.verifying_key())
    }

    /// Base64 DER ECDSA signature of `payload` (the value of [`SIGNATURE_ANNOTATION`]).
    pub fn sign(&self, payload: &[u8]) -> String {
        let signature: EcdsaSignature = self.0.sign(payload);
        STANDARD.encode(signature.to_der().as_bytes())
    }
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SigningKey(..)")
    }
}

/// A public key (ECDSA P-256) that verifies signatures.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    /// Reads an SPKI PEM public key (`-----BEGIN PUBLIC KEY-----`, as `cosign.pub`).
    pub fn from_pem(pem: &str) -> Result<Self, CosignError> {
        VerifyingKey::from_public_key_pem(pem)
            .map(Self)
            .map_err(|_| CosignError::Key("not an SPKI PEM public key for P-256".into()))
    }

    /// The key as SPKI PEM.
    pub fn to_pem(&self) -> String {
        self.0
            .to_public_key_pem(LineEnding::LF)
            .expect("a P-256 key encodes")
    }

    /// `SHA256:<base64>` of the SPKI encoding, to name the key in messages.
    pub fn fingerprint(&self) -> String {
        let der = self.0.to_public_key_der().expect("a P-256 key encodes");
        format!(
            "SHA256:{}",
            STANDARD_NO_PAD.encode(Sha256::digest(der.as_bytes()))
        )
    }

    /// Does `signature` (base64 DER) match `payload`?
    pub fn verifies(&self, payload: &[u8], signature: &str) -> bool {
        let Ok(der) = STANDARD.decode(signature.trim()) else {
            return false;
        };
        EcdsaSignature::from_der(&der)
            .is_ok_and(|signature| self.0.verify(payload, &signature).is_ok())
    }
}

impl std::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicKey({})", self.fingerprint())
    }
}

/// One signature of an image: the signed payload bytes and the base64 DER signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub payload: Vec<u8>,
    pub signature: String,
}

/// The simple signing payload for the image `identity` (`registry/repository`, no tag) with the
/// manifest `digest`.
pub fn payload(identity: &str, digest: &str) -> Vec<u8> {
    json!({
        "critical": {
            "identity": { "docker-reference": identity },
            "image": { "docker-manifest-digest": digest },
            "type": PAYLOAD_TYPE,
        },
        "optional": null,
    })
    .to_string()
    .into_bytes()
}

/// Verifies that one of `signatures` signs the image `identity` with manifest `digest` and is
/// valid for one of `keys`; returns the fingerprint of that key. Signatures of other images or
/// other keys do not count.
pub fn verify(
    signatures: &[Signature],
    keys: &[PublicKey],
    identity: &str,
    digest: &str,
) -> Result<String, CosignError> {
    if signatures.is_empty() {
        return Err(CosignError::Unsigned);
    }
    let mut about_other = false;
    for signature in signatures {
        if !payload_names(&signature.payload, identity, digest) {
            about_other = true;
            continue;
        }
        if let Some(key) = keys
            .iter()
            .find(|key| key.verifies(&signature.payload, &signature.signature))
        {
            return Ok(key.fingerprint());
        }
    }
    Err(CosignError::Invalid(if about_other {
        format!("no valid signature of {identity}@{digest} by a trusted key")
    } else {
        "no signature matches a trusted key".into()
    }))
}

/// Does the payload sign this very image (digest and repository)?
fn payload_names(payload: &[u8], identity: &str, digest: &str) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    let critical = &value["critical"];
    critical["type"] == PAYLOAD_TYPE
        && critical["image"]["docker-manifest-digest"] == digest
        && critical["identity"]["docker-reference"] == identity
}
