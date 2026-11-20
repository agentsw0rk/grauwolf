# ADR-0082: OpenAI-compatible providers with their own names

- **Status:** Accepted (User, 2026-11-09: "do this first, 2 (OpenAI-compatible providers)")
- **Date:** 2026-11-09

## Context
Many providers replicate OpenAI's chat completions interface (F02.6). So far, one reaches them only via `openai` with `OPENAI_BASE_URL`; this allows at most one such service per process, and a spec cannot mix OpenAI with, e.g., Mistral (own model per subgraph, ADR-0043). The spec check knows only the built-in names.

## Decision
- The facade knows further provider names that use the OpenAI adapter with their own address and their own key (Cargo feature `openai`):

  | Provider | Key | Default address |
  |---|---|---|
  | `openrouter` | `OPENROUTER_API_KEY` | `https://openrouter.ai/api/v1` |
  | `deepseek` | `DEEPSEEK_API_KEY` | `https://api.deepseek.com` |
  | `mistral` | `MISTRAL_API_KEY` | `https://api.mistral.ai/v1` |
  | `groq` | `GROQ_API_KEY` | `https://api.groq.com/openai/v1` |
  | `dashscope` | `DASHSCOPE_API_KEY` | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` |
  | `xai` | `XAI_API_KEY` | `https://api.x.ai/v1` |

- The address can be overridden per provider with `<NAME>_BASE_URL` (e.g., `DASHSCOPE_BASE_URL` for the Chinese region).
- The names are in `providers::BUILTIN`; thus `spec validate`, `spec create` and error messages check them without further changes. Embeddings (`knowledge`, long-term memory) use the same path; whether a provider offers them is decided by its server.
- No dedicated adapter: deviations of individual providers (e.g., missing `/moderations`, no `json_schema`) show up as errors of the provider. Custom names for further compatible services in the spec are not part of this decision; for them, `openai` with `OPENAI_BASE_URL` remains.

## Consequences
- A spec can use several compatible providers side by side, each with its own key.
- Price tables (`--prices`) and events name the provider, e.g. `mistral:mistral-small-latest`.
- The table must be maintained when providers change their addresses; `<NAME>_BASE_URL` helps until then.
