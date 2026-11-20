//! SPIFFE IDs, trust domains and the match an outbound client applies to the server's ID.

use std::fmt;

/// Why a string is not a SPIFFE ID or a trust domain.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct IdError(pub String);

/// Checks a trust domain name: lowercase letters, digits, `.`, `-`, `_`.
pub fn validate_trust_domain(domain: &str) -> Result<(), IdError> {
    if domain.is_empty() {
        return Err(IdError("the trust domain is empty".into()));
    }
    if domain.len() > 255 {
        return Err(IdError(
            "the trust domain is longer than 255 characters".into(),
        ));
    }
    if let Some(bad) = domain
        .chars()
        .find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '.' | '-' | '_'))
    {
        return Err(IdError(format!(
            "the trust domain '{domain}' has the character '{bad}' (allowed: a-z 0-9 . - _)"
        )));
    }
    Ok(())
}

/// A SPIFFE ID, `spiffe://<trust domain>/<path>`, validated.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpiffeId(String);

impl SpiffeId {
    /// Parses and validates `text`.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        let rest = text.strip_prefix("spiffe://").ok_or_else(|| {
            IdError(format!(
                "'{text}' is not a SPIFFE ID (spiffe://<trust domain>/<path>)"
            ))
        })?;
        let (domain, path) = match rest.split_once('/') {
            Some((domain, path)) => (domain, Some(path)),
            None => (rest, None),
        };
        validate_trust_domain(domain)?;
        if let Some(path) = path {
            for segment in path.split('/') {
                if segment.is_empty() || segment == "." || segment == ".." {
                    return Err(IdError(format!(
                        "'{text}': the path has an empty, '.' or '..' segment"
                    )));
                }
                if let Some(bad) = segment
                    .chars()
                    .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
                {
                    return Err(IdError(format!(
                        "'{text}': the path has the character '{bad}'"
                    )));
                }
            }
        }
        Ok(Self(text.to_owned()))
    }

    /// The whole ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The trust domain.
    pub fn trust_domain(&self) -> &str {
        let rest = &self.0["spiffe://".len()..];
        rest.split('/').next().unwrap_or(rest)
    }
}

impl fmt::Display for SpiffeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SpiffeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpiffeId({})", self.0)
    }
}

/// Which SPIFFE IDs an outbound client accepts from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpiffeMatch {
    /// Exactly this ID.
    Id(SpiffeId),
    /// One of these IDs.
    Ids(Vec<SpiffeId>),
    /// Any ID of this trust domain.
    TrustDomain(String),
}

impl SpiffeMatch {
    /// Does `id` satisfy the match?
    pub fn accepts(&self, id: &SpiffeId) -> bool {
        match self {
            Self::Id(expected) => expected == id,
            Self::Ids(expected) => expected.contains(id),
            Self::TrustDomain(domain) => id.trust_domain() == domain,
        }
    }
}

impl fmt::Display for SpiffeMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => write!(f, "{id}"),
            Self::Ids(ids) => {
                let ids: Vec<&str> = ids.iter().map(SpiffeId::as_str).collect();
                write!(f, "one of {}", ids.join(", "))
            }
            Self::TrustDomain(domain) => write!(f, "any ID of {domain}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_validated() {
        let id = SpiffeId::parse("spiffe://agentswork.de/ns/a/sa/a").unwrap();
        assert_eq!(id.trust_domain(), "agentswork.de");
        for bad in [
            "https://x/y",
            "spiffe://",
            "spiffe://Up.case/a",
            "spiffe://x/a//b",
            "spiffe://x/a/",
            "spiffe://x/../b",
            "spiffe://x/a b",
            "spiffe://x/a?q",
        ] {
            assert!(SpiffeId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn matches_by_id_list_or_domain() {
        let a = SpiffeId::parse("spiffe://d.de/ns/a").unwrap();
        let b = SpiffeId::parse("spiffe://d.de/ns/b").unwrap();
        let other = SpiffeId::parse("spiffe://e.de/ns/a").unwrap();
        assert!(SpiffeMatch::Id(a.clone()).accepts(&a));
        assert!(!SpiffeMatch::Id(a.clone()).accepts(&b));
        assert!(SpiffeMatch::Ids(vec![a.clone(), b.clone()]).accepts(&b));
        assert!(SpiffeMatch::TrustDomain("d.de".into()).accepts(&b));
        assert!(!SpiffeMatch::TrustDomain("d.de".into()).accepts(&other));
    }
}
