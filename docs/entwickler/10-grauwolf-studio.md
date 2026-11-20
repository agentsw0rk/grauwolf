# 10 – grauwolf studio

grauwolf studio ([ADR-0126](../adr/0126-grauwolf-studio.md)) ist der grafische Editor für Specs: ein
Rust-Backend, die App-Crate `crates/apps/grauwolf-studio`, und ein Next.js-Frontend in `studio/web/`,
das aus einem früheren Frontend des Projekts AgentWeaver hervorgegangen ist. Bauen, Starten, die
Routen und das Layout-Format stehen in `studio/README.md` (englisch); dieses
Kapitel behandelt den Code.

## Workspaces

Workspaces sind getrennte Projekte (ADR-0126 Entscheidung 11). Das Backend-Modul `workspaces.rs`
hält sie in einem von zwei Modi:

- **root** (`serve --root <ordner>`, Standard `~/grauwolf-studio/workspaces`): Eigene Workspaces
  sind die Unterordner, deren Name ein Slug ist und die `.grauwolf-studio/workspace.json` enthalten;
  ihre ID ist der Ordnername. Registrierte Ordner anderswo sind Einträge in
  `<root>/.registry.json` mit denselben Metadaten – in einen registrierten Ordner schreibt das
  Studio nie. Die Liste wird bei jeder Anfrage von der Platte gelesen, ein von Hand in den
  Wurzelordner kopierter Ordner erscheint also sofort.
- **single** (`serve --workspace <ordner>`): Der Ordner ist der einzige Workspace, ID =
  `slug(Ordnername)`; alles, was die Menge ändert, ist `409 single_workspace`.

`Workspaces::open(id)` macht aus einer ID den `Workspace` (`workspace.rs`, unverändert:
Pfadregeln, atomares Schreiben), auf dem die Specs- und Datei-Handler arbeiten. Die Handler
bekommen ihn über den Extraktor `api::Ws`, der `{id}` aus der Route liest und mit
`404 workspace_not_found` antwortet; `api::RelPath` liest `{path}`. Alle Specs- und Datei-Routen sind
deshalb `/workspaces/{id}/…`; `schema`, `node-kinds` und `openapi.json` bleiben oben. Änderungen
(Anlegen, Registrieren, Ändern, Entfernen) serialisiert ein Mutex; ein neuer Workspace entsteht in
einem versteckten Ordner `.creating-…` des Wurzelordners und wird an seinen Platz umbenannt,
Metadaten und Registry werden mit `write_atomic` geschrieben. Registrieren lehnt Überschneidungen ab
(den Wurzelordner, alles darin oder darum herum, jeden anderen Workspace) – das hält zwei
Workspaces davon ab, die Dateien des anderen zu sehen. Die Vorlage `examples` kopiert
`--examples <ordner>`, standardmäßig `docs/user-guide/examples` des Repositorys, in dem das Binary
gebaut wurde (`CARGO_MANIFEST_DIR`, zur Laufzeit geprüft); ein installiertes Binary ohne das
Repository antwortet `400 no_examples`, und das Frontend bietet die Vorlage abgeschaltet an
(`examples: false` in `GET /workspaces`).

Im Frontend zeigt `app/page.tsx` `components/studio/StartPage.tsx`, bis ein Workspace offen ist,
dann `EditorApp` mit der Workspace-ID als Schlüssel, sodass ein Umschalten den Editor neu einhängt.
`store/workspacesStore.ts` lädt die Liste, hält den offenen Workspace in der URL (`?w=<id>`,
`history.pushState`; Zurück und Vor folgen ihm), merkt sich die zuletzt geöffneten IDs im
localStorage, fragt vor einem Umschalten nach ungespeicherten Änderungen und setzt die Spec- und
Datei-Stores zurück. `lib/current-workspace.ts` hält die offene ID für jeden Aufruf unterhalb von
`/api/v1/workspaces/{id}/` (`workspaceParam()`); nur der Workspaces-Store setzt sie.
`components/studio/WorkspaceSwitcher.tsx` sitzt oben im Dateibaum und zeigt `tool_policy` und
`lockfile` des Workspaces.

`scripts/studio.sh` startet das Backend auf dem Wurzelordner und richtet die Workspaces dann über die
API ein: einmal (Marker `<root>/.grauwolf-studio-migrated`) registriert es den alten
Standardordner `~/grauwolf-studio-workspace` als „examples“; ohne jeden Workspace legt es
„examples“ aus der Vorlage an und markiert ihn mit `.grauwolf-studio-seeded`; jeder Ordner mit
diesem Marker bekommt die fehlenden Beispieldateien (`cp -Rn`).

## Das Modell: Spec-JSON

Die Zeichenfläche bearbeitet Spec-JSON direkt (ADR-0126 Q2). Ein zweites Graphmodell gibt es nicht:

- jeder Knoten auf der Zeichenfläche ist ein Eintrag von `graph.nodes` oder von `graph.nodes` eines
  Subgraphen, dazu die impliziten `start` und `finish` jedes Graphen;
- jede Verbindung ist ein Eintrag von `edges`, erkannt an ihrem Index;
- Positionen, Ausschnitt und eingeklappte Gruppen stehen in `<spec>.layout.json` (Q8 b), nie in der
  Spec.

Das Frontend hält die Spec als rohes JSON (`SpecJson` in `studio/web/lib/spec-canvas.ts`), damit
Felder, die die Zeichenfläche nicht kennt, jede Bearbeitung überstehen. Die Abbildung besteht aus
reinen Funktionen mit vitest-Tests (`lib/spec-canvas.test.ts`): `canvasFromSpec`, `addNode`,
`removeNode`, `renameNode`, `connect`, `suggestCondition`, `groupsOf`, `problemsByNode`, …
Schlüssel auf der Zeichenfläche sind `<subgraph>/<id>` für Knoten und `<subgraph>#<index>` für Kanten
(leerer Subgraph = Hauptgraph); IDs können kein `/` enthalten.

### Zwei Ebenen

Eine Spec hat zwei Ansichten (`view` in `store/specStore.ts`, Reiter in
`components/studio/ViewBar.tsx`):

- **Übersicht** (`lib/spec-overview.ts`): der Agenten-Knoten (`agent`) und je ein Knoten pro
  Ressource, jeder mit einem Schlüssel (`tool:<name>`, `tool_package:<i>`, `builtin:<gruppe>`,
  `mcp:<name>`, `agent_tool:<name>`, `knowledge`, `memory`, `skill:<i>`, `self_tools` …) und dem
  JSON-Pointer seines Teils der Spec. `decide`-Knoten haben keinen Knoten (Entscheidung 14); die eine
  abgeleitete Ressource, `system_one` (`derivedNodes`, `SYSTEM_ONE_KEY`, `isDerived`), hat keinen Pointer
  und steht nicht in `resourceNodes`, sodass `takeResource`, `removeResource`, `deleteResource` und
  `disconnectResource` sie nie anfassen (veraltete `decision:…`-Positionen älterer Layouts ignoriert
  `completeOverview`). Eingebaute Tools werden nach
  den `builtin_groups` von `/node-kinds` (`grauwolf_spec::BUILTIN_GROUPS`) gruppiert, nicht nach
  einer Kopie der Tabelle. Bearbeitet wird über den Pointer (`setAtPointer`, `writeResource`,
  `takeResource`, `removeResource`), sodass unbekannte Felder erhalten bleiben. Die Positionen
  stehen in `layout.overview.nodes`, vervollständigt von `completeOverview`. Ein lokaler Sub-Agent
  (`agent_tools.<name>.spec`) hat die Art `agent_tool` und seinen `spec`-Pfad als `ref`, ein
  entfernter (`url`) die Art `a2a_agent`.
- **Ablauf**: der bearbeitbare Graph wie hier beschrieben – nur für eine Spec mit eigenem `graph`.
  `lib/views.ts` (`hasOwnGraph`, `viewsOf`, `strategyKind`) bestimmt, welche Reiter `ViewBar`
  anbietet, und `setView` ignoriert eine Ansicht, die die Spec nicht hat.

**Agentenart und Ablaufzusammenfassung (Entscheidung 14).** `lib/views.ts` leitet sie als reine
Funktionen ab, getestet in `lib/views.test.ts` an den Formen der Beispiele des Benutzerhandbuchs:
`agentKind` (`simple` mit seiner Strategie für eine Spec mit Strategie, `planner` für einen eigenen
Graphen mit einem `plan`- oder `goap`-Knoten im Hauptgraphen oder einem beliebigen Subgraphen, sonst
`graph`), `agentKindLabel` („Agent“, „Graph-Agent“, „Planner-Agent“ – das Abzeichen des Agenten-Knotens),
`flowSummary`/`flowSummaryText` („Graph: 5 nodes, 1 decision“; „1 decision“, „2 subgraphs“ und „1 planner“ nur, wenn die Zahl nicht null ist;
Knoten aller Graphen, ohne das implizite `start`/`finish`) und `systemOneDecides` (die `decide`-Knoten
mit `decider: system_one`). `components/studio/AgentKindInfo.tsx` zeichnet, für einen Graph-
oder Planner-Agenten, die Zusammenfassung als Knopf, der den Ablauf öffnet (`setView`), bzw.
`StrategyActions` für einen Agenten mit Strategie; der Agenten-Knoten und der Inspektor nutzen beide (der Knoten zeigt die Art in seinem Abzeichen, `SpecNode`; der Inspektor als Zeile, `showKind`).

**System One (Entscheidung 14).** `derivedNodes` in `lib/spec-overview.ts` fügt den Knoten `system_one`
(„System One (RKEV)“, Art `system_one`, Port `resources`, `derived: true`, `uses` = die decide-Knoten)
hinzu, wenn `systemOneDecides` welche findet – zu `overviewOf` und `completeOverview` (so bekommt er eine
Position und eine Kante `overview#system_one`). `overviewDrawing` markiert ihn als `derived` mit dem
Hover `DERIVED_NOTE`; das Kontextmenü, die Entf-Taste (`removeResource` setzt eine Meldung) und der
Inspektor bieten weder Trennen noch Löschen. `SystemOneForm` in `Inspector.tsx` listet die
decide-Knoten (`openFlowNode` öffnet den Ablauf und wählt einen aus) und zeigt, ob `RKEV_BASE_URL` und
`RKEV_API_KEY` gesetzt sind, aus `GET /external/system-one` (`run_api::system_one_status`:
`{ base_url, api_key }`, je `{ variable, set }`, aus dem `Env` des Studios gelesen, nie der Wert).

