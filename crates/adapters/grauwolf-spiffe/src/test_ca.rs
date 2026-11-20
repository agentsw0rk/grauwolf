//! A test certificate authority: issues X.509-SVIDs for given SPIFFE IDs at test time (P-256,
//! DER built by hand and signed with aws-lc-rs; no `rcgen`, no `openssl`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

use crate::{SpiffeId, X509Svid};

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match content.len() {
        n if n < 0x80 => out.push(n as u8),
        n if n < 0x100 => out.extend([0x81, n as u8]),
        n => out.extend([0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(content);
    out
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

const ECDSA_SHA256: [u8; 12] = [
    0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02,
];
const EC_PUBLIC_KEY: [u8; 9] = [0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256: [u8; 10] = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

fn name(common_name: &str) -> Vec<u8> {
    let attribute = seq(&[
        &[0x06, 0x03, 0x55, 0x04, 0x03],
        &tlv(0x0c, common_name.as_bytes()),
    ]);
    seq(&[&tlv(0x31, &attribute)])
}

fn extension(oid_tail: u8, critical: bool, value: &[u8]) -> Vec<u8> {
    let oid = [0x06, 0x03, 0x55, 0x1d, oid_tail];
    if critical {
        seq(&[&oid, &[0x01, 0x01, 0xff], &tlv(0x04, value)])
    } else {
        seq(&[&oid, &tlv(0x04, value)])
    }
}

fn utc_time(secs: i64) -> Vec<u8> {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let text = format!(
        "{:02}{:02}{:02}{:02}{:02}{:02}Z",
        year % 100,
        month,
        day,
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    );
    tlv(0x17, text.as_bytes())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

struct Issued {
    der: Vec<u8>,
    key_pkcs8: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn certificate(
    serial: u64,
    issuer_cn: &str,
    issuer_key: &EcdsaKeyPair,
    subject_cn: &str,
    subject_key: &EcdsaKeyPair,
    uris: &[&str],
    authority: bool,
    valid_for: i64,
) -> Vec<u8> {
    let spki = seq(&[
        &seq(&[&EC_PUBLIC_KEY, &P256]),
        &tlv(
            0x03,
            &[&[0u8][..], subject_key.public_key().as_ref()].concat(),
        ),
    ]);
    let names: Vec<Vec<u8>> = uris.iter().map(|uri| tlv(0x86, uri.as_bytes())).collect();
    let names: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
    let san = extension(0x11, false, &seq(&names));
    let mut extensions = vec![san];
    if authority {
        extensions.push(extension(0x13, true, &seq(&[&[0x01, 0x01, 0xff]])));
        extensions.push(extension(0x0f, true, &[0x03, 0x02, 0x01, 0x06]));
    } else {
        extensions.push(extension(0x13, true, &seq(&[])));
        extensions.push(extension(0x0f, true, &[0x03, 0x02, 0x07, 0x80]));
        let server_auth = [0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
        let client_auth = [0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x02];
        extensions.push(extension(0x25, false, &seq(&[&server_auth, &client_auth])));
    }
    let extensions: Vec<&[u8]> = extensions.iter().map(Vec::as_slice).collect();
    let tbs = seq(&[
        &[0xa0, 0x03, 0x02, 0x01, 0x02],
        &tlv(0x02, &[&[0u8][..], &serial.to_be_bytes()].concat()),
        &ECDSA_SHA256,
        &name(issuer_cn),
        &seq(&[&utc_time(now() - 3600), &utc_time(now() + valid_for)]),
        &name(subject_cn),
        &spki,
        &tlv(0xa3, &seq(&extensions)),
    ]);
    let signature = issuer_key.sign(&SystemRandom::new(), &tbs).expect("sign");
    seq(&[
        &tbs,
        &ECDSA_SHA256,
        &tlv(0x03, &[&[0u8][..], signature.as_ref()].concat()),
    ])
}

fn new_key() -> (EcdsaKeyPair, Vec<u8>) {
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
        .expect("generate a key");
    let bytes = pkcs8.as_ref().to_vec();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &bytes).expect("key");
    (pair, bytes)
}

struct Inner {
    domain: String,
    key: EcdsaKeyPair,
    certificate: Vec<u8>,
    serial: AtomicU64,
}

/// A certificate authority for one trust domain.
#[derive(Clone)]
pub struct TestCa(Arc<Inner>);

impl TestCa {
    /// A new CA for `domain` (its certificate names `spiffe://<domain>`).
    pub fn new(domain: &str) -> Self {
        let (key, _) = new_key();
        let cn = format!("test ca {domain}");
        let certificate = certificate(
            1,
            &cn,
            &key,
            &cn,
            &key,
            &[&format!("spiffe://{domain}")],
            true,
            3650 * 86_400,
        );
        Self(Arc::new(Inner {
            domain: domain.to_owned(),
            key,
            certificate,
            serial: AtomicU64::new(1),
        }))
    }

    /// The trust domain.
    pub fn domain(&self) -> &str {
        &self.0.domain
    }

    /// The bundle: the CA certificate (DER).
    pub fn bundle(&self) -> Vec<Vec<u8>> {
        vec![self.0.certificate.clone()]
    }

    /// An SVID for `id`, valid for a day.
    pub fn issue(&self, id: &str) -> X509Svid {
        self.issue_valid_for(id, 86_400)
    }

    /// An SVID for `id` that expires `seconds` from now (negative: already expired).
    pub fn issue_valid_for(&self, id: &str, seconds: i64) -> X509Svid {
        self.issue_with_uris(id, &[id], seconds)
    }

    /// An SVID for `id` whose certificate carries all of `uris` as URI SANs, to test that more
    /// than one is refused.
    pub fn issue_with_uris(&self, id: &str, uris: &[&str], seconds: i64) -> X509Svid {
        let spiffe_id = SpiffeId::parse(id).expect("a valid SPIFFE ID");
        let (key, pkcs8) = new_key();
        let serial = self.0.serial.fetch_add(1, Ordering::SeqCst) + 1;
        let issued = Issued {
            der: certificate(
                serial,
                &format!("test ca {}", self.0.domain),
                &self.0.key,
                "svid",
                &key,
                uris,
                false,
                seconds,
            ),
            key_pkcs8: pkcs8,
        };
        X509Svid::new(spiffe_id, vec![issued.der], issued.key_pkcs8).expect("a usable SVID")
    }

    /// The raw DER chain and PKCS#8 key of a new SVID, as the Workload API sends them.
    pub fn issue_raw(&self, id: &str) -> (Vec<u8>, Vec<u8>) {
        let (key, pkcs8) = new_key();
        let serial = self.0.serial.fetch_add(1, Ordering::SeqCst) + 1;
        let der = certificate(
            serial,
            &format!("test ca {}", self.0.domain),
            &self.0.key,
            "svid",
            &key,
            &[id],
            false,
            86_400,
        );
        (der, pkcs8)
    }
}
