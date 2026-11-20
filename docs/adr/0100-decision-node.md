# ADR-0100: Decision node with an external decider

- **Status:** Accepted (User, 2026-11-10: "do the decision node first … also look at https://github.com/agentsw0rk/RKEV"; ADR-0092, F14.12)
- **Date:** 2026-11-10

## Context
AgentWeaver has decision nodes: a typed question about the current value routes the graph by the
answer, while the value itself passes on unchanged. Three kinds exist: `choice` (one of named
options), `noul` (yes/no; yes when the probability reaches a threshold) and `score` (a level of an
ordered scale; the mean level index is rounded to a level). An edge can stand for several answers;
each answer has exactly one edge.

AgentWeaver sends these questions to a *System One* service instead of letting the language model
write text that must be parsed. RKEV (github.com/agentsw0rk/RKEV) implements that API locally with
small decision models (Kev): `POST {RKEV_BASE_URL}/v1/systemone`, optional
`Authorization: Bearer {RKEV_API_KEY}`, request
`{state, model?, questions: {<id>: {type, instructions, criteria?}}}` – `criteria` is an ordered
object of option name → description or `null` (`choice`), an ordered list of level descriptions,
lowest first (`score`), or `{"true": …, "false": …}` (`noul`, optional) – and response
`{model, answers: {<id>: {type: "noul", noul} | {type: "choice", choice, confidence, probabilities}
| {type: "score", score, confidence, legend, probabilities}}, usage?, latency_ms?}`. Errors are
`{"error": {type, message}}` with 400/401/404/413/500, and 503 when all slots are busy (returned
immediately). The state may be a string or any JSON value.

grauwolf branches on values with conditions on edges; a condition may pass or transform the value
(ADR-0041), and it can read the agent context.

## Decision
- **Port `Decider`** (domain, `grauwolf-agents`): `decide(&Decision, state: &Value) ->
  BoxFuture<Result<Verdict, DecideError>>`.
  - `Decision { kind, question, model: Option<String> }`, `kind`: `Choice { options: [(name,
    description?)] }`, `YesNo { threshold, yes?, no? }`, `Score { levels: [description] }`.
  - `Verdict { answer: String, probability: Option<f64>, confidence: Option<f64>, probabilities:
    Vec<(String, f64)>, model: Option<String> }`. `answer` is the option name (`choice`), `yes`/`no`
    by the threshold (`yes_no`) or the level description after rounding the mean index and clamping
    it to the scale (`score`) – as AgentWeaver does.
- **Catalog node `decide`** (`T → T`, the value passes unchanged): parameters `kind`
  (`choice` | `yes_no` | `score`), `question`, `options` (`[{name, description?}]`, `choice`),
  `levels` (`[string]`, `score`, lowest first), `threshold` (0–1, default 0.5, `yes_no`), `decider`
  (`system_one` | `llm`, default `llm`), `model` (optional: System One checkpoint name, or a spec
  model for `llm`). The value is the `state` (text as a string, other values as JSON). The verdict is
  stored in the agent storage under `grauwolf.decision` (part of checkpoints).
- **Condition `on_decision`** (`T → T`): parameter `answers: [string]`; passes the value when the
  latest verdict's answer is one of them.
- **Validation:** an `on_decision` edge must leave a `decide` node; answers must exist for that
  node (option names, `yes`/`no`, level descriptions); every answer is covered by exactly one edge
  of the node; options and levels 1–255 and unique; `threshold` in 0–1.
- **Deciders:**
  - `llm` (domain): a structured request on the active model (or `model`) with an enum of the
    options/levels, or `yes`/`no` plus a probability; no network service needed.
  - `system_one` (adapter `grauwolf-systemone`, reqwest): the request above with the question id
    `decision`; base URL `RKEV_BASE_URL`, key `RKEV_API_KEY`; timeouts 10 s connect / 60 s
    request; 503 and network errors are transient (retried with the retry policy), other errors fail
    the node with the service's message.
  - The facade provides `system_one` from the environment; a spec that uses it without
    `RKEV_BASE_URL` fails before the run with a clear error.
- **Events:** `decision_starting { node }`, `decision_completed { node, answer, probability,
  confidence, model }`, `decision_failed { node, error }` – without the state; also as OpenTelemetry
  spans.
