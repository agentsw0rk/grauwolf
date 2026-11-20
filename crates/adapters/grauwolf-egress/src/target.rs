//! Parsing of the addresses that specs and variables name.

/// Scheme, host and port of an address; all lower case, no user info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
}

/// A name inside the cluster: `<service>.<namespace>.svc[...]`, `*.<namespace>.svc` or `*.svc`
/// (a label `svc` that is not the first).
pub fn is_cluster_host(host: &str) -> bool {
    host.split('.').skip(1).any(|label| label == "svc")
}

/// The port a scheme implies.
pub fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "postgres" | "postgresql" => Some(5432),
        _ => None,
    }
}

/// Parses `scheme://[user@]host[:port][/…]`; without a scheme `default_scheme` is used (an address
/// like `localhost:11434`). The port is the explicit one or the default of the scheme.
pub fn parse(text: &str, default_scheme: &str) -> Option<Target> {
    let text = text.trim();
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => (default_scheme.to_owned(), text),
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']')?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if host.is_empty() || host.contains(char::is_whitespace) || host.contains("${") {
        return None;
    }
    let port = match port {
        Some(port) if !port.is_empty() => Some(port.parse().ok()?),
        _ => default_port(&scheme),
    };
    Some(Target {
        scheme,
        host: host.to_ascii_lowercase(),
        port,
    })
}
