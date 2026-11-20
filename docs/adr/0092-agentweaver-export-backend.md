# ADR-0092: grauwolf as an export backend of AgentWeaver

- **Status:** Superseded by [ADR-0126](0126-grauwolf-studio.md) (2026-11-17); was Proposed (User, 2026-11-10: "goal is that I can use grauwolf in AgentWeaver"; decisions: generated Rust code, generated at export time, tool code in Rust, grauwolf serves AgentWeaver's endpoints, plan first)
- **Date:** 2026-11-10

## Context
AgentWeaver is a visual editor for agent graphs (Spring Boot backend, Next.js frontend). Each
*harness* on its canvas is exported by a *generator backend* into a project with a root
`Dockerfile` and `k8s/deployment.yaml`/`service.yaml`; the runtime tab builds the image in the
cluster with kaniko and rolls it out. Today all backends produce JVM projects. The exported agent
must serve `POST /api/chat` (`{message}` → `{response}`, or the fields of typed input/output) and
`GET /api/signature`; cron jobs call `/api/chat`, a poller reads `/api/signature` every 5 minutes
through the Kubernetes service proxy.

grauwolf describes agents as JSON specs and can generate a Rust crate from a spec (ADR-0050,
ADR-0084). A grauwolf backend gives AgentWeaver small images without a JVM and fast start-up.

## Decision
- **New AgentWeaver backend `grauwolf`** (per harness, like the existing backends). Pipeline:
  editor graph → AgentWeaver's internal agent model → **grauwolf spec(s)** → `grauwolf spec
  generate` **at export time** (AgentWeaver runs the grauwolf CLI as a process) → Rust crate in the
  export → `Dockerfile` builds it with `cargo build --release` on the grauwolf builder image →
  small runtime image. The generated code is visible in AgentWeaver's code explorer, ZIP and
  workspace repository; spec errors (`spec validate`) appear in the editor before any build.
- **Runtime contract:** the generated crate becomes an HTTP service (ADR-0093) that additionally
  serves AgentWeaver's endpoints `POST /api/chat` and `GET /api/signature` (ADR-0094). AgentWeaver's
  cron jobs, signature poller and runtime tab stay unchanged.
- **Tool code is Rust.** For a grauwolf harness, the bodies of custom tools are Rust (WASM sandbox,
  F13.11). The export stays plain text: tool sources go into the crate and are compiled to WASM in
  the image build (ADR-0095).
- **Images:** `grauwolf-builder` (toolchain, sources, vendored dependencies; offline build) and
  `grauwolf` (CLI for the AgentWeaver backend), versioned together with grauwolf (ADR-0096).
- **Gaps closed in grauwolf:** model `temperature` and `think` in the spec (ADR-0097), built-in
  tools `think`, `fetch`, key-value store and `todo` (ADR-0098), an opt-in command tool for
  containers (ADR-0099).
- **Not supported at first** (AgentWeaver rejects them for a grauwolf harness with a clear
  message): Kotlin code in graph agents (`node-custom`, `node-condition`, edge conditions and
  transformations), decision nodes (System One), multi-agent flows (`next`/`forEach`/`collect`),
  skills, Jira/Confluence/Git tools, Bedrock, PostgreSQL/pgvector storage, MCP over the old SSE
  transport, numeric GOAP facts. Each is a feature in E14; ADR-0100–0106 describe decisions, flows,
  code in graph agents, skills, Jira/Confluence/Git, Bedrock and pgvector.
- **Mapping** of editor nodes to the spec, the work packages and the preparation steps are listed
  in the feature catalog [E14](../features/E14-agentweaver.md).

## Preparation outside the code (owner)
1. **AgentWeaver repository:** `agentweaver/` needs its own git clone (remote URL), so changes there
   can be committed and reviewed separately from grauwolf.
2. **Image registry:** permission to publish `ghcr.io/agentsw0rk/grauwolf-builder` and
   `ghcr.io/agentsw0rk/grauwolf` (GitHub Actions `packages: write`, packages public or a pull
   secret in the cluster); first release tag of grauwolf (e.g. `v0.1.0`).
3. **Cluster:** a registry the runtime tab can push to (already required for Spring Boot), enough
   memory/CPU for the kaniko build of a Rust crate (≈ 2–4 GiB RAM, minutes of CPU), and Ollama or
   API keys as for existing harnesses.
4. **AgentWeaver container:** the backend image gets the `grauwolf` binary (copied from the
   `grauwolf` image); for local development the binary on `PATH` or a configured path.
5. **Decisions still open:** command tool policy (ADR-0099), whether Kotlin graph-agent code gets a
   Rust equivalent, which later features (E14) come first.

## Consequences
- grauwolf gets a second consumer with a fixed HTTP contract; changes to `/api/chat` or the
  signature format in AgentWeaver must be followed here (ADR-0094).
- The generated crate becomes the deployable artifact; generator gaps compared to the interpreter
  (`tools_dir`, `self_tools`, `max_cost`, ADR-0084) also apply to AgentWeaver.
- AgentWeaver features without a grauwolf counterpart stay JVM-only until their E14 feature is done.
- Build times depend on the builder image; without it, a Rust build in the cluster can exceed
  kaniko's deadline.
