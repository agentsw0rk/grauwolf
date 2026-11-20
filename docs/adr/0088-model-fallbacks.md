# ADR-0088: Fallback models on errors

- **Status:** Accepted (User, 2026-11-09: "Next, 1 (fallback between models) in case several models are entered. Check this.")
- **Date:** 2026-11-09

## Context
If a provider fails or is overloaded, the run fails as soon as the retries (ADR-0032) are exhausted, even though grauwolf knows five provider families (F02.10).

## Decision
- **Spec:** every model (`model`, a subgraph's model, `set_model`, a dedicated moderation model) can have `fallbacks: [{ provider, id }, …]`; fallbacks have no fallbacks of their own.
- **Check** ("Check this"): `spec validate` reports a fallback equal to the main model, or duplicate entries (`DuplicateFallback` at `/…/fallbacks/<i>`); the providers of the fallbacks count toward `provider_uses()` and `providers()`; unknown ones are reported by `spec validate` as for the main model, and the executor receives their clients.
- **Model:** `LlModel.fallbacks` (omitted in JSON when empty), `LlModel::with_fallbacks(..)`.
- **Flow** in the `AgentContext`: every LLM request (normal, structured, multiple responses, compression, facts, moderation) first asks the model; if it fails, after the executor's retries, the fallbacks one after another. No switch on `BudgetExceeded`. Streaming switches only as long as no frame has been passed on yet.
- **Visible:** event `llm_fallback { from, to, error }`; usage and costs count toward the model that answered. The active model of the context remains unchanged; the next request again tries the actual model first.
- Without fallbacks, nothing changes.

## Consequences
- A run survives the failure of a provider if a second one is entered and set up (keys in the environment).
- Responses of different models can appear in the same run; reasoning of foreign providers is, as before, not sent back.
