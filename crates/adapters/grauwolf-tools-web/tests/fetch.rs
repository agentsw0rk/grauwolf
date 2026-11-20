//! The `fetch` tool (ADR-0098) against a local mock server.

use std::net::IpAddr;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use grauwolf_tools::DynTool;
use grauwolf_tools_web::{FetchConfig, Resolver, fetch_tool, fetch_tool_with};
use serde_json::json;
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config(hosts: &[&str], allow_http: bool) -> FetchConfig {
    FetchConfig {
        allowed_hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
        allow_http,
        auth: Vec::new(),
    }
}

async fn fetch(tool: &Arc<dyn DynTool>, args: serde_json::Value) -> String {
    match tool.call_json(args).await {
        Ok(text) => text,
        Err(error) => error.to_string(),
    }
}

/// Resolves every name to one fixed address.
struct Fixed(IpAddr);

impl Resolver for Fixed {
    fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, Result<Vec<IpAddr>, String>> {
        let ip = self.0;
        Box::pin(async move { Ok(vec![ip]) })
    }
}

#[tokio::test]
async fn returns_status_headers_and_body_of_an_allowed_host() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/hello"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("Hello there", "text/plain"))
        .mount(&server)
        .await;
    let tool = fetch_tool(config(&["127.0.0.1"], true));

    let text = fetch(&tool, json!({ "url": format!("{}/hello", server.uri()) })).await;

    assert!(
        text.starts_with("HTTP 200 OK\ncontent-type: text/plain"),
        "{text}"
    );
    assert!(text.contains("content-length: 11"), "{text}");
    assert!(text.ends_with("\n\nHello there"), "{text}");
}

#[tokio::test]
async fn sends_method_headers_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/submit"))
        .and(header("x-token", "abc"))
        .and(body_string("payload"))
        .respond_with(ResponseTemplate::new(201).set_body_string("created"))
        .expect(1)
        .mount(&server)
        .await;
    let tool = fetch_tool(config(&["127.0.0.1"], true));

    let text = fetch(
        &tool,
        json!({ "url": format!("{}/submit", server.uri()), "method": "post",
                "headers": { "x-token": "abc" }, "body": "payload" }),
    )
    .await;

    assert!(text.starts_with("HTTP 201 Created"), "{text}");
}

#[tokio::test]
async fn refuses_hosts_that_are_not_listed() {
    let server = MockServer::start().await;
    let tool = fetch_tool(config(&["example.com", "*.example.org"], true));

    let denied = fetch(&tool, json!({ "url": format!("{}/", server.uri()) })).await;
    let sibling = fetch(&tool, json!({ "url": "https://example.org/" })).await;

    assert!(
        denied.contains("host '127.0.0.1' is not in allowed_hosts"),
        "{denied}"
    );
    assert!(sibling.contains("not in allowed_hosts"), "{sibling}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn refuses_plain_http_unless_the_spec_allows_it() {
    let server = MockServer::start().await;
    let tool = fetch_tool(config(&["127.0.0.1"], false));

    let text = fetch(&tool, json!({ "url": format!("{}/", server.uri()) })).await;

    assert!(text.contains("only https:// URLs"), "{text}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn follows_redirects_to_allowed_hosts_only() {
    let server = MockServer::start().await;
    Mock::given(path("/to-ok"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/ok"))
        .mount(&server)
        .await;
    Mock::given(path("/ok"))
        .respond_with(ResponseTemplate::new(200).set_body_string("arrived"))
        .mount(&server)
        .await;
    Mock::given(path("/to-evil"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "http://evil.example.net/x"),
        )
        .mount(&server)
        .await;
    let tool = fetch_tool(config(&["127.0.0.1"], true));

    let followed = fetch(&tool, json!({ "url": format!("{}/to-ok", server.uri()) })).await;
    let refused = fetch(&tool, json!({ "url": format!("{}/to-evil", server.uri()) })).await;

    assert!(followed.ends_with("arrived"), "{followed}");
    assert!(
        refused.contains("host 'evil.example.net' is not in allowed_hosts"),
        "{refused}"
    );
}

#[tokio::test]
async fn refuses_hosts_that_resolve_to_private_addresses_unless_listed_exactly() {
    let server = MockServer::start().await;
    Mock::given(path("/data"))
        .respond_with(ResponseTemplate::new(200).set_body_string("internal"))
        .mount(&server)
        .await;
    let port = server.address().port();
    let loopback: Arc<dyn Resolver> = Arc::new(Fixed("127.0.0.1".parse().unwrap()));
    let wildcard = fetch_tool_with(config(&["*.example.test"], true), loopback.clone());
    let exact = fetch_tool_with(config(&["intranet.example.test"], true), loopback);
    let link_local = fetch_tool_with(
        config(&["*.example.test"], true),
        Arc::new(Fixed("169.254.169.254".parse().unwrap())),
    );
    let url = format!("http://intranet.example.test:{port}/data");

    let refused = fetch(&wildcard, json!({ "url": url })).await;
    let metadata = fetch(&link_local, json!({ "url": url })).await;
    let allowed = fetch(&exact, json!({ "url": url })).await;

    assert!(
        refused.contains("private, loopback or link-local"),
        "{refused}"
    );
    assert!(
        metadata.contains("private, loopback or link-local"),
        "{metadata}"
    );
    assert!(allowed.ends_with("internal"), "{allowed}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn cuts_the_body_off_at_one_mebibyte() {
    let server = MockServer::start().await;
    Mock::given(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(2 * 1024 * 1024)))
        .mount(&server)
        .await;
    let tool = fetch_tool(config(&["127.0.0.1"], true));

    let text = fetch(&tool, json!({ "url": format!("{}/big", server.uri()) })).await;

    assert!(
        text.ends_with("\n[truncated after 1 MiB]"),
        "{}",
        &text[text.len() - 40..]
    );
    assert!(text.len() < 1024 * 1024 + 200);
}

#[tokio::test]
async fn reduces_html_to_text() {
    let server = MockServer::start().await;
    let html = "<html><head><title>Title</title><style>p { color: red }</style>\
                <script>var x = '<b>';</script></head>\
                <body><h1>Head &amp; line</h1><p>First <b>bold</b>\n paragraph.</p><!-- hidden -->\
                <p>Second</p></body></html>";
    Mock::given(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(html, "text/html; charset=utf-8"))
        .mount(&server)
        .await;
    let tool = fetch_tool(config(&["127.0.0.1"], true));

    let text = fetch(&tool, json!({ "url": format!("{}/page", server.uri()) })).await;

    let body = text.split("\n\n").nth(1).unwrap();
    assert_eq!(body, "Title\nHead & line\nFirst bold paragraph.\nSecond");
}
