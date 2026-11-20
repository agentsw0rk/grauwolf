# ADR-0103: Skills – instruction folders an agent loads on demand

- **Status:** Accepted (ADR-0092, F14.16; User, 2026-11-12: skills from local folders **and** from git repositories, copied into the image instead of embedded in the binary)
- **Date:** 2026-11-10

## Context
AgentWeaver attaches *skills* to agents: folders with a `SKILL.md` (front matter `name`,
`description`, then instructions) and optional files (references, templates, scripts), taken from a
git registry at a pinned commit and checked by a content hash. The agent sees the list of skills
and loads a skill's instructions only when it needs them (progressive disclosure). AgentWeaver
exports them to `skills/<agent>/<skill>/` and forbids combining skills with a file-system tool on
the same agent.

## Decision
- **Spec section `skills`**: a list of sources, `{ "dir": "skills", "names": ["…"]? }` (directory
  relative to the spec) or `{ "git": "https://…", "path"?, "rev" | "tag" | "branch", "names"? }`
  (a git source like those of tool packages, ADR-0107). A directory (or the `path` in the
  repository) holds one sub-directory per skill; without `names`, every sub-directory with a
  `SKILL.md`. Validation: every skill has a `SKILL.md` with `name` (equal to the folder name) and
  `description`, names unique across all sources, files ≤ 1 MiB each, no symlink leaving the skill
  folder.
- **System prompt:** the agent's system prompt is extended by a short list
  `- <name>: <description>` and the hint to call `load_skill`.
- **Tools** (built-in, read-only, under the skills directory):
  - `load_skill { name }` → the instructions of `SKILL.md` (without front matter) and the list of
    the skill's files;
  - `read_skill_file { name, path }` → a file of the skill (text, ≤ 1 MiB; paths cannot leave the
    skill folder).
- **Scripts** in skills are never run by these tools; running them needs `run_command` (ADR-0099)
  with the skill folder as an allowed root, decided per spec.
- **Events:** `skill_loaded { name }`.
- The AgentWeaver export copies the skill files from the graph (`files`, `commit`, `contentHash`
  checked there) into `skills/<agent>/<skill>/` next to the spec; the image contains them read-only.
  In grauwolf, generated crates and images get **copies** in `app/` (not embedded in the binary).
  Because the skill tools are separate and read-only, the combination with file tools is allowed in
  grauwolf.

## Consequences
- Agents can carry larger instruction sets without filling the context from the start.
- Skills are plain files; updating a skill means a new export/image (pinned commit, as in
  AgentWeaver).

## Implementation notes (2026-11-12)

### Where things live
- **Spec:** `SkillSourceSpec` and `GitSource::from_skill_source` in `grauwolf-spec-model` (same
  validation as git tool packages: https, http only for localhost, no credentials, exactly one of
  rev/tag/branch, relative path); `validate/skills.rs` in `grauwolf-spec` (`SkillSource`,
  `InvalidGitSource` with the field, `InvalidId` for names). The tool names `load_skill` and
  `read_skill_file` (`BUILTIN_SKILL_TOOLS`) are part of `external_tools()` while `skills` is set,
  so the existing namespace checks reserve them and the interpreter and the generator require them.
- **Content checks:** `SkillLibrary::load` in `grauwolf-builtin-tools` (no IO in the spec crate):
  front matter, `name` = folder name, unique names across all sources, 1 MiB per file, symlinks
  that leave a skill folder are refused. Local directories are checked by `spec validate` and at
  start-up (a broken skill stops a run before the first request); git sources when resolved.
- **Tools:** `load_skill { name }` (instructions without front matter, then the list of the other
  files), `read_skill_file { name, path }` (text only; the path is checked again at call time, so a
  file that changed after loading cannot escape; binary and large files give a clear text; an unknown
  skill lists the known names). Read-only, scripts are never run.

### System prompt block
The tools carry it: `DynTool::prompt_addition()` (new, default `None`) of `load_skill` returns
`- <name>: <description>` per skill plus the hint; `ToolRegistry::prompt_addition()` collects the
texts and the interpreter (at load time) and the generated `run_observed` (at run time, after
`tools(external)`) append them to the first system message with `Prompt::extend_system`. So both
paths build the same prompt without the spec crate knowing the descriptions (they live in files).

