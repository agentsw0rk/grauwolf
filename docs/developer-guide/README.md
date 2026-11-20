# grauwolf Developer Guide

> German version (as of 2026-11-15): [Handbuch für Entwickler](../entwickler/README.md).


This guide is for everyone who uses grauwolf as a Rust library, extends it, or works on the
framework itself. It assumes you know the basic concepts from the
[user guide](../user-guide/README.md) (spec, node, edge, tool, strategy). The chapters build on
each other: first the big picture, then working on the code, then the internals.

## Learning path

| Chapter | Topic | You learn … |
|---|---|---|
| [1](01-architecture.md) | Architecture | Layers, crates, how a run flows from the spec to the answer |
| [2](02-development-setup.md) | Development setup | Building, testing, checking; TDD, commits, ADRs and `AGENTS.md` |
| [3](03-rust-api.md) | grauwolf as a library | Loading and starting agents from Rust, typed tools with `#[tool]`, graphs in code |
| [4](04-tools-internals.md) | Tools from the inside | Tool contract, registry, build to WASM, sandbox, runtime tools |
| [5](05-extending.md) | Extending | Your own node kinds and conditions, your own LLM providers, ports and adapters |
| [6](06-generator.md) | Generator | How a spec becomes a crate and why the interpreter and generator must behave the same |
| [7](07-events-and-observability.md) | Events and observability | Events, features, interceptors, approvals, OpenTelemetry |
| [8](08-server-and-protocols.md) | Server and protocols | HTTP service, A2A, MCP – structure and extension |
| [9](09-identity-credentials-and-the-cluster-tool.md) | Identity, credentials and the cluster tool | Workload identity (SPIFFE), credential sources, delegation with Txn-Tokens, the OpenBao and Kubernetes adapters, `grauwolf-k8-ops` |
| [10](10-grauwolf-studio.md) | grauwolf studio | The studio backend and API, the spec ↔ canvas mapping and auto-layout, the generated TypeScript client, the working copy with its ops and the MCP endpoint |

## Further reading

- [Architecture overview](../architecture.md)
- [Architecture decisions (ADRs)](../adr/README.md)
- [Feature catalog](../features/README.md)
