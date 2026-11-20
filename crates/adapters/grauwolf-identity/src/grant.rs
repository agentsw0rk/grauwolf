//! Grants (ADR-0111 stage 3): a person's standing permission for unattended runs. A grant names
//! the entry agent, the trigger that may start it, the purposes, how long it is valid and how
//! long one run may last; the person's refresh token is stored apart from it.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_spiffe::SpiffeId;
use serde::{Deserialize, Serialize};

use crate::IdentityError;

/// The prefix of every grant ID.
pub const GRANT_ID_PREFIX: &str = "grt_";
/// How long a grant is valid when the request names no `valid_until`.
pub const DEFAULT_GRANT_VALIDITY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The longest validity of a grant.
pub const MAX_GRANT_VALIDITY: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// How long one run of a grant may last when the request names no `max_run`.
pub const DEFAULT_MAX_RUN: Duration = Duration::from_secs(4 * 60 * 60);
/// The token type of a grant reference at `POST /api/identity/token` (a grauwolf type).
pub const GRANT_TYPE: &str = "urn:grauwolf:params:oauth:token-type:grant";
/// The header with which a trigger names the grant of a run.
pub const GRANT_HEADER: &str = "Grauwolf-Grant";

/// Clock tolerance for a requested `valid_until` (the client computes it from its own clock).
const VALIDITY_LEEWAY_SECS: u64 = 300;

/// The person who created a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantOwner {
    /// The Entra object ID.
    pub oid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upn: Option<String>,
}

/// A grant: the metadata, never the refresh token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// `grt_` and 128 random bits as hex.
    pub id: String,
    pub owner: GrantOwner,
    /// The SPIFFE ID of the entry agent.
    pub agent: String,
    /// The SPIFFE ID of the workload that may start a run.
    pub trigger: String,
    /// The purposes; the first one becomes `purp` of a run's Txn-Token.
    #[serde(default)]
    pub purposes: Vec<String>,
    /// Unix seconds after which the grant is no longer valid.
    pub valid_until: u64,
    /// Seconds one run may last: the chain's `chain_exp` is its start plus this.
    pub max_run: u64,
    /// Unix seconds.
    pub created: u64,
    /// Unix seconds of the last run, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
}

/// What a person asks for when creating a grant (`POST /api/identity/grants` without the refresh
/// token).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRequest {
    pub agent: String,
    pub trigger: String,
    #[serde(default)]
    pub purposes: Vec<String>,
    /// Unix seconds; default now + [`DEFAULT_GRANT_VALIDITY`], at most now +
    /// [`MAX_GRANT_VALIDITY`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<u64>,
    /// Seconds; default [`DEFAULT_MAX_RUN`], at most `max_run_limit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_run: Option<u64>,
}

impl Grant {
    /// A new grant of `owner` from `request` at `now`: the IDs must be SPIFFE IDs, `valid_until`
    /// lies in the future and at most [`MAX_GRANT_VALIDITY`] ahead, `max_run` is between one
    /// minute and `max_run_limit`.
    pub fn new(
        owner: GrantOwner,
        request: GrantRequest,
        now: u64,
        max_run_limit: Duration,
    ) -> Result<Self, IdentityError> {
        let bad = |reason: String| Err(IdentityError::BadRequest(reason));
        for (field, value) in [("agent", &request.agent), ("trigger", &request.trigger)] {
            if let Err(error) = SpiffeId::parse(value) {
                return bad(format!("{field}: {error}"));
            }
        }
        if let Some(purpose) = request
            .purposes
            .iter()
            .find(|purpose| purpose.trim().is_empty() || purpose.contains(char::is_whitespace))
        {
            return bad(format!("the purpose '{purpose}' is empty or has spaces"));
        }
        let valid_until = request
            .valid_until
            .unwrap_or(now + DEFAULT_GRANT_VALIDITY.as_secs());
        if valid_until <= now {
            return bad("valid_until lies in the past".into());
        }
        if valid_until > now + MAX_GRANT_VALIDITY.as_secs() + VALIDITY_LEEWAY_SECS {
            return bad(format!(
                "a grant is valid for at most {} days",
                MAX_GRANT_VALIDITY.as_secs() / 86_400
            ));
        }
        let max_run = request.max_run.unwrap_or(DEFAULT_MAX_RUN.as_secs());
        if max_run < 60 || max_run > max_run_limit.as_secs() {
            return bad(format!(
                "max_run must be between 60 and {} seconds",
                max_run_limit.as_secs()
            ));
        }
        Ok(Self {
            id: new_grant_id(),
            owner,
            agent: request.agent,
            trigger: request.trigger,
            purposes: request.purposes,
            valid_until,
            max_run,
            created: now,
            last_used: None,
        })
    }

