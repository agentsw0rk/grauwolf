//! AWS Signature Version 4 (ADR-0105): a small own implementation instead of `aws-sigv4`, which
//! pulls in the smithy runtime. Only what Bedrock Runtime needs: signed headers, no query
//! string signing, no streaming payload signing.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Long-lived or temporary AWS credentials. `Debug` never shows the secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Temporary credentials (STS): sent as `x-amz-security-token`.
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// What is signed for: region and service of the credential scope.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub region: &'a str,
    pub service: &'a str,
}

/// The result of signing: header values to add to the request.
#[derive(Debug, PartialEq, Eq)]
pub struct Signed {
    /// The `Authorization` header.
    pub authorization: String,
    /// The `x-amz-date` header (`YYYYMMDD'T'HHMMSS'Z'`).
    pub amz_date: String,
    /// The hex signature alone (for tests).
    pub signature: String,
}

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// Signing key derived from the secret, the day, the region and the service.
pub fn signing_key(secret: &str, date: &str, scope: Scope<'_>) -> Vec<u8> {
    let day = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let region = hmac(&day, scope.region.as_bytes());
    let service = hmac(&region, scope.service.as_bytes());
    hmac(&service, b"aws4_request")
}

/// Percent-encodes everything but the unreserved characters `A-Z a-z 0-9 - _ . ~`.
pub fn uri_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn uri_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%' && index + 2 < bytes.len())
            .then(|| u8::from_str_radix(text.get(index + 1..index + 3)?, 16).ok())
            .flatten();
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Canonical URI for services other than S3: every segment of the path as sent is encoded
/// once more (so `%3A` in a model id becomes `%253A`).
fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        return "/".into();
    }
    path.split('/')
        .map(uri_encode)
        .collect::<Vec<_>>()
        .join("/")
}

/// Query parameters decoded, encoded again and sorted.
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (
                uri_encode(&uri_decode(name)),
                uri_encode(&uri_decode(value)),
            )
        })
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// The parts of a request that are signed. `path` and `query` are as they appear in the URL;
/// `headers` are all headers to sign (including `host` and `x-amz-date`); names are lowercased
/// and sorted when signing.
#[derive(Debug, Clone, Copy)]
pub struct RequestParts<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
}

/// Signs `request` at the time `amz_date`.
pub fn sign(
    credentials: &Credentials,
    scope: Scope<'_>,
    request: &RequestParts<'_>,
    amz_date: &str,
) -> Signed {
    let RequestParts {
        method,
        path,
        query,
        headers,
        body,
    } = *request;
    let mut headers: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| {
            (
                name.to_ascii_lowercase(),
                value.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    headers.sort();
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{method}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{}",
        canonical_uri(path),
        canonical_query(query),
        sha256_hex(body)
    );
    let date = &amz_date[..8];
    let credential_scope = format!("{date}/{}/{}/aws4_request", scope.region, scope.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let key = signing_key(&credentials.secret_access_key, date, scope);
    let signature = hex(&hmac(&key, string_to_sign.as_bytes()));
    Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
            credentials.access_key_id
        ),
        amz_date: amz_date.to_owned(),
        signature,
    }
}

/// `YYYYMMDD'T'HHMMSS'Z'` for `unix_seconds` (UTC).
pub fn amz_date(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let rest = unix_seconds % 86_400;
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}
