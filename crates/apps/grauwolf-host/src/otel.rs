//! Export of the agent spans via OTLP/HTTP (ADR-0049).
//!
//! Without an explicit endpoint, the standard OpenTelemetry variables apply:
//! `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`,
//! `OTEL_EXPORTER_OTLP_HEADERS` (e.g. for Langfuse or Datadog) and `OTEL_SERVICE_NAME`
//! (default `grauwolf`).

use std::sync::Arc;

use grauwolf_agents::events::Feature;
use grauwolf_otel::OtelFeature;
use opentelemetry::global::BoxedTracer;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::config::{ProcessEnv, TelemetryConfig};

/// Configured export; `shutdown` sends spans still pending. Clones share the same export.
#[derive(Clone)]
pub struct OtelTracing {
    provider: SdkTracerProvider,
}

impl OtelTracing {
    /// Sets up the export, to `endpoint` (base URL, `/v1/traces` is appended) or according to
    /// the standard variables.
    pub fn init(endpoint: Option<&str>) -> Result<Self, String> {
        Self::init_with(endpoint, &TelemetryConfig::from_env(&ProcessEnv))
    }

    /// Like [`init`](Self::init) with the service name from `config`. The endpoint variables
    /// (`OTEL_EXPORTER_OTLP_*`) are read by the OpenTelemetry SDK from the process environment.
    pub fn init_with(endpoint: Option<&str>, config: &TelemetryConfig) -> Result<Self, String> {
        let mut exporter = SpanExporter::builder().with_http();
        if let Some(endpoint) = endpoint {
            exporter =
                exporter.with_endpoint(format!("{}/v1/traces", endpoint.trim_end_matches('/')));
        }
        let exporter = exporter
            .build()
            .map_err(|error| format!("OpenTelemetry export: {error}"))?;
        let service = config.service_name();
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(Resource::builder().with_service_name(service).build())
            .build();
        Ok(Self { provider })
    }

    /// Feature for the agent `agent_name`; `content` also records contents.
    pub fn feature(&self, agent_name: &str, content: bool) -> Arc<dyn Feature> {
        let tracer = BoxedTracer::new(Box::new(self.provider.tracer("grauwolf")));
        Arc::new(OtelFeature::new(tracer, agent_name).with_content(content))
    }

    /// Sends pending spans and ends the export; blocks (do not call in an async context).
    pub fn shutdown(self) -> Result<(), String> {
        self.provider
            .shutdown()
            .map_err(|error| format!("OpenTelemetry export: {error}"))
    }
}