Eine Spec mit `strategy` hat keinen Flow-Reiter. `components/studio/StrategyActions.tsx` (im
Agenten-Knoten und im Inspektor) zeigt die Strategie mit „Edit as graph“ (`editAsGraph` ersetzt die
Spec durch `flow.converted`, danach erscheint der Flow-Reiter) und „Show flow“, das
`components/studio/FlowDialog.tsx` öffnet: der Ablauf von `POST /specs/flow`, nur lesend gezeichnet
mit demselben `flowDrawing`-, `sync`- und `fit`-Code wie die Flow-Ansicht (ein kleiner eigener
Rete-Editor, neu angeordnet, nie gespeichert), mit einem Hinweis zur Strategie und „Edit as graph“.
Der Store hält `flow` nur für eine solche Spec und `flowDialog` für den Dialog. Der Dateibaum
lässt `layout`-Dateien weg (`visibleEntries`); die API listet sie weiterhin. Lässt sich Monaco nicht
laden, zeigt `FileEditor` statt des Editors eine Meldung (es holt zuerst `/monaco/vs/loader.js`).

### Ports, Verbinden und Parken (Entscheidung 12)

Der Agenten-Knoten hat benannte Eingangs-Ports – `input` (zeigt den `input`-Typ der Spec), `tools`,
`agents`, `resources`, `skills` (`AGENT_PORTS`, seit Teil 1g vier Gruppen) – und den Ausgang
`output` mit dem `output`-Typ der Spec; jeder Ressourcen-Knoten hat einen Ausgang `agent`.
`portOf(kind)` ordnet einer Ressourcenart ihre Gruppe zu (Tools, Tool-Dateien, `tools_dir`,
eingebaute Gruppen, Tool-Pakete und `self_tools` zu `tools`; MCP-Server, Wissen, Gedächtnis und das abgeleitete System One zu `resources`; lokale und A2A-Agenten
zu `agents`). `lib/spec-drawing.ts`
beschreibt jeden Port als `DrawPort { key, label, socket, type? }`; eine Kante der Übersicht trägt
`fromPort`/`toPort`.

`lib/spec-rete.ts` hat **eine Socket-Klasse pro Port-Gruppe** (`ToolsSocket`,
`AgentsSocket`, `ResourcesSocket`, `SkillsSocket`, die
`InputTypeSocket`/`OutputTypeSocket` des Agenten und `FlowSocket` für jeden Knoten eines Graphen),
alle abgeleitet von `PortSocket`, dessen `isCompatibleWith` nur einen Socket derselben Klasse
annimmt. `makeConnection` fragt die beiden Sockets und ruft statt `connect` `refuse` auf (eine
Fehlermeldung), wenn sie nicht passen – so lehnt das Verbindungs-Plugin selbst einen Skill auf
Resources ab. `PORT_COLORS` in `SpecNode.tsx` gibt jeder Gruppe die Farbe von Socket, Beschriftung
und Verbindungen (eine `SpecReteConnection` trägt ihre `group`); die Ressourcen-Knoten behalten die
Farbe ihrer Art (`colorsOf`, `KIND_COLORS`, sonst die Farbe des Ports der Art über `portOf`). Ein Knoten wird neu gebaut, wenn sich die
Schlüssel oder Klassen seiner Ports ändern (`SpecReteNode.signature`); geänderte Beschriftungen (ein
neuer Typ) werden an Ort und Stelle gesetzt (`relabel`).

Verbinden und Trennen sind reine Funktionen in `lib/spec-overview.ts`, die der Store benutzt:

- `takeResource` / `disconnectResource`: die Ressource aus der Spec, ihre Art, ihr Name (der
  Schlüssel in `mcp_servers`/`agent_tools`) und ihr JSON geparkt in `layout.overview.parked` unter
  einem neuen Schlüssel `parked:<alter Schlüssel>` (`~2` …, falls vergeben), an der alten Position;
  die Positionen späterer Einträge einer über den Index geschlüsselten Liste (`tool_file:<i>`,
  `tool_package:<i>`, `skill:<i>`) rücken nach.
- `writeResource` / `connectParked`: das geparkte JSON an seine Stelle zurückgeschrieben – an eine
  Liste angehängt, unter einem freien Namen in eine Map (`server_2`), in `builtin_tools`
  eingemischt; ein einzelner Abschnitt, der schon da ist (`knowledge`, `long_term_memory`,
  `tools_dir`, `self_tools`), wird mit einer Meldung abgelehnt.
- `parkResource`: eine Paletten-Vorlage (`resourceTemplate`) oder eine fallen gelassene Spec
  (`agentReference`), geparkt neben dem Agenten (`besideAgent`) oder dort, wo sie fallen gelassen
  wurde; `deleteResource` entfernt eine geparkte oder verbundene Ressource aus Spec und Layout;
  `setParkedConfig` bearbeitet eine geparkte.
- `parkedNode` zeichnet eine geparkte Ressource, indem es sie in eine leere Spec schreibt und deren
  Knoten nimmt, sodass sie wie ihr verbundenes Ich aussieht, mit der Zeile „not connected“ und
  `parked: true` (gestrichelt, grau). Geparkte Ressourcen stehen nicht in der Spec, die Validierung
  sieht sie also nie.

Die Zeichenfläche bildet eine von einer Ressource zum Agenten gezogene Verbindung auf
`connectResource` ab (nur eine geparkte ändert etwas), einen Klick auf eine Verbindung der Übersicht
auf die Auswahl `{ type: "link" }` (Entf, das Kontextmenü oder der Inspektor trennen) und das Ablegen
von `RESOURCE_DRAG_TYPE` und `AGENT_DRAG_TYPE` auf der Übersicht auf `addResource`/
`addAgentReference`. Für lokale Sub-Agenten lädt der Store die referenzierten Specs
(`refreshAgents`, nach jeder Validierung, Pfade mit `resolveReference` relativ zur offenen Spec
aufgelöst), und `agentLine` gibt der Übersicht „runs <name> · <model>“.

**Ablauf-Ports (P5):** `flowPortTypes` gibt den Ports jedes Ablauf-Knotens ihre Typen – `start` den
`input` des Graphen, `finish` seinen `output`, `input`/`output`-Parameter (`agent`, `code`,
`for_each`) und der Subgraph von `subgraph`/`parallel` vor der Signatur aus `/node-kinds`. Während
eine Verbindung gezogen wird, rufen die Ereignisse `connectionpick`/`connectiondrop` des
Verbindungs-Plugins `pick` auf, und `compatiblePorts` hebt die Ports hervor, an denen ein Ablegen
enden könnte: dieselbe Socket-Klasse, in einem Ablauf derselbe Graph und `portsFit` – gleiche Typen,
eine Bedingung, deren Übergänge den einen in den anderen überführen (die Logik von
`suggestCondition`), jede Kante, die `decide` verlässt, oder ein unbekannter Typ.

### Der Ablauf einer Strategie

`POST /specs/flow` (`flow.rs`) lädt die Spec mit grauwolfs eigenem Lader (`check::load` →
`AgentSpec::from_json`, das `grauwolf_strategies::expand` aufruft); das Studio kopiert die Vorlagen
also nie. Die Antwort enthält den Hauptgraphen, `derived` und die Typen, die die Strategie ergänzt;
`converted` ist die rohe Spec ohne `strategy`, mit diesem Graphen und diesen Typen (fehlende
optionale Felder weggelassen). Sie lädt zu demselben `AgentSpec` (mit geleertem `strategy`) –
`tests/flow.rs` prüft das, und dass sie gleich validiert, für jedes Strategie-Beispiel des
Handbuchs. Alle vier Strategien (`single_run` mit `parallel_tools` oder `moderate`, `react`, `chat`,
`verified`) haben ein exaktes Gegenstück, also lassen sich alle umwandeln.

### Anordnung mit ELK (Entscheidung 19)

Ablauf und Übersicht ordnet der Eclipse Layout Kernel an, `elkjs` (fest auf 0.12.1; der gebündelte
Build läuft im Hauptthread und wird beim ersten Gebrauch mit einem dynamischen `import` geladen, der
statische Export hat ihn also als eigenen Chunk, kein CDN). Drei Module:

- `lib/elk-layout.ts` – die Engine, ohne Wissen über Specs: Sie ordnet *Formen* an (Knoten mit ihrer
  gezeichneten Größe und festen Ports, Kanten mit der Größe ihrer Beschriftung, Subgraphen,
  Verbindungen nur für die Anordnung) und liefert Positionen und Wege (`elkGraph` baut die ELK-Eingabe,
  `fromElk` liest das Ergebnis; beide rein). `arrange(shapes, "flow" | "overview")` behält das
  Ergebnis je Form des Graphen im Speicher.
- `lib/auto-layout.ts` – zwischen Zeichnung und Engine: `shapesOf(drawing)`, `completeLayout`,
  `tidyLayout`, `drawingRoutes` und `withRoutes` (unten).
- `lib/edge-routes.ts` – der Ersatz-Router, Beschriftungsplätze auf einem Weg und der abgerundete Pfad.

**Optionen.** Jeder Durchgang ist `elk.algorithm: layered`, `elk.direction: RIGHT`, `elk.edgeRouting:
ORTHOGONAL`, Kreuzungsminimierung `LAYER_SWEEP`, Knotenplatzierung `BRANDES_KOEPF` mit Ausrichtung
`BALANCED` (sie hielt die Reihen bei `04-decide` gerader als `NETWORK_SIMPLEX`), Modellreihenfolge für
Knoten und Kanten, `elk.edgeLabels.placement: CENTER` mit angefragten Inline-Beschriftungen, 48 px
zwischen Knoten, 64 px zwischen Schichten, 24 px zwischen Kanten und Knoten, 14 px zwischen Kanten,
Koordinaten relativ zur Wurzel (`elk.json.shapeCoords`/`edgeCoords: ROOT`). Beschriftungen gehören zur
Anordnung, ELK hält sie also von Knoten und voneinander fern (es setzt sie neben ihre Kante). Jeder
Knoten hat `elk.portConstraints: FIXED_POS`; `nodePorts` setzt die Ports dorthin, wo `SpecNode` sie
zeichnet – Eingänge links, Ausgänge rechts, je eine Zeile von 19 px unten im Knoten; ein Planer hat
einen Port in der Mitte seiner Unterkante für seine Verbindungen. Die Größen sind die gezeichneten:
`SpecNode` gibt einem Knoten mindestens seine Layout-Höhe (`minHeight`) und hält die Ports unten;
`nodeHeight(lines)` bemisst einen Ablaufknoten oder eine Ressource nach ihren Textzeilen
(`NODE_SIZE` 200×92, `TERMINAL_SIZE` 140×60, jede weitere Zeile 17 px, der Agent 300×260).

