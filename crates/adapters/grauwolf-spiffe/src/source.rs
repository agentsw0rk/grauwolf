//! The live source of the own X.509-SVID and the trust bundles. Two Workload API streams update
//! it; rustls resolvers and verifiers read the latest value on every handshake, so a rotation
//! needs no restart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::SpiffeError;
use crate::client::{
    FETCH_X509_BUNDLES, FETCH_X509_SVID, WorkloadApiClient, describe, is_transient,
};
use crate::der::split_certificates;
use crate::id::SpiffeId;
use crate::proto::{X509BundlesRequest, X509BundlesResponse, X509SvidRequest, X509SvidResponse};

const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// The own certificate chain and key.
#[derive(Clone)]
pub struct X509Svid {
    id: SpiffeId,
    key: Arc<CertifiedKey>,
}

impl X509Svid {
    /// From a DER chain (leaf first) and a PKCS#8 DER key.
    pub fn new(id: SpiffeId, chain: Vec<Vec<u8>>, pkcs8_key: Vec<u8>) -> Result<Self, SpiffeError> {
        let invalid = |reason: String| SpiffeError::Invalid(format!("X509-SVID {id}: {reason}"));
        if chain.is_empty() {
            return Err(invalid("empty certificate chain".into()));
        }
        let chain: Vec<CertificateDer<'static>> =
            chain.into_iter().map(CertificateDer::from).collect();
        let key = crate::provider()
            .key_provider
            .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_key)))
            .map_err(|error| invalid(format!("private key: {error}")))?;
        Ok(Self {
            id,
            key: Arc::new(CertifiedKey::new(chain, key)),
        })
    }

    /// The SPIFFE ID.
    pub fn id(&self) -> &SpiffeId {
        &self.id
    }

    pub(crate) fn certified_key(&self) -> Arc<CertifiedKey> {
        Arc::clone(&self.key)
    }
}

#[derive(Default)]
struct Snapshot {
    svid: Option<X509Svid>,
    /// Bundles from the `FetchX509Bundles` stream, by trust domain.
    bundles: HashMap<String, Arc<Vec<CertificateDer<'static>>>>,
    /// Bundles that came with the SVID response (own and federated domains).
    svid_bundles: HashMap<String, Arc<Vec<CertificateDer<'static>>>>,
}

#[derive(Default)]
struct Shared {
    state: RwLock<Snapshot>,
    last_error: Mutex<Option<SpiffeError>>,
}

/// The live X.509 source. Dropping it stops the streams.
pub struct X509Source {
    shared: Arc<Shared>,
    changed: watch::Sender<u64>,
    tasks: Vec<JoinHandle<()>>,
}

fn domain_of(key: &str) -> String {
    key.strip_prefix("spiffe://")
        .unwrap_or(key)
        .trim_end_matches('/')
        .to_owned()
}

fn parse_bundle(
    bundles: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> HashMap<String, Arc<Vec<CertificateDer<'static>>>> {
    bundles
        .into_iter()
        .filter_map(|(key, der)| {
            let certificates = split_certificates(&der)?;
            Some((
                domain_of(&key),
                Arc::new(certificates.into_iter().map(CertificateDer::from).collect()),
            ))
        })
        .collect()
}

impl X509Source {
    /// A source that is only changed by hand ([`set_svid`](Self::set_svid),
    /// [`set_bundle`](Self::set_bundle)): for tests and fakes.
    pub fn manual() -> Self {
        Self {
            shared: Arc::default(),
            changed: watch::channel(0).0,
            tasks: Vec::new(),
        }
    }

