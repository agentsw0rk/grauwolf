//! The grant side of the identity service (ADR-0111 stage 3): people create, list and revoke
//! their grants; a trigger's run enters through a grant reference that the entry agent passes on.

use std::sync::Arc;

use grauwolf_spiffe::SpiffeId;
use serde::Deserialize;
use serde_json::json;

use crate::grant::{Grant, GrantOwner, GrantRequest, GrantStore, is_grant_id};
use crate::jose::now_secs;
use crate::refresh::RefreshRedeemer;
use crate::service::{IdentityService, TokenRequest, new_txn_id};
use crate::txn::TxnClaims;
use crate::{IdentityError, Person};

/// Where grants are kept and how their refresh tokens are redeemed.
#[derive(Clone)]
pub(crate) struct Grants {
    pub store: Arc<dyn GrantStore>,
    pub refresh: Arc<dyn RefreshRedeemer>,
}

/// `POST /api/identity/grants`: the grant and the refresh token of a separate sign-in of the
/// person with `offline_access`.
#[derive(Clone, Deserialize)]
pub struct CreateGrant {
    #[serde(flatten)]
    pub grant: GrantRequest,
    pub refresh_token: String,
}

impl std::fmt::Debug for CreateGrant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreateGrant")
            .field("grant", &self.grant)
            .finish_non_exhaustive()
    }
}

/// A refusal of the person's sign-in while creating a grant is the request's fault (`400`), not
/// a missing authorization.
fn refused_at_creation(error: IdentityError) -> IdentityError {
    match error {
        IdentityError::Unauthorized(reason) => IdentityError::BadRequest(reason),
        other => other,
    }
}

impl IdentityService {
    /// Grants are kept in `store`; their refresh tokens are redeemed with `refresh`.
    pub fn with_grants(
        mut self,
        store: Arc<dyn GrantStore>,
        refresh: Arc<dyn RefreshRedeemer>,
    ) -> Self {
        self.grants = Some(Grants { store, refresh });
        self
    }

    fn grants(&self) -> Result<&Grants, IdentityError> {
        self.grants.as_ref().ok_or_else(|| {
            IdentityError::Unavailable(
                "this identity service has no grants (GRAUWOLF_IDENTITY_GRANT_CLIENT_ID)".into(),
            )
        })
    }

    async fn person(&self, access_token: &str) -> Result<Person, IdentityError> {
        self.people.verify(access_token).await
    }

    /// Creates a grant for the person of `access_token`: the request must be valid and inside the
    /// policy's `grants` section, and the refresh token must belong to the same person (it is
    /// redeemed once, and the rotated token is stored).
    pub async fn create_grant(
        &self,
        access_token: &str,
        request: CreateGrant,
    ) -> Result<Grant, IdentityError> {
        let grants = self.grants()?;
        let person = self.person(access_token).await?;
        let grant = Grant::new(
            GrantOwner {
                oid: person.oid.clone(),
                name: person.name.clone(),
                upn: person.upn.clone(),
            },
            request.grant,
            now_secs(),
            self.settings.max_chain_lifetime,
        )?;
        self.policy
            .allows_grant(&grant.agent, &grant.trigger, &grant.purposes)
            .map_err(IdentityError::Forbidden)?;
        if request.refresh_token.is_empty() {
            return Err(IdentityError::BadRequest("refresh_token is empty".into()));
        }
        let redeemed = grants
            .refresh
            .redeem(&request.refresh_token)
            .await
            .map_err(refused_at_creation)?;
        let holder = self
            .person(&redeemed.access_token)
            .await
            .map_err(refused_at_creation)?;
        if holder.oid != person.oid {
            return Err(IdentityError::Forbidden(
                "the refresh token belongs to another person".into(),
            ));
        }
        grants
            .store
            .save_token(&grant.id, &redeemed.refresh_token)
            .await?;
        grants.store.save(&grant).await?;
        eprintln!(
            "grauwolf identity: grant {} created: oid={} agent={} trigger={}",
            grant.id, grant.owner.oid, grant.agent, grant.trigger
        );
        Ok(grant)
    }

    /// The grants of the person of `access_token`.
    pub async fn own_grants(&self, access_token: &str) -> Result<Vec<Grant>, IdentityError> {
        let grants = self.grants()?;
        let person = self.person(access_token).await?;
        Ok(grants
            .store
            .list()
            .await?
            .into_iter()
            .filter(|grant| grant.owner.oid == person.oid)
            .collect())
    }

