//! An in-memory Kubernetes API server for tests: objects are stored by URL path, server-side
//! apply is a deep merge, `DELETE` of a namespace removes what is in it.

use std::collections::BTreeMap;
use std::future::{Future, Ready, ready};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use http::{Request, Response};
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};
use tower::Service;

use crate::Cluster;

/// What happens to a Job after it is created.
#[derive(Debug, Clone)]
pub enum JobOutcome {
    /// It never finishes (a timeout).
    Never,
    /// One pod runs and has `log`, but the Job does not finish (a build that is still running).
    Running(String),
    /// One pod runs, `log` is its log, the Job succeeds.
    Succeeds(String),
    /// One pod runs, `log` is its log, the Job fails.
    Fails(String),
}

/// What happens to a Deployment after it is applied (a pod `<name>-x1` with the labels of the
/// pod template appears and the status of the Deployment follows).
#[derive(Debug, Clone)]
pub enum DeploymentOutcome {
    /// No controller: nothing happens.
    Never,
    /// The pod runs and is ready; `log` is its log.
    Ready(String),
    /// The pod runs but never becomes ready.
    Starting(String),
    /// The container crashes and is restarted (waiting reason `CrashLoopBackOff`); `log` is the
    /// log of the last run.
    CrashLoop(String),
}

type ChangeHook = dyn Fn(&FakeCluster, &str, &str) + Send + Sync;
type ProxyHook = dyn Fn(&str, &str, &str, &str) -> (u16, String) + Send + Sync;

struct State {
    objects: BTreeMap<String, Value>,
    logs: BTreeMap<String, String>,
    version: u64,
    writes: Vec<String>,
    deleted: Vec<String>,
    jobs: Vec<Value>,
    denied: Vec<(String, String)>,
    job_outcome: JobOutcome,
    deployment_outcome: DeploymentOutcome,
    proxy: Option<Arc<ProxyHook>>,
    change_hooks: Vec<Arc<ChangeHook>>,
}

/// The fake API server. Clone it freely; all clones share the state.
#[derive(Clone)]
pub struct FakeCluster {
    state: Arc<Mutex<State>>,
}

