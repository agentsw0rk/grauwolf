use std::fmt;
use std::path::Path;

use bytes::Bytes;
use http_body_util::BodyExt;
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Kind;

/// The server-side apply field manager (ADR-0120).
pub const FIELD_MANAGER: &str = "grauwolf-k8-ops";
/// The annotation that holds the hash of the object as this tool last applied it.
pub const APPLIED_HASH: &str = "grauwolf.io/applied-hash";

/// What `ensure` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Updated,
    Unchanged,
}

/// A failed call: the HTTP status (if the API server answered) and a message without bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct K8sError {
    pub status: Option<u16>,
    pub message: String,
}

impl K8sError {
    fn other(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }

    pub fn is_forbidden(&self) -> bool {
        self.status == Some(403)
    }
}

impl fmt::Display for K8sError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for K8sError {}

/// A connection to one cluster.
#[derive(Clone)]
pub struct Cluster {
    client: Client,
    /// The API server's URL, if the connection came from a kubeconfig or the environment.
    server: Option<String>,
    /// The kubeconfig context in use, if there is one (not in-cluster).
    context: Option<String>,
}

impl Cluster {
    /// Connects with the kubeconfig at `kubeconfig`, or (without it) `KUBECONFIG`, the default
    /// location or the in-cluster account. `context` names the kubeconfig context to use
    /// (default: its current context); one that does not exist is an error that lists the
    /// available ones.
    pub async fn connect(
        kubeconfig: Option<&Path>,
        context: Option<&str>,
    ) -> Result<Self, K8sError> {
        // A program that links this crate may link more than one rustls crypto provider (other
        // crates bring aws-lc-rs); rustls then refuses to pick one and panics at the first TLS
        // client. Choose ring, the provider kube is built with here; an already installed
        // provider stays.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (config, context) = if kubeconfig.is_none() && context.is_none() {
            // The default places, or the in-cluster account.
            let current = kube::config::Kubeconfig::read()
                .ok()
                .and_then(|file| file.current_context);
            let config = kube::Config::infer()
                .await
                .map_err(|error| K8sError::other(format!("no usable kubeconfig: {error}")))?;
            (config, current)
        } else {
            let file = match kubeconfig {
                Some(file) => kube::config::Kubeconfig::read_from(file).map_err(|error| {
                    K8sError::other(format!("cannot read the kubeconfig: {error}"))
                })?,
                None => kube::config::Kubeconfig::read()
                    .map_err(|error| K8sError::other(format!("no usable kubeconfig: {error}")))?,
            };
            config_of(file, context).await?
        };
        let server = config.cluster_url.to_string();
        let client = Client::try_from(config)
            .map_err(|error| K8sError::other(format!("cannot create the client: {error}")))?;
        Ok(Self {
            client,
            server: Some(server.trim_end_matches('/').to_owned()),
            context,
        })
    }

    /// Connects with the kubeconfig `text` (e.g. pasted into grauwolf studio, ADR-0129) and
    /// `context` (default: its current one), like [`Cluster::connect`] with a file.
    pub async fn connect_text(text: &str, context: Option<&str>) -> Result<Self, K8sError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let file = kube::config::Kubeconfig::from_yaml(text)
            .map_err(|error| K8sError::other(format!("cannot read the kubeconfig: {error}")))?;
        let (config, context) = config_of(file, context).await?;
        let server = config.cluster_url.to_string();
        let client = Client::try_from(config)
            .map_err(|error| K8sError::other(format!("cannot create the client: {error}")))?;
        Ok(Self {
            client,
            server: Some(server.trim_end_matches('/').to_owned()),
            context,
        })
    }

    /// The API server's URL, if known.
    pub fn server(&self) -> Option<&str> {
        self.server.as_deref()
    }

    /// The kubeconfig context in use, if there is one.
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// `https://host:6443 (context home)`: which cluster this is, for the first line of a report.
    pub fn describe(&self) -> String {
        match (&self.server, &self.context) {
            (Some(server), Some(context)) => format!("{server} (context {context})"),
            (Some(server), None) => server.clone(),
            (None, Some(context)) => format!("context {context}"),
            (None, None) => "unknown cluster".to_owned(),
        }
    }

    /// The same connection, described as `server` and `context` (tests with a fake cluster).
    pub fn described_as(mut self, server: &str, context: Option<&str>) -> Self {
        self.server = Some(server.to_owned());
        self.context = context.map(str::to_owned);
        self
    }

    /// A cluster behind any client (tests use [`crate::testing::FakeCluster`]).
    pub fn from_client(client: Client) -> Self {
        Self {
            client,
            server: None,
            context: None,
        }
    }

    /// Any request through the API server; an HTTP error status is a normal answer.
    pub async fn raw(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<(u16, String), K8sError> {
        let mut builder = http::Request::builder().method(method).uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Body::from(body.unwrap_or_default()))
            .map_err(|error| K8sError::other(format!("invalid request: {error}")))?;
        let response = self.client.send(request).await.map_err(|error| {
            K8sError::other(format!("the API server is not reachable: {error}"))
        })?;
        let status = response.status().as_u16();
        let bytes: Bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|error| K8sError::other(format!("cannot read the answer: {error}")))?
            .to_bytes();
        Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<(u16, String), K8sError> {
        let (status, text) = self.raw(method, path, headers, body).await?;
        if status / 100 == 2 || status == 404 {
            return Ok((status, text));
        }
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|body| {
                body.get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        Err(K8sError {
            status: Some(status),
            message: format!("{method} {path}: HTTP {status}: {message}"),
        })
    }

    /// The Kubernetes version of the server (`v1.34.1+k3s1`).
    pub async fn version(&self) -> Result<String, K8sError> {
        let (_, text) = self.call("GET", "/version", &[], None).await?;
        serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|body| body["gitVersion"].as_str().map(str::to_owned))
            .ok_or_else(|| K8sError::other("the version answer is not understood"))
    }

    /// The object, `None` if it does not exist.
    pub async fn get(
        &self,
        kind: &Kind,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<Option<Value>, K8sError> {
        let (status, text) = self
            .call("GET", &kind.object(namespace, name), &[], None)
            .await?;
        if status == 404 {
            return Ok(None);
        }
        parse(&text).map(Some)
    }

    /// The objects of a kind (in `namespace`, or everywhere for `None`) matching `selector`
    /// (`a=b,c`).
    pub async fn list(
        &self,
        kind: &Kind,
        namespace: Option<&str>,
        selector: Option<&str>,
    ) -> Result<Vec<Value>, K8sError> {
        let mut path = kind.collection(namespace);
        if let Some(selector) = selector {
            path.push_str(&format!("?labelSelector={}", encode(selector)));
        }
        let (status, text) = self.call("GET", &path, &[], None).await?;
        if status == 404 {
            return Ok(Vec::new());
        }
        Ok(parse(&text)?["items"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Applies `desired` (server-side, forced, field manager [`FIELD_MANAGER`]) unless this tool
    /// applied exactly this object before.
    pub async fn ensure(
        &self,
        kind: &Kind,
        namespace: Option<&str>,
        desired: &Value,
    ) -> Result<Outcome, K8sError> {
        let name = desired
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .ok_or_else(|| K8sError::other("the object has no name"))?;
        let hash = hash_of(desired);
        let existing = self.get(kind, namespace, name).await?;
        let applied = existing
            .as_ref()
            .and_then(|object| object.pointer("/metadata/annotations"))
            .and_then(|annotations| annotations.get(APPLIED_HASH))
            .and_then(Value::as_str);
        if applied == Some(hash.as_str()) {
            return Ok(Outcome::Unchanged);
        }
        let mut object = desired.clone();
        object["metadata"]["annotations"][APPLIED_HASH] = json!(hash);
        let path = format!(
            "{}?fieldManager={FIELD_MANAGER}&force=true",
            kind.object(namespace, name)
        );
        self.call(
            "PATCH",
            &path,
            &[("content-type", "application/apply-patch+yaml")],
            Some(object.to_string().into_bytes()),
        )
        .await?;
        Ok(if existing.is_some() {
            Outcome::Updated
        } else {
            Outcome::Created
        })
    }

    /// A JSON merge patch (`null` removes a field) of an existing object, for small changes such
    /// as the replica count or a restart annotation; no ownership is taken. A missing object is
    /// an error (404), never created.
    pub async fn patch(
        &self,
        kind: &Kind,
        namespace: Option<&str>,
        name: &str,
        patch: &Value,
    ) -> Result<(), K8sError> {
        let path = kind.object(namespace, name);
        let (status, _) = self
            .call(
                "PATCH",
                &path,
                &[("content-type", "application/merge-patch+json")],
                Some(patch.to_string().into_bytes()),
            )
            .await?;
        if status == 404 {
            return Err(K8sError {
                status: Some(404),
                message: format!("PATCH {path}: HTTP 404: not found"),
            });
        }
        Ok(())
    }

    /// Deletes the object (propagation `Background`); `false` if there was none.
    pub async fn delete(
        &self,
        kind: &Kind,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<bool, K8sError> {
        let (status, _) = self
            .call(
                "DELETE",
                &format!(
                    "{}?propagationPolicy=Background",
                    kind.object(namespace, name)
                ),
                &[],
                None,
            )
            .await?;
        Ok(status != 404)
    }

    /// The last `lines` lines of the log of a pod.
    pub async fn pod_log(
        &self,
        namespace: &str,
        pod: &str,
        lines: u32,
    ) -> Result<String, K8sError> {
        let path = format!("/api/v1/namespaces/{namespace}/pods/{pod}/log?tailLines={lines}");
        let (status, text) = self.call("GET", &path, &[], None).await?;
        if status == 404 {
            return Err(K8sError {
                status: Some(404),
                message: format!("pod {namespace}/{pod} has no log"),
            });
        }
        Ok(text)
    }

    /// The log of the previous run of the pod's container (after a restart): the last `lines`
    /// lines; empty if there was none.
    pub async fn pod_log_previous(
        &self,
        namespace: &str,
        pod: &str,
        lines: u32,
    ) -> Result<String, K8sError> {
        let path = format!(
            "/api/v1/namespaces/{namespace}/pods/{pod}/log?tailLines={lines}&previous=true"
        );
        let (status, text) = self.raw("GET", &path, &[], None).await?;
        // The API server answers 400 when there was no previous run.
        Ok(if status / 100 == 2 {
            text
        } else {
            String::new()
        })
    }

    /// Streams the log of a pod (the last `tail` lines, then new ones with `follow`) to `sink`,
    /// one call per chunk of complete lines, until the log ends (or, with `follow`, the pod
    /// does or the connection breaks).
    pub async fn pod_log_stream(
        &self,
        namespace: &str,
        pod: &str,
        tail: u32,
        follow: bool,
        mut sink: impl FnMut(&str),
    ) -> Result<(), K8sError> {
        use http_body_util::BodyExt;
        let path = format!(
            "/api/v1/namespaces/{namespace}/pods/{pod}/log?tailLines={tail}&follow={follow}"
        );
        let request = http::Request::builder()
            .method("GET")
            .uri(&path)
            .body(Body::empty())
            .map_err(|error| K8sError::other(format!("invalid request: {error}")))?;
        let response = self.client.send(request).await.map_err(|error| {
            K8sError::other(format!("the API server is not reachable: {error}"))
        })?;
        let status = response.status().as_u16();
        let mut body = response.into_body();
        if status / 100 != 2 {
            let text = body
                .collect()
                .await
                .map(|collected| String::from_utf8_lossy(&collected.to_bytes()).into_owned())
                .unwrap_or_default();
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|body| body["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(K8sError {
                status: Some(status),
                message: format!("pod {namespace}/{pod} log: HTTP {status}: {message}"),
            });
        }
        let mut pending: Vec<u8> = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame =
                frame.map_err(|error| K8sError::other(format!("cannot read the log: {error}")))?;
            if let Some(data) = frame.data_ref() {
                pending.extend_from_slice(data);
                // Only complete lines go out, so that a character is never cut in two.
                if let Some(end) = pending.iter().rposition(|byte| *byte == b'\n') {
                    let complete: Vec<u8> = pending.drain(..=end).collect();
                    sink(&String::from_utf8_lossy(&complete));
                }
            }
        }
        if !pending.is_empty() {
            sink(&String::from_utf8_lossy(&pending));
        }
        Ok(())
    }

    /// SelfSubjectAccessReview: may the kubeconfig's identity do `verb` on `resource`
    /// (`pods/log` = resource with subresource) of `group` in `namespace` (`None`: cluster-wide)?
    pub async fn can_i(
        &self,
        verb: &str,
        group: &str,
        resource: &str,
        namespace: Option<&str>,
    ) -> Result<bool, K8sError> {
        let (resource, subresource) = match resource.split_once('/') {
            Some((resource, subresource)) => (resource, Some(subresource)),
            None => (resource, None),
        };
        let mut attributes = json!({ "verb": verb, "group": group, "resource": resource });
        if let Some(namespace) = namespace {
            attributes["namespace"] = json!(namespace);
        }
        if let Some(subresource) = subresource {
            attributes["subresource"] = json!(subresource);
        }
        let review = json!({
            "apiVersion": "authorization.k8s.io/v1",
            "kind": "SelfSubjectAccessReview",
            "spec": { "resourceAttributes": attributes },
        });
        let (_, text) = self
            .call(
                "POST",
                "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews",
                &[("content-type", "application/json")],
                Some(review.to_string().into_bytes()),
            )
            .await?;
        Ok(parse(&text)?
            .pointer("/status/allowed")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }
}

/// The client configuration of `file` with `context` (default: its current one) and the context
/// used; a context the file does not have is an error listing the available ones.
async fn config_of(
    file: kube::config::Kubeconfig,
    context: Option<&str>,
) -> Result<(kube::Config, Option<String>), K8sError> {
    if let Some(name) = context
        && !file.contexts.iter().any(|known| known.name == name)
    {
        let available: Vec<&str> = file
            .contexts
            .iter()
            .map(|known| known.name.as_str())
            .collect();
        return Err(K8sError::other(format!(
            "context '{name}' not found in the kubeconfig (available: {})",
            if available.is_empty() {
                "none".to_owned()
            } else {
                available.join(", ")
            }
        )));
    }
    let used = context
        .map(str::to_owned)
        .or_else(|| file.current_context.clone());
    let options = kube::config::KubeConfigOptions {
        context: context.map(str::to_owned),
        ..Default::default()
    };
    let config = kube::Config::from_custom_kubeconfig(file, &options)
        .await
        .map_err(|error| K8sError::other(format!("invalid kubeconfig: {error}")))?;
    Ok((config, used))
}

fn parse(text: &str) -> Result<Value, K8sError> {
    serde_json::from_str(text).map_err(|_| K8sError::other("the API server's answer is not JSON"))
}

/// The hash of an object as a short hex string.
pub fn hash_of(object: &Value) -> String {
    let digest = Sha256::digest(object.to_string().as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Percent-encoding of a query value (label selectors hold `=`, `,`, `!`, spaces).
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
