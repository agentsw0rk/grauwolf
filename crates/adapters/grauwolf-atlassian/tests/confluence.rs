//! The Confluence tools against a mock (ADR-0104).

mod common;

use grauwolf_atlassian::CONFLUENCE_TOOLS;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{client, tool};

async fn call(server: &MockServer, name: &str, args: Value) -> String {
    let tools = client(server)
        .confluence_tools(CONFLUENCE_TOOLS.iter().copied())
        .unwrap();
    tool(&tools, name).call_json(args).await.unwrap()
}

#[tokio::test]
async fn searches_pages_with_cql_and_a_clamped_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wiki/rest/api/search"))
        .and(query_param("cql", "type = page AND text ~ \"onboarding\""))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"results\":[]}"))
        .expect(1)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "confluence_search_pages",
        json!({ "cql": "type = page AND text ~ \"onboarding\"", "limit": 0 }),
    )
    .await;

    assert_eq!(result, "HTTP 200\n{\"results\":[]}");
}

#[tokio::test]
async fn gets_a_page_in_storage_format_and_refuses_ids_that_are_not_numbers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/pages/12345"))
        .and(query_param("body-format", "storage"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"id\":\"12345\"}"))
        .expect(1)
        .mount(&server)
        .await;

    let found = call(
        &server,
        "confluence_get_page",
        json!({ "page_id": "12345" }),
    )
    .await;
    let invalid = call(
        &server,
        "confluence_get_page",
        json!({ "page_id": "1/../2" }),
    )
    .await;

    assert_eq!(found, "HTTP 200\n{\"id\":\"12345\"}");
    assert_eq!(invalid, "Invalid page_id: 1/../2");
}

#[tokio::test]
async fn creates_a_page_in_the_space_found_by_its_key_below_a_parent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/spaces"))
        .and(query_param("keys", "TEAM"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "results": [{ "id": "777" }] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/wiki/api/v2/pages"))
        .and(body_json(json!({
            "spaceId": "777", "status": "current", "title": "Notes", "parentId": "42",
            "body": { "representation": "storage", "value": "<p>Hi</p>" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"id\":\"1\"}"))
        .expect(1)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "confluence_create_page",
        json!({ "space_key": "TEAM", "title": "Notes", "body": "<p>Hi</p>", "parent_id": "42" }),
    )
    .await;

    assert_eq!(result, "HTTP 200\n{\"id\":\"1\"}");
}

#[tokio::test]
async fn an_empty_parent_creates_a_top_level_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/spaces"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "results": [{ "id": "777" }] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/wiki/api/v2/pages"))
        .and(body_json(json!({
            "spaceId": "777", "status": "current", "title": "Top",
            "body": { "representation": "storage", "value": "" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "confluence_create_page",
        json!({ "space_key": "TEAM", "title": "Top", "body": "", "parent_id": " " }),
    )
    .await;

    assert_eq!(result, "HTTP 200\n{}");
}

#[tokio::test]
async fn names_an_unknown_space_and_returns_failed_lookups_as_they_are() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/spaces"))
        .and(query_param("keys", "NOPE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/spaces"))
        .and(query_param("keys", "DENIED"))
        .respond_with(ResponseTemplate::new(401).set_body_string("no"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let args = |key: &str| json!({ "space_key": key, "title": "t", "body": "b", "parent_id": "" });

    let unknown = call(&server, "confluence_create_page", args("NOPE")).await;
    let denied = call(&server, "confluence_create_page", args("DENIED")).await;

    assert_eq!(unknown, "Space 'NOPE' not found");
    assert_eq!(denied, "HTTP 401\nno");
}

#[tokio::test]
async fn updates_a_page_with_the_next_version_number() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/pages/42"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": "42", "version": { "number": 6 } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/wiki/api/v2/pages/42"))
        .and(body_json(json!({
            "id": "42", "status": "current", "title": "New",
            "body": { "representation": "storage", "value": "<p>x</p>" },
            "version": { "number": 7 }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}"))
        .expect(1)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "confluence_update_page",
        json!({ "page_id": "42", "title": "New", "body": "<p>x</p>" }),
    )
    .await;
    let invalid = call(
        &server,
        "confluence_update_page",
        json!({ "page_id": "4 2", "title": "t", "body": "b" }),
    )
    .await;

    assert_eq!(result, "HTTP 200\n{\"ok\":true}");
    assert_eq!(invalid, "Invalid page_id: 4 2");
}

#[tokio::test]
async fn a_failed_read_before_an_update_is_returned_and_nothing_is_written() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/wiki/api/v2/pages/42"))
        .respond_with(ResponseTemplate::new(404).set_body_string("missing"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "confluence_update_page",
        json!({ "page_id": "42", "title": "t", "body": "b" }),
    )
    .await;

    assert_eq!(result, "HTTP 404\nmissing");
}
