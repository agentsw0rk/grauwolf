//! Delegation (ADR-0111): composes the identity service of `grauwolf identity serve` and the
//! agent's client of it from the configuration and the workload identity.

use std::sync::Arc;

pub use grauwolf_identity::{
    CallAnswer, CallerCredential, DeviceCode, DeviceEndpoints, DeviceLogin, Grant, GrantRequest,
    GrantsClient, IdentityClient, IdentityError, IdentityPolicy, IdentityService, OAuthTokens,
    RunDelegation, TxnClaims, TxnToken, call_agent, peek_claims,
};
use grauwolf_spiffe::{AuthMode, ClientAuth, Identity, SpiffeId, SpiffeMatch};

use crate::config::SpiffeServiceConfig;

/// The client of the identity service for an agent (`GRAUWOLF_IDENTITY_URL`), authenticated with
/// the agent's workload identity: over `https://` with mTLS (the server must be
/// `GRAUWOLF_IDENTITY_SPIFFE_ID`, else any ID of the own trust domain), over `http://` with a
/// JWT-SVID for the audience `GRAUWOLF_IDENTITY_SPIFFE_ID` (required there). `None` without a URL.
pub fn agent_client(
    settings: &SpiffeServiceConfig,
    identity: Arc<dyn Identity>,
) -> Result<Option<Arc<IdentityClient>>, String> {
    let Some(url) = &settings.identity_url else {
        return Ok(None);
    };
    let own = identity
        .id()
        .ok_or("the workload identity has no SPIFFE ID")?;
    let server = settings
        .identity_spiffe_id
        .as_deref()
        .map(SpiffeId::parse)
        .transpose()
        .map_err(|error| format!("GRAUWOLF_IDENTITY_SPIFFE_ID: {error}"))?;
    let auth = if url.starts_with("https://") {
        let expected = match server {
            Some(id) => SpiffeMatch::Id(id),
            None => SpiffeMatch::TrustDomain(own.trust_domain().to_owned()),
        };
        ClientAuth::new(identity, AuthMode::Mtls, Some(expected), None)
    } else {
        let audience = server.ok_or(
            "GRAUWOLF_IDENTITY_URL over http:// needs GRAUWOLF_IDENTITY_SPIFFE_ID (the audience \
             of the JWT-SVID)",
        )?;
        ClientAuth::new(identity, AuthMode::Jwt, None, Some(audience.to_string()))
    }
    .map_err(|error| format!("GRAUWOLF_IDENTITY_URL: {error}"))?;
    IdentityClient::new(url, own.trust_domain(), Some(Arc::new(auth)))
        .map(|client| Some(Arc::new(client)))
}

/// How `grauwolf call` authenticates at `url` (ADR-0111 stage 3): `token` if given; else the
/// workload identity of `env` (`SPIFFE_ENDPOINT_SOCKET`) – over `https://` with mTLS (the server
/// must be `expected`, else any ID of the own trust domain), over `http://` with a JWT-SVID for
/// the audience `expected` (required there); else no credential.
pub async fn caller_credential(
    env: &dyn crate::config::Env,
    url: &str,
    expected: Option<&str>,
    token: Option<&str>,
) -> Result<CallerCredential, String> {
    if let Some(token) = token {
        return Ok(CallerCredential::Bearer(token.to_owned()));
    }
    let expected = expected
        .map(SpiffeId::parse)
        .transpose()
        .map_err(|error| format!("--spiffe-id: {error}"))?;
    let Some(identity) = crate::spiffe::workload(env).await? else {
        return Ok(CallerCredential::None);
    };
    let auth = if url.starts_with("https://") {
        let expected = match expected {
            Some(id) => SpiffeMatch::Id(id),
            None => SpiffeMatch::TrustDomain(
                identity
                    .id()
                    .ok_or("the workload identity has no SPIFFE ID")?
                    .trust_domain()
                    .to_owned(),
            ),
        };
        ClientAuth::new(identity, AuthMode::Mtls, Some(expected), None)
    } else {
        let audience = expected.ok_or(
            "a call over http:// with the workload identity needs --spiffe-id (the audience of \
             the JWT-SVID)",
        )?;
        ClientAuth::new(identity, AuthMode::Jwt, None, Some(audience.to_string()))
    }
    .map_err(|error| error.to_string())?;
    Ok(CallerCredential::Workload(Arc::new(auth)))
}

