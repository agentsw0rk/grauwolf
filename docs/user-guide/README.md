# grauwolf User Guide

> German version (as of 2026-11-15): [Handbuch für Nutzer](../handbuch/README.md).


This guide shows you step by step how to describe, run and operate agents with grauwolf – without
writing a single line of Rust. Each chapter builds on the previous ones and ends with an example
you can try right away. All example specs live in [`examples/`](examples/) and are checked
automatically on every test run.

## Learning path

| Chapter | Topic | You learn … |
|---|---|---|
| [1](01-getting-started.md) | Getting started | Set up grauwolf, validate, run and chat with your first agent |
| [2](02-models.md) | Models and providers | Use Ollama, OpenAI (and compatible providers), Anthropic and Gemini, mix several models, costs and budget |
| [3](03-tools.md) | Tools | Give the agent capabilities: your own tool code, tool files, tools directories, built-in tools (files, Jira, Confluence, git) |
| [4](04-flows.md) | Flows | Templates (`single_run`, `react`, `verified`, moderation) and your own graphs of nodes and edges |
| [5](05-structured-data.md) | Structured data | Your own types for input and output instead of plain text |
| [6](06-subgraphs-and-planners.md) | Subgraphs and planners | Split flows, work in parallel, let a planner plan (`plan`, GOAP) |
| [7](07-conversations-and-knowledge.md) | Conversations, memory and knowledge | Continue conversations, long-term memory, search your own documents (RAG) |
| [8](08-external-tools-and-agents.md) | External tools and other agents | Connect MCP servers, agents as tools of other agents (local and via A2A) |
| [9](09-operations.md) | Operations | Agent as a service, users and tokens, approvals, observability, checkpoints |
| [10](10-self-made-tools.md) | Self-made tools | Let the agent build its own tools during the run – safely and under control |
| [11](11-code-generation.md) | Code generation | Turn a spec into a standalone Rust crate and generate specs with an LLM |
| [12](12-credentials-from-openbao.md) | Credentials from OpenBao with SPIFFE | Let agents fetch tokens from a secret broker with their SPIFFE identity instead of the environment |
| [13](13-setting-up-a-cluster.md) | Setting up a cluster with grauwolf-k8-ops | `doctor`, one namespace and one OpenBao role per agent, secrets, a check Job – the steps of chapter 12 as commands |
| [14](14-acting-on-behalf-of-people.md) | Acting on behalf of people | Delegation with Entra ID and Txn-Tokens: concepts, prerequisites, setup step by step, the runtime flow as diagrams, the identity service, operations (policy, key rotation, uninstall), troubleshooting from the live check, Entra Local |
| [15](15-the-tool-catalog.md) | The tool catalog | Package metadata and sandbox limits, the catalog of a registry (`grauwolf-tools catalog list/search/show/add/remove/sync`) judged by your tool policy, a tool repository with CI from `grauwolf-tools new` |
| [16](16-grauwolf-studio.md) | grauwolf studio | Draw specs on a canvas: the palette from the node catalog, validation at the node, the layout file beside the spec, drawing together with a coding agent over MCP |

## Reference

- [CLI reference](reference-cli.md) – all commands and options of `grauwolf`
- [Spec reference](reference-spec.md) – all fields of a spec with their defaults
- [Configuration reference](reference-config.md) – `grauwolf.toml`: the tool policy and the credential sources; the identity service's policy file

If you want to extend grauwolf itself or use it as a Rust library, you will find everything else in
the [developer guide](../developer-guide/README.md).
