//! The decisions of the identity service (ADR-0111), without HTTP: the entry (a person's access
//! token or, for a grant run, a grant reference for a Txn-Token), renewal (a Txn-Token for a fresh one of the same chain), the public
//! keys and the configuration agents read.

use std::sync::Arc;
use std::time::Duration;

use grauwolf_spiffe::SpiffeId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::entra::PersonTokenVerifier;
use crate::grant::{GRANT_ID_PREFIX, GRANT_TYPE};
use crate::jose::{Jwks, now_secs, peek_claims};
use crate::policy::IdentityPolicy;
use crate::service_grants::Grants;
use crate::txn::{self, ACCESS_TOKEN_TYPE, TXN_TOKEN_TYPE, TxnClaims};
use crate::{IdentityError, TokenSigner};

/// The settings of an identity service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSettings {
    /// `iss` of the Txn-Tokens: the service's public URL.
    pub issuer: String,
    /// `aud` of the Txn-Tokens: the trust domain.
    pub trust_domain: String,
    /// The issuer of people's access tokens (Entra); agents read it from `/config` to tell an
    /// Entra token from a JWT-SVID.
    pub person_issuer: String,
    /// Lifetime of one Txn-Token.
    pub lifetime: Duration,
    /// Maximum lifetime of a chain, from its entry.
    pub max_chain_lifetime: Duration,
}

/// `POST /api/identity/token` (RFC 8693 shaped).
#[derive(Debug, Clone, Deserialize)]
pub struct TokenRequest {
    /// An access token of a person (entry), a grant ID (grant run) or a Txn-Token (renewal).
    pub subject_token: String,
    /// [`ACCESS_TOKEN_TYPE`], [`GRANT_TYPE`] or [`TXN_TOKEN_TYPE`]; without it a `grt_` ID is a
    /// grant and otherwise the kind follows from `iss`.
    #[serde(default)]
    pub subject_token_type: Option<String>,
    /// The purpose (`purp`) of a new chain (not for grant runs: their purposes are the grant's).
    #[serde(default)]
    pub purpose: Option<String>,
    /// For a grant run: the SPIFFE ID of the workload that called the entry agent.
    #[serde(default)]
    pub trigger: Option<String>,
}

/// The answer to a [`TokenRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    /// The Txn-Token.
    pub access_token: String,
    /// Always [`TXN_TOKEN_TYPE`].
    pub issued_token_type: String,
    /// `N_A` (it is not an OAuth access token).
    pub token_type: String,
    /// Seconds until it expires.
    pub expires_in: u64,
}

/// The identity service.
pub struct IdentityService {
    pub(crate) settings: ServiceSettings,
    signer: Arc<dyn TokenSigner>,
    pub(crate) people: Arc<dyn PersonTokenVerifier>,
    pub(crate) policy: IdentityPolicy,
    pub(crate) grants: Option<Grants>,
}

impl IdentityService {
    pub fn new(
        settings: ServiceSettings,
        signer: Arc<dyn TokenSigner>,
        people: Arc<dyn PersonTokenVerifier>,
        policy: IdentityPolicy,
    ) -> Self {
        Self {
            settings,
            signer,
            people,
            policy,
            grants: None,
        }
    }

    /// The settings.
    pub fn settings(&self) -> &ServiceSettings {
        &self.settings
    }

    /// `GET /api/identity/config`: what agents need besides the keys. No secrets.
    pub fn config(&self) -> Value {
        json!({
            "issuer": self.settings.issuer,
            "trust_domain": self.settings.trust_domain,
            "person_issuer": self.settings.person_issuer,
        })
    }

    /// `GET /api/identity/jwks.json`.
    pub async fn jwks(&self) -> Result<Jwks, IdentityError> {
        Ok(Jwks {
            keys: self.signer.public_keys().await?,
        })
    }

