# ADR-0120: `grauwolf-k8-ops` – setting up SPIRE and OpenBao for agents on Kubernetes

- **Status:** Accepted (User, 2026-11-14: "I would like a grauwolf CLI that lets me configure SPIRE and
  OpenBao – the steps we went through here: set the domain correctly, set key-values for an agent and
  other steps"; kube-rs and clap; decisions Q1–Q8 below)
- **Date:** 2026-11-14
- **Related:** ADR-0109 (SPIFFE for agents), ADR-0110 (credential sources, OpenBao, live check),
  ADR-0118 (a separate lean program instead of a bigger CLI), ADR-0108 (layers, apps order, central
  configuration), ADR-0016 (layers).

## Context
The live check of ADR-0110 needed a series of manual steps against a cluster: reading SPIRE's
configuration to find the trust domain, deploying the SPIRE OIDC discovery provider (in a namespace of
its own, because spire-controller-manager ignores `spire-system`), registering grauwolf pods with a
`ClusterSPIFFEID`, enabling OpenBao's JWT auth method with the provider's JWKS, a KV v2 mount, a policy
and a role bound to the agent's SPIFFE ID, storing a secret, and running a Job that resolves it. Each
step has traps found only live (ignored namespaces, the restricted Pod Security Standard needing a
numeric user, a registration entry that appears seconds after the pod). The example in
`examples/spire-openbao/` documents them, but repeating them by hand is slow and error-prone.

The setup also has a gap: one policy allowed `kv/data/agents/*`, so every agent could read every other
agent's secrets.

## Decision
1. **A program of its own, `grauwolf-k8-ops`** (Q1), in a new app crate beside `grauwolf-tools-app`. It
   depends on kube-rs (with `k8s-openapi`, the feature pinned to the newest Kubernetes version the
   crate supports up to the cluster's 1.34) and clap 4 (derive, as the other programs). The Kubernetes
   client stays out of `grauwolf` and the generated services: `k8s-openapi` is large, and only
   operators need it. It reads the kubeconfig from `--kubeconfig`, `KUBECONFIG` or the default
   location.
2. **One agent, one namespace** (Q2). An agent `<name>` (a DNS-1123 label) lives in the namespace
   `<name>` (`--namespace` overrides the default), labelled `grauwolf.io/agent=<name>` and
   `app.kubernetes.io/managed-by=grauwolf-k8-ops`, with the restricted Pod Security Standard enforced.
   Its service account is `<name>`, so its SPIFFE ID is `spiffe://<trust domain>/ns/<namespace>/sa/<name>`.
   One `ClusterSPIFFEID` (`grauwolf-agents`) selects all namespaces labelled `grauwolf.io/agent`; other
   service accounts in such a namespace get other IDs, which OpenBao refuses. In OpenBao each agent gets
   its **own role and policy** (`agent-<name>`): the role is bound to exactly that SPIFFE ID, and the
   policy only reads `kv/data/agents/<name>/*`. The shared `agents/*` policy is gone.
3. **Reaching OpenBao** (Q3): by default through the Kubernetes API service proxy (only the kubeconfig
   is needed, no ingress); `--openbao-addr` for a direct address. The token comes from `OPENBAO_TOKEN`
   or `--token-file`, never from an argument. The OpenBao HTTP code is shared with the credential
   source of ADR-0110: login and KV read for agents, plus the admin calls (auth methods, mounts,
   policies, roles, KV write, list and delete).
4. **The trust domain has one source**: the SPIRE server's configuration. The program reads it from
   there for every ID it writes. Changing it (Q4) is `trust-domain set <domain>`. Without `--apply` it
   only prints the plan. With `--apply` it first writes a backup of every object it will touch to a
   file. Then it rewrites the SPIRE server, agent and controller-manager configuration, recreates the
   server's data volume, restarts the server and the agents, and rewrites the bound subjects of the
   agent roles in OpenBao. Other systems that contain the domain (for example scion) are not built in.
   They are listed in a configuration file as extra ConfigMaps to rewrite and workloads to restart.
5. **Secrets** (Q5): `secret set <agent> <key>` reads the value only from stdin or a hidden prompt
   and stores it at `kv/agents/<agent>/<key>` (field `value`). Without a terminal it reads stdin only.
   `secret list <agent>` shows keys and versions, never values. `secret delete <agent> <key>` removes
   all versions. Values never appear in output, errors or logs. `--variable NAME` on `secret set` also
   maps the secret to a configuration variable in the agent's `grauwolf.toml` ConfigMap (ADR-0110:
   `NAME = "openbao:kv/agents/<agent>/<key>#value"`).
6. **Repeatable commands** (Q6): every command can run again and only changes what differs. There is
   no desired-state file for now.
7. **Commands, first stage** (Q7):
   - `doctor` (read-only): checks SPIRE server and agents, the controller-manager, the OIDC discovery
     provider and its keys, the OpenBao status, auth method and mount, and that the trust domain agrees
     everywhere it appears (SPIRE server, agents, controller-manager, the provider, the bound subjects
     of the agent roles). It ends with a non-zero exit code when a check fails.
   - `oidc install`: the discovery provider in its own namespace (default `spire-oidc`), with the image
     version of the running SPIRE server and its `ClusterSPIFFEID`.
   - `openbao setup`: the JWT auth method with the provider's `jwks_url`, and the KV v2 mount.
   - `agent add <name>`: the namespace, the service account, the `ClusterSPIFFEID` (once), the OpenBao
     role and policy, and a ConfigMap `grauwolf-config` with the `[credentials.openbao]` section for
     that role. Optionally a pull secret, copied from an existing one. `agent list` and `agent remove`.
   - `secret set | list | delete` (5).
   - `agent check <name> --image <image>`: runs the check Job of ADR-0110 (`grauwolf credentials
     check`) in the agent's namespace under the restricted standard, waits, prints its result and
     deletes it.
   - **Second stage:** `trust-domain set` (4).
8. **Tests** (Q8): the Kubernetes side against a mocked API server (kube-rs with a mock tower
   service), OpenBao against a mock as in `grauwolf-credentials`. Then a live check in a cluster with an
   image built for it.

## Consequences
- Operators get one tool for the setup that ADR-0110 describes. The example manifests stay as
  documentation of what the tool does.
- Agents no longer share a policy: a compromised agent reads only its own secrets.
- A new heavy dependency (kube-rs, `k8s-openapi`), limited to one app crate.
- The program changes clusters with whatever rights the kubeconfig has. Destructive steps (removing an
  agent, changing the trust domain) print what they do. The trust-domain change needs `--apply` and
  writes a backup first.

## Implementation notes (stage 1, 2026-11-14)
- **Crates.** `grauwolf-k8-ops-app` (apps; library `grauwolf_k8_ops_app`, binary `grauwolf-k8-ops`),
  `grauwolf-k8s` (adapters: kube-rs behind a small `Cluster` API) and `grauwolf-openbao` (adapters: the
  OpenBao HTTP calls). `cargo xtask check-arch` knows the new app crate: it depends on no other app
  crate and nothing depends on it (`arch::K8S_OPS_APP`, rule test in `xtask/tests/arch_rules.rs`).
  Kubernetes and OpenBao are separate adapters so that the heavy client reaches only this program.
- **kube-rs.** `kube` 5.1.0 with `client`, `rustls-tls`, `ring` (no OpenSSL, no `runtime`, no `ws`) and
  `k8s-openapi` 0.29.0 with `v1_34` (the newest feature up to the cluster's 1.34; the crate does not use
  its types, it only selects the version). Objects, including `ClusterSPIFFEID`, are plain JSON
  values addressed by a `Kind` and sent through `kube::Client` as raw requests; there is no generated
  type and no `DynamicObject` (deviation in the means, not in the result). Writes are server-side
  apply (`PATCH` with `application/apply-patch+yaml`, `force=true`) with the field manager
  `grauwolf-k8-ops`.
- **Repeatable.** `Cluster::ensure` reads the object and skips the write when its annotation
  `grauwolf.io/applied-hash` equals the hash of the desired object (a field comparison does not work
  for a Deployment: the API server adds defaults to the arrays). An object made by hand (for example
  from `examples/spire-openbao/`) has no annotation, so `oidc install` applies over it and takes over
  the fields (`updated`). OpenBao: `ensure_*` read and compare (a desired subset of the stored
  values) and write only if they differ.
- **Reaching OpenBao.** `ProxyTransport` sends the OpenBao request through
  `/api/v1/namespaces/<ns>/services/http:<name>:<port>/proxy/…` with the token in `X-Vault-Token`;
  `HttpTransport` goes direct (`--openbao-addr`). `grauwolf-credentials` now uses the shared
  `Bao` for login and KV read (behaviour and error texts unchanged, its tests stayed green).
- **Trust domain.** Read from the SPIRE server ConfigMap (`server.conf`) by a minimal extractor
  (`hcl::trust_domain_of_hcl`: first `trust_domain = "…"` outside comments, no HCL library; the
  controller manager's YAML by `trust_domain_of_yaml`). Stage 1 never rewrites HCL. For stage 2,
  `spire::trust_domain` is the one reading function and `spire::trust_domain_sources` already lists
  every place (server, agent, controller-manager, OIDC provider; `doctor` adds the bound subjects of
  the agent roles).
- **Agent objects.** Namespace labels `grauwolf.io/agent`, `app.kubernetes.io/managed-by`,
  `pod-security.kubernetes.io/enforce=restricted`; one `ClusterSPIFFEID grauwolf-agents` with a
  namespace selector (`matchExpressions: grauwolf.io/agent Exists`) and no pod selector; OpenBao role
  and policy `agent-<name>` (role: `role_type=jwt`, `user_claim=sub`, audience, `bound_subject` = the
  exact SPIFFE ID, `token_ttl` 1 h). `agent add` refuses a namespace that exists without the agent's
  label and an agent whose name already lives in another namespace; `agent remove` finds the
  namespace by both labels, so it never deletes a foreign one. `--pull-secret <ns>/<name>` copies the
  Secret (values are copied, never printed) and attaches it to the service account.
  `secret set --variable` rewrites the agent's `grauwolf.toml` ConfigMap; `agent add` keeps the
  variables already mapped. `--variable` accepts only names from `grauwolf_config::VARIABLES`.
- **Secrets.** `Io::read_secret`: a hidden prompt (`rpassword`) when stdin is a terminal, else all of
  stdin without the trailing newline; an empty value is refused. The OpenBao error type carries the
  status and the server's `errors` only, so no value can appear in an error.
- **`agent check`.** Job `grauwolf-check-<6 hex digits>` (restricted-standard security context as in the
  example, `backoffLimit 0`, `activeDeadlineSeconds` = timeout, `ttlSecondsAfterFinished 600` as a
  safety net), polled every 250 ms until it succeeded, failed or the timeout (default 120 s). It
  runs `grauwolf credentials check <variable>` for the first mapped variable (or `--variable`), because that
  command takes one variable. The last log line is printed; the Job is deleted (propagation `Background`)
  in every case. Exit code 1 on failure or timeout.
  - *Update 2026-11-15 (owner's decision):* `grauwolf credentials check` takes several names or `--all` and
    prints one line per variable (the error naming the variable on stderr, exit code 1 if any fails;
    `CredentialEnv::check_many`: each variable on its own, one shared OpenBao login). `agent check` now
    checks every mapped variable in one Job (`--variable` is repeatable and narrows it) and prints the whole
    log. On a `permission denied` of a read (not of a login) it adds a hint: the agent's policy only reads
    `<kv>/data/agents/<name>/*`, and the path the variable maps to (from the agent's `grauwolf-config`).
- **`doctor` (owner's request, 2026-11-14: check the prerequisites, in `doctor`, not in a command of
  its own).** Two sections, each line `ok`/`warn`/`fail` with a reason and, where one exists, the next
  step as `hint` (`--json`: `section`, `check`, `status`, `reason`, `hint`); exit code 1 on any `fail`.
  *Prerequisites* (not installed by the tool): the cluster is reachable (and its Kubernetes version);
  the kubeconfig's rights, checked with `SelfSubjectAccessReview` for namespaces, serviceaccounts,
  configmaps, services, deployments, jobs, pods, pods/log, clusterspiffeids and – unless
  `--openbao-addr` – services/proxy in the OpenBao namespace; the SPIRE server StatefulSet is ready;
  the SPIRE agent DaemonSet is ready on every node (desired = ready = number of nodes); the CSIDriver
  `csi.spiffe.io`; the CRD `clusterspiffeids.spire.spiffe.io`; spire-controller-manager (a ready
  Deployment or a container in the spire-server pod); OpenBao is reachable, initialized and unsealed
  (`sys/health`); the token is valid and its `sys/capabilities-self` cover `sys/auth/<jwt>` (sudo and
  write), `sys/mounts/<kv>`, a policy, a role and a secret path (without a token this is a warning);
  Pod Security Admission (built in since Kubernetes 1.25: `ok`; an older server gets a warning, the
  admission plugin itself cannot be read through the API). A missing SPIRE part says: "SPIRE is not
  installed - grauwolf-k8-ops does not install SPIRE". If the cluster is unreachable nothing else is
  checked. *Setup* (installed by the tool): the OIDC discovery provider (ready) and its keys
  (`/keys` through the service proxy has at least one key), its `ClusterSPIFFEID`, the JWT auth method
  and its `jwks_url`, the KV v2 mount, `ClusterSPIFFEID grauwolf-agents` (a warning while no agent
  exists), the trust domain in every place that holds it (a difference is a failure and names the
  places), and per agent: namespace (restricted standard enforced), service account, ConfigMap, role
  (bound to exactly the expected SPIFFE ID) and policy (equal to the policy limited to the agent's
  path). A leftover shared role `grauwolf-agents` from the example is a warning.
- **Tests.** The Kubernetes side runs against `grauwolf_k8s::testing::FakeCluster`, an in-memory API
  server as a tower service (object store by URL path, label selectors, namespace cascade, access
  review rules, a Job outcome simulation, a service-proxy hook, a log of the writes that changed
  something) instead of `tower_test::mock`: the commands make many requests whose order is not
  the point. OpenBao is mocked with wiremock in `grauwolf-openbao`'s tests and with a small in-memory
  fake behind the proxy hook in the program's tests. They cover repeat runs without a change, the
  secret never in output or errors, list without values, remove, check success/failure/timeout with
  Job deletion, and every `doctor` case (missing SPIRE, sealed OpenBao, token without rights, invalid
  token, missing rights, daemon set not ready on every node, missing setup with its hints, mismatched
  trust domain, role bound to another domain, wide policy, legacy role).
- **Open points.** (Stage 2 `trust-domain set` is built, see below.) The binary is not in the container images (the grauwolf
  image and the tools image stay as they are); `scripts/build-macos.sh --k8-ops` builds it. The live
  check against a cluster. Measured for the debug build without debug info (cold, `-j 4`): 27 s,
  21.6 MB stripped (226 crates in the normal dependency tree; the lean `grauwolf-tools` is about 8 MB
  in release; no release build of this program was made).
- **First live run (2026-11-14):** `doctor` against the real cluster panicked at the first TLS connection.
  The program links two rustls crypto providers (kube uses ring, other crates bring aws-lc-rs), and
  rustls would not choose between them. `Cluster::connect` now installs ring when no provider is set. A
  binary test checks that an unreachable HTTPS server gives an error and not a panic. The fake API
  server of the other tests has no TLS, so they did not catch it. After the fix, `doctor` reported every
  prerequisite as ok. It gave two warnings: no agent exists yet, and the legacy shared role of the
  example is still there.
- **Live check of the write commands (2026-11-14, image built from `35ad032`):**
  - `oidc install` adopted the provider installed from the example (`updated`, then `unchanged`). `openbao setup` and a second `agent add` changed nothing.
  - `agent add reviewer` and `agent add writer` worked with `--pull-secret`. `secret set … --variable` stored a random value and mapped it.
  - `agent check reviewer` printed `resolved ATLASSIAN_API_TOKEN from openbao (31 bytes)`.
  - Isolation: with `reviewer` configured to read `kv/agents/writer/git`, OpenBao answered `permission denied`.
  - `doctor` reported every agent check as ok. Without `--yes` and without a terminal, `agent remove` cancelled. With `--purge-secrets --yes` it removed the namespace, role, policy and secrets.
  - One finding: `agent list` still showed agents whose namespace was terminating. It now leaves them out.
  - Afterwards the example's shared role and policy `grauwolf-agents` and its `ClusterSPIFFEID` `grauwolf` were removed from the cluster.

## Implementation notes (stage 2, `trust-domain set`, 2026-11-14)
The command is what was done by hand on the real cluster on 2026-11-14 (example.org to agentswork.de),
as a plan and then a change.
- **Plan first, always.** `make_plan` reads the SPIRE server, agent and controller-manager
  configuration, the OIDC provider's, the CA in the bundle ConfigMap and the `bound_subject` of every
  `agent-*` role, and changes nothing. Without `--apply` that is all (a test asserts no write to the
  cluster or OpenBao). The *old* domain is the one other domain the cluster holds: the server's
  value, else (for an interrupted run) whichever place still has it, including the bundle CA. More
  than one other domain is refused; `--from <domain>` names it when nothing in the cluster does
  (finishing the consumers of an interrupted run) and must agree with what is found. If nothing holds
  another domain the answer is `already <domain>, nothing to do` (exit 0).
- **Rewriting without an HCL library.** Only the `trust_domain` assignments whose value is exactly the old
  domain change, on the line (`hcl::rewrite_trust_domain`: indentation, quotes and comments stay;
  YAML `trustDomain:` the same). Lines that still mention the old domain afterwards (a `jwt_issuer`)
  are listed as a warning in the plan and after the rewrite. Plugin blocks (`DataStore "sql"`,
  `KeyManager "disk"`, `Notifier "k8sbundle"`) are found by brace matching on the comment-free text.
- **Refusals** (nothing changed): federation (`federation {`, `federates_with`), a `DataStore "sql"` that
  is not `sqlite3`, the SQLite file or the `disk` key manager's `keys_path` (or the default
  `<data_dir>/keys.json`) not on a mounted volume that is a `volumeClaimTemplate` of the StatefulSet
  (`data_claims`: the longest mount path that is a prefix of the file, in the spire-server container),
  a bundle ConfigMap that does not exist, a consumer that does not exist, a server scaled to 0 without
  the memo, a missing `spire-agent` DaemonSet. A missing agent, controller-manager or OIDC
  ConfigMap is skipped and said so.
- **Order with `--apply`:** confirmation (`--yes`; without a terminal and without `--yes` it
  cancels like `agent remove`), **backup** (`--backup`, default `trust-domain-backup-<UTC>.json`,
  `create_new`, mode 0600, the full objects of every ConfigMap rewritten and workload touched and the
  roles; no Secret is read), configuration (JSON merge patch of `data`), server data (scale to 0 with
  the replica count in the annotation `grauwolf.io/trust-domain-replicas`, wait until the pods
  `<name>-<n>` are gone, delete the PVCs `<template>-<name>-<n>` and wait until they are gone, scale back,
  wait until ready), **wait for the bundle** (a CA with the URI SAN `spiffe://<new>`), restart of the
  agent DaemonSet, the controller-manager Deployment (if there is one) and the OIDC provider (the
  `kubectl.kubernetes.io/restartedAt` annotation; waiting for `observedGeneration`, updated and
  ready counts), OpenBao roles (read, set `bound_subject`, write all fields), consumers (ConfigMaps,
  then restarts), verification (every configuration, the bundle and every role hold the new
  domain). A failure names the step and the backup file and says that the same command continues.
- **Resume.** Places that already hold the new domain are left alone; the server's volume is
  recreated only while the bundle has no CA of the new domain (so a run that stopped after the new
  CA exists does not delete the new database), and a StatefulSet found at 0 replicas comes back to
  the count in the memo annotation.
- **Bundle parsing.** The lockfile has no X.509 parser, so `x509` reads the URI SAN with a small DER
  walk and its own base64 (no new dependency); its test PEMs are made with openssl.
- **Consumers.** `--consumers <file.toml>` with `[[configmap]]` (`namespace`, `name`, optional `keys`)
  and `[[restart]]` (`kind`, `namespace`, `name`); unknown fields are an error. Replacement rule
  (`consumers::replace_domain`): `spiffe://<old>` followed by `/`, `"`, `'`, white space or the end,
  and the exact quoted strings `"<old>"` and `'<old>'`; never a bare substring. Example:
  `examples/k8-ops/trust-domain-consumers.toml` (scion; tested).
- **Tests.** `FakeCluster` got `PATCH` with a merge patch (a missing object is 404, `null` removes a
  field), `remove` and `on_change` hooks; the tests play the controllers with them (a StatefulSet
  scales, pods disappear, a server with a fresh volume writes a CA of the domain in its
  configuration), so a forgotten PVC deletion shows up as a bundle timeout. Covered: the plan
  without any write, every refusal, the order of the writes, the backup before the first write and
  its content, cancel, consumers with `foo.example.org` staying, timeouts (pod, bundle, readiness),
  resume after a timeout, a mixed state, `--from`, and `doctor` agreeing afterwards.
- **Open points.** (`trust-domain restore <backup>` is built, see below.) The live check of the command against the cluster is the
  coordinator's.
- **Live check of `trust-domain set` (2026-11-14, image built from `e82ed88`):**
  - Setup: the plan ran first, with the scion consumers file, against the real cluster. It found the right ConfigMaps, the PVC `data-spire-server-0`, the bundle ConfigMap, the restarts and the scion lines.
  - Finding (fixed): the rewritten lines were reported as leftovers of the old domain, because `test.agentswork.de` contains `agentswork.de`. The search now leaves the new domain out.
  - Round trip with a test agent `reviewer`: `agentswork.de` → `test.agentswork.de` → `agentswork.de`, each step with `--apply --yes --backup` and taking about 30–40 s.
  - After each step:
    - `doctor` found the new domain in every place, including the agent's role (`bound_subject` rewritten).
    - The SPIRE agent attested with the new ID, and scion's ACL and `trust_domain` were rewritten. `scion-server` restarted and logged no errors.
    - `agent check reviewer` resolved the agent's secret (31 bytes) under both domains.
  - The test agent, the image and the build namespace were removed afterwards.

## Implementation notes (`trust-domain restore`, 2026-11-14)
The owner asked for the open point of stage 2: `trust-domain restore <backup> [--apply] [--yes]
[--backup FILE] [--timeout SECONDS]`, module `restore`, built on the steps of `trust_domain`
(`plan_recreate`, `spire_restarts`, `rewrite_configs`, `recreate_server`, `restart`, `verify`).
- **The backup is read, not trusted.** `tool` must be `grauwolf-k8-ops trust-domain set` or
  `... restore`, `format` absent (stage 2 files) or 1 (new files carry `"format": 1`), `from` a valid trust domain;
  every ConfigMap needs `metadata.namespace` and `name`, every workload `kind` (Deployment,
  StatefulSet, DaemonSet) and both, every role `mount`, `name`, `data`. Anything else is refused with
  the reason before the cluster is read. The domain restored is `from`; in the backup written by a
  restore `from` is the domain of the cluster at that time and `to` the restored one, so the
  meaning is the same for both tools.
- **Plan.** Per ConfigMap of the backup and per key a line-level diff (`line_diff`: a longest common
  subsequence in which lines that match after putting the trust domain back count as aligned, so a
  rewritten `trust_domain` line is one change and an unrelated added line stands alone). Only `data`
  is restored; keys of the cluster that the backup lacks stay. A ConfigMap that no longer exists is
  refused by name before anything changes. Changed lines that mention neither the current nor the
  restored domain are listed separately as a warning (a restore undoes them too). Roles: restored
  only for an agent whose namespace is managed by the tool and not being deleted; others are skipped
  and the plan says so. Roles of agents not in the backup stay and are only warnings in the
  verification (so a restore does not exit 1 because of an agent added later). Server data: recreated
  (as `set`) only if the bundle CA lacks `spiffe://<from>`; the datastore and federation refusals
  judge both the current and the restored server configuration. Restarts: agents, controller-manager
  if present, OIDC provider, then the backup's workloads that are not SPIRE's own; one that no longer
  exists is skipped with a note.
- **Order with `--apply`:** confirmation (as `set`), backup of the current state (same format;
  the ConfigMaps of the backup as they are now, the server StatefulSet if its data is recreated, the
  restarted workloads, the roles as OpenBao returns them; `create_new`, 0600; nothing is changed if it
  fails), ConfigMaps, server data, wait for the bundle, restarts (SPIRE first, then consumers),
  roles (all fields of the backup on top of the current ones, `ensure_jwt_role`), verification
  (`verify` of `set` with the domain restored). A failure names the step and both files (the one restored from,
  the one of the state before).
- **Idempotence.** If every ConfigMap key, every role to restore and the bundle already match,
  the answer is `already as in the backup, nothing to do` (exit 0) and no backup is written. An
  interrupted restore is finished by the same command with another `--backup` file: the server data is
  recreated only while the CA lacks the domain, a StatefulSet at 0 comes back by its memo annotation.
- **Not restored.** Metadata, labels and annotations, workloads other than by restart, Secrets,
  agents added or removed since the backup, and the SPIRE server's old database and CA (the data is
  recreated, every workload gets a new SVID again). A backup whose ConfigMaps are gone is restored by hand
  (user guide, chapter 13).
- **Tests** (`tests/trust_domain.rs`): the plan without any write and with the other changed lines,
  broken/foreign/unknown-format files, a missing ConfigMap, federation and postgres, a removed agent's
  role skipped, the backup written first (and nothing changed if it cannot be), the order of the writes
  and restarts after the bundle has the old CA, the round trip `set` A to B then `restore` with `doctor` agreeing,
  the restore of the restore, consumers, no-op without a backup, resume after a timeout. The tests
  name the fixtures' objects (`metadata`), as a real cluster does. A restart within the same second as
  the last one changes nothing (the annotation is the time in seconds), which the tests account for.
- **Open points.** Not checked against a real cluster yet; the coordinator's live check follows.
- **Live check of `trust-domain restore` (2026-11-14):**
  - The plan of `backup-td-2.json` (the state at `test.agentswork.de`, taken before the earlier switch back) showed the right ConfigMap diffs, including scion's. It skipped the role of the meanwhile removed agent `reviewer`.
  - `restore --apply` moved the cluster to `test.agentswork.de` in 28 s. Restoring the backup that this restore had written moved it back to `agentswork.de` in 39 s.
  - Running the same restore again printed "already as in the backup, nothing to do" and wrote no backup.
  - Afterwards `doctor` agreed. The SPIRE agent had attested as `spiffe://agentswork.de/...`, and `scion-server` ran without errors on its restored ACL.

## Implementation notes (small improvements, 2026-11-15)

The owner decided on four small improvements. They change no decision above.

- **Credentials check of all variables.** See the update in the notes of `agent check`.
- **OpenBao without the root token.**
  - `openbao setup` also writes the ACL policy `grauwolf-k8-ops`. Its rules (`openbao_setup::rules`) are derived from the admin calls of `grauwolf-openbao` and from what `doctor` reads:
    - `sys/auth` (read) and `sys/auth/<jwt>` (create, update, sudo: enabling an auth method needs sudo) and `auth/<jwt>/config`;
    - `sys/mounts` (read) and `sys/mounts/<kv>`;
    - `sys/policies/acl/agent-*` (create, read, update, delete) and `sys/policies/acl/grauwolf-k8-ops` (read only: the tool cannot widen its own policy);
    - `auth/<jwt>/role/agent-*` and `auth/<jwt>/role/grauwolf-agents` (read, for the legacy-role warning of `doctor`);
    - `<kv>/data/agents/*` and `<kv>/metadata/agents/*`;
    - `sys/health`, `sys/capabilities-self` (update) and `auth/token/lookup-self`.
    There is no `sys/*` wildcard and no right to create tokens.
  - `openbao token [--ttl 8h] [--renewable]` calls `auth/token/create` with `policies: [grauwolf-k8-ops]`, `no_default_policy` and `no_parent: true`; if the token in use may not create orphans (the server answers 403) it asks again without `no_parent`, so the token is a child of the token in use. The token is the only thing on stdout; a note goes to stderr. It refuses a missing policy (`run openbao setup`) and a `--ttl` that is not a duration.
  - `doctor` has the new check `OpenBao token policy` (`auth/token/lookup-self`): `warn` with "working with the root token" while the token has the policy `root`, `ok` otherwise.
  - Tests: the fake OpenBao enforces a policy path by path (`Env::restrict_to`; exact path, else the longest `prefix*`; `sudo` for writes below `sys/auth/`), and `tests/restricted_token.rs` runs `openbao setup`, `agent add|check|deploy|status|allow|undeploy|remove`, `secret set|list|get|delete` and `doctor` with it and asserts that nothing was refused. `trust-domain` is not in that run (its plan needs a more complete SPIRE fake); its OpenBao calls are the role reads and writes of `agent add`, which are in it.
  - Decision: a token that has the policy but is a child of an expiring parent expires with it; the tool says nothing about that, `--ttl` and `doctor` are the way to see it.
- **`--context <name>`.** A global option; `Cluster::connect(kubeconfig, context)` reads the kubeconfig (`--kubeconfig`, `KUBECONFIG`, default location) and uses that context, else its current context; an unknown context is an error naming the available ones (`context 'x' not found in the kubeconfig (available: a, b)`), before any request. `doctor` prints `Cluster <server URL> (context <name>)` as the first line of its text output (in `--json` the reason of the check `cluster` carries it, so the array stays valid). Decision: the other commands print nothing about it, because they are not cheap to make quiet (`secret get --reveal` and `openbao token` print one value, `--json` outputs and `agent logs` are captured); `doctor` is the command to look at first.
- **`agent deploy --events`** and `GRAUWOLF_EVENTS` for generated services: see the notes of ADR-0121.

### Live check of the small improvements (2026-11-15, images built from `9da4bea` and `9205022`)
- **`--context`.**
  - `--context nope` fails with `context 'nope' not found in the kubeconfig (available: default)`.
  - With `--context default`, `doctor` starts with `Cluster https://kubeapi.agentswork.de (context default)`.
- **Restricted token.**
  - `openbao setup` with the root token created the policy `grauwolf-k8-ops`, and `openbao token --ttl 2h` printed only the token on stdout.
  - `doctor` warned with the root token and said ok with the restricted one.
  - With the restricted token these all passed against the real OpenBao:
    - `openbao setup`, `agent add` ×2, `secret set` ×3, `secret list` (a LIST on `kv/metadata/agents/…`), `secret get --reveal`;
    - `allow`, `callers`, `deny`, `agent deploy`, `agent check`;
    - `agent remove --purge-secrets`.
  - `openbao token` with the restricted token was refused, as intended. The token was revoked afterwards.
- **`agent check`.**
  - It checked all four mapped variables in one Job. Three resolved; `OPENAI_API_KEY`, mapped on purpose to `kv/agents/coordinator/…`, was refused.
  - **Finding (fixed in `9205022`):** OpenBao 2.7 wraps its errors as `"1 error occurred:\n\t* permission denied\n\n"`. The message went over two log lines, so the policy hint did not appear. `BaoError` now turns wrapped errors into one line (`read: HTTP 403: permission denied`). With an image of that commit, the hint appeared: `hint: the policy agent-reviewer of the agent only reads kv/data/agents/reviewer/*; OPENAI_API_KEY maps to … kv/data/agents/coordinator/openai …`.
- **`--events`.**
  - `agent deploy --events` for both agents; `status` showed `events: on`.
  - After one call, `agent logs --events-only` of `coordinator` showed `agent_starting`, `node_starting`, `node_completed` and `agent_completed`.
  - In `reviewer`, `agent_starting` carried `caller = spiffe://agentswork.de/ns/coordinator/sa/coordinator`, visible without Tempo.
  - A redeploy without `--events` restarted the pod, and `status` showed `events: off`.
  - Image deploys (`GRAUWOLF_EVENTS`) were covered by the slow test only.
- **Behaviour seen again.** A caller whose target is not deployed crash-loops until the target is up, because the agent card is fetched at start. It recovered on its own once `reviewer` was ready.
- Everything was removed afterwards: agents, policies, secrets, the build namespace and the images. The policy `grauwolf-k8-ops` stays, because it is part of the setup.

## Implementation notes: the identity service (2026-11-16, ADR-0111 stage 1 part B)
- New command group `identity install | status | uninstall | allow | deny`; it follows the rules of
  this ADR: repeatable with `created`/`updated`/`unchanged`, checks before it writes, refuses a
  namespace `identity` it did not make, asks before `uninstall` (`--yes`), and `doctor` has the line
  `identity service`. Details in ADR-0111, "Implementation notes: stage 1 part B".
- The least-privilege policy `grauwolf-k8-ops` grows by `sys/mounts/transit`, `transit/keys/*`
  (create, read, update, delete – `uninstall --delete-key` sets `deletion_allowed` first) and the
  policy and role `grauwolf-identity`. Never `transit/sign/*`: the tool can create the key, not sign
  with it. **`openbao setup` must run again** (with an admin token) before the restricted token can
  install the identity service.