    /// Revokes the person's grant `id`: metadata and refresh token are deleted. Another person's
    /// grant is `NotFound`, like one that does not exist.
    pub async fn revoke_grant(&self, access_token: &str, id: &str) -> Result<(), IdentityError> {
        let grants = self.grants()?;
        let person = self.person(access_token).await?;
        let not_found = || IdentityError::NotFound(format!("you have no grant {id}"));
        let grant = match is_grant_id(id) {
            true => grants.store.get(id).await?.ok_or_else(not_found)?,
            false => return Err(not_found()),
        };
        if grant.owner.oid != person.oid {
            return Err(not_found());
        }
        grants.store.delete(id).await?;
        eprintln!("grauwolf identity: grant {id} revoked: oid={}", person.oid);
        Ok(())
    }

    /// The entry of a grant run: the entry agent `caller` passes on the grant reference and the
    /// SPIFFE ID of the workload that called it (`trigger`). Checks, in this order: the grant
    /// exists (`401`), `caller` is its agent and `trigger` its trigger (`403`), the policy allows
    /// both with the grant's purposes (`403`), the grant is still valid (`401`), the person's
    /// refresh token is redeemed (refused `401`, unreachable `503`) and names the owner. The
    /// rotated token and `last_used` are stored; `chain_exp` is now + `max_run`.
    pub(crate) async fn grant_entry(
        &self,
        caller: &SpiffeId,
        request: &TokenRequest,
    ) -> Result<TxnClaims, IdentityError> {
        let grants = self.grants()?;
        let id = request.subject_token.as_str();
        let gone = || IdentityError::Unauthorized(format!("the grant {id} does not exist"));
        if !is_grant_id(id) {
            return Err(IdentityError::Unauthorized("not a grant ID".into()));
        }
        let mut grant = grants.store.get(id).await?.ok_or_else(gone)?;
        let trigger = request.trigger.as_deref().ok_or_else(|| {
            IdentityError::BadRequest("a grant reference needs the trigger".into())
        })?;
        if caller.as_str() != grant.agent {
            return Err(IdentityError::Forbidden(format!(
                "the grant {id} is not for {caller}"
            )));
        }
        if trigger != grant.trigger {
            return Err(IdentityError::Forbidden(format!(
                "{trigger} is not the trigger of the grant {id}"
            )));
        }
        self.policy
            .allows_grant(&grant.agent, &grant.trigger, &grant.purposes)
            .map_err(IdentityError::Forbidden)?;
        let now = now_secs();
        if !grant.is_valid_at(now) {
            return Err(IdentityError::Unauthorized(format!(
                "the grant {id} has expired"
            )));
        }
        let refresh_token = grants.store.token(id).await?.ok_or_else(gone)?;
        let redeemed =
            grants
                .refresh
                .redeem(&refresh_token)
                .await
                .map_err(|error| match error {
                    IdentityError::Unauthorized(reason) => IdentityError::Unauthorized(format!(
                        "the person's sign-in of the grant {id} is no longer valid: {reason}"
                    )),
                    other => other,
                })?;
        let person = self.person(&redeemed.access_token).await?;
        if person.oid != grant.owner.oid {
            return Err(IdentityError::Unauthorized(format!(
                "the sign-in of the grant {id} names another person"
            )));
        }
        grants.store.save_token(id, &redeemed.refresh_token).await?;
        grant.last_used = Some(now);
        grant.owner.name = person.name.clone().or(grant.owner.name.take());
        grant.owner.upn = person.upn.clone().or(grant.owner.upn.take());
        grants.store.save(&grant).await?;
        let chain_exp = now + grant.max_run;
        let claims = TxnClaims {
            iss: self.settings.issuer.clone(),
            aud: self.settings.trust_domain.clone(),
            sub: person.oid,
            name: grant.owner.name.clone(),
            upn: grant.owner.upn.clone(),
            txn: new_txn_id(),
            purp: (!grant.purposes.is_empty()).then(|| grant.purposes.join(" ")),
            azd: Some(json!({ "grant": grant.id })),
            req_wl: caller.to_string(),
            iat: now,
            exp: (now + self.settings.lifetime.as_secs()).min(chain_exp),
            chain_exp,
        };
        eprintln!(
            "grauwolf identity: grant {id} used: oid={} trigger={trigger} agent={caller} txn={}",
            claims.sub, claims.txn
        );
        Ok(claims)
    }
}
