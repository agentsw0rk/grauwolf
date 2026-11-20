//! The HTTP side of the identity service (ADR-0111, `grauwolf identity serve`):
//! `POST /api/identity/token`, `GET /api/identity/jwks.json`, `GET /api/identity/config`,
//! `POST|GET /api/identity/grants`, `DELETE /api/identity/grants/{id}` and `GET /health`. The
//! caller of `/token` is a workload, authenticated by its client certificate on the mTLS port or
//! by a JWT-SVID as bearer, exactly as agents authenticate their callers (ADR-0109); the caller of
//! `/grants` is a person with their Entra access token as bearer (stage 3); the keys and the
//! configuration are public.

use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use grauwolf_identity::{CreateGrant, IdentityError, IdentityService, TokenRequest};
use grauwolf_spiffe::SpiffeId;
use serde_json::json;

use crate::spiffe::{PeerSpiffe, SpiffeGate};

/// The identity service with the gate that validates JWT-SVIDs of callers on the plain port.
#[derive(Clone)]
struct Identity {
    service: Arc<IdentityService>,
    gate: Option<Arc<SpiffeGate>>,
}

/// The routes of the identity service. `gate` validates JWT-SVIDs (`None`: only callers with a
/// client certificate, i.e. the mTLS port, can request tokens).
pub fn identity_router(service: Arc<IdentityService>, gate: Option<Arc<SpiffeGate>>) -> Router {
    Router::new()
        .route("/api/identity/token", post(token))
        .route("/api/identity/jwks.json", get(jwks))
        .route("/api/identity/config", get(config))
        .route("/api/identity/grants", post(create_grant).get(list_grants))
        .route("/api/identity/grants/{id}", delete(revoke_grant))
        .route("/health", get(|| async { "ok" }))
        .with_state(Identity { service, gate })
}

fn refuse(error: &IdentityError) -> Response {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(json!({ "error": error.to_string() }))).into_response()
}

/// The workload that calls: the client certificate, else a JWT-SVID bearer.
async fn caller(
    identity: &Identity,
    peer: Option<ConnectInfo<PeerSpiffe>>,
    headers: &HeaderMap,
) -> Result<SpiffeId, IdentityError> {
    if let Some(ConnectInfo(PeerSpiffe(id))) = peer {
        return Ok(id);
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let (Some(gate), Some(bearer)) = (&identity.gate, bearer) else {
        return Err(IdentityError::Unauthorized(
            "the caller has no workload identity (mTLS or a JWT-SVID)".into(),
        ));
    };
    gate.validate(bearer).await.map_err(|error| {
        if error.is_invalid_token() {
            IdentityError::Unauthorized("the JWT-SVID is not valid for this service".into())
        } else {
            IdentityError::Unavailable("the workload identity service is not available".into())
        }
    })
}

/// Largest accepted body of a token request.
const MAX_BODY: usize = 64 * 1024;

async fn token(State(identity): State<Identity>, request: axum::extract::Request) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<PeerSpiffe>>()
        .cloned();
    let headers = request.headers().clone();
    let Ok(body) = axum::body::to_bytes(request.into_body(), MAX_BODY).await else {
        return refuse(&IdentityError::BadRequest("the body is too large".into()));
    };
    let caller = match caller(&identity, peer, &headers).await {
        Ok(caller) => caller,
        Err(error) => return refuse(&error),
    };
    // The body's error text could quote the token: it is not echoed.
    let request: TokenRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return refuse(&IdentityError::BadRequest(
                "the body must be JSON { subject_token, subject_token_type?, purpose?, trigger? }"
                    .into(),
            ));
        }
    };
    match identity.service.token(&caller, request).await {
        Ok(answer) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::to_value(answer).unwrap_or_default()),
        )
            .into_response(),
        Err(error) => {
            eprintln!("grauwolf identity: token for {caller} refused: {error}");
            refuse(&error)
        }
    }
}

/// The person's access token (`Authorization: Bearer`) of a `/grants` request.
fn person_token(headers: &HeaderMap) -> Result<&str, IdentityError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            IdentityError::Unauthorized(
                "sign in first: Authorization: Bearer <access token>".into(),
            )
        })
}

