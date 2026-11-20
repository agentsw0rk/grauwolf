use std::future::Future;
use std::time::Duration;

use serde_json::Value;

use crate::BaoError;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// One request: `path` starts with `/v1/` and may carry a query.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub token: Option<String>,
    pub body: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Sends requests to one OpenBao server. Errors are transport errors only; an HTTP error status is
/// a normal `Response`.
pub trait Transport: Send + Sync {
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, BaoError>> + Send;
}

/// Direct HTTP to an address such as `http://openbao.openbao.svc:8200`.
pub struct HttpTransport {
    http: reqwest::Client,
    address: String,
}

impl HttpTransport {
    pub fn new(address: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            address: address.trim_end_matches('/').to_owned(),
        }
    }
}

impl Transport for HttpTransport {
    async fn send(&self, request: Request) -> Result<Response, BaoError> {
        let url = format!("{}{}", self.address, request.path);
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut builder = self.http.request(method, url);
        if let Some(token) = &request.token {
            builder = builder.header("X-Vault-Token", token);
        }
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| BaoError::other(format!("request failed: {}", without_url(&error))))?;
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        Ok(Response { status, body })
    }
}

/// The reason of a transport error without the URL.
fn without_url(error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "cannot connect"
    } else {
        "transport error"
    };
    kind.to_owned()
}
