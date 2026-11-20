# grauwolf-atlassian

**Layer:** `adapters` · **Status:** `AtlassianClient::new(base_url, email, api_token)` / `from_env(base_url, env)` (`ATLASSIAN_EMAIL`, `ATLASSIAN_API_TOKEN`; missing or blank → `<NAME> is not set`) reduces the base URL to scheme, host and port (a pasted `…/wiki` URL works), sends Basic auth and provides `DynTool`s via `jira_tools(names)` and `confluence_tools(names)` (`JIRA_TOOLS`, `CONFLUENCE_TOOLS`; unknown names → error). Jira (REST v3): `jira_search_issues { jql, max_results }` (`GET /rest/api/3/search/jql`, `maxResults` clamped to 1–50, `fields=summary,status,assignee`), `jira_get_issue { issue_key }`, `jira_create_issue { project_key, issue_type, summary, description }` (ADF), `jira_add_comment { issue_key, comment }` (ADF), `jira_transition_issue { issue_key, status_name }` (reads the transitions, picks the one whose name or target status matches ignoring case, otherwise `No transition to '<name>'. Available: …`). Confluence: `confluence_search_pages { cql, limit }` (`/wiki/rest/api/search`), `confluence_get_page { page_id }` (`/wiki/api/v2/pages/<id>?body-format=storage`), `confluence_create_page { space_key, title, body, parent_id }` (space ID from `/wiki/api/v2/spaces?keys=…`, otherwise `Space '<key>' not found`; empty `parent_id` → top level), `confluence_update_page { page_id, title, body }` (reads the version, `PUT` with version + 1). Results are `HTTP <status>\n<body>`, also for errors; an invalid issue key (`[A-Za-z][A-Za-z0-9_]*-[0-9]+|[0-9]+`) or page ID (`[0-9]+`) returns `Invalid <field>: <value>` without a request; a network error is `ToolError::Execution`. ADF: one paragraph per line (`\n`, `\r\n`, `\r`), blank lines as empty paragraphs. Timeouts `CONNECT_TIMEOUT` 10 s, `REQUEST_TIMEOUT` 60 s. Tests with wiremock (`tests/jira.rs`, `tests/confluence.rs`, `tests/client.rs`).

## Purpose
Built-in tools for Jira and Confluence Cloud (E14 F14.17, ADR-0104), behaving like AgentWeaver's.

## Features / ADRs
E14 F14.17, ADR-0104

## Allowed dependencies
- grauwolf crates: grauwolf-prompt, grauwolf-tools
- External: futures-util, regex, reqwest, serde_json; dev: tokio, wiremock

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- Credentials appear in no result, error text or `Debug` output.
- IDs that go into a URL path are checked against their pattern before the request.
- The `https://` rule belongs to the spec; the client accepts any scheme so tests can use a mock server.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-atlassian`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
