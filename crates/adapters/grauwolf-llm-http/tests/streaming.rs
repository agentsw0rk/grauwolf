//! Streamed responses: lines (NDJSON) and server-sent events (ADR-0033).

use futures_util::StreamExt;
use grauwolf_llm_http::{HttpClient, SseEvent};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn reads_a_response_line_by_line() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("{\"a\":1}\n\n{\"a\":2}\n{\"a\":3}"),
        )
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let lines: Vec<String> = client
        .post_lines("/api/chat", &json!({}))
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;

    assert_eq!(lines, ["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]);
}

#[tokio::test]
async fn reads_server_sent_events_with_names_and_multi_line_data() {
    let body = "event: message_start\ndata: {\"x\":1}\n\n: comment\ndata: line1\ndata: line2\n\ndata: [DONE]\n\n";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let events: Vec<SseEvent> = client
        .post_sse("/v1/messages", &json!({}))
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;

    assert_eq!(
        events,
        [
            SseEvent {
                event: Some("message_start".into()),
                data: "{\"x\":1}".into()
            },
            SseEvent {
                event: None,
                data: "line1\nline2".into()
            },
            SseEvent {
                event: None,
                data: "[DONE]".into()
            },
        ]
    );
}

#[tokio::test]
async fn reports_error_status_before_streaming() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
        .mount(&server)
        .await;
    let client = HttpClient::new(server.uri());

    let error = client.post_sse("/x", &json!({})).await.err().unwrap();

    assert!(matches!(
        grauwolf_llm::LlmError::from(error),
        grauwolf_llm::LlmError::Transient { .. }
    ));
}

/// Timeouts (ADR-0065): the read timeout applies per read; a stream that delivers data
/// regularly runs longer than it.
#[tokio::test]
async fn keeps_reading_a_stream_that_lasts_longer_than_the_read_timeout() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        for line in ["{\"a\":1}\n", "{\"a\":2}\n", "{\"a\":3}\n"] {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let chunk = format!("{:x}\r\n{line}\r\n", line.len());
            socket.write_all(chunk.as_bytes()).await.unwrap();
        }
        socket.write_all(b"0\r\n\r\n").await.unwrap();
    });
    let client = HttpClient::new(format!("http://{address}"))
        .with_read_timeout(std::time::Duration::from_millis(300));

    let lines: Vec<String> = client
        .post_lines("/api/chat", &json!({}))
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;

    assert_eq!(lines, ["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"]);
}