    /// `POST /api/identity/token` from the workload `caller` (already authenticated by SPIFFE).
    pub async fn token(
        &self,
        caller: &SpiffeId,
        request: TokenRequest,
    ) -> Result<TokenResponse, IdentityError> {
        if request.subject_token.is_empty() {
            return Err(IdentityError::BadRequest("subject_token is empty".into()));
        }
        let kind = match request.subject_token_type.as_deref() {
            Some(TXN_TOKEN_TYPE) => Kind::Renewal,
            Some(ACCESS_TOKEN_TYPE) => Kind::Entry,
            Some(GRANT_TYPE) => Kind::Grant,
            Some(other) => {
                return Err(IdentityError::BadRequest(format!(
                    "subject_token_type '{other}' is not supported"
                )));
            }
            None if request.subject_token.starts_with(GRANT_ID_PREFIX) => Kind::Grant,
            None if peek_claims(&request.subject_token)
                .is_some_and(|claims| claims["iss"] == *self.settings.issuer) =>
            {
                Kind::Renewal
            }
            None => Kind::Entry,
        };
        let claims = match kind {
            Kind::Renewal => self.renewal(caller, &request.subject_token).await?,
            Kind::Entry => self.entry(caller, &request).await?,
            Kind::Grant => self.grant_entry(caller, &request).await?,
        };
        let access_token = txn::issue(self.signer.as_ref(), &claims).await?;
        Ok(TokenResponse {
            access_token,
            issued_token_type: TXN_TOKEN_TYPE.into(),
            token_type: "N_A".into(),
            expires_in: claims.exp.saturating_sub(claims.iat),
        })
    }

    async fn entry(
        &self,
        caller: &SpiffeId,
        request: &TokenRequest,
    ) -> Result<TxnClaims, IdentityError> {
        self.policy
            .allows(caller, request.purpose.as_deref())
            .map_err(IdentityError::Forbidden)?;
        let person = self.people.verify(&request.subject_token).await?;
        let now = now_secs();
        let chain_exp = now + self.settings.max_chain_lifetime.as_secs();
        Ok(TxnClaims {
            iss: self.settings.issuer.clone(),
            aud: self.settings.trust_domain.clone(),
            sub: person.oid,
            name: person.name,
            upn: person.upn,
            txn: new_txn_id(),
            purp: request.purpose.clone(),
            azd: None,
            req_wl: caller.to_string(),
            iat: now,
            exp: (now + self.settings.lifetime.as_secs()).min(chain_exp),
            chain_exp,
        })
    }

    /// A fresh token of the same chain: everything kept but `iat` and `exp`, which never goes
    /// beyond `chain_exp`. Any workload of the token's trust domain may renew it (it holds it
    /// already; renewal widens nothing but the time, within the chain's limit).
    async fn renewal(&self, caller: &SpiffeId, token: &str) -> Result<TxnClaims, IdentityError> {
        let kid = txn::kid_of(token)?;
        let keys = self.signer.public_keys().await?;
        let key = keys.iter().find(|key| key.kid == kid).ok_or_else(|| {
            IdentityError::Unauthorized("Txn-Token is signed with an unknown key".into())
        })?;
        let now = now_secs();
        let mut claims = txn::check(
            token,
            key,
            &self.settings.trust_domain,
            Some(&self.settings.issuer),
            now,
        )?;
        if caller.trust_domain() != claims.aud {
            return Err(IdentityError::Forbidden(format!(
                "{caller} is outside the trust domain of the Txn-Token"
            )));
        }
        if claims.chain_exp <= now {
            return Err(IdentityError::Unauthorized(
                "the delegation chain has reached its maximum lifetime".into(),
            ));
        }
        claims.iat = now;
        claims.exp = (now + self.settings.lifetime.as_secs()).min(claims.chain_exp);
        Ok(claims)
    }
}

/// What a token request asks for.
enum Kind {
    Entry,
    Grant,
    Renewal,
}

/// A new chain ID: 128 random bits as hex.
pub(crate) fn new_txn_id() -> String {
    let mut bytes = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut bytes).expect("system random source");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