**Zwei Durchgänge für den Ablauf.** Zuerst wurde `elk.hierarchyHandling: INCLUDE_CHILDREN` mit den
Subgraphen als zusammengesetzten Knoten versucht: Eine Richtung für alle Ebenen zog die Hauptlinie von
`06-subgraph` auf 2650 px oder stellte die Subgraphen links davon. Deshalb wird jeder Graph – der
Hauptgraph und jeder Subgraph – für sich angeordnet, dann die Graphen als Kästen (der Kasten eines
Subgraphen mit Platz für seinen Rahmen: `GROUP_PADDING` ringsum, `GROUP_HEADER` darüber) von oben nach
unten (`elk.direction: DOWN`): Die Verbindungen der Planer sind Kanten dieses Durchgangs, von einem
Port unter dem Planer zur Oberkante des Kastens des Aktions-Subgraphen, sodass die Aktions-Subgraphen
eines Planers in einer Reihe darunter stehen; die übrigen Subgraphen hängen über verborgene Kanten an
ihren linken Kanten untereinander. Auch die Verbindungen bekommen Wege und Beschriftungsplätze von ELK
(in der Größe des Beschriftungskastens der Überlagerung, `linkLabelSize`).

**Positionen.** `completeLayout(spec, layout, context)` (asynchron) gibt jedem Knoten jedes Graphen und
der Übersicht eine Position. Gespeicherte Positionen ändern sich nie. Ist keine gespeichert, kommen die
Knoten dorthin, wo ELK sie hinstellt; sonst wird jeder fehlende Knoten (`placeMissing`) so verschoben
wie der gespeicherte Knoten, der ihm in ELKs Anordnung am nächsten ist, und dann nach unten, bis er
nichts überdeckt – ein Knoten aus der JSON-Ansicht oder von einem MCP-Client landet neben seinen
Nachbarn. Der Spec-Store wartet darauf, wenn eine Spec geöffnet, angelegt oder umgewandelt wird, und
beim Speichern; nach jeder anderen Änderung vervollständigt ein Abonnement (`completeShown`), was
fehlt, ein Lauf nach dem anderen, außerhalb der Rückgängig-Historie (wie bisher erreichen die
Positionen die Arbeitskopie mit der nächsten Änderung des Layouts). Bis dahin wird ein Knoten ohne
Position nicht gezeichnet (`flowDrawing` lässt ihn aus; mit `all` listet es jeden Knoten jedes Graphen,
auch eingeklappter Subgraphen – das, was angeordnet wird). Geparkte Ressourcen behalten die
Platzierung von `completeOverview`.

**Tidy up.** `tidyLayout(spec, layout, view)` ordnet den Ablauf oder die Übersicht neu an; `tidyUp` des
Stores setzt das mit `asEdit({ label: "Tidy up" })`, ein Rückgängig-Schritt (Entscheidung 18), und
erhöht `fitRequest`, worauf die Zeichenfläche nach dem Abgleich die Ansicht einpasst. Der Befehl
`view.tidy-up` (⇧⌥F / Umschalt+Alt+F; `matchesShortcut` vergleicht auch `event.code`, da ⌥ auf dem Mac
aus ⇧⌥F ein „Ï“ macht) und der Knopf *Tidy up* neben dem Ansichts-Umschalter rufen es auf.

**Wege.** Wege werden nicht gespeichert; `SpecCanvas.sync` holt sie bei jedem Zeichnen von
`drawingRoutes(drawing)`. Das fragt nach ELKs Anordnung der Zeichnung (aus dem Speicher, wenn sich die
Form des Graphen nicht geändert hat) und ermittelt den Versatz, den die meisten Knoten zwischen
gezeichneter und angeordneter Position teilen. Eine Kante (oder Verbindung) behält ELKs Weg, um diesen
Versatz verschoben, wenn beide Enden dort stehen, wo ELK sie hingestellt hat, und weder ihr Weg noch
ihre Beschriftung einen gezeichneten Knoten oder eine andere Beschriftung trifft. ELK kann nicht um
Knoten an Positionen herumführen, die es nicht selbst gewählt hat – `layered` stellt die Knoten selbst,
und der Algorithmus `fixed` führt keine Kanten –, daher führt `routeAround` in `lib/edge-routes.ts`
jede andere Kante: 20 px gerade aus dem Port, dann über das Gitter der Linien, die 16 px Abstand zu
jeder Knotenbox (und zu den Rahmen anderer Subgraphen) halten, Dijkstra über Gitterpunkte und
Richtungen mit 40 px je Knick, gerade in den Port. Ihre Beschriftung kommt auf ein freies Stück des
Wegs (`placeLabel`: zuerst die Mitte des längsten waagerechten Stücks). `withRoutes` fügt der Zeichnung
die Wege hinzu, vergrößert jeden Subgraph-Rahmen um die Wege und Beschriftungen seiner Kanten (ELK kann
eine Kante außerhalb der Knotengrenzen führen) und berechnet den `extent` der Zeichnung.
`SpecConnectionView` zeichnet den Weg mit abgerundeten Ecken (8 px, `roundedPath`), seine Enden an die
Sockets verschoben, wie Rete sie platziert hat (`snapEnds`, so bleibt der Weg orthogonal, während ein
Knoten gezogen wird), die Beschriftung im Beschriftungskasten des Wegs; Auswahl, Hover, ungültig (rot)
und die Gruppenfarben sind wie zuvor. `plannerOverlay` nimmt die Wege der Verbindungen, die der Abgleich
zurückgibt, und zeichnet sie ebenso, gestrichelt.

**Einpassen.** `fit` passt die Knoten und den `extent` der Zeichnung (Rahmen, Wege, Beschriftungen) ein,
zentriert und nie über Zoom 1; bräuchte das weniger als `MIN_FIT_ZOOM` (0,6), bleibt es bei 0,6 und
beginnt am linken (oder oberen) Ende – der Rest ist eine Verschiebung entfernt. Es läuft, wenn eine Spec
oder Ansicht ohne gespeicherten Ausschnitt öffnet, nach Tidy up und wenn ein Lauf startet oder ein Zug
oder der Lauf endet und ein Knoten außer Sicht ist.

### Planer im Ablauf (Entscheidung 15)

`POST /specs/planners` (`planners.rs`) liest jeden `goap`- und `plan`-Knoten aus dem Spec-JSON, wie es
gesendet wird (es muss nicht laden): die Aktionen mit ihrem Ziel (`subgraph` oder `agent`), `pre`,
`effects`, `cost` (bei GOAP standardmäßig 1) und Beschreibung, den Startzustand und die Ziele. Für GOAP
kommt der **erwartete Plan** aus `grauwolf_planner::goap::expected_plan(params)` dazu, nie eine Kopie der
Suche: Das Domain-Crate baut denselben `GoapPlanner` wie `fact_planner`, aber mit Ausführern, die nie
laufen (`Executor::Planning`), und ruft `GoapPlanner::plan(state)` – die Suche, die `build_plan` vor
jedem Schritt eines Laufs verwendet – für den Startzustand mit leerer Aufgabe auf. `Err` ist der Grund
(`no_plan`): kein Ziel in den Grenzen erreichbar oder ungültige Parameter. Ein `plan`-Knoten hat eine
Aktion, benannt nach seinem Schritt-Subgraphen, und keinen erwarteten Plan.

Das Frontend hält die Antwort im Store (`planners`, mit der Validierung geholt, wenn die Spec einen
Planer-Knoten hat). `lib/planner-view.ts` (rein, getestet) baut das Ansichtsmodell: `plannerLines`
(Zustand, Ziele, „⚠ no valid plan“; `flowDrawing` setzt sie auf den Knoten und zählt einen fehlenden
Plan als Warnung), `plannerOverlay` – eine `ActionLink` je Planer und Ziel mit den Aktionen als
Beschriftungen, ihren Positionen im erwarteten Plan, ihrem Hover-Text (`actionHover`, `factsText`) und
ihren Laufmarken, dazu Kästen für Sub-Agenten – und `nextPlanStep`/`closePlanSteps` für einen Lauf.
`SpecCanvas.drawPlannerLinks` zeichnet sie imperativ in eine Ebene zwischen den Subgraph-Rahmen und den
Knoten (ein SVG gestrichelter Pfade, HTML-Beschriftungen mit Hover): Es sind keine Rete-Verbindungen,
man kann sie also nicht auswählen, löschen oder speichern. Die Anordnung liest die Ziele der Planer allein
aus der Spec (`plannerTargets`, bevor das Backend geantwortet hat) und stellt die Aktions-Subgraphen in
eine Reihe unter ihren Planer (siehe *Anordnung mit ELK*).

In einem Lauf sagt das Ereignis `plan_step` des Backends (unten), welche Aktion läuft. `applyRunEvent`
hält die Schritte in `RunView.plan`: Ein neuer Schritt schließt den offenen des Planers, die Laufnummer
steigt, außer dieselbe Aktion läuft gleich noch einmal (①②②③), und `node_finished` des Planers schließt
seinen letzten Schritt. Die Knoten in einer Aktion melden keine Ereignisse (der Planer führt seine
Aktionsgraphen ohne den Beobachter des Laufs aus); daher wird `start` des Aktions-Subgraphen markiert,
wenn sein Schritt beginnt, und `finish`, wenn er endet.

### Rete

`lib/spec-drawing.ts` macht aus dem Store eine Zeichnung – Knoten mit Rolle (`flow`, `agent`,
`resource`), Sockets, Titel, Detailzeilen und Problemmarken, Kanten mit Beschriftung,
Subgraph-Rahmen und ob sie bearbeitbar ist – als reine Funktionen mit Tests. `lib/spec-rete.ts`
richtet Rete.js ein (Area-, Connection-, Kontextmenü- und React-Plugin, die Ansichten aus
`components/studio/SpecNode.tsx`). `components/studio/SpecCanvas.tsx` hält Rete mit der Zeichnung
gleich: Eine neue `generation` (Spec geöffnet oder ersetzt, andere Ansicht) zeichnet alles neu und
stellt den Ausschnitt dieser Ansicht wieder her; eine Bearbeitung fügt Knoten hinzu, entfernt und
aktualisiert sie, führt die Kanten und zeichnet die Verbindungen neu, wenn sich die Kanten geändert
haben, oder aktualisiert die Wege der gebliebenen (beim Ziehen).

Die Zeichenfläche ändert die Spec nie über Retes eigene Ereignisse:

- das Connection-Plugin läuft mit dem bidirektionalen Ablauf und einem `makeConnection`, das
  `connect` im Store aufruft und selbst nichts zeichnet – der nächste Abgleich zeichnet die Kante.
  Ein Druck auf einen Eingangs-Socket nimmt daher nie eine vorhandene Kante auf (und entfernt sie);
- Kanten und Knoten werden über den Store gelöscht: Entf auf der Auswahl, das Kontextmenü, der
  Inspektor;