impl Default for FakeCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeCluster {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                objects: BTreeMap::new(),
                logs: BTreeMap::new(),
                version: 0,
                writes: Vec::new(),
                deleted: Vec::new(),
                jobs: Vec::new(),
                denied: Vec::new(),
                job_outcome: JobOutcome::Never,
                deployment_outcome: DeploymentOutcome::Never,
                proxy: None,
                change_hooks: Vec::new(),
            })),
        }
    }

    /// A `Cluster` that talks to this fake.
    pub fn cluster(&self) -> Cluster {
        Cluster::from_client(Client::new(self.clone(), "default"))
            .described_as("https://fake-cluster.test:6443", Some("fake-context"))
    }

    /// Puts an object at `path` (as `GET path` returns it).
    pub fn insert(&self, path: &str, object: Value) {
        self.state
            .lock()
            .unwrap()
            .objects
            .insert(path.to_owned(), object);
    }

    /// Removes the object at `path` (no cascade, no hooks).
    pub fn remove(&self, path: &str) {
        self.state.lock().unwrap().objects.remove(path);
    }

    /// Calls `hook(fake, method, path)` after every successful `PATCH` and `DELETE`, outside the
    /// state lock, so the hook may read and change the fake: a test plays the controllers (a pod
    /// goes away when a StatefulSet is scaled to 0, a ConfigMap appears).
    pub fn on_change(&self, hook: impl Fn(&FakeCluster, &str, &str) + Send + Sync + 'static) {
        self.state.lock().unwrap().change_hooks.push(Arc::new(hook));
    }

    /// The object at `path`.
    pub fn object(&self, path: &str) -> Option<Value> {
        self.state.lock().unwrap().objects.get(path).cloned()
    }

    /// The paths of all objects whose path starts with `prefix`.
    pub fn paths(&self, prefix: &str) -> Vec<String> {
        let state = self.state.lock().unwrap();
        state
            .objects
            .keys()
            .filter(|path| path.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// `PATCH path` of every write that changed something, in order.
    pub fn writes(&self) -> Vec<String> {
        self.state.lock().unwrap().writes.clone()
    }

    /// The paths of the objects deleted by `DELETE` requests, in order.
    pub fn deleted(&self) -> Vec<String> {
        self.state.lock().unwrap().deleted.clone()
    }

    /// Every Job that was created, in order (a finished Job is deleted by its caller).
    pub fn created_jobs(&self) -> Vec<Value> {
        self.state.lock().unwrap().jobs.clone()
    }

    /// The access reviews for `verb` on `resource` (`pods/log`) answer `allowed: false`.
    pub fn deny(&self, verb: &str, resource: &str) {
        self.state
            .lock()
            .unwrap()
            .denied
            .push((verb.to_owned(), resource.to_owned()));
    }

    pub fn set_job_outcome(&self, outcome: JobOutcome) {
        self.state.lock().unwrap().job_outcome = outcome;
    }

    pub fn set_deployment_outcome(&self, outcome: DeploymentOutcome) {
        self.state.lock().unwrap().deployment_outcome = outcome;
    }

    /// Answers requests below `.../services/<service>/proxy/` with
    /// `hook(method, service, path, body)` (`service` as in the URL: `http:openbao:8200`).
    pub fn on_proxy(
        &self,
        hook: impl Fn(&str, &str, &str, &str) -> (u16, String) + Send + Sync + 'static,
    ) {
        self.state.lock().unwrap().proxy = Some(Arc::new(hook));
    }

    fn handle(
        &self,
        method: &str,
        uri: &http::Uri,
        merge_patch: bool,
        body: &[u8],
    ) -> (u16, String) {
        let path = uri.path().to_owned();
        let query = uri.query().unwrap_or("").to_owned();
        let mut state = self.state.lock().unwrap();
        if path == "/version" {
            return ok(json!({"gitVersion": "v1.34.1+fake"}));
        }
        if path == "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews" && method == "POST" {
            let review: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            let attributes = &review["spec"]["resourceAttributes"];
            let mut resource = attributes["resource"].as_str().unwrap_or("").to_owned();
            if let Some(sub) = attributes["subresource"].as_str() {
                resource = format!("{resource}/{sub}");
            }
            let verb = attributes["verb"].as_str().unwrap_or("");
            let denied = state
                .denied
                .iter()
                .any(|(v, r)| v == verb && *r == resource);
            return ok(json!({"status": {"allowed": !denied}}));
        }
        if let Some(position) = path.find("/proxy/") {
            let hook = state.proxy.clone();
            drop(state);
            let service = path[..position].rsplit('/').next().unwrap_or("").to_owned();
            let rest = format!("/{}", &path[position + "/proxy/".len()..]);
            let rest = if query.is_empty() {
                rest
            } else {
                format!("{rest}?{query}")
            };
            return match hook {
                Some(hook) => hook(method, &service, &rest, &String::from_utf8_lossy(body)),
                None => not_found(),
            };
        }
        if let Some(pod) = path.strip_suffix("/log") {
            return match state.logs.get(pod) {
                Some(log) => (200, log.clone()),
                None => not_found(),
            };
        }
        let Some(shape) = Shape::of(&path) else {
            return not_found();
        };
        match (method, shape) {
            ("GET", Shape::Collection { prefix }) => {
                let selector = query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("labelSelector="))
                    .map(decode)
                    .unwrap_or_default();
                let items: Vec<Value> = state
                    .objects
                    .iter()
                    .filter(|(key, _)| in_collection(key, &prefix))
                    .filter(|(_, object)| matches_selector(object, &selector))
                    .map(|(_, object)| object.clone())
                    .collect();
                ok(json!({"items": items}))
            }
            ("GET", Shape::Object) => match state.objects.get(&path) {
                Some(object) => ok(object.clone()),
                None => not_found(),
            },
            ("PATCH", Shape::Object) if merge_patch => {
                let Some(existing) = state.objects.get(&path) else {
                    return not_found();
                };
                let patch: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                let merged = merge(existing, &patch);
                if &merged != existing {
                    state.writes.push(format!("PATCH {path}"));
                    state.version += 1;
                    state.objects.insert(path.clone(), merged.clone());
                }
                ok(merged)
            }
            ("PATCH", Shape::Object) => {
                let mut desired: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                let name = path.rsplit('/').next().unwrap_or("").to_owned();
                desired["metadata"]["name"] = json!(name);
                if let Some(namespace) = namespace_of(&path) {
                    desired["metadata"]["namespace"] = json!(namespace);
                }
                let merged = match state.objects.get(&path) {
                    Some(existing) => merge(existing, &desired),
                    None => desired,
                };
                if state.objects.get(&path) != Some(&merged) {
                    state.writes.push(format!("PATCH {path}"));
                    state.version += 1;
                    state.objects.insert(path.clone(), merged.clone());
                    if merged["kind"] == "Job" {
                        state.jobs.push(merged.clone());
                        simulate_job(&mut state, &path, &merged);
                    }
                    if merged["kind"] == "Deployment" {
                        simulate_deployment(&mut state, &path, &merged);
                    }
                }
                ok(merged)
            }
            ("DELETE", Shape::Object) => {
                if state.objects.remove(&path).is_none() {
                    return not_found();
                }
                state.deleted.push(path.clone());
                let inside = format!("{path}/");
                // Deleting a namespace deletes what is in it.
                if is_namespace(&path) {
                    let namespace = path.rsplit('/').next().unwrap_or("");
                    let marker = format!("/namespaces/{namespace}/");
                    state.objects.retain(|key, _| !key.contains(&marker));
                }
                state.objects.retain(|key, _| !key.starts_with(&inside));
                ok(json!({"kind": "Status", "status": "Success"}))
            }
            _ => (405, status_body(405, "method not allowed")),
        }
    }
}

