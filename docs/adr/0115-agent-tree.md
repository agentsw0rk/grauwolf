# ADR-0115: The prepared agent tree – one walk over a spec and its sub-agents

- **Status:** Accepted (User, 2026-11-13: architecture review, candidate "prepared agent tree";
  decisions Q1–Q15 of the review session)
- **Date:** 2026-11-13
- **Related:** ADR-0114 (provisioning starts from a loaded, resolved spec; `SubAgentSupplier`),
  ADR-0040 (sub-agents), ADR-0055 (A2A sub-agents), ADR-0101 (`agent`/`for_each`, sub-agent
  signatures), ADR-0073 (`code_file`, `tool_files`), ADR-0102 (code pieces), ADR-0103 (skills),
  ADR-0107 (tool packages, lockfile, policy, "the image build stays offline"), ADR-0096
  (`--dockerfile`, `app/`). Terms: `GLOSSARY.md` (agent tree, prepared agent tree).

## Context
A spec and the specs of its sub-agents form a tree. Loading it means, per node: read the spec,
read code files, localise git skills, resolve tool packages against the lockfile, build code tools,
determine companion files, collect sub-agent signatures, detect cycles. Five places walk or partly
walk this tree, each with its own rules (facts gathered 2026-11-13):
- the **loader** recurses with an ancestor chain: cycles fail, but a sub-spec under two parents
  (a diamond) is loaded, built and provisioned twice, each copy with its own MCP servers and
  knowledge index;
- **`spec validate`** keeps a seen-set: a cycle passes silently, every sub-spec is read twice, an
  unreadable child is skipped for the signature check;
- **`spec generate --dockerfile`** identifies nodes by their path relative to `app/`, skips
  sub-specs with `..` or absolute paths silently, localises git skills of sub-agents but neither
  builds their code tools nor resolves their packages – the image would fetch them at run time
  without a lockfile, against ADR-0107;
- **`spec build`** and **`tools lock`** look at the root only; `tools lock` reports "nothing to lock"
  when only sub-agents have packages or git skills;
- every spec file resolves against its **own** `grauwolf.lock` and `grauwolf.toml`.
So validate can accept a tree the loader rejects, and a new source kind (git skills touched 24 files
in 10 crates) has to be handled in all of these places.

## Decision
1. **New module `grauwolf_host::agent_tree`** with `AgentTree` and `AgentNode` (the prepared agent
   tree). All five callers use it.
2. **Two phases.**
   - **Read** (local, no network): read every spec, read `code_file`/`tool_files`, find the
     children, compute sub-agent signatures and companion files, check cycles and paths. This is
     enough for `spec validate` (validation never needs the network, ADR-0107).
   - **Resolve** (network, lockfile): localise git skills, resolve tool packages, build code tools,
     write the lockfile or check it with `--locked`. Used by `build`, `tools lock`, `run`, `serve`,
     `chat`, `generate`.
3. **One rule set for the whole tree.**
   - A node is identified by its canonical path. A sub-spec reached from several parents (diamond)
     is read and resolved once and shared; at run time it is also **provisioned once** and shared by
     its parents (one set of MCP servers, one knowledge index).
   - A cycle is an error everywhere, naming the chain (`a.json → b.json → a.json`).
   - A sub-spec outside the root spec's directory (`..` or an absolute path) is an error everywhere
     ("sub-agent spec outside the agent's directory"), so every tree can be copied into an image.
   - A sub-agent reached over A2A (`url`) is a leaf: URL, no children, no signature, no companions.
   - Errors from any node use the format `<file>:<pointer>: …`, the same in every command.
4. **One lockfile and one policy per tree.** `grauwolf.lock` next to the root spec covers all nodes;
   entries of sub-nodes carry the node's relative spec path in their key, entries of the root keep
   today's keys (existing lockfiles of specs without sub-agents stay unchanged). A lockfile next to a
   sub-spec is not used inside a tree (warning). The tool policy (`GRAUWOLF_TOOL_POLICY`, else
   `grauwolf.toml` next to the root) applies to every node, so no sub-spec brings a laxer policy.
5. **Callers.**
   - The loader takes a prepared tree and serves provisioning's `SubAgentSupplier` from its nodes
     instead of recursing itself (ADR-0114).
   - `spec validate` reports on the read tree.
   - `spec build` and `tools lock` resolve the whole tree; "nothing to lock" only when no node has
     anything to lock.
   - `spec generate --dockerfile` copies the tree into `app/` (specs, companion files, localised
     skills) **together with the tool-cache entries of every node and the lockfile**; the generated
     service loads its sub-agents with `--locked` and needs no network for them. The copy logic
     moves from the CLI into the module; `companion_paths` stays in the generator as the pure
     answer to "which paths belong to a spec" and is used by the tree.
6. **Tests through the interface.** The main verification is `AgentTree` tests over temporary spec
   trees with the test git server and the test registry: diamonds, cycles, outside paths, A2A leaves,
   lockfile and `--locked` without network, policy of the root, companions and signatures, the copy
   into `app/`. The CLI binary tests shrink to a thin safety net.
7. **Order of work**, every commit green: the module (read phase) with its tests → the resolve phase
   → `spec validate` → the loader (diamonds shared in provisioning) → `build` and `tools lock` →
   `generate --dockerfile` (copy with cache and lockfile) → remove the old walkers, move tests, docs.

## Consequences
- One place decides what a tree is; `validate`, `build`, `lock`, `run` and `generate` agree.
- Behaviour changes (deliberate): cycles fail in `validate` too; sub-specs outside the root's
  directory are refused; diamonds are shared; sub-agents' lockfiles and policies are replaced by the
  root's; `build`/`lock` cover sub-agents; images build sub-agents' tools in and run offline.
