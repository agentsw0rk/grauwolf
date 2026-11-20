//! A minimal Workload API client: gRPC over HTTP/2 on the agent's Unix socket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use futures_util::future::BoxFuture;
use grauwolf_config::Env;
use prost::Message;
use tokio::net::UnixStream;

use crate::proto::{
    JwtSvidRequest, JwtSvidResponse, ValidateJwtSvidRequest, ValidateJwtSvidResponse,
};
use crate::{JwtSvid, SpiffeError, SpiffeId, WorkloadIdentity};

/// The variable that names the Workload API socket.
pub const ENDPOINT_VARIABLE: &str = "SPIFFE_ENDPOINT_SOCKET";

const FETCH_JWT_SVID: &str = "/SpiffeWorkloadAPI/FetchJWTSVID";
const VALIDATE_JWT_SVID: &str = "/SpiffeWorkloadAPI/ValidateJWTSVID";
pub(crate) const FETCH_X509_SVID: &str = "/SpiffeWorkloadAPI/FetchX509SVID";
pub(crate) const FETCH_X509_BUNDLES: &str = "/SpiffeWorkloadAPI/FetchX509Bundles";
const TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const FIRST_BACKOFF: Duration = Duration::from_millis(200);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// The variable that sets how long to wait for the identity at start-up (`30s`, `1m`, `0s`).
pub const FETCH_TIMEOUT_VARIABLE: &str = "SPIFFE_FETCH_TIMEOUT";

/// `250ms`, `30s`, `5m`, `1h`; `0s` is allowed (no retry).
fn parse_duration(text: &str) -> Option<Duration> {
    let (number, unit) = match text.strip_suffix("ms") {
        Some(number) => (number, "ms"),
        None => text.split_at(text.len().checked_sub(1)?),
    };
    let number: u64 = number.parse().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(number)),
        "s" => Some(Duration::from_secs(number)),
        "m" => Some(Duration::from_secs(number * 60)),
        "h" => Some(Duration::from_secs(number * 3600)),
        _ => None,
    }
}

/// An error that may go away: the identity is not issued yet (`PERMISSION_DENIED`), the API is
/// not ready (`UNAVAILABLE`) or cannot be reached.
pub(crate) fn is_transient(error: &SpiffeError) -> bool {
    matches!(
        error,
        SpiffeError::Unavailable { .. } | SpiffeError::Refused { status: 7 | 14, .. }
    )
}