/// `POST /api/identity/grants` with `{ agent, trigger, purposes?, valid_until?, max_run?,
/// refresh_token }`: `201` with the grant (never the token).
async fn create_grant(
    State(identity): State<Identity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let token = match person_token(&headers) {
        Ok(token) => token,
        Err(error) => return refuse(&error),
    };
    // The body holds the refresh token: a parse error is not echoed.
    let request: CreateGrant = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return refuse(&IdentityError::BadRequest(
                "the body must be JSON { agent, trigger, purposes?, valid_until?, max_run?, \
                 refresh_token }"
                    .into(),
            ));
        }
    };
    match identity.service.create_grant(token, request).await {
        Ok(grant) => (
            StatusCode::CREATED,
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::to_value(grant).unwrap_or_default()),
        )
            .into_response(),
        Err(error) => {
            eprintln!("grauwolf identity: grant refused: {error}");
            refuse(&error)
        }
    }
}

/// `GET /api/identity/grants`: `{ "grants": [..] }`, the caller's own.
async fn list_grants(State(identity): State<Identity>, headers: HeaderMap) -> Response {
    let token = match person_token(&headers) {
        Ok(token) => token,
        Err(error) => return refuse(&error),
    };
    match identity.service.own_grants(token).await {
        Ok(grants) => Json(json!({ "grants": grants })).into_response(),
        Err(error) => refuse(&error),
    }
}

/// `DELETE /api/identity/grants/{id}`: `204`; `404` for an unknown grant or another person's.
async fn revoke_grant(
    State(identity): State<Identity>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let token = match person_token(&headers) {
        Ok(token) => token,
        Err(error) => return refuse(&error),
    };
    match identity.service.revoke_grant(token, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refuse(&error),
    }
}

async fn jwks(State(identity): State<Identity>) -> Response {
    match identity.service.jwks().await {
        Ok(jwks) => Json(serde_json::to_value(jwks).unwrap_or_default()).into_response(),
        Err(error) => refuse(&error),
    }
}

async fn config(State(identity): State<Identity>) -> Json<serde_json::Value> {
    Json(identity.service.config())
}

/// Serves the identity service until SIGTERM or SIGINT with its workload identity `identity`:
/// the routes on `port` (JWT-SVID callers for the audience of `settings` – default the own
/// SPIFFE ID –, public keys and configuration) and over mTLS on the SPIFFE port of `settings`
/// (callers with a client certificate of the own or an accepted trust domain).
#[cfg(feature = "service")]
pub async fn serve_identity(
    service: Arc<IdentityService>,
    identity: Arc<dyn grauwolf_spiffe::Identity>,
    settings: &grauwolf_host::config::SpiffeServiceConfig,
    bind: std::net::IpAddr,
    port: u16,
) -> Result<(), String> {
    use std::net::SocketAddr;
    let own = identity
        .id()
        .ok_or("the workload identity has no SPIFFE ID")?;
    let mut accepted = vec![own.trust_domain().to_owned()];
    accepted.extend(settings.trust_domains.iter().cloned());
    accepted.dedup();
    let inbound = crate::spiffe::SpiffeInbound::with_identity(
        settings.port,
        identity,
        crate::spiffe::CallerMap::empty(),
        settings.audience.clone(),
        &accepted,
        false,
    )?;
    let router = identity_router(service, Some(Arc::clone(&inbound.gate)));
    let main = tokio::net::TcpListener::bind(SocketAddr::new(bind, port))
        .await
        .map_err(|error| format!("port {port}: {error}"))?;
    let mtls = tokio::net::TcpListener::bind(SocketAddr::new(bind, inbound.port))
        .await
        .map_err(|error| format!("SPIFFE port {}: {error}", inbound.port))?;
    eprintln!(
        "grauwolf identity service on http://{} and mTLS port {}",
        main.local_addr().map_err(|error| error.to_string())?,
        inbound.port
    );
    let shutdown = crate::service::shutdown_signal();
    let plain_router = router.clone();
    let plain = async {
        axum::serve(main, plain_router)
            .with_graceful_shutdown(shutdown.clone())
            .await
            .map_err(|error| error.to_string())
    };
    let (plain, mtls) = tokio::join!(plain, inbound.serve(mtls, router, shutdown.clone()));
    plain.and(mtls)
}
