//! Access to the service: shared token (ADR-0053), user tokens (ADR-0068), callers with a SPIFFE
//! identity (ADR-0109) and delegation by Txn-Tokens (ADR-0111).

use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use grauwolf_agents::events::OnBehalfOf;
use grauwolf_identity::{GRANT_HEADER, IdentityClient, IdentityError, RunDelegation, TxnToken};
use grauwolf_spiffe::SpiffeId;
use grauwolf_tools::{Delegation, TXN_TOKEN_HEADER};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::spiffe::{CallerSpiffe, PeerSpiffe, SpiffeGate};

/// Who makes a request; set in the request extensions after the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Caller {
    /// Trusted backend with the shared token; may name any `user_id`.
    Service,
    /// End user with their own token; their runs belong to them.
    User(String),
}

/// The verified delegation of a request (ADR-0111): the Txn-Token of the person the request acts
/// for, from the entry or from the calling agent. Set in the request extensions together with
/// `Caller::User(<the person's oid>)`.
#[derive(Clone)]
pub(crate) struct Delegated {
    token: TxnToken,
    client: Arc<IdentityClient>,
}

impl Delegated {
    /// The person, for `agent_starting` and the spans; never the token.
    pub(crate) fn on_behalf_of(&self) -> OnBehalfOf {
        let claims = &self.token.claims;
        OnBehalfOf {
            oid: claims.sub.clone(),
            name: claims.name.clone(),
            upn: claims.upn.clone(),
            txn: claims.txn.clone(),
            grant: claims
                .azd
                .as_ref()
                .and_then(|azd| azd["grant"].as_str())
                .map(str::to_owned),
        }
    }

    /// The Txn-Token for the run's outgoing calls, renewed at the identity service when needed.
    pub(crate) fn delegation(&self) -> Arc<dyn Delegation> {
        Arc::new(RunDelegation::new(
            Arc::clone(&self.client),
            self.token.clone(),
        ))
    }
}

/// Users and the SHA-256 hashes of their tokens (ADR-0068).
#[derive(Debug, Clone, Default)]
pub struct Users {
    hashes: Vec<(String, String)>,
}

impl Users {
    /// Reads `{ "<user>": "sha256:<hex>" }`.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let entries: serde_json::Map<String, Value> =
            serde_json::from_str(text).map_err(|error| format!("users file: {error}"))?;
        let mut hashes = Vec::with_capacity(entries.len());
        for (user, entry) in entries {
            grauwolf_agents::memory::check_user_id(&user)
                .map_err(|error| format!("users file: {error}"))?;
            let hash = entry
                .as_str()
                .and_then(|entry| entry.strip_prefix("sha256:"))
                .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| format!("users file: entry for '{user}' is not \"sha256:<hex>\""))?;
            hashes.push((hash.to_ascii_lowercase(), user));
        }
        Ok(Self { hashes })
    }

    /// User for `token`; compares all hashes in constant time.
    fn user(&self, token: &str) -> Option<&str> {
        let presented = token_hash(token);
        self.hashes
            .iter()
            .filter(|(hash, _)| equal(hash.as_bytes(), presented.as_bytes()))
            .map(|(_, user)| user.as_str())
            .next_back()
    }
}

/// New user token: `gw_` and 128 bits of randomness as hex text (ADR-0068).
pub fn new_user_token() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("system random source not available");
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("gw_{hex}")
}

/// SHA-256 of `token` as hex text, as it appears in the users file.
pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Shared token and user tokens.
struct Auth {
    service_token: Option<String>,
    users: Users,
    /// Callers with a SPIFFE identity (ADR-0109).
    spiffe: Option<Arc<SpiffeGate>>,
}

/// Requires `Authorization: Bearer <token>` for `router`: the shared token or that of a
/// user from `users`, otherwise `401`; `/health` and `/.well-known/…` stay open
/// (ADR-0053, ADR-0054, ADR-0068).
pub fn with_auth(router: Router, service_token: Option<String>, users: Users) -> Router {
    with_spiffe_auth(router, service_token, users, None)
}

