//! A fake cluster with SPIRE installed and a fake OpenBao behind the API server's service proxy.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use clap::Parser;
use grauwolf_k8_ops_app::{Cli, Io, Services, execute};
use grauwolf_k8s::testing::FakeCluster;
use serde_json::{Value, json};

pub const DOMAIN: &str = "agentswork.de";

/// A CA certificate with the URI SAN `spiffe://<domain>` (made with openssl, see the x509 tests).
pub fn ca_pem(domain: &str) -> &'static str {
    match domain {
        "example.org" => include_str!("example.org.pem"),
        "agentswork.de" => include_str!("agentswork.de.pem"),
        "other.test" => include_str!("other.test.pem"),
        _ => panic!("no test CA for {domain}"),
    }
}

pub struct BaoState {
    pub sealed: bool,
    pub initialized: bool,
    /// `None`: the token is invalid (403); else the capabilities it has on every path.
    pub capabilities: Option<Vec<String>>,
    pub auth: BTreeMap<String, String>,
    pub jwt_config: BTreeMap<String, Value>,
    pub mounts: BTreeMap<String, Value>,
    pub policies: BTreeMap<String, String>,
    pub roles: BTreeMap<String, Value>,
    /// `kv/agents/a/key` -> versions (values).
    pub kv: BTreeMap<String, Vec<String>>,
    /// `kv/identity/grants/grt_…` -> the whole data of the latest version (secrets with other
    /// fields than `value`).
    pub kv_data: BTreeMap<String, Value>,
    /// Writes of secrets answer 403.
    pub deny_kv_writes: bool,
    /// The mutating calls, in order.
    pub writes: Vec<String>,
    /// The policies of the token in use (`auth/token/lookup-self`).
    pub token_policies: Vec<String>,
    /// The policy that every call (except `sys/health`) is checked against, as OpenBao does
    /// for a token with only that policy; `None`: the token may do everything (root).
    pub enforce: Option<String>,
    /// `METHOD /path` of the calls the enforced policy refused.
    pub denied: Vec<String>,
    /// The bodies of `auth/token/create`.
    pub created_tokens: Vec<Value>,
    /// Transit keys `<mount>/<name>` -> type.
    pub transit_keys: BTreeMap<String, String>,
}

/// The capabilities `policy` (HCL: `path "<p>" { capabilities = [...] }`) gives on `path`: the
/// exact path wins, else the longest `prefix*`.
pub fn policy_capabilities(policy: &str, path: &str) -> Vec<String> {
    let mut rules: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<String> = None;
    for line in policy.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("path \"") {
            current = rest.split('"').next().map(str::to_owned);
        } else if let (Some(path), Some(list)) = (&current, line.strip_prefix("capabilities = [")) {
            let caps = list
                .trim_end_matches(']')
                .split(',')
                .map(|cap| cap.trim().trim_matches('"').to_owned())
                .filter(|cap| !cap.is_empty())
                .collect();
            rules.push((path.clone(), caps));
        }
    }
    if let Some((_, caps)) = rules.iter().find(|(rule, _)| rule == path) {
        return caps.clone();
    }
    rules
        .iter()
        .filter_map(|(rule, caps)| Some((rule.strip_suffix('*')?, caps)))
        .filter(|(prefix, _)| path.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, caps)| caps.clone())
        .unwrap_or_default()
}

/// Whether `policy` allows the request: `read`, `list`, `create`/`update` or `delete`; writing
/// below `sys/auth/` also needs `sudo`.
pub fn policy_allows(policy: &str, method: &str, path: &str, list: bool) -> bool {
    let caps = policy_capabilities(policy, path.trim_start_matches("/v1/"));
    let has = |names: &[&str]| caps.iter().any(|cap| names.contains(&cap.as_str()));
    match method {
        "GET" if list => has(&["list"]),
        "GET" => has(&["read"]),
        "POST" | "PUT" if path.trim_start_matches("/v1/").starts_with("sys/auth/") => {
            has(&["sudo"]) && has(&["create", "update"])
        }
        "POST" | "PUT" => has(&["create", "update"]),
        "DELETE" => has(&["delete"]),
        _ => false,
    }
}

pub struct Env {
    pub fake: FakeCluster,
    pub bao: Arc<Mutex<BaoState>>,
    pub token: Option<String>,
    /// The status `/health` of a deployed agent answers through the service proxy.
    pub health: Arc<AtomicU16>,
    /// What host names resolve to for the egress policies (nothing by default).
    pub dns: Mutex<BTreeMap<String, Vec<std::net::IpAddr>>>,
}