    /// Whether the grant can still be used at `now`.
    pub fn is_valid_at(&self, now: u64) -> bool {
        now < self.valid_until
    }
}

/// Whether `id` has the form of a grant ID (`grt_` and 32 lowercase hex digits); anything else is
/// never looked up, so it cannot reach a store path.
pub fn is_grant_id(id: &str) -> bool {
    id.strip_prefix(GRANT_ID_PREFIX).is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn new_grant_id() -> String {
    let mut bytes = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut bytes).expect("system random source");
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{GRANT_ID_PREFIX}{hex}")
}

/// Where grants and their refresh tokens are kept. Errors of the store itself are
/// `IdentityError::Unavailable`; the refresh token never appears in one.
pub trait GrantStore: Send + Sync {
    /// Creates or replaces the metadata of `grant`.
    fn save<'a>(&'a self, grant: &'a Grant) -> BoxFuture<'a, Result<(), IdentityError>>;
    /// Stores the refresh token of the grant `id` (at creation and after every rotation).
    fn save_token<'a>(
        &'a self,
        id: &'a str,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<(), IdentityError>>;
    /// The grant `id`, `None` if there is none.
    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Grant>, IdentityError>>;
    /// The refresh token of the grant `id`, `None` if there is none.
    fn token<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<String>, IdentityError>>;
    /// Every grant, ordered by ID.
    fn list(&self) -> BoxFuture<'_, Result<Vec<Grant>, IdentityError>>;
    /// Removes the grant `id` and its token; whether there was one.
    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<bool, IdentityError>>;
}

/// Grants in memory (tests, a service without OpenBao).
#[derive(Default)]
pub struct MemoryGrantStore {
    grants: Mutex<BTreeMap<String, Grant>>,
    tokens: Mutex<BTreeMap<String, String>>,
}

impl MemoryGrantStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl GrantStore for MemoryGrantStore {
    fn save<'a>(&'a self, grant: &'a Grant) -> BoxFuture<'a, Result<(), IdentityError>> {
        self.grants
            .lock()
            .unwrap()
            .insert(grant.id.clone(), grant.clone());
        Box::pin(async { Ok(()) })
    }

    fn save_token<'a>(
        &'a self,
        id: &'a str,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<(), IdentityError>> {
        self.tokens
            .lock()
            .unwrap()
            .insert(id.to_owned(), refresh_token.to_owned());
        Box::pin(async { Ok(()) })
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Grant>, IdentityError>> {
        let grant = self.grants.lock().unwrap().get(id).cloned();
        Box::pin(async move { Ok(grant) })
    }

