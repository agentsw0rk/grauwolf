# ADR-0130: The project repository – generated harness projects pushed to GitHub on every change

- **Status:** Proposed (User, 2026-11-19: "push the build folder as a project to a GitHub repository,
  with a token and the repository given – every change to an agent should be pushed; think about
  several agents in one project"; decisions P1–P4 of the same day)
- **Date:** 2026-11-19
- **Changes:** ADR-0128 decision 3 (the projects in `build/` are no longer versioned with the
  workspace, but in a repository of their own), ADR-0129 B2 (Kaniko clones that repository, not the
  workspace's).
- **Related:** ADR-0128 (harnesses), ADR-0129 (building in the cluster), ADR-0126 (studio;
  part 2b: GitHub workspaces).

## Context
A harness is one agent tree and one project (ADR-0128 H2): its root spec and its local
sub-agents are generated into `build/<name>/`; independent agents are separate harnesses. ADR-0129
built from the workspace's own repository, which needed the generated code committed next to the
specs and every push done by hand. The owner wants the generated projects in a repository of their
own on GitHub, kept current with every change.

## Decision
1. **One project repository per workspace, one folder per harness (P1).** `build/` of the workspace
   is a clone of the project repository; the project of harness `<name>` is its folder `<name>/`
   (as before `build/<name>/`). A commit touches the folder of one harness only, so the history of
   each agent can be read per folder and a CI or GitOps tool can react per folder. Sub-agents are
   not separate projects: they are generated into the folder of their harness.
2. **Every change is pushed (P2).** When a spec or a harness file is saved in the studio, the studio
   finds the harnesses it belongs to – the harness itself, or every valid harness whose agent tree
   contains the spec as root or local sub-agent –, generates their projects again, commits each
   changed folder (`<name>: regenerated from <root spec>`, the saved file named) and pushes. A
   project whose files did not change gives no commit. Removing a harness file removes its folder
   in a commit. The work runs per workspace one after another in the background; a save never
   waits for it and never fails because of it. The System tab shows per harness the last pushed
   commit, "pushing…", or why it failed (generation error, push refused).
3. **Repository per workspace, token per studio (P3).** The workspace's settings hold the repository
   (`owner/name` on GitHub; the field also takes a pasted GitHub address, which is stored as `owner/name`, owner's decision 2026-11-19) and the branch (default `main`) – no secret, in
   `.grauwolf-studio/workspace.json`. The studio holds one GitHub token (*our choice:* the git token
   of ADR-0129's Kubernetes dialog becomes the studio's GitHub token, mode 0600; owner's decision
   2026-11-19: only the token is entered – git over https needs no user name; the studio asks GitHub
   whose it is, `GET /user`, and stores `login`, `name` and `id` with it): it pushes (it needs write access to the repository's contents) and Kaniko clones with
   it. The token is set in the studio's GitHub dialog together with the project repository (owner's
   decision 2026-11-19), routes `/api/v1/github/token`; the Kubernetes dialog only says whether it
   is set.
4. **Creating the repository on request (P4).** If the repository does not exist, the settings offer
   *Create repository* (private, through the GitHub API with the token); then the studio clones it
   into `build/`. An existing `build/` that is no clone is turned into one (git init, `origin`, the
   branch fetched) and its content is committed with the first push.
5. **The workspace's own repository** (*our choice*): if the workspace is a git checkout, the studio
   adds `/build/` to its `.gitignore` when the project repository is set up, and says so.
6. **Building (ADR-0129) is unchanged in shape:** `build_source(build/<name>)` finds `build/.git`, so
   Kaniko clones the project repository at the pushed commit and builds `<name>/`.
7. **Layers.** Cloning, committing a folder and pushing over https with a token live in
   `grauwolf-git` (libgit2); creating a GitHub repository is a small HTTP call in the studio.

## Open points
- A command line counterpart (`grauwolf harness publish`) for working without the studio.
- Changes made outside the studio (an editor, a coding agent through MCP that saves through the
  studio counts as the studio) are pushed only at the next save in the studio.
- A push refused because the remote moved on (someone else pushed): rebase or stop with the message.

## Implementation notes (decision 2)
- `Publisher` (`crates/apps/grauwolf-studio/src/publish.rs`) holds one queue and one background
  task per workspace; a harness queued twice is one job. A job is skipped (state `idle`) unless
  `build/` is a clone of the project repository. Generation, commit and push use the same code as
  `harness generate` (`grauwolf_host::project::generate_harness`) and `grauwolf-git`
  (`commit_folder`, `push`), so a commit touches `build/<name>/` only.
- Triggers are every save the studio knows: the document `:save`, `PUT /specs`, `PUT /harnesses`,
  `PUT /files` and the MCP tools that save or write. A harness is affected if the saved file is its
  harness file, its root spec, or a node of its agent tree (read phase only, no network). Tool
  source files (not `.json`) do not trigger; they are published with the next spec save.
- A deleted harness file removes `build/<name>/` in a commit `<name>: removed`; the name is read
  before the delete, else the last published one is used.
- Commit author: the name (else the login) of the GitHub user the token belongs to and `<id>+<login>@users.noreply.github.com` (`<login>@…` for a token stored before the account was read), else
  `grauwolf studio <studio@grauwolf.invalid>`. Messages: `<name>: regenerated from <root spec>`
  (the root spec of the harness, relative to the workspace, not the saved file).
- `GET /workspaces/{id}/project-repository/status` and the document event `published` carry
  `idle|queued|publishing|published|unchanged|failed` with the last commit and the error (token
  masked). The state is kept in memory only: after a restart every harness is `idle` until its
  next save.
- Open: a push refused because the remote moved on is a `failed` state with the server's message
  (no rebase); the queue does not retry.

## Consequences
- Every save may build tool code (generation resolves the agent tree) and pushes a commit: the
  project repository's history is fine-grained.
- The workspace repository no longer carries generated code; the project repository carries only
  generated code (with `GENERATED.md` in every folder).
- The GitHub token can write to the project repository; it is stored like the API keys and never
  shown again.