pub struct Run {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Run {
    pub fn all(&self) -> String {
        format!("{}{}", self.out, self.err)
    }
}

/// SPIRE as k3s runs it, with a fake OpenBao that has the token's capabilities of an admin.
pub fn env() -> Env {
    let fake = FakeCluster::new();
    seed_spire(&fake, DOMAIN);
    let bao = Arc::new(Mutex::new(BaoState {
        sealed: false,
        initialized: true,
        capabilities: Some(vec!["root".into()]),
        auth: BTreeMap::from([("token".into(), "token".into())]),
        jwt_config: BTreeMap::new(),
        mounts: BTreeMap::new(),
        policies: BTreeMap::new(),
        roles: BTreeMap::new(),
        kv: BTreeMap::new(),
        kv_data: BTreeMap::new(),
        deny_kv_writes: false,
        writes: Vec::new(),
        token_policies: vec!["root".into()],
        enforce: None,
        denied: Vec::new(),
        created_tokens: Vec::new(),
        transit_keys: BTreeMap::new(),
    }));
    let state = bao.clone();
    let health = Arc::new(AtomicU16::new(200));
    let health_status = health.clone();
    fake.on_proxy(move |method, service, path, body| {
        if path == "/health" {
            return (health_status.load(Ordering::SeqCst), "ok".to_owned());
        }
        if service == "http:spire-oidc-discovery-provider:80" {
            return (200, json!({"keys": [{"kid": "k"}]}).to_string());
        }
        bao_call(&mut state.lock().unwrap(), method, path, body)
    });
    Env {
        fake,
        bao,
        token: Some("root-token".into()),
        health,
        dns: Mutex::new(BTreeMap::new()),
    }
}

pub fn seed_spire(fake: &FakeCluster, domain: &str) {
    let ns = "/apis/apps/v1/namespaces/spire-system";
    fake.insert(
        &format!("{ns}/statefulsets/spire-server"),
        json!({"kind": "StatefulSet", "spec": {"replicas": 1, "template": {"spec": {"containers": [
            {"name": "spire-server", "image": "ghcr.io/spiffe/spire-server:1.15.5"},
            {"name": "spire-controller-manager", "image": "ghcr.io/spiffe/spire-controller-manager:0.6.3"}]}}},
            "status": {"readyReplicas": 1}}),
    );
    fake.insert(
        &format!("{ns}/daemonsets/spire-agent"),
        json!({"kind": "DaemonSet", "status": {"desiredNumberScheduled": 1, "numberReady": 1}}),
    );
    fake.insert(
        "/api/v1/nodes/node-1",
        json!({"kind": "Node", "metadata": {"name": "node-1"}}),
    );
    fake.insert(
        "/apis/storage.k8s.io/v1/csidrivers/csi.spiffe.io",
        json!({"kind": "CSIDriver"}),
    );
    fake.insert(
        "/apis/apiextensions.k8s.io/v1/customresourcedefinitions/clusterspiffeids.spire.spiffe.io",
        json!({"kind": "CustomResourceDefinition"}),
    );
    let cm = "/api/v1/namespaces/spire-system/configmaps";
    fake.insert(
        &format!("{cm}/spire-server"),
        json!({"data": {"server.conf": format!("server {{\n  trust_domain = \"{domain}\"\n}}\n")}}),
    );
    fake.insert(
        &format!("{cm}/spire-agent"),
        json!({"data": {"agent.conf": format!("agent {{\n  trust_domain = \"{domain}\"\n}}\n")}}),
    );
    fake.insert(
        &format!("{cm}/spire-controller-manager"),
        json!({"data": {"controller-manager.yaml": format!("trustDomain: {domain}\nignoreNamespaces:\n  - spire-system\n")}}),
    );
}

/// Marks the Deployment of the OIDC provider as ready.
pub fn oidc_ready(fake: &FakeCluster) {
    let path = "/apis/apps/v1/namespaces/spire-oidc/deployments/spire-oidc-discovery-provider";
    let mut object = fake.object(path).expect("oidc install ran");
    object["status"] = json!({"readyReplicas": 1});
    fake.insert(path, object);
}

impl Env {
    /// Runs one command line (without the program name) with `stdin`.
    pub async fn run(&self, args: &[&str], stdin: &str) -> Run {
        let mut argv = vec!["grauwolf-k8-ops"];
        argv.extend(args);
        let cli = Cli::try_parse_from(argv).expect("arguments parse");
        let mut services = Services::new(
            cli.settings.clone(),
            self.fake.cluster(),
            self.token.clone(),
        );
        services.resolver = grauwolf_k8_ops_app::Resolver::Fixed(self.dns.lock().unwrap().clone());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut input = Cursor::new(stdin.as_bytes().to_vec());
        let code = {
            let mut io = Io::new(&mut out, &mut err, &mut input);
            execute(cli.command, &services, &mut io).await
        };
        Run {
            code,
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
        }
    }

