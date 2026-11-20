# 10 – grauwolf studio

> **Enterprise** – grauwolf studio is not part of the open source distribution; its code is not in the public repository. The paths named in this chapter exist only in the private repository.

grauwolf studio ([ADR-0126](../adr/0126-grauwolf-studio.md)) is the visual editor for specs: a
Rust backend, the app crate `crates/apps/grauwolf-studio`, and a Next.js frontend in `studio/web/`
that started as a copy of the AgentWeaver frontend. How to build and run it, the routes and the
layout format are in `studio/README.md` (Enterprise, not in the public repository); this chapter is about the code.

## Workspaces

Workspaces are separate projects (ADR-0126 decision 11). The backend module `workspaces.rs` holds
them in one of two modes:

- **root** (`serve --root <dir>`, default `~/grauwolf-studio/workspaces`): own workspaces are the
  subfolders whose name is a slug and that hold `.grauwolf-studio/workspace.json`; their id is the
  folder name. Registered folders elsewhere are entries of `<root>/.registry.json`, with the same
  metadata – the studio never writes into a registered folder. The listing is read from disk on
  every request, so a folder copied into the root by hand appears at once.
- **single** (`serve --workspace <dir>`): the folder is the only workspace, id = `slug(folder
  name)`; everything that changes the set is `409 single_workspace`.

`Workspaces::open(id)` turns an id into the `Workspace` (`workspace.rs`, unchanged: path rules,
atomic writes) the specs and files handlers work on. The handlers get it through the extractor
`api::Ws`, which reads `{id}` from the route and answers `404 workspace_not_found`; `api::RelPath`
reads `{path}`. All specs and files routes are therefore `/workspaces/{id}/…`; `schema`,
`node-kinds` and `openapi.json` stay at the top. Changes (create, register, update, remove) are
serialised by a mutex; a new workspace is built in a hidden `.creating-…` folder of the root and
renamed into place, metadata and registry are written with `write_atomic`. Register refuses
overlaps (the root, anything inside or around it, any other workspace), which is what keeps two
workspaces from seeing each other's files. The template `examples` copies `--examples <dir>`, by
default `docs/user-guide/examples` of the repository the binary was built in
(`CARGO_MANIFEST_DIR`, checked at run time); an installed binary without the repository answers
`400 no_examples` and the frontend offers the template disabled (`examples: false` in
`GET /workspaces`).

In the frontend `app/page.tsx` shows `components/studio/StartPage.tsx` until a workspace is open, then
`EditorApp` keyed by the workspace id, so a switch mounts the editor anew.
`store/workspacesStore.ts` loads the list, keeps the open workspace in the URL (`?w=<id>`,
`history.pushState`; back and forward follow it), remembers the recently opened ids in
localStorage, asks about unsaved changes before a switch and resets the spec and file stores.
`lib/current-workspace.ts` holds the open id for every call below `/api/v1/workspaces/{id}/`
(`workspaceParam()`); only the workspaces store sets it. `components/studio/WorkspaceSwitcher.tsx`
sits at the top of the file tree and shows `tool_policy` and `lockfile` of the workspace.

`scripts/studio.sh` starts the backend on the root folder and then sets the workspaces up through
the API: once (marker `<root>/.grauwolf-studio-migrated`) it registers the old default folder
`~/grauwolf-studio-workspace` as "examples"; with no workspace at all it creates "examples" from the
template and marks it `.grauwolf-studio-seeded`; every folder with that marker gets the missing
example files (`cp -Rn`).

## The model: spec JSON

The canvas edits spec JSON directly (ADR-0126 Q2). There is no second graph model:

- every canvas node is one entry of `graph.nodes` or of a subgraph's `graph.nodes`, plus the
  implicit `start` and `finish` of each graph;
- every connection is one entry of `edges`, identified by its index;
- positions, the viewport and collapsed groups live in `<spec>.layout.json` (Q8 b), never in the
  spec.

The frontend keeps the spec as raw JSON (`SpecJson` in `studio/web/lib/spec-canvas.ts`), so fields
the canvas does not know survive every edit. The mapping is a set of pure functions with vitest
tests (`lib/spec-canvas.test.ts`): `canvasFromSpec`, `addNode`, `removeNode`, `renameNode`,
`connect`, `suggestCondition`, `groupsOf`, `problemsByNode`, … Canvas keys are
`<subgraph>/<id>` for nodes and `<subgraph>#<index>` for edges (empty subgraph = main graph);
IDs cannot contain `/`.

### Two levels

A spec has two views (`view` in `store/specStore.ts`, tabs in `components/studio/ViewBar.tsx`):

- **Overview** (`lib/spec-overview.ts`): the agent node (`agent`) and one node per resource, each
  with a key (`tool:<name>`, `tool_package:<i>`, `builtin:<group>`, `mcp:<name>`,
  `agent_tool:<name>`, `knowledge`, `memory`, `skill:<i>`, `self_tools` …)
  and the JSON pointer of its part of the spec. `decide` nodes have no node (decision 14); the one
  derived resource, `system_one` (`derivedNodes`, `SYSTEM_ONE_KEY`, `isDerived`), has no pointer and is
  not in `resourceNodes`, so `takeResource`, `removeResource`, `deleteResource` and `disconnectResource`
  never touch it (stale `decision:…` positions of earlier layouts are ignored by `completeOverview`). Built-in tools are grouped by the
  `builtin_groups` of `/node-kinds` (`grauwolf_spec::BUILTIN_GROUPS`), not by a copy of the table.
  Edits go through the pointer (`setAtPointer`, `writeResource`, `takeResource`, `removeResource`),
  so unknown fields survive. Positions are `layout.overview.nodes`, completed by `completeOverview`.
  A local sub-agent (`agent_tools.<name>.spec`) has the kind `agent_tool` and its `spec` path as
  `ref`; a remote one (`url`) the kind `a2a_agent`.
- **Flow**: the editable graph as described here – only for a spec with a `graph` of its own.
  `lib/views.ts` (`hasOwnGraph`, `viewsOf`, `strategyKind`) decides which tabs `ViewBar` offers,
  and `setView` ignores a view the spec does not have.

**Agent kind and flow summary (decision 14).** `lib/views.ts` derives them as pure functions, tested
in `lib/views.test.ts` over the shapes of the user guide examples: `agentKind` (`simple` with its
strategy for a spec with a strategy, `planner` for an own graph with a `plan` or `goap` node in the main
graph or any subgraph, else `graph`), `agentKindLabel` ("Agent", "Graph-Agent",
"Planner-Agent" – the badge of the agent node), `flowSummary`/`flowSummaryText` ("Graph: 5 nodes, 1 decision"; "1 decision", "2 subgraphs"
and "1 planner" only when the count is not zero; nodes of all graphs, not the implicit `start`/`finish`) and `systemOneDecides` (the
`decide` nodes with `decider: system_one`). `components/studio/AgentKindInfo.tsx` draws, for
a graph or planner agent, the summary as a button that opens the flow (`setView`), or `StrategyActions`
for an agent with a strategy; the agent node and the inspector both use it (the node shows the kind in its badge, `SpecNode`; the inspector as a line, `showKind`).

