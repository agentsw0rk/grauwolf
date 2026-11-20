# E11 – Integrations & runtime


| ID | Feature | Rust approach | Prio |
|----|---------|-------------|------|
| F11.1 | Automatic configuration from properties/env (API keys, base URLs, timeouts, retry per provider) | `grauwolf::Config` with `serde` + `figment` (TOML/YAML/env, prefix `GRAUWOLF_`) → builds the executor | M2 |
| F11.2 | Web framework integration: executor/agent as shared state, route helpers | Crate `grauwolf-axum`: `State<Arc<grauwolf::Runtime>>`, extractors, SSE streaming response helper | M3 |
| F11.4 | Amazon Bedrock AgentCore runtime (host an agent as an AgentCore service: `/invocations`, `/ping`) | axum server following the AgentCore contract | M3 |
| F11.5 | CLI agents: drive external coding agents (Claude Code, Codex CLI) as a grauwolf agent | `tokio::process` + JSON stream parsing; `CliAgent` implements the same agent trait | M3 |
| F11.6 | Skills: discovery of `SKILL.md` skills, embedding them in the system prompt | Directory scan, YAML frontmatter (`serde_yaml`), prompt rendering | M2 |
| F11.8 | Facade crate (everything via feature flags) | Crate `grauwolf` with re-exports | M1 |
| F11.9 | Examples (calculator, banking, chess, guesser, attachments, MCP, OTel, Langfuse …) | Own `examples/` in the workspace | M1+ |
| F11.10 | Multiplatform (JVM, JS, Wasm, iOS, Android) | **dropped** – native targets only (Linux/macOS/Windows) | – / later |
| F11.11 | Python/Node bindings | **dropped** | – |
