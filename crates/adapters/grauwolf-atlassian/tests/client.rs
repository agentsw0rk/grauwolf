//! The client: base URL, credentials, results (ADR-0104).

mod common;

use grauwolf_atlassian::{AtlassianClient, CONFLUENCE_TOOLS, JIRA_TOOLS};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{AUTH, client, tool};

#[test]
fn reduces_the_base_url_to_scheme_and_host() {
    let client =
        AtlassianClient::new("https://example.atlassian.net/wiki/spaces/X", "a", "b").unwrap();

    assert_eq!(client.base_url(), "https://example.atlassian.net");
}

#[test]
fn rejects_an_unusable_base_url() {
    assert!(AtlassianClient::new("not a url", "a", "b").is_err());
}

#[test]
fn reads_the_credentials_from_the_environment_and_names_the_missing_one() {
    let env = |name: &str| (name == "ATLASSIAN_EMAIL").then(|| "a@b.c".to_owned());

    let error = AtlassianClient::from_env("https://x.atlassian.net", env).unwrap_err();
    let blank = AtlassianClient::from_env("https://x.atlassian.net", |_| Some("  ".into()));

    assert_eq!(error, "ATLASSIAN_API_TOKEN is not set");
    assert_eq!(blank.unwrap_err(), "ATLASSIAN_EMAIL is not set");
}

#[test]
fn debug_output_hides_the_credentials() {
    let client = AtlassianClient::new("https://x.atlassian.net", "me@x.io", "s3cret").unwrap();

    let shown = format!("{client:?}");

    assert!(
        !shown.contains("s3cret") && !shown.contains("me@x.io"),
        "{shown}"
    );
}

#[test]
fn unknown_tool_names_are_an_error_and_all_known_names_exist() {
    let client = AtlassianClient::new("https://x.atlassian.net", "a", "b").unwrap();

    assert_eq!(
        client.jira_tools(JIRA_TOOLS.iter().copied()).unwrap().len(),
        5
    );
    assert_eq!(
        client
            .confluence_tools(CONFLUENCE_TOOLS.iter().copied())
            .unwrap()
            .len(),
        4
    );
    assert!(client.jira_tools(["jira_delete_everything"]).is_err());
}

#[tokio::test]
async fn returns_the_status_and_body_also_for_errors_and_sends_basic_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/api/3/issue/ABC-1"))
        .and(header("authorization", AUTH))
        .respond_with(ResponseTemplate::new(404).set_body_string("{\"errorMessages\":[\"gone\"]}"))
        .mount(&server)
        .await;
    let tools = client(&server).jira_tools(["jira_get_issue"]).unwrap();

    let result = tool(&tools, "jira_get_issue")
        .call_json(json!({ "issue_key": "ABC-1" }))
        .await
        .unwrap();

    assert_eq!(result, "HTTP 404\n{\"errorMessages\":[\"gone\"]}");
}

#[tokio::test]
async fn a_network_failure_is_an_error_for_the_model() {
    let client = AtlassianClient::new("http://127.0.0.1:1", "a", "b").unwrap();
    let tools = client.jira_tools(["jira_get_issue"]).unwrap();

    let error = tool(&tools, "jira_get_issue")
        .call_json(json!({ "issue_key": "ABC-1" }))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("failed"), "{error}");
}