**System One (decision 14).** `derivedNodes` in `lib/spec-overview.ts` adds the node `system_one`
("System One (RKEV)", kind `system_one`, port `resources`, `derived: true`, `uses` = the decide nodes)
when `systemOneDecides` finds any, to `overviewOf` and `completeOverview` (so it gets a position and
an edge `overview#system_one`). `overviewDrawing` marks it `derived` with the hover `DERIVED_NOTE`; the
context menu, the Delete key (`removeResource` sets a message) and the inspector offer no disconnect or
delete. `Inspector.tsx`'s `SystemOneForm` lists the decide nodes (`openFlowNode` opens the flow and
selects one) and shows whether `RKEV_BASE_URL` and `RKEV_API_KEY` are set from
`GET /external/system-one` (`run_api::system_one_status`: `{ base_url, api_key }`, each `{ variable, set }`,
read from the studio's `Env`, never the value).

A spec with a `strategy` has no flow tab. `components/studio/StrategyActions.tsx` (in the agent
node and the inspector) shows the strategy with "Edit as graph" (`editAsGraph` replaces the spec
with `flow.converted`, then the flow tab appears) and "Show flow", which opens
`components/studio/FlowDialog.tsx`: the flow of `POST /specs/flow` drawn read only by the same
`flowDrawing`, `sync` and `fit` code as the flow view (a small Rete editor of its own, laid out
anew, never saved), with a note on the strategy and "Edit as graph". The store keeps `flow` only
for such a spec, and `flowDialog` for the dialog. The file tree leaves out `layout` files
(`visibleEntries`); the API still lists them. If Monaco cannot be loaded, `FileEditor` shows a
message instead of the editor (it fetches `/monaco/vs/loader.js` first).

### Ports, connecting and parking (decision 12)

The agent node has named input ports – `input` (shows the spec's `input` type), `tools`,
`agents`, `resources`, `skills` (`AGENT_PORTS`, four groups since part 1g) – and the output
`output` with the spec's `output` type; every resource node has one output `agent`. `portOf(kind)`
maps a resource kind to its group (tools, tool files, `tools_dir`, built-in groups, tool packages and
`self_tools` to `tools`; MCP servers, knowledge, memory and the derived System One to `resources`; local and A2A agents to `agents`). `lib/spec-drawing.ts` describes each port as `DrawPort { key, label,
socket, type? }`; an overview edge carries `fromPort`/`toPort`.

`lib/spec-rete.ts` has **one socket class per port group** (`ToolsSocket`,
`AgentsSocket`, `ResourcesSocket`, `SkillsSocket`, the agent's
`InputTypeSocket`/`OutputTypeSocket`, and `FlowSocket` for every node of a graph), all derived from
`PortSocket`, whose `isCompatibleWith` accepts only a socket of the same class. `makeConnection`
asks the two sockets and calls `refuse` (an error message) instead of `connect` when they do not
fit, so the connection plugin itself refuses a skill on Resources. `PORT_COLORS` in
`SpecNode.tsx` gives each group the colour of its socket, label and connections (a
`SpecReteConnection` carries its `group`); the resource nodes keep the colour of their kind
(`colorsOf`, `KIND_COLORS`, else the colour of the kind's port via `portOf`). A node is rebuilt when the keys or classes of its ports change
(`SpecReteNode.signature`); changed labels (a new type) are set in place (`relabel`).

Connecting and disconnecting are pure functions in `lib/spec-overview.ts`, used by the store:

- `takeResource` / `disconnectResource`: the resource out of the spec, its kind, its name (the key
  in `mcp_servers`/`agent_tools`) and its JSON parked in `layout.overview.parked` under a new key
  `parked:<old key>` (`~2` … if taken), at the old position; the positions of later entries of an
  index-keyed list (`tool_file:<i>`, `tool_package:<i>`, `skill:<i>`) move up.
- `writeResource` / `connectParked`: the parked JSON written back at its place – appended to a list,
  under a free name in a map (`server_2`), merged into `builtin_tools`; a single section that is there
  already (`knowledge`, `long_term_memory`, `tools_dir`, `self_tools`) is refused with a message.
- `parkResource`: a palette template (`resourceTemplate`) or a dropped spec (`agentReference`)
  parked next to the agent (`besideAgent`) or where it was dropped; `deleteResource` removes a
  parked or connected resource from spec and layout; `setParkedConfig` edits a parked one.
- `parkedNode` draws a parked resource by writing it into an empty spec and taking its node, so it
  looks like its connected self, with the line "not connected" and `parked: true` (dashed, grey).
  Parked resources are not in the spec, so the validation never sees them.

The canvas maps a dragged connection from a resource to the agent to `connectResource` (only a
parked one changes anything), a click on an overview connection to the selection `{ type: "link" }`
(Delete, the context menu or the inspector disconnect), and drops of `RESOURCE_DRAG_TYPE` and
`AGENT_DRAG_TYPE` on the overview to `addResource`/`addAgentReference`. For local sub-agents the
store loads the referenced specs (`refreshAgents`, after every validation, paths resolved with
`resolveReference` relative to the open spec) and `agentLine` gives the overview "runs <name> ·
<model>".

**Flow ports (P5):** `flowPortTypes` gives each flow node's ports their types – `start` the graph's
`input`, `finish` its `output`, `input`/`output` parameters (`agent`, `code`, `for_each`) and the
subgraph of `subgraph`/`parallel` before the signature from `/node-kinds`. While a connection is
dragged, the connection plugin's `connectionpick`/`connectiondrop` events call `pick`, and
`compatiblePorts` lights the ports a drop could end at: the same socket class, on a flow the same
graph and `portsFit` – equal types, a condition whose transitions turn one into the other (the logic
of `suggestCondition`), any edge leaving `decide`, or an unknown type.

### The flow of a strategy

`POST /specs/flow` (`flow.rs`) loads the spec with grauwolf's own loader (`check::load` →
`AgentSpec::from_json`, which calls `grauwolf_strategies::expand`), so the studio never copies the
templates. The answer has the main graph, `derived` and the types the strategy adds; `converted`
is the raw spec with `strategy` removed and that graph and those types inserted (absent optional
fields dropped). It loads to the same `AgentSpec` (with `strategy` cleared) – `tests/flow.rs`
checks this, and that it validates the same, for every strategy example of the user guide. All four
strategies (`single_run` with `parallel_tools` or `moderate`, `react`, `chat`, `verified`) have an
exact equivalent, so all convert.

### Layout with ELK (decision 19)

The flow and the overview are laid out by the Eclipse Layout Kernel, `elkjs` (pinned to 0.12.1; the
bundled build runs in the main thread and is loaded on first use with a dynamic `import`, so the
static export has it as a chunk of its own, no CDN). Three modules:

- `lib/elk-layout.ts` – the engine, knowing nothing about specs: it lays out *shapes* (nodes with
  their drawn size and fixed ports, edges with their label sizes, subgraphs, layout-only links) and
  returns positions and routes (`elkGraph` builds the ELK input, `fromElk` reads the result; both
  pure). `arrange(shapes, "flow" | "overview")` keeps the result in memory per shape of the graph.
- `lib/auto-layout.ts` – between drawing and engine: `shapesOf(drawing)`, `completeLayout`,
  `tidyLayout`, `drawingRoutes` and `withRoutes` (below).
- `lib/edge-routes.ts` – the fallback router, label places on a route, and the rounded path.

**Options.** Every pass is `elk.algorithm: layered`, `elk.direction: RIGHT`, `elk.edgeRouting:
ORTHOGONAL`, `LAYER_SWEEP` crossing minimisation, node placement `BRANDES_KOEPF` with `BALANCED`
alignment (it kept rows straighter than `NETWORK_SIMPLEX` on `04-decide`), model order for nodes and
edges, `elk.edgeLabels.placement: CENTER` with inline labels requested, 48 px between nodes, 64 px
between layers, 24 px between edges and nodes, 14 px between edges, coordinates relative to the
root (`elk.json.shapeCoords`/`edgeCoords: ROOT`). Labels are part of the layout, so ELK keeps them
off nodes and off each other (it places them beside their edge). Every node has
`elk.portConstraints: FIXED_POS`; `nodePorts` puts the ports where `SpecNode` draws them – inputs
left, outputs right, one 19 px row each at the bottom of the node; a planner has a port in the
middle of its bottom edge for its links. The sizes are the drawn ones: `SpecNode` gives a node at
least its layout height (`minHeight`) and keeps the ports at its bottom; `nodeHeight(lines)` sizes a
flow node or resource by its text lines (`NODE_SIZE` 200×92, `TERMINAL_SIZE` 140×60, one more line
17 px, the agent 300×260).

**Two passes for the flow.** `elk.hierarchyHandling: INCLUDE_CHILDREN` with the subgraphs as compound
nodes was tried first: one direction for all levels stretched the main line of `06-subgraph` to
2650 px or put the subgraphs left of it. So each graph – the main graph and every subgraph – is laid
out on its own, then the graphs are laid out as boxes (a subgraph's box with room for its frame:
`GROUP_PADDING` around, `GROUP_HEADER` above) top to bottom (`elk.direction: DOWN`): the planners'
links are edges of that pass, from a port under the planner to the top of the action subgraph's box,
so the action subgraphs of a planner stand in a row below it; the other subgraphs are chained by
hidden edges along their left edges, one below the other. The links get ELK's routes and label
places too (the size of the overlay's label box, `linkLabelSize`).

**Positions.** `completeLayout(spec, layout, context)` (async) gives every node of every graph and
of the overview a position. Saved positions never change. Without any saved one, the nodes go where
ELK puts them; otherwise each missing node (`placeMissing`) is moved like the saved node nearest to
it in ELK's arrangement and then down until it overlaps nothing – a node added in the JSON view or
by an MCP client lands beside its neighbours. The spec store awaits it when a spec is opened,
created or converted and on Save; after any other change a subscription (`completeShown`) completes
what is missing, one run at a time, outside the undo history (as before, the positions reach the
working copy with the next change of the layout). Until then a node without a position is not drawn
(`flowDrawing` leaves it out; with `all` it lists every node of every graph, collapsed subgraphs
too, which is what the layout lays out). Parked resources keep the placement of `completeOverview`.

**Tidy up.** `tidyLayout(spec, layout, view)` lays out the flow or the overview anew; the store's
`tidyUp` sets it with `asEdit({ label: "Tidy up" })`, one undo step (decision 18), and bumps
`fitRequest`, on which the canvas fits the view after the sync. The command `view.tidy-up`
(⇧⌥F / Shift+Alt+F; `matchesShortcut` also compares `event.code`, as ⌥ turns ⇧⌥F into "Ï" on a
Mac) and the button *Tidy up* next to the view switch call it.

**Routes.** Routes are not saved; `SpecCanvas.sync` gets them from `drawingRoutes(drawing)` on every
draw. It asks for ELK's arrangement of the drawing (from the memory when the graph's shape did not
change) and finds the offset most nodes share between drawn and arranged positions. An edge (or
link) keeps ELK's route, moved by that offset, when both ends stand where ELK put them and neither
its route nor its label meets a drawn node or another label. ELK cannot route around nodes at
positions it did not choose – `layered` places the nodes itself, and the `fixed` algorithm routes
nothing – so every other edge is routed by `routeAround` in `lib/edge-routes.ts`: straight out of the
port for 20 px, then over the grid of lines that keep 16 px from every node box (and from the frames
of other subgraphs), Dijkstra over grid points and directions with 40 px per bend, straight into the
port. Its label goes on a free stretch of the route (`placeLabel`: the middle of the longest
horizontal stretch first). `withRoutes` adds the routes to the drawing, grows each subgraph frame
around the routes and labels of its edges (ELK may run an edge outside the nodes' bounds) and
computes the drawing's `extent`. `SpecConnectionView` draws the route with 8 px rounded corners
(`roundedPath`), its ends moved to the sockets as Rete placed them (`snapEnds`, so the route stays
orthogonal while a node is dragged), the label at the route's label box; selection, hover, invalid
(red) and the group colours are as before. `plannerOverlay` takes the link routes the sync returned
and draws them dashed the same way.

**Fit.** `fit` fits the nodes and the drawing's `extent` (frames, routes, labels), centred and never
above zoom 1; when that needs less than `MIN_FIT_ZOOM` (0.6) it stays at 0.6 and starts at the left
(or top) end – the rest is a scroll away. It runs when a spec or view opens without a saved viewport,
after Tidy up, and when a run starts or a turn or the run ends with a node out of view.

### Planners in the flow (decision 15)

`POST /specs/planners` (`planners.rs`) reads every `goap` and `plan` node from the spec JSON as sent
(it need not load): the actions with their target (`subgraph` or `agent`), `pre`, `effects`, `cost`
(GOAP's default 1) and description, the start state and the goals. For GOAP it adds the
**expected plan** from `grauwolf_planner::goap::expected_plan(params)`, never a copy of the search:
the domain crate builds the same `GoapPlanner` as `fact_planner`, but with executors that never run
(`Executor::Planning`), and calls `GoapPlanner::plan(state)` – the search `build_plan` uses before
every step of a run – on the start state with an empty task. `Err` is the reason (`no_plan`): no goal
reachable within the limits, or invalid parameters. A `plan` node has one action named after its step
subgraph and no expected plan.

The frontend keeps the answer in the store (`planners`, fetched with the validation when the spec
has a planner node). `lib/planner-view.ts` (pure, tested) makes the view model: `plannerLines` (state,
goals, "⚠ no valid plan"; `flowDrawing` puts them on the node and counts a missing plan as a warning),
`plannerOverlay` – one `ActionLink` per planner and target with the actions as labels, their positions
in the expected plan, their hover text (`actionHover`, `factsText`) and their run marks, plus boxes for
sub-agent targets – and `nextPlanStep`/`closePlanSteps` for a run. `SpecCanvas.drawPlannerLinks` draws
them imperatively into a layer between the subgraph frames and the nodes (an SVG of dashed paths,
HTML labels with a hover): they are not Rete connections, so they cannot be selected, deleted or saved.
The layout reads the planners' targets from the spec alone (`plannerTargets`, before the backend
answered) and puts the action subgraphs in a row below their planner (see *Layout with ELK*).

In a run the backend's `plan_step` event (below) says which action runs. `applyRunEvent` keeps the
steps in `RunView.plan`: a new step closes the planner's open one, the run number goes up unless the
same action runs again right away (①②②③), and `node_finished` of the planner closes its last step. The
nodes inside an action report no events (the planner runs its action graphs without the run's
observer), so the action subgraph's `start` is marked when its step begins and `finish` when it ends.

### Rete

`lib/spec-drawing.ts` turns the store into a drawing – nodes with role (`flow`, `agent`,
`resource`), sockets, title, detail lines and problem markers, edges with label, subgraph frames,
and whether it is editable – as pure functions with tests. `lib/spec-rete.ts` sets up Rete.js
(area, connection, context menu and React plugins, the views of
`components/studio/SpecNode.tsx`). `components/studio/SpecCanvas.tsx` keeps Rete in line with the
drawing: a new `generation` (a spec opened or replaced, another view) redraws everything and
restores that view's viewport; an edit adds, removes and updates nodes, routes the edges and
redraws the connections when the edges changed, or updates the routes of the ones that stayed (a
drag).

The canvas never edits the spec through Rete's own events:

- the connection plugin runs the bidirectional flow with a `makeConnection` that calls the store's
  `connect` and draws nothing itself – the next sync draws the edge. Pressing an input socket
  therefore never picks up (and removes) an existing edge;
- edges and nodes are deleted through the store: Delete on the selection, the context menu, the
  inspector;
- a moved node updates the layout, a picked node or a clicked connection selects it; Rete events
  raised while a sync runs are ignored.

Syncs run one after another on a promise queue. In the first version they ran concurrently when a
spec was opened (the rebuild and the update effect both fired), and the connection events of one
sync reached the store while the other had reset the "syncing" flag – edges were added twice or
removed from the spec. The other bug that kept the canvas from working in a browser: the socket
view had negative margins, so the element `RefSocket` registers had zero width; the connection
plugin finds the socket under the pointer with `elementsFromPoint`, found none, and a drag moved
the node instead of starting a connection.

### Files

The workspace folder is the studio's file system (ADR-0126 decision 10). `store/fileStore.ts` holds
the tree (`GET /files`), the shown file (path, content, saved content) and the other open file tabs
(`stash`, by path), the main tab (`canvas` or `file`) and the references of the open spec (`POST /specs/references`; a subscription to the spec
store refreshes them shortly after every change of the spec). `components/studio/FileTree.tsx`
draws the tree (pure helpers in `lib/files.ts`: visible entries, the editor language, the
references below a pointer, the reference a problem is about), with inline inputs for new
spec/file/folder and rename. `components/studio/FileEditor.tsx` is Monaco, loaded only in the
browser (`next/dynamic`), with Ctrl/Cmd+S as an editor command; the global Ctrl+S saves the file
while the file tab is shown and the spec otherwise. Monaco is not fetched from a CDN:
`scripts/copy-monaco.mjs` copies `node_modules/monaco-editor/min/vs` to `public/monaco/vs` before
`dev` and `build` (ignored by Git), and `loader.config` points there. `FileReferences.tsx` shows the
references of the selection in the inspector's Files tab (by JSON pointer prefix: `/tools/0`,
`/graph/nodes/<i>`, `/graph/edges/<i>`, the resource pointer) with Open and Create from template;
the problems panel offers Create from template for a problem whose pointer is that of a missing
reference. Creating calls `POST /specs/template` and writes the result with `PUT /files`.

### Palette placement

A palette click places the resource inside the visible canvas
(`placeInView` in `lib/spec-overview.ts` with `SpecCanvas.visibleRect()`): next to the agent when that
spot is visible, else the first free spot in view; only when there is no room, next to the agent,
and the canvas scrolls there (`reveal`).

### End-to-end check

`studio/web/e2e/studio.e2e.cjs` drives the canvas in headless Chromium with real pointer events
(Playwright, not a dependency of the frontend): it starts `grauwolf-studio serve --root` on an empty
temporary root folder and first checks the start page – a workspace from each template (the
examples one is where the rest runs), a registered folder with nothing written into it, rename,
delete with the typed id and removing a registered folder, switching between two workspaces with
the question about unsaved changes and separate files and specs, `?w=` after a reload and an
unknown id. Then it checks the overview (agent and resource nodes and their connections), the named ports
(labels, socket classes, a skill refused on Resources, only the Resources port lit while dragging), a
palette resource placed unconnected and then connected, disconnecting a tool (out of the spec,
parked in the layout, not validated), reconnecting it with the same configuration after a reload,
`13-reviewer.json` dragged onto `13-coordinator.json` and connected to Agents (the `agent_tools`
entry validates), the flow's port labels and highlighted ports, the tabs of strategy
and graph specs, the dialog with the derived flow and Edit as graph, that layout files are not in the
tree, the message when Monaco is missing, the drawn edges with labels of graph and subgraph examples, dragging a
connection and saving, deleting edges, moving nodes and restoring positions, a palette drop, that
edge labels cover no node or other label and fit their background, that a saved spec keeps its key
order, following the `code_file` of `03-tool-files.json` to `03-tools/add.rs`, editing and saving
it in Monaco, creating a missing tool file from the template offered by the validation (the spec
then validates), and renaming and deleting in the tree. Before those, the checks of part 2a run
against a small Ollama stand-in the script starts itself (`OLLAMA_HOST` points to it; the keys go to a
temporary `--config-dir`): a run of `01-hello.json` with the agent glowing and then marked, a
streaming spec with the running node glowing in the flow and the answer in pieces, an approval
round trip with a shell command, Stop, the two-run limit (429 and the panel's message), a chat with
a second message, setting and removing a key that never appears in the page or the API, the models
of the stand-in in the inspector, "List tools" of a stdio MCP server written by the script, a
palette resource placed in view while the agent is out of sight, and the layout cases below. Real example specs run in the Flow tab, the page recording how the nodes light
up (a `MutationObserver` on `data-run`) and comparing it with the `node_started` events: `04-decide.json`
for a billing and a technical ticket (the stand-in answers the decision by the ticket text, and
`diagnose` does not light on the billing path), `04-tool-loop.json` (tool call and result in the
panel) and `06-subgraph.json` with the nodes keyed `<subgraph>/<id>` (its `assess` model swapped for the
stand-in's; unchanged, the run fails at once and names `ANTHROPIC_API_KEY`). `E2E_ONLY=part2a` stops after them. `E2E_ONLY=planner` creates the workspace through the API and runs
only the planner scenarios of decision 15: the dashed links of `06-goap.json` with ① ② ③ (and no extra
connection), the hover with preconditions, effects and cost, a run against the stand-in (the `check`
model swapped) that lights the links in the order write_rough_draft → revise → check_facts and ends with
three ✓, and the one link of `06-plan.json` without numbers (screenshots `planner-goap-flow`,
`planner-goap-run`, `planner-plan-flow`). `ideChecks` (decision 16; alone with `E2E_ONLY=ide`)
checks the frame: a spec and `add.rs` as two tabs, ● after an edit, the close question (Cancel keeps
the tab, OK discards), Overview / Flow / JSON with a JSON edit visible on the overview and invalid
JSON in PROBLEMS, the inspector tabs, PROBLEMS selecting a node on the flow, EVENTS and TOOLS during a
run of `04-tool-loop.json`, collapsing and resizing the panel, the status bar (valid, policy, lock,
*ollama reachable* from the stand-in, the run), the command palette (Ctrl+K, filter, *Add MCP
server to Resources*, *Go to node…*, Ctrl+P, Tab staying inside, Esc) and Search (screenshots
`ide-overview`, `ide-json`, `ide-flow-run`, `ide-palette`). `mcpChecks` (decision 17; alone with
`E2E_ONLY=mcp`) drives the studio with a small MCP client written in the script (`mcpClient`, plain
JSON-RPC over HTTP): the connect dialog with the token hidden and a command whose URL and token
work, `401` without or with a wrong token, the status bar's *MCP: connected (claude-code)*, a node
and an MCP server added by the client appearing on flow and overview without a reload, lit up, with
EVENTS lines, the user dragging a node while the client sets a field (both in the working copy, the
selection kept, the inspector and the JSON view following), Save writing the merged result, and a
`run_spec` whose run shows in RUN (screenshots `mcp-connect`, `mcp-live-edit`, also copied to
`PUBLISHED_SHOTS`). `undoChecks` (decision 18; alone with `E2E_ONLY=undo`) runs the owner-approved undo
scenarios: a drag undone with Ctrl+Z and redone with Ctrl+Shift+Z (the node's place on the screen,
the toolbar's tooltip), a drawn connection undone, a palette resource undone and – once connected –
its connection and then itself, typing into `system_prompt` undone in one step, a node of the MCP
client that Ctrl+Z leaves and its EVENTS **Undo** removes (the line shows *undone*), a move undone
while the client set the node's params (the params stay), and undo after Save (screenshot
`undo-events`, also copied to `PUBLISHED_SHOTS`). `layoutChecks` (decision 19; alone with
`E2E_ONLY=layout`) runs the owner-approved layout cases: copies of `04-tool-loop`, `04-decide`,
`06-subgraph`, `06-goap` (flow) and `08-coordinator` (overview) without a layout file are opened, and
the page's own geometry is checked – every edge and planner link path sampled against the node
boxes, every label against the nodes and the other labels, the zoom after the fit at least 0.6 –;
then a dragged node (the edges go around it), Tidy up with ⇧⌥F as one undo step, ⌘Z bringing the
dragged position back, and Tidy up on the overview (screenshots `layout-04-tool-loop`,
`layout-04-decide`, `layout-06-subgraph`, `layout-06-goap`, `layout-overview`). The other checks reach the explorer and the
palette of the side bar, the inspector's and the panel's tabs through helpers (`sidebar`,
`paletteClick`, `paletteDrag`, `inspectorTab`, `panelTab`, `viewTab`); `open` closes every tab
first, so each check starts from the file on disk. Screenshots go to `SHOTS`. Tool code is built
with `cargo`, so it must be on the `PATH` of the script.
Run it after `npm run build`, with Playwright installed somewhere else:

```bash
STUDIO_BIN=$CARGO_TARGET_DIR/debug/grauwolf-studio STUDIO_WEB=studio/web/out \
EXAMPLES=docs/user-guide/examples NODE_PATH=<dir>/node_modules node studio/web/e2e/studio.e2e.cjs
```

## The shell and the command registry (decision 16)

`app/EditorApp.tsx` composes the frame: `StudioToolbar` (the header with `WorkspaceSwitcher`),
`ActivityBar` and `SideBar` (Explorer = `FileTree`, Palette = `Palette`, Search = `SearchView`),
`EditorTabs` (the open files and the switch Overview / Flow / JSON), the canvas – with `SpecSource`
over it for the JSON view – or `FileEditor`, `Inspector` (tabs Properties, Files, JSON),
`BottomPanel` (RUN = `RunPanel`, which stays mounted, EVENTS, PROBLEMS = `ProblemsPanel`, TOOLS),
`StatusBar` and `CommandPalette`.

- **Frame state:** `store/layoutStore.ts` – the side view and whether the side bar is open, the
  panel's tab, height (remembered in localStorage) and whether it is open, the inspector's tab and the
  palette (open, and the picker command it opens at).
- **Tabs:** the spec store holds the shown spec and the other open specs in `stash` (a
  `SpecSnapshot`: spec, layout, dirty, problems, selection, view, JSON view …); `open(path)` stashes the
  shown one and restores a stashed one without reading the file again. The file store does the same
  for files. `store/tabsStore.ts` keeps the order and the active tab and follows both stores with
  `syncTabs` (`lib/tabs.ts`, pure, tested: open, close with the neighbour becoming active, the
  unsaved mark, rename keeping the place), so a spec or file opened from anywhere gets its tab;
  `close` asks with `window.confirm` when the tab is unsaved.
- **JSON view:** `source` in the spec store switches it on; `lib/spec-source.ts` (pure, tested) writes
  the text (`specText`), parses it with the place of an error (`parseSpecText`, `errorOffset`), keeps
  the user's text while it says the same or does not parse (`syncedText`) and finds the line of a JSON
  pointer (`pointerLine`) for PROBLEMS' "open the JSON at the pointer" (`revealSource`). An edit
  calls `replaceSpec` 300 ms after typing stops; invalid text sets `sourceError`, which PROBLEMS and
  the status bar count.
- **Selection:** `lib/selection.ts` gives the JSON pointer and the JSON of the selection for the
  inspector's Files and JSON tabs.
- **Events:** `applyRunEvent(view, event, now)` also folds the event lines of EVENTS
  (`RunView.events`, start times in `started`); a tool result completes the line of its call
  (`⇢ translator ✓ 1.1 s`).
- **Search:** `lib/search.ts` (`indexSpec`, `searchWorkspace`, pure, tested); `SearchView` reads each
  spec of the workspace once and uses the open ones as they stand.
- **Status bar:** the model backend is asked through `GET /discovery/models?provider=` of the shown
  spec's provider, once per provider change and cached for a minute – never polled.

**Command registry.** `lib/commands.ts` holds every command: `{ id, title, keywords?, shortcut?,
alternatives?, enabled?, run? | items? }` (`alternatives` are further shortcuts that are not shown,
such as Ctrl+Y for *Redo*; `matchesCommand` checks all of them). A command with `items` is a picker: the palette shows its entries as a
second step (*Go to node…*). `registerCommands` adds commands and returns the function that removes
them; `filterCommands`/`fuzzyFilter` rank by a fuzzy subsequence score (consecutive letters and word
starts count more); `matchesShortcut` reads `Mod+K` as Ctrl or ⌘. The studio's commands are in
`components/studio/studio-commands.ts`, registered when `EditorApp` mounts; the shell's one key
handler opens the palette on Ctrl/⌘+K and runs the shortcut of any registered command (in a text
field only pickers such as Ctrl+P). A new feature adds its commands with `registerCommands` – palette
entry and shortcut come with it:

```ts
useEffect(() => registerCommands([{ id: "trace.open", title: "Open the trace…", items: traceItems }]), []);
```

## The backend

`grauwolf-studio` is an app crate (the architecture check knows it as `arch::STUDIO`: it may use the
host and the server, and no app crate depends on it). Modules:

| Module | Content |
|---|---|
| `lib.rs` | `StudioConfig`, `router` (the API under `/api/v1`, the static export with an SPA fallback), `serve` |
| `api.rs` | the handlers and the OpenAPI description (`ApiDoc`, utoipa) |
| `workspace.rs` | one workspace folder: path checks, listing (specs and harness files, not `build/`), reading, atomic writes |
| `workspaces.rs` | the root folder and registry (or single mode): list, create from a template, register, rename, delete, `open(id)` |
| `ordered.rs` | `OrderedJson`, a JSON value that keeps the key order of a spec file |
| `files.rs` | the file API: tree with roles, read (text up to 1 MiB), write, rename, new folder, delete |
| `references.rs` | the files a spec refers to, from `grauwolf_agent_tree::file_references` |
| `templates.rs` | the template for a missing referenced file |
| `layout.rs` | the layout format, version 1 (with the optional `overview` and its `parked`) |
| `check.rs` | `load` and `validate` on a spec in memory; pointer → node, edge, subgraph |
| `flow.rs` | the flow of a spec and its conversion to a graph |
| `palette.rs` | `/node-kinds` from the catalogs |
| `error.rs` | `ApiError`, the one error shape |
| `runs.rs` | preview runs: start, events, budgets, approvals, chat turns, stop (part 2a) |
| `keys.rs` | the providers' API keys in `keys.json` (part 2a) |
| `discovery.rs` | model lists and the tools of an MCP server (part 2a) |
| `run_api.rs` | the routes of runs, keys and discovery |
| `ops.rs` | the ops of the working copy: apply with inverse, compose, transform (conflict rules), diff – pure |
| `documents.rs`, `document_api.rs` | the working copy: open documents, versions, batches, save, revert, the disk check, the event stream |
| `mcp.rs`, `mcp_tools.rs` | the MCP endpoint `/mcp` (token, sessions, context) and its tools |

Validation follows `grauwolf spec validate` without an agent tree: the file references are resolved
with `grauwolf_agent_tree::resolve_file_references` relative to the spec's folder, then
`AgentSpec::from_json`, `validate()`, the known providers (`grauwolf_host::providers::BUILTIN`) and
the warnings. The node of a pointer is looked up in the raw JSON, so it is found even when the spec
does not parse.

**Files and references.** The file API uses the path rules of the specs (relative, no `..`,
inside the workspace even through symbolic links) plus no hidden segment. The references of a spec
are not a list of fields in the studio: `grauwolf_agent_tree::file_references` walks the spec with
the same code as the reader's `resolve_file_references` (`walk_code_files` with a visitor: the
resolver inlines each file, the lister records it and puts empty code in its place), and – if the
spec then loads – adds `tools_dir`, local sub-agents and `grauwolf_generator::companions` (the
companion paths of an image, now with the JSON pointer and kind of each field). The studio adds the
workspace path, whether the file exists and the template that can create it. The templates
(`templates.rs`) build: `tests/templates.rs` creates every kind for one spec, validates it, and in a
slow test builds all tools and code pieces with `grauwolf-toolbuild`.

**Harnesses (ADR-0128).** The studio reads, lists and writes harness files with the format of
`grauwolf_spec::Harness` (a file is only written when it parses) and generates a project with
`grauwolf_host::project::generate_harness` – the same function `grauwolf harness generate` calls –
into `build/<name>/` of the workspace. The generation moved from the CLI into the host for this:
the studio may use the host and the server, never the CLI. The spec and harness listings skip the
top-level `build/` (its `app/` holds copies of the specs); the file tree shows it.

**Key order.** `serde_json::Value` sorts object keys. The studio reads and writes specs as
`OrderedJson` and converts to `Value` only for grauwolf's loader. serde_json's `preserve_order`
was not used: Cargo unifies features over everything built together, so it would change the map
order of every grauwolf crate in a workspace build.

The palette is derived, not copied: catalog kinds come with `NodeKindDescriptor::description()`,
`params_schema()`, `example_params()` and the signature of the example parameters as port types;
conditions report which types they turn into which. The kinds outside the catalogs come from
`grauwolf_spec::STRUCTURAL_NODE_KINDS` and `STRUCTURAL_CONDITION_KINDS`. A new node kind therefore
appears in the studio as soon as it is in the catalog with a description.

## Runs, keys and discovery (part 2a)

ADR-0126 decision 13. Three modules of the backend, their routes in `run_api.rs`:

| Module | Content |
|---|---|
| `runs.rs` | `Runs`: start, list, stop, chat turns, approvals; the events of a run; budgets and the limit of concurrent runs |
| `keys.rs` | `KeyStore`: the providers' API keys in `keys.json` |
| `discovery.rs` | model lists of the providers, the tools of an MCP server |

**A run** loads the *saved* spec file the way `grauwolf spec run` does – `AgentLoader` with a
`ToolBuilder` on `<workspace>/.grauwolf/tools`, `load_provisioned`, then
`InterpretedAgent::run_with` with the `RunExtras` of `provisioned.run(None).extras()` plus:

- a `Feature` that turns the agent's events into run events and counts the steps;
- the frame sink (text deltas – only `llm_request_streaming` nodes stream);
- an approver for `AgentSpec::approval_tools()` that asks through the event list and waits for
  `POST …/approvals/{id}`;
- the attachments and, for a chat, the conversation (`MemoryChatHistoryStore` keyed by the run, or
  with `conversation` a `FileChatHistoryStore` in `<workspace>/.grauwolf/conversations`).

The loader gets its own `Env`: the stored keys, then `GRAUWOLF_TOOL_POLICY` = the workspace's
`grauwolf.toml` (if there is one), then `StudioConfig::env` – the process environment, or a
`MapEnv` in tests. Nothing is written into the process environment, so two runs never see each
other's keys. `say_to_user` becomes a `notice`, `ask_user` is refused.

Each turn runs in a task of its own; a supervisor task waits for it, for a cancel (stop, or the
step budget exceeded in the feature) and for the wall time, and aborts it – the agent, its tools and
MCP servers drop with it. Budget per turn: steps (node starts, counted by the feature), tokens (the
interpreter's `Budget`, through `AgentLoader::with_budget`; the spec's lower `limits.max_tokens`
wins) and seconds; defaults `DEFAULT_BUDGET` (100 / 200000 / 300), at most `MAX_BUDGET` (1000 /
2000000 / 3600). At most `MAX_ACTIVE_RUNS` = 2 turns run at once (`429 too_many_runs`); idle chat
runs do not count. `KEPT_RUNS` = 20 runs are listed; older ones that are not running are dropped.

**Events.** Every run keeps its events in a list; `GET …/runs/{id}/events` sends them as SSE from
the first (or `?from=`, or after `Last-Event-ID`) and then as they come, until `finished`. The SSE
event name is the `type`, the data `{ seq, type, … }`, the SSE id `seq`:

| Type | Fields |
|---|---|
| `turn_started` | `turn`, `input` |
| `loaded` | `name` – the agent is provisioned (first turn) |
| `node_started`, `node_finished` | `node`, `subgraph?`, `path` |
| `plan_step` | `node`, `subgraph?`, `path` of the planner node, `iteration`, `action?`, `action_subgraph?`, `step?` |
| `llm_started` | `model` (`provider:id`) |
| `text` | `text` – a streamed piece |
| `tool_call` | `id`, `tool`, `args` (longer than 4000 characters: a cut JSON text) |
| `tool_result` | `id`, `tool`, `content` (cut to 4000 characters), `truncated` |
| `approval`, `approval_decided` | `id`, `tool`, `args`; `id`, `approved` |
| `usage` | `input_tokens`, `output_tokens`, `cost?` – the run so far |
| `notice` | `message` – `say_to_user`, fallbacks, decisions, skipped tool files |
| `output`, `error` | `output`; `message` |
| `turn_finished`, `finished` | `turn`, `status`; `status` |

`node` and `subgraph` match the canvas: the frontend keys a node `<subgraph>/<id>`. The backend
finds the subgraph from the path (`runs::subgraph_of`: each node before the last names its subgraph
in `params.subgraph`, or is a branch named like one). `plan_step` comes from the planner's
`PlanBuilt` (decision 15): the feature keeps a stack of the planner nodes that run (pushed by their
`NodeStarting`, popped by their `NodeCompleted`) and names the innermost one; for GOAP `action` is the
first name of the plan, for `plan` its step subgraph and `step` the description of the first open
step. The interpreter is not changed for it. The frontend folds the events with
`lib/run-events.ts` (`applyRunEvent`, pure and tested) into the transcript, the usage and the active
and finished nodes, which `SpecCanvas` marks on the nodes (`SpecReteNode.run`). A `turn_started` clears
the marks (`done`, `runs`), so every turn of a chat run shows its own path; the agent node carries the
number of marked nodes as `data-marks`, which the e2e script reads (a strategy's flow is not drawn).
When a run starts and when a turn or the run ends, `SpecCanvas` fits the flow (`fit(…, readable: false)`,
the whole flow at any zoom) if `anyOutside` (`lib/view-fit.ts`) finds a node outside the visible part –
never in between and not while a pointer button is held, so panning during a run is not undone.

**Runs per spec (decision 16 note).** `lib/spec-runs.ts` (pure, tested in `spec-runs.test.ts`) is
the model: a `RunRecord` per run (`RunView`, input, start and end, `n` = its number in the spec) in
`SpecRuns { records, order, selected, dropped }`; `order` lists the run ids per spec key
(`specKey(workspace, path)` – not the tab – newest first), `selected` is the run picked from the
history. `shownRun(state, key)` is the selected run, else the newest; `startRun`/`addRun` keep the last
`HISTORY_LIMIT` (10) runs per spec and never drop one that is running; `applyEvent` folds an event into
its record; `runningRuns`/`specRunning` feed the status bar and the running dot; `mergeListed` adds the
runs of `GET …/runs` for a path that the studio does not know (after a reload) as not yet `loaded`.
`store/runStore.ts` wraps it: `start(path, input)`, `send`/`stop`/`decide(path, …)` act on the run shown
for `path`, `select(path, id)`, `loadHistory(path)` (called by `RunPanel` when the shown spec changes),
`reset()` on a workspace switch. One `EventSource` per run lives in a module map until `finished`
(or until the run leaves the history), independent of the panel and the shown tab, so a run goes on in
the background. A listed run is followed (its events replayed) only when it is selected. The components
read `useShownRun()` (the run of the open spec; `NO_VIEW` while there is none), so `RunPanel`,
`BottomPanel`, `SpecCanvas` (marks, planner steps) and `StatusBar` need no run of their own. The tab
shows the running dot (`specRunning`) and the number of validation errors of the spec (the shown spec's
`problems`, the stash's `problems` for the others). No backend change: the backend keeps the last 20 runs of the whole
workspace (`KEPT_RUNS`), which is why the studio keeps its own last 10 per spec. `E2E_ONLY=runs-per-spec`
runs only the approved scenarios (`runsPerSpecChecks`). The `@/` alias is set for vitest in
`vitest.config.mts`, so stores can be tested (`store/runStore.test.ts`, with a fake `EventSource`).

**Keys.** `KeyStore` reads `<config dir>/keys.json` on every use and writes it through a temporary
file created with mode 0600, then renamed; the folder gets 0700. The configuration folder is
`--config-dir`, else `GRAUWOLF_STUDIO_CONFIG_DIR`, else `~/.config/grauwolf/studio`. Values are
never returned, logged or put into an error message (a broken file is reported with line and
column, not its content).

**Discovery.** `discovery::models` asks Ollama (`/api/tags` at `providers::ollama_host`),
OpenAI-compatible APIs (`<base url>/models`), Anthropic and Gemini with the key of the run's
environment; otherwise a static list with a `note`. `discovery::mcp_tools` uses
`grauwolf_host::mcp::list_tools`, the connect code of a run, with a timeout of 20 seconds.

**Tests** never call a model: `tests/common/stub.rs` is a small axum server that speaks Ollama's
`/api/chat` (streamed and not, text or a tool call, or no answer at all), `/api/tags` and an
OpenAI-style `/v1/models`; the studio gets its address through `StudioConfig::with_env`.
`common::events` reads an SSE stream event by event. `tests/runs.rs`, `tests/keys.rs` and
`tests/discovery.rs` cover the order of the events, approvals, stop, the two-run limit, the budgets,
chat turns, the key file and the model lists; the e2e script does the same in a browser against a
stub in Node.

## The working copy and the MCP endpoint (decision 17)

The studio is an MCP editor: a coding agent and the browser edit the same specs at the same time.
For that the backend holds the **working copy** – it, not the browser, owns the unsaved state of
every open spec.

### Documents

`documents::Documents` keeps one document per workspace id and spec path: the content
`{ "spec": …, "layout": … }` (the spec as `OrderedJson` in its key order, the layout as stored or
`null`), a version, the content as the file holds it (for `dirty`, compared without regard to key
order, so changing a value back makes the document clean again) and the modification time and size
of the spec and layout files. Every open, batch and save compares those with the disk: a clean
document whose file changed (a `git checkout`) is reloaded and sent as `reverted` with origin `disk`;
a dirty one is marked `disk_changed`, keeps the unsaved work, and its save is refused (`409
disk_changed`) until it is reverted or saved with `force`. `PUT /specs`, the file routes and the MCP
file tools tell the documents when they wrote, renamed or deleted a file.

### Ops

Every change is a batch of **ops** on JSON pointers into the document (`ops.rs`; the frontend has the
same model in `lib/doc-ops.ts`):

| Op | Effect | Inverse |
|---|---|---|
| `set { path, value, at? }` | replaces a value or adds an object member (at the end, or at `at`) | `set` of the old value, or `remove` |
| `insert { path, value }` | inserts into a list before the index (`-` appends) | `remove` of that index |
| `remove { path }` | removes a list item or an object member | `insert`, or `set` with `at` – the member comes back at its place |
| `move { from, path }` | moves a value (a rename in place within one object) | the move back (plus the value it replaced) |

`apply` returns the inverse ops, `apply_all` applies a batch all or nothing (an error names
`ops/<index>` and the pointer), `compose` joins batches, `diff(old, new)` gives the ops between two
states – objects by key; lists by common prefix and suffix, then item by item when the items changed
in place, else by a longest common subsequence (removes from the back, inserts from the front). The
inverses come back with every batch and event; undo and redo are not built yet but need nothing
else.

**Concurrency.** A batch names its `base_version`. Batches of other clients applied after it are
concurrent, and the batch is rebased over them with `ops::transform`:

- indices shift through an insert or remove in the same list (two inserts at one index: the one
  applied first stays first; an insert at the index of a removed item stays there);
- an op whose target a concurrent op removed is **dropped**;
- an op on the same pointer as a concurrent `set`, removed object member or move – or on a pointer
  inside or around it – is applied and **overwrites** it: the last writer wins;
- everything else commutes.

Dropped and overwriting ops are listed as `conflicts` (index, pointer, outcome, the other origin) in
the answer and the event. A client's own earlier batches are not concurrent – it wrote on top of
them –, so the browser may send a batch before the answer to the previous one. A batch older than
the last load or revert, or than the 1000 batches a document keeps, is `409 stale_base`: the client
opens the document again.

### Routes

| Route | Body | Answer |
|---|---|---|
| `POST /workspaces/{id}/documents/{path}:open` | – | `DocumentState { path, spec, layout, layout_problem?, version, dirty, disk_changed }` |
| `POST /workspaces/{id}/documents/{path}/ops` | `{ base_version, ops, client_id }` | `{ version, dirty, ops (as applied), inverse, conflicts }` |
| `POST /workspaces/{id}/documents/{path}:save` | optional `{ force }` | the state (`409 disk_changed`) |
| `POST /workspaces/{id}/documents/{path}:revert` | – | the state loaded from the file |
| `GET /workspaces/{id}/documents/events` | – | SSE: `ops`, `saved`, `reverted`, `disk_changed`, `closed`, `clients`, `files`, `run`, `resync` |

An `ops` event carries the version, the ops and their inverse, the sender's `client` id, the
`origin` (`studio` or `mcp:<client name>`), `dirty`, the conflicts and – from MCP tools – a `label`
(`add_node ask (llm_request)`) and a `highlight` (node keys `<subgraph>/<id>`, resource keys). The
stream is a `tokio::sync::broadcast` of 1024 events; a subscriber that falls behind gets `resync`.
`GET`/`PUT /specs/{path}` stay and work on the file.

### The frontend on the working copy

`store/documentStore.ts` keeps a **mirror** per open document (`lib/doc-mirror.ts`, pure): the
confirmed state at its version, the pending local batches and `local` = confirmed + pending, which
the spec store shows. The spec store did not change its actions: they still compute a new spec and
layout with the pure functions of `lib/spec-canvas.ts` and `lib/spec-overview.ts`, and a subscription
turns every such change into ops by diffing against the mirror (`/spec`, and `/layout` only when the
canvas changed it, since the document's layout may lack the positions the canvas computed). Batches
are sent one at a time; edits made meanwhile are joined into the next batch. A batch of another
client moves the confirmed state on and the pending batches are rebased over it with the backend's
rules (`transformAll`), so both survive; our own batch is confirmed by its answer or its event,
whichever comes first, and events that arrive ahead of a missing version wait. The spec store then
takes the mirror's spec and a layout from `lib/doc-layout.ts` – the document's positions, completed
with the places the canvas already shows and then automatically – without touching the selection.
The JSON view merges a remote change into the text with `rebasedText`/`mergeEdited`
(`lib/spec-source.ts`): the user's own edits of the text stay. Dirty marks come from the mirror,
Save and Revert go to the backend; closing a dirty tab reverts the document.

### Undo and redo (decision 18)

The undo model is `lib/undo.ts` (pure, tested in `lib/undo.test.ts`); `store/documentStore.ts` keeps
one `History` per open document. A **step** is a label and the inverse ops of one user action,
written against the mirror's `local` – the document as shown. Nothing in the backend changed: an
undo is an ordinary local edit, sent as a new batch based on the current version.

- **Recording.** The spec store's actions name their change through `asEdit({ label, group,
  window })`; the subscription that turns a change into ops also computes its inverse – the spec
  back to the mirror's, the layout back to what the canvas showed before (not to the document's
  layout, which may lack computed positions) – and passes both to `editDocument`. Without a label
  the step is named from the ops ("Edit system_prompt"); `track: false` marks an edit that is no
  action (Save completing the layout).
- **Grouping.** An edit merges into the top step if it has the same `group`, the step is not sealed
  and its last edit is younger than `window`: fields (`field:<pointer>`, `params:…`, `edge:…`) use
  1 s, a drag (`move:…`) and the JSON view (`json`) `Infinity`. `sealUndo` ends a step; the shell
  calls it on every `focusout` and `pointerup` (capturing), and the JSON view applies a waiting edit
  before it seals on blur. Each action is one store call, so Convert to graph or connecting with a
  suggested condition is one step already.
- **Undo and redo** (`stepDocument`): the top step is applied to the mirror as a local edit; the
  difference back (`diff` of the two `local` states, the same as the backend's inverse) is pushed
  onto the other stack, and the spec store follows with change kind `undo`, which bumps `revision`
  so the inspector's fields remount. A new own action clears the redo stack; each stack keeps 200.
- **Rebasing the stored inverses.** The backend rebases a batch only over *other* clients'
  batches since its base, never over the sender's own, so a step sent with an old base would miss
  the user's later edits. The browser keeps every stored inverse current instead: every change of
  `local` that is not a step – a remote batch, our batch coming back rebased, a refused batch, an
  EVENTS undo, Save's layout – is expressed as ops (`diff` of `local` before and after) and rebased
  into the stacks from the top down: the step is transformed over the change, the change over the
  step, then the next step meets it as it was before the step above (`rebase`). Own actions need no
  rebasing – the stack is last in, first out. A step whose target a remote change overwrote or
  removed is marked with its origin; its undo is refused with *Could not undo '<label>': changed by
  <origin>* and the step is dropped. If the ops do not apply, or the backend's answer to an undo
  batch reports conflicts, the same note appears and the step is dropped; the document stays as the
  backend applied it.
- **EVENTS undo.** An MCP `ops` event carries its `inverse` against the confirmed document; the
  store rebases it over our pending batches and keeps it per EVENTS line (`EditLine.undo`:
  `ready`, `undone`, `failed`, `gone`), rebasing it over every later change, own ones included, and
  marking it when a later change overwrote its target. `undoLine` sends it as a local edit that is
  no step of the user's stacks (they are rebased over it).
- **Clearing.** `:open`, Revert, a `reverted` event (reload from disk), `stale_base` (which opens
  the document again) and closing the tab clear the document's history and mark its EVENTS undos
  `gone`; Save keeps them; a renamed document keeps them under the new path.

### The MCP endpoint

`POST /mcp` on the studio's address (outside `/api/v1`) speaks MCP over Streamable HTTP with
grauwolf's own server side: `grauwolf_mcp::McpServer::handle` answers `initialize`, `ping`,
`tools/list` and `tools/call`, built per request with the tools of the calling session – as
`grauwolf-server`'s agent endpoint does – and the route is wrapped in `grauwolf_server::with_token`.
The token is random per start (`McpHub::random_token`) unless `serve --mcp-token` or
`GRAUWOLF_STUDIO_MCP_TOKEN` names one; `GET /api/v1/mcp` gives URL, token, the `claude mcp add`
command and the clients. `initialize` opens a session (`Mcp-Session-Id`) holding the client's name;
an unknown session is `404` so the client initializes again; `GET /mcp` is `405`, `DELETE /mcp`
ends the session. A client is active while it called within the last ten minutes; new and ended
sessions go out as `clients` events.

The tools (`mcp_tools.rs`) work on the workspace the browser reported last with
`PUT /api/v1/mcp/context` (or the only one of `serve --workspace`), and a missing `path` is the
active tab. Edits are batches of `Documents::edit` – the tool computes ops from the document, the
batch carries client `mcp:<session>`, origin `mcp:<client name>`, label and highlight. The resource
keys and sections of the overview (`resources`, `write_resource`) follow the frontend's
`lib/spec-overview.ts`; `connect` picks the condition the palette's transitions offer between the
source's output and the target's input type, or explains that the ports do not fit. `run_spec`
starts a run through `Runs::start`, sends a `run` event (the run store follows it like its own) and
waits for the end.

**Adding a tool:** a test in `tests/mcp.rs` against a temporary workspace (`Studio::new`, `call`,
`ok`/`error`), the definition (name, description for a coding agent, JSON schema) in `definitions()`,
the arm in `call`, an edit through `Ctx::edit` with label and highlight; then the crate's
`AGENTS.md`, chapter 16 of the user guide and this chapter.

## The generated client

The frontend talks to the backend only through `studio/web/lib/api/`:

1. `grauwolf-studio openapi` prints the OpenAPI description; `npm run gen-api` writes it to
   `lib/api/openapi.json` and runs openapi-typescript (pinned) into `lib/api/schema.d.ts`;
2. `lib/api/client.ts` wraps openapi-fetch (pinned) and turns the error shape into an `ApiError`.

Two checks keep it current: the Rust test `the_committed_openapi_description_is_current` compares the
committed `openapi.json` with the backend (`cargo xtask check --affected` selects the studio when
`studio/web/lib/api/` changes), and `npm run check-api` regenerates `schema.d.ts` into a temporary
file and compares.

## Adding a route

1. Write the test in `crates/apps/grauwolf-studio/tests/` against `router(…)` with
   `tower::ServiceExt::oneshot` and `grauwolf_test::TestDir` (`StudioConfig::single(dir)` and
   `common::ws(dir, "/specs…")` for a route of one workspace, `StudioConfig::root(dir)` for the
   workspaces).
2. Add the handler with `#[utoipa::path]` to `api.rs`, list it in `ApiDoc` and the router; a route
   of a workspace goes below `/workspaces/{id}/`, takes `Ws` (and `RelPath`) and lists the `id`
   parameter. Types with `ToSchema`. Read bodies as bytes with `parse_body`, so a bad body is the error shape too.
3. `npm run gen-api` in `studio/web`, use the new types through `lib/api/client.ts`.
4. Document the route in the crate's `AGENTS.md` and in `studio/README.md`.

## Part 2b

GitHub sign-in and GitHub workspaces (a `source` of kind `github` with repository and branch beside
`local`) follow. Their AgentWeaver code is kept in `studio/web` but not mounted (see
`studio/web/AGENTS.md`). The MCP editor with live sync is decision 17, above; undo and redo on its
inverses are next.