- ein verschobener Knoten ändert das Layout, ein angeklickter Knoten oder eine angeklickte
  Verbindung wird ausgewählt; Rete-Ereignisse während eines Abgleichs werden ignoriert.

Abgleiche laufen nacheinander in einer Promise-Warteschlange. In der ersten Version liefen sie beim
Öffnen einer Spec gleichzeitig (der Neuaufbau und der Bearbeitungs-Effekt feuerten beide), und die
Verbindungsereignisse des einen erreichten den Store, während der andere das „syncing“-Flag schon
zurückgesetzt hatte – Kanten wurden doppelt angelegt oder aus der Spec entfernt. Der andere Fehler,
der die Zeichenfläche im Browser unbrauchbar machte: Die Socket-Ansicht hatte negative Ränder, das
Element, das `RefSocket` registriert, war also null Pixel breit; das Connection-Plugin sucht den
Socket unter dem Zeiger mit `elementsFromPoint`, fand keinen, und ein Ziehen verschob den Knoten,
statt eine Verbindung zu beginnen.

### Dateien

Der Workspace-Ordner ist das Dateisystem des Studios (ADR-0126 Entscheidung 10). `store/fileStore.ts`
hält den Baum (`GET /files`), die gezeigte Datei (Pfad, Inhalt, gespeicherter Inhalt) und die
anderen offenen Datei-Reiter (`stash`, nach Pfad), den Haupt-Reiter (`canvas` oder `file`) und die Verweise der geöffneten Spec (`POST /specs/references`;
ein Abo des Spec-Stores lädt sie kurz nach jeder Änderung der Spec neu). `components/studio/FileTree.tsx`
zeichnet den Baum (reine Helfer in `lib/files.ts`: sichtbare Einträge, die Editor-Sprache, die
Verweise unter einem Pointer, der Verweis, um den es bei einem Problem geht), mit Eingabefeldern für
neue Spec, Datei, Ordner und zum Umbenennen. `components/studio/FileEditor.tsx` ist Monaco, nur im
Browser geladen (`next/dynamic`), mit Strg/Cmd+S als Editor-Befehl; das globale Strg+S speichert die
Datei, solange der Datei-Reiter angezeigt wird, sonst die Spec. Monaco wird nicht von einem CDN
geladen: `scripts/copy-monaco.mjs` kopiert `node_modules/monaco-editor/min/vs` vor `dev` und `build`
nach `public/monaco/vs` (von Git ignoriert), und `loader.config` zeigt dorthin. `FileReferences.tsx`
zeigt die Verweise der Auswahl im Reiter Files des Inspektors (nach JSON-Pointer-Präfix: `/tools/0`,
`/graph/nodes/<i>`, `/graph/edges/<i>`, der Ressourcen-Pointer) mit Open und Create from template;
das Problem-Panel bietet Create from template für ein Problem, dessen Pointer der eines fehlenden
Verweises ist. Anlegen ruft `POST /specs/template` auf und schreibt das Ergebnis mit `PUT /files`.

### Palette-Platzierung

Ein Klick aus der Palette setzt die Ressource in die sichtbare Zeichenfläche
(`placeInView` in `lib/spec-overview.ts` mit `SpecCanvas.visibleRect()`): neben dem Agenten, wenn diese
Stelle sichtbar ist, sonst der erste freie Platz im Bild; nur wenn kein Platz ist, neben dem Agenten,
und die Zeichenfläche scrollt dorthin (`reveal`).

### End-to-End-Prüfung

`studio/web/e2e/studio.e2e.cjs` steuert die Zeichenfläche in einem Headless-Chromium mit echten
Zeigerereignissen (Playwright, keine Abhängigkeit des Frontends): Es startet `grauwolf-studio serve
--root` auf einem leeren temporären Wurzelordner und prüft zuerst die Startseite – einen Workspace aus
jeder Vorlage (im Beispiel-Workspace läuft der Rest), einen registrierten Ordner, in den nichts
geschrieben wird, Umbenennen, Löschen mit getippter ID und das Entfernen eines registrierten
Ordners, das Umschalten zwischen zwei Workspaces mit der Frage nach ungespeicherten Änderungen und
getrennten Dateien und Specs, `?w=` nach einem Neuladen und eine unbekannte ID. Dann prüft es die
Übersicht (Agenten- und Ressourcen-Knoten und ihre
Verbindungen), die benannten Ports (Beschriftungen, Socket-Klassen, ein auf Resources abgelehnter
Skill, beim Ziehen nur der Resources-Port hervorgehoben), eine Paletten-Ressource, die unverbunden
platziert und dann verbunden wird, das Trennen eines Tools (aus der Spec, im Layout geparkt, nicht
validiert), sein erneutes Verbinden mit derselben Konfiguration nach einem Neuladen,
`13-reviewer.json` auf `13-coordinator.json` gezogen und mit Agents verbunden (der
`agent_tools`-Eintrag validiert), die Port-Beschriftungen und hervorgehobenen Ports des Ablaufs, die Reiter von Strategie- und Graph-Specs, den Dialog mit dem abgeleiteten Ablauf und Edit as
graph, dass Layout-Dateien nicht im Baum stehen, die Meldung bei fehlendem Monaco, die gezeichneten Kanten mit
Beschriftung von Graph- und Subgraph-Beispielen, das Ziehen einer Verbindung samt Speichern, das
Löschen von Kanten, das Verschieben von Knoten und das Wiederherstellen der Positionen, ein Ablegen
aus der Palette, dass Kantenbeschriftungen keinen Knoten und keine andere Beschriftung überdecken und
in ihren Hintergrund passen, dass eine gespeicherte Spec ihre Schlüsselreihenfolge behält, dass dem
`code_file` von `03-tool-files.json` nach `03-tools/add.rs` gefolgt wird, das Bearbeiten und
Speichern in Monaco, das Anlegen einer fehlenden Tool-Datei aus der Vorlage der Validierung (die Spec
validiert danach) sowie das Umbenennen und Löschen im Baum. Davor laufen die Prüfungen von Teil 2a gegen einen kleinen
Ollama-Stellvertreter, den das Skript selbst startet (`OLLAMA_HOST` zeigt darauf; die Schlüssel gehen
in ein temporäres `--config-dir`): ein Lauf von `01-hello.json` mit leuchtendem und danach markiertem
Agenten, eine streamende Spec mit leuchtendem laufendem Knoten im Ablauf und der Antwort in Stücken,
ein Freigabe-Zyklus mit einem Shell-Befehl, Stop, die Grenze von zwei Läufen (429 und die Meldung des
Panels), ein Chat mit einer zweiten Nachricht, das Setzen und Entfernen eines Schlüssels, der weder in
der Seite noch in der API erscheint, die Modelle des Stellvertreters im Inspektor, „List tools“ eines
stdio-MCP-Servers, den das Skript schreibt, eine Ressource aus der Palette, die im Bild platziert wird,
während der Agent außer Sicht ist, und die Anordnungsfälle unten. Echte Beispiel-Specs laufen im Reiter Flow, die
Seite zeichnet auf, wie die Knoten aufleuchten (ein `MutationObserver` auf `data-run`), und vergleicht das
mit den `node_started`-Ereignissen: `04-decide.json` für ein Billing- und ein Technik-Ticket (der
Stellvertreter beantwortet die Entscheidung anhand des Ticket-Textes, auf dem Billing-Weg leuchtet
`diagnose` nicht), `04-tool-loop.json` (Werkzeugaufruf und Ergebnis im Panel) und `06-subgraph.json` mit
den Knoten `<subgraph>/<id>` (sein `assess`-Modell ist durch das des Stellvertreters ersetzt; unverändert
schlägt der Lauf sofort fehl und nennt `ANTHROPIC_API_KEY`).
`E2E_ONLY=part2a` bricht danach ab. `E2E_ONLY=planner` legt den Workspace über die API an und führt nur die
Planer-Szenarien von Entscheidung 15 aus: die gestrichelten Verbindungen von `06-goap.json` mit ① ② ③ (und
keine zusätzliche Verbindung), den Hover mit Vorbedingungen, Wirkungen und Kosten, einen Lauf gegen den
Stellvertreter (das `check`-Modell ersetzt), der die Verbindungen in der Reihenfolge write_rough_draft →
revise → check_facts aufleuchten lässt und mit drei ✓ endet, und die eine Verbindung von `06-plan.json`
ohne Nummern (Screenshots `planner-goap-flow`, `planner-goap-run`, `planner-plan-flow`). `ideChecks`
(Entscheidung 16; allein mit `E2E_ONLY=ide`) prüft den Rahmen: eine Spec und `add.rs` als zwei Reiter,
● nach einer Änderung, die Frage beim Schließen (Abbrechen behält den Reiter, OK verwirft), Overview /
Flow / JSON mit einer JSON-Änderung, die in der Übersicht sichtbar wird, und ungültigem JSON unter
PROBLEMS, die Reiter des Inspektors, PROBLEMS, das einen Knoten im Ablauf auswählt, EVENTS und TOOLS
während eines Laufs von `04-tool-loop.json`, Einklappen und Höhe des Panels, die Statusleiste (valid,
Policy, Lock, *ollama reachable* vom Stellvertreter, der Lauf), die Befehlspalette (Strg+K, Filter,
*Add MCP server to Resources*, *Go to node…*, Strg+P, Tab bleibt drinnen, Esc) und die Suche
(Screenshots `ide-overview`, `ide-json`, `ide-flow-run`, `ide-palette`). `mcpChecks` (Entscheidung
17; allein mit `E2E_ONLY=mcp`) steuert das Studio mit einem kleinen MCP-Client im Skript
(`mcpClient`, schlichtes JSON-RPC über HTTP): der Verbindungsdialog mit verborgenem Token und einem
Befehl, dessen URL und Token funktionieren, `401` ohne oder mit falschem Token, *MCP: connected
(claude-code)* in der Statusleiste, ein Knoten und ein MCP-Server, die der Client hinzufügt und die
ohne Neuladen in Ablauf und Übersicht erscheinen, aufleuchtend, mit EVENTS-Zeilen, der Benutzer, der
einen Knoten zieht, während der Client ein Feld setzt (beides in der Arbeitskopie, die Auswahl
bleibt, Inspektor und JSON-Ansicht folgen), Save, das das zusammengeführte Ergebnis schreibt, und ein
`run_spec`, dessen Lauf in RUN erscheint (Screenshots `mcp-connect`, `mcp-live-edit`, auch nach
`PUBLISHED_SHOTS` kopiert). `undoChecks` (Entscheidung 18; allein mit `E2E_ONLY=undo`) führt die vom
Eigentümer freigegebenen Undo-Szenarien aus: ein Ziehen, mit Strg+Z zurückgenommen und mit
Strg+Umschalt+Z wiederholt (der Platz des Knotens auf dem Bildschirm, der Tooltip der Werkzeugleiste),
eine gezeichnete Verbindung zurückgenommen, eine Palettenressource zurückgenommen und – einmal
verbunden – erst ihre Verbindung, dann sie selbst, Tippen in `system_prompt` in einem Schritt
zurückgenommen, ein Knoten des MCP-Clients, den Strg+Z stehen lässt und dessen EVENTS-**Undo** ihn
entfernt (die Zeile zeigt *undone*), eine Verschiebung, zurückgenommen, während der Client die Params
des Knotens setzte (die Params bleiben), und Rückgängig nach Save (Screenshot `undo-events`, auch nach
`PUBLISHED_SHOTS` kopiert). `layoutChecks` (Entscheidung 19; allein mit `E2E_ONLY=layout`) führt die vom
Eigentümer freigegebenen Anordnungsfälle aus: Kopien von `04-tool-loop`, `04-decide`, `06-subgraph`,
`06-goap` (Ablauf) und `08-coordinator` (Übersicht) ohne Layout-Datei werden geöffnet und die Geometrie
der Seite selbst geprüft – jeder Pfad von Kanten und Planer-Verbindungen gegen die Knotenboxen
abgetastet, jede Beschriftung gegen die Knoten und die anderen Beschriftungen, der Zoom nach dem
Einpassen mindestens 0,6 –; dann ein gezogener Knoten (die Kanten führen um ihn herum), Tidy up mit
⇧⌥F als ein Rückgängig-Schritt, ⌘Z, das die gezogene Position zurückbringt, und Tidy up auf der
Übersicht (Screenshots `layout-04-tool-loop`, `layout-04-decide`, `layout-06-subgraph`,
`layout-06-goap`, `layout-overview`). Die übrigen Prüfungen
erreichen Explorer und Palette der Seitenleiste, die Reiter von Inspektor und Panel über Helfer
(`sidebar`, `paletteClick`, `paletteDrag`, `inspectorTab`, `panelTab`, `viewTab`); `open` schließt
vorher alle Reiter, jede Prüfung beginnt also mit der Datei auf der Platte. Screenshots landen in
`SHOTS`. Tool-Code wird mit `cargo` gebaut, es muss also im `PATH` des Skripts sein.
Starte es nach `npm run build`, mit Playwright an anderer Stelle installiert:

```bash
STUDIO_BIN=$CARGO_TARGET_DIR/debug/grauwolf-studio STUDIO_WEB=studio/web/out \
EXAMPLES=docs/user-guide/examples NODE_PATH=<ordner>/node_modules node studio/web/e2e/studio.e2e.cjs
```

## Der Rahmen und die Befehlsregistrierung (Entscheidung 16)

`app/EditorApp.tsx` setzt den Rahmen zusammen: `StudioToolbar` (die Kopfzeile mit
`WorkspaceSwitcher`), `ActivityBar` und `SideBar` (Explorer = `FileTree`, Palette = `Palette`,
Search = `SearchView`), `EditorTabs` (die offenen Dateien und der Schalter Overview / Flow / JSON),
die Zeichenfläche – mit `SpecSource` darüber für die JSON-Ansicht – oder `FileEditor`, `Inspector`
(Reiter Properties, Files, JSON), `BottomPanel` (RUN = `RunPanel`, das eingehängt bleibt, EVENTS,
PROBLEMS = `ProblemsPanel`, TOOLS), `StatusBar` und `CommandPalette`.

- **Zustand des Rahmens:** `store/layoutStore.ts` – die Seitenansicht und ob die Seitenleiste offen
  ist, Reiter, Höhe (in localStorage gemerkt) und Offen-Zustand des Panels, der Reiter des Inspektors
  und die Palette (offen, und der Auswahlbefehl, mit dem sie öffnet).
- **Reiter:** Der Spec-Store hält die gezeigte Spec und die anderen offenen Specs in `stash` (ein
  `SpecSnapshot`: Spec, Layout, dirty, Probleme, Auswahl, Ansicht, JSON-Ansicht …); `open(path)` legt
  die gezeigte ab und holt eine abgelegte zurück, ohne die Datei neu zu lesen. Der Datei-Store macht
  dasselbe für Dateien. `store/tabsStore.ts` hält Reihenfolge und aktiven Reiter und folgt beiden
  Stores mit `syncTabs` (`lib/tabs.ts`, rein, getestet: öffnen, schließen mit dem Nachbarn als neuem
  aktiven Reiter, die Ungespeichert-Markierung, Umbenennen an derselben Stelle), so bekommt eine Spec
  oder Datei ihren Reiter, egal von wo sie geöffnet wird; `close` fragt mit `window.confirm`, wenn der
  Reiter ungespeichert ist.
- **JSON-Ansicht:** `source` im Spec-Store schaltet sie ein; `lib/spec-source.ts` (rein, getestet)
  schreibt den Text (`specText`), liest ihn mit der Stelle eines Fehlers (`parseSpecText`,
  `errorOffset`), behält den Text des Nutzers, solange er dasselbe sagt oder nicht lesbar ist
  (`syncedText`), und findet die Zeile eines JSON-Pointers (`pointerLine`) für „JSON am Pointer
  öffnen“ aus PROBLEMS (`revealSource`). Eine Änderung ruft 300 ms nach dem Tippen `replaceSpec` auf;
  ungültiger Text setzt `sourceError`, den PROBLEMS und die Statusleiste zählen.
- **Auswahl:** `lib/selection.ts` liefert JSON-Pointer und JSON der Auswahl für die Reiter Files und
  JSON des Inspektors.
- **Ereignisse:** `applyRunEvent(view, event, now)` faltet auch die Zeilen von EVENTS
  (`RunView.events`, Startzeiten in `started`); ein Tool-Ergebnis vervollständigt die Zeile seines
  Aufrufs (`⇢ translator ✓ 1.1 s`).
- **Suche:** `lib/search.ts` (`indexSpec`, `searchWorkspace`, rein, getestet); `SearchView` liest jede
  Spec des Workspaces einmal und nimmt die offenen, wie sie gerade sind.
- **Statusleiste:** Das Modell-Backend wird über `GET /discovery/models?provider=` des Anbieters der
  gezeigten Spec gefragt, einmal je Anbieterwechsel und eine Minute zwischengespeichert – nie im
  Takt abgefragt.

**Befehlsregistrierung.** `lib/commands.ts` hält jeden Befehl: `{ id, title, keywords?, shortcut?,
alternatives?, enabled?, run? | items? }` (`alternatives` sind weitere, nicht angezeigte Kürzel, etwa
Strg+Y für *Redo*; `matchesCommand` prüft alle). Ein Befehl mit `items` ist eine Auswahl: Die Palette zeigt seine Einträge
als zweiten Schritt (*Go to node…*). `registerCommands` fügt Befehle hinzu und gibt die Funktion
zurück, die sie wieder entfernt; `filterCommands`/`fuzzyFilter` ordnen nach einer unscharfen
Teilfolgen-Bewertung (aufeinanderfolgende Buchstaben und Wortanfänge zählen mehr); `matchesShortcut`
liest `Mod+K` als Strg oder ⌘. Die Befehle des Studios stehen in
`components/studio/studio-commands.ts` und werden registriert, wenn `EditorApp` eingehängt wird; der
eine Tasten-Handler des Rahmens öffnet die Palette mit Strg/⌘+K und führt das Kürzel jedes
registrierten Befehls aus (in einem Textfeld nur Auswahlbefehle wie Strg+P). Ein neues Feature fügt
seine Befehle mit `registerCommands` hinzu – Paletteneintrag und Kürzel kommen mit:

```ts
useEffect(() => registerCommands([{ id: "trace.open", title: "Open the trace…", items: traceItems }]), []);
```

## Das Backend

`grauwolf-studio` ist eine App-Crate (der Architekturtest kennt sie als `arch::STUDIO`: Sie darf Host
und Server nutzen, und keine App-Crate hängt von ihr ab). Module:

| Modul | Inhalt |
|---|---|
| `lib.rs` | `StudioConfig`, `router` (die API unter `/api/v1`, der statische Export mit SPA-Rückfall), `serve` |
| `api.rs` | die Handler und die OpenAPI-Beschreibung (`ApiDoc`, utoipa) |
| `workspace.rs` | ein Workspace-Ordner: Pfadprüfung, Auflisten, Lesen, atomares Schreiben |
| `workspaces.rs` | Wurzelordner und Registry (oder Single-Modus): Auflisten, Anlegen aus einer Vorlage, Registrieren, Umbenennen, Löschen, `open(id)` |
| `ordered.rs` | `OrderedJson`, ein JSON-Wert, der die Schlüsselreihenfolge einer Spec-Datei behält |
| `files.rs` | die Datei-API: Baum mit Rollen, Lesen (Text bis 1 MiB), Schreiben, Umbenennen, neuer Ordner, Löschen |
| `references.rs` | die Dateien, auf die eine Spec verweist, aus `grauwolf_agent_tree::file_references` |
| `templates.rs` | die Vorlage für eine fehlende referenzierte Datei |
| `layout.rs` | das Layout-Format, Version 1 (mit dem optionalen `overview` samt `parked`) |
| `check.rs` | `load` und `validate` auf einer Spec im Speicher; Pointer → Knoten, Kante, Subgraph |
| `flow.rs` | der Ablauf einer Spec und ihre Umwandlung in einen Graphen |
| `palette.rs` | `/node-kinds` aus den Katalogen |
| `error.rs` | `ApiError`, die eine Fehlerform |
| `runs.rs` | Vorschau-Läufe: starten, Ereignisse, Budgets, Freigaben, Chat-Turns, Stop (Teil 2a) |
| `keys.rs` | die API-Schlüssel der Anbieter in `keys.json` (Teil 2a) |
| `discovery.rs` | Modelllisten und die Tools eines MCP-Servers (Teil 2a) |
| `run_api.rs` | die Routen von Läufen, Schlüsseln und Discovery |
| `ops.rs` | die Operationen der Arbeitskopie: Anwenden mit Umkehrung, Zusammensetzen, Transformieren (Konfliktregeln), Diff – rein |
| `documents.rs`, `document_api.rs` | die Arbeitskopie: offene Dokumente, Versionen, Stapel, Speichern, Zurücksetzen, die Plattenprüfung, der Ereignisstrom |
| `mcp.rs`, `mcp_tools.rs` | der MCP-Endpunkt `/mcp` (Token, Sitzungen, Kontext) und seine Tools |

Die Validierung folgt `grauwolf spec validate`, aber ohne Agentenbaum: Die Dateiverweise werden mit
`grauwolf_agent_tree::resolve_file_references` relativ zum Ordner der Spec aufgelöst, dann folgen
`AgentSpec::from_json`, `validate()`, die bekannten Anbieter (`grauwolf_host::providers::BUILTIN`)
und die Warnungen. Der Knoten zu einem Pointer wird im rohen JSON nachgeschlagen, also auch dann
gefunden, wenn die Spec sich nicht laden lässt.

