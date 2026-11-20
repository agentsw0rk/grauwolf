# ADR-0047: Intervening interceptors for LLM and tool calls

- **Status:** Accepted (User, 2026-11-08: "yes", item 8 of the agent features)
- **Date:** 2026-11-08

## Context
Features receive events read-only (ADR-0007, F06.1–F06.3). What is needed are interceptors that intervene (F06.4): change the prompt before an LLM call, post-process answers, rewrite tool arguments or deny calls. Typical cases: privacy filters, additional instructions, approval of dangerous tools, truncating large tool results.

## Decision
- Trait `Interceptor` in `grauwolf-agents::events` with default implementations (do nothing):
  - `before_llm_call(&mut Prompt, &mut Vec<ToolDescriptor>)` – affects only this request, not the history;
  - `after_llm_call(&mut Message)` – before appending to the history;
  - `before_tool_call(&mut ToolCall) -> ToolDecision` (`Proceed` or `Deny(text)`; denied calls return `text` as the tool result to the LLM);
  - `after_tool_call(&mut ToolResult)`.
- `AgentContext::intercept(interceptor)`; all request kinds (normal, tool choice, multiple answers, structured, streaming) go through the same hooks. Multiple interceptors act in installation order; forks of parallel branches inherit them.
- `Interpreter::with_interceptor(..)` installs them in every run. Interceptors are code, not part of the DSL.

## Consequences
- Events show what was actually sent or executed (after the interceptors).
- Interceptors run synchronously and should be fast; approvals that wait for humans follow as an asynchronous variant when needed.

## Addendum (2026-11-09): asynchronous approval implemented
The asynchronous variant for approvals that wait for humans is implemented as the hook `approve_tool_call(&ToolCall) -> BoxFuture<ToolDecision>` on the same trait; it runs after the synchronous `before_tool_call` hooks (ADR-0061).

## Addendum: moderation (User, 2026-11-09: "apply prompt interceptors to moderation")
Moderation requests (ADR-0066) used to bypass the interceptors. `AgentContext::moderate` now puts the text to be checked into a prompt with the ID `MODERATION_PROMPT_ID` (`moderation`) and a user message and runs all `before_llm_call` hooks over it (without tools). The text of the user messages afterwards is what gets checked – so an interceptor can, for example, mask personal data before the text goes to a moderation service. The ID makes moderation requests recognizable to interceptors. `after_llm_call` does not apply here because the verdict is not a message; the node `llm_moderate` still passes on the original text in the field `text`.
