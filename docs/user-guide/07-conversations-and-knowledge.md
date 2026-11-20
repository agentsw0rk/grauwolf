# 7 Conversations, memory and knowledge

A run with `spec run` always starts from scratch: the agent knows only its system prompt and your
input. That is not enough for real assistants. This chapter shows three ways for your agent to know
more:

| What | How long | What for |
|---|---|---|
| **Conversation** (`spec chat`, `--conversation`) | one conversation, saved if you want | follow-up questions and references to what was said earlier |
| **Long-term memory** (`long_term_memory`) | across all conversations | facts about users: name, preferences, plans |
| **Knowledge** (`knowledge`) | as long as the documents are there | answering questions about your own documents |

Memory and knowledge work with **embeddings**: an embedding model turns a text into a series of
numbers that describes its meaning. Similar texts have similar numbers; this lets grauwolf find
matching passages even when different words are used. The examples use `nomic-embed-text` via
Ollama:

```bash
ollama pull nomic-embed-text
```

## Chatting: `spec chat`

`spec chat` starts a conversation on the console. Every line you type is a run of its own – but all
runs share the same history. This works with any spec, without any changes. A simple agent is
enough for this chapter:

```json
{
  "spec_version": 1,
  "name": "conversation_partner",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You are a helpful conversation partner. Answer briefly and in English, and refer to the conversation so far.",
  "strategy": { "kind": "single_run" }
}
```

The file: [`examples/07-conversation.json`](examples/07-conversation.json).

```bash
grauwolf spec chat docs/user-guide/examples/07-conversation.json
```

Roughly:

```text
> I'm planning a bike tour along the Moselle.
Great idea! How many days have you planned?
> Three. Where should I stay overnight?
For three days on the Moselle, Bernkastel-Kues and Cochem make good stopovers.
> /exit
```

- The prompt `> ` goes to stderr, the answers to stdout. So you can also pipe lines in and process
  only the answers further.
- The conversation ends with `/exit` or Ctrl+D (end of input). Empty lines are skipped.
- If a run fails, the error is written to stderr – the chat continues.
- If the spec has its own type as `input` ([chapter 5](05-structured-data.md)), every line is
  JSON.
- `spec chat` understands most options of `spec run` (e.g. `--events`, `--confirm-tools`), but not
  yet `--stream` and `--otel`.

Unless you specify otherwise, the conversation lives only in memory and is gone after you quit.

## Saving and continuing conversations: `--conversation`

With `--conversation <id>`, grauwolf saves the history after every successful run and loads it again
the next time. This works with `spec chat` and with individual `spec run` calls – even mixed:

```bash
grauwolf spec run docs/user-guide/examples/07-conversation.json --conversation moselle \
  "I'm planning a bike tour along the Moselle, three days."
grauwolf spec run docs/user-guide/examples/07-conversation.json --conversation moselle \
  "How many kilometers per day do you suggest?"
grauwolf spec chat docs/user-guide/examples/07-conversation.json --conversation moselle
```

The second answer knows the first question, roughly: `With three days, 40 to 50 kilometers per day
is comfortably doable.`

The conversation ID may contain only letters, digits, `-` and `_`. The history is saved without the
system prompt; grauwolf takes that fresh from the spec on every run. So you can change the system
prompt without losing old conversations.

You decide where the history is stored:

| Option | Storage location | Default |
|---|---|---|
| `--conversations-dir <dir>` | one JSON file per conversation: `<dir>/<id>.json` | `.grauwolf/conversations` |
| `--conversations-db <url>` | a database, e.g. `sqlite://chats.db` or `postgres://user@host/db` | – |

grauwolf creates an SQLite file itself if needed. A database pays off mainly when several processes
use the same conversations, for example a service ([chapter 9](09-operations.md)).

### Conversations per user: `--user`

With `spec run` you can additionally specify a user. The conversation then belongs to this user:
the same conversation ID is a separate conversation for Anna and for Ben.

```bash
grauwolf spec run docs/user-guide/examples/07-conversation.json \
  --user anna --conversation moselle "What had we discussed?"
```

In the directory this ends up under `<dir>/users/<user-hex>/<id>.json`; the user name is written in
hexadecimal so that any name works (including email addresses). User IDs are 1 to 128 bytes long; the service answers other IDs with `400`, and a users file with such a name does not load.
`--user` needs either `--conversation` or a long-term memory in the spec (next section).
`spec chat` does not know `--user`; there is only the shared conversation.