**Dateien und Verweise.** Die Datei-API nutzt die Pfadregeln der Specs (relativ, kein `..`, innerhalb
des Workspaces auch über symbolische Links) plus kein verstecktes Segment. Die Verweise einer Spec
sind im Studio keine Feldliste: `grauwolf_agent_tree::file_references` durchläuft die Spec mit
demselben Code wie `resolve_file_references` des Lesers (`walk_code_files` mit einem Visitor: der
Auflöser fügt jede Datei ein, der Auflister erfasst sie und setzt leeren Code an ihre Stelle), und –
wenn die Spec sich dann lädt – ergänzt `tools_dir`, lokale Sub-Agenten und
`grauwolf_generator::companions` (die Begleitpfade eines Bildes, jetzt mit JSON-Pointer und Art jedes
Feldes). Das Studio ergänzt den Workspace-Pfad, ob die Datei existiert und die Vorlage, die sie
anlegen kann. Die Vorlagen (`templates.rs`) bauen: `tests/templates.rs` erzeugt jede Art für eine
Spec, validiert sie, und ein langsamer Test baut alle Tools und Code-Teile mit `grauwolf-toolbuild`.

**Schlüsselreihenfolge.** `serde_json::Value` sortiert Objektschlüssel. Das Studio liest und schreibt
Specs als `OrderedJson` und wandelt nur für den Lader von grauwolf in `Value` um. Das Feature
`preserve_order` von serde_json wurde nicht genutzt: Cargo vereinheitlicht Features über alles, was
zusammen gebaut wird, es hätte also die Map-Reihenfolge jeder grauwolf-Crate in einem Workspace-Build
verändert.

Die Palette wird abgeleitet, nicht kopiert: Katalogarten kommen mit
`NodeKindDescriptor::description()`, `params_schema()`, `example_params()` und der Signatur der
Beispielparameter als Porttypen; Bedingungen melden, welche Typen sie in welche verwandeln. Die
Arten außerhalb der Kataloge kommen aus `grauwolf_spec::STRUCTURAL_NODE_KINDS` und
`STRUCTURAL_CONDITION_KINDS`. Eine neue Knotenart erscheint also im Studio, sobald sie mit
Beschreibung im Katalog steht.

## Läufe, Schlüssel und Discovery (Teil 2a)

ADR-0126 Entscheidung 13. Drei Module des Backends, ihre Routen in `run_api.rs`:

| Modul | Inhalt |
|---|---|
| `runs.rs` | `Runs`: Start, Liste, Stop, Chat-Turns, Freigaben; die Ereignisse eines Laufs; Budgets und die Grenze gleichzeitiger Läufe |
| `keys.rs` | `KeyStore`: die API-Schlüssel der Anbieter in `keys.json` |
| `discovery.rs` | Modelllisten der Anbieter, die Tools eines MCP-Servers |

**Ein Lauf** lädt die *gespeicherte* Spec-Datei so, wie `grauwolf spec run` es tut – `AgentLoader` mit
einem `ToolBuilder` auf `<workspace>/.grauwolf/tools`, `load_provisioned`, dann
`InterpretedAgent::run_with` mit den `RunExtras` von `provisioned.run(None).extras()` plus:

- ein `Feature`, das die Ereignisse des Agenten in Lauf-Ereignisse umsetzt und die Schritte zählt;
- die Frame-Senke (Text-Deltas – nur `llm_request_streaming`-Knoten streamen);
- ein Freigeber für `AgentSpec::approval_tools()`, der über die Ereignisliste fragt und auf
  `POST …/approvals/{id}` wartet;
- die Anhänge und, für einen Chat, die Unterhaltung (`MemoryChatHistoryStore`, geschlüsselt nach dem
  Lauf, oder mit `conversation` ein `FileChatHistoryStore` in `<workspace>/.grauwolf/conversations`).

Der Lader bekommt eine eigene `Env`: zuerst die gespeicherten Schlüssel, dann `GRAUWOLF_TOOL_POLICY` =
die `grauwolf.toml` des Workspace (falls vorhanden), dann `StudioConfig::env` – die Prozessumgebung
oder in Tests eine `MapEnv`. Nichts wird in die Prozessumgebung geschrieben, sodass zwei Läufe nie die
Schlüssel des anderen sehen. `say_to_user` wird zu einer `notice`, `ask_user` wird abgelehnt.

Jeder Turn läuft in einer eigenen Task; eine Supervisor-Task wartet auf sie, auf einen Abbruch (Stop
oder die im Feature überschrittene Schrittzahl) und auf die Echtzeit, und bricht sie ab – der Agent,
seine Tools und MCP-Server fallen mit ihr weg. Budget pro Turn: Schritte (Knotenstarts, vom Feature
gezählt), Tokens (das `Budget` des Interpreters, über `AgentLoader::with_budget`; das niedrigere
`limits.max_tokens` der Spec gewinnt) und Sekunden; Standardwerte `DEFAULT_BUDGET` (100 / 200000 / 300),
höchstens `MAX_BUDGET` (1000 / 2000000 / 3600). Höchstens `MAX_ACTIVE_RUNS` = 2 Turns laufen gleichzeitig
(`429 too_many_runs`); untätige Chat-Läufe zählen nicht. `KEPT_RUNS` = 20 Läufe werden aufgelistet;
ältere, die nicht laufen, werden verworfen.

**Ereignisse.** Jeder Lauf hält seine Ereignisse in einer Liste; `GET …/runs/{id}/events` sendet sie als
SSE ab dem ersten (oder `?from=`, oder nach `Last-Event-ID`) und danach, sowie sie kommen, bis
`finished`. Der SSE-Ereignisname ist der `type`, die Daten `{ seq, type, … }`, die SSE-ID `seq`:

| Typ | Felder |
|---|---|
| `turn_started` | `turn`, `input` |
| `loaded` | `name` – der Agent ist bereitgestellt (erster Turn) |
| `node_started`, `node_finished` | `node`, `subgraph?`, `path` |
| `plan_step` | `node`, `subgraph?`, `path` des Planer-Knotens, `iteration`, `action?`, `action_subgraph?`, `step?` |
| `llm_started` | `model` (`provider:id`) |
| `text` | `text` – ein gestreamtes Stück |
| `tool_call` | `id`, `tool`, `args` (länger als 4000 Zeichen: ein gekürzter JSON-Text) |
| `tool_result` | `id`, `tool`, `content` (auf 4000 Zeichen gekürzt), `truncated` |
| `approval`, `approval_decided` | `id`, `tool`, `args`; `id`, `approved` |
| `usage` | `input_tokens`, `output_tokens`, `cost?` – der Lauf bisher |
| `notice` | `message` – `say_to_user`, Ausweichmodelle, Entscheidungen, übersprungene Tool-Dateien |
| `output`, `error` | `output`; `message` |
| `turn_finished`, `finished` | `turn`, `status`; `status` |

`node` und `subgraph` passen zur Zeichenfläche: Das Frontend schlüsselt einen Knoten als
`<subgraph>/<id>`. Das Backend ermittelt den Subgraphen aus dem Pfad (`runs::subgraph_of`: jeder Knoten
vor dem letzten nennt seinen Subgraphen in `params.subgraph`, oder ist ein Zweig, der wie einer heißt).
`plan_step` kommt aus dem `PlanBuilt` des Planers (Entscheidung 15): Das Feature hält einen Stapel der
laufenden Planer-Knoten (abgelegt bei ihrem `NodeStarting`, entfernt bei ihrem `NodeCompleted`) und nennt
den innersten; bei GOAP ist `action` der erste Name des Plans, bei `plan` sein Schritt-Subgraph und `step`
die Beschreibung des ersten offenen Schritts. Der Interpreter wird dafür nicht geändert.
Das Frontend faltet die Ereignisse mit `lib/run-events.ts` (`applyRunEvent`, rein und getestet) in den
Verlauf, den Verbrauch und die aktiven und fertigen Knoten, die `SpecCanvas` an den Knoten markiert
(`SpecReteNode.run`). Ein `turn_started` löscht die Markierungen (`done`, `runs`), sodass jeder Zug
eines Chat-Laufs seinen eigenen Weg zeigt; der Agenten-Knoten trägt die Zahl der markierten Knoten als
`data-marks`, das das e2e-Skript liest (der Ablauf einer Strategie wird nicht gezeichnet). Wenn ein Lauf
beginnt und wenn ein Zug oder der Lauf endet, passt `SpecCanvas` den Ablauf ein (`fit(…, readable:
false)`, der ganze Ablauf bei jedem Zoom), falls `anyOutside` (`lib/view-fit.ts`) einen Knoten außerhalb
des sichtbaren Teils findet – nie dazwischen und nicht, solange eine Maustaste gedrückt ist, sodass ein
Verschieben während des Laufs nicht rückgängig gemacht wird.

**Läufe je Spec (Anmerkung zu Entscheidung 16).** `lib/spec-runs.ts` (rein, getestet in
`spec-runs.test.ts`) ist das Modell: ein `RunRecord` je Lauf (`RunView`, Eingabe, Start und Ende, `n` =
seine Nummer in der Spec) in `SpecRuns { records, order, selected, dropped }`; `order` listet die
Lauf-IDs je Spec-Schlüssel (`specKey(workspace, path)` – nicht der Reiter –, neueste zuerst),
`selected` ist der in der Historie gewählte Lauf. `shownRun(state, key)` ist der gewählte Lauf, sonst
der neueste; `startRun`/`addRun` behalten die letzten `HISTORY_LIMIT` (10) Läufe je Spec und werfen nie
einen laufenden weg; `applyEvent` faltet ein Ereignis in seinen Record; `runningRuns`/`specRunning`
speisen die Statusleiste und den Lauf-Punkt; `mergeListed` fügt die Läufe aus `GET …/runs` für einen
Pfad, die das Studio nicht kennt (nach einem Neuladen), als noch nicht `loaded` hinzu.
`store/runStore.ts` umhüllt es: `start(path, input)`, `send`/`stop`/`decide(path, …)` wirken auf den
für `path` gezeigten Lauf, `select(path, id)`, `loadHistory(path)` (ruft `RunPanel`, wenn die gezeigte
Spec wechselt), `reset()` beim Workspace-Wechsel. Je Lauf lebt ein `EventSource` in einer Modul-Map bis
`finished` (oder bis der Lauf die Historie verlässt), unabhängig vom Panel und vom gezeigten Reiter –
so läuft ein Lauf im Hintergrund weiter. Ein gelisteter Lauf wird erst beim Auswählen verfolgt (seine
Ereignisse werden wiedergegeben). Die Komponenten lesen `useShownRun()` (der Lauf der offenen Spec;
`NO_VIEW`, solange es keinen gibt), daher brauchen `RunPanel`, `BottomPanel`, `SpecCanvas`
(Markierungen, Planerschritte) und `StatusBar` keinen eigenen Lauf. Der Reiter zeigt den Lauf-Punkt
(`specRunning`) und die Zahl der Validierungsfehler der Spec (`problems` der gezeigten Spec, die des
Stashs für die anderen). Keine Backend-Änderung: Das Backend behält die letzten 20 Läufe des ganzen
Workspaces (`KEPT_RUNS`), deshalb merkt sich das Studio selbst die letzten 10 je Spec.
`E2E_ONLY=runs-per-spec` führt nur die freigegebenen Szenarien aus (`runsPerSpecChecks`). Der
`@/`-Alias für vitest steht in `vitest.config.mts`, damit Stores testbar sind
(`store/runStore.test.ts`, mit einem falschen `EventSource`).

