//! Resolving `from_env` with the values a deployment knows.

use crate::Destination;
use crate::collect::Collector;
use crate::providers::bedrock_url;
use crate::target::{self, Target};

/// `destination` with the address `lookup` knows for its `from_env` variable; unchanged if the
/// variable has no value (a default address stays, an unknown one stays unknown). `from_env` is
/// kept, so the entry still says where the address came from.
pub fn resolve(destination: &Destination, lookup: &dyn Fn(&str) -> Option<String>) -> Destination {
    let Some(variable) = destination.from_env.as_deref() else {
        return destination.clone();
    };
    let value = if variable == "AWS_REGION" {
        // `bedrock`: BEDROCK_BASE_URL replaces the whole address, else the region names the host.
        lookup("BEDROCK_BASE_URL").or_else(|| {
            lookup("AWS_REGION")
                .or_else(|| lookup("AWS_DEFAULT_REGION"))
                .map(|region| bedrock_url(&region))
        })
    } else {
        lookup(variable)
    };
    let default_scheme = destination.scheme.as_deref().unwrap_or("https");
    let Some(Target { scheme, host, port }) = value
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| {
            target::parse(
                value,
                if variable == "OLLAMA_HOST" {
                    "http"
                } else {
                    default_scheme
                },
            )
        })
    else {
        return destination.clone();
    };
    Destination {
        scheme: Some(scheme),
        host: Some(host),
        port,
        ..destination.clone()
    }
}

/// [`resolve`] for all destinations; entries that become equal are merged.
pub fn resolve_all(
    destinations: &[Destination],
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Vec<Destination> {
    let mut collector = Collector::default();
    collector.extend(
        destinations
            .iter()
            .map(|destination| resolve(destination, lookup)),
    );
    collector.finish()
}
