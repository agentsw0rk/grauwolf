# ADR-0042: Request kinds – tool choice in the prompt, "best of n" via the port

- **Status:** Accepted (User, 2026-11-07: "yes, keep going", item 3 of the agent features)
- **Date:** 2026-11-07

## Context
Besides the normal request, there is a need for: calling only tools, forcing a specific tool, and multiple responses to choose from (with a selection strategy), F04.7/F05.9. grauwolf so far sends only history and tools; request parameters are missing entirely.

## Decision
- `Prompt` gains `params: LlmParams` (serde default, so old checkpoints remain readable) with `tool_choice: ToolChoice` = `Auto` (default) | `Required` | `None` | `Tool(name)`. The port signature stays; adapters read the parameters from the prompt.
  - OpenAI: `tool_choice` `auto`/`required`/`none`/`{type: function, function: {name}}`.
  - Anthropic: `tool_choice` `{type: auto|any|none|tool, name}`.
  - Ollama does not know tool choice: `Tool(name)` offers only this tool, `Required` appends a system instruction, `None` offers no tools.
- `AgentContext::request_llm_with(tool_choice)` sets the choice only for this request.
- Node kinds: `llm_request_only_tools` (`string → message`, tool call mandatory) and `llm_request_force_tool` (`params { tool }`, `string → message`).
- "Best of n": `LlmClient::execute_choices(prompt, model, tools, n)` returns n responses; default = n individual requests, OpenAI natively via `n`. Node kind `llm_request_best_of` (`params { choices, criteria }`, `string → message`): asks for n responses; one evaluation call without tools chooses the best via a structured response `{ best }`; only that one goes into the history.

## Consequences
- Tool choice works the same across providers, only approximated with Ollama.
- "Best of n" costs n + 1 requests; the default path works with every provider.
