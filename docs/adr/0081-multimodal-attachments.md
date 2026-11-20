# ADR-0081: Multimodal attachments on user messages

- **Status:** Accepted (User, 2026-11-09: "then let's start with multimodal content")
- **Date:** 2026-11-09

## Context
The feature catalog lists multimodal content (F01.5: image, audio, file). So far a user message consists only of text; images or PDFs cannot be given to an agent, although all built-in providers understand at least images.

## Decision
- **Model:** `Message::User { content, attachments }`; `attachments: Vec<Attachment>` is missing in older histories and is not written when empty. `Attachment { media_type, data }` with `AttachmentData::{Base64(text), Url(text)}`. The kind follows from `media_type`: `image/*`, `audio/*`, `application/pdf`.
- **Providers** (phase 1): OpenAI: images (Base64 as data URL or URL), audio `wav`/`mp3` (Base64), PDF (Base64); Anthropic: images and PDF (Base64 or URL); Ollama: images (Base64). What a provider cannot handle, its adapter rejects before the request with `LlmError::Client` and a clear message (provider, media type, what is possible); nothing is silently left out.
- **Path into the run:** `RunExtras.attachments`; the interpreter gives them to the context (`AgentContext::attach_to_input`), which attaches them to the next user message that is appended, i.e. the input of the run. Checkpoint runs take them as well.
- **Inputs:** CLI `spec run --attach <file>` (repeatable; media type from the extension: png, jpg/jpeg, gif, webp, pdf, wav, mp3; at most 20 MB per file); `spec chat` with the line `/attach <file>` for the next input; service `/run`, `/run/stream` with `attachments: [{ media_type, data | url }]`.
- **Not in phase 1:** A2A/MCP, attachments in tool results, Gemini, generated code (whose `run` takes only the input).

## Consequences
- Stored conversations contain attachments as Base64 and become correspondingly large.
- The mock LLM and moderation see only the text; interceptors see the attachments in the prompt.

## Addendum 2026-11-09: Implementation and live test
Implemented as decided. Checked live with `claude-haiku-5-5` (PNG, PDF, rejection of `audio/wav` before the request, reference to the image in a later chat line) and Ollama `qwen3.5:9b` (PNG). The service reports attachments that have neither exactly one of `data` and `url` with `400` and `attachments/<i>: …`.
