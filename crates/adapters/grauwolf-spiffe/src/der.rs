//! A minimal DER reader: just enough to split certificate chains and read the URI subject
//! alternative names of a certificate. No certificate is validated here (that is webpki's job).

use crate::id::SpiffeId;

/// One DER element: `(tag, content, rest after it)`.
pub(crate) fn element(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
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
const SUBJECT_ALT_NAME: [u8; 3] = [0x55, 0x1d, 0x11];

/// Splits concatenated DER certificates (the Workload API's format) into single certificates.
pub(crate) fn split_certificates(mut data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut certificates = Vec::new();
    while !data.is_empty() {
        let (SEQUENCE, content, rest) = element(data)? else {
            return None;
        };
        let used = data.len() - rest.len();
        let _ = content;
        certificates.push(data[..used].to_vec());
        data = rest;
    }
    Some(certificates)
}

/// The URI names of the subjectAltName extension of one certificate.
pub(crate) fn uri_sans(certificate: &[u8]) -> Vec<String> {
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

/// The SPIFFE ID of a certificate: exactly one URI SAN, with the scheme `spiffe`.
pub fn spiffe_id_of_certificate(certificate: &[u8]) -> Result<SpiffeId, String> {
    let uris = uri_sans(certificate);
    match uris.as_slice() {
        [] => Err("the certificate has no URI subject alternative name".into()),
        [uri] => SpiffeId::parse(uri).map_err(|error| error.to_string()),
        _ => Err("the certificate has more than one URI subject alternative name".into()),
    }
}