## Long conversations: the `chat` strategy

A saved conversation grows with every run. At some point it becomes too large for the model – the
answers get slow and expensive, and eventually the history no longer fits into the context window.
The `chat` strategy takes care of this
([`examples/07-chat.json`](examples/07-chat.json)):

```json
{
  "spec_version": 1,
  "name": "chatterbox",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You are a friendly conversation partner. Answer briefly and in English.",
  "strategy": { "kind": "chat", "params": { "max_messages": 20, "keep": 6 } }
}
```

It works like `single_run` (tools included, with `parallel_tools` by default), but checks the length
of the history before every answer. If it has more than `max_messages` messages, the model condenses
all but the last `keep` into a list of facts. The heading of this list is a built-in instruction
to the model: `Facts from the conversation so far:`.

```text
Facts from the conversation so far:
- The user's name is Anna and she lives in Leipzig.
- Anna is planning a bike tour along the Elbe in May.
- Open: Anna is still looking for accommodation in Dresden.
```

The last `keep` messages are kept word for word, and the system prompt is kept anyway. If the
history is compressed again later, the old list is merged into the new one – so there is never more
than one.

| Parameter | Default | Meaning |
|---|---|---|
| `max_messages` | 30 | With more than this many messages (the system prompt counts, the new input does not yet), the history is compressed before the answer |
| `keep` | 10 | This many of the last messages are kept word for word (must be less than `max_messages`) |
| `parallel_tools` | `true` | Several tool calls per answer |

Try it:

```bash
grauwolf spec chat docs/user-guide/examples/07-chat.json --conversation anna
```

Good to know:

- Every compression costs one additional request. The larger the gap between `max_messages` and
  `keep`, the less often it happens.
- Compression is final: a saved conversation then contains the list of facts instead of the old
  messages.
- Facts in the history apply only to this conversation. What the agent should know across
  conversations belongs in long-term memory (next section) – with `remember_compressed`, the lasting
  facts from a compression end up there automatically.

## Long-term memory: `long_term_memory`

A conversation holds the whole history – that gets long and applies only to this conversation.
**Long-term memory**, by contrast, holds individual **facts** ("Anna is a vegetarian."), across all
conversations and restarts. For this, the agent gets two tools:

- `remember` with the argument `fact` stores a fact as a complete sentence,
- `recall` with the argument `query` returns the best-matching remembered facts.

```json
{
  "spec_version": 1,
  "name": "assistant_with_memory",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You are a personal assistant with long-term memory. When the user tells you something lasting about themselves (name, preferences, plans), store it with remember as a complete sentence. Before you answer a question about the user, look it up with recall. Answer briefly.",
  "long_term_memory": {
    "store": "../../../.grauwolf/memory/assistant.json",
    "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
    "top_k": 3
  },
  "strategy": { "kind": "single_run" }
}
```

The file: [`examples/07-memory.json`](examples/07-memory.json). Feel free to state clearly in the
system prompt when the agent should use the tools – this helps small models in particular.

| Field | Meaning | Default |
|---|---|---|
| `store` | JSON file relative to the spec in which the facts are kept (exactly one of `store` and `vector_store`, see below) | – |
| `embedding_model` | embedding model `{ provider, id }` (required); provider `ollama`, `openai`, `gemini` or an OpenAI-compatible one with embeddings, e.g. `mistral` with `mistral-embed` | – |
| `top_k` | this many facts are returned per recall | `3` |
| `remember_tool` | name of the tool for remembering | `remember` |
| `recall_tool` | name of the tool for recalling | `recall` |
| `auto_recall` | automatically add matching facts before every run | `false` |
| `auto_remember` | automatically store new facts after every run | `false` |
| `tools` | offer the two tools to the agent | `true` |

Here `store` points three levels up into the `.grauwolf` folder in the project directory; grauwolf
creates missing directories. Identical facts are stored in the file only once. The file remembers
the embedding model – if you change it later, you need a new file.

```bash
grauwolf spec run docs/user-guide/examples/07-memory.json "My name is Anna and I don't eat meat."
grauwolf spec run docs/user-guide/examples/07-memory.json "What should I cook today?"
```

The second answer is roughly: `How about a vegetable curry, Anna? Completely meat-free.`

**Separated by user:** with `spec run --user <id>`, the agent sees and fills only that user's area
in the same file. Without `--user` (and with `spec chat`), it uses the shared area.