    /// Everything the tool needs in OpenBao and the cluster for `agent add`.
    pub async fn prepare(&self) {
        assert_eq!(self.run(&["oidc", "install"], "").await.code, 0);
        oidc_ready(&self.fake);
        assert_eq!(self.run(&["openbao", "setup"], "").await.code, 0);
    }

    /// From now on every call is checked against the policy `name` as written to OpenBao, like
    /// a token that has only that policy.
    pub fn restrict_to(&self, name: &str) {
        let mut bao = self.bao.lock().unwrap();
        assert!(bao.policies.contains_key(name), "no policy {name}");
        bao.enforce = Some(name.to_owned());
        bao.token_policies = vec![name.to_owned()];
        bao.capabilities = Some(Vec::new());
    }

    pub fn bao_writes(&self) -> usize {
        self.bao.lock().unwrap().writes.len()
    }
}

fn bao_call(state: &mut BaoState, method: &str, path: &str, body: &str) -> (u16, String) {
    let body: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let list = query.contains("list=true");
    let ok = |value: Value| (200, value.to_string());
    let empty = || (204, String::new());
    let missing = || (404, json!({"errors": []}).to_string());
    if let Some(name) = state.enforce.clone() {
        let policy = state.policies.get(&name).cloned().unwrap_or_default();
        if path != "/v1/sys/health" && !policy_allows(&policy, method, path, list) {
            state.denied.push(format!("{method} {path}"));
            return (403, json!({"errors": ["permission denied"]}).to_string());
        }
    }
    if method != "GET" {
        state.writes.push(format!("{method} {path}"));
    }
    let segments: Vec<&str> = path.trim_start_matches("/v1/").split('/').collect();
    match (method, segments.as_slice()) {
        ("GET", ["sys", "health"]) => (
            if state.sealed { 503 } else { 200 },
            json!({"initialized": state.initialized, "sealed": state.sealed, "version": "2.7.2"})
                .to_string(),
        ),
        ("POST", ["sys", "capabilities-self"]) => match &state.capabilities {
            None => (403, json!({"errors": ["permission denied"]}).to_string()),
            Some(caps) => {
                let policy = state
                    .enforce
                    .as_ref()
                    .and_then(|name| state.policies.get(name))
                    .cloned();
                let data: serde_json::Map<String, Value> = body["paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|path| {
                        let path = path.as_str()?;
                        let caps = match &policy {
                            Some(policy) => policy_capabilities(policy, path),
                            None => caps.clone(),
                        };
                        Some((path.to_owned(), json!(caps)))
                    })
                    .collect();
                ok(json!({"data": data}))
            }
        },
        ("GET", ["auth", "token", "lookup-self"]) => {
            ok(json!({"data": {"policies": state.token_policies, "id": "never-printed"}}))
        }
        ("POST", ["auth", "token", "create"]) => {
            state.created_tokens.push(body.clone());
            ok(json!({"auth": {"client_token": "s.restricted-token",
                "policies": body["policies"]}}))
        }
        ("GET", ["sys", "auth"]) => ok(
            json!({"data": state.auth.iter().map(|(m, t)| (format!("{m}/"), json!({"type": t}))).collect::<serde_json::Map<_, _>>()}),
        ),
        ("POST", ["sys", "auth", mount]) => {
            state.auth.insert(
                (*mount).to_owned(),
                body["type"].as_str().unwrap_or("").to_owned(),
            );
            empty()
        }
        ("GET", ["sys", "mounts"]) => ok(
            json!({"data": state.mounts.iter().map(|(m, v)| (format!("{m}/"), v.clone())).collect::<serde_json::Map<_, _>>()}),
        ),
        ("POST", ["sys", "mounts", mount]) => {
            state.mounts.insert((*mount).to_owned(), body);
            empty()
        }
        ("GET", ["auth", mount, "config"]) => match state.jwt_config.get(*mount) {
            Some(config) => ok(json!({"data": config})),
            None => missing(),
        },
        ("POST", ["auth", mount, "config"]) => {
            state.jwt_config.insert((*mount).to_owned(), body);
            empty()
        }
        ("GET", ["auth", _, "role"]) if list => {
            ok(json!({"data": {"keys": state.roles.keys().collect::<Vec<_>>()}}))
        }
        ("GET", ["auth", _, "role", name]) => match state.roles.get(*name) {
            Some(role) => ok(json!({"data": role})),
            None => missing(),
        },
        ("POST", ["auth", _, "role", name]) => {
            state.roles.insert((*name).to_owned(), body);
            empty()
        }
        ("DELETE", ["auth", _, "role", name]) => {
            state.roles.remove(*name);
            empty()
        }
        ("GET", ["sys", "policies", "acl", name]) => match state.policies.get(*name) {
            Some(policy) => ok(json!({"data": {"policy": policy}})),
            None => missing(),
        },
        ("PUT", ["sys", "policies", "acl", name]) => {
            state.policies.insert(
                (*name).to_owned(),
                body["policy"].as_str().unwrap_or("").to_owned(),
            );
            empty()
        }
        ("DELETE", ["sys", "policies", "acl", name]) => {
            state.policies.remove(*name);
            empty()
        }
        ("GET", [mount, "keys", name]) if mount_is(state, mount, "transit") => {
            match state.transit_keys.get(&format!("{mount}/{name}")) {
                Some(kind) => ok(json!({"data": {"type": kind, "latest_version": 1}})),
                None => missing(),
            }
        }
        ("POST", [mount, "keys", name]) if mount_is(state, mount, "transit") => {
            state.transit_keys.insert(
                format!("{mount}/{name}"),
                body["type"].as_str().unwrap_or("aes256-gcm96").to_owned(),
            );
            empty()
        }
        ("POST", [mount, "keys", _, "config"]) if mount_is(state, mount, "transit") => empty(),
        ("DELETE", [mount, "keys", name]) if mount_is(state, mount, "transit") => {
            state.transit_keys.remove(&format!("{mount}/{name}"));
            empty()
        }
        ("POST", [_, "data", ..]) if state.deny_kv_writes => {
            (403, json!({"errors": ["permission denied"]}).to_string())
        }
        ("POST", [mount, "data", rest @ ..]) => {
            let versions = state
                .kv
                .entry(format!("{mount}/{}", rest.join("/")))
                .or_default();
            versions.push(body["data"]["value"].as_str().unwrap_or("").to_owned());
            let version = versions.len();
            state
                .kv_data
                .insert(format!("{mount}/{}", rest.join("/")), body["data"].clone());
            ok(json!({"data": {"version": version}}))
        }
        ("GET", [mount, "data", rest @ ..]) => {
            let key = format!("{mount}/{}", rest.join("/"));
            match (state.kv.get(&key), state.kv_data.get(&key)) {
                (Some(_), Some(data)) if data.get("value").is_none() => {
                    ok(json!({"data": {"data": data}}))
                }
                (Some(versions), _) => ok(json!({"data": {"data": {"value": versions.last()}}})),
                (None, _) => missing(),
            }
        }
        ("GET", [mount, "metadata", rest @ ..]) if list => {
            let prefix = format!("{mount}/{}/", rest.join("/"));
            let mut keys: Vec<String> = state
                .kv
                .keys()
                .filter_map(|key| key.strip_prefix(&prefix))
                .map(|name| match name.split_once('/') {
                    Some((folder, _)) => format!("{folder}/"),
                    None => name.to_owned(),
                })
                .collect();
            keys.dedup();
            if keys.is_empty() {
                missing()
            } else {
                ok(json!({"data": {"keys": keys}}))
            }
        }
        ("GET", [mount, "metadata", rest @ ..]) => {
            match state.kv.get(&format!("{mount}/{}", rest.join("/"))) {
                Some(versions) => ok(json!({"data": {"current_version": versions.len()}})),
                None => missing(),
            }
        }
        ("DELETE", [mount, "metadata", rest @ ..]) => {
            state.kv.remove(&format!("{mount}/{}", rest.join("/")));
            state.kv_data.remove(&format!("{mount}/{}", rest.join("/")));
            empty()
        }
        _ => missing(),
    }
}

fn mount_is(state: &BaoState, mount: &str, kind: &str) -> bool {
    state
        .mounts
        .get(mount)
        .is_some_and(|body| body["type"] == kind)
}

impl Env {
    /// Stores a grant of the identity service as it does: metadata and refresh token apart.
    pub fn store_grant(&self, grant: Value) {
        let id = grant["id"].as_str().unwrap().to_owned();
        let mut bao = self.bao.lock().unwrap();
        bao.kv
            .insert(format!("kv/identity/grants/{id}"), vec![String::new()]);
        bao.kv_data
            .insert(format!("kv/identity/grants/{id}"), grant);
        bao.kv.insert(
            format!("kv/identity/grant-tokens/{id}"),
            vec![String::new()],
        );
        bao.kv_data.insert(
            format!("kv/identity/grant-tokens/{id}"),
            json!({"refresh_token": "rt-secret"}),
        );
    }
}
