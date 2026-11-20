//! The Jira tools against a mock (ADR-0104).

mod common;

use grauwolf_atlassian::JIRA_TOOLS;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{client, tool};

async fn call(server: &MockServer, name: &str, args: Value) -> String {
    let tools = client(server)
        .jira_tools(JIRA_TOOLS.iter().copied())
        .unwrap();
    tool(&tools, name).call_json(args).await.unwrap()
}

#[tokio::test]
async fn searches_issues_with_jql_and_a_clamped_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/search/jql"))
        .and(query_param(
            "jql",
            "project = ABC AND status = 'In Progress'",
        ))
        .and(query_param("maxResults", "50"))
        .and(query_param("fields", "summary,status,assignee"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"issues\":[]}"))
        .expect(1)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "jira_search_issues",
        json!({ "jql": "project = ABC AND status = 'In Progress'", "max_results": 500 }),
    )
    .await;

    assert_eq!(result, "HTTP 200\n{\"issues\":[]}");
}

#[tokio::test]
async fn gets_an_issue_and_refuses_keys_that_could_leave_the_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/issue/ABC-123"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"key\":\"ABC-123\"}"))
        .expect(1)
        .mount(&server)
        .await;

    let found = call(&server, "jira_get_issue", json!({ "issue_key": "ABC-123" })).await;
    let escaped = call(&server, "jira_get_issue", json!({ "issue_key": "../../x" })).await;
    let invalid = call(
        &server,
        "jira_get_issue",
        json!({ "issue_key": "ABC-1/../2" }),
    )
    .await;

    assert_eq!(found, "HTTP 200\n{\"key\":\"ABC-123\"}");
    assert_eq!(escaped, "Invalid issue_key: ../../x");
    assert_eq!(invalid, "Invalid issue_key: ABC-1/../2");
}

#[tokio::test]
async fn creates_an_issue_with_the_description_as_adf() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue"))
        .and(body_json(json!({ "fields": {
            "project": { "key": "ABC" },
            "issuetype": { "name": "Bug" },
            "summary": "Crash",
            "description": { "type": "doc", "version": 1, "content": [
                { "type": "paragraph", "content": [{ "type": "text", "text": "line one" }] },
                { "type": "paragraph" },
                { "type": "paragraph", "content": [{ "type": "text", "text": "line three" }] }
            ] }
        } })))
        .respond_with(ResponseTemplate::new(201).set_body_string("{\"key\":\"ABC-9\"}"))
        .expect(1)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "jira_create_issue",
        json!({ "project_key": "ABC", "issue_type": "Bug", "summary": "Crash",
                "description": "line one\n\nline three" }),
    )
    .await;

    assert_eq!(result, "HTTP 201\n{\"key\":\"ABC-9\"}");
}

#[tokio::test]
async fn adds_a_comment_as_adf_and_an_empty_text_is_one_empty_paragraph() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue/ABC-1/comment"))
        .and(body_json(
            json!({ "body": { "type": "doc", "version": 1, "content": [
            { "type": "paragraph", "content": [{ "type": "text", "text": "Done" }] },
            { "type": "paragraph" }
        ] } }),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "jira_add_comment",
        json!({ "issue_key": "ABC-1", "comment": "Done\r\n" }),
    )
    .await;
    let invalid = call(
        &server,
        "jira_add_comment",
        json!({ "issue_key": "x y", "comment": "c" }),
    )
    .await;

    assert_eq!(result, "HTTP 201\n{}");
    assert_eq!(invalid, "Invalid issue_key: x y");
}

fn transitions() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "transitions": [
        { "id": "11", "name": "Start", "to": { "name": "In Progress" } },
        { "id": "31", "name": "Finish", "to": { "name": "Done" } }
    ] }))
}

#[tokio::test]
async fn moves_an_issue_by_transition_name_or_target_status_ignoring_case() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/issue/ABC-1/transitions"))
        .respond_with(transitions())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/rest/api/3/issue/ABC-1/transitions"))
        .and(body_json(json!({ "transition": { "id": "31" } })))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&server)
        .await;

    let by_status = call(
        &server,
        "jira_transition_issue",
        json!({ "issue_key": "ABC-1", "status_name": "done" }),
    )
    .await;
    let by_name = call(
        &server,
        "jira_transition_issue",
        json!({ "issue_key": "ABC-1", "status_name": "FINISH" }),
    )
    .await;

    assert_eq!(by_status, "HTTP 204\n");
    assert_eq!(by_name, "HTTP 204\n");
}

#[tokio::test]
async fn lists_the_available_transitions_when_none_matches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/issue/ABC-1/transitions"))
        .respond_with(transitions())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;

    let result = call(
        &server,
        "jira_transition_issue",
        json!({ "issue_key": "ABC-1", "status_name": "Closed" }),
    )
    .await;

    assert_eq!(
        result,
        "No transition to 'Closed'. Available: Start, Finish"
    );
}

#[tokio::test]
async fn a_failed_transition_lookup_is_returned_as_it_is() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/issue/ABC-1/transitions"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .mount(&server)
        .await;

    let result = call(
        &server,
        "jira_transition_issue",
        json!({ "issue_key": "ABC-1", "status_name": "Done" }),
    )
    .await;

    assert_eq!(result, "HTTP 403\nforbidden");
}

#[tokio::test]
async fn missing_arguments_are_invalid_arguments() {
    let server = MockServer::start().await;
    let tools = client(&server).jira_tools(["jira_get_issue"]).unwrap();

    let error = tool(&tools, "jira_get_issue")
        .call_json(json!({}))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("issue_key"), "{error}");
    let descriptor = tool(&tools, "jira_get_issue").descriptor();
    assert_eq!(descriptor.parameters["required"], json!(["issue_key"]));
}
