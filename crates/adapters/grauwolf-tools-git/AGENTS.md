# grauwolf-tools-git

**Layer:** `adapters` · **Status:** `GitTools::new(GitConfig)` (ADR-0104, F14.17) provides `git_checkout { ref }`, `git_list_branches {}`, `git_create_branch { name }`, `git_commit { message }`, `git_push {}` (`GIT_TOOLS`) for one repository, implemented with libgit2 (`git2`, vendored libgit2, statically linked, without its HTTPS/OpenSSL support): no `git` program at run time; `http` and `https` remotes are served by the own smart-HTTP transport of the shared adapter `grauwolf-git` (`transport.rs`, reqwest blocking client with rustls; since ADR-0107 stage 3, moved out of this crate unchanged). `GitConfig { repo_url, directory, push_policy (NewBranches | Any), token, author_name, author_email, network_timeout (120 s), local_timeout (60 s) }`. Refusals and failures are returned as text, never as `ToolError`: `Refused: …`, `No checkout yet. Call checkout first.`, `Nothing to commit`, `Unknown ref: <ref>`, `Invalid branch name: <name>`, `Push failed: <reason>`, `Error: git <command> failed: <reason>` (`clone`, `fetch`, `ls-remote`, `branch`, `commit`, …; AgentWeaver's texts). Results `On branch <b> at <hash>`, `Detached at <hash>`, `Committed <hash7> on branch <b>`, `Pushed <b>`, `branches: …\ntags: …`. libgit2 is blocking: each call runs in `spawn_blocking`, holding the process-wide lock of its directory, so calls on one directory run one after another; the branches created by `git_create_branch` are remembered process-wide per directory (the push policy `NewBranches` allows only these). Timeouts: the call is abandoned with `timed out after …` after `network_timeout` (`local_timeout` for `git_commit`); `spawn_blocking` cannot be cancelled, so the lock stays with the abandoned call until libgit2 returns, which the HTTP timeouts of the transport (connect `network_timeout`, whole request `network_timeout` + 1 s) guarantee; libgit2's socket timeouts are not set because its own network code never runs. Remote guard before fetch, ls-remote and push: `.git/config` must name exactly `repo_url` as `remote.origin.url`, no `pushurl`, no `url.*.insteadOf`/`pushInsteadOf`. libgit2 never reads the system and user configuration (search paths emptied once) and runs no hooks. Transport (in grauwolf-git): registered once per process for `http`/`https` (`unsafe` and global: libgit2 keeps the transports in an unlocked vector, so it happens in `GitTools::new` before the first network call); per-call settings (token, scheme and host of `repo_url`, timeout) travel in a thread-local set around each blocking call. `GET …/info/refs?service=…`, `POST …/git-upload-pack|git-receive-pack`, `Content-Type` of the listing checked (smart protocol only), no redirect is followed (any 3xx is `the server answered with a redirect (HTTP n); redirects are not followed`; libgit2's own redirect code crashes on a `Location` without query), 401/403/404 named, no retry. Token: `Authorization: Basic` (user `x-access-token`, in memory) on every request, only to the scheme and host of `repo_url`, never in URLs, results or logs (masked as `***` in every error text). The `https://` rule belongs to the spec; the tools accept every URL libgit2 understands (tests use local bare repositories and a small HTTP server under cargo's `CARGO_TARGET_TMPDIR`).

## Purpose
The git tools for one repository (E14 F14.17), split off `grauwolf-builtin-tools` in ADR-0108 (stage 3a) so that only builds that need libgit2 pay for it.

## Features / ADRs
E14 F14.17, ADR-0104; ADR-0108

## Allowed dependencies
- grauwolf crates: grauwolf-git (transport, libgit2 settings), grauwolf-prompt, grauwolf-spiffe, grauwolf-tools
- External: futures-util, git2 (libgit2 built from the vendored C sources, no `https`/OpenSSL feature; needs a C compiler at build time), serde_json, tokio (rt, sync, time); dev: tokio

Other dependencies only after checking against [ADR-0016](../../../docs/adr/0016-layered-architecture.md).

## Invariants
- The git token appears in no URL, result or log; it reaches the network only as the `Authorization` header of the own transport, only for the scheme and host of the configured URL, and no redirect is ever followed.
- Transport and token invariants are those of grauwolf-git. `git2` is used by this crate, grauwolf-git and what depends on them.

## Tests
- `tests/git.rs`: bare repositories under `CARGO_TARGET_TMPDIR/git-tests` (not below the workspace: a mounted volume showed vanishing directories under parallel tests). The tests need no `git` program.

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- `cargo test -p grauwolf-tools-git`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.

ADR-0110 stage 1: `GitConfig.auth: Option<Arc<ClientAuth>>` replaces `GIT_TOKEN`: mTLS in the own transport (server's SPIFFE ID checked) and/or, in jwt mode, a fresh JWT-SVID fetched before every blocking call (`Settings.bearer`). Errors of the transport say `TLS handshake with <host> failed: <reason>`, `access denied (HTTP 403)`. Tests: `tests/spiffe.rs` against the test git server in its mTLS mode (clone and push with the right identity, read-only and unmapped identities get 403, wrong expected server ID, jwt bearer).