/// Like [`with_auth`], plus callers with a SPIFFE identity (ADR-0109). Who is calling, in this
/// order:
/// 0. with an identity service at the gate (ADR-0111): `Authorization: Bearer` with a person's
///    access token (its `iss` is the `person_issuer` of the service), exchanged there for a
///    Txn-Token – the person is the request's user; refused `401`, service down `503`;
/// 1. the client certificate of the mTLS port (`ConnectInfo<PeerSpiffe>`), already verified by
///    the handshake;
/// 2. `Authorization: Bearer` with a JWT-SVID (three dot-separated parts, not `gw_…`, not the
///    shared token), validated by the Workload API for the audience;
/// 3. the token rules above.
///
/// An authenticated SPIFFE ID without an entry in the callers file gets `403`; a mapped one is
/// the request's user exactly like the user of a user token. Without a token, users and a
/// presented credential, the plain port stays open as before; the mTLS port always has an ID.
///
/// A `Txn-Token` header (ADR-0111) is accepted only from a mapped SPIFFE caller (1, 2); it is
/// verified with the identity service's keys (`aud`, `exp`; invalid `401`, keys unreachable
/// `503`) and the person in it becomes the request's user. From any other caller it gets `403`;
/// an agent without an identity service answers `401`.
///
/// A `Grauwolf-Grant` header (stage 3) is likewise accepted only from a mapped SPIFFE caller: the
/// grant ID and the caller's ID (the trigger) are exchanged at the identity service with this
/// agent's identity, and the grant's owner becomes the request's user. The service's `403`
/// (wrong trigger or agent, outside its policy) and `401` (unknown, expired or revoked grant,
/// the person's sign-in refused) are passed on, an unreachable service is `503`. Together with a
/// `Txn-Token` it is `400`.
pub fn with_spiffe_auth(
    router: Router,
    service_token: Option<String>,
    users: Users,
    spiffe: Option<Arc<SpiffeGate>>,
) -> Router {
    router.layer(axum::middleware::from_fn_with_state(
        Arc::new(Auth {
            service_token,
            users,
            spiffe,
        }),
        check,
    ))
}

/// Looks like a JWT: three non-empty parts separated by dots.
fn looks_like_jwt(token: &str) -> bool {
    !token.starts_with("gw_") && {
        let parts: Vec<&str> = token.split('.').collect();
        parts.len() == 3 && parts.iter().all(|part| !part.is_empty())
    }
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// The answer to a refused or failed delegation: `503` when the identity service or its keys
/// cannot be reached, `403` when the identity service's policy refuses the agent, else `401`. The reason never contains a token.
fn refuse_delegation(error: &IdentityError) -> Response {
    match error {
        IdentityError::Unavailable(_) => refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "the identity service is not available",
        ),
        IdentityError::Forbidden(_) => refuse(
            StatusCode::FORBIDDEN,
            &format!("delegation refused: {error}"),
        ),
        other => refuse(
            StatusCode::UNAUTHORIZED,
            &format!("delegation refused: {other}"),
        ),
    }
}

/// Admits an authenticated SPIFFE caller: its user from the callers file (else `403`) and, with a
/// `Txn-Token`, the person of the verified token as the request's user (ADR-0111); with a
/// `Grauwolf-Grant`, the grant's owner from the identity service (stage 3).
async fn admit_workload(
    gate: &SpiffeGate,
    id: SpiffeId,
    txn_token: Option<&str>,
    grant: Option<&str>,
    request: &mut Request,
) -> Result<(), Box<Response>> {
    let Some(user) = gate.user(&id) else {
        return Err(Box::new(refuse(
            StatusCode::FORBIDDEN,
            &format!("{id} is authenticated but not allowed to call this agent"),
        )));
    };
    if let Some(grant) = grant {
        let client = gate.delegation().ok_or_else(|| Box::new(no_delegation()))?;
        let token = client
            .grant(grant, &id)
            .await
            .map_err(|error| Box::new(refuse_delegation(&error)))?;
        request.extensions_mut().insert(CallerSpiffe(id));
        request
            .extensions_mut()
            .insert(Caller::User(token.claims.sub.clone()));
        request.extensions_mut().insert(Delegated {
            token,
            client: Arc::clone(client),
        });
        return Ok(());
    }
    request.extensions_mut().insert(CallerSpiffe(id));
    let Some(txn_token) = txn_token else {
        request.extensions_mut().insert(Caller::User(user));
        return Ok(());
    };
    let client = gate.delegation().ok_or_else(|| Box::new(no_delegation()))?;
    let claims = client
        .verify(txn_token)
        .await
        .map_err(|error| Box::new(refuse_delegation(&error)))?;
    request
        .extensions_mut()
        .insert(Caller::User(claims.sub.clone()));
    request.extensions_mut().insert(Delegated {
        token: TxnToken::new(txn_token.to_owned(), claims),
        client: Arc::clone(client),
    });
    Ok(())
}

/// A `Txn-Token` or a grant sent to an agent that has no identity service.
fn no_delegation() -> Response {
    refuse(
        StatusCode::UNAUTHORIZED,
        "this agent accepts no Txn-Token and no grant (it has no GRAUWOLF_IDENTITY_URL)",
    )
}

