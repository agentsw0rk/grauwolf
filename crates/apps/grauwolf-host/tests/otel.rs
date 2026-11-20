//! OTLP export of spans (F06.10, ADR-0049) against a recording HTTP server.

use grauwolf_agents::events::AgentEvent;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn exports_spans_over_otlp_http() {
    let collector = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&collector)
        .await;
    let tracing = grauwolf_host::otel::OtelTracing::init(Some(&collector.uri())).unwrap();
    let feature = tracing.feature("echo", false);

    feature.on_event(&AgentEvent::AgentStarting {
        input: json!("Hello"),
        caller: None,
        trace_parent: None,
        on_behalf_of: None,
    });
    feature.on_event(&AgentEvent::AgentCompleted {
        output: json!("Hello"),
        usage: None,
        cost: None,
    });
    tokio::task::spawn_blocking(move || tracing.shutdown())
        .await
        .unwrap()
        .unwrap();

    let requests = collector.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "{requests:?}");
    // Protobuf contains strings unchanged.
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("invoke_agent echo"), "{body}");
    assert!(body.contains("grauwolf"), "{body}");
}
