# ADR-0105: Amazon Bedrock as a provider

- **Status:** Accepted (User; ADR-0092, F14.18) – implemented, see "Implementation notes"
- **Date:** 2026-11-10

## Context
AgentWeaver supports Bedrock as a model provider (`AWS_BEDROCK_ACCESS_KEY`,
`AWS_BEDROCK_SECRET_ACCESS_KEY`, `AWS_BEDROCK_REGION`, default `us-west-2`). grauwolf has adapters
for Ollama, OpenAI (and compatible), Anthropic and Gemini (ADR-0005, ADR-0082, ADR-0083).

## Decision
- **New adapter `grauwolf-bedrock`** on the Bedrock Runtime **Converse API** (`/model/{id}/converse`
  and `/converse-stream`), which covers all chat models on Bedrock with one message format,
  tool use (`toolConfig`, `toolUse`/`toolResult` blocks), system prompts, images and documents as
  attachments (ADR-0081), and usage.
- **Signing:** AWS Signature Version 4 on reqwest – not the full AWS SDK (own implementation instead
  of the `aws-sigv4` crate, see the implementation notes). Streaming responses use the AWS event-stream framing (decoded in the adapter).
- **Credentials:** standard AWS variables `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
  `AWS_SESSION_TOKEN`, `AWS_REGION`; alternatively a Bedrock API key `AWS_BEARER_TOKEN_BEDROCK`. The
  AgentWeaver export maps its `AWS_BEDROCK_*` names onto these in the deployment.
- Provider name `bedrock`; model ids as Bedrock expects them (including inference profiles like
  `eu.anthropic…`). Structured output via tool use (forced tool), tool choice native.
- Embeddings (Titan, Cohere on Bedrock) follow when needed.
- Conformance suite (ADR-0005) and recorded HTTP responses as for the other adapters.

## Consequences
- AgentWeaver agents with Bedrock export to grauwolf.
- One more adapter to maintain; the event-stream decoder is the only new protocol piece.

## Implementation notes
Status of the code: adapter `grauwolf-bedrock`, provider `bedrock`, facade/host feature `bedrock`
(on by default).

- **SigV4 is implemented in the adapter** (`sigv4`: canonical request, string to sign, HMAC-SHA256
  key derivation; about 150 lines on `sha2` and `hmac`, both already in the workspace) instead of
  using `aws-sigv4`. `cargo tree` for `aws-sigv4` alone shows 58 crates, among them
  `aws-smithy-runtime-api`, `aws-smithy-types`, `aws-smithy-async`, `aws-smithy-http` and
  `aws-credential-types` – the smithy runtime that the decision wanted to avoid, and a second
  generation of the `http` types next to reqwest's. Our own code adds only `hmac` (already in
  `Cargo.lock` through the SQL driver). The same reasoning as for the OCI client. It is tested
  against the published AWS examples (signing key derivation, IAM `ListUsers`, `get-vanilla`,
  `post-vanilla`). Service `bedrock`; signed headers `host`, `x-amz-date` and, with temporary
  credentials, `x-amz-security-token`; the path is encoded twice in the canonical request, as for
  every service but S3 (a `:` in a model id is `%3A` in the URL and `%253A` in the signature).
  `HttpClient` gained `with_signer` (headers computed from the finished request) and `post_chunks`
  (raw body chunks) for this; credentials and signatures appear in no `Debug` output or error.
- **Authentication:** access keys (SigV4) or `AWS_BEARER_TOKEN_BEDROCK` (`Authorization: Bearer`, no
  signing); the API key wins if both are set. Region from `AWS_REGION`, else `AWS_DEFAULT_REGION`,
  else `us-west-2`; `BEDROCK_BASE_URL` overrides the endpoint. All in `grauwolf::config`
  (`ProvidersConfig`, `VARIABLES`). Missing credentials fail at startup naming the variables.
  Instance profiles, EKS Pod Identity and web identity are **not** read (open point).
- **Event stream:** `eventstream::Decoder` reads the binary framing of `converse-stream` (prelude
  with both lengths and its CRC32, headers, payload, message CRC32; both checksums verified; chunks
  may split anywhere; exceptions carry their type). A damaged frame or a stream cut off before
  `messageStop` is an error; frames before the damage are still delivered. Usage arrives in the
  `metadata` event after `messageStop`, so `Usage` and `End` are emitted when the transport ends.
- **Model reporting:** the Converse API does not say which model answered (neither in the response
  nor in the stream), so `Completion.model` is `None` and no `StreamFrame::Model` is emitted
  (ADR-0094).
- **`think`:** not sent (ADR-0097: Ollama only; `spec validate` rejects it for other providers).
  Reasoning blocks that a model returns (`reasoningContent`, also redacted) are kept as
  `Reasoning { provider: "bedrock" }` and sent back unchanged; foreign reasoning is dropped.
- **Tool choice:** `auto` is not sent, `Required` is `any`, `Tool(name)` is `tool`. Converse has no
  `none`: with `ToolChoice::None` the tools are left out of the request – unless the history
  already contains `toolUse`/`toolResult` blocks, which the API only accepts together with a
  `toolConfig`; then the tools stay with `auto`. Models that cannot force a tool call (the list of
  ADR-0063, matched in Bedrock ids too) report `forces_tool_choice() == false`.
- **Structured output:** a forced tool named after the type whose input schema is the schema of the
  answer (a non-object root is wrapped in `{ value }`); the tool input is returned as JSON text.
  For models that cannot force tools the schema goes into the prompt (the manual strategy).
- **Messages:** consecutive messages of the same role are merged (Converse wants alternating
  roles); blank text blocks are left out; tool results are `json` for JSON objects, else `text`.
  Attachments (ADR-0081): png/jpeg/gif/webp as `image`, pdf/csv/doc/docx/xls/xlsx/html/txt/md as
  `document`, as bytes or from an `s3://` URL; audio, video and other URLs are rejected before the
  request.
- **Errors:** 400 (ValidationException), 403 (AccessDenied: model access not granted), 404
  (unknown model) are `LlmError::Client` with an explanation; 408/429/5xx (Throttling,
  ServiceUnavailable, InternalServer, ModelTimeout) are transient and retried by the policy of the
  executor. Guardrail or content-filter stops (`guardrail_intervened`, `content_filtered`) are
  client errors.
- **Embeddings:** not implemented; an embedding model with provider `bedrock` is
  `ProviderError::NoEmbeddings` (follows when needed).
- **Generated crates** enable the facade feature `bedrock` when the spec uses the provider.

### Deviations and open points
- No prompt-cache breakpoints (`cachePoint`) are sent: not every model accepts them, and a model
  that does not answers with a validation error. The response cache of the executor (ADR-0086)
  works as for every provider; cache read/write tokens of the response are reported (ADR-0067).
- `maxTokens` is only sent when configured (`with_max_tokens`); the model's default applies.
- **Live check:** no AWS account was available. The adapter is verified against handwritten
  responses in the documented Converse formats, hand-built event-stream frames and the SigV4
  examples only. A live check needs credentials; `crates/adapters/grauwolf-bedrock/tests/live.rs`
  runs the conformance suite (ADR-0005) when they are set and is skipped otherwise.