```bash
grauwolf spec run docs/user-guide/examples/07-memory.json --user anna "What don't I eat?"
```

### Remembering and recalling automatically

Small models often forget to call the tools. With `auto_recall` and `auto_remember`, grauwolf does
it itself:

- **`auto_recall`**: before every run, the `top_k` best-matching facts for the input are looked up
  and added as a system message `Known facts from long-term
  memory: …` (built-in instruction). If there are no matches,
  no message is added.
- **`auto_remember`**: after every successful run, grauwolf asks the model in a separate request for
  lasting, new facts from the input and the answer, and stores them. This costs one additional
  request per run.
- **`tools: false`** hides `remember` and `recall`; the memory then works only automatically. This
  prevents duplicate entries in slightly different wording.

```json
"long_term_memory": {
  "store": "../../../.grauwolf/memory/assistant-auto.json",
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "auto_recall": true,
  "auto_remember": true,
  "tools": false
}
```

The complete file: [`examples/07-memory-auto.json`](examples/07-memory-auto.json).

### Keeping facts from long conversations: `remember_compressed`

When the `chat` strategy (or `compress_history` with `facts`) compresses a long conversation, a list
of facts is produced – but it applies only to this conversation. With `"remember_compressed": true`,
grauwolf additionally asks in the same compression request which of these facts apply to the user
**permanently** (name, preferences, plans, relationships – no open questions, no events of the
day), and stores exactly those in long-term memory. This does not cost an additional request
([`examples/07-chat-memory.json`](examples/07-chat-memory.json)):

```json
"long_term_memory": {
  "store": "../../../.grauwolf/memory/companion.json",
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "auto_recall": true,
  "remember_compressed": true,
  "tools": false
},
"strategy": { "kind": "chat", "params": { "max_messages": 20, "keep": 6 } }
```

Together with `auto_recall`, the agent thus knows in a new conversation what it learned in an
earlier one. Facts are stored in the user's area (`--user` or the user in the service), otherwise
in the shared area; `--events` shows this as `memory_remembered`.
Errors while remembering or recalling automatically do not abort the run; with `--events` you see
them as `memory_failed`, and successful steps as `memory_recalled` and `memory_remembered`.

Typical messages from `spec validate`:

```text
…:/long_term_memory/tools: memory without tools needs `auto_recall`, `auto_remember` or `remember_compressed`
…:/long_term_memory/recall_tool: tool name 'recall' is used more than once (the other one is the … at …)
```

## Searching your own documents: `knowledge`

With `knowledge`, your agent answers questions about your own documents. This technique is called
**RAG** (retrieval-augmented generation, i.e. "generating with lookup"): at startup, grauwolf splits
the documents into sections, embeds them and offers the agent a **search tool**. The model decides
itself when to look things up, and gets back the best-matching sections together with their source.

```json
{
  "spec_version": 1,
  "name": "caretaker",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You answer questions about the house rules. Before every answer, look things up with search_house_rules and name the source. If something is not in the documents, say so.",
  "knowledge": {
    "sources": ["07-knowledge"],
    "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
    "tool": "search_house_rules",
    "chunk_size": 500,
    "top_k": 2,
    "index": "../../../.grauwolf/knowledge/house-rules.json"
  },
  "strategy": { "kind": "single_run" }
}
```

The file: [`examples/07-knowledge.json`](examples/07-knowledge.json); the documents are in
[`examples/07-knowledge/`](examples/07-knowledge/).