    /// Connects to the Workload API, starts the two streams and waits until an SVID and the
    /// bundle of its trust domain have arrived, at most the client's fetch timeout (with the
    /// same retry rules as for JWT-SVIDs). The streams then keep running and reconnect.
    pub async fn start(client: &WorkloadApiClient) -> Result<Self, SpiffeError> {
        let shared = Arc::<Shared>::default();
        let (changed, mut receiver) = watch::channel(0u64);
        let svid_task = tokio::spawn(run_stream(
            client.clone(),
            Arc::clone(&shared),
            changed.clone(),
            FETCH_X509_SVID,
            |shared: &Shared, response: X509SvidResponse| {
                let mut state = shared.state.write().unwrap();
                if let Some(first) = response.svids.into_iter().next() {
                    let svid = SpiffeId::parse(&first.spiffe_id)
                        .map_err(|error| SpiffeError::Invalid(error.to_string()))
                        .and_then(|id| {
                            let chain = split_certificates(&first.x509_svid).ok_or_else(|| {
                                SpiffeError::Invalid("malformed certificate chain".into())
                            })?;
                            X509Svid::new(id, chain, first.x509_svid_key)
                        });
                    match svid {
                        Ok(svid) => {
                            let mut bundles = parse_bundle(response.federated_bundles);
                            bundles.extend(parse_bundle([(
                                svid.id().trust_domain().to_owned(),
                                first.bundle,
                            )]));
                            state.svid = Some(svid);
                            state.svid_bundles = bundles;
                        }
                        Err(error) => {
                            *shared.last_error.lock().unwrap() = Some(error);
                        }
                    }
                }
            },
            X509SvidRequest {},
        ));
        let bundle_task = tokio::spawn(run_stream(
            client.clone(),
            Arc::clone(&shared),
            changed.clone(),
            FETCH_X509_BUNDLES,
            |shared: &Shared, response: X509BundlesResponse| {
                shared.state.write().unwrap().bundles = parse_bundle(response.bundles);
            },
            X509BundlesRequest {},
        ));
        let source = Self {
            shared,
            changed,
            tasks: vec![svid_task, bundle_task],
        };
        let timeout = client.fetch_timeout();
        let wait = async {
            loop {
                if source.is_ready() {
                    return;
                }
                if timeout.is_zero() && source.shared.last_error.lock().unwrap().is_some() {
                    return;
                }
                if receiver.changed().await.is_err() {
                    return;
                }
            }
        };
        let limit = if timeout.is_zero() {
            Duration::from_secs(10)
        } else {
            timeout
        };
        let started = std::time::Instant::now();
        if tokio::time::timeout(limit, wait).await.is_err() || !source.is_ready() {
            let last = source
                .shared
                .last_error
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| client.unavailable("no X509-SVID and bundle received"));
            return Err(if timeout.is_zero() || !is_transient(&last) {
                last
            } else {
                SpiffeError::GaveUp {
                    waited: describe(started.elapsed()),
                    last: Box::new(last),
                }
            });
        }
        Ok(source)
    }

    fn is_ready(&self) -> bool {
        let state = self.shared.state.read().unwrap();
        state.svid.as_ref().is_some_and(|svid| {
            state.bundles.contains_key(svid.id().trust_domain())
                || state.svid_bundles.contains_key(svid.id().trust_domain())
        })
    }

    /// Replaces the own SVID (a rotation).
    pub fn set_svid(&self, svid: X509Svid) {
        self.shared.state.write().unwrap().svid = Some(svid);
        self.changed.send_modify(|n| *n += 1);
    }

    /// Replaces the bundle of `domain` with DER certificates.
    pub fn set_bundle(&self, domain: &str, certificates: Vec<Vec<u8>>) {
        let certificates = certificates.into_iter().map(CertificateDer::from).collect();
        self.shared
            .state
            .write()
            .unwrap()
            .bundles
            .insert(domain.to_owned(), Arc::new(certificates));
        self.changed.send_modify(|n| *n += 1);
    }

    /// The current SVID.
    pub fn svid(&self) -> Option<X509Svid> {
        self.shared.state.read().unwrap().svid.clone()
    }

    /// The current bundle of `domain`.
    pub fn bundle(&self, domain: &str) -> Option<Arc<Vec<CertificateDer<'static>>>> {
        let state = self.shared.state.read().unwrap();
        state
            .bundles
            .get(domain)
            .or_else(|| state.svid_bundles.get(domain))
            .cloned()
    }
}

impl Drop for X509Source {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Runs one stream forever: applies every message, reconnects with a doubling pause.
async fn run_stream<Req, Resp>(
    client: WorkloadApiClient,
    shared: Arc<Shared>,
    changed: watch::Sender<u64>,
    path: &'static str,
    apply: impl Fn(&Shared, Resp),
    request: Req,
) where
    Req: prost::Message,
    Resp: prost::Message + Default,
{
    let mut pause = client.first_backoff();
    loop {
        let mut received = false;
        let result = client
            .stream(path, &request, |message: Resp| {
                received = true;
                apply(&shared, message);
                changed.send_modify(|n| *n += 1);
            })
            .await;
        if received {
            pause = client.first_backoff();
        }
        let error = match result {
            Ok(()) => client.unavailable("the stream ended"),
            Err(error) => error,
        };
        *shared.last_error.lock().unwrap() = Some(error);
        changed.send_modify(|n| *n += 1);
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(MAX_BACKOFF);
    }
}
