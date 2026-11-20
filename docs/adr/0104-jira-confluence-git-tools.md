# ADR-0104: Built-in tools for Jira, Confluence and Git

- **Status:** Accepted (User, 2026-11-10: "can you implement ADR-0104 – Jira, Confluence, Git"; ADR-0092, F14.17)
- **Date:** 2026-11-10

## Context
AgentWeaver offers built-in tools for Jira, Confluence (Atlassian Cloud) and Git, and its agents use
them a lot (ticket triage, documentation, code changes). For a grauwolf harness they must behave the
same, so exported graphs keep working. Alternative considered: existing MCP servers. They need a
second runtime (Python or Node) or OAuth, their tool names and options differ, and images would grow.

AgentWeaver's behavior, which grauwolf follows:
- **Atlassian:** base URL reduced to scheme and host (a pasted `…/wiki` URL still works); Basic auth
  from `ATLASSIAN_EMAIL` and `ATLASSIAN_API_TOKEN`, read at start-up (missing → fail before the run);
  results are `HTTP <status>\n<body>`, also for errors, so the model sees what went wrong; issue keys
  must match `[A-Za-z][A-Za-z0-9_]*-\d+|\d+`, page ids `\d+`; texts for Jira go out as Atlassian
  Document Format (one paragraph per line, blank lines as empty paragraphs).
- **Git:** one fixed repository per tool node (`repoUrl`, `directory`, default `./repo`); read-only
  by default (only checkout and list); push policy `new-branches` (default; only branches the agent
  created with `createBranch` in this process) or `any`; token only from `GIT_TOKEN` (user
  `x-access-token`), optional (`useToken`); author from `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL`; calls
  on one directory run one after another; failures go back to the model as text; network calls time
  out after 120 s.

## Decision
- **New adapter crate `grauwolf-atlassian`** (reqwest) and **git tools in `grauwolf-builtin-tools`**
  (they run the `git` program; pure-Rust git libraries lack push, libgit2 adds C and TLS
  dependencies). All are external tools in the sense of ADR-0050, provided by the facade
  (`external::tools`), so interpreter, CLI, service and generated crates get them alike.
- **Spec** (`builtin_tools`, names in `tools` as for the other built-in tools):
  ```json
  "builtin_tools": {
    "tools": ["jira_search_issues", "jira_get_issue", "confluence_get_page", "git_checkout", "git_list_branches"],
    "jira": { "base_url": "https://example.atlassian.net" },
    "confluence": { "base_url": "https://example.atlassian.net" },
    "git": { "repo_url": "https://github.com/org/repo.git", "directory": "repo",
             "push_policy": "new_branches", "use_token": true }
  }
  ```
  Validation: `base_url` and `repo_url` must be `https://`; `repo_url` without user info; a tool
  name of a group requires that group's section; `directory` relative to the root of
  `builtin_tools` (or the spec directory) and inside it; `push_policy` `new_branches` | `any`. Writing
  git tools are simply the ones the spec lists (AgentWeaver's `readOnly: true` → only `git_checkout`
  and `git_list_branches`).
- **Jira tools** (REST API v3):
  - `jira_search_issues { jql, max_results }` – `GET /rest/api/3/search/jql?jql=…&maxResults=<1–50>&fields=summary,status,assignee`
  - `jira_get_issue { issue_key }` – `GET /rest/api/3/issue/<key>`
  - `jira_create_issue { project_key, issue_type, summary, description }` – `POST /rest/api/3/issue`
    (description as ADF)
  - `jira_add_comment { issue_key, comment }` – `POST /rest/api/3/issue/<key>/comment` (ADF)
  - `jira_transition_issue { issue_key, status_name }` – reads the transitions and picks the one whose
    name or target status matches case-insensitively; none → `No transition to '<name>'. Available: …`
