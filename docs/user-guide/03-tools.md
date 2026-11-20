# Chapter 3: Tools

A language model is good with words but bad at arithmetic, and it sees neither your files nor the
current time. **Tools** close this gap: they are functions the model can call during a run. In this
chapter you write your own tools, move them into their own files and directories, and use the
built-in tools for files and questions to the user.

## What a tool is

A tool has three things the model sees:

- a **name** (snake_case, e.g. `multiply`),
- a **description** from which the model can tell what the tool is good for,
- an **argument schema** (JSON Schema) that says which values the tool expects.

How a tool call works: the model does not answer with text but with a *tool call* ("call `multiply`
with `a = 12.5`, `b = 8`"). grauwolf runs the tool and returns the result to the model, which then
continues – until it gives a text answer. The `single_run` strategy from Chapter 1 does exactly that.

Tool names apply to the whole agent: your own, built-in and external tools must not share a name,
otherwise `spec validate` reports an error.

## Your own tool code

Your own tool goes into the `tools` list. Here is a calculator that can multiply
([`examples/03-calculator.json`](examples/03-calculator.json)):

```json
{
  "spec_version": 1,
  "name": "calculator",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You are a calculation assistant. Use the tool multiply for every multiplication and answer in English.",
  "tools": [
    {
      "name": "multiply",
      "description": "Multiplies two numbers a and b",
      "args": {
        "type": "object",
        "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
        "required": ["a", "b"]
      },
      "code": "let a = args[\"a\"].as_f64().ok_or(\"a is missing\")?;\nlet b = args[\"b\"].as_f64().ok_or(\"b is missing\")?;\nOk((a * b).to_string())"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

All four fields `name`, `description`, `args` and `code` are required.

### What goes into `code`

`code` is the **body** of a Rust function with this signature:

```rust
fn tool(args: serde_json::Value) -> Result<String, String>
```

Written out, the code from the example looks like this:

```rust
let a = args["a"].as_f64().ok_or("a is missing")?;
let b = args["b"].as_f64().ok_or("b is missing")?;
Ok((a * b).to_string())
```

Even without knowing Rust, a few building blocks get you a long way:

- `args["a"]` fetches the argument `a` from the JSON the model sent.
- `.as_f64()` reads it as a number, `.as_str()` as text, `.as_bool()` as a boolean.
- `.ok_or("a is missing")?` stops with this error message if the value is missing or has the wrong
  type.
- `Ok(…)` returns the result as text, `Err(…)` an error. Both end up with the model – it can read
  an error text and, for example, try again with better arguments.
- `format!("{count} words")` builds text from values.

The `Value` type from `serde_json` is already imported.

### The sandbox

Tool code does not run directly on your machine but as a WebAssembly module in a **sandbox** – an
isolated environment. Inside it:

- Only the Rust standard library and `serde_json` are allowed.
- No network, no file system, no environment variables.
- Computing time and memory are limited; every call starts fresh.
- Constructs that could access your machine during compilation or weaken protections are rejected
  by grauwolf up front: `include!`, `include_str!`, `include_bytes!`, `env!`, `option_env!`, `mod`,
  `extern`, `unsafe` and attributes (`#[…]`).

For files there are built-in tools (further below), for network services MCP servers
([Chapter 8](08-external-tools-and-agents.md)).

### Running it

```sh
grauwolf spec run docs/user-guide/examples/03-calculator.json "What is 12.5 times 8?"
```

Roughly:

```text
12.5 times 8 is 100.
```

With `--events` you see the tool call on stderr, roughly like this (shortened):

```text
{"event":"tool_call_starting","call":{"id":"call_1","tool":"multiply","args":{"a":12.5,"b":8}}}
{"event":"tool_call_completed","result":{"id":"call_1","tool":"multiply","content":"100"}}
```

## Building and caching: `spec build`

Before a tool can run, grauwolf compiles the code to WebAssembly with `cargo`. For this you need
the Rust toolchain and the `wasm32-unknown-unknown` target (see
[Chapter 1](01-getting-started.md)); on the very first build, cargo also downloads `serde_json`
from the internet.

`spec run` builds missing tools automatically. The first build of a tool takes a few seconds up to
about a minute. With `spec build` you build in advance, e.g. before you demo an agent:

```sh
grauwolf spec build docs/user-guide/examples/03-calculator.json
```

Output, roughly (one line per tool with name and checksum):

```text
multiply: 3f9c2a…
```

Built tools end up in a **cache**, by default `.grauwolf/tools` in the current directory;
`--cache <dir>` picks another one. A tool is only rebuilt when its name, code or argument schema or
the Rust version changes – otherwise it comes straight from the cache.

`spec validate` does **not** compile the code. Compile errors only show up with `spec build` or
`spec run` – with a JSON pointer and line/column in the tool code, e.g.:

```text
docs/user-guide/examples/03-calculator.json:/tools/0/code:3:5: error: …
```

For services that build third-party code, you can additionally wrap cargo in an operating-system
sandbox: `--build-wrapper` puts a command in front of it (e.g. `bwrap`), given once per part.

## Tool code in its own file: `code_file`

Code as JSON text with `\n` and `\"` is tedious. With `code_file` it lives in a normal file – with
syntax highlighting in your editor ([`examples/03-code-file.json`](examples/03-code-file.json)):

```json
{
  "spec_version": 1,
  "name": "adder",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You are a calculation assistant. Use the tool add for every addition and answer in English.",
  "tools": [
    {
      "name": "add",
      "description": "Adds two numbers a and b",
      "args": {
        "type": "object",
        "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
        "required": ["a", "b"]
      },
      "code_file": "03-tools/add.rs"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

The file [`examples/03-tools/add.rs`](examples/03-tools/add.rs) contains only the body:

```rust
let a = args["a"].as_f64().ok_or("a is missing")?;
let b = args["b"].as_f64().ok_or("b is missing")?;
Ok((a + b).to_string())
```

```sh
grauwolf spec run docs/user-guide/examples/03-code-file.json "What is 19 plus 23?"
```

Roughly:

```text
19 plus 23 is 42.
```

Rules:

- The path is **relative to the spec file**, not to the directory you run the command from.
- A tool has exactly one of `code` and `code_file`; both together report
  `` /tools/0: give exactly one of `code` and `code_file` ``.
- If the file is missing, the message names the pointer and the path, e.g.
  `/tools/0/code_file: …/missing.rs: No such file or directory (os error 2)`.
- Compile errors still point to `/tools/<i>/code`; lines are counted from the start of the file.

## Whole tools in their own files: `tool_files`

If you want to use a tool in several specs, put the **whole definition** into its own JSON file – a
small tool library. The file
[`examples/03-tools/count_words.tool.json`](examples/03-tools/count_words.tool.json) has the same
format as an entry in `tools`:

```json
{
  "name": "count_words",
  "description": "Counts the words of a text",
  "args": {
    "type": "object",
    "properties": { "text": { "type": "string" } },
    "required": ["text"]
  },
  "code_file": "count_words.rs"
}
```

With the code in [`count_words.rs`](examples/03-tools/count_words.rs):

```rust
let text = args["text"].as_str().ok_or("text is missing")?;
let count = text.split_whitespace().count();
Ok(format!("{count} words"))
```

The spec includes it via `tool_files` and can have its own `tools` alongside
([`examples/03-tool-files.json`](examples/03-tool-files.json)):

```json
{
  "spec_version": 1,
  "name": "text_tools",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You help with texts. Use the tools instead of counting or calculating yourself.",
  "tool_files": ["03-tools/count_words.tool.json"],
  "tools": [
    {
      "name": "add",
      "description": "Adds two numbers a and b",
      "args": {
        "type": "object",
        "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
        "required": ["a", "b"]
      },
      "code_file": "03-tools/add.rs"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

```sh
grauwolf spec run docs/user-guide/examples/03-tool-files.json "How many words are in the sentence: The hedgehog sleeps in leaves?"
```

Roughly:

```text
The sentence has 5 words.
```

The paths in `tool_files` are relative to the spec; a `code_file` in a tool file is relative to the
**tool file**. On loading, grauwolf appends the tools to the end of `tools`; after that it is an
ordinary spec. The schema from `spec schema` knows both fields: for a tool, `code` or `code_file`
is enough.

## Loading tools at runtime: `tools_dir`

With `tools_dir` you name a directory from which grauwolf reads all `*.json` files **before every
run** (not in subdirectories). A new tool is then just a new file – without changing the spec and
without a restart ([`examples/03-tools-dir.json`](examples/03-tools-dir.json)):

```json
{
  "spec_version": 1,
  "name": "toolbox",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You help with texts. Use suitable tools if there are any.",
  "tools_dir": "03-tools/folder",
  "strategy": { "kind": "single_run" }
}
```

The directory contains [`reverse.json`](examples/03-tools/folder/reverse.json) in the same format as
for `tool_files` (with `code` or `code_file`):

```json
{
  "name": "reverse",
  "description": "Writes a text backwards",
  "args": {
    "type": "object",
    "properties": { "text": { "type": "string" } },
    "required": ["text"]
  },
  "code": "let text = args[\"text\"].as_str().ok_or(\"text is missing\")?;\nOk(text.chars().rev().collect::<String>())"
}
```

```sh
grauwolf spec run docs/user-guide/examples/03-tools-dir.json "Write 'grauwolf' backwards."
```

Roughly:

```text
Written backwards: flowuarg
```

This is how the directory behaves:

- `spec run` reads it once, `spec chat` before every line, `spec serve` before every request.
- Building works as above with the cache: only new or changed tools cost time.
- Unusable files (not JSON, missing fields, invalid name, compile errors) are skipped and reported
  on stderr with `tool folder: …`; the run continues.
- A tool from the directory must not shadow a tool of the spec or a built-in or external tool – that
  is skipped as well.
- A missing directory counts as empty.
- `spec validate` cannot check the tools in the directory, and a custom graph cannot refer to them
  by name. The `single_run` and `react` strategies offer them; subgraphs with their own tool list
  do not see them.

## Tool packages from a registry: `tool_packages`

Tools can also come ready-built from an OCI registry – the same kind of server that holds container
images (ghcr.io, Harbor, GitLab, a local `registry:2`). A **tool package** is a versioned bundle of
one or more tools: their definitions (name, description, argument schema) and the compiled WASM
modules. You do not need the Rust toolchain for them, and the module you run is exactly the one that
was published ([`examples/03-tool-packages.json`](examples/03-tool-packages.json)):

```json
{
  "spec_version": 1,
  "name": "pack_user",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You calculate. Use the tools of the package for every calculation.",
  "tool_packages": [
    {
      "oci": "ghcr.io/example/grauwolf-tools/math:1.0",
      "tools": ["add", "multiply"],
      "as": { "multiply": "times" }
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

- `oci` is `registry/repository[:tag][@sha256:<digest>]`. The registry host is always written out;
  without tag and digest the tag is `latest`. Prefer a version tag or a digest.
- `tools` selects tools of the package (default: all), `as` renames them – useful when two packages
  or your own tools use the same name. Names must be unique across all tools of the spec.
- The tools run in the same sandbox as your own tools: no files, no network.

Look at a package before you use it, and fetch it in advance:

```sh
grauwolf tools inspect ghcr.io/example/grauwolf-tools/math:1.0
grauwolf tools pull ghcr.io/example/grauwolf-tools/math:1.0
```

`inspect` prints the digest, the tool ABI version, and per tool its description, argument schema and
module hash; `pull` fetches and verifies the package into the cache (`.grauwolf/tools/packages/`).

### The lockfile `grauwolf.lock`

The first time a spec with `tool_packages` is run (`spec run`, `chat`, `serve`, `build`, `generate`)
or locked with `grauwolf tools lock spec.json`, grauwolf resolves every reference to the **digest** of
the package and writes `grauwolf.lock` next to the spec: per reference as written, the manifest
digest and the SHA-256 of every module. Commit it. From then on the reference is fetched by digest,
not by tag, so a moved tag changes nothing; delete the entry (or the file) to update.

A package whose digest is in the cache is never fetched again, and every module is checked against
its hash each time it is loaded. In CI and in image builds use `--locked`: the command then fails
instead of changing the lockfile (a missing or stale entry is the error; for a tree of agents, an entry that no agent of the tree
uses is stale).

```sh
grauwolf tools lock docs/user-guide/examples/03-tool-packages.json
grauwolf spec run spec.json "What is 17 times 3?" --locked
```

Roughly:

```text
docs/user-guide/examples/grauwolf.lock: 1 locked (2 tools)
17 times 3 is 51.
```

### Tool packages from a git repository

A package folder (see [below](#publishing-your-own-package-tools-pack-and-tools-push)) can also be
used straight from a git repository, without a registry
([`examples/03-tool-packages-git.json`](examples/03-tool-packages-git.json)):

```json
"tool_packages": [
  {
    "git": "https://github.com/example/grauwolf-tools.git",
    "path": "text-and-math",
    "tag": "v1.0.0",
    "tools": ["add", "count_words"],
    "as": { "count_words": "word_count" }
  }
]
```

- `git` is the repository URL: `https://`, or `http://` for `localhost` and `127.0.0.1` only. No
  user name or password in the URL; for a private repository set `GIT_TOKEN` (see below).
- `path` is the folder of the package in the repository (relative, no `..`); without it the
  repository root is the package folder.
- Exactly one of `rev` (a commit, full or abbreviated to at least 7 hex digits), `tag` or `branch`.
  A tag or branch is resolved to a commit once and then locked; prefer `rev` or a tag.
- `tools` and `as` work as for `oci`.

grauwolf fetches that commit (shallow when the server allows it), reads `grauwolf-tools.json` and
`src/<tool>.rs` from the folder and **builds the modules itself** with the isolated tool build, so
the Rust toolchain is needed the first time (see [the build options](reference-cli.md)). Prebuilt
`.wasm` files in the folder are ignored.

The lockfile records the commit and the hash of every module under a key that is the entry as
written: `git+<url>[//<path>]@<rev>`, `…@tag:<tag>` or `…@branch:<branch>`, for example
`git+https://github.com/example/grauwolf-tools.git//text-and-math@tag:v1.0.0`. A locked entry is
fetched by its commit, never by tag or branch. The built package is cached per commit and path in
`.grauwolf/tools/packages/`; with the cache and the lockfile a later run needs neither network nor
toolchain. If the cache is gone and another toolchain builds different bytes, the module hash no
longer matches the lock and resolving fails: git packages are reproducible by commit, not
bit for bit. `--locked` behaves as for OCI.

`GIT_TOKEN` (the same variable as for the [git tools](03-tools.md)) is sent as HTTP basic
authentication (user `x-access-token`) only to the host of the URL, never in messages or logs;
without it the access is anonymous. Redirects are not followed.

`tools pull` and `tools inspect` take git packages too. The reference is the lock key, with the
revision always given (`git+<url>[//<path>]@<commit>`, `…@tag:<tag>` or `…@branch:<branch>`);
everything that does not start with `git+` is an OCI reference. The same rules as in a spec apply
(`https`, `http` only for `localhost` and `127.0.0.1`, no credentials in the URL, a relative folder):

```sh
grauwolf tools inspect git+https://github.com/example/grauwolf-tools.git//text-and-math@tag:v1.0.0
grauwolf tools pull    git+https://github.com/example/grauwolf-tools.git//text-and-math@tag:v1.0.0
```

`inspect` fetches the commit but **builds nothing** (no Rust toolchain needed). It prints the
commit, the package name, version and ABI, whether the package is already in the cache (then with
the hashes of its modules), the tools with description and arguments, and the signature:
`signed by <principal> (<key fingerprint>)`, `unsigned` or `signature invalid: …`. The signature is
checked against `git_allowed_signers` of the [tool policy](#trust-the-tool-policy-and-signatures) or
against the file given with `--allowed-signers <file>`; without either, a signed commit is reported
as `signed, not checked`. Like for OCI, `inspect` applies the allowlist (`allowed_git_hosts`) but
shows a missing signature instead of refusing the package.

`pull` does what resolving a spec does for the entry: the policy (host allowlist and, with
`require_signatures`, the SSH signature, before anything is built), fetch, build from the sources
(Rust toolchain, `--build-wrapper` as for `spec build`) and store in the cache. It prints
`<reference> -> commit <40 hex digits>` and the tools. A spec that names this commit (`rev`) then
resolves from the cache without network and toolchain; `tools lock` writes the lock from there.

### Registries, logins and plain http

Public packages are pulled anonymously. For a registry that needs a login set
`GRAUWOLF_REGISTRY_AUTH` to a JSON map from registry to `"user:token"` (or to a bare bearer token):

```sh
export GRAUWOLF_REGISTRY_AUTH='{"ghcr.io": "my-user:ghp_xxxxxxxx"}'
```

If you are already logged in with Docker (`docker login`), nothing else is needed: when a registry
asks for credentials and `GRAUWOLF_REGISTRY_AUTH` has no entry for it, grauwolf reads the Docker
configuration (`$DOCKER_CONFIG/config.json`, else `~/.docker/config.json`). The order is
`GRAUWOLF_REGISTRY_AUTH`, then the Docker configuration, where a credential helper wins over
stored `auths`:

```json
{
  "auths": { "registry.example.com": { "auth": "bXktdXNlcjpteS10b2tlbg==" } },
  "credHelpers": { "ghcr.io": "pass" },
  "credsStore": "desktop"
}
```

- `auths.<registry>.auth` is base64 of `user:password`; `identitytoken` is sent as a bearer token.
- `credHelpers.<registry>` and `credsStore` name a helper: grauwolf runs
  `docker-credential-<name> get` (found in `PATH`) with the registry on stdin, as Docker does. This
  is a program you configured yourself; its output is never printed or logged. A missing helper is
  reported by name.
- Credentials are only looked up when the registry answers with a login challenge, so anonymous
  pulls never start a helper. `docker.io` is understood as `https://index.docker.io/v1/`.

Registries are reached over https. Only `localhost` and `127.0.0.1` registries (with any port)
use plain http, like Docker's insecure-local default – handy for a local `registry:2`.

### Publishing your own package: `tools pack` and `tools push`

A **package folder** has a manifest `grauwolf-tools.json` and one file with the code body per tool
in `src/`, the same format as [`code_file`](#tool-code-in-its-own-file-code_file). The folder
[`examples/03-package`](examples/03-package/grauwolf-tools.json) holds two tools:

```text
03-package/
  grauwolf-tools.json      name, version, and per tool: name, description, args
  src/add.rs               code body of the tool add
  src/count_words.rs       code body of the tool count_words
```

```json
{
  "name": "text-and-math",
  "version": "1.0.0",
  "tools": [
    { "name": "add", "description": "Adds two numbers",
      "args": { "type": "object",
                "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
                "required": ["a", "b"] } }
  ]
}
```

(`format`, `abi` and the module file name `<tool>.wasm` are optional and derived.) `tools pack`
builds every tool in the isolated tool build, so the Rust toolchain is needed here, and writes an
OCI image layout: a directory with `oci-layout`, `index.json` and `blobs/sha256/…`, by default
`<folder>/dist`. The sources are included as the last layer, and the same folder always gives the
same digests. `tools push` uploads the layout (or a package folder, which is packed first):

```sh
grauwolf tools pack docs/user-guide/examples/03-package
grauwolf tools push docs/user-guide/examples/03-package/dist registry.example.com/team/text-and-math:1.0
```

Before packing you can check a folder without building anything: `grauwolf tools validate
docs/user-guide/examples/03-package` reads `grauwolf-tools.json` and the code of every tool in
`src/` with the rules of `tools pack` (including the forbidden constructs of tool code), reports
every problem and exits with 1 if there is one.

#### The lean program `grauwolf-tools`

Everything in `grauwolf tools …` is also available as a program of its own, `grauwolf-tools`,
for tool authors and CI pipelines that only build, check, pack and publish tools: it has no model
providers, sandbox, MCP or HTTP server, so it is about 8 MB and builds in under a minute. The
commands and options are the same without the `tools` word:

```sh
grauwolf-tools validate docs/user-guide/examples/03-package
grauwolf-tools pack docs/user-guide/examples/03-package
grauwolf-tools push docs/user-guide/examples/03-package registry.example.com/team/text-and-math:1.0
```

Install it with `cargo install --git https://github.com/agentsw0rk/grauwolf grauwolf-tools-app`, build
it on a Mac with `scripts/build-macos.sh --tools`, or use the image `ghcr.io/agentsw0rk/grauwolf-tools`
(`docker/tools.Dockerfile`; it contains the Rust toolchain with the WASM target, which building
tools needs). `grauwolf-tools lock <spec>` locks the tool packages and git skills of a spec and its
local sub-agents like `grauwolf tools lock`, but does not check the spec otherwise (providers,
skill directories); use `grauwolf spec validate` for that.

Roughly:

```text
docs/user-guide/examples/03-package/dist: packed sha256:a025e887… (4 blobs)
registry.example.com/team/text-and-math:1.0@sha256:a025e887…
```

The last line is the full reference by digest; put it into a spec to pin the package exactly.
Blobs the registry already has are not uploaded again, a reference without tag is pushed as
`latest`, and a registry that asks for a login is answered with the credentials described above
(with the push permission). Check the result with `grauwolf tools inspect <ref>`.

A package can also describe itself (`description`, `license`, `authors`, `source`) and ask for
stricter sandbox `limits` per tool; `push` adds every pushed version to the **tool catalog** of the
registry, and `grauwolf-tools new` scaffolds a package folder with tests and a CI workflow – all in
[chapter 15](15-the-tool-catalog.md).

### Trust: the tool policy and signatures

A tool package is third-party code. The sandbox limits what it can do, and a **tool policy** limits
where it may come from and who must have vouched for it. Without a policy everything is allowed
and nothing has to be signed. The policy is the table `[tool_packages]` of a `grauwolf.toml` next
to the spec, or of the file named by `GRAUWOLF_TOOL_POLICY` (which wins). Paths in it are relative
to the policy file ([`examples/03-policy/grauwolf.toml`](examples/03-policy/grauwolf.toml); all
settings in the [configuration reference](reference-config.md)):

```toml
[tool_packages]
allowed_registries = ["registry.example.com/team", "ghcr.io/example"]
allowed_git_hosts = ["github.com/example"]
require_signatures = true
oci_keys = ["keys/team.pub"]
git_allowed_signers = "keys/allowed_signers"
```

- `allowed_registries` and `allowed_git_hosts` take hosts or prefixes (`github.com`,
  `github.com/example`; a prefix matches whole path parts). Missing means any, an empty list none.
  A package from another source is refused before any request, **also when it is in the cache or
  the lockfile**, so tightening the policy takes effect at once.
- `require_signatures` demands a valid signature on every package, OCI and git alike.

**Signing OCI packages.** Create a key pair once, sign when you push, and give the consumers the
public key:

```sh
grauwolf tools keygen team                 # writes team.key (keep secret) and team.pub
grauwolf tools push docs/user-guide/examples/03-package registry.example.com/team/text-and-math:1.0 \
  --sign-key team.key
mkdir keys && cp team.pub keys/            # referenced by oci_keys in grauwolf.toml
```

The signature is a cosign-compatible one (ECDSA P-256 over the simple-signing payload of the
package digest, stored in the registry next to the package as `sha256-<digest>.sig`), so
`oci_keys` also accepts public keys made by cosign. Keyless signing is not supported yet. When a
package is pulled with `require_signatures`, grauwolf looks for a signature of one of the
`oci_keys` for exactly this registry, repository and digest; the signatures are stored with the
cache entry, so the check works again offline, a changed key list applies to cached packages
without network, and with `--locked` nothing is fetched to check a package that was cached before.
An unsigned package is refused with a message that names it:

```text
error: `registry.example.com/team/text-and-math@sha256:a025e887…` does not satisfy the tool policy
(`require_signatures`): unsigned
```

`grauwolf tools inspect <ref> [--key team.pub]` shows the status without refusing:
`signed by SHA256:…`, `unsigned` or `signature invalid`. For git packages see
[above](#tool-packages-from-a-git-repository): `tools inspect git+… [--allowed-signers <file>]`.

**Signing git packages.** The commit (`rev`, `branch`) or the tag must carry an SSH signature, as
made by `git commit -S` / `git tag -s` with `gpg.format = ssh`. `git_allowed_signers` is a file in
the format of git's `gpg.ssh.allowedSignersFile`:

```text
alice@example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI… alice
*@example.com namespaces="git" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI… team
```

The principals are matched against the e-mail address of the committer (or tagger), `*` and `?`
work; the only supported option is `namespaces` (must include `git`). GPG and X.509 signatures
are not supported. A `rev` or `branch` entry needs a signed commit; a `tag` entry a signed
**annotated** tag or else a signed commit (a lightweight tag on an unsigned commit is refused). The check
happens **before anything is built**. The raw commit and tag objects are stored with the cache
entry, so a cached package is checked again without network. If they do not satisfy a changed policy
(or were cached before the policy existed), grauwolf fetches them again, which `--locked` forbids
(the message says so; resolve once without it). A git entry under `require_signatures` needs
`git_allowed_signers`, otherwise it is refused with that hint.

### Generated crates and images

`spec generate` fetches the packages and embeds their modules like local tools; the generated crate
and the image build (`--dockerfile`) need no registry and no network. Use `--locked` when you
generate in CI.

### What is checked when

- `spec validate` checks the reference syntax and the names it knows without network: the tools you
  list in `tools`, after renaming, against each other and against your other tools. It never fetches
  anything. Name the tools you want in `tools` if a subgraph's tool list refers to them.
- When the package is resolved: digests of manifest and modules, that it is a grauwolf tool package,
  the tool ABI version (only `1` exists), that the tools you select exist and that no name clashes.
- With a [tool policy](#trust-the-tool-policy-and-signatures): the allowed registries and git hosts
  first (before the cache, the lockfile or the network are looked at), then, with
  `require_signatures`, the signature.

## Skills: instructions the agent loads on demand: `skills`

A *skill* is a folder with instructions: a `SKILL.md` and, if you like, files next to it (references,
templates, examples). Long instruction sets do not belong in the system prompt, where they would fill
the context of every request. With skills the agent sees only a list of names and descriptions and
loads the instructions of a skill when a task needs it.

```
skills/
  haiku-writer/
    SKILL.md
    references/signature.md
```

```markdown
---
name: haiku-writer
description: Writes a haiku and signs it. Use it when someone asks for a poem or a haiku.
---
Answer with exactly one haiku in the pattern 5-7-5 syllables, one line each.
After the poem add a last line with the signature from `references/signature.md`.
```

The front matter needs `name` (equal to the name of the folder) and `description`; the rest of the
file is the instructions. The spec lists the *sources* of skills:

```json
{
  "system_prompt": "You are a helpful assistant. Check your skills before you answer.",
  "skills": [{ "dir": "03-skills" }]
}
```

(`examples/03-skills.json`, with the skill folder `examples/03-skills/`.) Every sub-directory of
`dir` that contains a `SKILL.md` is a skill; `"names": ["haiku-writer"]` takes only some of them.
`dir` is relative to the spec file.

What the agent gets, as soon as `skills` is set:

- A block at the end of its system prompt: one line `- <name>: <description>` per skill and the hint
  to call `load_skill`.
- The tool `load_skill { name }`: returns the instructions without the front matter and the list of
  the other files of the skill. An unknown name gives an error that lists the known skills.
- The tool `read_skill_file { name, path }`: returns a text file of the skill (at most 1 MiB; the
  path stays inside the skill folder; binary files are refused with a message).
- The event `skill_loaded { name }` after every successful `load_skill` (in `--events`, the JSONL
  log and the OpenTelemetry spans as a span event).

The two tools only read. Scripts in a skill folder are never run by them. Their names are reserved
(a tool of yours called `load_skill` is a validation error), and the agent may have file tools
besides skills.

### Skills from a git repository

```json
"skills": [
  { "dir": "03-skills" },
  { "git": "https://github.com/example/grauwolf-skills.git", "path": "writing",
    "tag": "v1.0.0", "names": ["pirate-translator"] }
]
```

(`examples/03-skills-git.json`.) A git source works like a git
[tool package](#tool-packages-from-a-git-repository): `https://` (`http://` only for `localhost`),
no user info in the URL (private repositories use `GIT_TOKEN`), `path` is the folder with the skill
folders (default: the repository root), and exactly one of `rev`, `tag` and `branch`. The commit is
recorded in the `skills` section of `grauwolf.lock` next to the spec, together with a hash of the
files; `grauwolf tools lock <spec>` writes it (the same command also locks `tool_packages`), and
`spec run`, `spec serve` and the others resolve a missing entry when they start. With `--locked` the
lockfile is never changed and a locked source is read from the cache
(`.grauwolf/tools/packages/skills-<commit>…`) without network access; the files in the cache are
verified on every use. The [tool policy](#trust-the-tool-policy-and-signatures) applies too:
`allowed_git_hosts`, and with `require_signatures` a commit or tag signed by one of the
`git_allowed_signers`.

### What is checked when

- `spec validate`: the form of every source (exactly one of `dir` and `git`, a usable URL or
  directory, valid names) and, for local directories, the folders themselves: a `SKILL.md` with
  `name` and `description`, the front matter `name` equal to the folder name, every file at most
  1 MiB and no symbolic link that leaves its skill folder. Skill names must be unique across all
  sources. Git sources are checked when they are fetched.
- A run, `spec serve` and `spec build` do the same checks when they start; a broken skill stops them
  before the first request, with the file name.

### Generated crates and images

Skills are not compiled into the binary. `spec generate --dockerfile` copies local skill directories
to `app/` (next to knowledge sources, same relative path), so the image contains them read-only. Git
sources are fetched at generate time (use `--locked` in CI) and copied to `app/skills-git/<n>/`; the
spec inside the crate names these copies, so the image build and the running service need no git and
no network. The same holds for the skills of sub-agents (the whole [agent tree](08-external-tools-and-agents.md#agents-as-tools-agent_tools) is copied). Without `--dockerfile`, a generated crate
reads local skill directories next to its spec like the interpreter does, and git sources are
refused.

## Built-in tools: `builtin_tools`

Some tools come ready-made with grauwolf. They do not run in the sandbox but with the permissions
of the grauwolf process – that is why you select each one individually.

| Tool | Arguments | Effect |
|---|---|---|
| `read_file` | `path` | read a file |
| `list_directory` | `path` | list the contents of a directory (directories end with `/`) |
| `write_file` | `path`, `content` | write a file, create missing directories |
| `edit_file` | `path`, `old`, `new` | replace `old` with `new`; `old` must occur exactly once |
| `search_files` | `pattern`, optional `path`, `max_results` | search text files with a regular expression |
| `say_to_user` | `message` | tell the user something |
| `ask_user` | `question` | ask the user something and return the answer |

The first five are **file tools** and need `root`: the directory (relative to the spec) in which
they may work. Paths are relative to this directory; absolute paths, `..` and symbolic links
(symlinks) pointing outside are rejected. `search_files` returns matches as `path:line: text`, by
default at most 50 (`max_results` up to 200), and skips hidden entries, symlinks, binary files and
files over 1 MiB.

`say_to_user` and `ask_user` are **user tools**: in the CLI they write to stderr and read the
answer as a line from stdin.

An agent that reads notes and asks back when something is unclear
([`examples/03-files.json`](examples/03-files.json)):

```json
{
  "spec_version": 1,
  "name": "notes_helper",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You help with the notes in the workspace folder. Look things up before you answer, and ask back if something is unclear. Answer in English.",
  "builtin_tools": {
    "tools": ["list_directory", "read_file", "search_files", "ask_user"],
    "root": "03-tools/workspace"
  },
  "strategy": { "kind": "single_run" }
}
```

```sh
grauwolf spec run docs/user-guide/examples/03-files.json "When is my dentist appointment?"
```

Roughly:

```text
Your dentist appointment is on Tuesday at 9 am.
```

The spec deliberately gives the agent only reading file tools. You should select writing tools
(`write_file`, `edit_file`) only if the agent really is supposed to write – and preferably together
with a confirmation prompt (next section).

`spec validate` checks the selection, roughly like this:

```text
bt.json:/builtin_tools/tools/1: unknown tool 'shell'
bt.json:/builtin_tools/root: built-in file and key-value tools need `root` (directory relative to the spec)
```

## Thinking, web requests, notes and tasks

More built-in tools help agents that work through several steps. They are listed in
`builtin_tools.tools` like the others ([`examples/03-fetch-notes.json`](examples/03-fetch-notes.json)).

| Tool | Arguments | Effect |
|---|---|---|
| `think` | `thought` | gives the model room to reason between tool calls; answers `ok`, changes nothing |
| `fetch` | `url`, optional `method`, `headers`, `body` | HTTP request; needs `builtin_tools.fetch` |
| `store_value` | `key`, `value` | store a text value in the key-value store (overwrites) |
| `retrieve_value` | `key` | read a value (`Key not found: <key>` if there is none) |
| `list_keys` | – | list all keys (`No keys stored` if empty) |
| `add_task` | `title` | add a task to the task list (`Added task #1: <title>`) |
| `complete_task` | `id` | mark a task as done |
| `remove_task` | `id` | remove a task |
| `list_tasks` | – | list the tasks as `#1 [x] <title>` (`No tasks` if empty) |

**`fetch`** is safe by default because the spec has to name the hosts:

```json
"fetch": { "allowed_hosts": ["example.com", "*.wikipedia.org"], "allow_http": false }
```

- A host is allowed if it is listed exactly or matches `*.domain` (all subdomains, not the domain
  itself). Everything else is refused with a message that names the host.
- Only `https://` works, unless `allow_http` is `true`.
- Redirects are followed (at most five), but every target is checked against the list again.
- The host name is resolved first and the connection goes to the checked address. If it resolves
  to a private, loopback or link-local address (for example `127.0.0.1`, `10.x.x.x`,
  `169.254.169.254`), the request is refused – unless that very host is listed exactly (`"localhost"`,
  `"10.0.0.5"`). This keeps an agent from reaching into your cluster.
- The request times out after 30 seconds. The answer is the status line (`HTTP 200 OK`), the headers
  `content-type` and `content-length`, and the body as text, cut off after 1 MiB. HTML is reduced
  to its text (scripts, styles and tags removed).
- `spec validate` reports a missing section (`MissingBuiltinSection`), an empty list
  (`EmptyAllowedHosts`) and entries that are not hosts (`InvalidAllowedHost`).

### `fetch` with SPIFFE: `fetch.auth`

Inside a cluster that runs SPIRE, internal services can check *which agent* calls them instead of
a shared token. Give such a host an identity (ADR-0110):

```json
"fetch": {
  "allowed_hosts": ["reviewer.agents.svc", "*.tools.svc", "example.com"],
  "auth": {
    "reviewer.agents.svc": { "spiffe": { "id": "spiffe://agentswork.de/ns/agents/sa/reviewer" } },
    "*.tools.svc": { "spiffe": { "mode": "jwt", "audience": "tools" } }
  }
}
```

- Every key of `auth` must be an entry of `allowed_hosts` (written the same way, patterns such as
  `*.tools.svc` included); the value has the form of [`auth.spiffe`](reference-spec.md#authspiffe-in-agent_toolsid-and-mcp_serversid)
  (`mtls` by default or `jwt` with `audience`; exactly one of `id`, `ids`, `trust_domain` for `mtls`).
- A request to such a host presents the agent's X.509-SVID and accepts the server only if its SPIFFE
  ID fits (`mtls`), or sends a fresh JWT-SVID as `Authorization: Bearer` (`jwt`). Other hosts, such as
  `example.com` above, are fetched without any identity, and so is the next hop of a redirect that
  leaves an identity host.
- **Identity hosts are only called over `https://`.** A URL with `http://` is refused
  (`'<host>' needs a workload identity (`auth`), which only works over https://`), even with
  `allow_http: true`: there is no silent fall back to plain HTTP. (`allow_http` is global, so this
  is checked per request and not by `spec validate`.)
- A refused handshake says why, as for MCP servers and A2A: `TLS handshake with <host> failed: the
  server is <id>, expected <id>; the request was not sent`.
- The model never sees a certificate or a token: the host process makes the request. Without
  `SPIFFE_ENDPOINT_SOCKET` the agent does not start; the error names
  `builtin_tools.fetch.auth.<host>`.

**Key-value store:** one JSON file `kv.json` in `root` (so `root` is required). With a user
(`--user`, or the users of the service, as for the memory in [Chapter 7](07-conversations-and-knowledge.md))
each user gets their own file `users/<hex>/kv.json`. Keys have at most 256 bytes, values 64 KiB;
writes are atomic. It stores exact values under exact keys and is not the semantic long-term
memory (`remember`/`recall`). The file is visible to the file tools below the same `root`.

**Task list:** the list belongs to the run and lives in the agent storage, so it is part of the
checkpoints and is still there after a `--resume`. IDs are whole numbers that are never reused.

## Jira, Confluence and git

Three more groups of built-in tools work with systems outside the machine. Each group needs its own
section in `builtin_tools`; listing a tool without the section of its group is a spec error
(`MissingBuiltinSection`). As with the other built-in tools, only the tools you list are offered.

| Group | Tools | Section |
|---|---|---|
| Jira | `jira_search_issues { jql, max_results }`, `jira_get_issue { issue_key }`, `jira_create_issue { project_key, issue_type, summary, description }`, `jira_add_comment { issue_key, comment }`, `jira_transition_issue { issue_key, status_name }` | `"jira": { "base_url": "https://<site>.atlassian.net" }` |
| Confluence | `confluence_search_pages { cql, limit }`, `confluence_get_page { page_id }`, `confluence_create_page { space_key, title, body, parent_id }`, `confluence_update_page { page_id, title, body }` | `"confluence": { "base_url": "https://<site>.atlassian.net" }` |
| Git | `git_checkout { ref }`, `git_list_branches`, `git_create_branch { name }`, `git_commit { message }`, `git_push` | `"git": { "repo_url": "https://…", "directory": "repo", "push_policy": "new_branches", "use_token": true }` |

**Jira and Confluence (Atlassian Cloud).** `base_url` must be `https://`; only scheme and host count,
so a pasted `…/wiki` address works. The credentials come from the environment, read before the run
starts: `ATLASSIAN_EMAIL` (the account's address) and `ATLASSIAN_API_TOKEN` (an API token). Every
result is `HTTP <status>` and a line break followed by the body of the answer – also for errors, so
the model sees what went wrong. Issue keys (`ABC-123`) and page IDs (digits) are checked before the
request. Jira texts go out as Atlassian Document Format: one paragraph per line. Jira Server and
Data Center are not covered.

**Git.** One fixed repository per spec: `repo_url` (`https://`, without user name and password),
`directory` (below `root`, or the spec's directory without `root`; default `repo`). The tools are part
of grauwolf (libgit2 with an own HTTP transport); no `git` program is needed, not even in the container image. `git_checkout` clones on the
first call and fetches afterwards, then switches to a branch (fast-forwarded to the remote), a tag or
a commit hash (detached). It refuses to run with uncommitted changes, when leaving a branch with
commits that are not pushed, and when a branch has diverged from the remote – the refusals start with
`Refused:` and tell the model what to do. `git_create_branch` makes a new branch at the current
commit; `git_commit` stages everything and commits on the current branch; `git_push` pushes the
current branch to the same name, never with force.

- **Read-only repository:** list only `git_checkout` and `git_list_branches`.
- **`push_policy`:** `new_branches` (default) lets `git_push` push only branches that the agent
  created with `git_create_branch` in this process; `any` allows every branch.
- **`use_token`:** with `true` the access token from `GIT_TOKEN` is used (read before the run). It
  is sent in memory as the `Authorization` header (user `x-access-token`), only for network
  commands and only to the scheme and host of `repo_url`, and never appears in a URL, a result or
  a log. Commits use `GIT_AUTHOR_NAME` and `GIT_AUTHOR_EMAIL` (default
  `grauwolf agent`, `agent@grauwolf.local`).
- **Guard:** before it fetches, lists or pushes, grauwolf checks that `.git/config` still names exactly
  `repo_url` as `origin`, without `pushurl` and without `url.*.insteadOf`. This way a file tool that
  writes into the same directory cannot redirect the token to another server; the call is refused.
- **Redirects are refused:** the tools follow no HTTP redirect (not even to the same host); a
  server that answers with one – a moved repository, for example – gives
  `Error: git <command> failed: the server answered with a redirect (HTTP 301); redirects are not
  followed`. Use the final URL as `repo_url`. Only servers that speak the smart HTTP protocol work.
- Calls on one directory run one after another, network calls time out after 120 seconds, and
  failures come back to the model as text (`Error: git <command> failed: …`).

### git with SPIFFE: `git.auth`

If your git server sits behind a proxy that checks the SPIFFE ID of the caller and maps it to a git
user, the git tools can use the agent's identity instead of `GIT_TOKEN` (ADR-0110):

```json
"git": {
  "repo_url": "https://git.tools.svc:8443/team/service.git",
  "auth": { "git.tools.svc": { "spiffe": { "id": "spiffe://agentswork.de/ns/git/sa/git-proxy" } } }
}
```

- The key must be the host of `repo_url` (or a pattern such as `*.tools.svc` that matches it;
  `AuthHostNotRepository` otherwise); the value is an [`auth.spiffe`](reference-spec.md#authspiffe-in-agent_toolsid-and-mcp_serversid) block.
- Clone, fetch and push present the X.509-SVID (`mtls`, the server's ID is checked) or send a fresh
  JWT-SVID as `Authorization: Bearer` (`jwt`). The server answers `403` for an identity it does not
  map and for a read-only identity that pushes (`access denied (HTTP 403)`).
- `auth` and `use_token` exclude each other (`GitTokenWithAuth`): the identity replaces `GIT_TOKEN`.
- Only the tools' own repository is covered. The git sources of tool packages and skills are
  resolved at build time, usually outside the cluster, and do not use `auth` yet.
- To try it without your own proxy, the test git server in
  [`examples/test-git-server/`](../../examples/test-git-server/README.md) serves a repository over
  mTLS with a mapping file from SPIFFE IDs to users (it is for tests only).

An agent that works on a ticket ([`examples/03-jira-git.json`](examples/03-jira-git.json)):

```json
{
  "spec_version": 1,
  "name": "ticket_worker",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You work on Jira tickets. Read the ticket, check out the repository, make the change on a new branch, commit and push it, then comment on the ticket with the branch name. Answer in English.",
  "builtin_tools": {
    "tools": [
      "jira_get_issue", "jira_add_comment",
      "git_checkout", "git_list_branches", "git_create_branch", "git_commit", "git_push",
      "read_file", "write_file"
    ],
    "root": "work",
    "jira": { "base_url": "https://example.atlassian.net" },
    "git": {
      "repo_url": "https://github.com/example/service.git",
      "directory": "service",
      "push_policy": "new_branches",
      "use_token": true
    }
  },
  "strategy": { "kind": "react" }
}
```

```sh
export ATLASSIAN_EMAIL=me@example.com ATLASSIAN_API_TOKEN=… GIT_TOKEN=…
grauwolf spec run docs/user-guide/examples/03-jira-git.json "Work on ticket ABC-42"
```

Without a variable the run does not start:

```text
03-jira-git.json: the Jira tools need Atlassian Cloud credentials: ATLASSIAN_EMAIL is not set (ATLASSIAN_EMAIL is the account's e-mail address, ATLASSIAN_API_TOKEN an API token)
```

These tools change third-party systems. Give the agent only the tools it needs, prefer
`push_policy: new_branches`, and consider `--confirm-tools` (next section). Services made from such a
spec run in the same small image as every other service.

## Running commands

The built-in tool `run_command` lets the agent start programs, for example `git`, `ls` or `python3`.
It is the only built-in tool with process-level side effects, so it is off unless the spec says so
explicitly: list `run_command`, set a `root`, and add a `command` section
([`examples/03-command.json`](examples/03-command.json)):

```json
{
  "spec_version": 1,
  "name": "file_counter",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "You answer questions about the files in the workspace folder. Use run_command with the programs ls and wc; do not guess. Answer in English.",
  "builtin_tools": {
    "tools": ["run_command"],
    "root": "03-tools/workspace",
    "command": {
      "allowed": ["ls", "wc"],
      "timeout_secs": 20,
      "max_output_bytes": 16384,
      "env": ["PATH", "LANG"]
    }
  },
  "strategy": { "kind": "single_run" }
}
```

**Allowlist mode (the default).** The model calls `run_command { program, args?, cwd? }`, for
example `{ "program": "wc", "args": ["-l", "README.md"] }`.

- Only programs from `allowed` start: a bare name (looked up in the `PATH` of the cleared
  environment, see below) or an absolute path that is listed exactly. Anything else is refused with
  a message that lists the allowed programs.
- The program starts **directly**, without a shell: `;`, `|`, `*` and `$HOME` in an argument stay
  literal text.
- `cwd` is relative to `root` and must stay inside it (no absolute path, no `..`, no symlink that
  leads out) – the same rules as for the file tools. The default is `root` itself.
- The environment is **cleared**: the program sees only the variables named in `env` (default
  `["PATH"]`), with their values from the process. API keys and tokens stay invisible unless you
  list them. List `PATH` if bare program names should be found.
- Standard input is closed; stdout and stderr are captured, each cut off after `max_output_bytes`
  (default 65536, at most 10 MiB) with a note.
- After `timeout_secs` (default 60, at most 3600) the whole process group is killed, including
  everything the program started.
- The result starts with `exit code: N`, `terminated by signal N` or `timed out after N s and was
  killed`, followed by the `stdout:` and `stderr:` sections.
- No approval is needed: the list is the approval.

`spec validate` checks the section: it is required for the tool (`MissingBuiltinSection`), `root`
too (`MissingBuiltinRoot`), `allowed` must not be empty (`EmptyAllowedCommands`), a program is a bare
name or an absolute path (`InvalidAllowedProgram`), `*` only exists in shell mode
(`WildcardWithoutShell`), and the limits must be in range (`CommandLimitOutOfRange`).

**Shell mode (only explicitly, and risky).** If the model needs pipes, globbing or variables:

```json
"command": { "allowed": ["*"], "shell": true, "i_understand_shell": true }
```

The tool then takes `{ command }` and runs it with `sh -c` in `root`, with the same cleared
environment, timeout and output limit. It is a shell with the permissions of the grauwolf process
and no OS sandbox: the model can delete files below your user's reach, read what the environment of
`PATH` programs can read and reach every service your machine can reach. That is why three
deliberate settings are required (`shell`, `allowed: ["*"]`, `i_understand_shell`; any other
combination is a validation error) and every `spec validate` and every command that loads the spec
prints a warning.

- **Approval per call.** In shell mode every call needs an approval (default
  `"approve_each_call": true`). `spec run` and `spec chat` ask on the console before each call
  (`The agent wants to run the shell command … Run? [y/N]`); `spec serve` asks through `/approvals`
  as described for [`--confirm-tools`](#asking-before-tools-run---confirm-tools); a run without
  anyone who can approve refuses the call and tells the model so. Unattended services may switch it
  off with `"approve_each_call": false`.
- **Generated crates** gate the shell like the interpreter (ADR-0119): the call is refused without an
  approver in `RunOptions`, and a generated service asks through `/approvals` on `/run/stream`.
- **Containers.** If you run an agent with this tool as a service, harden the container: a non-root
  user, a read-only root file system with a writable volume only at the root, no service account
  token (`automountServiceAccountToken: false`), no privilege escalation, dropped capabilities, and
  a network policy that allows only the model endpoint and the hosts the agent needs. AgentWeaver
  sets these automatically for every harness whose spec uses `run_command`.
- Prefer the allowlist. Allowing `python3` or `sh` in the list is the same as shell mode.

**The agent's identity for a program: `workload_api`.** A program that logs in with SPIFFE itself
(a CLI for an internal service, say) needs the Workload API socket, but the cleared environment
hides it. List the programs and set (ADR-0110):

```json
"command": { "allowed": ["internal-cli"], "workload_api": true }
```

`SPIFFE_ENDPOINT_SOCKET` (and `SPIFFE_FETCH_TIMEOUT`, if set) is then added to the environment of
the allowlisted programs. They act **as the agent**: SPIRE does not tell the processes of a pod
apart. Without the socket at run time the agent does not start
(`builtin_tools.command.workload_api: … SPIFFE_ENDPOINT_SOCKET is not set`). In shell mode the
combination is a validation error (`WorkloadApiWithShell`): a shell that the model controls must not
act with the agent's identity towards internal services.

The tool needs a Unix system (process groups, `sh`); elsewhere the run does not start. It belongs to
the facade feature `command` (on by default); a build without it refuses a spec with `run_command`
before the run.

## Asking before tools run: `--confirm-tools`

With `--confirm-tools`, grauwolf asks on the console before every call of the named tools
(comma-separated):

```sh
grauwolf spec run docs/user-guide/examples/03-files.json "What is in notes.md?" --confirm-tools read_file
```

Roughly:

```text
Run tool read_file with {"path":"notes.md"}? [y/N] y
Your notes contain a shopping list (flour, eggs, milk), a dentist appointment on Tuesday at 9 am and an idea for a recipe agent.
```

`y` or `yes` allows the call (`j` and `ja` work as well). Anything else declines it; the model
then receives `The user refused the call of read_file.` as the result and can react to it. The option works on
every tool, including your own. How approvals work when running as a service is shown in
[Chapter 9](09-operations.md).

## Self-made tools

An agent can also write new tools for itself during a run: with `self_tools` it gets a
`create_tool` tool, grauwolf builds the code into the sandbox, and by default a human confirms
every new tool. How to do this safely and under control is shown in
[Chapter 10](10-self-made-tools.md).

## Key points

- A tool has `name`, `description`, `args` (JSON Schema) and code; the model decides when it is called.
- The code is the body of `fn tool(args: serde_json::Value) -> Result<String, String>` – only std and `serde_json`, in a sandbox without network and files.
- `spec run` builds tools automatically, `spec build` in advance; the cache lives in `.grauwolf/tools`. `spec validate` does not compile.
- `code_file` moves the code out, `tool_files` whole tool definitions; paths are relative to the file that names them.
- `tools_dir` loads tools from a directory before every run; unusable files are skipped and reported.
- `tool_packages` imports ready-built tools from an OCI registry (`oci`, `tools`, `as`); `grauwolf.lock` pins the digests, `--locked` forbids changing it, `GRAUWOLF_REGISTRY_AUTH` or the Docker configuration hold credentials; `tools pack` and `tools push` publish your own package folder. A tool policy (`grauwolf.toml`) can restrict registries and git hosts and require signatures (`tools keygen`, `tools push --sign-key`, signed git commits and tags).
- `skills` lists skill folders (`dir`, or `git` with `rev`/`tag`/`branch`); the agent gets a skill list in its system prompt and the read-only tools `load_skill` and `read_skill_file`; git sources are locked in `grauwolf.lock` (`tools lock`), generated images contain copies in `app/`.
- `builtin_tools` provides file tools (only under `root`) and the user tools `say_to_user`/`ask_user`.
- `think`, `fetch` (hosts from `builtin_tools.fetch`, HTTPS only, no private addresses unless listed), `store_value`/`retrieve_value`/`list_keys` (`kv.json` under `root`) and the todo tools (list per run, part of checkpoints) round off the built-in tools.
- Jira, Confluence and git tools need their section in `builtin_tools` and credentials from the environment (`ATLASSIAN_EMAIL`, `ATLASSIAN_API_TOKEN`, `GIT_TOKEN`); a missing one stops the run before it starts.
- `--confirm-tools a,b` asks before these tools run; the model receives a refusal as the result.

Continue with [Chapter 4: Flows](04-flows.md).
