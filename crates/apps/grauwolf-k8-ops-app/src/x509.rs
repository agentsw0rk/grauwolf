//! The SPIFFE trust domains of the CA certificates in a bundle, read from the URI subject
//! alternative names (`spiffe://<trust domain>`). A minimal DER walk: no certificate is
//! validated, only the one extension is found (the lockfile has no X.509 parser, and this one
//! question does not justify a new dependency).

/// The trust domains named by the URI SANs of all certificates in `pem` (a bundle may hold
/// several), in order, without duplicates.
pub fn spiffe_trust_domains(pem: &str) -> Vec<String> {
    let mut domains: Vec<String> = Vec::new();
    for der in pem_blocks(pem) {
        for uri in uri_sans(&der) {
            if let Some(domain) = uri
                .strip_prefix("spiffe://")
                .map(|rest| rest.split('/').next().unwrap_or(""))
                .filter(|domain| !domain.is_empty())
                && !domains.iter().any(|known| known == domain)
            {
                domains.push(domain.to_owned());
            }
        }
    }
    domains
}

/// The decoded bodies of the `CERTIFICATE` blocks.
fn pem_blocks(pem: &str) -> Vec<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut blocks = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(BEGIN) {
        let body = &rest[start + BEGIN.len()..];
        let Some(end) = body.find(END) else { break };
        if let Some(der) = base64_decode(&body[..end]) {
            blocks.push(der);
        }
        rest = &body[end + END.len()..];
    }
    blocks
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut bits, mut count) = (0u32, 0);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

/// One DER element: `(tag, content, rest after it)`.
fn element(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *data.first()?;
    let first = *data.get(1)?;
    let (length, header) = if first < 0x80 {
        (usize::from(first), 2)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let bytes = data.get(2..2 + count)?;
        (
            bytes.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b)),
            2 + count,
        )
    };
    let content = data.get(header..header.checked_add(length)?)?;
    Some((tag, content, &data[header + length..]))
}

fn children(mut data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut found = Vec::new();
    while let Some((tag, content, rest)) = element(data) {
        found.push((tag, content));
        data = rest;
    }
    found
}

const SEQUENCE: u8 = 0x30;
const OCTET_STRING: u8 = 0x04;
const OID: u8 = 0x06;
const EXTENSIONS: u8 = 0xa3;
const URI_NAME: u8 = 0x86;
/// 2.5.29.17, subjectAltName.
const SUBJECT_ALT_NAME: [u8; 3] = [0x55, 0x1d, 0x11];

/// The URI names of the subjectAltName extension of one certificate.
fn uri_sans(certificate: &[u8]) -> Vec<String> {
    let mut uris = Vec::new();
    let Some((SEQUENCE, certificate, _)) = element(certificate) else {
        return uris;
    };
    let Some((SEQUENCE, tbs, _)) = element(certificate) else {
        return uris;
    };
    let Some((_, extensions)) = children(tbs)
        .into_iter()
        .find(|(tag, _)| *tag == EXTENSIONS)
    else {
        return uris;
    };
    let Some((SEQUENCE, extensions, _)) = element(extensions) else {
        return uris;
    };
    for (tag, extension) in children(extensions) {
        if tag != SEQUENCE {
            continue;
        }
        let parts = children(extension);
        if parts.first() != Some(&(OID, SUBJECT_ALT_NAME.as_slice())) {
            continue;
        }
        // The value is the last part (an optional `critical` BOOLEAN comes before it).
        let Some((OCTET_STRING, value)) = parts.last().copied() else {
            continue;
        };
        let Some((SEQUENCE, names, _)) = element(value) else {
            continue;
        };
        for (tag, name) in children(names) {
            if tag == URI_NAME
                && let Ok(uri) = std::str::from_utf8(name)
            {
                uris.push(uri.to_owned());
            }
        }
    }
    uris
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with `openssl req -x509 -addext subjectAltName=URI:spiffe://example.org`.
    const EXAMPLE_ORG: &str = include_str!("../tests/common/example.org.pem");
    const AGENTSWORK: &str = include_str!("../tests/common/agentswork.de.pem");
    /// `DNS:example.org`, `URI:https://example.org/x` and `URI:spiffe://multi.test/a`.
    const MIXED: &str = include_str!("../tests/common/mixed-san.pem");

    #[test]
    fn reads_the_trust_domain_of_the_uri_san() {
        assert_eq!(spiffe_trust_domains(EXAMPLE_ORG), ["example.org"]);
        assert_eq!(spiffe_trust_domains(AGENTSWORK), ["agentswork.de"]);
    }

    #[test]
    fn a_bundle_lists_every_certificate_and_ignores_other_names() {
        let bundle = format!("{EXAMPLE_ORG}\n{AGENTSWORK}{EXAMPLE_ORG}");
        assert_eq!(
            spiffe_trust_domains(&bundle),
            ["example.org", "agentswork.de"]
        );
        assert_eq!(spiffe_trust_domains(MIXED), ["multi.test"]);
    }

    #[test]
    fn garbage_gives_no_domain_and_no_panic() {
        assert!(spiffe_trust_domains("").is_empty());
        assert!(
            spiffe_trust_domains("-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----")
                .is_empty()
        );
        assert!(
            spiffe_trust_domains(
                "-----BEGIN CERTIFICATE-----\nMAMCAQE=\n-----END CERTIFICATE-----"
            )
            .is_empty()
        );
        let truncated = &AGENTSWORK[..AGENTSWORK.len() / 2];
        assert!(spiffe_trust_domains(truncated).is_empty());
    }
}
