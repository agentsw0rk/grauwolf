# ADR-0043: Model switching during a run and a separate model per subgraph

- **Status:** Accepted (User, 2026-11-07: "yes+", item 4 of the agent features)
- **Date:** 2026-11-07

## Context
An agent should be able to switch the LLM during a run and carry the history along (F04.9); subgraphs may have their own model (F05.6). Typical use: a small, fast model for tool calls, a large one for the final answer. In grauwolf a run so far has exactly one model.

## Decision
- `AgentContext::set_model(model)` switches the model and returns the previous one; the history remains. Checkpoints already include the active model.
- `scopes::SubgraphScope` combines the tool subset and model of a subgraph: set on entering, reset on leaving (also after errors). `ToolSubset` remains as a shorthand.
- Specs: `subgraphs.<name>.model` (`{ provider, id }`, optional). Node kind `set_model` (`params { provider, id, value }`, signature `value → value`) switches for the rest of the run.
- The executor needs clients for all providers of a spec: the facade builds it with `executor_for(providers)` from all named providers (main model, subgraphs, `set_model`).

## Consequences
- A run can mix providers; reasoning parts of one provider are already ignored by other adapters (ADR-0032).
- Resuming after an abort takes the most recently active model; when re-entering the subgraph, its model applies again.
