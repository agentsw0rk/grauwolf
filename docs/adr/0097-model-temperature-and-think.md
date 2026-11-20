# ADR-0097: `temperature` and `think` in the model of a spec

- **Status:** Accepted (User, 2026-11-11: "close the smaller grauwolf gaps: temperature/think (ADR-0097) and the tools think, fetch, key-value and todo (ADR-0098)"; ADR-0092)
- **Date:** 2026-11-10

## Context
A spec model has `provider`, `id` and `fallbacks` (ADR-0088). Sampling temperature cannot be set,
and reasoning of Ollama models is switched only by the environment variable `OLLAMA_THINK` for the
whole process. AgentWeaver sets `temperature` per agent and `ollamaThink` per Ollama agent.

## Decision
- `model.temperature` (number, 0–2, optional): sent as the provider's temperature (OpenAI
  `temperature`, Anthropic `temperature`, Gemini `generationConfig.temperature`, Ollama
  `options.temperature`). Absent → the provider default, as today.
- `model.think` (bool, optional): Ollama's `think`; takes precedence over `OLLAMA_THINK`. For
  other providers the validation rejects it (no silent no-op).
- Both live on `LlModel`, so fallback models, subgraph models and `set_model` carry their own
  values; the response cache key includes them (ADR-0086).
- Generator and interpreter read them the same way; the Rust API gets
  `LlModel::with_temperature`/`with_think`.

## Consequences
- AgentWeaver's agent settings map 1:1; agents become reproducible with `temperature: 0`.
- Cached responses from before are not reused for models with these fields set.

## Implementation notes (2026-11-11)
- `think` is validated per provider: every model, fallback and `set_model`/`llm_moderate` entry with
  `think` and a provider other than `ollama` is `ThinkNotSupported`; `temperature` outside 0–2 is
  `TemperatureOutOfRange`; embedding models reject both (`EmbeddingSampling`).
- `LlModel` implements `Eq`/`Hash` by hand (the temperature by its bits), because `f64` has neither.
- The response cache key contains `temperature` and `think` only when they are set, so existing
  cache files stay valid for models without them.
- The model of a `decide` node (`provider/id` text) has no `temperature`/`think` inside; the node
  has its own parameters (ADR-0100, implementation notes).
- Live check (Ollama `qwen3.5:9b`): `temperature: 0` gave the same answer twice; `think: true` used
  412 output tokens for a small question, `think: false` 3. The reasoning text itself does not
  appear in events (`llm_call_completed` carries only usage).