**Schlüssel.** `KeyStore` liest `<config dir>/keys.json` bei jeder Nutzung und schreibt sie über eine
temporäre Datei, die mit Modus 0600 angelegt und dann umbenannt wird; der Ordner bekommt 0700. Der
Konfigurationsordner ist `--config-dir`, sonst `GRAUWOLF_STUDIO_CONFIG_DIR`, sonst
`~/.config/grauwolf/studio`. Werte werden nie zurückgegeben, protokolliert oder in eine Fehlermeldung
gesetzt (eine kaputte Datei wird mit Zeile und Spalte gemeldet, nicht mit ihrem Inhalt).

**Discovery.** `discovery::models` fragt Ollama (`/api/tags` unter `providers::ollama_host`),
OpenAI-kompatible APIs (`<base url>/models`), Anthropic und Gemini mit dem Schlüssel der Umgebung des
Laufs; sonst eine feste Liste mit einem `note`. `discovery::mcp_tools` nutzt
`grauwolf_host::mcp::list_tools`, den Verbindungscode eines Laufs, mit einem Timeout von 20 Sekunden.

**Tests** rufen nie ein Modell auf: `tests/common/stub.rs` ist ein kleiner axum-Server, der Ollamas
`/api/chat` spricht (gestreamt und nicht, Text oder ein Tool-Aufruf oder gar keine Antwort),
`/api/tags` und ein OpenAI-artiges `/v1/models`; das Studio bekommt seine Adresse über
`StudioConfig::with_env`. `common::events` liest einen SSE-Strom Ereignis für Ereignis.
`tests/runs.rs`, `tests/keys.rs` und `tests/discovery.rs` decken die Reihenfolge der Ereignisse,
Freigaben, Stop, die Grenze von zwei Läufen, die Budgets, Chat-Turns, die Schlüsseldatei und die
Modelllisten ab; das e2e-Skript prüft dasselbe im Browser gegen einen Stub in Node.

## Die Arbeitskopie und der MCP-Endpunkt (Entscheidung 17)

Das Studio ist ein MCP-Editor: Ein Coding-Agent und der Browser bearbeiten dieselben Specs zur selben
Zeit. Dafür hält das Backend die **Arbeitskopie** – es, nicht der Browser, besitzt den ungespeicherten
Zustand jeder offenen Spec.

### Dokumente

`documents::Documents` hält je Workspace-ID und Spec-Pfad ein Dokument: den Inhalt
`{ "spec": …, "layout": … }` (die Spec als `OrderedJson` in ihrer Schlüsselreihenfolge, das Layout wie
gespeichert oder `null`), eine Version, den Inhalt so, wie die Datei ihn hat (für `dirty`, verglichen
ohne Rücksicht auf die Schlüsselreihenfolge – wer einen Wert zurückändert, macht das Dokument wieder
sauber), und Änderungszeit und Größe der Spec- und der Layout-Datei. Jedes Öffnen, jeder Stapel und
jedes Speichern vergleicht sie mit der Platte: Ein sauberes Dokument, dessen Datei sich geändert hat
(ein `git checkout`), wird neu geladen und als `reverted` mit Herkunft `disk` gesendet; ein
geändertes wird als `disk_changed` markiert, behält die ungespeicherte Arbeit, und sein Speichern wird
abgelehnt (`409 disk_changed`), bis es zurückgesetzt oder mit `force` gespeichert wird. `PUT /specs`,
die Dateirouten und die Datei-Tools von MCP melden den Dokumenten, wenn sie eine Datei geschrieben,
umbenannt oder gelöscht haben.

### Operationen

Jede Änderung ist ein Stapel von **Operationen** auf JSON-Pointern in das Dokument (`ops.rs`; das
Frontend hat dasselbe Modell in `lib/doc-ops.ts`):

| Operation | Wirkung | Umkehrung |
|---|---|---|
| `set { path, value, at? }` | ersetzt einen Wert oder fügt ein Objektfeld hinzu (am Ende oder an `at`) | `set` des alten Werts, oder `remove` |
| `insert { path, value }` | fügt vor dem Index in eine Liste ein (`-` hängt an) | `remove` dieses Index |
| `remove { path }` | entfernt ein Listenelement oder ein Objektfeld | `insert`, oder `set` mit `at` – das Feld kommt an seine Stelle zurück |
| `move { from, path }` | verschiebt einen Wert (innerhalb eines Objekts eine Umbenennung an Ort und Stelle) | die Verschiebung zurück (samt dem Wert, den sie ersetzt hat) |

`apply` liefert die Umkehr-Operationen, `apply_all` wendet einen Stapel ganz oder gar nicht an (ein
Fehler nennt `ops/<index>` und den Pointer), `compose` fügt Stapel zusammen, `diff(old, new)` liefert
die Operationen zwischen zwei Zuständen – Objekte nach Schlüssel; Listen über gemeinsamen Anfang und
gemeinsames Ende, dann Element für Element, wenn die Elemente an ihrer Stelle geändert wurden, sonst
über eine längste gemeinsame Teilfolge (Entfernen von hinten, Einfügen von vorn). Die Umkehrungen
kommen mit jedem Stapel und jedem Ereignis zurück; Rückgängig und Wiederholen sind noch nicht gebaut,
brauchen aber nichts weiter.

**Nebenläufigkeit.** Ein Stapel nennt seine `base_version`. Stapel anderer Clients, die danach
angewendet wurden, sind nebenläufig, und der Stapel wird mit `ops::transform` über sie hinweg
verschoben:

- Indizes verschieben sich durch ein Einfügen oder Entfernen in derselben Liste (zwei Einfügungen am
  selben Index: die zuerst angewendete bleibt vorn; eine Einfügung am Index eines entfernten
  Elements bleibt dort);
- eine Operation, deren Ziel eine nebenläufige entfernt hat, **entfällt**;
- eine Operation auf demselben Pointer wie ein nebenläufiges `set`, ein entferntes Objektfeld oder
  eine Verschiebung – oder auf einem Pointer darin oder darum – wird angewendet und **überschreibt**
  sie: Wer zuletzt schreibt, gewinnt;
- alles andere ist vertauschbar.

Entfallene und überschreibende Operationen stehen als `conflicts` (Index, Pointer, Ausgang, die
andere Herkunft) in der Antwort und im Ereignis. Die eigenen früheren Stapel eines Clients sind nicht
nebenläufig – er hat auf ihnen aufgebaut –, also darf der Browser einen Stapel senden, bevor die
Antwort auf den vorigen da ist. Ein Stapel, der älter ist als das letzte Laden oder Zurücksetzen oder
als die 1000 Stapel, die ein Dokument behält, ist `409 stale_base`: Der Client öffnet das Dokument
neu.

### Routen

| Route | Body | Antwort |
|---|---|---|
| `POST /workspaces/{id}/documents/{path}:open` | – | `DocumentState { path, spec, layout, layout_problem?, version, dirty, disk_changed }` |
| `POST /workspaces/{id}/documents/{path}/ops` | `{ base_version, ops, client_id }` | `{ version, dirty, ops (wie angewendet), inverse, conflicts }` |
| `POST /workspaces/{id}/documents/{path}:save` | optional `{ force }` | der Zustand (`409 disk_changed`) |
| `POST /workspaces/{id}/documents/{path}:revert` | – | der aus der Datei geladene Zustand |
| `GET /workspaces/{id}/documents/events` | – | SSE: `ops`, `saved`, `reverted`, `disk_changed`, `closed`, `clients`, `files`, `run`, `resync` |

Ein `ops`-Ereignis trägt die Version, die Operationen und ihre Umkehrung, die `client`-ID des
Absenders, die `origin` (`studio` oder `mcp:<client name>`), `dirty`, die Konflikte und – von
MCP-Tools – ein `label` (`add_node ask (llm_request)`) und ein `highlight` (Knotenschlüssel
`<subgraph>/<id>`, Ressourcenschlüssel). Der Strom ist ein `tokio::sync::broadcast` von 1024
Ereignissen; ein Abonnent, der zurückfällt, bekommt `resync`. `GET`/`PUT /specs/{path}` bleiben und
arbeiten auf der Datei.

### Das Frontend auf der Arbeitskopie

`store/documentStore.ts` hält je offenem Dokument einen **Spiegel** (`lib/doc-mirror.ts`, rein): den
bestätigten Zustand mit seiner Version, die ausstehenden lokalen Stapel und `local` = bestätigt +
ausstehend, was der Spec-Store zeigt. Die Aktionen des Spec-Stores sind unverändert: Sie berechnen
weiter mit den reinen Funktionen aus `lib/spec-canvas.ts` und `lib/spec-overview.ts` eine neue Spec
und ein neues Layout, und ein Abonnement macht aus jeder solchen Änderung Operationen, als Diff gegen
den Spiegel (`/spec`, und `/layout` nur, wenn die Zeichenfläche es geändert hat, denn dem Layout des
Dokuments können Positionen fehlen, die die Zeichenfläche berechnet hat). Stapel gehen einzeln
hinaus; Änderungen in der Zwischenzeit werden zum nächsten Stapel zusammengefügt. Ein Stapel eines
anderen Clients schiebt den bestätigten Zustand weiter, und die ausstehenden Stapel werden mit den
Regeln des Backends darüber verschoben (`transformAll`), sodass beide bleiben; der eigene Stapel wird
durch seine Antwort oder sein Ereignis bestätigt, je nachdem, was zuerst kommt, und Ereignisse, die
vor einer fehlenden Version eintreffen, warten. Der Spec-Store übernimmt dann die Spec des Spiegels
und ein Layout aus `lib/doc-layout.ts` – die Positionen des Dokuments, ergänzt um die Plätze, die die
Zeichenfläche schon zeigt, und dann automatisch –, ohne die Auswahl anzurühren. Die JSON-Ansicht
führt eine entfernte Änderung mit `rebasedText`/`mergeEdited` (`lib/spec-source.ts`) in den Text ein:
die eigenen Änderungen am Text bleiben. Die Ungespeichert-Markierung kommt aus dem Spiegel, Speichern
und Zurücksetzen gehen ans Backend; das Schließen eines geänderten Reiters setzt das Dokument zurück.