    fn token<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<String>, IdentityError>> {
        let token = self.tokens.lock().unwrap().get(id).cloned();
        Box::pin(async move { Ok(token) })
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<Grant>, IdentityError>> {
        let grants = self.grants.lock().unwrap().values().cloned().collect();
        Box::pin(async move { Ok(grants) })
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<bool, IdentityError>> {
        let existed = self.grants.lock().unwrap().remove(id).is_some();
        let had_token = self.tokens.lock().unwrap().remove(id).is_some();
        Box::pin(async move { Ok(existed || had_token) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "spiffe://d.de/ns/coordinator/sa/coordinator";
    const TRIGGER: &str = "spiffe://d.de/ns/schedules/sa/morning";
    const NOW: u64 = 1_800_000_000;
    const DAY: u64 = 86_400;

    fn owner() -> GrantOwner {
        GrantOwner {
            oid: "oid-alice".into(),
            name: Some("Alice".into()),
            upn: None,
        }
    }

    fn request() -> GrantRequest {
        GrantRequest {
            agent: AGENT.into(),
            trigger: TRIGGER.into(),
            purposes: vec!["review".into()],
            ..GrantRequest::default()
        }
    }

    fn create(request: GrantRequest) -> Result<Grant, IdentityError> {
        Grant::new(owner(), request, NOW, Duration::from_secs(8 * 3600))
    }

    #[test]
    fn a_new_grant_has_an_id_and_the_defaults() {
        let grant = create(request()).unwrap();
        assert!(is_grant_id(&grant.id), "{}", grant.id);
        assert_eq!(grant.valid_until, NOW + 30 * DAY);
        assert_eq!(grant.max_run, 4 * 3600);
        assert_eq!((grant.created, grant.last_used), (NOW, None));
        assert_eq!(grant.purposes, ["review"]);
        assert!(grant.is_valid_at(NOW + 30 * DAY - 1));
        assert!(!grant.is_valid_at(NOW + 30 * DAY));
        assert_ne!(create(request()).unwrap().id, grant.id);
    }

    #[test]
    fn validity_run_time_and_ids_are_checked() {
        let with = |change: fn(&mut GrantRequest)| {
            let mut request = request();
            change(&mut request);
            create(request)
        };
        assert!(with(|r| r.valid_until = Some(NOW + 90 * DAY)).is_ok());
        assert!(with(|r| r.valid_until = Some(NOW + 91 * DAY)).is_err());
        assert!(with(|r| r.valid_until = Some(NOW - 1)).is_err());
        assert!(with(|r| r.max_run = Some(8 * 3600)).is_ok());
        assert!(with(|r| r.max_run = Some(8 * 3600 + 1)).is_err());
        assert!(with(|r| r.max_run = Some(30)).is_err());
        assert!(with(|r| r.agent = "coordinator".into()).is_err());
        assert!(with(|r| r.trigger = "https://x/y".into()).is_err());
        assert!(with(|r| r.purposes = vec!["a b".into()]).is_err());
        assert!(matches!(
            with(|r| r.purposes = vec![String::new()]),
            Err(IdentityError::BadRequest(_))
        ));
    }

    #[test]
    fn only_well_formed_ids_are_grant_ids() {
        assert!(is_grant_id("grt_0123456789abcdef0123456789abcdef"));
        for bad in [
            "grt_",
            "grt_0123456789ABCDEF0123456789abcdef",
            "grt_../../sys/policy/x0123456789abcd",
            "0123456789abcdef0123456789abcdef",
        ] {
            assert!(!is_grant_id(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn the_memory_store_keeps_grants_and_tokens_apart() {
        let store = MemoryGrantStore::new();
        let mut grant = create(request()).unwrap();
        store.save(&grant).await.unwrap();
        store.save_token(&grant.id, "rt-1").await.unwrap();
        assert_eq!(store.get(&grant.id).await.unwrap(), Some(grant.clone()));
        assert_eq!(
            store.token(&grant.id).await.unwrap().as_deref(),
            Some("rt-1")
        );
        grant.last_used = Some(NOW + 1);
        store.save(&grant).await.unwrap();
        store.save_token(&grant.id, "rt-2").await.unwrap();
        assert_eq!(store.list().await.unwrap(), [grant.clone()]);
        assert_eq!(
            store.token(&grant.id).await.unwrap().as_deref(),
            Some("rt-2")
        );
        assert!(store.delete(&grant.id).await.unwrap());
        assert_eq!(store.get(&grant.id).await.unwrap(), None);
        assert_eq!(store.token(&grant.id).await.unwrap(), None);
        assert!(!store.delete(&grant.id).await.unwrap());
    }
}
