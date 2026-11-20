//! The workload identity for outbound calls (ADR-0109): one identity per Workload API socket,
//! shared by all clients of the process, and the `auth.spiffe` of the spec turned into a
//! [`ClientAuth`]. The identities are kept per process (the one exception to "no process-wide
//! state": the Workload API streams should run once, not per client).

use std::collections::HashMap;
use std::sync::Arc;

use grauwolf_spec::{AuthSpec, SpiffeMode};
use grauwolf_spiffe::{
    AuthMode, ClientAuth, Identity, SpiffeId, SpiffeMatch, SpiffeWorkload, WorkloadApiClient,
};
use tokio::sync::Mutex;

use crate::config::Env;

static WORKLOADS: std::sync::OnceLock<Mutex<HashMap<String, Arc<SpiffeWorkload>>>> =
    std::sync::OnceLock::new();

/// The live identity at `SPIFFE_ENDPOINT_SOCKET` (started on first use, waiting for the first
/// SVID up to `SPIFFE_FETCH_TIMEOUT`), or `None` if the variable is not set.
pub async fn workload(env: &dyn Env) -> Result<Option<Arc<dyn Identity>>, String> {
    let Some(client) = WorkloadApiClient::from_env(env).map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let key = client.socket().display().to_string();
    let mut known = WORKLOADS.get_or_init(Default::default).lock().await;
    if let Some(workload) = known.get(&key) {
        return Ok(Some(Arc::clone(workload) as Arc<dyn Identity>));
    }
    let workload = Arc::new(
        SpiffeWorkload::connect(client)
            .await
            .map_err(|error| error.to_string())?,
    );
    known.insert(key, Arc::clone(&workload));
    Ok(Some(workload))
}

/// The variables that hand the Workload API to a child process (`workload_api: true`,
/// ADR-0110): `SPIFFE_ENDPOINT_SOCKET` and, if set, `SPIFFE_FETCH_TIMEOUT`. Without a socket an
/// error that names `what` (`MCP server 'x'`, `builtin_tools.command`).
pub fn workload_env(env: &dyn Env, what: &str) -> Result<Vec<(String, String)>, String> {
    let socket = env
        .var(grauwolf_spiffe::ENDPOINT_VARIABLE)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "{what}: `workload_api` needs a workload identity, but {} is not set",
                grauwolf_spiffe::ENDPOINT_VARIABLE
            )
        })?;
    let mut vars = vec![(grauwolf_spiffe::ENDPOINT_VARIABLE.to_owned(), socket)];
    if let Some(timeout) = env
        .var(grauwolf_spiffe::FETCH_TIMEOUT_VARIABLE)
        .filter(|value| !value.is_empty())
    {
        vars.push((grauwolf_spiffe::FETCH_TIMEOUT_VARIABLE.to_owned(), timeout));
    }
    Ok(vars)
}

/// The client authentication for `auth` of the target `what` (`sub-agent 'x'`, `MCP server
/// 'y'`); without a Workload API socket an error that names the target.
pub async fn client_auth(
    env: &dyn Env,
    what: &str,
    auth: &AuthSpec,
) -> Result<Arc<ClientAuth>, String> {
    let identity = workload(env).await?.ok_or_else(|| {
        format!(
            "{what}: `auth.spiffe` needs a workload identity, but {} is not set",
            grauwolf_spiffe::ENDPOINT_VARIABLE
        )
    })?;
    let spiffe = &auth.spiffe;
    let parse = |id: &str| SpiffeId::parse(id).map_err(|error| format!("{what}: {error}"));
    let expected = if let Some(id) = &spiffe.id {
        Some(SpiffeMatch::Id(parse(id)?))
    } else if !spiffe.ids.is_empty() {
        Some(SpiffeMatch::Ids(
            spiffe
                .ids
                .iter()
                .map(|id| parse(id))
                .collect::<Result<_, _>>()?,
        ))
    } else {
        spiffe.trust_domain.clone().map(SpiffeMatch::TrustDomain)
    };
    let mode = match spiffe.mode() {
        SpiffeMode::Mtls => AuthMode::Mtls,
        SpiffeMode::Jwt => AuthMode::Jwt,
    };
    ClientAuth::new(identity, mode, expected, spiffe.audience.clone())
        .map(Arc::new)
        .map_err(|error| format!("{what}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MapEnv;
    use grauwolf_spiffe::testing::FakeWorkloadApi;
    use grauwolf_test::TestDir;

    const ID: &str = "spiffe://agentswork.de/ns/reviewer/sa/reviewer";

    fn auth(json: serde_json::Value) -> AuthSpec {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn workload_env_hands_on_the_socket_and_the_timeout_or_names_the_missing_socket() {
        let env = MapEnv::new([
            ("SPIFFE_ENDPOINT_SOCKET", "unix:///s.sock"),
            ("SPIFFE_FETCH_TIMEOUT", "5s"),
        ]);
        assert_eq!(
            workload_env(&env, "x").unwrap(),
            [
                (
                    "SPIFFE_ENDPOINT_SOCKET".to_owned(),
                    "unix:///s.sock".to_owned()
                ),
                ("SPIFFE_FETCH_TIMEOUT".to_owned(), "5s".to_owned())
            ]
        );
        let error = workload_env(&MapEnv::default(), "MCP server 'files'").unwrap_err();
        assert!(error.contains("MCP server 'files'"), "{error}");
        assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
    }

    #[tokio::test]
    async fn without_a_socket_the_error_names_the_target_and_the_variable() {
        let error = client_auth(
            &MapEnv::default(),
            "sub-agent 'reviewer'",
            &auth(serde_json::json!({ "spiffe": { "id": ID } })),
        )
        .await
        .unwrap_err();
        assert!(error.contains("sub-agent 'reviewer'"), "{error}");
        assert!(error.contains("SPIFFE_ENDPOINT_SOCKET"), "{error}");
    }

    #[tokio::test]
    async fn with_a_socket_mtls_gets_a_tls_config_and_jwt_a_bearer() {
        let dir = TestDir::new("host-spiffe-auth");
        let api = FakeWorkloadApi::start_denying(&dir.join("agent.sock"), ID, 0);
        let env = MapEnv::new([("SPIFFE_ENDPOINT_SOCKET", api.endpoint().as_str())]);
        let mtls = client_auth(
            &env,
            "x",
            &auth(serde_json::json!({ "spiffe": { "id": ID } })),
        )
        .await
        .unwrap();
        assert!(mtls.tls_config().unwrap().is_some());
        assert!(mtls.bearer().await.unwrap().is_none());
        let jwt = client_auth(
            &env,
            "x",
            &auth(serde_json::json!({ "spiffe": { "mode": "jwt", "audience": "tickets" } })),
        )
        .await
        .unwrap();
        assert!(jwt.tls_config().unwrap().is_none());
        assert!(jwt.bearer().await.unwrap().is_some());
        assert_eq!(api.requests(), vec![vec!["tickets".to_owned()]]);
    }
}