async fn check(State(auth): State<Arc<Auth>>, mut request: Request, next: Next) -> Response {
    // Health check and agent card (A2A, ADR-0054) stay public.
    let path = request.uri().path();
    // The signature of the chat API contains no secrets and its poller sends no token (ADR-0094).
    if path == "/health" || path == "/api/signature" || path.starts_with("/.well-known/") {
        return next.run(request).await;
    }
    // A header that is not text is kept as an empty token, which no verification accepts.
    let txn_token = request
        .headers()
        .get(TXN_TOKEN_HEADER)
        .map(|value| value.to_str().unwrap_or_default().to_owned());
    let grant = request
        .headers()
        .get(GRANT_HEADER)
        .map(|value| value.to_str().unwrap_or_default().to_owned());
    let delegation = auth.spiffe.as_ref().and_then(|gate| gate.delegation());
    if (txn_token.is_some() || grant.is_some()) && delegation.is_none() {
        return no_delegation();
    }
    if txn_token.is_some() && grant.is_some() {
        return refuse(
            StatusCode::BAD_REQUEST,
            "a request carries either a Txn-Token or a grant, not both",
        );
    }
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let presented = presented.as_deref();
    let shared = |token: &str| {
        auth.service_token
            .as_ref()
            .is_some_and(|shared| equal(token.as_bytes(), shared.as_bytes()))
    };
    // 0. The entry (ADR-0111): a person's access token is exchanged for a Txn-Token at the
    //    identity service, with this agent's workload identity.
    if let (Some(client), Some(token)) = (delegation, presented)
        && looks_like_jwt(token)
        && !shared(token)
    {
        match client.is_person_token(token).await {
            Err(error) => return refuse_delegation(&error),
            Ok(false) => {}
            Ok(true) if txn_token.is_some() || grant.is_some() => return foreign_txn_token(),
            Ok(true) => {
                let token = match client.exchange(token).await {
                    Ok(token) => token,
                    Err(error) => return refuse_delegation(&error),
                };
                request
                    .extensions_mut()
                    .insert(Caller::User(token.claims.sub.clone()));
                request.extensions_mut().insert(Delegated {
                    token,
                    client: Arc::clone(client),
                });
                return next.run(request).await;
            }
        }
    }
    if let Some(gate) = &auth.spiffe {
        // 1. the client certificate of the mTLS port.
        let peer = request
            .extensions()
            .get::<ConnectInfo<PeerSpiffe>>()
            .map(|info| info.0.0.clone());
        if let Some(id) = peer {
            return match admit_workload(
                gate,
                id,
                txn_token.as_deref(),
                grant.as_deref(),
                &mut request,
            )
            .await
            {
                Ok(()) => next.run(request).await,
                Err(refusal) => *refusal,
            };
        }
    }
    // 2. a JWT-SVID in the bearer header.
    if let (Some(gate), Some(token)) = (&auth.spiffe, presented)
        && looks_like_jwt(token)
        && !shared(token)
    {
        return match gate.validate(token).await {
            Ok(id) => match admit_workload(
                gate,
                id,
                txn_token.as_deref(),
                grant.as_deref(),
                &mut request,
            )
            .await
            {
                Ok(()) => next.run(request).await,
                Err(refusal) => *refusal,
            },
            Err(error) if error.is_invalid_token() => refuse(
                StatusCode::UNAUTHORIZED,
                "the JWT-SVID is not valid for this agent",
            ),
            Err(_) => refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "the workload identity service is not available",
            ),
        };
    }
    // A Txn-Token or a grant travels only between workloads with a SPIFFE identity.
    if txn_token.is_some() || grant.is_some() {
        return foreign_txn_token();
    }
    // 3. the token rules; without any configured token or user the port is open as before.
    if auth.spiffe.as_ref().is_some_and(|gate| gate.anonymous)
        && auth.service_token.is_none()
        && auth.users.hashes.is_empty()
        && presented.is_none()
    {
        return next.run(request).await;
    }
    let caller = presented.and_then(|presented| {
        if shared(presented) {
            Some(Caller::Service)
        } else {
            auth.users
                .user(presented)
                .map(|user| Caller::User(user.to_owned()))
        }
    });
    match caller {
        Some(caller) => {
            request.extensions_mut().insert(caller);
            next.run(request).await
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "token missing or wrong" })),
        )
            .into_response(),
    }
}

/// A `Txn-Token` or a grant from a caller without a SPIFFE identity (ADR-0111).
fn foreign_txn_token() -> Response {
    refuse(
        StatusCode::FORBIDDEN,
        "a Txn-Token or a grant is accepted only from a caller with a SPIFFE identity",
    )
}

/// Comparison in constant time (for equal length), so that the running time does not reveal
/// the token.
fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}
