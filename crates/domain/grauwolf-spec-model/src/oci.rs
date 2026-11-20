//! References to artifacts in OCI registries (ADR-0107): `registry/repository[:tag][@sha256:…]`.

/// A parsed OCI reference. The registry is always explicit: grauwolf has no default registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciReference {
    /// Host with optional port, e.g. `ghcr.io` or `localhost:5000`.
    pub registry: String,
    /// Repository path, e.g. `acme/tools/math`.
    pub repository: String,
    /// Tag; without tag and digest the tag is `latest`.
    pub tag: Option<String>,
    /// Content digest `sha256:<64 hex digits>`; it wins over the tag.
    pub digest: Option<String>,
}

impl OciReference {
    /// Parses `text`; the error says what is wrong without repeating the text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (rest, digest) = match text.split_once('@') {
            Some((rest, digest)) => {
                if !is_sha256_digest(digest) {
                    return Err("the digest must be `sha256:` followed by 64 hex digits".into());
                }
                (rest, Some(digest.to_owned()))
            }
            None => (text, None),
        };
        let (registry, path) = rest
            .split_once('/')
            .ok_or("expected `registry/repository`, e.g. ghcr.io/acme/tools/math:1.0")?;
        if !is_registry_host(registry) {
            return Err(
                "the first part must be the registry host (with a dot, a port or `localhost`), \
                 e.g. ghcr.io"
                    .into(),
            );
        }
        let (repository, tag) = match path.rsplit_once(':') {
            Some((repository, tag)) => (repository, Some(tag)),
            None => (path, None),
        };
        if repository.is_empty() || !repository.split('/').all(is_repository_component) {
            return Err(
                "the repository must consist of lowercase path components (a-z, 0-9, `.`, `_`, `-`)"
                    .into(),
            );
        }
        if let Some(tag) = tag {
            if !is_tag(tag) {
                return Err(
                    "the tag must be 1 to 128 characters of A-Z, a-z, 0-9, `_`, `.`, `-`".into(),
                );
            }
        }
        Ok(Self {
            registry: registry.to_owned(),
            repository: repository.to_owned(),
            tag: tag.map(str::to_owned),
            digest,
        })
    }

    /// What goes after `/manifests/` in the registry API: the digest, else the tag, else `latest`.
    pub fn reference(&self) -> String {
        self.digest
            .clone()
            .or_else(|| self.tag.clone())
            .unwrap_or_else(|| "latest".to_owned())
    }
}

/// `sha256:` and 64 lowercase hex digits.
pub fn is_sha256_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    })
}

fn is_registry_host(text: &str) -> bool {
    let (host, port) = match text.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (text, None),
    };
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-']);
    let port_ok = port.is_none_or(|port| {
        !port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit())
    });
    host_ok && port_ok && (host.contains('.') || port.is_some() || host == "localhost")
}

fn is_repository_component(text: &str) -> bool {
    let alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    !text.is_empty()
        && text.starts_with(alnum)
        && text.ends_with(alnum)
        && text
            .chars()
            .all(|c| alnum(c) || matches!(c, '.' | '_' | '-'))
}

fn is_tag(text: &str) -> bool {
    text.len() <= 128
        && text
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric() || first == '_')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}
