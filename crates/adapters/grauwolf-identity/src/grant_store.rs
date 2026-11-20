//! Grants in OpenBao KV v2 (ADR-0111 stage 3, Q3): the metadata at
//! `<mount>/identity/grants/<id>`, the refresh token apart at `<mount>/identity/grant-tokens/<id>`,
//! so a policy can let admins list grants without reading tokens.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_openbao::{Method, Request, TokenSource, Transport};
use serde_json::{Value, json};

use crate::IdentityError;
use crate::grant::{Grant, GrantStore, is_grant_id};

/// The folder of the grants' metadata below the KV mount.
pub const GRANTS_PATH: &str = "identity/grants";
/// The folder of the grants' refresh tokens below the KV mount.
pub const GRANT_TOKENS_PATH: &str = "identity/grant-tokens";

/// Grants in a KV v2 mount of OpenBao; tokens from `tokens` (the identity service's login).
pub struct OpenBaoGrantStore<T> {
    transport: T,
    tokens: Arc<dyn TokenSource>,
    mount: String,
}

impl<T: Transport> OpenBaoGrantStore<T> {
    pub fn new(transport: T, tokens: Arc<dyn TokenSource>, mount: &str) -> Self {
        Self {
            transport,
            tokens,
            mount: mount.trim_matches('/').to_owned(),
        }
    }

    /// One call; `Ok(None)` for `404`, any other failure is `Unavailable` without a value.
    async fn call(
        &self,
        method: Method,
        path: String,
        body: Option<Value>,
    ) -> Result<Option<Value>, IdentityError> {
        let unavailable = |reason: String| IdentityError::Unavailable(format!("OpenBao: {reason}"));
        let token = self
            .tokens
            .token()
            .await
            .map_err(|error| unavailable(format!("login: {error}")))?;
        let response = self
            .transport
            .send(Request {
                method,
                path,
                token: Some(token),
                body,
            })
            .await
            .map_err(|error| unavailable(error.to_string()))?;
        match response.status {
            404 => Ok(None),
            200..=299 => Ok(Some(
                serde_json::from_str(&response.body).unwrap_or(Value::Null),
            )),
            status => Err(unavailable(format!("grant store: HTTP {status}"))),
        }
    }

    fn data_path(&self, folder: &str, id: &str) -> String {
        format!("/v1/{}/data/{folder}/{id}", self.mount)
    }

    async fn read(&self, folder: &str, id: &str) -> Result<Option<Value>, IdentityError> {
        if !is_grant_id(id) {
            return Ok(None);
        }
        Ok(self
            .call(Method::Get, self.data_path(folder, id), None)
            .await?
            .and_then(|body| body.pointer("/data/data").cloned()))
    }

    async fn write(&self, folder: &str, id: &str, data: Value) -> Result<(), IdentityError> {
        if !is_grant_id(id) {
            return Err(IdentityError::BadRequest("not a grant ID".into()));
        }
        self.call(
            Method::Post,
            self.data_path(folder, id),
            Some(json!({ "data": data })),
        )
        .await
        .map(drop)
    }

    async fn remove(&self, folder: &str, id: &str) -> Result<bool, IdentityError> {
        let path = format!("/v1/{}/metadata/{folder}/{id}", self.mount);
        let existed = self.call(Method::Get, path.clone(), None).await?.is_some();
        self.call(Method::Delete, path, None).await?;
        Ok(existed)
    }
}

impl<T: Transport> GrantStore for OpenBaoGrantStore<T> {
    fn save<'a>(&'a self, grant: &'a Grant) -> BoxFuture<'a, Result<(), IdentityError>> {
        Box::pin(async move {
            let data = serde_json::to_value(grant)
                .map_err(|error| IdentityError::Unavailable(error.to_string()))?;
            self.write(GRANTS_PATH, &grant.id, data).await
        })
    }

    fn save_token<'a>(
        &'a self,
        id: &'a str,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<(), IdentityError>> {
        Box::pin(async move {
            self.write(
                GRANT_TOKENS_PATH,
                id,
                json!({ "refresh_token": refresh_token }),
            )
            .await
        })
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Grant>, IdentityError>> {
        Box::pin(async move {
            match self.read(GRANTS_PATH, id).await? {
                None => Ok(None),
                Some(data) => serde_json::from_value(data).map(Some).map_err(|_| {
                    IdentityError::Unavailable(format!("OpenBao: the grant {id} is malformed"))
                }),
            }
        })
    }

    fn token<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<String>, IdentityError>> {
        Box::pin(async move {
            Ok(self
                .read(GRANT_TOKENS_PATH, id)
                .await?
                .and_then(|data| data["refresh_token"].as_str().map(str::to_owned)))
        })
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<Grant>, IdentityError>> {
        Box::pin(async move {
            let path = format!("/v1/{}/metadata/{GRANTS_PATH}?list=true", self.mount);
            let ids: Vec<String> = self
                .call(Method::Get, path, None)
                .await?
                .and_then(|body| body.pointer("/data/keys").cloned())
                .and_then(|keys| serde_json::from_value(keys).ok())
                .unwrap_or_default();
            let mut grants = Vec::new();
            for id in ids.iter().filter(|id| is_grant_id(id)) {
                if let Some(grant) = self.get(id).await? {
                    grants.push(grant);
                }
            }
            grants.sort_by(|a, b| a.id.cmp(&b.id));
            Ok(grants)
        })
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<bool, IdentityError>> {
        Box::pin(async move {
            if !is_grant_id(id) {
                return Ok(false);
            }
            // The token first: a grant without a token can no longer run.
            let token = self.remove(GRANT_TOKENS_PATH, id).await?;
            let grant = self.remove(GRANTS_PATH, id).await?;
            Ok(token || grant)
        })
    }
}
