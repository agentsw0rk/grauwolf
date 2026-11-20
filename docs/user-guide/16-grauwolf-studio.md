# 16 – grauwolf studio: drawing specs

> **Enterprise** – grauwolf studio is not part of the open source distribution; its code is not in the public repository. This chapter documents it for users of the Enterprise edition.

grauwolf studio is a visual editor for your specs. You draw the flow of an agent on a canvas – nodes
from the palette, edges between them – and the studio writes the spec JSON you know from the
previous chapters. Nothing is translated: the canvas edits the spec file directly, so what you see
is what `grauwolf spec run` runs.

This chapter covers parts 1 and 2a of the studio: local workspaces – separate projects, each a
folder of specs –, drawing, and running a spec right in the studio with live events, approvals, API
keys and model lists, in an IDE-like frame with tabs, panels and a command palette – and drawing
together with a coding agent such as Claude Code, which edits the same specs through MCP while you
watch. GitHub workspaces come later; the header shows them, disabled.

## Starting the studio

The studio is one program, `grauwolf-studio`, that serves the editor in your browser. Build the
editor once and start the studio:

```bash
cd studio/web && npm ci && npm run build && cd ../..
cargo run -p grauwolf-studio -- serve --web studio/web/out
```

Open <http://127.0.0.1:8090>. The studio listens only on your own machine; it has no sign-in yet.
It keeps its workspaces in `~/grauwolf-studio/workspaces` (`--root <dir>` for another folder);
`--workspace <dir>` works on that one folder only, `--addr` picks another address, `--config-dir`
another folder for your API keys (default `~/.config/grauwolf/studio`), `--mcp-token` a fixed
token for coding agents (see [Drawing together with a coding agent](#drawing-together-with-a-coding-agent)).

The script `scripts/studio.sh` does all of this in one go: it installs the frontend's packages,
builds backend and frontend and starts the studio on `~/grauwolf-studio/workspaces`. The first time,
it creates the workspace "examples" with copies of the example specs (so the originals stay
untouched) – or, if you used the studio before, registers your old `~/grauwolf-studio-workspace` as
"examples", leaving its files where they are:

```bash
scripts/studio.sh             # backend on :8090 and the editor with hot reload on http://127.0.0.1:3008
scripts/studio.sh --static    # static export, everything on http://127.0.0.1:8090
scripts/studio.sh --build-only
```

`--root <dir>` picks another root folder, `--workspace <dir>` a single folder, `--host 0.0.0.0` makes
it reachable from outside a container; `--help` lists all options. Ctrl+C stops everything.

## Workspaces

A workspace is a project of its own: a folder with specs, the files they refer to, a tool policy
(`grauwolf.toml`) and a lockfile (`grauwolf.lock`). Validation, file references and templates work
inside the open workspace; another workspace never sees its files.

Without an open workspace the studio shows its **start page**: every workspace with its name,
description, whether it is *own* or *registered*, the number of specs and the folder.

- **New workspace** asks for a name, a description and what it starts with: *Empty* (one first spec,
  `agent.json`, ready to run), *The user guide's examples* (a copy of them) or *A copy of another
  workspace*. The studio makes a folder for it in the root folder and opens it.
- **Register folder** makes an existing folder a workspace – a project of yours, or
  `docs/user-guide/examples` itself. Give the path (`/home/me/agents` or `~/agents`) and, if you
  like, a name. The files stay where they are, and the studio writes nothing into the folder; it
  notes the folder in `.registry.json` of the root folder.
- **Rename** (the pencil) changes name and description; the id in the address stays.
- **Delete** (the bin): for an own workspace you type its id to confirm, and its folder with every
  file in it is deleted. A registered folder is only removed from the list; its files stay.

The open workspace is part of the address, `http://127.0.0.1:8090/?w=examples`, so a reload or a
bookmark opens it again. An address with a workspace that no longer exists shows the start page with
a note.

In the header, next to *grauwolf studio*, is the **switcher**: the name of the open workspace, a menu with the
recently opened ones and *All workspaces…* (the start page). The status bar shows whether
the workspace has a tool policy (`policy:`) and a lockfile (`lock:`). Switching asks first if a spec or
file tab has unsaved changes, and closes them.

With `--workspace <dir>` the studio has just that one workspace (named after the folder): the start
page lists it, and creating, registering, renaming and deleting are not offered.

## Two levels: overview and flow

A spec opens on its **overview**: one node for the agent – name, model with its fallbacks, the kind
of agent (next section), the start of the system prompt, limits – and one node for each of its
resources, connected to it:

| Resource node | Part of the spec |
|---|---|
| tool | one entry of `tools` |
| tool file, tool folder | one entry of `tool_files`, `tools_dir` |
| tool package | one entry of `tool_packages` |
| built-in: *group* | the tools of `builtin_tools` of one group (files, git, kv, todo, …) |
| MCP *name* | one entry of `mcp_servers` |
| agent *name* | one entry of `agent_tools` (a local spec or an A2A agent) |
| knowledge, long-term memory | `knowledge`, `long_term_memory` |
| skills | one entry of `skills` |
| self-built tools | `self_tools` |
| System One (RKEV) | derived from the flow, not a section of the spec: a `decide` node with `decider: system_one` (see below) |

Select a resource to edit its part of the spec as JSON on the right. Select the agent to edit name,
model and system prompt.

Decisions (`decide` nodes) are steps of the flow, so they are **not** on the overview: you find them in
the *Flow* tab.

### The kind of agent

The badge at the top of the agent node says what kind of agent the spec describes (shown in capitals, like the other badges):

| Shown | The spec has |
|---|---|
| **Agent** | a ready-made `strategy` (`single_run`, `react`, `chat`, `verified`) |
| **Graph-Agent** | a `graph` of its own without planner nodes |
| **Planner-Agent** | a `graph` of its own with a `plan` or `goap` node, in the main graph or in a subgraph |

A graph or planner agent also shows a one-line summary of its flow, for example
`Graph: 5 nodes, 1 decision`; `1 decision`, `2 subgraphs` and `1 planner` appear only when there are any. The nodes
of the subgraphs are counted, the implicit `start` and `finish` are not. Click the summary to open the
*Flow* tab. An agent with a strategy shows its strategy with *Show flow* and *Edit as graph* instead (see
[The flow](#the-flow)). The inspector shows the same when the agent or nothing is selected.

### System One as a resource

If a `decide` node of the main graph or of a subgraph has `decider: system_one` (see
[chapter 4](04-flows.md)), the overview shows one resource, **System One (RKEV)**, in its own colour
(pink), connected to the agent's **Resources** port – an external service, like an MCP server. It is
derived from the flow and is not a section of the spec, so it cannot be disconnected, parked or
deleted: it has no such actions, the hover says why, and it disappears when no `decide` node uses
System One any more. Its inspector lists the `decide` nodes that use it (a click opens the flow and
selects that node) and names the variables `RKEV_BASE_URL` (required) and `RKEV_API_KEY` (optional),
each shown as *set* or *not set* in the environment of the studio process – never the value.

### Ports

The agent node has one named port for each group of resources, on its left edge:

| Port | Takes |
|---|---|
| Tools | tools, tool files, the tool folder (`tools_dir`), built-in tool groups, tool packages (`tool_packages`), self-built tools |
| Agents | `agent_tools`: other agents, local specs or remote A2A agents |
| Resources | MCP servers (`mcp_servers`), `knowledge`, `long_term_memory` |
| Skills | `skills` |

Above them, **Input** shows the type of the spec's `input`; on the right, **Output** the type of
its `output`. Every resource has one output, **Agent**. Each port group has one colour for its socket and the
connections (tools green, agents orange, resources purple, skills rose); a resource node keeps the
colour of its kind (a tool package is lime, knowledge cyan, memory amber). A resource connects only
to the port of its group: dropping a skill on *Resources* connects nothing, and the studio says
which port it needs ("A skill does not connect to Resources: its port is Skills."). While you drag a connection, the port it fits is
highlighted. Decisions (`decide` nodes) are part of the flow and have no node on the overview.

### Connecting, disconnecting, deleting

The connections **are** the spec: what is connected to the agent is in the spec file.

- **Connect** a resource by dragging from its *Agent* output to its port of the agent (or click
  **Connect** in the inspector). It is written into the spec at its place – a tool into `tools`, an
  MCP server under its name into `mcp_servers` (`server_2` if the name is taken), and so on.
- **Disconnect** by clicking the connection and pressing Delete, with the context menu of the
  connection or the resource, or with **Disconnect** in the inspector. The resource leaves the spec
  but stays on the canvas, dashed and grey, marked *not connected*. Its configuration is kept
  (*parked*) in the layout file, so connecting it again writes back exactly what it was. A parked
  resource is not validated: it is not part of the spec. Select it to edit its configuration.
- **Delete** a resource (Delete on the selected node, the context menu, **Delete** in the inspector)
  to remove it from the spec and the canvas.

The palette of the overview places a new tool, MCP server, agent as tool, tool package, knowledge,
long-term memory, skills or built-in tool **unconnected** where you can see it – next to the agent
when that place is visible, else in a free spot of the visible canvas (or where you drop it) –,
with a template to fill in; connecting it writes it into the spec. A spec has only one `knowledge`,
`long_term_memory` and `tools_dir`: connecting a second one is refused until the first is
disconnected or deleted.

### Agents of the workspace

Below the resources the palette lists the other specs of the workspace. Drag one onto the overview
(or click it): it becomes an agent node, not connected yet, showing the referenced agent's name and
model. Connected to **Agents**, it becomes an entry of `agent_tools` that runs that spec as a
sub-agent – `{ "spec": "13-reviewer.json", "description": "…" }`, the path relative to the open
spec, as in [chapter 8](08-external-tools-and-agents.md); edit the description the model reads in
the inspector. Existing `agent_tools` entries with a local `spec` show the name and model of the
spec they run, so the overview shows who calls whom; remote A2A agents (`url`) keep a node kind of
their own (*a2a agent*).

### The flow

The **flow** is the second level: the tab *Flow* above the canvas, or a double click on the agent
node. It exists only for a spec with a `graph` of its own, which is shown there, ready to edit
(next sections). Each port of a flow node shows the type that passes there (`string`, `message`,
`tool_call`, `tool_results`, or a type of the spec; `start` the spec's input type, `finish` its
output type), and while you drag a connection the inputs it fits – the same type, or a type a
condition turns it into – are highlighted.

A spec with a `strategy` (`single_run`, `react`, `chat`, `verified`) has no *Flow* tab, and a double
click on the agent does not open a flow. The agent node (badge **Agent**) and the inspector show the strategy ("Strategy: single_run") and two actions:

- **Show flow** opens a dialog with the flow grauwolf builds from the strategy, read only: the
  studio asks grauwolf for it, so it is exactly what runs. The dialog says in a sentence what the
  strategy does and has the **Edit as graph** button too.
- **Edit as graph** writes that flow into the spec as an ordinary `graph` (with the types the
  strategy adds, such as `Verification`) – the spec then behaves the same and can be edited node by
  node. The spec is marked unsaved, and the *Flow* tab appears. All four strategies convert.

### Planners in the flow

The actions of a planner node (`goap`, `plan`, [chapter 6](06-subgraphs-and-planners.md)) are no
edges of the spec – the planner chooses them while it runs. The flow shows them anyway:

- **Dashed links** go from the planner node to the subgraph each action runs (or to a dashed box
  `agent <name>` for an action with `agent`), labelled with the action names. Dashed means "may
  run", not "always runs". Several actions with the same subgraph share one link with a label per
  action (`06-goap`: `write_rough_draft` and `revise` both run `write`). The links are drawing only:
  they cannot be selected, deleted or saved. The subgraphs a planner of the main graph runs stand
  side by side in a row below it, so the links read from top to bottom.
- **The planner node** shows the start state (`state: draft=none, facts_checked=false`) and the
  goals; the full text is in the hover.
- **The expected plan (GOAP only).** The studio asks grauwolf's own GOAP planner which plan it makes
  for the start state and numbers the actions in that order: ① write_rough_draft, ② revise,
  ③ check_facts. Without a valid plan (no goal reachable, or invalid parameters) the node shows
  "⚠ no valid plan" and counts it as a warning; the hover says why. The node input is not known
  before a run, so an `input_fact` starts empty. An LLM planner (`plan`) has no expected plan: its
  one link to the step subgraph has no number.
- **Hovering an action** shows its preconditions, effects and cost (`pre: draft=final`, `effects:
  facts_checked=true`, `cost: 3`) and its description.
- **The inspector** of a planner node lists the actions in a table – number in the expected plan,
  name, what it runs, `pre`, `effects`, cost – with the expected plan or the reason there is none.

**In a run** the actions light up in the order the planner actually chose them: the link of the
action that runs glows, and each action that ran gets a ✓ with its run number (`✓1`, `✓2` …).
Replanned steps keep being numbered in run order; when the planner runs the same action again right
away (a retry after a failure), the repeat keeps its number – ①②②③ if `revise` ran twice. The nodes
inside an action subgraph report no events of their own, so the subgraph gets its `start` and
`finish` marks, not marks on its inner nodes.

## Harnesses and their projects

A **harness** says how an agent and its local sub-agents are shipped as one generated, deployable
project – a file `<name>.harness.json` beside the specs ([chapter 11](11-code-generation.md#a-project-per-harness-grauwolf-harness-generate),
[ADR-0128](../adr/0128-harness.md)). There are two places to work with them: the **overview** of a
spec that is the root of a harness, and the **Workspace** tab; the **Harness** menu of the menu bar
lists them everywhere.

- On the **overview** of such a spec, a blue dashed frame titled *harness <name>* goes around the
  agent and everything that is built into its project – tools, MCP servers, knowledge, local
  sub-agents; remote (A2A) agents stay outside. The frame shows where the last publication stands
  (see below). **Click the title** (or the badge) to select the harness: the inspector then shows
  it – name, root spec (relative to the harness file), and what `grauwolf-k8-ops agent deploy
  --harness` reads – image, port and the environment (values that are not secret). Empty Image and
  Port fields show the default: 8080, and `<registry>/<name>:<version>` once `grauwolf.toml` has
  `[harness] registry = "…"`. **Save** writes the file; a harness the format refuses is not
  written, and the message says which field is wrong. Click the canvas background to select the
  spec again.
- The inspector of the spec has a **Harness** section: its harnesses (click one to select it, the
  publish state is next to it), or **New harness** with this spec as the root, if it has none.
- **Generate project** (in the harness's inspector) writes the saved harness's project into
  `build/<name>/` of the workspace – the service crate with its `Dockerfile` and `app/`, as
  `grauwolf harness generate` does. A project is output only: generating again replaces the
  folder. The file tree shows `build/`; the spec list leaves it out.
- The **Harness** menu of the [menu bar](#the-menu-bar) (the command *Show harnesses* opens the
  Workspace tab) lists every harness of the workspace with its publish state. A harness file that
  is not valid, or whose root spec does not exist, shows the reason in red. A click selects the
  harness for the inspector: on the overview it opens the harness's root spec with the harness
  selected, on the Workspace tab it selects its frame. The menu has the workspace-wide entries
  **New harness…** and **Generate all** (command *Generate all harness projects*: every valid
  harness; one line per harness in the menu says where its project is, with warnings, or why it
  failed). **GitHub…** and **Kubernetes…** are in the **⚙ Settings** menu of the header.

### The workspace canvas

The **Workspace** tab (the command *Show workspace*)
shows the whole workspace on one canvas. A workspace that has harnesses opens on it. Each harness
is a blue frame around the overview of its root spec – the agent, its resources and its local
sub-agents, drawn and placed exactly as on that spec's overview, with the publish badge next to
the title. A spec that is the root of no harness and no local sub-agent of another spec stands
outside the frames with its agent only, marked *· no project*; its frame has **New harness**.

- **Click a node** to select it in its spec: the inspector edits it as on the spec's own tab (the
  spec opens in the background as a tab of its own, and Undo and Redo act on the spec you last
  selected or changed here). **Click a frame's title** to select its harness.
- **Double-click an agent** to open its spec – on the flow, or on the overview for a strategy.
- **Drag a node** to move it: the position is saved in that spec's layout file, so the spec's own
  overview shows it there too. **Drag a frame by its title** to move the whole frame; the place is
  saved in `.grauwolf-studio/workspace.layout.json` (the shift of the frame's overview, by harness
  file, or `spec:<path>` for a spec without a harness). Frames without a place are laid out in a
  row.
- Changes of a spec – yours, a coding agent's, from the disk – show on the canvas at once.

You edit the agents of the frames here as on the overview of their spec; every edit is one undo
step of the spec it changes (Undo and Redo follow the spec you last touched):

- **Connect**: drag from a resource's *Agent* output to the matching port of the agent in the same
  frame; the resource is written into the spec. Only fitting ports light up, a socket that does not
  fit is refused with a message, and so is a connection between nodes of two different specs
  (agent-to-agent edges between harnesses come later).
- **Add**: pick an entry of the [menu bar](#the-menu-bar) or click a resource in the **Palette** to add it, unconnected, next to the agent of the
  spec you last selected (click a node or a frame first), or drag it into a frame to drop it there.
  Dragging a spec of the workspace from the palette into a frame adds it as an agent tool (local
  sub-agent) of that frame's spec.
- **Park and delete**: right-click a resource for *Disconnect* (it stays on the canvas, dashed),
  *Connect* or *Delete*, a connection for *Disconnect*; **Delete** removes the selected resource.

**New harness** (the Harness menu, the Harness section of a spec's inspector, command *New harness…*) asks
for the root spec – a spec of the workspace – and a name, suggested from the spec's name as a DNS
label, and writes `harnesses/<name>.harness.json` with `{ "version": 1, "name": …, "spec":
"../<spec>" }`; the new harness is opened at its root spec and selected. A name that is no DNS
label is refused, and an existing harness file is never replaced. A harness file written by hand in
the explorer shows in the Harness menu the next time the workspace is loaded.

**Building the image in a cluster** ([ADR-0129](../adr/0129-building-harness-images-in-the-cluster.md)).
**Kubernetes…** in the **⚙ Settings** menu of the header (or the command *Kubernetes settings…*)
opens a dialog for what a build needs; the studio keeps it on this machine with mode 0600 and never
shows a kubeconfig or token again:

- **Kubeconfig:** paste one and save; afterwards only *kubeconfig set* and a **Context** select
  show. **Check** connects and reports the server, the Kubernetes version and either *ready to
  build* or the rights that are missing (or why the cluster cannot be reached).
- **Registry:** server, user and token of the registry the image is pushed to. The token field is
  empty after saving and the dialog says *token set*; **Remove** deletes it.
- A line says whether the **GitHub token** is set; Kaniko clones private repositories with it. The token itself is set in the GitHub dialog (below), with the project repository.

**The project repository** ([ADR-0130](../adr/0130-project-repository.md)). A workspace has one
project repository on GitHub, and every harness has one folder in it (`<name>/`, the project of its
agent tree); `build/` of the workspace is a clone of it. Every save of a spec or harness file that
belongs to a harness generates its project again, commits its folder and pushes it, in the
background – a save never waits for it and never fails because of it. Open **GitHub**
(**GitHub…** in the **⚙ Settings** menu, or the command *GitHub…*); it holds the token and the repository:

- **Token:** just the token (no user name), used by the whole studio – pushing the harness projects to the project
  repository and Kaniko clones. It needs write access to the repository's contents, and
  Administration write for **Create repository** with a fine-grained token, or the `repo` scope of a
  classic token. When you set it, the studio asks GitHub whose token it is (a token GitHub refuses is not stored); the field is empty afterwards and the dialog says *token of <user> (<name>)*, and your commits are made under that name; **Remove** deletes
  it. Without a token, the dialog says so where it is needed and points to this section.
- **Project repository:** enter the repository as `owner/name` or paste its GitHub URL (`https://github.com/owner/name`,
  `git@github.com:owner/name.git`, …; only GitHub is supported for now), enter the branch (`main` by
  default) and **Save**. The repository is kept as `owner/name` in the workspace's settings; there
  is no secret in it.
- **Create repository** makes the repository on GitHub, private, with that token; if it exists
  already, nothing happens. **Set up** clones it into `build/` of the workspace. Files that were
  already in `build/` are adopted and committed with the first push (the dialog says *adopted
  existing build/ files*). If the workspace is itself a git checkout, `/build/` is added to its
  `.gitignore` (*added /build/ to the workspace's .gitignore*), so the generated projects are
  versioned in the project repository only.
- **Remove** (asks first) forgets the setting; nothing is pushed any more, and the repository and
  `build/` stay as they are.

Each harness shows where its last publication stands – on its frame on the overview, in the Harness menu, in the
**Project** section of its inspector and next to it in the Harness section of the spec inspector:
*queued*, *publishing…*, *published* with the short commit (a link to it on GitHub), *unchanged*
(the project did not change, so there is no commit), *failed* (the reason on hover and in the
inspector) or *not set up*. The states follow live. **Build & push** then builds from the project
repository: Kaniko clones it at the pushed commit and builds the folder of the harness.

**Build & push** (inspector, or the command *Build & push harness image* for the selected harness)
builds the saved, valid harness: Kaniko in the namespace `grauwolf-builds` clones the repository at
the commit of the project and pushes the image named by the harness (its `image`, else the default
from `[harness] registry`). The project folder `build/<name>/` must therefore be generated, committed
and pushed – otherwise the build is refused as *not buildable* with the reason (uncommitted changes,
not pushed, no git repository, a remote a cluster cannot clone). Other refusals: no image to push,
no kubeconfig (with a link to the dialog), and a build of the same harness that still runs. The
inspector shows the source (repository, branch, short commit, folder), the image and the Job, then
Kaniko's log live, and at the end *succeeded*, *failed*, *timed out* or *error* with the reason.

## The screen

The studio is laid out like a code editor (ADR-0126 decision 16):

- **Header:** the workspace switcher (the name of the open workspace; the menu lists the recently
  opened ones and *All workspaces…*), **Commands** (the command palette), **Validate**, **Save**,
  **Diagram**, **Run** (shows the RUN panel and puts the cursor into its input), **Agent** (connect a
  coding agent, see [below](#drawing-together-with-a-coding-agent)) and **⚙ Settings** with
  *GitHub…*, *Kubernetes…* and *API keys…* (each opens its dialog).
- **Menu bar** over the canvas, see [below](#the-menu-bar).
- **Activity bar** at the left edge with three views for the side bar next to it:
  - **Explorer** – the workspace folder as a file tree (see *The files of the workspace*). Specs are
    marked with a flow icon and their name; a click opens a spec on the canvas and any other file
    in the editor.
  - **Palette** – on the overview the resources and the other specs of the workspace, on the flow the
    node kinds. These come from grauwolf's node catalog, so the palette always shows exactly the kinds
    your version of grauwolf knows, each with a description and its types (`string → message`); below
    the catalog kinds are the kinds that need the rest of the spec: `subgraph`, `plan`, `goap`,
    `parallel`, `for_each`, `code`.
  - **Search** – file names, spec names and node ids of the whole workspace. A file opens in the
    editor, a spec on the canvas, a node on the flow of its spec, selected.

  A click on the active view collapses the side bar; another click brings it back.
### The menu bar

Over the canvas of the Workspace tab and of a spec's overview, a menu bar (as in the AgentModler)
adds what the palette adds, without dragging; a click on an entry does what a click on the
palette does (on the Workspace tab: in the spec you last selected – the entries say *Select a spec
first* until you have):

- **+ Agent** – *New spec…* (a dialog for the name – a path in the workspace such as
  `agents/support`, `.json` optional; it refuses a name whose file already exists), *A2A agent*, and
  every other spec of the workspace as a local sub-agent.
- **+ Tools** – *Tool*, *Tool file*, every built-in tool group (all its tools) and *Tool package*.
- **+ Resources** – *MCP server*, *Knowledge (RAG)*, *Long-term memory* and *Skills*.
- **Harness** – *New harness…*, *Generate all* with the result lines of the last run, and every
  harness with its publish state; a click selects the harness for the inspector.

Over the flow the bar has **+ LLM** (LLM requests and model nodes: `llm_*`, `set_model`,
`compress_history`), **+ Tools** (`execute_tool`, `execute_multiple_tools`) and **+ Flow** (all other
kinds of the catalog and the structural ones), which add a node in the middle of the view. The
**Palette** stays in the side bar for dragging a resource or node to a place.

- **Editor area with tabs.** Every spec and file you open gets a tab, so several can be open at once
  – for example `08-coordinator.json` and `add.rs`. A tab with unsaved changes is marked **●**; the
  **×** closes it and asks first if it has unsaved changes. A spec keeps its state in its tab
  (unsaved edits, the view, the selection) while another tab is shown. Each spec tab has the switch
  **Overview / Flow / JSON**:
  - *Overview* and *Flow* are the two levels of the canvas (see above). *Flow* is there only for a
    spec with a graph of its own; a spec with a strategy keeps *Show flow* and *Edit as graph*.
    In the flow `start` and `finish` are always there; subgraphs appear as framed groups below the
    main flow. Drag the background to pan, use the mouse wheel to zoom. **Tidy up**, left of the
    switch, lays out the view anew (see [Layout and Tidy up](#layout-and-tidy-up)).
  - *JSON* is the spec source in Monaco, in sync with the canvas: an edit appears on the canvas a
    moment after you stop typing, and a change on the canvas or in the inspector appears in the text.
    Text that is not valid JSON is marked in the editor and listed in PROBLEMS; the canvas keeps the
    last valid spec until the text parses again. Ctrl+S (Cmd+S) saves.
- **Inspector** on the right, with three tabs for what you selected (with nothing selected, the spec):
  - **Properties** – the form: name, model, system prompt, input and output type and the whole spec
    as JSON for everything the canvas does not draw; for a node its ID and parameters, for an edge its
    condition, for a resource its part of the spec.
  - **Files** – the files the selection refers to, with **Open** or **Create from template**.
  - **JSON** – the selection's JSON, read only, with *Edit in the JSON view*.
- **Bottom panel** with four tabs, each with a count:
  - **RUN** – the run panel: input, transcript, approvals, usage, budget, Stop and chat, and the
    history of the spec's runs (next section). RUN, EVENTS and TOOLS always show the run of the
    spec whose tab is active.
  - **EVENTS** – one line per event of the run: the input (`›`), node starts (`▸ draft`) and
    finishes with their time (`✓ draft 1.1 s`), plan steps (`◆ planner → revise`), tool calls with
    result and time (`⇢ translator ✓ 1.1 s`), the model of each request, approvals, the output and
    the end of the turn and the run. A line of a node selects it on the flow. A change a coding agent
    made to the spec gets a line too (`✎ claude-code · add_node ask (llm_request)`).
  - **PROBLEMS** – the validation of the shown spec. A click on a problem selects its node or edge
    on the flow, or opens the JSON view at the problem's place.
  - **TOOLS** – the tool calls of the run, folded; unfolded they show arguments and result.

  Drag the panel's top edge to resize it (the height is remembered in the browser); the chevron at
  the right – or a click on the active tab – collapses it to its tab row.
- **Status bar** at the bottom: *✓ valid* or the number of problems (a click opens PROBLEMS), the
  workspace's tool policy (`grauwolf.toml`) and lockfile (`grauwolf.lock`), whether the model backend
  of the spec's provider is reachable (for example *ollama reachable*, asked through the model
  discovery when the provider changes and then kept for a minute – a click asks again), the run of
  the shown spec and, while runs are going on, a list such as *2 running: 04-decide, 07-chat* – a
  click on a name switches to that spec's tab; while a coding agent is connected,
  *MCP: connected (claude-code)*.

### Command palette

**Ctrl+K** (⌘K on a Mac) or **Commands** in the header opens the command palette over the canvas.
Type a few letters – the list is filtered fuzzily, so `add mcp` finds *Add MCP server to Resources* –,
choose with ↑ and ↓, run with ↵. Commands with *…* show a second list to choose from (*Go to node…*
lists the nodes of the view you see, *Open file…* the files of the workspace); Backspace in an empty
second list goes back. Esc closes the palette; while it is open, the keyboard stays in it.

| Command | Shortcut | What it does |
|---|---|---|
| Add *resource* to *port* (e.g. *Add MCP server to Resources*) | | parks a new resource from its template on the overview, visible and selected |
| List tools of MCP server… | | selects a connected MCP server and lists its tools in the inspector |
| Go to node… | Ctrl+P / ⌘P | scrolls to a node of the overview or the flow and selects it |
| Run with last input | Ctrl+↵ / ⌘↵ | runs the shown spec again with the input of the last run (saving first) |
| Undo | Ctrl+Z / ⌘Z | takes back your last change of the shown spec (see [Undo and redo](#undo-and-redo)) |
| Redo | Ctrl+Shift+Z / ⇧⌘Z, Ctrl+Y | does it again |
| Tidy up | Shift+Alt+F / ⇧⌥F | lays out the shown flow or overview anew (see [Layout and Tidy up](#layout-and-tidy-up)) |
| Save | Ctrl+S / ⌘S | saves the shown spec or file |
| Validate | | validates the spec |
| Switch view: Overview / Flow / JSON | | the switch of the spec tab |
| Open file… / Open workspace… | | opens a file of the workspace / switches the workspace |
| Close the tab | | closes the shown tab (asking if it is unsaved) |
| Toggle the bottom panel, Show EVENTS / PROBLEMS / TOOLS | | the bottom panel |
| Toggle the side bar, Show Explorer / Palette / Search | | the side bar |
| Toggle the inspector | | the inspector |

Ctrl+P and Ctrl+K also work while the cursor is in a text field; the other shortcuts belong to the
field there (Monaco saves with Ctrl+S itself).

## Running a spec

The **RUN** tab of the bottom panel runs the shown spec in the studio – the same interpreter as `grauwolf spec
run`, inside the studio process. Type an input and press **Run** (or Enter); *Run with last input*
(Ctrl+↵) repeats the last one:

- **The saved file runs.** With unsaved changes the button says **Save & run** and saves first.
- **The transcript** shows your input, the answer, the tool calls (folded: click to see the
  arguments and the result, cut to 4000 characters) and notes such as a fallback model or
  `say_to_user`. EVENTS lists the run event by event, TOOLS its tool calls. Text appears piece by piece where the spec streams (`llm_request_streaming`); a
  strategy such as `single_run` answers in one piece when it is done.
- **The canvas follows the run:** the node that runs has a glowing frame, nodes that ran get a
  small check mark – in the flow, matched by node id and subgraph; on the overview the agent glows
  while the run lasts. When a run starts and when it ends, the flow is fitted into the view if a node
  (for example `finish`) lies outside it; in between the view stays where you put it, so panning during
  a run is never undone. Only the nodes on the path taken are marked: after a `decide` the branch
  that was not chosen stays dark. The implicit `start` gets its check mark when the graph begins
  and `finish` when the run has its output; a subgraph's own `start` and `finish` are marked the
  same way, and its node in the main flow lights up while it runs.
- **Usage and cost** stand in the panel's header (tokens in and out; the cost when a price table is
  set with `GRAUWOLF_PRICES`).
- **Approvals.** A tool that needs an approval – `run_command` in shell mode with
  `approve_each_call`, building a tool with `self_tools.confirm` – asks in the transcript with
  **Approve** and **Refuse**; the run waits for your answer.
- **Stop** ends the run at once; its tools and MCP servers end with it.
- **Chat.** A spec with the strategy `chat` keeps its conversation: after an answer the panel waits
  for your next message (**Send**) until you stop it; the marks on the canvas are cleared when each
  turn starts, so every turn shows its own path. Other specs answer once; the next input starts a new
  run.
- `ask_user` is not available in studio runs yet; the agent is told to answer without asking.

The run uses the workspace as its working directory: relative paths of the spec resolve inside it as
always, tool code is built into `<workspace>/.grauwolf/tools`, the tool policy is the workspace's
`grauwolf.toml` and the lockfile the one beside the spec. The tools are exactly those of the spec –
`run_command` only if the spec enables it.

### Budget and limits of a run

Every turn of a run has a budget; the panel's **budget** shows and changes it for the next run:

| Limit | Default | Studio maximum |
|---|---|---|
| steps (node starts, subgraphs included) | 100 | 1000 |
| tokens (input and output of the agent; the spec's lower `limits.max_tokens` wins) | 200000 | 2000000 |
| wall time in seconds (loading and waiting for approvals included) | 300 | 3600 |

A turn beyond its budget ends with an error naming it. At most **two runs** run at the same time; a
third is refused with "at most 2 runs at the same time" until one ends or you stop it (a chat run
waiting for your message does not count).

### Runs belong to their spec

Every spec has its own runs. **RUN**, **EVENTS** and **TOOLS** show the run of the spec whose tab is
active – the one going on, else the last one. A spec that has not run yet shows *No run yet – type an
input below and press Run*; after running `04-tool-loop` and opening `01-hello`, the panel is empty
until `01-hello` runs. The runs are kept by workspace and path, so closing a tab and opening the
spec again finds them again (until the studio page is reloaded; then the history below is read from
the studio's list of recent runs).

- **Runs go on in the background.** Switching tabs does not stop a run or its events. A spec with a
  run going on has a blue dot on its tab, and the status bar lists the running specs (*2 running:
  04-decide, 07-chat*); a click on a name shows that tab and the RUN panel. The limit of two runs at
  the same time is unchanged.
- **Problems on the tab.** A spec with validation errors shows their number as a small red figure
  on its tab; PROBLEMS lists them for the active spec.
- **History.** Under the header of RUN, a collapsed strip *History (3)* shows the shown run
  (`#3 · completed · 1.5 s`); opened, it lists the last runs of the spec in at most four rows above
  the transcript, never over it, and selecting one collapses it again. The list is newest first, e.g. `#3 · completed · 2.1 s · 'Review this draft…'` (number, status, time, start of
  the input). Selecting one shows its transcript, and its events and tools in EVENTS and TOOLS,
  read only; *Show the latest run* (or the next run) goes back. The studio keeps the last 10 runs
  per spec in the browser; the studio server itself lists only its last 20 runs of the whole
  workspace, so older ones may have gone from there.
- **The canvas follows the run shown** for the active spec: node marks and planner steps are those of
  the selected run.

## API keys

**⚙ Settings > API keys…** in the header opens the **API keys**: for OpenAI, Anthropic, Gemini, Bedrock (an API
key, `AWS_BEARER_TOKEN_BEDROCK`) and the OpenAI-compatible providers it shows *set*, *not set* or
*from env*. **Set** stores a key, **Remove** deletes it (a key from the environment stays). Keys are
stored for you on this machine in `~/.config/grauwolf/studio/keys.json` (mode 0600, folder 0700;
`--config-dir <dir>` or `GRAUWOLF_STUDIO_CONFIG_DIR` for another folder) and never shown again – not
in the editor and not by the API. Runs and the model lists use them in front of the environment;
they never enter the environment of the studio process. Ollama needs no key.

## Models and MCP tools

In the inspector of the spec, **model.provider** is a list and **model.id** a text field with
suggestions: focusing it, or **List models**, asks the provider – Ollama for the models it has
installed (`/api/tags` at `OLLAMA_HOST`), OpenAI and the OpenAI-compatible providers, Anthropic and
Gemini with their key; without a key, and for Bedrock, a short built-in list. You can still type
any id. When the provider cannot be reached the inspector says so, e.g. "Ollama not reachable at
http://localhost:11434".

An MCP server on the overview has **List tools** in its inspector: the studio connects to the
server as the entry stands (a stdio server is started for this and ends again, 20 seconds at most)
and lists every tool it offers. Tick the ones the agent should use; they go into the entry's
`tools`.

## The files of the workspace

The folder of the open workspace is the studio's file system: everything a spec refers to – tool code
(`code_file`, `tool_files`, `tools_dir`), the code of `code` nodes, `on_code` conditions and
transformations, skills, knowledge documents – lies in it next to the specs, and the studio shows
and edits it there.

- **The tree** shows every file and folder of the workspace except hidden ones (`.git`,
  `.grauwolf`), `node_modules` and `target`, and without the layout files (`*.layout.json`), which
  belong to their spec. Icons tell specs, tool code, skills and knowledge apart; files a spec refers to get the role of the reference (a knowledge source and the
  documents in it, a skill folder).
- **New spec, new file, new folder** – the three buttons above the tree; the new entry goes into the
  selected folder (or beside the selected file). A name ending in `.json` under *New spec* gets a
  spec with `start` and `finish`. All three refuse a name that is already a
  file or folder (or that leaves the workspace) and show the reason under the input, so an existing
  file is never replaced.
- **Rename** and **Delete** appear when you point at an entry. A spec's layout file is renamed and
  deleted with it; a folder is deleted with everything in it after you confirm.
- **The editor** is Monaco, with syntax highlighting for Rust, Markdown and JSON (and plain text for
  the rest). Each file opens in a tab of its own beside the specs. Ctrl+S (Cmd+S) saves; ● in the tab and "unsaved" mark changes not saved yet. After a
  save the studio validates the open spec again, so a fixed tool file clears its problem at once.
- **From the inspector's Files tab:** a selected tool, tool file, skill, knowledge, a `code` node or an edge with
  code lists its files. **Open** opens a file in the editor (a folder in the tree). A file that does
  not exist is marked *missing* and offers **Create from template**; with nothing selected, the
  tab lists all files of the spec.
- **From PROBLEMS:** a problem about a missing file (for example
  `/tools/0/code_file: …/add.rs: No such file or directory`) offers the same **Create from
  template**.

The templates fit the place the file is for:

| Reference | Template |
|---|---|
| `code_file` of a tool (also in a tool file) | the body of `fn tool(args)`: one variable per declared argument, read with the right accessor (`as_str`, `as_f64`, `as_i64`, `as_bool`; required arguments with an error if missing), and an answer that shows them |
| `code_file` of a `code` node | the body of `fn run(input: In) -> Result<Out, String>` that returns a value of the output type |
| `code_file` of `on_code` | the body of `fn check(value: &In) -> bool`, returning `true` |
| `code_file` of a `transform` | the body of `fn transform(value: In) -> Out` |
| a skill folder (`skills[].dir`) | `<folder>/<name>/SKILL.md` with `name` and `description` (the first of `names`, else `new-skill`) |
| a knowledge source | a Markdown document (the source itself, or `notes.md` in a source folder) |

Each template validates and builds as it is, so the spec is valid right after; then replace the
`TODO` with what the code should do.

## Drawing a flow

1. Drag `llm_request` from the palette onto the canvas (or click it).
2. Drag from the output of `start` (the dot on its right edge) to the input of the new node (the
   dot on its left edge), then from its output to `finish`.
3. The edge to `finish` gets the condition `on_assistant_message` by itself: the studio picks the
   condition that turns the source's type (`message`) into the target's (`string`). An edge to
   `execute_tool` gets `on_tool_call`, one to `execute_multiple_tools` `on_multiple_tool_calls`,
   edges leaving a `decide` node `on_decision`.
4. Select a node to change its ID or its parameters (as JSON; the parameter schema is shown below).
   Select an edge to change its condition, the condition's parameters or a transformation.
5. To delete an edge, click it and press Delete, or right-click it and choose *Delete edge*; nodes
   the same way.
6. Save with **Save** or Ctrl+S (Cmd+S).

The result is the same spec you would write by hand, for example the tool loop of
[chapter 4](04-flows.md).

## Validation at the node

The studio validates the spec after every change, with the same rules as `grauwolf spec validate`.
A node with an error gets a red border and a count, a node with a warning a yellow one, an edge
with an error turns red. PROBLEMS in the bottom panel shows every problem with its JSON pointer; a
click selects the node or edge on the flow, or opens the JSON view at the pointer; the status bar
counts them. On the overview a problem marks the resource it belongs to (an MCP server, a tool …) or, for the model, the prompt or the strategy, the agent node.

**Diagram** shows the flow as Mermaid text, as `grauwolf spec graph` prints it, to copy into a
document.

## Drawing together with a coding agent

The studio is also an MCP server: a coding agent – Claude Code, or any other MCP client – reads and
changes the specs of the open workspace, and you see every change on the canvas the moment it is
made. You draw, the agent draws, nobody reloads anything.

### Connecting Claude Code

Click **Agent** in the header (or run *Connect a coding agent…* from the command palette). The
dialog shows

- the ready command for Claude Code, e.g.

  ```bash
  claude mcp add --transport http grauwolf-studio http://127.0.0.1:8090/mcp --header "Authorization: Bearer <token>"
  ```

  with **Copy** – run it in the project where you use Claude Code;
- the URL of the endpoint (`/mcp` on the studio's address) and the **token**, hidden until you
  click **Show**, with **Copy**;
- a note for other MCP clients: Streamable HTTP at that URL with the header
  `Authorization: Bearer <token>`.

The token is made anew at every start of the studio, and only the studio shows it; a request without
it gets `401`. Started with `--mcp-token <token>` (or `GRAUWOLF_STUDIO_MCP_TOKEN`), the studio keeps
that token, so you do not have to add the server again after a restart.

While an agent is connected, the status bar says **MCP: connected (claude-code)**.

### What the agent can do

The agent works on the workspace you have open in the browser, and on the spec of the active tab
unless it names another one. Its tools:

| Tool | What it does |
|---|---|
| `list_specs`, `read_spec` | the specs of the workspace; one spec as the studio has it now – unsaved changes included – with its layout and validation problems |
| `list_node_kinds`, `get_schema`, `validate` | the palette (node kinds, conditions, resource kinds), the spec format, the validation |
| `add_node`, `remove_node`, `connect`, `disconnect`, `set_params`, `set_field` | the flow: nodes and edges, the parameters of a node, any field of the spec by JSON pointer |
| `add_resource`, `remove_resource`, `move_node` | the overview: tools, MCP servers, agents, knowledge, memory, skills …, connected to their port of the agent; positions on the canvas |
| `create_spec`, `write_file` | a new spec; a text file of the workspace (tool code, `SKILL.md` …) |
| `save_spec` | write the spec to disk |
| `run_spec` | run the spec as **Run** does and wait for output, status and token usage |

When something does not fit, the agent hears why: an unknown node kind (with the known ones), two
ports whose types do not fit and no condition converts (pass `when`), a JSON pointer that leads
nowhere, a spec with a `strategy` that has no graph to add nodes to.

### Live, with both of you editing

A change of the agent appears at once: on the flow and the overview, in the JSON view and in the
inspector. What it touched lights up for a moment, and EVENTS gets a line per change, e.g.
`claude-code · add_node ask (llm_request)`; a click on a line selects the node. Your selection
stays where it was.

You can keep editing while the agent edits. Both work on the same **working copy** that the studio
keeps for every open spec: changes to different parts of the spec – you move a node, the agent sets
a prompt – both survive. If both of you change the same value, the later change wins and the studio
tells you; a change to something the other one deleted meanwhile is dropped, with a note.

A run the agent starts with `run_spec` appears in RUN and in the history of its spec like your own.

### Saving

Nothing the agent does is saved on its own: the tab shows ● and **Save** writes the spec – yours and
the agent's changes together. An agent you asked to save calls `save_spec`. `run_spec` runs the saved
file: with unsaved changes it refuses, unless the agent passes `save: true`.

Closing a tab with unsaved changes discards them, the agent's included. If the file changes on disk
while the spec has unsaved changes (a `git checkout`), the studio says so, and **Save** asks before it
overwrites the file; a spec without unsaved changes simply follows the file.

## Undo and redo

**Ctrl+Z** (⌘Z on a Mac) takes back your last change of the shown spec (on the Workspace tab, of the
spec you last selected or moved a node of), **Ctrl+Shift+Z** (⇧⌘Z) or
**Ctrl+Y** does it again. The arrows in the header do the same; their tooltip names the step, e.g.
*Undo Move node ask*. The command palette has *Undo* and *Redo* too.

| You did | One undo takes back |
|---|---|
| dragged a node or a resource | the whole drag (the node goes back where it was) |
| drew or deleted a connection | the connection |
| added a resource from the palette, connected or disconnected it, deleted it | that one step (undo a connection first, then the resource) |
| typed into a field of the inspector | the typing until you paused for about a second or left the field |
| edited in the JSON view | everything since you went into the editor, once you leave it |
| *Edit as graph*, renaming, adding or deleting a node | the whole action |

In a text field and in the JSON editor, Ctrl+Z is the field's own undo; the spec's undo applies
when the field does not have the cursor.

**Only your own changes.** Ctrl+Z never takes back what a coding agent (or another browser tab)
changed: its node stays. Your steps still fit after its changes – you move a node, the agent sets
the node's prompt, and undoing your move puts the node back while the prompt stays. If the agent
changed or deleted exactly what a step would change, the step cannot be applied: the studio says
*Could not undo 'Move node ask': changed by claude-code* and drops the step.

**Undo an agent's change from EVENTS.** Every line of a change the agent made has an **Undo** button
that takes back exactly that change, e.g. the node of `claude-code · add_node ask (llm_request)`.
The line then shows *undone*. If something changed it since, the line says so instead.

The steps are kept per spec, also across **Save**, up to 200. **Revert**, closing the tab and the spec
being loaded again from disk clear them. A new change of yours clears what you could redo.

## Key order

Saving keeps the order of the keys in the spec file: a spec you wrote with `name` first and `graph`
last is written back that way, and *Edit as graph* puts `graph` where `strategy` was.

## Layout and Tidy up

The studio lays out flows and the overview with the Eclipse Layout Kernel (ELK): the flow runs from
left to right in layers with as few crossings as it can, `finish` at the end; subgraphs are groups
below the main graph – the subgraphs a planner runs side by side in a row below it, the others one
below the other. **Edges are orthogonal** – straight stretches with rounded corners – and run around
the nodes, never under one; a condition label sits beside its edge, on its own background, where it
covers no node and no other label. The selected edge is brighter, an invalid one red; the planners'
links are dashed, as before.

This happens by itself only for nodes **without a saved position**: a spec without a layout file,
or a node you added in the JSON view or an agent added. A new node is placed next to the nodes it
is connected to, where it overlaps nothing; nodes with a position stay where they are. When you
drag a node, its edges are routed around the nodes as they now stand.

**Tidy up** lays out the whole view you see – the flow or the overview – anew: the button
*Tidy up* next to *Overview / Flow / JSON*, the command *Tidy up* in the command palette, or
**Shift+Alt+F** (⇧⌥F on a Mac). It is one step: **Ctrl+Z** (⌘Z) puts every node back where it was.
After it, and when a flow opens or a run starts, the view is fitted – but not below 60 % zoom, so
texts stay readable; a wider flow starts at its left end, and you scroll to see the rest.

## Where the positions are kept

The positions of the nodes are not part of the spec. The studio keeps them in a second file beside
it: `writer.json` gets `writer.layout.json`, with the positions and the view of the flow and of the
overview, and the configuration of parked (not connected) resources. If you delete that file you
lose only the positions and the parked resources. The flow shown for a strategy is laid out anew
each time; its positions are not saved.

A spec without a layout file – one you wrote by hand, one from `grauwolf spec create`, one Claude
Code wrote – is laid out automatically when you open it (see [Layout and Tidy up](#layout-and-tidy-up)).
If the layout file lacks some nodes (because you added them in the JSON), only those are placed,
next to their neighbours, where they overlap nothing; the others keep their positions. The next save
writes all positions. The routes of the edges are not saved: the studio works them out each time it
draws.

## Limits

- Sub-agents in `agent_tools` are not opened from the canvas; open their spec file from the tree
  (the inspector of the agent resource offers **Open** too).
- A workspace from a GitHub repository comes with part 2b.
- A harness is edited on the overview of its root spec; a sub-agent's overview does not say which
  harness it is part of. Calls between harnesses (A2A) come later.
- A node you dragged keeps its place even when it is in the way: its edges go around it, but
  only *Tidy up* moves the nodes themselves.
- Undo and redo are kept in the browser tab: a reload of the page starts with empty steps.
- Runs use the saved spec; `ask_user` does not work in them yet, and a spec in a subfolder uses the
  lockfile beside it, not the one at the top of the workspace.
- The type check of `agent` nodes against the sub-agent's spec and the check of local skill
  folders run only in `grauwolf spec validate` so far.

For the API, the root folder and registry, the layout file in detail and building the studio, see
`studio/README.md` (Enterprise, not in the public repository).
