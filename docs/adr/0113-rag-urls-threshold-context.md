# ADR-0113: RAG: URL sources, similarity threshold and knowledge as context

- **Status:** Accepted (User, 2026-11-13)
- **Date:** 2026-11-13

## Context
`knowledge` (ADR-0038, ADR-0057, ADR-0106) indexes local files and offers the search as a tool. The
RAG settings of AgentWeaver (`urlSources`, `similarityThreshold`, `maxDocuments`, `injectAsContext`,
`provideAsTool`) go further: web pages as sources, a minimum similarity, and the best sections added
to the prompt automatically. Three of them were missing in grauwolf.

## Decision
Mapping: `urlSources` -> `knowledge.urls`, `similarityThreshold` -> `knowledge.min_similarity`,
`maxDocuments` -> `knowledge.top_k` (existing), `injectAsContext` -> `knowledge.inject`,
`provideAsTool` -> `knowledge.tool_enabled`.

1. **Spec.** `knowledge.sources` stays (files and directories relative to the spec). New
   `knowledge.urls: ["https://...", ...]`, a separate list. Validation (`InvalidUrl` at
   `/knowledge/urls/<i>`, the URL is never repeated): `https://` (plain `http://` only for
   `localhost`/`127.0.0.1`), a host, no credentials, no fragment, no duplicates. At least one of
   `sources`/`urls` is needed (`NoKnowledgeSources`).
2. **Fetching at start with a cache** (User decision). Every start (interpreter, `spec serve`,
   generated apps through the host) fetches every URL with a conditional request
   (`If-None-Match`/`If-Modified-Since` from the validators stored with the index). `304` means
   unchanged; a changed document goes through the incremental path (ADR-0057/ADR-0106), so only
   changed chunks are embedded. A URL removed from the spec loses its chunks. If a URL fails
   (network, 4xx/5xx, size, type) and indexed content for it exists, that content is kept and a
   warning goes to stderr; without indexed content the start fails naming the URL and the reason.
   Validators live in the index file (file store) and in the table `grauwolf_url_validators`
   (PostgreSQL).
3. **HTTP rules.** The URLs are the allow list: the spec author names them, so there is no host
   list and no private-address block (a local documentation server is a valid source). Redirects
   are followed (at most 5) only within the same origin (scheme, host, port); a redirect elsewhere
   is a failure with the target host named. 30 s timeout, at most 5 MiB per URL (checked on the
   declared length and while streaming), generic `User-Agent: grauwolf`. Cookies and
   authorization are never sent.
4. **Formats** by `Content-Type` (fallback: the extension): `text/plain`, `text/markdown`,
   `text/html` and `application/xhtml+xml`. HTML becomes readable text by a small converter of our
   own in `grauwolf-tools-web` (no new dependency; `regex` is already allowed there): `script`,
   `style`, `noscript`, `svg`, `nav`, `header`, `footer`, `aside`, `form` and comments are dropped;
   headings become `#` lines, paragraphs and list items (`- `) their own lines, table cells are
   separated by ` | `; entities (named basics and numeric) are decoded. PDF and other types are
   refused with a clear message. Charset from the header (`charset=`; UTF-8 default,
   ISO-8859-1/Latin-1 supported, others refused).
5. **Indexing.** The source name of a URL chunk is the URL (search results name it). Chunking and
   hashing equal the file case.
6. **`knowledge.min_similarity`** (0..=1, optional): hits below it are dropped on both stores
   (applied by the retriever, so the PostgreSQL store needs no SQL change). Order and `top_k`
   are kept; with nothing left the tool answers `No matches.`.
7. **Knowledge as context.** `knowledge.inject: true` (default `false`): before every request
   that starts from a new user input (same trigger as `auto_recall`, ADR-0069) the best `top_k`
   chunks (honouring `min_similarity`) are appended as a system message naming the sources.
   `knowledge.tool_enabled` (default `true`) turns the search tool off so only the injection is
   used; both off is a validation error (`UnusedKnowledge`). The event `knowledge_injected
   { sources, count }` carries no text. Failures to retrieve do not abort the run (event
   `knowledge_failed { error }`), like memory.

## Implementation notes
- **Knowledge context without a new port in the host:** `DynTool::knowledge_context()` (default
  `None`) lets the search tool hand its retriever to the agent; the interpreter and the generated
  crates take it from the provided tools by the name `knowledge.tool`. A generated crate with
  `inject` always takes external tools (`EXTERNAL_TOOLS` may be empty).
- **File store granularity:** a changed URL is embedded again as a whole file (ADR-0057 is per
  file); only the PostgreSQL store compares chunk hashes.
- **PostgreSQL:** a rebuild (model or dimension change) cannot keep unchanged URL sources and fails
  with a message; the next start fetches everything again.
- **HTML converter:** own, regex-based, in `grauwolf-tools-web` (no new dependency); `header` drops
  a page heading that sits inside a `<header>`.
- The AgentWeaver import maps the fields as listed above; German handbooks are unchanged.

## Consequences
- Web documentation can be a knowledge source without a manual download step; startup costs one
  conditional request per URL.
- A model without tool support can use knowledge through `inject`.
- Redirects to other hosts need the final URL in the spec.

## Live check (2026-11-13)
Local page served with `python3 -m http.server` (it sends `Last-Modified` and answers conditional
requests with `304`), Ollama `qwen3.5:9b` and `nomic-embed-text`, `OLLAMA_THINK=false`, file index:
- **Search tool:** with the default prompt the small model answered from its own head and did not call
  the tool (the reason for `inject`); with a prompt that demands the call it answered "Sigurd ... nine
  in the morning" from the page.
- **`inject: true`, `tool_enabled: false`:** "When are the wolves fed?" -> "at eight in the evening",
  source named as the URL; the page's navigation and footer text did not appear.
- **Unchanged page:** the server logged `GET ... 304` at the next starts, nothing was embedded
  (answers came in about 5 s).
- **Changed page** (keeper and time edited): `GET ... 200`, the new answer ("Sigurd ... nine in the
  morning") came from the re-embedded page; the following start was `304` again.
- **Server down:** with the index, a warning `knowledge URL ...: ...; using the last indexed content`
  and the answer from the indexed page; with a fresh index the start failed naming the URL and
  the reason.
- **PostgreSQL store:** not run live. The owner's container at `host.docker.internal:5432` refused the
  connection from this container ("cannot connect to the PostgreSQL server named by
  PGVECTOR_URL"), so the new table `grauwolf_url_validators` and its SQL are tested only against the
  in-memory stand-in; `tests/postgres.rs` has no URL case yet.
