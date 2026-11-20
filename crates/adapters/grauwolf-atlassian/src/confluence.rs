//! The Confluence tools (REST API v2, search v1).

use futures_util::FutureExt;
use grauwolf_tools::ToolError;
use reqwest::Method;
use serde_json::{Value, json};

use crate::{
    AtlassianClient, Operation, PAGE_ID_PATTERN, invalid, limit, matches, optional_text, text,
};

const BODY_HELP: &str = "Page content in Confluence storage format (XHTML, e.g. <p>Text</p>)";

pub(crate) fn operation(name: &str) -> Option<Operation> {
    let object = |properties: Value, required: &[&str]| json!({ "type": "object", "properties": properties, "required": required });
    let page_id = json!({ "type": "string", "description": "Page ID" });
    Some(match name {
        "confluence_search_pages" => Operation {
            name: "confluence_search_pages",
            description: "Search Confluence content with CQL. Returns id, title and excerpt per result.",
            parameters: object(
                json!({
                    "cql": { "type": "string", "description": "CQL query, e.g. type = page AND text ~ \"onboarding\"" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum number of results (1-50)" }
                }),
                &["cql", "limit"],
            ),
            run: |client, args| search(client, args).boxed(),
        },
        "confluence_get_page" => Operation {
            name: "confluence_get_page",
            description: "Get a Confluence page with its content in storage format",
            parameters: object(json!({ "page_id": page_id }), &["page_id"]),
            run: |client, args| get_page(client, args).boxed(),
        },
        "confluence_create_page" => Operation {
            name: "confluence_create_page",
            description: "Create a Confluence page",
            parameters: object(
                json!({
                    "space_key": { "type": "string", "description": "Space key, e.g. TEAM" },
                    "title": { "type": "string", "description": "Page title" },
                    "body": { "type": "string", "description": BODY_HELP },
                    "parent_id": { "type": "string", "description": "ID of the parent page, empty for a top-level page" }
                }),
                &["space_key", "title", "body", "parent_id"],
            ),
            run: |client, args| create_page(client, args).boxed(),
        },
        "confluence_update_page" => Operation {
            name: "confluence_update_page",
            description: "Replace title and content of a Confluence page",
            parameters: object(
                json!({
                    "page_id": page_id,
                    "title": { "type": "string", "description": "Page title" },
                    "body": { "type": "string", "description": BODY_HELP }
                }),
                &["page_id", "title", "body"],
            ),
            run: |client, args| update_page(client, args).boxed(),
        },
        _ => return None,
    })
}

fn storage(value: &str) -> Value {
    json!({ "representation": "storage", "value": value })
}

async fn search(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let cql = text("confluence_search_pages", args, "cql")?;
    let query = [
        ("cql", cql.to_owned()),
        ("limit", limit(args, "limit", 10).to_string()),
    ];
    let response = client
        .call(Method::GET, "/wiki/rest/api/search", &query, None)
        .await?;
    Ok(response.text())
}

async fn get_page(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let id = text("confluence_get_page", args, "page_id")?;
    if !matches(PAGE_ID_PATTERN, id) {
        return Ok(invalid("page_id", id));
    }
    let path = format!("/wiki/api/v2/pages/{id}");
    let query = [("body-format", "storage".to_owned())];
    Ok(client.call(Method::GET, &path, &query, None).await?.text())
}

async fn create_page(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let tool = "confluence_create_page";
    let space_key = text(tool, args, "space_key")?;
    let spaces = client
        .call(
            Method::GET,
            "/wiki/api/v2/spaces",
            &[("keys", space_key.to_owned())],
            None,
        )
        .await?;
    if !spaces.is_success() {
        return Ok(spaces.text());
    }
    let parsed: Value = serde_json::from_str(&spaces.body).unwrap_or(Value::Null);
    let Some(space_id) = parsed["results"][0]["id"].as_str() else {
        return Ok(format!("Space '{space_key}' not found"));
    };
    let mut page = json!({
        "spaceId": space_id,
        "status": "current",
        "title": text(tool, args, "title")?,
        "body": storage(text(tool, args, "body")?),
    });
    let parent = optional_text(args, "parent_id");
    if !parent.trim().is_empty() {
        page["parentId"] = json!(parent);
    }
    let response = client
        .call(Method::POST, "/wiki/api/v2/pages", &[], Some(page))
        .await?;
    Ok(response.text())
}

async fn update_page(client: &AtlassianClient, args: &Value) -> Result<String, ToolError> {
    let tool = "confluence_update_page";
    let id = text(tool, args, "page_id")?;
    if !matches(PAGE_ID_PATTERN, id) {
        return Ok(invalid("page_id", id));
    }
    let path = format!("/wiki/api/v2/pages/{id}");
    let current = client.call(Method::GET, &path, &[], None).await?;
    if !current.is_success() {
        return Ok(current.text());
    }
    let parsed: Value = serde_json::from_str(&current.body).unwrap_or(Value::Null);
    let Some(version) = parsed["version"]["number"].as_i64() else {
        return Err(ToolError::Execution(
            "the page has no version number in the response".to_owned(),
        ));
    };
    let page = json!({
        "id": id,
        "status": "current",
        "title": text(tool, args, "title")?,
        "body": storage(text(tool, args, "body")?),
        "version": { "number": version + 1 },
    });
    Ok(client
        .call(Method::PUT, &path, &[], Some(page))
        .await?
        .text())
}
