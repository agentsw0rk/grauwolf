//! SSH signatures of git commits and tags (ADR-0107 stage 4): reading the signature out of the raw
//! object, the allowed-signers file of `gpg.ssh.allowedSignersFile` and the check against it.
//! Namespace `git`; GPG and X.509 signatures are not supported.

use git2::{ObjectType, Oid};
use ssh_key::public::KeyData;
use ssh_key::{HashAlg, PublicKey, SshSig};

/// The namespace git signs with.
pub const GIT_NAMESPACE: &str = "git";
const SSH_BEGIN: &str = "-----BEGIN SSH SIGNATURE-----";

/// Why an object does not pass the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    /// Neither the commit nor (for a tag) the tag carries a signature.
    Unsigned,
    /// There is a signature, but it is no valid SSH signature of an allowed signer.
    Invalid(String),
}

impl std::fmt::Display for SignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsigned => f.write_str("unsigned"),
            Self::Invalid(reason) => write!(f, "signature invalid: {reason}"),
        }
    }
}

impl std::error::Error for SignatureError {}

/// One line of an allowed-signers file: principal patterns, optional namespaces and a key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    principals: Vec<String>,
    namespaces: Option<Vec<String>>,
    key: PublicKey,
}

/// The keys that may sign, in the format of git's `gpg.ssh.allowedSignersFile`
/// (`principals [options] keytype base64-key [comment]`). Supported option: `namespaces="…"`;
/// every other option is an error, as are negated principals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedSigners {
    entries: Vec<Entry>,
}

impl AllowedSigners {
    /// Parses the text of an allowed-signers file; blank lines and `#` comments are skipped.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut entries = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let entry =
                parse_line(line).map_err(|reason| format!("line {}: {reason}", index + 1))?;
            entries.push(entry);
        }
        Ok(Self { entries })
    }

    /// No signer at all?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The fingerprints (`SHA256:…`) of the listed keys.
    pub fn fingerprints(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.key.fingerprint(HashAlg::Sha256).to_string())
            .collect()
    }

    fn allows(&self, key: &KeyData, identity: Option<&str>) -> bool {
        self.entries.iter().any(|entry| {
            entry.key.key_data() == key
                && entry
                    .namespaces
                    .as_ref()
                    .is_none_or(|namespaces| namespaces.iter().any(|n| n == GIT_NAMESPACE))
                && entry
                    .principals
                    .iter()
                    .any(|pattern| identity.map_or(pattern == "*", |who| glob(pattern, who)))
        })
    }
}

fn parse_line(line: &str) -> Result<Entry, String> {
    let (principals, rest) = line
        .split_once(char::is_whitespace)
        .ok_or("expected `principals keytype key`")?;
    if principals.starts_with('!') || principals.contains(",!") {
        return Err("negated principals are not supported".into());
    }
    let principals: Vec<String> = principals
        .trim_matches('"')
        .split(',')
        .map(str::to_owned)
        .collect();
    let mut rest = rest.trim_start();
    let mut namespaces = None;
    while !is_key_type(rest.split_whitespace().next().unwrap_or("")) {
        let (options, after) = take_options(rest)?;
        for option in options {
            match option.split_once('=') {
                Some(("namespaces", value)) => {
                    namespaces = Some(
                        value
                            .trim_matches('"')
                            .split(',')
                            .map(str::to_owned)
                            .collect(),
                    );
                }
                _ => {
                    let name = option.split('=').next().unwrap_or(&option);
                    return Err(format!(
                        "unsupported option `{name}` (only `namespaces` is supported)"
                    ));
                }
            }
        }
        rest = after;
    }
    let mut parts = rest.split_whitespace();
    let (Some(key_type), Some(key)) = (parts.next(), parts.next()) else {
        return Err("expected `principals keytype key`".into());
    };
    let key = PublicKey::from_openssh(&format!("{key_type} {key}"))
        .map_err(|error| format!("invalid {key_type} key: {error}"))?;
    Ok(Entry {
        principals,
        namespaces,
        key,
    })
}

fn is_key_type(token: &str) -> bool {
    token.starts_with("ssh-") || token.starts_with("ecdsa-sha2-") || token.starts_with("sk-")
}

/// The comma-separated options at the start of `text` (quotes may hold commas and no spaces
/// outside them) and the rest of the line.
fn take_options(text: &str) -> Result<(Vec<String>, &str), String> {
    let mut quoted = false;
    let mut end = text.len();
    for (index, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                end = index;
                break;
            }
            _ => {}
        }
    }
    if quoted {
        return Err("unterminated quote in the options".into());
    }
    let mut options = Vec::new();
    let mut current = String::new();
    let mut inside = false;
    for c in text[..end].chars() {
        match c {
            '"' => {
                inside = !inside;
                current.push(c);
            }
            ',' if !inside => options.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    options.push(current);
    Ok((options, text[end..].trim_start()))
}

/// Shell-style match with `*` and `?`.
fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text): (Vec<char>, Vec<char>) =
        (pattern.chars().collect(), text.chars().collect());
    let (mut p, mut t, mut star, mut mark) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// A signature found in a raw git object: what was signed, the armored signature and the email
/// of the committer or tagger (the identity that is matched against the principals).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    pub payload: Vec<u8>,
    pub signature: String,
    pub identity: Option<String>,
}

