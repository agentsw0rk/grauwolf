# ADR-0128: Harnesses – the boundary of a generated, deployable project

- **Status:** Proposed (User, 2026-11-19: "approach the harness, the generation of the code with a
  Dockerfile, as a project – a box around the nodes in the studio with its own properties; several
  harnesses lead to several generated projects"; decisions H1–H4 as recommended; H5 2026-11-19: Kubernetes through `grauwolf-k8-ops` with the harness file as input, no manifests in the project; H6 2026-11-19: the system level as an editor tab drawn with Rete, laid out by ELK without saved positions; H7 2026-11-19: a default image from a registry per workspace; H8 2026-11-19: the harness frame on the overview; H9 2026-11-19: one place to work – stage 1 without the System tab, stage 2 a workspace canvas; H10 2026-11-19: the workspace canvas, 2a select and edit in the inspector, 2b edit on the canvas)
- **Date:** 2026-11-19
- **Related:** ADR-0126 (studio, decision 2 "one harness is one spec", decision 5.2 deployment
  through `grauwolf-k8-ops`), ADR-0115 (agent tree), ADR-0093/0096/0119 (generated service,
  `--dockerfile`, builder image), ADR-0120/0121 (`grauwolf-k8-ops`, one agent, one namespace,
  `agent deploy`), ADR-0055/0101 (A2A sub-agents, `agent`/`for_each`), ADR-0123 (traces).

## Context
The studio treats "one harness as one spec" (ADR-0126), but the harness has no shape of its own:
there is no box on the canvas, no properties and no project that comes out of it. Everything needed
to make one exists separately:
- `spec generate --service --dockerfile` writes a crate with a `Dockerfile` that builds the agent
  tree offline on the builder image (ADR-0096);
- `grauwolf-k8-ops agent deploy` runs an image or a spec in the agent's own namespace with
  identity, configuration and secrets (ADR-0121);
- the agent tree (ADR-0115) already knows which specs, code files, skills and packages belong to
  one root spec.

In AgentWeaver the harness box was exactly this boundary: everything inside it became one exported
project with a root `Dockerfile` and Kubernetes manifests, and several harnesses on one canvas gave
several projects. Its properties were a name (DNS label), the backend, the port, the Docker base
image, a skill registry and OpenTelemetry settings.

What grauwolf lacks is a description of **how** an agent tree is packaged and run – which is not
part of what the agent does, and so does not belong in the spec.

## Decision
1. **A harness file per harness (H1).**
   - A harness is described in a file of its own, `<name>.harness.json` (for example under
     `harnesses/`), which refers to its root spec by a path relative to the file.
   - The spec stays free of packaging and deployment details; one spec can be the root of several
     harnesses (for example a generated and an interpreted variant).
   - The file is a grauwolf format, versioned (`"version": 1`) and read by the CLI as well as the
     studio, so a harness can be generated without the studio.
   - First draft of the content:
     - `name` – DNS label (a–z, 0–9, `-`, starts with a letter, at most 63 characters); it names
       the generated crate, the image, the Kubernetes namespace and service, and the A2A host;
     - `spec` – the root spec;
     - `mode` – `generated` (crate and image, the default) or `interpreted` (`spec serve` on the
       CLI image with the agent tree in a ConfigMap, ADR-0121 decision 1);
     - `service` – port, A2A port and base path (ADR-0093);
     - `image` – repository and tag of the image, builder image (ADR-0096);
     - `env` – values that are not secret; `secrets` – references to OpenBao, never values
       (ADR-0110);
     - `tracing` – export of traces (ADR-0123);
     - `replicas` and `resources`.
2. **A harness is one agent tree (H2).**
   - A box contains the root spec and its local sub-agents – exactly the prepared agent tree of
     ADR-0115. That is one crate, one image, one namespace (ADR-0120).
   - Independent agents are not grouped into one container; they get a harness each.
   - An edge from an agent in one box to an agent in another box is an **A2A call**: the caller's
     spec gets an A2A sub-agent whose URL is derived from the target harness's name and service at
     generation time.
3. **The generated project lives in the workspace (H3).**
   - Generating a harness writes `build/<name>/` in the workspace: the crate from
     `spec generate --service --dockerfile` (with its `Dockerfile` and `app/`). No Kubernetes
     manifests: see decision 6.
   - It is visible in the studio's file explorer, versioned with the workspace and can be
     downloaded as a ZIP.
   - Generating again replaces the folder completely; the folder is output, not a place for
     hand edits (a note file in it says so).
   - Several harnesses give several projects side by side.
4. **First step: box, properties and generation (H4).**
   - A new **system** level in the studio above overview and flow: all harnesses of the workspace
     as boxes (Rete frames, laid out by ELK as compound nodes like the subgraph frames of decision
     19 of ADR-0126), inside each its agents; a double click on an agent opens its overview.
   - (H9, User 2026-11-19, replaces H6) **One place to work, in two stages.** The split between the
     System tab and the spec's overview is given up. *Stage 1:* the System tab goes; a click on
     the harness frame of a spec's overview selects the harness, and the spec's inspector edits it
     (properties, Generate, Build & push, publish state); the workspace-wide parts – every harness
     with its state and problem, New harness, Generate all, GitHub, Kubernetes – are a side bar
     view **Harnesses** of the activity bar. *Stage 2:* a workspace canvas with every harness frame
     and its agent, resources and local sub-agents editable on one surface (the flow stays in the
     spec's tab), where the A2A edges of step 5 are drawn.
   - (H10, User 2026-11-19) **Stage 2, the workspace canvas.** An editor tab **Workspace** shows
     every harness as a frame around the overview of its root spec (agent, resources, local
     sub-agents – the same drawing and the same positions as the spec's overview, which stay in
     the spec's layout file); root specs without a harness stand outside the frames, marked "no
     project", with *New harness*. Only the frames' places are new: `.grauwolf-studio/
     workspace.layout.json`. A double click on an agent opens its spec's flow. It is the start
     view of a workspace that has harnesses. *2a:* select a node or frame and edit it in the
     inspector (through the spec's document: undo and live sync as in the spec tab), move frames
     and nodes. *2b:* add, connect, park and delete resources on the workspace canvas itself.
   - (H8, User 2026-11-19) The harness is **also drawn where one works**: the overview of a spec
     that is the root of a harness shows a frame around the agent, its resources and its local
     sub-agents, titled with the harness's name (several names if the spec is the root of several
     harnesses); remote agents (`url`, A2A) stay outside. A click on the title opens the harness in
     the System tab; a spec without a harness offers *New harness* for itself. The System tab stays
     the view of all projects of the workspace (and of the A2A edges of step 5).
   - (H6, User 2026-11-19) The system level is an **editor tab of its own, "System"**, one per
     workspace and without a file, opened from the activity bar and the command palette; it is
     drawn with **Rete** like the overview and the flow (agents as nodes, harnesses as frames), so
     that the A2A edges of step 5 fit in. The boxes are **laid out by ELK every time**; their
     positions are not saved yet (a `.grauwolf-studio/system.layout.json` comes when moving them is
     needed).
   - The inspector edits the harness's properties with the same validation as the CLI.
   - "Generate" (per harness and for all) writes the projects of decision 3; errors of the agent
     tree appear at the box.
   - Building the image and deploying with `grauwolf-k8-ops` from the studio is the next step
     (ADR-0126 decision 5.2), with the harness file as its input.
5. **Layers.** The harness format, its validation and the generation live below the apps (the
   format next to the agent tree, the project writing in the generator) and get a CLI command; the
   studio backend only calls them.
6. **Kubernetes through `grauwolf-k8-ops`, with the harness file as input (H5, User 2026-11-19).**
   - The project gets no `k8s/` folder. What `agent deploy` writes depends on the cluster: the
     namespace, service account and OpenBao role of `agent add`, the SPIFFE CSI socket, the
     `grauwolf.toml` with the OpenBao mapping, a generated server token, `grauwolf-callers` and the
     egress NetworkPolicy resolved through DNS. Static manifests could not be applied on their own,
     and rendering them in a second place would duplicate `deploy.rs`.
   - Instead `grauwolf-k8-ops agent deploy --harness <file>` reads from the harness file what is
     given as flags today (the agent's name, and with later fields the image, ports, `--env`,
     tracing); `grauwolf-k8-ops` stays the only program that writes cluster objects.
   - Harness fields are added with the deploy option they replace; a flag given next to
     `--harness` for the same setting is refused, so there is one source per value.
   - Extracting the object rendering into a shared crate, to show manifests in the project, stays
     possible later; it is not planned.
   - (H7, User 2026-11-19) **A default image from a registry per workspace.** Without `image` in
     the harness, the image is `<registry>/<name>:<grauwolf version>`, with `registry` from the
     section `[harness]` of the nearest `grauwolf.toml` – in the harness file's folder or a folder
     above it. Without such an entry there is no default and the image must be given. The port
     needs none: it is 8080 in the generated `Dockerfile` and in `agent deploy` alike, and the
     studio shows that in the empty field. Caveat: a rebuilt image of the same grauwolf version
     keeps its name, so the Deployment does not change; a new build of an agent gets an `image`
     with its own tag (or a restart).

## Open points
- The exact place of the harness format – decided in the implementation plan against ADR-0016
  (step 1: `grauwolf-spec-model`).
- The name of the CLI command (`grauwolf harness generate <file>` as a first proposal).
- Where the system canvas keeps its layout once boxes can be moved – decided with H10 2a:
  `.grauwolf-studio/workspace.layout.json`.
- H10 2a, left open: the workspace canvas opens a working copy of every spec of the workspace (it
  must know the local sub-agents to tell root specs from sub-agents), which costs a request per spec
  on a large workspace; a node's position is read from the mirror's layout, so a spec that has
  never been opened and has no layout file shows the default overview positions (`completeOverview`)
  until it is shown in a spec tab, where ELK may place it differently; problem markers of the
  nodes are not drawn on this canvas; the viewport is saved only together
  with the frames' places (a frame drag).
- How the studio shows that a generated project is older than its specs.
- Whether an `interpreted` harness also writes a project folder or is only deployed with
  `agent deploy --spec`.

## Consequences
- "One harness is one spec" in ADR-0126 becomes "one harness is one agent tree with its own
  harness file"; a spec without a harness can still be opened, validated and run in the preview.
- Packaging and deployment get one description that the CLI, the studio and
  `grauwolf-k8-ops` share, instead of command-line flags repeated in each.
- A2A between harnesses becomes visible on the canvas; the URL of a sub-agent in another harness
  is no longer typed by hand.
- Generated code in the workspace makes reviews of the generated project possible, but enlarges the
  workspace's repository.

## Implementation notes (2026-11-19, step 1: format and CLI)

- **Format:** `Harness` in `grauwolf-spec-model` (`src/harness.rs`; re-exported by `grauwolf-spec`):
  `version`, `name`, `spec` for now – the other fields of decision 1 come with the steps that use
  them. `Harness::parse` reads with `deny_unknown_fields` and the spec's format errors
  (`SpecError::Format` with a JSON pointer), then refuses another `version` than
  `SUPPORTED_HARNESS_VERSION` (`/version`) and a `name` that is no DNS label (`/name`).
- **CLI:** `grauwolf harness generate <file> [--out-dir build]` (`commands/harness.rs` of
  `grauwolf-cli`) resolves `spec` against the harness file's directory, removes an earlier
  `<out-dir>/<name>/`, runs `spec generate --dockerfile` into it and adds `GENERATED.md`. The
  folder is removed only after the harness file was read without errors.
- **Open point decided:** the CLI command is `grauwolf harness generate`.

## Implementation notes (2026-11-19, step 2: `agent deploy --harness`)

- **Fields:** `image`, `port` and `env` (an object `NAME` → value) – optional, read only by
  `grauwolf-k8-ops`. The draft's `service` object, `mode`, `secrets`, `tracing`, `replicas` and
  `resources` are not added yet; the A2A port and base path are no deploy option today.
- **`grauwolf-k8-ops agent deploy --harness <file>`** (`src/harness.rs`, `agent.rs`): the positional
  name is refused next to it (clap), `--image` is required only without a harness. `one_source`
  refuses a flag for a setting the file sets, before anything is written. The harness's `env`
  becomes `NAME=VALUE` lines through the checks of `--env` (names, reserved variables, OpenBao
  mapping). `--port` lost its clap default; `DEFAULT_PORT` (8080) applies when neither gives one.
- **Shared messages:** `Harness::parse_named(name, json)` in `grauwolf-spec-model` words the errors
  for the CLI and `grauwolf-k8-ops`, which may not depend on each other.
- `spec` of the harness is not used by `agent deploy` yet (an interpreted harness, `--spec`, is an
  open point).

## Implementation notes (2026-11-19, step 3: the studio's API)

- **Routes** (`grauwolf-studio`, `/api/v1/workspaces/{id}/…`): `GET /harnesses` (every
  `*.harness.json` with `name` and `spec`, or only its `problem`), `GET`/`PUT /harnesses/{path}`
  (as stored, keys in their order; only a valid harness is written, `422 invalid_harness`) and
  `POST /harnesses/{path}:generate` (`{ project, warnings }`, `422 generate_failed`).
- **Layers (decision 5):** the generation moved from the CLI into
  `grauwolf_host::project` (`generate_harness`, `write_crate`, `copy_app_files`, `BUILDER_CRATES`,
  `NOTE_FILE`); the chat API's file names moved with it (`grauwolf_server::chat_api` re-exports
  them). The CLI and the studio call the same function, so a project is the same from both.
- **`build/`** is left out of the studio's spec and harness listings (its `app/` holds copies of
  the specs); the file tree shows it.
- **Open:** a failed generation has already removed the earlier project when the agent tree or a
  tool build fails after the harness file was read; generating into a temporary folder and
  swapping it in would keep the old one.

## Implementation notes (2026-11-19, step 4: the System tab)

- **Tab:** `SYSTEM_TAB` (`lib/tabs.ts`, path `system:` – a `:` no workspace path holds); the file
  store's main tab `system`; `store/systemStore.ts` loads `GET /harnesses` and `GET /specs`.
- **Drawing:** `lib/system-view.ts` – `systemModel` (a box per harness, its root spec resolved
  against the harness file), `systemDrawing` (agents as nodes of kind `agent` with role `resource`,
  since the role `agent` shows the open spec's badge; an invalid harness as a marked node of kind
  `harness`; one frame per harness), `systemLayout` (ELK, each harness as a group). The canvas is
  the read-only Rete set-up of the derived flow (`sync`, `drawFrames`).
- **Inspector:** `HarnessInspector` with `lib/harness-form.ts` (the file's key order kept, empty
  optional fields left out); Save through `PUT /harnesses/{path}`.
- **Generate:** *Generate project* and *Generate all* through `POST /harnesses/{path}:generate`,
  one result line per harness.
- **New harness:** a dialog (header, empty state, command) with the root spec and a name suggested
  from it (`suggestedName`), checked before writing (`newHarnessProblem`: a DNS label, the file must
  not exist – `PUT` would replace it), written as `harnesses/<name>.harness.json` (`newHarness`).
- **Not yet:** the local sub-agents inside a box, saved positions; `npm run build`
  cannot run in the development container (the `node_modules` there come from macOS), so the build
  and a look in the browser are done on the owner's machine.

## Implementation notes (2026-11-19, step 4b: the harness frame on the overview, H8)

- `harnessesOfSpec` and `harnessFrame` (`lib/system-view.ts`): the overview of a root spec gets a
  `CanvasGroup` with `title` and `harness` around the agent and its resources, without `a2a_agent`
  nodes and parked resources; `drawFrames` draws such a frame blue and opens its harness in the
  System tab instead of folding (the System tab's frames got the same title, `harness <name>`,
  instead of `subgraph <name>`). `EditorApp` loads the harnesses on mount, so the overview knows
  them without the System tab.
- The spec's inspector has a *Harness* section (`HarnessSection`): its harnesses, or *New harness*
  with the spec preselected (`newHarnessDefaults`); the dialog moved to `EditorApp`.
- Not yet: a sub-agent's overview does not say which harness it is part of.

## Implementation notes (2026-11-19, H9 stage 1: one place to work)

- **Selection:** `{ type: "harness", path }` in the spec store's `Selection` and `lib/selection.ts`
  (no pointer, no JSON: a harness is not part of the spec). `selectHarness(path)` sets it, shows
  the inspector's Properties tab and reads the harness file; the spec's `Inspector` shows
  `HarnessEditor` (form, Save, Generate project, Build & push with its log, publish state) for it.
  A click on a harness frame's title or badge on the overview, and the buttons of the *Harness*
  section, select the harness; the frame carries the publish badge.
- **Side bar view Harnesses:** `SideView` `harnesses`, `HarnessesView`, with `harnessRows` and
  `repositoryHint` (`lib/harness-list.ts`, pure). A row opens the root spec on the overview with
  the harness selected (`openHarness`); a harness with a problem (invalid file, missing root spec)
  opens its file in the file editor. New harness, Generate all (with the result lines), GitHub… and
  Kubernetes… are its buttons; the dialogs are mounted in `EditorApp`. The command *Show system:
  harnesses* became *Show harnesses*.
- **Removed:** the System tab (`SYSTEM_TAB`, the tab kind and main tab `system`, `SystemView`,
  `SystemCanvas`, the activity bar's System button, `systemDrawing`, `systemLayout`, the node-key
  helpers). `systemStore` is now `harnessStore`, `lib/system-view.ts` `lib/harness-view.ts`,
  `HarnessInspector` `HarnessEditor`.
- **Not yet:** stage 2 – the workspace canvas with every harness frame; a harness file written by
  hand shows in the Harnesses view when the view is opened (it loads the harnesses then).

## Implementation notes (2026-11-19, H10 stage 2a: the workspace canvas)

- **Backend:** `GET|PUT /workspaces/{id}/workspace-layout` (`layout::WorkspaceLayout`,
  `workspace::WORKSPACE_LAYOUT_FILE`) for `.grauwolf-studio/workspace.layout.json`:
  `{ workspace_layout_version: 1, frames: { "<frame key>": { x, y } }, viewport?: { x, y, zoom } }`.
  The frame key is the harness file's path, or `spec:<spec path>` for a root spec without a
  harness. Missing file → empty layout; `PUT` checks version, finite places and a positive zoom
  (`400 invalid_layout`) and writes atomically. **A frame's place is the shift of its spec's
  overview** (added to the node positions of the spec's layout), not the frame's top left corner:
  moving a node inside a frame changes the frame's box but never the other nodes.
- **Model:** `lib/workspace-canvas.ts` (pure, tested): one frame per valid harness root spec,
  keys `<spec path>|<overview key>`; root specs that are no harness root and no `agent_tools[].spec`
  of another spec: agent node only, frame `spec:<path>` "<name> · no project" with *New harness*;
  frames without a place in a row; `inSpec` for the way back.
- **Tab and store:** the tab kind `workspace` (`lib/tabs.ts`), `store/workspaceCanvasStore.ts`,
  `components/studio/WorkspaceView.tsx` and `WorkspaceCanvas.tsx` (reusing `createSpecEditor`,
  `sync`, `drawFrames`). The specs are not read with `GET /specs/{path}` but through the document
  store's working copy (`ensureDocument`), so unsaved edits, MCP changes and undo history are the
  same as in the spec tabs; the spec store opens specs through the same function and so reuses a
  mirror the canvas holds.
- **Editing:** a click on a node makes its spec the shown spec of the spec store in the background
  (so it also gets a spec tab) and selects it, the inspector edits it; a click on a frame selects
  its harness in its root spec; a double click on an agent opens its spec (flow, or overview for a
  strategy). Nodes of the shown spec move through the spec store, those of another spec as an
  undo step of that spec's document; Undo/Redo in the tab act on the spec last selected or moved
  (`workspaceCanvasStore.lastSpec`). Delete does nothing on the tab (2b).
- **Start:** opening a workspace that has harnesses shows the tab; command *Show workspace*, button
  *Show workspace* of the Harnesses view.
- **Not yet:** the A2A edges of step 5. (2b below.)

## Implementation notes (2026-11-19, H10 stage 2b: editing on the workspace canvas)

- No backend change. Every edit makes the spec of the frame the shown spec of the spec store in
  the background (as the selection does in 2a) and runs the spec store's own action, so the undo
  label and group, the refusals and `refreshAgents` are those of the overview and not copied:
  `workspaceCanvasStore.connectResource|disconnectResource|removeResource|addResource|
  addAgentReference` (`onSpec`), each setting `lastSpec`, so Undo/Redo follow the spec last touched.
- **Routing, pure and tested** (`lib/workspace-canvas.ts`): `connectionOf` (resource and agent of
  one spec; two specs → error message, A2A edges are step 5), `dropTarget` (the frame under a canvas
  point → its spec and the point in the overview's coordinates, `WorkspaceDrawing.frames`),
  `resourceOfEdge`.
- **Canvas:** `connect`/`refuse`/`pick` callbacks of `createSpecEditor` (highlighting limited to the
  picked node's spec), context menu (Connect/Disconnect/Delete of a resource, Disconnect of a
  connection, Open the spec on an agent), drop of palette resources and workspace specs into a frame
  (outside a frame: a message); palette click adds to `lastSpec` (message when none), parked next to
  the agent. Delete removes the selected resource of `lastSpec` on the tab.
- **Left smaller:** a palette click does not scroll or place into the visible area (the overview's
  `visibleRect` logic) but puts the node next to its agent, which may be off screen; connections
  cannot be selected by click on this canvas, so Delete on a connection is the menu only; the store
  routing and the component wiring have no automated test (the pure parts do; Playwright was not
  run); a drop onto a frame whose spec's agent is a strategy is accepted like on the overview.
- **Open:** A2A edges between harness frames (step 5); a click on a palette entry with no spec ever
  selected only shows a message.
