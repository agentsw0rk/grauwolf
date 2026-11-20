//! Jira and Confluence Cloud tools (ADR-0104): REST calls with Basic auth; every result is
//! `HTTP <status>\n<body>`, also for errors, so the model sees what went wrong.

mod adf;
mod confluence;
mod jira;

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use grauwolf_prompt::ToolDescriptor;
use grauwolf_tools::{DynTool, ToolError};
use regex::Regex;
use serde_json::Value;

/// Names of the Jira tools.
pub const JIRA_TOOLS: &[&str] = &[
    "jira_search_issues",
    "jira_get_issue",
    "jira_create_issue",
    "jira_add_comment",
    "jira_transition_issue",
];

/// Names of the Confluence tools.
pub const CONFLUENCE_TOOLS: &[&str] = &[
    "confluence_search_pages",
    "confluence_get_page",
    "confluence_create_page",
    "confluence_update_page",
];

/// Time to connect to the site.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time for one request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// A site and its credentials; the `Debug` output never shows the credentials.
#[derive(Clone)]
pub struct AtlassianClient {
    base_url: String,
    auth: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for AtlassianClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtlassianClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl AtlassianClient {
    /// A client for `base_url`, reduced to scheme and host (a pasted `…/wiki` URL still
    /// works), with Basic auth from `email` and `api_token`.
    pub fn new(base_url: &str, email: &str, api_token: &str) -> Result<Self, String> {
        let url = reqwest::Url::parse(base_url)
            .map_err(|error| format!("invalid Atlassian base URL: {error}"))?;
        let host = url
            .host_str()
            .ok_or_else(|| "invalid Atlassian base URL: no host".to_owned())?;
        let mut reduced = format!("{}://{host}", url.scheme());
        if let Some(port) = url.port() {
            reduced.push_str(&format!(":{port}"));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            base_url: reduced,
            auth: format!(
                "Basic {}",
                base64(format!("{email}:{api_token}").as_bytes())
            ),
            http,
        })
    }

    /// Like [`new`](Self::new) with `ATLASSIAN_EMAIL` and `ATLASSIAN_API_TOKEN` from `env`;
    /// a missing or blank variable is an error.
    pub fn from_env(base_url: &str, env: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let get = |name: &str| {
            env(name)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("{name} is not set"))
        };
        Self::new(
            base_url,
            &get("ATLASSIAN_EMAIL")?,
            &get("ATLASSIAN_API_TOKEN")?,
        )
    }

    /// The site as scheme and host.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The Jira tools with the names `names`; unknown names are an error.
    pub fn jira_tools<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<Arc<dyn DynTool>>, String> {
        self.tools(names, jira::operation)
    }

    /// The Confluence tools with the names `names`; unknown names are an error.
    pub fn confluence_tools<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
    ) -> Result<Vec<Arc<dyn DynTool>>, String> {
        self.tools(names, confluence::operation)
    }

    fn tools<'n>(
        &self,
        names: impl IntoIterator<Item = &'n str>,
        lookup: fn(&str) -> Option<Operation>,
    ) -> Result<Vec<Arc<dyn DynTool>>, String> {
        names
            .into_iter()
            .map(|name| {
                let operation =
                    lookup(name).ok_or_else(|| format!("unknown built-in tool '{name}'"))?;
                Ok(Arc::new(AtlassianTool {
                    client: self.clone(),
                    operation,
                }) as Arc<dyn DynTool>)
            })
            .collect()
    }

    /// One request; the result is `HTTP <status>\n<body>`.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Response, ToolError> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .header("Authorization", &self.auth)
            .header("Accept", "application/json")
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let failed = |error: reqwest::Error| {
            ToolError::Execution(format!(
                "request to {} failed: {}",
                self.base_url,
                error.without_url()
            ))
        };
        let response = request.send().await.map_err(failed)?;
        let status = response.status().as_u16();
        let body = response.text().await.map_err(failed)?;
        Ok(Response { status, body })
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Status and body of a response.
pub(crate) struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// `HTTP <status>\n<body>`.
    pub fn text(&self) -> String {
        format!("HTTP {}\n{}", self.status, self.body)
    }
}

/// What a tool does: its description and how it runs.
pub(crate) struct Operation {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
    pub run: for<'a> fn(&'a AtlassianClient, &'a Value) -> BoxFuture<'a, Result<String, ToolError>>,
}

struct AtlassianTool {
    client: AtlassianClient,
    operation: Operation,
}

impl DynTool for AtlassianTool {
    fn name(&self) -> &str {
        self.operation.name
    }

    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.operation.name.to_owned(),
            description: self.operation.description.to_owned(),
            parameters: self.operation.parameters.clone(),
        }
    }

    fn call_json(&self, args: Value) -> BoxFuture<'_, Result<String, ToolError>> {
        Box::pin(async move { (self.operation.run)(&self.client, &args).await })
    }
}

/// A required text argument.
pub(crate) fn text<'a>(tool: &str, args: &'a Value, field: &str) -> Result<&'a str, ToolError> {
    args[field].as_str().ok_or_else(|| ToolError::InvalidArgs {
        tool: tool.to_owned(),
        reason: format!("missing field '{field}' (text)"),
    })
}

/// A text argument that may be missing.
pub(crate) fn optional_text<'a>(args: &'a Value, field: &str) -> &'a str {
    args[field].as_str().unwrap_or_default()
}

/// A count in 1–50, `default` if missing.
pub(crate) fn limit(args: &Value, field: &str, default: i64) -> i64 {
    args[field].as_i64().unwrap_or(default).clamp(1, 50)
}

/// The text of a request that fails before it is sent: `Invalid <field>: <value>`.
pub(crate) fn invalid(field: &str, value: &str) -> String {
    format!("Invalid {field}: {value}")
}

/// `value` matches `pattern` completely.
pub(crate) fn matches(pattern: &str, value: &str) -> bool {
    Regex::new(&format!("^(?:{pattern})$")).is_ok_and(|regex| regex.is_match(value))
}

/// Pattern of issue keys.
pub(crate) const ISSUE_KEY_PATTERN: &str = "[A-Za-z][A-Za-z0-9_]*-[0-9]+|[0-9]+";
/// Pattern of page IDs.
pub(crate) const PAGE_ID_PATTERN: &str = "[0-9]+";