- A new source kind that needs resolving is added once.

## Implementation notes (2026-11-13)
Built in the order of decision 7, one commit per step; the module is `grauwolf_host::agent_tree`
(`AgentTree::read` / `read_spec`, `resolve(&ResolveOptions)`, `copy_into`, `AgentNode`).
- **Lockfile key format.** Entries of the root keep their keys; entries of every other node are
  `<spec path relative to the root's directory>#<key>`, in all three sections (`packages`, `git`,
  `skills`), e.g. `agents/writer.json#git+https://host/repo.git//tools@v1`. Each node resolves
  against its view of the file (`PackageResolver::resolve_in` / `resolve_skills_in` work on
  lockfile content); entries that no node claims fall into the root's view. Uniformly for `packages`, `git` and
  `skills`: under `--locked` any entry that no node of the tree claims (for example one left over
  from a removed sub-agent, or any entry when no node has packages or git skills) is a
  stale-lockfile error naming the entry; without `--locked` resolving rewrites the lockfile
  without it (a lockfile without entries is removed). Before, such entries counted as stale only
  when the root itself had tool packages (or git skills), and a tree without packages never looked
  at the lockfile. A tree without packages and without a lockfile still leaves everything alone.
  The copy for an image (`copy_into`) writes the lockfile without `skills`: git skills travel as
  directories and the copied specs name those, so nothing in the copy would claim the entries.
  Tests in `agent_tree_resolve.rs`: leftover of a removed sub-agent, sub-agent-only tree with a
  root-level entry, unclaimed git and skills entries.
- **Deviations and additions.**
  - The read phase also checks unknown providers and local skill directories (what `spec validate`
    did per file), so `run` refuses the same trees. An absolute sub-spec path is refused even
    inside the root's directory (the copied spec file would keep it); `..` that stays inside is
    allowed.
  - `ResolveOptions` has `build_code` (`tools lock` does not build) and `resolve_root` (the
    generated service embeds its root: `AgentLoader::supply` neither resolves nor builds it and
    keeps its lock entries).
  - `AgentLoader::for_provisioning` is `locked`. The tool policy is not copied into the image; it
    applies when the image is built.
  - The image has no `rustc`: `ToolBuilder` takes the toolchain version for its cache key from
    `<cache>/toolchain` where `rustc` is missing, and `copy_into` writes that file
    (`ToolBuilder::export_toolchain`) with the shared `Cargo.lock`, the built artifacts and the
    package entries of the sub-agents' lock entries in `app/.grauwolf/tools`, plus `grauwolf.lock`.
  - Sub-agent specs in `app/` are written from the original document with `code_file` and
    `tool_files` inlined and git skills replaced by `skills-git/<index>` (the old `patch_skills`
    kept the file references, which the image could not resolve any more).
  - `spec generate` without `--dockerfile` now reads and resolves the whole tree too.
  - Removed: `validate_tree`, `copy_companions`, `localize_skills`, `patch_skills`,
    `copy_skill_copies`, the loader recursion (`load_in`, `supply_in`, chain), `skills::localize`
    and `skills::resolve_git`, the CLI helpers `resolve_packages` / `resolve_git_skills`.
- **Tests.** `tests/agent_tree.rs` (11, read), `tests/agent_tree_resolve.rs` (7, one slow: lockfile,
  prefixed keys, `--locked` offline, sub-lockfile warning, root policy, diamond once, code tools in
  the shared cache), `tests/agent_tree_copy.rs` (2, one slow: package + git skill run from the copy
  without network; code tools and toolchain file), `tests/loader.rs` (+1: a diamond is provisioned
  once), `tests/git_skills.rs` (ported to the tree), CLI safety net (+3: validate refuses cycles and
  outside paths, `tools lock` / `spec build` over a sub-agent's package, `generate --dockerfile`
  copies it with the lockfile), `grauwolf-toolbuild` (+2).
- **Live check** (Ollama `qwen3.5:9b`, no Docker): tree `coordinator → {writer, reviewer}`, `writer →
  reviewer`, a code tool in `reviewer`: `spec validate` valid, `spec build --locked` builds the tool
  once, `spec run` answers through the shared sub-agent, `spec generate --dockerfile` writes
  `app/agents/{writer,reviewer}.json` and `app/.grauwolf/tools/{artifacts,Cargo.lock,toolchain}`; a
  cycle and an outside path give the same error in `validate` and `run`.
- German handbooks unchanged (snapshot).
- **Moved to `grauwolf-agent-tree` (2026-11-13, ADR-0118).** The module left the host for the
  adapter crate `grauwolf-agent-tree`, with the spec reading it is built on (`read_spec`,
  `tool_files`/`code_file`, `warning_lines`, `build_tools`) and the skill directory check, so that
  `grauwolf-tools` reads trees with the same code. The provider check needs the host's providers
  and became an option of the read phase (`ReadOptions::providers`); `grauwolf_host::agent_tree`
  re-exports the types and has `read` / `read_spec` with the host's options, so `validate`, `run`
  and `generate` check exactly what they did. The resolve phase and `copy_into` moved too (the
  crate has `grauwolf-toolpkg` and `grauwolf-toolbuild` anyway; feature `packages` as before). The
  tests moved with it to `crates/adapters/grauwolf-agent-tree/tests/` (`read.rs`, `resolve.rs`,
  `copy.rs`, `git_skills.rs`; the test registry and git server are dev-dependencies of the crate);
  the host's `tests/agent_tree.rs` and `tests/git_skills.rs` keep what only the host does (its read
  options, the loader over a copied tree and over git skills).