- **Confluence tools:**
  - `confluence_search_pages { cql, limit }` – `GET /wiki/rest/api/search?cql=…&limit=<1–50>`
  - `confluence_get_page { page_id }` – `GET /wiki/api/v2/pages/<id>?body-format=storage`
  - `confluence_create_page { space_key, title, body, parent_id }` – space id from
    `GET /wiki/api/v2/spaces?keys=…` (`Space '<key>' not found`), then `POST /wiki/api/v2/pages` with
    storage format; empty `parent_id` → top level
  - `confluence_update_page { page_id, title, body }` – reads the version, `PUT` with version + 1
- **Git tools** (`git` program, no shell, `GIT_TERMINAL_PROMPT=0`, timeout 120 s per network call):
  - `git_checkout { ref }` – clone on the first call, otherwise fetch (with prune); refuses with
    uncommitted changes, with unpushed commits when leaving a branch, and with a branch that diverged
    from the remote; a remote branch is created/fast-forwarded locally; a local-only branch is
    switched to; otherwise a tag or a commit hash (4–40 hex) is checked out detached; else
    `Unknown ref: <ref>`. Result: `On branch <b> at <hash>` or `Detached at <hash>`.
  - `git_list_branches {}` – `branches: …\ntags: …` from `ls-remote` (sorted).
  - `git_create_branch { name }` – valid ref name, not existing locally or remotely; creates it at
    the current commit, switches to it and remembers it as created by the agent (process-wide, per
    directory).
  - `git_commit { message }` – on a branch only; stages everything (including deletions);
    `Nothing to commit`; author/committer from the environment, defaults `grauwolf agent` /
    `agent@grauwolf.local`. Result `Committed <hash7> on branch <b>`.
  - `git_push {}` – the current branch to the same name on `origin`, never with force; with
    `new_branches` only branches created by `git_create_branch`; result `Pushed <b>` or
    `Push failed: …`.
  - **Remote guard:** before fetch, `ls-remote` and push, `.git/config` must still name exactly
    `repo_url` as `remote.origin.url`, have no `pushurl` and no `url.*.insteadOf`/`pushInsteadOf`;
    otherwise the call is refused – a file tool on the same directory must not redirect the token.
  - **Token:** `GIT_TOKEN` is passed through `GIT_ASKPASS` (a helper that prints it), never in URLs,
    arguments, results or logs; the helper and environment are only set for network commands.
  - Messages to the model are the same texts as in AgentWeaver (refusals start with `Refused:`);
    errors of the `git` program are returned as `Error: git <command> failed: <stderr, token
    masked>`.
- **Name mapping in the AgentWeaver export:** `searchIssues` → `jira_search_issues`, `getIssue` →
  `jira_get_issue`, … , `checkout` → `git_checkout`, `listBranches` → `git_list_branches`,
  `createBranch` → `git_create_branch`, `commit` → `git_commit`, `push` → `git_push`;
  `readOnly`, `pushPolicy` (`new-branches` → `new_branches`), `useToken`, `directory`, `baseUrl`,
  `repoUrl` 1:1.
- **Images:** services that use git tools need the `git` program; their runtime base is Debian slim
  with `git` instead of distroless (ADR-0096).

## Consequences
- AgentWeaver graphs with Jira, Confluence and Git export without extra processes; the model sees
  the same tools and messages.
- These are the first built-in tools with side effects on third-party systems; tests use HTTP mocks
  (Atlassian) and local bare repositories (git; the https rule is relaxed only through the Rust API
  for tests).
- Jira/Confluence Server and Data Center (API v2, personal access tokens) are not covered.
- The git tools depend on a `git` program at run time; without it they fail with a clear error
  before the run.

## Implementation notes
Implemented as decided (spec sections, `grauwolf-atlassian`, git tools in `grauwolf-builtin-tools`
under `external::tools`). Details and small deviations:
- `use_token` defaults to `false` (AgentWeaver treats a missing value as no token); `directory`
  defaults to `repo`; the git section is also valid without `root` (then the spec directory).