fn simulate_job(state: &mut State, path: &str, job: &Value) {
    let (log, field) = match state.job_outcome.clone() {
        JobOutcome::Never => return,
        JobOutcome::Running(log) => (log, ""),
        JobOutcome::Succeeds(log) => (log, "succeeded"),
        JobOutcome::Fails(log) => (log, "failed"),
    };
    let name = job["metadata"]["name"].as_str().unwrap_or("job").to_owned();
    let namespace = namespace_of(path).unwrap_or("default").to_owned();
    let pod_path = format!("/api/v1/namespaces/{namespace}/pods/{name}-x1");
    state.objects.insert(
        pod_path.clone(),
        json!({"kind": "Pod", "metadata": {"name": format!("{name}-x1"), "namespace": namespace,
            "labels": {"job-name": name}}}),
    );
    state.logs.insert(pod_path, log);
    if !field.is_empty() {
        state.objects.get_mut(path).unwrap()["status"] = json!({ field: 1 });
    }
}

/// A Deployment controller in miniature: the generation grows with every change, one pod follows
/// the template, the status says whether it is ready (see [`DeploymentOutcome`]).
fn simulate_deployment(state: &mut State, path: &str, deployment: &Value) {
    let generation = deployment["metadata"]["generation"].as_u64().unwrap_or(0) + 1;
    let outcome = state.deployment_outcome.clone();
    let object = state.objects.get_mut(path).unwrap();
    object["metadata"]["generation"] = json!(generation);
    let (log, ready, waiting) = match outcome {
        DeploymentOutcome::Never => return,
        DeploymentOutcome::Ready(log) => (log, true, None),
        DeploymentOutcome::Starting(log) => (log, false, None),
        DeploymentOutcome::CrashLoop(log) => (log, false, Some("CrashLoopBackOff")),
    };
    object["status"] = json!({"observedGeneration": generation, "replicas": 1,
        "updatedReplicas": 1, "readyReplicas": u32::from(ready),
        "availableReplicas": u32::from(ready)});
    let name = deployment["metadata"]["name"].as_str().unwrap_or("agent");
    let namespace = namespace_of(path).unwrap_or("default").to_owned();
    let container = &deployment["spec"]["template"]["spec"]["containers"][0];
    let state_field = match waiting {
        Some(reason) => json!({"waiting": {"reason": reason}}),
        None => json!({"running": {}}),
    };
    let pod_path = format!("/api/v1/namespaces/{namespace}/pods/{name}-x1");
    // The old pod goes away with the old template.
    let selector = deployment["spec"]["template"]["metadata"]["labels"].clone();
    state.objects.insert(
        pod_path.clone(),
        json!({"kind": "Pod", "metadata": {"name": format!("{name}-x1"), "namespace": namespace,
            "labels": selector},
            "status": {"phase": "Running", "containerStatuses": [{
                "name": container["name"], "image": container["image"], "ready": ready,
                "restartCount": if waiting.is_some() { 3 } else { 0 }, "state": state_field}]}}),
    );
    state.logs.insert(pod_path, log);
}

enum Shape {
    Collection { prefix: String },
    Object,
}