| Field | Meaning | Default |
|---|---|---|
| `sources` | files or directories relative to the spec; directories are read with all subfolders (at least one of `sources` and `urls`) | – |
| `urls` | web pages that are fetched at every start, see [below](#web-pages-as-sources-urls) | – |
| `min_similarity` | minimum similarity (0 to 1) of a hit; weaker sections are dropped | – |
| `inject` | add the best sections to every question automatically, see [below](#knowledge-as-context-inject) | `false` |
| `tool_enabled` | offer the search tool to the model; with `false` you need `inject: true` | `true` |
| `embedding_model` | embedding model `{ provider, id }` (required); provider `ollama`, `openai`, `gemini` or an OpenAI-compatible one with embeddings, e.g. `mistral` with `mistral-embed` | – |
| `tool` | name of the search tool | `search_knowledge` |
| `top_k` | this many sections are returned per search | `3` |
| `chunk_size` | maximum length of a section in characters | `1000` |
| `index` | file relative to the spec in which sections and embeddings are kept between runs | – |

```bash
grauwolf spec run docs/user-guide/examples/07-knowledge.json "When may I put out bulky waste?"
```

Roughly:

```text
Report bulky waste to the property management and put it out on the street no earlier than the
evening before collection (source: 07-knowledge/house-rules.md).
```

Good to know:

- All files under a source are read as UTF-8 text. So put only text files there (Markdown, plain
  text); a file that is not UTF-8 aborts startup with an error and its path.
- Without `index`, grauwolf splits and embeds **all** documents again on **every start**. With
  `index`, it remembers a checksum per file: on the next start, only new or changed files are
  embedded again, and deleted ones drop out. If you change the embedding model or `chunk_size`,
  everything is embedded again.
- A smaller `chunk_size` gives more precise but shorter matches; with `top_k` you control how much
  text per search goes into the conversation.
- The search tool is a tool like any other: its name must not match any other tool, and you can
  list it in the `tools` list of a subgraph ([chapter 6](06-subgraphs-and-planners.md)).

### Web pages as sources: `urls`

`urls` lists web pages (`https://`; plain `http://` only for `localhost` and `127.0.0.1`) that are
indexed like files. Every start fetches each URL with a **conditional request** (`ETag` and
`Last-Modified` from the last fetch, kept next to the index): a `304 Not Modified` means nothing is
embedded again, and a changed page is split again – with the file `index` the whole page is
embedded again, with the PostgreSQL store only its changed sections. A URL you remove from the spec
drops out of the index. The source name in search results is the URL.

```json
"knowledge": {
  "sources": ["07-knowledge"],
  "urls": ["https://docs.example.com/guide/getting-started.html", "https://docs.example.com/faq.md"],
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "min_similarity": 0.5,
  "inject": true,
  "tool_enabled": false,
  "index": "../../../.grauwolf/knowledge/docs-helper.json"
}
```

The file: [`examples/07-knowledge-urls.json`](examples/07-knowledge-urls.json).

- **Formats:** `text/plain`, `text/markdown`, `text/html` and `application/xhtml+xml` (by
  `Content-Type`, otherwise by the file extension). HTML becomes readable text: scripts, styles,
  navigation, header, footer, sidebars and forms are dropped; headings, paragraphs, list items and
  table cells stay. PDF and other binary types are refused with a message. The charset comes from
  the header (UTF-8 by default, Latin-1 also works).
- **Rules:** at most 5 MiB per page, 30 seconds, no cookies or credentials. Redirects are followed
  only within the same origin; a redirect to another host is an error – put the final URL into
  `urls`.
- **Failures:** if a URL cannot be fetched and the index already holds its content, that content
  is used and a warning goes to stderr. Without indexed content the start fails and names the URL
  and the reason. With the file store, content is only kept between runs when you set `index`.
- Validation: no credentials in the URL, no `#fragment`, no duplicates.

### Knowledge as context: `inject`

With `"inject": true`, grauwolf looks up the question itself: before every request that starts from
a new user input, the best `top_k` sections (honouring `min_similarity`) are added to the
conversation as a system message that names their sources. The model does not have to call a tool,
so this also works with small models that rarely use tools. The event `knowledge_injected`
(`sources`, `count`, no text) reports it, `knowledge_failed` a failed lookup (the run continues).

By default the search tool stays available as well. With `"tool_enabled": false` only the injection
is used; switching both off is a validation error (`knowledge without the tool needs inject: true`).

### Fields of other tools, named differently

If you know the RAG settings of AgentWeaver: `urlSources` is `urls`, `similarityThreshold` is
`min_similarity`, `maxDocuments` is `top_k`, `injectAsContext` is `inject` and `provideAsTool` is
`tool_enabled`.

Typical messages from `spec validate`:

```text
…:/knowledge/sources: knowledge needs at least one source
…:/knowledge/tool: tool name 'recall' is used more than once (the other one is the … at …)
```

## Knowledge and memory in PostgreSQL with pgvector

By default, knowledge lives in memory (with an `index` file as a cache) and long-term memory in a
JSON file – ideal for one container and a few thousand entries. For **large document sets**, for
**several replicas that share one index** and for a **stateless container**, both can live in
PostgreSQL with the [pgvector](https://github.com/pgvector/pgvector) extension instead.

```bash
docker run -d -p 5432:5432 -e POSTGRES_PASSWORD=postgres pgvector/pgvector:pg17
export PGVECTOR_URL=postgres://postgres:postgres@localhost:5432/postgres
```

The URL is **never** part of the spec: the spec names the environment variable (`url_env`), and
the value comes from the environment (a Kubernetes secret, `docker run -e`, …). Errors such as a
missing variable, a refused connection or a missing extension name the variable and never print
the URL or the password. grauwolf creates what it needs on start (`CREATE EXTENSION IF NOT EXISTS
vector`, its tables and HNSW indexes with cosine distance); if the database user may not create
extensions, an administrator runs `CREATE EXTENSION vector` once.

### Knowledge: `knowledge.store`

```json
"knowledge": {
  "sources": ["07-knowledge"],
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "store": { "kind": "postgres", "url_env": "PGVECTOR_URL" }
}
```

The file: [`examples/07-knowledge-postgres.json`](examples/07-knowledge-postgres.json). `kind` is
`file` (the default, with `index`) or `postgres`; `index` only applies to `file` and is an error
together with `postgres`.

- The sections are stored in the table `grauwolf_chunks`, in a collection named
  `<spec name>/<tool name>` – several agents can share one database.
- Indexing is incremental like with `index`: a hash per section; at start only new or changed
  sections are embedded, sections of deleted files are removed. If you change the embedding model
  or its dimension, the collection is rebuilt.
- Several replicas starting at the same time index one after the other (an advisory lock per
  collection), so every section is embedded only once.

### Memory: `long_term_memory.vector_store`

```json
"long_term_memory": {
  "vector_store": { "kind": "postgres", "url_env": "PGVECTOR_URL", "ttl_days": 90, "min_similarity": 0.4 },
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
}
```

The file: [`examples/07-memory-postgres.json`](examples/07-memory-postgres.json). Give **exactly
one** of `store` (JSON file) and `vector_store`.

| Field | Meaning | Default |
|---|---|---|
| `kind` | `postgres` | required |
| `url_env` | name of the environment variable that holds the URL | required |
| `ttl_days` | facts expire this many days after they were stored (at least 1); storing the same fact again renews it | never expire |
| `min_similarity` | recall returns only facts at least this similar to the query (0 to 1, cosine) | none |

Facts are kept per agent (the spec name) and per user exactly like in the file: `--user` and the
service's `user_id` see only their own area, everything else the shared one. Identical facts are
stored once per area. Recall returns at most `top_k` facts that reach `min_similarity`, best first;
expired facts are never returned and are deleted when the same area stores its next fact (there is
no background job). The table `grauwolf_memories` also counts how often a fact was recalled
(`access_count`). `min_similarity` applies to the PostgreSQL store only. Like the file, the
database remembers the embedding model per agent – a different model is refused.

Good to know: pgvector indexes vectors of up to 2000 values; a table has one dimension, so
embedding models with different dimensions need separate databases (or schemas).
Generated crates (`spec generate`) take the same variables; the facade feature `pgvector` is
enabled for them automatically.

## What do I use when?

- **Follow-up questions within a session** → `spec chat`, with `--conversation` to continue later.
- **Conversations that run for a long time** → the `chat` strategy; older parts are kept as facts.
- **The agent should know people over weeks** → `long_term_memory`, with small models using
  `auto_recall`/`auto_remember`, with several users using `--user`.
- **Questions about manuals, rules, notes** → `knowledge` with `index`.
- **Large collections, several replicas, facts that expire** → the PostgreSQL stores above.

Everything can be combined: an agent can chat in a saved conversation, remember facts and look
things up in documents at the same time.

## Key points

- `spec chat <spec>` runs every line as a run in the same conversation; end with `/exit` or EOF.
- `--conversation <id>` saves conversations (default `.grauwolf/conversations`, alternatively
  `--conversations-db`); with `spec run`, `--user` binds them to a user.
- The `chat` strategy condenses long conversations into facts before the answer (`max_messages`,
  `keep`).
- `long_term_memory` gives the agent `remember`/`recall`; `auto_recall`, `auto_remember` and
  `tools: false` make it automatic.
- `knowledge` makes documents and web pages (`urls`) searchable via a search tool or as automatic context (`inject`); `index` saves re-embedding.

Continue with [chapter 8: External tools and other agents](08-external-tools-and-agents.md).
