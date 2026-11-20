# ADR-0063: Tool choice for models without forced tool choice

- **Status:** Accepted (User, 2026-11-08: "first fix the tool_choice problem")
- **Date:** 2026-11-08

## Context
ADR-0042 sends `Required` and `Tool(name)` as Anthropic `tool_choice` `{type: any}` and `{type: tool, name}`, respectively. Claude Fable 5.1, Claude Mythos 5.1, Claude Opus 5.5 and Claude Sonnet 5.5 reject that with `400` ("tool_choice: type "tool" and "any" are not supported for this model."). The nodes `llm_request_only_tools` and `llm_request_force_tool` therefore always fail there.

Anthropic recommends `auto` plus an appended system message that demands the call, and a check whether it happened. The message must stay in the history: these models treat later changes to the history as an edit ("preserved thinking"), which devalues thinking blocks and, on newer accounts, leads to `400`. The adapter does not own the history and therefore cannot insert the instruction permanently.

## Decision
- Port: `LlmClient::forces_tool_choice(&LlModel) -> bool` (default `true`); `PromptExecutor::forces_tool_choice(model)` asks the client of the provider.
- The Anthropic adapter reports `false` for `claude-fable-5-1`, `claude-mythos-5-1`, `claude-opus-5-5` and `claude-sonnet-5-5` (also with prefix/suffix, e.g. Bedrock IDs). New models are added to the list as soon as they show the same behavior.
- `AgentContext::request_llm_with(Required | Tool(name))` for such a model:
  - permanently appends a system message to the history that demands the call (naming the tool for `Tool(name)`),
  - requests with `Auto`,
  - if the required call is missing, the response is discarded (never appended) and requested again exactly once; the second response is appended in any case.
- Other providers and models behave unchanged.

## Consequences
- The nodes work with the new Claude models; the history stays append-only.
- With these models, the history contains additional system messages; they belong to the conversation and are stored with it.
- Without forcing there is no guarantee: even after the retry, a response without a call may come; the graph's edges handle that as before.
- The model list must be maintained.