impl Shape {
    /// Collection or object, from the URL path layout of the Kubernetes API.
    fn of(path: &str) -> Option<Shape> {
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        let rest = match segments.as_slice() {
            ["api", _, rest @ ..] => rest,
            ["apis", _, _, rest @ ..] => rest,
            _ => return None,
        };
        let object = match rest {
            ["namespaces", _, _, _] | ["namespaces", _] | [_, _] => true,
            ["namespaces", _, _] | [_] => false,
            _ => return None,
        };
        Some(if object {
            Shape::Object
        } else {
            Shape::Collection {
                prefix: path.trim_end_matches('/').to_owned(),
            }
        })
    }
}

/// `key` is an object directly in the collection `prefix` (not in a deeper one).
/// `key` is an object of the collection `prefix`; a collection without a namespace
/// (`/api/v1/services`) also holds the objects of every namespace.
fn in_collection(key: &str, prefix: &str) -> bool {
    let direct = key
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|name| !name.contains('/'));
    direct || in_every_namespace(key, prefix)
}

fn in_every_namespace(key: &str, prefix: &str) -> bool {
    let Some((base, plural)) = prefix.rsplit_once('/') else {
        return false;
    };
    let Some(rest) = key
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix("/namespaces/"))
    else {
        return false;
    };
    let mut parts = rest.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(namespace), Some(kind), Some(name), None)
            if !namespace.is_empty() && kind == plural && !name.is_empty()
    )
}

fn is_namespace(path: &str) -> bool {
    path.starts_with("/api/v1/namespaces/") && path.matches('/').count() == 4
}

/// The namespace of an object in one (the Namespace object itself has none).
fn namespace_of(path: &str) -> Option<&str> {
    let (_, rest) = path.split_once("/namespaces/")?;
    let (namespace, tail) = rest.split_once('/')?;
    (!tail.is_empty()).then_some(namespace)
}

fn matches_selector(object: &Value, selector: &str) -> bool {
    selector
        .split(',')
        .filter(|term| !term.is_empty())
        .all(|term| {
            let labels = &object["metadata"]["labels"];
            match term.split_once('=') {
                Some((key, value)) => labels[key].as_str() == Some(value),
                None => labels.get(term).is_some(),
            }
        })
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Server-side apply in miniature: objects merge by key, everything else is replaced.
fn merge(existing: &Value, desired: &Value) -> Value {
    match (existing, desired) {
        (Value::Object(old), Value::Object(new)) => {
            let mut merged = old.clone();
            for (key, value) in new {
                if value.is_null() {
                    merged.remove(key);
                    continue;
                }
                let next = match old.get(key) {
                    Some(previous) => merge(previous, value),
                    None => value.clone(),
                };
                merged.insert(key.clone(), next);
            }
            Value::Object(merged)
        }
        _ => desired.clone(),
    }
}

fn ok(body: Value) -> (u16, String) {
    (200, body.to_string())
}

fn not_found() -> (u16, String) {
    (404, status_body(404, "not found"))
}

fn status_body(code: u16, message: &str) -> String {
    json!({"kind": "Status", "apiVersion": "v1", "status": "Failure", "message": message,
        "reason": if code == 404 { "NotFound" } else { "Failure" }, "code": code})
    .to_string()
}

impl Service<Request<Body>> for FakeCluster {
    type Response = Response<Body>;
    type Error = std::convert::Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let (parts, body) = request.into_parts();
        // The bodies the client sends are in memory; collect them synchronously.
        let bytes = futures_body(body);
        let merge_patch = parts
            .headers
            .get("content-type")
            .is_some_and(|value| value == "application/merge-patch+json");
        let (status, text) = self.handle(parts.method.as_str(), &parts.uri, merge_patch, &bytes);
        if status / 100 == 2 && matches!(parts.method.as_str(), "PATCH" | "DELETE") {
            let hooks = self.state.lock().unwrap().change_hooks.clone();
            for hook in hooks {
                hook(self, parts.method.as_str(), parts.uri.path());
            }
        }
        let response = Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(text.into_bytes()))
            .unwrap();
        ready(Ok(response))
    }
}

/// The bytes of a request body that is already complete in memory.
fn futures_body(body: Body) -> Vec<u8> {
    use http_body_util::BodyExt;
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut collect = Box::pin(body.collect());
    match collect.as_mut().poll(&mut context) {
        Poll::Ready(Ok(collected)) => collected.to_bytes().to_vec(),
        _ => Vec::new(),
    }
}