pub(crate) fn describe(duration: Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

/// The Workload API at a Unix socket. It connects per call, so a restarted SPIRE agent needs no
/// special handling.
#[derive(Debug, Clone)]
pub struct WorkloadApiClient {
    socket: PathBuf,
    fetch_timeout: Duration,
    first_backoff: Duration,
}

impl WorkloadApiClient {
    /// A client for the socket at `socket`.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
            first_backoff: FIRST_BACKOFF,
        }
    }

    /// How long `jwt_svid` keeps trying while the identity is not available yet (the pod's
    /// registration entry is not created or synced, the socket does not exist or refuses
    /// connections). `Duration::ZERO` tries once. Default 30 seconds.
    pub fn with_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    /// The first pause between attempts (doubles up to 5 seconds); default 200 ms.
    pub fn with_first_backoff(mut self, backoff: Duration) -> Self {
        self.first_backoff = backoff;
        self
    }

    /// A client for an endpoint as `SPIFFE_ENDPOINT_SOCKET` writes it: `unix:///path` or
    /// `unix:path`.
    pub fn from_endpoint(endpoint: &str) -> Result<Self, SpiffeError> {
        let path = endpoint
            .strip_prefix("unix://")
            .or_else(|| endpoint.strip_prefix("unix:"))
            .filter(|path| !path.is_empty())
            .ok_or_else(|| SpiffeError::BadEndpoint(endpoint.to_owned()))?;
        Ok(Self::new(path))
    }

    /// The client for `SPIFFE_ENDPOINT_SOCKET`, or `None` if it is unset or blank.
    pub fn from_env(env: &dyn Env) -> Result<Option<Self>, SpiffeError> {
        match env.var(ENDPOINT_VARIABLE).filter(|v| !v.trim().is_empty()) {
            Some(endpoint) => {
                let mut client = Self::from_endpoint(endpoint.trim())?;
                if let Some(text) = env
                    .var(FETCH_TIMEOUT_VARIABLE)
                    .filter(|v| !v.trim().is_empty())
                {
                    client.fetch_timeout = parse_duration(text.trim())
                        .ok_or_else(|| SpiffeError::BadTimeout(text.trim().to_owned()))?;
                }
                Ok(Some(client))
            }
            None => Ok(None),
        }
    }

    /// The socket.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub(crate) fn unavailable(&self, reason: impl ToString) -> SpiffeError {
        SpiffeError::Unavailable {
            endpoint: self.socket.display().to_string(),
            reason: reason.to_string(),
        }
    }

    /// Fetches, retrying transient errors with a doubling pause until the deadline.
    async fn fetch_with_retry(&self, audience: &str) -> Result<JwtSvid, SpiffeError> {
        let started = std::time::Instant::now();
        let mut pause = self.first_backoff;
        loop {
            let error = match self.fetch(audience).await {
                Ok(svid) => return Ok(svid),
                Err(error) => error,
            };
            if self.fetch_timeout.is_zero() || !is_transient(&error) {
                return Err(error);
            }
            if started.elapsed() + pause > self.fetch_timeout {
                return Err(SpiffeError::GaveUp {
                    waited: describe(started.elapsed()),
                    last: Box::new(error),
                });
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(MAX_BACKOFF);
        }
    }

    async fn fetch(&self, audience: &str) -> Result<JwtSvid, SpiffeError> {
        let request = JwtSvidRequest {
            audience: vec![audience.to_owned()],
            spiffe_id: String::new(),
        };
        let response: JwtSvidResponse = self.call(FETCH_JWT_SVID, &request).await?;
        let svid = response
            .svids
            .into_iter()
            .next()
            .ok_or_else(|| self.unavailable("no JWT-SVID in the response"))?;
        Ok(JwtSvid::new(svid.svid, svid.spiffe_id))
    }

    pub(crate) fn fetch_timeout(&self) -> Duration {
        self.fetch_timeout
    }

    pub(crate) fn first_backoff(&self) -> Duration {
        self.first_backoff
    }

    async fn validate(&self, token: &str, audience: &str) -> Result<SpiffeId, SpiffeError> {
        let request = ValidateJwtSvidRequest {
            audience: audience.to_owned(),
            svid: token.to_owned(),
        };
        let response: ValidateJwtSvidResponse = self.call(VALIDATE_JWT_SVID, &request).await?;
        SpiffeId::parse(&response.spiffe_id).map_err(|error| self.unavailable(error))
    }

    /// One server-streaming gRPC call: `on_message` gets every message until the stream ends
    /// (`Ok`) or fails.
    pub(crate) async fn stream<Req: Message, Resp: Message + Default>(
        &self,
        path: &str,
        message: &Req,
        mut on_message: impl FnMut(Resp),
    ) -> Result<(), SpiffeError> {
        let connect = async {
            let stream = UnixStream::connect(&self.socket)
                .await
                .map_err(|error| self.unavailable(error))?;
            let (sender, connection) = h2::client::handshake(stream)
                .await
                .map_err(|error| self.unavailable(error))?;
            Ok::<_, SpiffeError>((sender, tokio::spawn(connection)))
        };
        let (sender, driver) = tokio::time::timeout(TIMEOUT, connect)
            .await
            .map_err(|_| self.unavailable("timed out"))??;
        let result = self
            .read_stream(sender, path, message, &mut on_message)
            .await;
        driver.abort();
        result
    }

    async fn read_stream<Req: Message, Resp: Message + Default>(
        &self,
        sender: h2::client::SendRequest<Bytes>,
        path: &str,
        message: &Req,
        on_message: &mut impl FnMut(Resp),
    ) -> Result<(), SpiffeError> {
        let request = self.request(path)?;
        let mut sender = sender
            .ready()
            .await
            .map_err(|error| self.unavailable(error))?;
        let (response, mut stream) = sender
            .send_request(request, false)
            .map_err(|error| self.unavailable(error))?;
        stream
            .send_data(frame(message), true)
            .map_err(|error| self.unavailable(error))?;
        let response = tokio::time::timeout(TIMEOUT, response)
            .await
            .map_err(|_| self.unavailable("timed out"))?
            .map_err(|error| self.unavailable(error))?;
        let (parts, mut body) = response.into_parts();
        let mut pending = BytesMut::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|error| self.unavailable(error))?;
            let _ = body.flow_control().release_capacity(chunk.len());
            pending.extend_from_slice(&chunk);
            while pending.len() >= 5 {
                let length =
                    u32::from_be_bytes([pending[1], pending[2], pending[3], pending[4]]) as usize;
                if pending[0] != 0 {
                    return Err(self.unavailable("compressed gRPC message"));
                }
                if pending.len() < 5 + length {
                    break;
                }
                let frame = pending.split_to(5 + length).freeze().slice(5..);
                on_message(Resp::decode(frame).map_err(|error| self.unavailable(error))?);
            }
        }
        let trailers = body
            .trailers()
            .await
            .map_err(|error| self.unavailable(error))?;
        self.check_status(trailers.as_ref().unwrap_or(&parts.headers))
    }

    fn request(&self, path: &str) -> Result<http::Request<()>, SpiffeError> {
        http::Request::post(format!("http://localhost{path}"))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            // Required by the Workload API: marks the call as made by a workload.
            .header("workload.spiffe.io", "true")
            .body(())
            .map_err(|error| self.unavailable(error))
    }

    fn check_status(&self, headers: &http::HeaderMap) -> Result<(), SpiffeError> {
        let status = headers
            .get("grpc-status")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u32>().ok());
        match status {
            Some(0) => Ok(()),
            Some(status) => Err(SpiffeError::Refused {
                endpoint: self.socket.display().to_string(),
                status,
                message: headers
                    .get("grpc-message")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned(),
            }),
            None => Err(self.unavailable("the response has no grpc-status")),
        }
    }

    /// One unary gRPC call.
    async fn call<Req: Message, Resp: Message + Default>(
        &self,
        path: &str,
        message: &Req,
    ) -> Result<Resp, SpiffeError> {
        let exchange = async {
            let stream = UnixStream::connect(&self.socket)
                .await
                .map_err(|error| self.unavailable(error))?;
            let (sender, connection) = h2::client::handshake(stream)
                .await
                .map_err(|error| self.unavailable(error))?;
            let driver = tokio::spawn(connection);
            let result = self.exchange(sender, path, message).await;
            driver.abort();
            result
        };
        tokio::time::timeout(TIMEOUT, exchange)
            .await
            .map_err(|_| self.unavailable("timed out"))?
    }

    async fn exchange<Req: Message, Resp: Message + Default>(
        &self,
        sender: h2::client::SendRequest<Bytes>,
        path: &str,
        message: &Req,
    ) -> Result<Resp, SpiffeError> {
        let request = self.request(path)?;
        let framed = frame(message);
        let mut sender = sender
            .ready()
            .await
            .map_err(|error| self.unavailable(error))?;
        let (response, mut stream) = sender
            .send_request(request, false)
            .map_err(|error| self.unavailable(error))?;
        stream
            .send_data(framed, true)
            .map_err(|error| self.unavailable(error))?;
        let response = response.await.map_err(|error| self.unavailable(error))?;
        let (parts, mut body) = response.into_parts();
        let mut data = BytesMut::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.map_err(|error| self.unavailable(error))?;
            let _ = body.flow_control().release_capacity(chunk.len());
            data.extend_from_slice(&chunk);
        }
        let trailers = body
            .trailers()
            .await
            .map_err(|error| self.unavailable(error))?;
        // A call that fails at once answers with the status in the headers ("trailers only").
        self.check_status(trailers.as_ref().unwrap_or(&parts.headers))?;
        if data.len() < 5 || data[0] != 0 {
            return Err(self.unavailable("malformed or compressed gRPC message"));
        }
        let mut data = data.freeze();
        data.advance(1);
        let length = data.get_u32() as usize;
        if data.len() < length {
            return Err(self.unavailable("truncated gRPC message"));
        }
        Resp::decode(data.slice(..length)).map_err(|error| self.unavailable(error))
    }
}

