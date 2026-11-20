# ADR-0066: Content moderation

- **Status:** Accepted (self-prioritized; User, 2026-11-08: "keep going")
- **Date:** 2026-11-08

## Context
The feature catalog lists content moderation as a port (F01.15) and as a node/edge (F05.17). An agent that is publicly reachable as a service (ADR-0053) should be able to recognize and reject harmful input before it reaches the model or tools. Needed are `moderate(prompt, model) → { harmful, categories }` (natively through OpenAI's moderation API where it exists) and nodes that check text and pass on the result together with the text.

## Decision
- Port: `LlmClient::moderate(text, model) → Moderation { harmful, categories }` in `grauwolf-llm`; `PromptExecutor::moderate` routes and retries as usual.
  - Default for every provider: a separate classification request via `execute_structured` with a fixed prompt (system instruction plus the text as user message) and the schema `{ harmful, categories }`. It runs on its own prompt; the agent's conversation history remains untouched.
  - OpenAI supports moderation natively via `POST /v1/moderations`, if a moderation model is explicitly chosen (model ID contains `moderation`, e.g. `omni-moderation-latest`); otherwise the default path with the chat model applies, because compatible gateways often do not offer `/moderations`. `grauwolf_llm::classify_moderation` is public for this purpose.
- Node kind `llm_moderate` (`params { type, provider?, id? }`, `string → <type>`): checks the text with the model from `provider` and `id` (only together, as with `set_model`) or with the active model, and yields `{ harmful, categories, text, message }`; `message` is a rejection for the user. Continuing works with the existing building blocks `on_field harmful` and `get_field text`/`get_field message`.
- `single_run` accepts `params.moderate: true`: the strategy places the check before the first LLM call and creates the type `Moderation` itself (like `verified`, ADR-0041); harmful input ends with the rejection, without the model seeing it.
- Interpreter and generator support the node kind equally (ADR-0018).
- Observability (added, User 2026-11-08: "make moderation visible in events and OTel"): dedicated events `moderation_starting { model }`, `moderation_completed { harmful, categories }`, `moderation_failed { error }`, deliberately not `llm_call_*`, so that evaluations of chat calls stay correct. OpenTelemetry records them as span `moderate <model>` with `gen_ai.operation.name = moderation` and the verdict. The checked text appears neither in the event nor in the span, not even with content recording: it is already in the run's input, and harmful content should not additionally accumulate in tracing systems.

## Consequences
- Moderation costs one additional request per check (free natively with OpenAI).
- The quality of the default path depends on the model; for reliable checks, specify a dedicated `model`.
- Only the input is checked; responses and tool results are checked by graphs as needed, with further `llm_moderate` nodes.
