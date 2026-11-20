# grauwolf-k8s

**Layer:** `adapters` · **Status:** ADR-0120. The Kubernetes access of `grauwolf-k8-ops`: a small API over kube-rs (rustls, no OpenSSL) that the commands use, so the heavy client stays in one crate.

## Purpose
`Cluster` wraps a `kube::Client`; `Cluster::connect(kubeconfig, context)` selects the kubeconfig context (default: the current one; an unknown one is an error listing the available ones) and keeps the server URL and context for `describe()` (`https://host:6443 (context name)`; `FakeCluster::cluster()` says `https://fake-cluster.test:6443 (context fake-context)`). Objects are plain `serde_json::Value`s addressed by a `Kind` (group, version, plural, namespaced) – no generated types, so `ClusterSPIFFEID` and the built-in kinds are handled the same way. Calls: `get`, `list` (label selector), `ensure` (server-side apply with the field manager `grauwolf-k8-ops`, skipped when the annotation `grauwolf.io/applied-hash` already holds the hash of the desired object: the answer is `Created`, `Updated` or `Unchanged`), `patch` (JSON merge patch of an existing object, for the replica count or a restart annotation; no ownership taken, a missing object is a 404 error), `delete` (propagation `Background`), `pod_log` (last lines), `pod_log_previous` (the log of the run before a restart), `pod_log_stream` (the log in chunks of complete lines to a sink, with `follow` until it ends; ADR-0121), `version`, `can_i` (SelfSubjectAccessReview) and `raw` (any request through the API server, used for the OpenBao service proxy). `hash_of` (the short hash that `ensure` stores) is public so that a program can put one into a pod template annotation (ADR-0121); `kinds` has `INGRESS`, `NETWORK_POLICY` and `CRON_JOB` among the others. `Cluster::connect` reads the kubeconfig (`--kubeconfig`, `KUBECONFIG`, default location, or the in-cluster account). Feature `testing`: `FakeCluster`, an in-memory API server as a tower service (object store, label selectors, namespace cascade, access review rules, a Job outcome simulation, a Deployment simulation (`set_deployment_outcome`: generation, status, one pod `<name>-x1` that is ready, starting or in `CrashLoopBackOff`, with its log), a service-proxy hook, `on_change` hooks after every patch and delete so a test can play controllers, a log of the writes that changed something).

`image_build` (ADR-0129): `kaniko_job(&BuildRequest)` – the build Job of a harness image: one Kaniko container, `backoffLimit: 0`, context `git://<repository>#<ref>#<commit>` with `--context-sub-path` (the project folder), `--destination`, the git token from a Secret as `GIT_USERNAME`/`GIT_PASSWORD`, the registry's `config.json` from a `dockerconfigjson` Secret at `/kaniko/.docker`; named `<name>-build-<first 8 of the commit>`. `registry_secret(name, namespace, server, username, token)` – the `kubernetes.io/dockerconfigjson` Secret Kaniko pushes with (`stringData`, `auth` = base64 of `user:token`). `kaniko_repository(remote)` – `<host>/<path>.git` from an `https://`, `ssh://` or `git@host:path` remote, credentials in the URL dropped (a local path is an error). `run_build(cluster, request, timeout)` – creates the Job (namespace and Secrets must exist), polls it every 0.5 s until it succeeded, failed or `timeout` passed (`BuildFinished`), reads the non-empty log lines of its pod (`job-name=<job>`, up to 2000) and deletes the Job whatever happened (`Build { finished, log }`; test `tests/image_build.rs`); `run_build_with(cluster, request, timeout, on_line)` is the same and also hands each new log line to `on_line` once, in order, with every poll of the Job and at the end (`FakeCluster` `JobOutcome::Running(log)`: a pod with a log whose Job never finishes). The waiting and the log reading are the same as in `grauwolf-k8-ops agent check` (`check.rs`), which could use them. Kaniko runs as root: the build namespace cannot enforce the restricted Pod Security Standard.


`Cluster::connect_text(text, context)` (ADR-0129 B1): the same as `connect` with a kubeconfig given as text (the studio's stored one); both choose the context through `config_of`. `kubeconfig_contexts(text)` (ADR-0129 B1): the contexts of a kubeconfig given as text, each with its cluster's server, and its current context, without connecting (a pasted kubeconfig in grauwolf studio); not a kubeconfig is an error.

## Features / ADRs
ADR-0120, ADR-0121, ADR-0129

## Allowed dependencies
- grauwolf crates: none
- External: base64 (the registry Secret's `auth`), kube (client, rustls-tls, ring; no `runtime`, no `ws`), k8s-openapi (feature `v1_34`, only to select the Kubernetes version), http, http-body-util, bytes, serde_json, sha2, tokio (time); feature `testing`: tower

## Invariants
- Only this crate (and the program on top of it) depends on kube and k8s-openapi; they stay out of the agent runtime.
- `ensure` writes only when the desired object differs from what this tool applied last (hash annotation), so a repeated run changes nothing.
- Errors carry the HTTP status and the API server's message; no object bodies (a ConfigMap or Secret may hold values).

## Working on this crate
- TDD: one test case per cycle, test + implementation + docs in one commit.
- Tests run against `FakeCluster` (`tests/cluster.rs`).
- `cargo test -p grauwolf-k8s`; before the commit `cargo fmt --all` and `cargo xtask check --affected`; before a push `cargo xtask check --full`.
- Update this file when purpose, API, dependencies, or invariants change.