- Spec errors added: `MissingBuiltinSection`, `InvalidUrl` (never repeats the URL, so a password in
  it does not end up in a message) and `DirectoryOutsideRoot`; `base_url` also rejects user info.
- Refusal texts are AgentWeaver's verbatim, including `Call checkout first.` and `createBranch`
  although the tools are called `git_checkout` and `git_create_branch`.
- `git` runs with `core.hooksPath=/dev/null`, `core.fsmonitor=false` and `credential.helper=`
  because file tools may write into `.git`; with a token the system and user git configuration is
  ignored for the network commands. The token reaches the helper as `GRAUWOLF_GIT_TOKEN` in the
  environment of that one `git` process; a missing credential before the run is checked in
  `builtin::tools_with_env` (also whether `git` can be started), before file tools are set up.
- Errors of the `git` program are returned as text (`Error: git <command> failed: …`), refusals too;
  only missing arguments are `ToolError::InvalidArgs`. `git_push` failures are `Push failed: …`.
- Local commands time out after 60 s, network commands after 120 s (`GitConfig`).
- Not done: the Debian slim runtime image for services with git tools (ADR-0096 images are not
  implemented in this repository yet). Jira and Confluence were tested against mocks only; no
  Atlassian account was available.

## Addendum (2026-11-11): git2 instead of the `git` program
User: "switch to git2" – the git tools should work like the Jira tools: everything inside the
program, nothing extra in the image (the JVM backend of AgentWeaver uses a git library as well).
- **Library:** `git2` (bindings to libgit2) with `vendored-libgit2` and `vendored-openssl`, linked
  statically into the binary. `gix` was checked again and still lacks push and a complete checkout
  workflow.
- **Behavior unchanged:** tool names, arguments, all refusal and result texts, the remote guard on
  `.git/config`, the push policy, the per-directory lock and the created-branches registry stay as
  they are; the existing tests stay and must pass.
- **Token:** passed to libgit2 through the credentials callback (user `x-access-token`), in memory
  only – no helper program, no environment variable. libgit2 runs no hooks, so the hook-related git
  options of the first implementation are no longer needed.
- **Consequences:** no `git` program at run time: the check for it before the run, the `runtime-git`
  image variant and the git package in the CLI image are dropped (ADR-0096); services with git
  tools use the distroless runtime like all others. The first build compiles libgit2 and OpenSSL
  from C sources (pre-warmed in the builder image).
- **Details of the implementation:** libgit2 is blocking, so every tool call runs in
  `spawn_blocking` holding the directory lock; the 120 s limit is a `tokio::time::timeout` around
  it (`timed out after …`), and because a blocking call cannot be cancelled, the lock stays with
  the abandoned call and the global libgit2 socket timeouts are set to the limit plus one second
  (global to the process, the last `GitTools::new` wins). The credentials callback answers once per
  call and only for the scheme and host of `repo_url`, so a redirect cannot receive the token and a
  rejected token cannot loop. The system and user git configuration is never read (libgit2 search
  paths emptied once), which replaces `GIT_CONFIG_NOSYSTEM`/`GIT_CONFIG_GLOBAL`; the owner check
  of the directory is off. After a clone `remote.origin.url` is set to exactly `repo_url` (libgit2
  may normalize local paths). `git_commit` adds a trailing newline to the message like `git commit
  -m`. The tests no longer need the `git` program and keep their fixtures under cargo's
  `CARGO_TARGET_TMPDIR`: below the workspace, on a mounted volume, directories vanished for a
  moment under parallel tests. The tests that used a fake `git` program were replaced by tests
  against a small local HTTP server (credential flow, no retry loop, no token for a redirect target,
  timeout). libgit2 1.9.7 crashes (segmentation fault) on an HTTP redirect whose `Location` does
  not keep the request path (`…/info/refs?service=git-upload-pack`); not worked around here,
  fixed by the addendum of 2026-11-11 below.