/// A gRPC frame: no compression, length, message.
fn frame(message: &impl Message) -> Bytes {
    let body = message.encode_to_vec();
    let mut framed = BytesMut::with_capacity(5 + body.len());
    framed.extend_from_slice(&[0]);
    framed.extend_from_slice(&(body.len() as u32).to_be_bytes());
    framed.extend_from_slice(&body);
    framed.freeze()
}

impl WorkloadIdentity for WorkloadApiClient {
    fn jwt_svid<'a>(&'a self, audience: &'a str) -> BoxFuture<'a, Result<JwtSvid, SpiffeError>> {
        Box::pin(self.fetch_with_retry(audience))
    }

    fn validate_jwt_svid<'a>(
        &'a self,
        token: &'a str,
        audience: &'a str,
    ) -> BoxFuture<'a, Result<SpiffeId, SpiffeError>> {
        Box::pin(self.validate(token, audience))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(status: u32) -> SpiffeError {
        SpiffeError::Refused {
            endpoint: "s".into(),
            status,
            message: String::new(),
        }
    }

    #[test]
    fn only_denied_and_unavailable_are_retried() {
        assert!(
            is_transient(&refused(7)),
            "PERMISSION_DENIED: not issued yet"
        );
        assert!(is_transient(&refused(14)), "UNAVAILABLE");
        assert!(!is_transient(&refused(3)), "INVALID_ARGUMENT");
        assert!(!is_transient(&refused(12)), "UNIMPLEMENTED");
        assert!(!is_transient(&SpiffeError::BadEndpoint("x".into())));
        assert!(is_transient(&SpiffeError::Unavailable {
            endpoint: "s".into(),
            reason: "no socket".into()
        }));
    }

    #[test]
    fn durations_have_a_unit() {
        assert_eq!(parse_duration("250ms"), Some(Duration::from_millis(250)));
        assert_eq!(parse_duration("0s"), Some(Duration::ZERO));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("5"), None);
        assert_eq!(parse_duration(""), None);
    }
}
