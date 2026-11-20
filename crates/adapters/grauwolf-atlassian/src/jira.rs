//! The Jira tools (REST API v3).

use futures_util::FutureExt;
use grauwolf_tools::ToolError;
use reqwest::Method;
use serde_json::{Value, json};

use crate::{AtlassianClient, ISSUE_KEY_PATTERN, Operation, adf, invalid, limit, matches, text};

const COMMENT_HELP: &str = "Plain text; line breaks start new paragraphs";

pub(crate) fn operation(name: &str) -> Option<Operation> {
    let object = |properties: Value, required: &[&str]| json!({ "type": "object", "properties": properties, "required": required });
    let issue_key = json!({ "type": "string", "description": "Issue key, e.g. ABC-123" });
    Some(match name {
        "jira_search_issues" => Operation {
            name: "jira_search_issues",
            description: "Search Jira issues with JQL. Returns key, summary, status and assignee per issue.",
            parameters: object(
                json!({
                    "jql": { "type": "string", "description": "JQL query, e.g. project = ABC AND status = 'In Progress'" },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum number of issues (1-50)" }
                }),
                &["jql", "max_results"],
            ),
            run: |client, args| search(client, args).boxed(),
        },
        "jira_get_issue" => Operation {
            name: "jira_get_issue",
            description: "Get a Jira issue with all its fields",
            parameters: object(json!({ "issue_key": issue_key }), &["issue_key"]),
            run: |client, args| get_issue(client, args).boxed(),
        },
        "jira_create_issue" => Operation {
            name: "jira_create_issue",
            description: "Create a Jira issue",
            parameters: object(
                json!({
                    "project_key": { "type": "string", "description": "Project key, e.g. ABC" },
                    "issue_type": { "type": "string", "description": "Issue type name, e.g. Task, Bug, Story" },
                    "summary": { "type": "string", "description": "One-line summary" },
                    "description": { "type": "string", "description": COMMENT_HELP }
                }),
                &["project_key", "issue_type", "summary", "description"],
            ),
            run: |client, args| create_issue(client, args).boxed(),
        },
        "jira_add_comment" => Operation {
            name: "jira_add_comment",
            description: "Add a comment to a Jira issue",
            parameters: object(
                json!({ "issue_key": issue_key, "comment": { "type": "string", "description": COMMENT_HELP } }),
                &["issue_key", "comment"],
            ),
            run: |client, args| add_comment(client, args).boxed(),
        },
        "jira_transition_issue" => Operation {
            name: "jira_transition_issue",
            description: "Move a Jira issue to another status",
            parameters: object(
                json!({ "issue_key": issue_key, "status_name": { "type": "string", "description": "Target status name, e.g. Done" } }),
                &["issue_key", "status_name"],
            ),
            run: |client, args| transition(client, args).boxed(),
        },
        _ => return None,
    })
}

async fn search(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let jql = text("jira_search_issues", args, "jql")?;
    let query = [
        ("jql", jql.to_owned()),
        ("maxResults", limit(args, "max_results", 10).to_string()),
        ("fields", "summary,status,assignee".to_owned()),
    ];
    let response = client
        .call(Method::GET, "/rest/api/3/search/jql", &query, None)
        .await?;
    Ok(response.text())
}

/// The key of the issue, or the text that goes back to the model instead of a request.
fn issue_key<'a>(tool: &str, args: &'a Value) -> Result<Result<&'a str, String>, ToolError> {
    let key = text(tool, args, "issue_key")?;
    Ok(if matches(ISSUE_KEY_PATTERN, key) {
        Ok(key)
    } else {
        Err(invalid("issue_key", key))
    })
}

async fn get_issue(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let key = match issue_key("jira_get_issue", args)? {
        Ok(key) => key,
        Err(message) => return Ok(message),
    };
    let path = format!("/rest/api/3/issue/{key}");
    Ok(client.call(Method::GET, &path, &[], None).await?.text())
}

async fn create_issue(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let tool = "jira_create_issue";
    let body = json!({ "fields": {
        "project": { "key": text(tool, args, "project_key")? },
        "issuetype": { "name": text(tool, args, "issue_type")? },
        "summary": text(tool, args, "summary")?,
        "description": adf::document(text(tool, args, "description")?),
    } });
    let response = client
        .call(Method::POST, "/rest/api/3/issue", &[], Some(body))
        .await?;
    Ok(response.text())
}

async fn add_comment(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let key = match issue_key("jira_add_comment", args)? {
        Ok(key) => key,
        Err(message) => return Ok(message),
    };
    let body = json!({ "body": adf::document(text("jira_add_comment", args, "comment")?) });
    let path = format!("/rest/api/3/issue/{key}/comment");
    Ok(client
        .call(Method::POST, &path, &[], Some(body))
        .await?
        .text())
}

async fn transition(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let tool = "jira_transition_issue";
    let key = match issue_key(tool, args)? {
        Ok(key) => key,
        Err(message) => return Ok(message),
    };
    let wanted = text(tool, args, "status_name")?;
    let path = format!("/rest/api/3/issue/{key}/transitions");
    let response = client.call(Method::GET, &path, &[], None).await?;
    if !response.is_success() {
        return Ok(response.text());
    }
    let parsed: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
    let transitions = parsed["transitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let name_of = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let same = |name: &Value| name_of(name).to_lowercase() == wanted.to_lowercase();
    let Some(chosen) = transitions
        .iter()
        .find(|t| same(&t["name"]) || same(&t["to"]["name"]))
    else {
        let available: Vec<String> = transitions.iter().map(|t| name_of(&t["name"])).collect();
        return Ok(format!(
            "No transition to '{wanted}'. Available: {}",
            available.join(", ")
        ));
    };
    let body = json!({ "transition": { "id": name_of(&chosen["id"]) } });
    Ok(client
        .call(Method::POST, &path, &[], Some(body))
        .await?
        .text())
}