- **Measured:** the release binary of the CLI grew from 45.2 MB to 52.9 MB (+7.7 MB) with libgit2
  and OpenSSL linked in. Live check (2026-11-11): `git_list_branches` and `git_checkout` against
  the public `https://github.com/agentsw0rk/RKEV.git` (no token) with `qwen3.5:9b` cloned the
  repository over HTTPS with `PATH=/nonexistent` (no `git` program reachable) and the model
  answered with the first heading and the branch list. The images were not built (no Docker here).

## Addendum (2026-11-11): own HTTP transport instead of libgit2's
User: "fix the libgit2 crash". libgit2 1.9.7 dereferences a missing query in `remove_service_suffix`
when it applies a redirect (`git_net_url_apply_redirect`); `RemoteRedirect::None` only blocks
redirects to other hosts, so a same-host `Location` without `?service=…` still crashes the process.
- **Decision:** the git tools register their own smart-HTTP subtransport for `http` and `https`
  (`git2::transport::register`, once per process) built on reqwest (blocking client with rustls),
  so libgit2's HTTP code never runs. It speaks the smart protocol (`GET …/info/refs?service=…`,
  `POST …/git-upload-pack` / `git-receive-pack`), follows **no** redirect (any 3xx is an error
  naming the status), sets `Authorization: Basic` itself only for the scheme and host of
  `repo_url` (user `x-access-token`, sent with the first request, not after a 401 challenge), and
  uses reqwest's connect and total timeouts (the call's limit plus one second as a safety net, the
  call's own limit fires first), which really end a hanging connection. Proxies from the
  environment keep working (reqwest). Only the smart protocol is supported: the `Content-Type` of
  the listing must be `application/x-git-<service>-advertisement`.
- **Consequences:** the credentials callback is no longer needed for HTTP; git2 is built without its
  `https` feature, so OpenSSL is not compiled any more (no `perl`/`make` in image builds, smaller
  binary). Local paths and `file://` remotes still use libgit2's own local transport (tests).
- **Implementation notes:** `git_transport.rs` registers the subtransport for `http` and `https`
  once (`std::sync::Once`; `git2::transport::register` is `unsafe` because libgit2 keeps the
  transports in a vector without a lock, which is why it runs in `GitTools::new` before this crate's
  first network call). The per-call settings (token, scheme and host of `repo_url`, timeout) are a
  thread-local that is set around the blocking libgit2 call; libgit2 creates the transport on that
  thread. The credentials callback, the global socket timeouts of libgit2 and `git2`'s `https` and
  `vendored-openssl` features are gone (`cargo tree -i openssl-sys` finds nothing in the workspace);
  the `perl`/`make` installs are removed from the three Dockerfiles (not verified, no Docker here).
  The crash of the libgit2 redirect is no longer an open point: tests cover `301/302/303/307/308`
  to a same-host path without query, an absolute URL without query and another host with and
  without query, for `git_list_branches`, `git_checkout` (clone) and `git_push`. Errors: `401`
  `authentication failed (HTTP 401)` (once, no retry), `403`, `404`, other statuses by number.
- **Measured:** the release binary of the CLI shrank from 52.9 MB (libgit2 with OpenSSL) to 46.8 MB
  (46,751,968 bytes; 45.2 MB before libgit2 at all). Live check (2026-11-11, `qwen3.5:9b`,
  `OLLAMA_THINK=false`): `git_list_branches` + `git_checkout` against the public
  `https://github.com/agentsw0rk/RKEV.git` listed `main` and cloned the repository through the new
  transport (listing and `POST …/git-upload-pack`); `git_list_branches` against the private
  `https://github.com/agentsw0rk/grauwolf.git` without a token answered
  `Error: git ls-remote failed: authentication failed (HTTP 401)`, no crash.