/// The signature of the raw commit object `raw` (header `gpgsig`), if it has one. The signed
/// data is the object without that header.
pub fn commit_signature(raw: &[u8]) -> Option<Signed> {
    let split = raw.windows(2).position(|w| w == b"\n\n").map(|i| i + 1)?;
    let (head, body) = raw.split_at(split);
    let mut payload = Vec::with_capacity(raw.len());
    let mut signature: Option<Vec<String>> = None;
    let mut in_signature = false;
    for line in head.split_inclusive(|byte| *byte == b'\n') {
        let text = String::from_utf8_lossy(line);
        let text = text.trim_end_matches('\n');
        if let Some(first) = text.strip_prefix("gpgsig ") {
            signature = Some(vec![first.to_owned()]);
            in_signature = true;
        } else if in_signature && text.starts_with(' ') {
            if let Some(lines) = signature.as_mut() {
                lines.push(text[1..].to_owned());
            }
        } else {
            in_signature = false;
            payload.extend_from_slice(line);
        }
    }
    payload.extend_from_slice(body);
    let signature = signature?.join("\n");
    Some(Signed {
        payload,
        signature,
        identity: identity_of(raw, "committer "),
    })
}

/// The signature appended to the message of the raw tag object `raw`, if there is one (SSH or
/// other); the signed data is the object before it.
pub fn tag_signature(raw: &[u8]) -> Option<Signed> {
    let text = String::from_utf8_lossy(raw);
    let start = [
        "-----BEGIN SSH SIGNATURE-----",
        "-----BEGIN PGP SIGNATURE-----",
        "-----BEGIN SIGNED MESSAGE-----",
    ]
    .iter()
    .filter_map(|marker| text.find(marker))
    .min()?;
    // The marker starts a line; String::from_utf8_lossy keeps the offsets for valid UTF-8.
    let (payload, signature) = raw.split_at(start);
    Some(Signed {
        payload: payload.to_vec(),
        signature: String::from_utf8_lossy(signature).trim_end().to_owned(),
        identity: identity_of(raw, "tagger "),
    })
}

/// The email in `<…>` of the header line starting with `prefix`.
fn identity_of(raw: &[u8], prefix: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let line = text
        .lines()
        .take_while(|line| !line.is_empty())
        .find(|line| line.starts_with(prefix))?;
    let start = line.find('<')? + 1;
    let end = line[start..].find('>')? + start;
    Some(line[start..end].to_owned())
}

/// The raw commit object (and, for a tag, the raw tag object) that were fetched; enough to check
/// signatures again later without the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedObjects {
    pub commit: Vec<u8>,
    pub tag: Option<Vec<u8>>,
}

/// Who signed, and what carried the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    /// `SHA256:…` fingerprint of the signing key.
    pub fingerprint: String,
    /// The e-mail of the committer or tagger the signer was matched against, if the object has one.
    pub identity: Option<String>,
    /// `commit` or `tag`.
    pub signed: &'static str,
}

/// Checks that `objects` really are the commit `commit_id` (and the tag `tag_name` pointing to
/// it) and that the commit, or for a tag first the tag, carries a valid SSH signature of an
/// allowed signer.
pub fn verify_objects(
    objects: &SignedObjects,
    commit_id: &str,
    tag_name: Option<&str>,
    signers: &AllowedSigners,
) -> Result<Verified, SignatureError> {
    let invalid = |reason: String| SignatureError::Invalid(reason);
    match Oid::hash_object(ObjectType::Commit, &objects.commit) {
        Ok(id) if id.to_string() == commit_id => {}
        _ => {
            return Err(invalid(
                "the stored commit object does not match the commit".into(),
            ));
        }
    }
    let mut failure = None;
    if let (Some(name), Some(raw)) = (tag_name, &objects.tag) {
        let head = String::from_utf8_lossy(raw);
        let has = |line: String| {
            head.lines()
                .take_while(|l| !l.is_empty())
                .any(|l| l == line)
        };
        if !has(format!("object {commit_id}")) || !has(format!("tag {name}")) {
            return Err(invalid(format!(
                "the stored tag object is not the tag `{name}` of the commit"
            )));
        }
        if let Some(signed) = tag_signature(raw) {
            match verify_signed(&signed, signers) {
                Ok(fingerprint) => {
                    return Ok(Verified {
                        fingerprint,
                        identity: signed.identity.clone(),
                        signed: "tag",
                    });
                }
                Err(error) => failure = Some(error),
            }
        }
    }
    match commit_signature(&objects.commit) {
        Some(signed) => verify_signed(&signed, signers)
            .map(|fingerprint| Verified {
                fingerprint,
                identity: signed.identity.clone(),
                signed: "commit",
            })
            .map_err(|error| failure.take().unwrap_or(error)),
        None => Err(failure.unwrap_or(SignatureError::Unsigned)),
    }
}

fn verify_signed(signed: &Signed, signers: &AllowedSigners) -> Result<String, SignatureError> {
    let invalid = |reason: String| SignatureError::Invalid(reason);
    if !signed.signature.starts_with(SSH_BEGIN) {
        return Err(invalid(
            "not an SSH signature (only SSH signatures are supported, not GPG or X.509)".into(),
        ));
    }
    let signature = SshSig::from_pem(signed.signature.as_bytes())
        .map_err(|error| invalid(format!("unreadable SSH signature: {error}")))?;
    if signature.namespace() != GIT_NAMESPACE {
        return Err(invalid(format!(
            "the signature is for the namespace `{}`, not `git`",
            signature.namespace()
        )));
    }
    let key = PublicKey::from(signature.public_key().clone());
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    key.verify(GIT_NAMESPACE, &signed.payload, &signature)
        .map_err(|_| {
            invalid(format!(
                "the signature of {fingerprint} does not match the object"
            ))
        })?;
    if !signers.allows(signature.public_key(), signed.identity.as_deref()) {
        return Err(invalid(format!(
            "signed by {fingerprint}, which is not an allowed signer for {}",
            signed.identity.as_deref().unwrap_or("an unknown identity")
        )));
    }
    Ok(fingerprint)
}
