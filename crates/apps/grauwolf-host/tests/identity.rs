//! The agent's client of the identity service from the settings (ADR-0111).

use std::sync::Arc;

use grauwolf_host::config::{MapEnv, SpiffeServiceConfig};
use grauwolf_host::identity::agent_client;
use grauwolf_spiffe::testing::FakeIdentity;

fn settings(pairs: &[(&str, &str)]) -> SpiffeServiceConfig {
    SpiffeServiceConfig::from_env(&MapEnv::new(pairs.iter().copied())).unwrap()
}

fn identity() -> Arc<FakeIdentity> {
    Arc::new(FakeIdentity::new(
        "spiffe://agents.local/ns/agents/sa/reviewer",
    ))
}

#[test]
fn there_is_no_client_without_an_identity_url() {
    assert!(agent_client(&settings(&[]), identity()).unwrap().is_none());
}

#[test]
fn https_uses_mtls_and_http_needs_the_spiffe_id_of_the_service() {
    let https = settings(&[(
        "GRAUWOLF_IDENTITY_URL",
        "https://identity.identity.svc:8443",
    )]);
    assert!(agent_client(&https, identity()).unwrap().is_some());
    let http = settings(&[("GRAUWOLF_IDENTITY_URL", "http://identity.identity.svc:8080")]);
    let error = agent_client(&http, identity()).unwrap_err();
    assert!(error.contains("GRAUWOLF_IDENTITY_SPIFFE_ID"), "{error}");
    let named = settings(&[
        ("GRAUWOLF_IDENTITY_URL", "http://identity.identity.svc:8080"),
        (
            "GRAUWOLF_IDENTITY_SPIFFE_ID",
            "spiffe://agents.local/ns/identity/sa/identity",
        ),
    ]);
    assert!(agent_client(&named, identity()).unwrap().is_some());
}