### Event
`skill_loaded { name }` is an `AgentEvent`; **tool execution emits it** (`tool_execution.rs`) after
a successful call of the tool named `grauwolf_tools::LOAD_SKILL_TOOL`, right after
`tool_call_completed`. Tools cannot emit events themselves, and doing it where results are seen
keeps interpreter and generated code identical without a new port. JSONL logs and `--events` pick it
up automatically; the OpenTelemetry feature records a span event `skill_loaded`
(`grauwolf.skill.name`). Failed loads (unknown skill) report no `skill_loaded`.

### Git sources, lockfile, cache, policy
- `grauwolf.lock` gets a `skills` section (key `git+<url>[//<path>]@<rev | tag:… | branch:…>` like
  git tool packages → `{ commit, tree }`, `tree` is the hash of the file list of the cache entry);
  the section is absent in lockfiles without git skills, so existing lockfiles stay byte-identical.
  `PackageResolver::resolve` (tool packages) and `resolve_skills` each rewrite only their own
  sections. One lockfile, one git client (`grauwolf-git`), one resolver; no second implementation.
- **Cache:** `<tool cache>/packages/skills-<commit>[-<16 hex of sha256(path)>]/` with `tree/` (the
  files below the folder) and `files.json` (SHA-256 of every file); verified on every load, a
  changed file is an error (no silent refetch). The raw commit/tag objects are stored next to them.
- **Policy:** `allowed_git_hosts` and `require_signatures` with the SSH `git_allowed_signers` apply
  as for git tool packages (checked before anything is stored; applied again offline from the stored
  objects; `--locked` forbids fetching them again). `GIT_TOKEN` as everywhere.
- `--locked` fails before any request when a source is missing from or stale in the lockfile, and
  works offline from the cache. Nothing is ever built.
- **Command:** `grauwolf tools lock <spec>` also locks skills (decided against a new `spec lock`:
  one place for the lockfile, and the command already means "write `grauwolf.lock`"); `spec build`
  fetches them too. The loader resolves them once per load and hands the tools local cache
  directories (`skills::localize`: a copy of the spec in which git sources are absolute local
  directories).

### Images and generated crates
`spec generate --dockerfile` copies local skill directories to `app/` (`companion_paths`, also for
sub-agents) and fetches git sources at generate time into `app/skills-git/<index>/`; the spec in the
crate (`SPEC_JSON`) – and the copy of a sub-agent's spec file in the image (patched JSON, the rest
untouched) – names these directories as `dir`, so the image needs no git and no network. Without
`--dockerfile`, a git source is refused (`--dockerfile` is where the copies go). The generated crate
reads skills from `<base_dir>/<dir>` at run time through the host's `external::tools`.

### Deviations and notes
- The hint in the brief to allow a `spec lock` was not taken (see above).
- The skill list is not in the spec's `system_prompt` text; it is appended at run time (the
  interpreter and the generated code agree by construction; the equivalence test compares output and
  events – the mock LLM ignores system messages, so the prompt itself is covered by unit tests of the
  interpreter and the generator).
- The directory name `skills-git` in `app/` could clash with a local directory of that name.

### Live check (2026-11-12)
Ollama `qwen3.5:9b` (`OLLAMA_THINK=false`), a spec with the local skill `haiku-writer` and a git
skill `pirate-translator` (tag `v1`, served by the `serve_folder` example of `grauwolf-git`).
`tools lock` wrote the `skills` section; "Write a haiku about rain." called
`load_skill {"name":"haiku-writer"}`, the event log showed `skill_loaded`, and the answer was a
three-line haiku with the exact signature line `-- the grauwolf poet`; "Say 'Good morning, friends'
in pirate talk." (`--locked`, from the cache) loaded `pirate-translator` and answered
"Ahoy, matey! …" ending in `Arr! -- Cap'n Grauwolf`; "What is 2+2?" loaded no skill.