- Interpreter and generated code use the same runtime functions; the generated crate adds the
  System One client only when a spec uses it.

## Consequences
- AgentWeaver decision nodes map 1:1 (`noul` → `yes_no`, `onDecision` edges → `on_decision`).
- Decisions work without a System One service through the model, at the cost of tokens and of less
  calibrated probabilities.
- The System One format belongs to RKEV/AgentWeaver; changes there must be followed in the adapter.
- `on_decision` reads the latest verdict, so a `decide` node must be followed directly by its
  `on_decision` edges (enforced by the validation).

## Implementation notes (2026-11-10)
What the implementation settled where the decision left room, or changed:
- **`value` parameter:** like `store` and `set_model`, `decide` names the type of the passed-through
  value in `value` (`T → T`); the catalog needs it to type the edges. `kind`, `question` and `value`
  are required.
- **`yes`/`no` parameters:** `yes_no` accepts optional descriptions of what yes and no mean. System One
  receives them as `criteria` (`{"true": …, "false": …}`), the model decider in its request.
- **`model` of the `llm` decider** is written `provider/id` (a model of the same kind as in
  `set_model`, in one text); the provider counts toward `AgentSpec::providers()` and
  `provider_uses()`, so unknown providers are found before the run. For `system_one` it is the
  checkpoint name, passed on unchanged.
- **`llm` and `yes_no`:** the model is asked for the probability only, and the threshold decides –
  one rule for both deciders, no way for a model to answer "no" with a probability above the
  threshold. For `choice` and `score` the answer is an enum of the option names or level
  descriptions; an answer outside it is an error.
- **Validation** goes further than listed: every edge that leaves a `decide` node must use
  `on_decision` (the condition reads the latest verdict, and any other edge would bypass the
  decision). The parameters themselves are checked by the node catalog (`InvalidParams`), the edges
  by the spec (`DecisionEdgeSource`, `DecisionEdgeCondition`, `UnknownDecisionAnswer`,
  `DecisionAnswerUncovered`, `DecisionAnswerDuplicated`). `AgentSpec::uses_system_one()` tells the
  runtime that a service is needed.
- **Node ID in events:** `decision_*` carry the ID of the node; the interpreter and the generated
  code pass it to the node (`implementations::decide(id, params)`, `nodes::decide::<T>(id, params)`).
- **Verdict storage:** `grauwolf.decision` holds `{ node, answer, probability?, confidence?,
  probabilities, model? }`; `on_decision` reads `answer`.
- **Retries:** `RetryingDecider` (domain, `grauwolf-agents`) wraps a decider and asks again after
  `DecideError::Transient` as the retry policy allows; the facade wraps the System One client in it
  with `RetryPolicy::default()` and `TokioClock`.
- **Timeouts:** the adapter builds its own `reqwest` client (10 s connect, 60 s per request), not
  the `grauwolf-llm-http` base: that one has a connect timeout of 30 s and no total timeout.
- **Wiring:** `Interpreter::with_decider`, `AgentLoader::with_decider`, `AppOptions.decider`; the
  facade function `systemone::decider_for(spec, provided, env)` builds the client from
  `RKEV_BASE_URL`/`RKEV_API_KEY` and fails with a clear message before the run when a spec needs a
  service and finds none. Generated crates with `system_one` decisions get `run_with_decider` and
  enable the facade feature `systemone` for `run_app`; others do not carry the client.
- **Not covered:** the endpoints `/v1/systemone/separate` and `/v1/systemone/permute` of RKEV, and
  several questions per request (every decision is its own request with the question ID `decision`).


## Implementation notes (2026-11-11)
- **`temperature` and `think`** are optional parameters of `decide` for the `llm` decider: they apply
  to the model of the decision request (`model`, or the active model) and to this request only –
  the active model and later requests stay unchanged (ADR-0097). `Decision` carries them;
  `AgentContext::decide_with_model` adds them to the model, so interpreter and generated code
  behave the same.
- **Validation:** `temperature` 0–2 (`TemperatureOutOfRange`) and `think` only for the provider
  `ollama` (`ThinkNotSupported`; the provider of `model`, else of the spec's main model), at
  `…/params/temperature` and `…/params/think`. With `decider: system_one` both are an error
  (`SamplingWithSystemOne`): a decision model has no sampling settings of a language model.