/// The identity service of `grauwolf identity serve` from its settings (ADR-0111): Entra
/// verification, Transit signing with an OpenBao login by the service's JWT-SVID, the policy file
/// (watched; call inside a tokio runtime). `identity` is the service's own workload identity.
#[cfg(feature = "credentials")]
pub fn service(
    config: &crate::config::IdentityServiceConfig,
    identity: Arc<dyn Identity>,
) -> Result<IdentityService, String> {
    use crate::config::IdentityServiceConfig as Config;
    use grauwolf_identity::{
        DEFAULT_LIFETIME, EntraSettings, EntraVerifier, MAX_CHAIN_LIFETIME, OpenBaoGrantStore,
        PublicClientRefresh, ServiceSettings, TokenEndpoint, TransitSigner,
    };
    use grauwolf_openbao::{Bao, HttpTransport};

    let own = identity
        .id()
        .ok_or("the workload identity has no SPIFFE ID")?;
    let entra = EntraSettings {
        tenant: Config::required(&config.tenant, "GRAUWOLF_IDENTITY_TENANT")?.to_owned(),
        issuer: Config::required(&config.issuer, "GRAUWOLF_IDENTITY_ISSUER")?.to_owned(),
        audiences: config.required_audiences()?,
        scope: config.scope.clone(),
        jwks_url: config.jwks_url.clone(),
    };
    let settings = ServiceSettings {
        issuer: Config::required(&config.public_url, "GRAUWOLF_IDENTITY_PUBLIC_URL")?.to_owned(),
        trust_domain: config
            .trust_domain
            .clone()
            .unwrap_or_else(|| own.trust_domain().to_owned()),
        person_issuer: entra.issuer.clone(),
        lifetime: DEFAULT_LIFETIME,
        max_chain_lifetime: MAX_CHAIN_LIFETIME,
    };
    let policy_file = Config::required(&config.policy, "GRAUWOLF_IDENTITY_POLICY")?;
    let policy = IdentityPolicy::watch(
        std::path::Path::new(policy_file),
        grauwolf_identity::POLICY_RELOAD_INTERVAL,
    )?;
    let address = Config::required(&config.openbao_address, "GRAUWOLF_IDENTITY_OPENBAO_ADDR")?;
    let login = grauwolf_credentials::OpenBaoConfig {
        address: address.to_owned(),
        login: grauwolf_credentials::SpiffeLogin {
            audience: config.openbao_audience.clone(),
            role: Config::required(&config.openbao_role, "GRAUWOLF_IDENTITY_OPENBAO_ROLE")?
                .to_owned(),
            mount: config.openbao_mount.clone(),
        },
    };
    let tokens = Arc::new(grauwolf_credentials::OpenBaoClient::new(
        login,
        Arc::new(JwtSide(Arc::clone(&identity))),
    ));
    let signer = TransitSigner::new(
        Bao::new(HttpTransport::new(address)),
        Arc::clone(&tokens) as Arc<dyn grauwolf_openbao::TokenSource>,
        &config.transit_mount,
        Config::required(&config.transit_key, "GRAUWOLF_IDENTITY_TRANSIT_KEY")?,
    );
    let issuer = entra.issuer.clone();
    let service = IdentityService::new(
        settings,
        Arc::new(signer),
        Arc::new(EntraVerifier::with_settings(entra)?),
        policy,
    );
    // Grants (stage 3): with the public client whose refresh tokens they hold.
    let Some(client_id) = &config.grant_client_id else {
        return Ok(service);
    };
    let endpoint = match &config.grant_token_url {
        Some(url) => TokenEndpoint::Url(url.clone()),
        None => TokenEndpoint::Discovery(issuer),
    };
    let scope = config
        .grant_scope()
        .ok_or("grants need GRAUWOLF_IDENTITY_GRANT_SCOPE or GRAUWOLF_IDENTITY_AUDIENCE")?;
    let refresh = PublicClientRefresh::new(endpoint, client_id, &scope)?;
    let store = OpenBaoGrantStore::new(HttpTransport::new(address), tokens, &config.grant_kv_mount);
    Ok(service.with_grants(Arc::new(store), Arc::new(refresh)))
}

/// The JWT side of a full identity, for the OpenBao login.
#[cfg(feature = "credentials")]
struct JwtSide(Arc<dyn Identity>);

#[cfg(feature = "credentials")]
impl grauwolf_spiffe::WorkloadIdentity for JwtSide {
    fn jwt_svid<'a>(
        &'a self,
        audience: &'a str,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<grauwolf_spiffe::JwtSvid, grauwolf_spiffe::SpiffeError>,
    > {
        self.0.jwt_svid(audience)
    }

    fn validate_jwt_svid<'a>(
        &'a self,
        token: &'a str,
        audience: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<SpiffeId, grauwolf_spiffe::SpiffeError>> {
        self.0.validate_jwt_svid(token, audience)
    }
}
