use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::BaoError;
use crate::transport::{Method, Request, Response, Transport};

/// What an `ensure_*` call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Updated,
    Unchanged,
}

/// The token of a login and its lease in seconds.
pub struct Token {
    pub value: String,
    pub lease_seconds: u64,
}

/// The answer of `sys/health`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub initialized: bool,
    pub sealed: bool,
    pub version: Option<String>,
}

/// The calls of OpenBao that agents and the setup program need.
pub struct Bao<T> {
    transport: T,
    token: Option<String>,
}

impl<T: Transport> Bao<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            token: None,
        }
    }

    /// The same server, calls made with `token` (admin calls need one).
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub(crate) fn transport(&self) -> &T {
        &self.transport
    }

    async fn call(
        &self,
        what: &str,
        method: Method,
        path: String,
        body: Option<Value>,
        ok: &[u16],
    ) -> Result<Response, BaoError> {
        let response = self
            .transport
            .send(Request {
                method,
                path,
                token: self.token.clone(),
                body,
            })
            .await?;
        if response.status / 100 == 2 || ok.contains(&response.status) {
            Ok(response)
        } else {
            Err(BaoError::status(what, response.status, &response.body))
        }
    }

    /// `Ok(None)` when the server answers `404`.
    async fn find(&self, what: &str, path: String) -> Result<Option<Value>, BaoError> {
        let response = self.call(what, Method::Get, path, None, &[404]).await?;
        if response.status == 404 {
            return Ok(None);
        }
        json(&response).map(Some)
    }

    // ---- agents: login and KV read -----------------------------------------------------------

    /// `POST /v1/auth/<mount>/login`: log in with a JWT-SVID.
    pub async fn login_jwt(&self, mount: &str, role: &str, jwt: &str) -> Result<Token, BaoError> {
        let path = format!("/v1/auth/{}/login", trim(mount));
        let response = self
            .call(
                "login",
                Method::Post,
                path,
                Some(json!({ "role": role, "jwt": jwt })),
                &[],
            )
            .await?;
        let body = json(&response).map_err(|_| BaoError::other("the login answer is not JSON"))?;
        let value = body
            .pointer("/auth/client_token")
            .and_then(Value::as_str)
            .ok_or_else(|| BaoError::other("the login answer has no client_token"))?
            .to_owned();
        let lease_seconds = body
            .pointer("/auth/lease_duration")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Ok(Token {
            value,
            lease_seconds,
        })
    }

    /// The field `field` of the KV v2 secret `path` in `mount`.
    pub async fn kv_read_field(
        &self,
        mount: &str,
        path: &str,
        field: &str,
    ) -> Result<String, BaoError> {
        let response = self
            .call(
                "read",
                Method::Get,
                format!("/v1/{}/data/{path}", trim(mount)),
                None,
                &[],
            )
            .await?;
        let body = json(&response)?;
        match body.pointer(&format!("/data/data/{}", escape(field))) {
            Some(Value::String(value)) => Ok(value.clone()),
            Some(_) => Err(BaoError::other(format!("field '{field}' is not a string"))),
            None => Err(BaoError::other(format!(
                "the secret has no field '{field}'"
            ))),
        }
    }

    // ---- status ------------------------------------------------------------------------------

    /// `sys/health` (answers with a status other than 200 when sealed or standby; the body is
    /// read anyway).
    pub async fn health(&self) -> Result<Health, BaoError> {
        let response = self
            .transport
            .send(Request {
                method: Method::Get,
                path: "/v1/sys/health".to_owned(),
                token: None,
                body: None,
            })
            .await?;
        let body: Value = serde_json::from_str(&response.body)
            .map_err(|_| BaoError::status("health", response.status, &response.body))?;
        Ok(Health {
            initialized: body["initialized"].as_bool().unwrap_or(false),
            sealed: body["sealed"].as_bool().unwrap_or(false),
            version: body["version"].as_str().map(str::to_owned),
        })
    }

    /// `sys/capabilities-self`: what the token may do on each of `paths` (e.g. `["read",
    /// "update"]`, `["deny"]`). Fails with `403` if the token is invalid.
    pub async fn capabilities_self(
        &self,
        paths: &[&str],
    ) -> Result<BTreeMap<String, Vec<String>>, BaoError> {
        let response = self
            .call(
                "check capabilities",
                Method::Post,
                "/v1/sys/capabilities-self".to_owned(),
                Some(json!({ "paths": paths })),
                &[],
            )
            .await?;
        let body = json(&response)?;
        let data = body.get("data").unwrap_or(&body);
        Ok(paths
            .iter()
            .map(|path| {
                let caps = data
                    .get(*path)
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                ((*path).to_owned(), caps)
            })
            .collect())
    }

    /// `auth/token/lookup-self`: the policies of the token in use (`root`, `default`, ...).
    pub async fn token_policies(&self) -> Result<Vec<String>, BaoError> {
        let response = self
            .call(
                "look up the token",
                Method::Get,
                "/v1/auth/token/lookup-self".to_owned(),
                None,
                &[],
            )
            .await?;
        let body = json(&response)?;
        Ok(body
            .pointer("/data/policies")
            .and_then(Value::as_array)
            .map(|policies| {
                policies
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `auth/token/create`: a token with exactly the policy `policy` (not `default`), valid for
    /// `ttl` (`8h`), `renewable` or not. It is created without a parent if the token in use may
    /// (root or `sudo`), so it outlives the token that made it; else as a child of that token
    /// (the server answers `403` to `no_parent`). Returns the token, which is shown only now.
    pub async fn create_token(
        &self,
        policy: &str,
        ttl: &str,
        renewable: bool,
    ) -> Result<String, BaoError> {
        let body = |no_parent: bool| {
            json!({
                "policies": [policy],
                "ttl": ttl,
                "renewable": renewable,
                "no_default_policy": true,
                "no_parent": no_parent,
            })
        };
        let create = |no_parent: bool| {
            self.call(
                "create token",
                Method::Post,
                "/v1/auth/token/create".to_owned(),
                Some(body(no_parent)),
                &[],
            )
        };
        let response = match create(true).await {
            Err(error) if error.status == Some(403) => create(false).await?,
            other => other?,
        };
        json(&response)?
            .pointer("/auth/client_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| BaoError::other("the answer has no client_token"))
    }

    /// The enabled auth methods: mount path (without slash) to type.
    pub async fn auth_methods(&self) -> Result<BTreeMap<String, String>, BaoError> {
        let body = self
            .find("list auth methods", "/v1/sys/auth".to_owned())
            .await?
            .unwrap_or(Value::Null);
        Ok(types_of(&body))
    }

    /// The mounted secret engines: mount path (without slash) to (type, KV version).
    pub async fn mounts(&self) -> Result<BTreeMap<String, (String, Option<String>)>, BaoError> {
        let body = self
            .find("list mounts", "/v1/sys/mounts".to_owned())
            .await?
            .unwrap_or(Value::Null);
        let data = body.get("data").unwrap_or(&body);
        let mut mounts = BTreeMap::new();
        if let Some(map) = data.as_object() {
            for (path, mount) in map {
                if let Some(kind) = mount.get("type").and_then(Value::as_str) {
                    let version = mount
                        .pointer("/options/version")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    mounts.insert(trim(path).to_owned(), (kind.to_owned(), version));
                }
            }
        }
        Ok(mounts)
    }

    // ---- setup: auth method, KV mount ------------------------------------------------------

    /// The JWT auth method at `mount`, trusting the keys at `jwks_url` (and `bound_issuer` if
    /// given).
    pub async fn ensure_jwt_auth(
        &self,
        mount: &str,
        jwks_url: &str,
        bound_issuer: Option<&str>,
    ) -> Result<Outcome, BaoError> {
        let mount = trim(mount);
        let methods = self.auth_methods().await?;
        let mut outcome = Outcome::Unchanged;
        match methods.get(mount).map(String::as_str) {
            Some("jwt") => {}
            Some(other) => {
                return Err(BaoError::other(format!(
                    "auth/{mount}/ exists with type '{other}', not 'jwt'"
                )));
            }
            None => {
                self.call(
                    "enable auth method",
                    Method::Post,
                    format!("/v1/sys/auth/{mount}"),
                    Some(json!({ "type": "jwt" })),
                    &[],
                )
                .await?;
                outcome = Outcome::Created;
            }
        }
        let desired = match bound_issuer {
            Some(issuer) => json!({ "jwks_url": jwks_url, "bound_issuer": issuer }),
            None => json!({ "jwks_url": jwks_url }),
        };
        let config_path = format!("/v1/auth/{mount}/config");
        let current = self.find("read auth config", config_path.clone()).await?;
        let up_to_date = current
            .as_ref()
            .and_then(|body| body.get("data"))
            .is_some_and(|data| subset(&desired, data));
        if !up_to_date {
            self.call(
                "write auth config",
                Method::Post,
                config_path,
                Some(desired),
                &[],
            )
            .await?;
            if outcome == Outcome::Unchanged {
                outcome = Outcome::Updated;
            }
        }
        Ok(outcome)
    }

    /// The `jwks_url` configured for the JWT auth method at `mount`.
    pub async fn jwt_config(&self, mount: &str) -> Result<Option<Value>, BaoError> {
        Ok(self
            .find(
                "read auth config",
                format!("/v1/auth/{}/config", trim(mount)),
            )
            .await?
            .and_then(|body| body.get("data").cloned()))
    }

    /// A KV v2 mount at `mount`.
    pub async fn ensure_kv2(&self, mount: &str) -> Result<Outcome, BaoError> {
        let mount = trim(mount);
        match self.mounts().await?.get(mount) {
            Some((kind, version)) if kind == "kv" && version.as_deref() == Some("2") => {
                Ok(Outcome::Unchanged)
            }
            Some((kind, version)) => Err(BaoError::other(format!(
                "{mount}/ exists as '{kind}' (version {}), not as KV v2",
                version.as_deref().unwrap_or("1")
            ))),
            None => {
                self.call(
                    "enable KV mount",
                    Method::Post,
                    format!("/v1/sys/mounts/{mount}"),
                    Some(json!({ "type": "kv", "options": { "version": "2" } })),
                    &[],
                )
                .await?;
                Ok(Outcome::Created)
            }
        }
    }

    /// A Transit secrets engine at `mount` (ADR-0111: the identity service signs with it).
    pub async fn ensure_transit(&self, mount: &str) -> Result<Outcome, BaoError> {
        let mount = trim(mount);
        match self.mounts().await?.get(mount) {
            Some((kind, _)) if kind == "transit" => Ok(Outcome::Unchanged),
            Some((kind, _)) => Err(BaoError::other(format!(
                "{mount}/ exists as '{kind}', not as transit"
            ))),
            None => {
                self.call(
                    "enable transit mount",
                    Method::Post,
                    format!("/v1/sys/mounts/{mount}"),
                    Some(json!({ "type": "transit" })),
                    &[],
                )
                .await?;
                Ok(Outcome::Created)
            }
        }
    }

    /// The type of the Transit key `key` (`ecdsa-p256`, ...); `None` if there is none.
    pub async fn transit_key_type(
        &self,
        mount: &str,
        key: &str,
    ) -> Result<Option<String>, BaoError> {
        Ok(self
            .find(
                "read transit key",
                format!("/v1/{}/keys/{key}", trim(mount)),
            )
            .await?
            .and_then(|body| {
                body.pointer("/data/type")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }))
    }

    /// The Transit key `key` of type `kind`, created if missing; a key of another type is an error
    /// (a key is never replaced: its versions sign tokens that are still valid).
    pub async fn ensure_transit_key(
        &self,
        mount: &str,
        key: &str,
        kind: &str,
    ) -> Result<Outcome, BaoError> {
        match self.transit_key_type(mount, key).await? {
            Some(existing) if existing == kind => Ok(Outcome::Unchanged),
            Some(existing) => Err(BaoError::other(format!(
                "transit key '{key}' exists with type '{existing}', not '{kind}'"
            ))),
            None => {
                self.call(
                    "create transit key",
                    Method::Post,
                    format!("/v1/{}/keys/{key}", trim(mount)),
                    Some(json!({ "type": kind })),
                    &[],
                )
                .await?;
                Ok(Outcome::Created)
            }
        }
    }

    /// Deletes the Transit key `key` (first allowing its deletion); `false` if there was none.
    pub async fn delete_transit_key(&self, mount: &str, key: &str) -> Result<bool, BaoError> {
        if self.transit_key_type(mount, key).await?.is_none() {
            return Ok(false);
        }
        let mount = trim(mount);
        self.call(
            "allow deleting the transit key",
            Method::Post,
            format!("/v1/{mount}/keys/{key}/config"),
            Some(json!({ "deletion_allowed": true })),
            &[],
        )
        .await?;
        self.call(
            "delete transit key",
            Method::Delete,
            format!("/v1/{mount}/keys/{key}"),
            None,
            &[],
        )
        .await?;
        Ok(true)
    }

    // ---- policies --------------------------------------------------------------------------

    pub async fn read_policy(&self, name: &str) -> Result<Option<String>, BaoError> {
        Ok(self
            .find("read policy", format!("/v1/sys/policies/acl/{name}"))
            .await?
            .and_then(|body| {
                body.pointer("/data/policy")
                    .or_else(|| body.get("policy"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }))
    }

    pub async fn ensure_policy(&self, name: &str, hcl: &str) -> Result<Outcome, BaoError> {
        let current = self.read_policy(name).await?;
        if current.as_deref().map(str::trim) == Some(hcl.trim()) {
            return Ok(Outcome::Unchanged);
        }
        self.call(
            "write policy",
            Method::Put,
            format!("/v1/sys/policies/acl/{name}"),
            Some(json!({ "policy": hcl })),
            &[],
        )
        .await?;
        Ok(if current.is_some() {
            Outcome::Updated
        } else {
            Outcome::Created
        })
    }

    /// Deletes the policy; `false` if there was none.
    pub async fn delete_policy(&self, name: &str) -> Result<bool, BaoError> {
        let existed = self.read_policy(name).await?.is_some();
        if existed {
            self.call(
                "delete policy",
                Method::Delete,
                format!("/v1/sys/policies/acl/{name}"),
                None,
                &[],
            )
            .await?;
        }
        Ok(existed)
    }

    // ---- JWT roles -------------------------------------------------------------------------

    pub async fn read_jwt_role(&self, mount: &str, name: &str) -> Result<Option<Value>, BaoError> {
        Ok(self
            .find("read role", format!("/v1/auth/{}/role/{name}", trim(mount)))
            .await?
            .and_then(|body| body.get("data").cloned()))
    }

    /// Writes the role if it differs from `desired` (see [`jwt_role`]).
    pub async fn ensure_jwt_role(
        &self,
        mount: &str,
        name: &str,
        desired: &Value,
    ) -> Result<Outcome, BaoError> {
        let current = self.read_jwt_role(mount, name).await?;
        if current.as_ref().is_some_and(|data| subset(desired, data)) {
            return Ok(Outcome::Unchanged);
        }
        self.call(
            "write role",
            Method::Post,
            format!("/v1/auth/{}/role/{name}", trim(mount)),
            Some(desired.clone()),
            &[],
        )
        .await?;
        Ok(if current.is_some() {
            Outcome::Updated
        } else {
            Outcome::Created
        })
    }

    pub async fn list_jwt_roles(&self, mount: &str) -> Result<Vec<String>, BaoError> {
        self.list(&format!("/v1/auth/{}/role", trim(mount))).await
    }

    /// Deletes the role; `false` if there was none.
    pub async fn delete_jwt_role(&self, mount: &str, name: &str) -> Result<bool, BaoError> {
        let existed = self.read_jwt_role(mount, name).await?.is_some();
        if existed {
            self.call(
                "delete role",
                Method::Delete,
                format!("/v1/auth/{}/role/{name}", trim(mount)),
                None,
                &[],
            )
            .await?;
        }
        Ok(existed)
    }

    // ---- KV v2 -----------------------------------------------------------------------------

    /// Stores `value` in the field `field` of `path` and returns the new version. The value is
    /// sent, never returned or put into an error.
    pub async fn kv_put_field(
        &self,
        mount: &str,
        path: &str,
        field: &str,
        value: &str,
    ) -> Result<u64, BaoError> {
        let response = self
            .call(
                "write",
                Method::Post,
                format!("/v1/{}/data/{path}", trim(mount)),
                Some(json!({ "data": { field: value } })),
                &[],
            )
            .await?;
        Ok(json(&response)
            .ok()
            .and_then(|body| body.pointer("/data/version").and_then(Value::as_u64))
            .unwrap_or(0))
    }

    /// Every field of the KV v2 secret `path` (its `data` object), `None` if it does not exist.
    pub async fn kv_read(&self, mount: &str, path: &str) -> Result<Option<Value>, BaoError> {
        Ok(self
            .find("read", format!("/v1/{}/data/{path}", trim(mount)))
            .await?
            .and_then(|body| body.pointer("/data/data").cloned()))
    }

    /// The names below `path` (a name ending in `/` is a folder); empty if there are none.
    pub async fn kv_list(&self, mount: &str, path: &str) -> Result<Vec<String>, BaoError> {
        self.list(&format!("/v1/{}/metadata/{path}", trim(mount)))
            .await
    }

    /// The current version of the secret at `path`, `None` if it does not exist.
    pub async fn kv_current_version(
        &self,
        mount: &str,
        path: &str,
    ) -> Result<Option<u64>, BaoError> {
        Ok(self
            .find(
                "read metadata",
                format!("/v1/{}/metadata/{path}", trim(mount)),
            )
            .await?
            .and_then(|body| {
                body.pointer("/data/current_version")
                    .and_then(Value::as_u64)
            }))
    }

    /// Removes the secret at `path` with all its versions.
    pub async fn kv_delete_all(&self, mount: &str, path: &str) -> Result<(), BaoError> {
        self.call(
            "delete",
            Method::Delete,
            format!("/v1/{}/metadata/{path}", trim(mount)),
            None,
            &[404],
        )
        .await?;
        Ok(())
    }

    /// Removes every secret below `prefix` (recursively) and returns their paths.
    pub async fn kv_delete_tree(&self, mount: &str, prefix: &str) -> Result<Vec<String>, BaoError> {
        let mut removed = Vec::new();
        let mut pending = vec![prefix.trim_end_matches('/').to_owned()];
        while let Some(folder) = pending.pop() {
            for name in self.kv_list(mount, &folder).await? {
                match name.strip_suffix('/') {
                    Some(sub) => pending.push(format!("{folder}/{sub}")),
                    None => {
                        let path = format!("{folder}/{name}");
                        self.kv_delete_all(mount, &path).await?;
                        removed.push(path);
                    }
                }
            }
        }
        removed.sort();
        Ok(removed)
    }

    async fn list(&self, path: &str) -> Result<Vec<String>, BaoError> {
        let body = self
            .find("list", format!("{path}?list=true"))
            .await?
            .unwrap_or(Value::Null);
        Ok(body
            .pointer("/data/keys")
            .and_then(Value::as_array)
            .map(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// The body of a JWT role: a JWT with `audience` and exactly the subject `subject` gets `policy`.
pub fn jwt_role(audience: &str, subject: &str, policy: &str) -> Value {
    json!({
        "role_type": "jwt",
        "user_claim": "sub",
        "bound_audiences": [audience],
        "bound_subject": subject,
        "token_policies": [policy],
        "token_ttl": 3600,
    })
}

/// A policy that only reads the KV v2 secrets below `prefix` in `mount`.
pub fn kv_read_policy(mount: &str, prefix: &str) -> String {
    format!(
        "path \"{}/data/{}/*\" {{\n  capabilities = [\"read\"]\n}}\n",
        trim(mount),
        prefix.trim_matches('/')
    )
}

fn trim(mount: &str) -> &str {
    mount.trim_matches('/')
}

fn json(response: &Response) -> Result<Value, BaoError> {
    serde_json::from_str(&response.body).map_err(|_| BaoError::other("the answer is not JSON"))
}

/// JSON pointer escaping of a field name.
fn escape(field: &str) -> String {
    field.replace('~', "~0").replace('/', "~1")
}

fn types_of(body: &Value) -> BTreeMap<String, String> {
    let data = body.get("data").unwrap_or(body);
    data.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(path, entry)| {
            let kind = entry.get("type")?.as_str()?;
            Some((trim(path).to_owned(), kind.to_owned()))
        })
        .collect()
}

/// Every key of `desired` is in `actual` with an equal value (objects recursively).
fn subset(desired: &Value, actual: &Value) -> bool {
    match (desired, actual) {
        (Value::Object(want), Value::Object(have)) => want
            .iter()
            .all(|(key, value)| have.get(key).is_some_and(|other| subset(value, other))),
        _ => desired == actual,
    }
}
