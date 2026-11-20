# ADR-0129: Building harness images in the cluster – kubeconfig, Kaniko from git, push to a registry

- **Status:** Proposed (User, 2026-11-19: "in the system overview: the Kubernetes config, building the
  image and pushing it into a registry – I must enter a kubeconfig, build with Kaniko and push into
  a registry I define"; decisions B1–B4 of the same day)
- **Date:** 2026-11-19
- **Related:** ADR-0128 (harnesses; decision 3: the project in `build/<name>/` of the workspace,
  versioned with it; H7: the default image `<registry>/<name>:<version>`), ADR-0126 (studio;
  decision 5.2: deployment through `grauwolf-k8-ops`; keys stored by the studio), ADR-0096 (the
  builder image the generated `Dockerfile` builds `FROM`), ADR-0120/0121 (`grauwolf-k8-ops`,
  `grauwolf-k8s`).

## Context
A harness project (ADR-0128) is a folder with a `Dockerfile`; turning it into an image needs a
container builder, and the studio has none. AgentWeaver built in the cluster with Kaniko: a build
pod whose init container waited until the backend had copied the context in through `exec`, then
Kaniko built and pushed. `grauwolf-k8s` has no `exec`.

Google archived Kaniko in June 2025; maintained forks continue it – Chainguard's (source only) and
`osscontainertools/kaniko` (images on `ghcr.io/osscontainertools/kaniko`).

## Decision
1. **Kubeconfig pasted into the studio (B1).** The System tab has a *Kubernetes* setting where the
   user pastes a kubeconfig; the studio stores it like the API keys (ADR-0126 R3): in its config
   directory, mode 0600, written through a temporary file, never shown again, only "set" and its
   contexts and server. The user picks a context; a *Check* calls the cluster's version and the
   rights the build needs (`can-i`). The studio does not read `~/.kube/config`.
2. **The build context comes from git (B2).** Kaniko clones the workspace's repository at a commit
   and builds `build/<name>/` of it (`--context git://<repo>#<ref>#<commit>`,
   `--context-sub-path build/<name>`). Nothing is copied into the cluster, so no `exec`. This needs
   the project committed and pushed; the studio shows the repository, branch and commit it will
   use and refuses while `build/<name>/` has changes that are not committed or the commit is not on
   the remote. A private repository needs a git token (a Secret, Kaniko's `GIT_USERNAME` /
   `GIT_PASSWORD`). Committing and pushing from the studio come with ADR-0126 part 2b; until then the
   user does it.
3. **Kaniko image (B3):** `ghcr.io/osscontainertools/kaniko` at a pinned version by default,
   configurable. Kaniko builds `FROM` the grauwolf builder image (ADR-0096), which the cluster must
   be able to pull.
4. **Push to a registry the user defines.** The destination is the harness's image, else its
   default image (ADR-0128 H7). Registry credentials (server, user, token) are stored by the studio
   like the kubeconfig and handed to the build as a Secret of type `kubernetes.io/dockerconfigjson`
   in the build namespace, mounted at `/kaniko/.docker/config.json`.
5. **Layers (B4).** Rendering the build pod (Kaniko arguments, git and registry Secrets, labels,
   deadline), starting it, following its log and reporting the outcome live in a shared adapter
   (`grauwolf-k8s`), used by a new `grauwolf-k8-ops agent build --harness <file>` and by the
   studio, so a build works without the studio as well.
6. **Where the build runs:** a namespace of its own (default `grauwolf-builds`), one pod per build
   (`<name>-build-<short commit>`), deleted after it ended; its log streams into the System tab.

## Implementation notes
- **Shared adapter (B4).** `grauwolf-k8s` holds the build: `kaniko_job`, `registry_secret`,
  `kaniko_repository`, `run_build_with` (Job, wait, log lines handed to a callback, Job deleted), and the
  two constants `KANIKO_IMAGE` and `BUILD_NAMESPACE` that both programs use. `grauwolf-git::build_source`
  decides what may be built (committed and pushed, remote `origin`).
- **Studio** (`grauwolf-studio`, `builds.rs`, `build_api.rs`, `kubernetes.rs`, `cluster_api.rs`): the kubeconfig and the registry
  and git credentials are stored like the API keys and never shown again; a build is started with
  `POST …/harnesses/{path}:build`, kept in memory (state `running`, `succeeded`, `failed`, `timed_out`,
  `error`), its log is read and streamed as server-sent events; a second build of a harness is refused while one
  runs. The namespace carries `app.kubernetes.io/managed-by: grauwolf-studio`; its Secrets are
  `grauwolf-studio-registry` and `grauwolf-studio-git`.
- **Without the studio** (`grauwolf-k8-ops`, `src/build.rs`): `agent build --harness <file>` does the same
  flow with the kubeconfig of the operator: source from `build_source` of `build/<name>` (or `--project`),
  destination the harness's `image` else its default image, the namespace ensured (label
  `app.kubernetes.io/managed-by: grauwolf-k8-ops`, no Pod Security level because Kaniko runs as root), the
  log printed line by line, exit code 0 or 1. `--registry-secret` and `--git-secret` name Secrets that must
  exist; `build secret registry|git <name>` makes them from a token read from stdin or a hidden prompt.
  Everything that can be checked (harness, source, image) is checked before the cluster is touched.

## Open points
- Deploying the built image from the studio (`agent deploy --harness`) – the next step after this.
- The git token and registry credentials per workspace or per studio.
- Whether the studio checks that the builder image is pullable before a build.

## Consequences
- The studio writes to a cluster for the first time; the kubeconfig it holds can do what its user
  can. It is stored like the API keys and never sent anywhere but to its cluster.
- A build always matches a commit: reproducible and visible in the repository's history, but
  nothing can be built that is not pushed.
- The `build/` folder of a workspace must be committed (ADR-0128 decision 3 allowed it; now it is
  needed for a build).
