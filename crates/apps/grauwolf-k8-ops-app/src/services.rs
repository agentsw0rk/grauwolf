use grauwolf_k8s::Cluster;
use grauwolf_openbao::{Bao, BaoError, HttpTransport, Request, Response, Transport};

use crate::cli::Settings;

/// Reaches OpenBao through the Kubernetes API's service proxy (ADR-0120 decision 3).
pub struct ProxyTransport {
    cluster: Cluster,
    namespace: String,
    service: String,
}

impl ProxyTransport {
    pub fn new(cluster: Cluster, namespace: &str, name: &str, port: &str) -> Self {
        Self {
            cluster,
            namespace: namespace.to_owned(),
            service: format!("http:{name}:{port}"),
        }
    }
}

impl Transport for ProxyTransport {
    async fn send(&self, request: Request) -> Result<Response, BaoError> {
        let path = format!(
            "/api/v1/namespaces/{}/services/{}/proxy{}",
            self.namespace, self.service, request.path
        );
        let mut headers = vec![("content-type", "application/json")];
        if let Some(token) = request.token.as_deref() {
            headers.push(("x-vault-token", token));
        }
        let body = request.body.map(|body| body.to_string().into_bytes());
        let method = request.method.as_str();
        let (status, body) = self
            .cluster
            .raw(method, &path, &headers, body)
            .await
            .map_err(|error| BaoError {
                status: None,
                message: format!("OpenBao is not reachable through the API server: {error}"),
            })?;
        // The API server answers its own errors (no such service, no endpoints) as a Status;
        // OpenBao's own errors carry an `errors` list.
        if matches!(status, 404 | 502 | 503)
            && body.contains("\"kind\":\"Status\"")
            && !body.contains("\"errors\"")
        {
            return Err(BaoError {
                status: Some(status),
                message: format!("the API server cannot reach the OpenBao service: HTTP {status}"),
            });
        }
        Ok(Response { status, body })
    }
}

/// Direct HTTP or the service proxy.
pub enum AnyTransport {
    Http(HttpTransport),
    Proxy(ProxyTransport),
}

impl Transport for AnyTransport {
    async fn send(&self, request: Request) -> Result<Response, BaoError> {
        match self {
            AnyTransport::Http(transport) => transport.send(request).await,
            AnyTransport::Proxy(transport) => transport.send(request).await,
        }
    }
}

/// What the commands work with: the cluster and OpenBao.
pub struct Services {
    pub settings: Settings,
    pub cluster: Cluster,
    pub bao: Bao<AnyTransport>,
    pub has_token: bool,
    /// Resolves host names for the egress policies (tests use a fixed table).
    pub resolver: crate::egress::Resolver,
}

impl Services {
    /// Connects as `settings` say: the kubeconfig, the token (`--token-file` or `OPENBAO_TOKEN`),
    /// and OpenBao directly or through the API server.
    pub async fn connect(settings: Settings) -> Result<Self, String> {
        let cluster = Cluster::connect(settings.kubeconfig.as_deref(), settings.context.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        let token = match &settings.token_file {
            Some(file) => Some(
                std::fs::read_to_string(file)
                    .map_err(|error| {
                        format!("cannot read the token file {}: {error}", file.display())
                    })?
                    .trim()
                    .to_owned(),
            ),
            None => std::env::var("OPENBAO_TOKEN").ok(),
        };
        Ok(Self::new(
            settings,
            cluster,
            token.filter(|token| !token.is_empty()),
        ))
    }

    /// The services for an existing cluster connection (tests use the fake cluster).
    pub fn new(settings: Settings, cluster: Cluster, token: Option<String>) -> Self {
        let transport = match &settings.openbao_addr {
            Some(address) => AnyTransport::Http(HttpTransport::new(address)),
            None => {
                let (name, port) = settings.openbao_service_parts();
                AnyTransport::Proxy(ProxyTransport::new(
                    cluster.clone(),
                    &settings.openbao_namespace,
                    name,
                    port,
                ))
            }
        };
        let has_token = token.is_some();
        let bao = Bao::new(transport);
        let bao = match token {
            Some(token) => bao.with_token(token),
            None => bao,
        };
        Self {
            settings,
            cluster,
            bao,
            has_token,
            resolver: crate::egress::Resolver::default(),
        }
    }

    /// The OpenBao client, or the reason why admin calls are not possible.
    pub fn admin(&self) -> Result<&Bao<AnyTransport>, String> {
        if self.has_token {
            Ok(&self.bao)
        } else {
            Err("no OpenBao token: set OPENBAO_TOKEN or pass --token-file".to_owned())
        }
    }
}
