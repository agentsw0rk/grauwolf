# ADR-0091: English everywhere except the German handbooks

- **Status:** Accepted (User, 2026-11-10: "All content in the project should be English, except the two things: the handbooks and the developer handbook. The .claude directory should go, too.")
- **Date:** 2026-11-10

## Context
ADR-0090 made the guides, their examples, README.md and all messages English but kept code comments,
ADRs, the crate `AGENTS.md` files, test data, the roadmap and the built-in instructions to the model
German. A repository that mixes both languages is hard to read for contributors who do not speak
German. The German handbooks were restored next to the English guides as an alternative (addendum to
ADR-0090).

## Decision
- **English:** all content of the repository – README.md, the guides in `docs/user-guide/` and
  `docs/developer-guide/` with their examples, ADRs (including the existing ones, translated in
  place), `docs/architecture.md`, the feature catalog, `AGENTS.md` files, code and doc comments,
  test data and fixtures, example specs, messages, identifiers, and the built-in instructions sent to
  the model (history compression, facts, moderation, verification, planner, builder, …).
- **German:** only `docs/handbuch/` and `docs/entwickler/`, a snapshot of 2026-11-10 with their
  examples. They are updated only when needed; their example specs stay tested.
- **`.claude/`** (local progress notes of AI agents) is not part of the repository and is ignored by
  git. `.cargo/config.toml` stays: it defines the `cargo xtask` alias.

## Consequences
- New content is written in English; supersedes the language split of ADR-0090 and ADR-0085.
- The built-in instructions to the model changed language; they were checked with live runs against
  a small local model, because their wording influences the results.
- The German handbooks may lag behind; each starts with a note on its state and links to the
  maintained English guide.

## Addendum (2026-11-10): sharpened instructions
Two built-in instructions are no longer literal translations: the default verify prompt
(`DEFAULT_VERIFY_PROMPT`, `llm_verify` and the strategy `verified` without `verify_prompt`) and the
system prompt of the plan critic. The literal English versions behaved badly with `qwen3.5:9b`
(the verifier accepted wrong answers or rejected correct ones repeatedly; the critic rejected every
plan until `plan not fulfilled after 8 steps`). The mechanism is unchanged (`correct`/`feedback`,
`should_continue`/`reason`); only the wording changed.

- **Verifier:** judges only the content of the answer against the task and does not ask for tool
  calls or working steps; `correct` is false only for a wrong or incomplete answer, otherwise true
  with empty feedback.
- **Critic:** `should_continue = false` makes the planner discard the plan and build a new one, so
  the prompt says exactly that. A critic that called a plan with a reached goal "pointless" answered
  false and sent the planner into a replan loop. Now `true` is the normal case and `false` is for
  remaining steps that cannot reach the goal. The same change translated three German fragments
  that the language switch had missed in the text sent to the planner and critic (`Plan – Ziel:`,
  `[erledigt]`, `Bisherige Ergebnisse:`).

Measured with `qwen3.5:9b` (Ollama, think off), temporary specs/binaries, not part of the
repository. Verifier: two arithmetic questions (`120 + 35 + 7`, `1000 + 234 + 56 + 3`) with the
tool `add`; "wrong/loop" counts runs that ended with a wrong answer accepted or without an
accepted answer (limit/timeout); runs where the agent failed before the verifier (malformed tool
call) are excluded. Critic: `06-plan.json` with "Name one sight in Dresden and one nearby cafe.".

| Instruction | Variant | Runs | Result |
|---|---|---|---|
| verify | German original | 18 | 15 ok, 3 wrong |
| verify | English literal | 16 | 11 ok, 2 wrong, 3 loop/timeout |
| verify | sharp (chosen) | 16 | 14 ok, 2 wrong |
| verify | sharp, "when in doubt accept" | 17 | 8 ok, 9 wrong |
| critic | English literal (German fragments) | 5 | 2 finished, 3 plan not fulfilled |
| critic | English literal, fragments translated | 5 | 3 finished, 2 plan not fulfilled |
| critic | German original | 3 (+1 early) | 2 finished |
| critic | sharp (chosen) | 5 | 5 finished, no replan |
| critic | sharp, other wording | 5 | 3 finished, 1 plan not fulfilled, 1 timeout |
| critic | two earlier sharp wordings ("stop when the goal is reached") | 1 each | both plan not fulfilled |

The differences for the verifier are within the noise of 16–18 runs: no wording is reliable because
the small model also makes arithmetic mistakes itself; the chosen one was the best and never ran
into the node limit. The critic result (5/5 against 2/5 and 3/5) is clearer but still a small
sample. The longer guide input ("Plan two days in Hamburg …") ran into timeouts with every variant
(one step takes minutes) and does not discriminate. All other built-in instructions remain literal
translations.