### Rückgängig und Wiederholen (Entscheidung 18)

Das Undo-Modell ist `lib/undo.ts` (rein, getestet in `lib/undo.test.ts`); `store/documentStore.ts`
hält eine `History` pro offenem Dokument. Ein **Schritt** ist ein Label und die inversen Operationen
einer Benutzeraktion, geschrieben gegen `local` des Spiegels – das Dokument, wie es gezeigt wird. Im
Backend hat sich nichts geändert: Ein Rückgängig ist eine gewöhnliche lokale Bearbeitung, gesendet als
neuer Batch auf der aktuellen Version.

- **Aufzeichnen.** Die Aktionen des Spec-Stores benennen ihre Änderung über `asEdit({ label, group,
  window })`; das Abonnement, das eine Änderung in Operationen verwandelt, berechnet auch ihre Inverse
  – die Spec zurück zu der des Spiegels, das Layout zurück zu dem, was die Zeichenfläche vorher zeigte
  (nicht zum Layout des Dokuments, dem berechnete Positionen fehlen können) – und gibt beides an
  `editDocument`. Ohne Label wird der Schritt nach den Operationen benannt ("Edit system_prompt");
  `track: false` kennzeichnet eine Bearbeitung, die keine Aktion ist (Save vervollständigt das Layout).
- **Gruppieren.** Eine Bearbeitung verschmilzt mit dem obersten Schritt, wenn sie dieselbe `group`
  hat, der Schritt nicht versiegelt ist und seine letzte Bearbeitung jünger als `window` ist: Felder
  (`field:<pointer>`, `params:…`, `edge:…`) nutzen 1 s, ein Ziehen (`move:…`) und die JSON-Ansicht
  (`json`) `Infinity`. `sealUndo` beendet einen Schritt; der Rahmen ruft es bei jedem `focusout` und
  `pointerup` auf (in der Capture-Phase), und die JSON-Ansicht wendet eine wartende Bearbeitung an,
  bevor sie beim Verlassen versiegelt. Jede Aktion ist ein Aufruf des Stores, also sind *Convert to
  graph* oder ein Verbinden mit vorgeschlagener Bedingung schon ein Schritt.
- **Rückgängig und Wiederholen** (`stepDocument`): Der oberste Schritt wird als lokale Bearbeitung auf
  den Spiegel angewendet; der Unterschied zurück (`diff` der beiden `local`-Zustände, dasselbe wie die
  Inverse des Backends) kommt auf den anderen Stapel, und der Spec-Store folgt mit der Änderungsart
  `undo`, die `revision` erhöht, damit die Felder des Inspektors neu einhängen. Eine neue eigene
  Aktion leert den Redo-Stapel; jeder Stapel hält 200.
- **Die gespeicherten Inversen rebasen.** Das Backend rebast einen Batch nur über die Batches
  *anderer* Clients seit seiner Basis, nie über die eigenen des Senders; ein Schritt mit alter Basis
  würde also die späteren Bearbeitungen des Benutzers verfehlen. Stattdessen hält der Browser jede
  gespeicherte Inverse aktuell: Jede Änderung von `local`, die kein Schritt ist – ein fremder Batch,
  unser eigener rebast zurückgekommen, ein abgelehnter Batch, ein EVENTS-Undo, das Layout von Save –,
  wird als Operationen ausgedrückt (`diff` von `local` vorher und nachher) und von oben nach unten in
  die Stapel rebast: der Schritt über die Änderung, die Änderung über den Schritt, dann trifft der
  nächste Schritt sie so, wie sie vor dem Schritt darüber war (`rebase`). Eigene Aktionen brauchen
  kein Rebase – der Stapel ist last in, first out. Ein Schritt, dessen Ziel eine fremde Änderung
  überschrieben oder entfernt hat, wird mit deren Herkunft markiert; sein Rückgängig wird mit *Could
  not undo '<label>': changed by <origin>* abgelehnt und der Schritt verworfen. Passen die Operationen
  nicht mehr oder meldet die Antwort des Backends auf einen Undo-Batch Konflikte, erscheint derselbe
  Hinweis und der Schritt wird verworfen; das Dokument bleibt, wie das Backend es angewendet hat.
- **EVENTS-Undo.** Ein MCP-`ops`-Ereignis trägt seine `inverse` gegen das bestätigte Dokument; der
  Store rebast sie über unsere ausstehenden Batches und hält sie pro EVENTS-Zeile (`EditLine.undo`:
  `ready`, `undone`, `failed`, `gone`), rebast sie über jede spätere Änderung, eigene eingeschlossen,
  und markiert sie, wenn eine spätere Änderung ihr Ziel überschrieben hat. `undoLine` sendet sie als
  lokale Bearbeitung, die kein Schritt der Stapel des Benutzers ist (diese werden über sie rebast).
- **Löschen.** `:open`, Revert, ein `reverted`-Ereignis (Neuladen von der Platte), `stale_base` (das
  Dokument wird neu geöffnet) und das Schließen des Reiters löschen die History des Dokuments und
  markieren seine EVENTS-Undos als `gone`; Save behält sie; ein umbenanntes Dokument behält sie unter
  dem neuen Pfad.

### Der MCP-Endpunkt

`POST /mcp` unter der Adresse des Studios (außerhalb von `/api/v1`) spricht MCP über Streamable HTTP
mit grauwolfs eigener Serverseite: `grauwolf_mcp::McpServer::handle` beantwortet `initialize`,
`ping`, `tools/list` und `tools/call`, je Anfrage mit den Tools der aufrufenden Sitzung gebaut – wie
es der Agenten-Endpunkt von `grauwolf-server` tut –, und die Route steckt in
`grauwolf_server::with_token`. Das Token ist bei jedem Start zufällig (`McpHub::random_token`), außer
`serve --mcp-token` oder `GRAUWOLF_STUDIO_MCP_TOKEN` nennt eines; `GET /api/v1/mcp` liefert URL,
Token, den Befehl `claude mcp add` und die Clients. `initialize` öffnet eine Sitzung
(`Mcp-Session-Id`) mit dem Namen des Clients; eine unbekannte Sitzung ist `404`, damit der Client neu
initialisiert; `GET /mcp` ist `405`, `DELETE /mcp` beendet die Sitzung. Ein Client gilt als aktiv,
solange er in den letzten zehn Minuten aufgerufen hat; neue und beendete Sitzungen gehen als
`clients`-Ereignisse hinaus.

Die Tools (`mcp_tools.rs`) arbeiten auf dem Workspace, den der Browser zuletzt mit
`PUT /api/v1/mcp/context` gemeldet hat (oder dem einzigen von `serve --workspace`), und ein fehlender
`path` ist der aktive Reiter. Änderungen sind Stapel von `Documents::edit` – das Tool berechnet
Operationen aus dem Dokument, der Stapel trägt Client `mcp:<session>`, Herkunft
`mcp:<client name>`, Label und Hervorhebung. Die Ressourcenschlüssel und Abschnitte der Übersicht
(`resources`, `write_resource`) folgen `lib/spec-overview.ts` des Frontends; `connect` wählt die
Bedingung, die die Übergänge der Palette zwischen dem Ausgangstyp der Quelle und dem Eingangstyp des
Ziels anbieten, oder erklärt, dass die Ports nicht passen. `run_spec` startet einen Lauf über
`Runs::start`, sendet ein `run`-Ereignis (der Lauf-Store folgt ihm wie einem eigenen) und wartet auf
das Ende.

**Ein Tool hinzufügen:** ein Test in `tests/mcp.rs` gegen einen temporären Workspace
(`Studio::new`, `call`, `ok`/`error`), die Definition (Name, Beschreibung für einen Coding-Agenten,
JSON-Schema) in `definitions()`, der Zweig in `call`, eine Änderung über `Ctx::edit` mit Label und
Hervorhebung; danach das `AGENTS.md` der Crate, Kapitel 16 des Handbuchs und dieses Kapitel.

## Der erzeugte Client

Das Frontend spricht mit dem Backend nur über `studio/web/lib/api/`:

1. `grauwolf-studio openapi` gibt die OpenAPI-Beschreibung aus; `npm run gen-api` schreibt sie nach
   `lib/api/openapi.json` und erzeugt mit openapi-typescript (feste Version) `lib/api/schema.d.ts`;
2. `lib/api/client.ts` umhüllt openapi-fetch (feste Version) und macht aus der Fehlerform einen
   `ApiError`.

Zwei Prüfungen halten ihn aktuell: Der Rust-Test `the_committed_openapi_description_is_current`
vergleicht die eingecheckte `openapi.json` mit dem Backend (`cargo xtask check --affected` wählt das
Studio, wenn sich `studio/web/lib/api/` ändert), und `npm run check-api` erzeugt `schema.d.ts` in eine
temporäre Datei und vergleicht.

## Eine Route hinzufügen

1. Schreib den Test in `crates/apps/grauwolf-studio/tests/` gegen `router(…)` mit
   `tower::ServiceExt::oneshot` und `grauwolf_test::TestDir` (`StudioConfig::single(ordner)` und
   `common::ws(ordner, "/specs…")` für eine Route eines Workspaces, `StudioConfig::root(ordner)` für
   die Workspaces).
2. Ergänze den Handler mit `#[utoipa::path]` in `api.rs`, trag ihn in `ApiDoc` und den Router ein;
   eine Route eines Workspaces gehört unter `/workspaces/{id}/`, nimmt `Ws` (und `RelPath`) und
   listet den Parameter `id`. Typen mit `ToSchema`. Lies Bodies als Bytes mit `parse_body`, damit auch ein kaputter Body die
   Fehlerform bekommt.
3. `npm run gen-api` in `studio/web`, die neuen Typen über `lib/api/client.ts` nutzen.
4. Dokumentiere die Route im `AGENTS.md` der Crate und in `studio/README.md`.

## Teil 2b

GitHub-Anmeldung und GitHub-Workspaces (eine `source` der Art `github` mit Repository und Branch
neben `local`) folgen. Ihr Code aus dem AgentWeaver-Projekt bleibt in `studio/web`, ist aber nicht
eingebunden (siehe `studio/web/AGENTS.md`). Der MCP-Editor mit Live-Abgleich ist Entscheidung 17,
oben; Rückgängig und Wiederholen auf seinen Umkehrungen kommen als Nächstes.
