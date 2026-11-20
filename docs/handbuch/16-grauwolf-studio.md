# 16 – grauwolf studio: Specs zeichnen

grauwolf studio ist ein grafischer Editor für deine Specs. Du zeichnest den Ablauf eines Agenten auf
einer Zeichenfläche – Knoten aus der Palette, Kanten dazwischen – und das Studio schreibt das
Spec-JSON, das du aus den vorigen Kapiteln kennst. Nichts wird übersetzt: Die Zeichenfläche
bearbeitet die Spec-Datei direkt, also ist das, was du siehst, genau das, was `grauwolf spec run`
ausführt.

Dieses Kapitel behandelt die Teile 1 und 2a des Studios: lokale Workspaces – getrennte Projekte,
jedes ein Ordner mit Specs –, das Zeichnen und das Ausführen einer Spec direkt im Studio mit
Live-Ereignissen, Freigaben, API-Schlüsseln und Modelllisten – und das gemeinsame Zeichnen mit einem
Coding-Agenten wie Claude Code, der über MCP dieselben Specs bearbeitet, während du zusiehst.
GitHub-Workspaces folgen später; die Kopfzeile zeigt sie abgeschaltet an. Der Rahmen ist der eines
Code-Editors, mit Reitern, Panels und einer Befehlspalette.

## Das Studio starten

Das Studio ist ein Programm, `grauwolf-studio`, das den Editor in deinem Browser bereitstellt. Baue
den Editor einmal und starte das Studio:

```bash
cd studio/web && npm ci && npm run build && cd ../..
cargo run -p grauwolf-studio -- serve --web studio/web/out
```

Öffne <http://127.0.0.1:8090>. Das Studio lauscht nur auf deinem eigenen Rechner; eine Anmeldung
gibt es noch nicht. Seine Workspaces liegen in `~/grauwolf-studio/workspaces` (`--root <ordner>` für
einen anderen Ordner); `--workspace <ordner>` arbeitet nur auf diesem einen Ordner, `--addr` wählt
eine andere Adresse, `--config-dir <ordner>` einen anderen Ordner für deine API-Schlüssel
(Standard `~/.config/grauwolf/studio`), `--mcp-token` ein festes Token für Coding-Agenten (siehe
[Gemeinsam mit einem Coding-Agenten zeichnen](#gemeinsam-mit-einem-coding-agenten-zeichnen)).

Das Skript `scripts/studio.sh` erledigt das alles in einem Schritt: Es installiert die Pakete des
Frontends, baut Backend und Frontend und startet das Studio auf `~/grauwolf-studio/workspaces`. Beim
ersten Mal legt es den Workspace „examples“ mit Kopien der Beispiel-Specs an (die Originale bleiben
unberührt) – oder, wenn du das Studio schon benutzt hast, registriert es deinen alten
`~/grauwolf-studio-workspace` als „examples“; seine Dateien bleiben, wo sie sind:

```bash
scripts/studio.sh             # backend on :8090 and the editor with hot reload on http://127.0.0.1:3008
scripts/studio.sh --static    # static export, everything on http://127.0.0.1:8090
scripts/studio.sh --build-only
```

`--root <ordner>` wählt einen anderen Wurzelordner, `--workspace <ordner>` einen einzelnen Ordner,
`--host 0.0.0.0` macht es von außerhalb eines Containers erreichbar; `--help` zeigt alle Optionen.
Strg+C beendet alles.

## Workspaces

Ein Workspace ist ein eigenes Projekt: ein Ordner mit Specs, den Dateien, auf die sie verweisen,
einer Tool-Policy (`grauwolf.toml`) und einer Lock-Datei (`grauwolf.lock`). Validierung,
Dateiverweise und Vorlagen arbeiten innerhalb des offenen Workspaces; ein anderer Workspace sieht
seine Dateien nie.

Ohne offenen Workspace zeigt das Studio seine **Startseite**: jeden Workspace mit Name,
Beschreibung, ob er *own* (eigen) oder *registered* (registriert) ist, der Zahl der Specs und dem
Ordner.

- **New workspace** fragt nach Name, Beschreibung und Inhalt: *Empty* (eine erste Spec,
  `agent.json`, gleich ausführbar), *The user guide's examples* (eine Kopie davon) oder *A copy of
  another workspace*. Das Studio legt dafür einen Ordner im Wurzelordner an und öffnet ihn.
- **Register folder** macht einen vorhandenen Ordner zum Workspace – ein Projekt von dir oder
  `docs/user-guide/examples` selbst. Gib den Pfad an (`/home/ich/agenten` oder `~/agenten`) und, wenn
  du magst, einen Namen. Die Dateien bleiben, wo sie sind, und das Studio schreibt nichts in den
  Ordner; es vermerkt ihn in `.registry.json` des Wurzelordners.
- **Umbenennen** (der Stift) ändert Name und Beschreibung; die ID in der Adresse bleibt.
- **Löschen** (der Papierkorb): Bei einem eigenen Workspace tippst du zur Bestätigung seine ID, dann
  wird sein Ordner mit allen Dateien gelöscht. Ein registrierter Ordner wird nur aus der Liste
  entfernt; seine Dateien bleiben.

Der offene Workspace ist Teil der Adresse, `http://127.0.0.1:8090/?w=examples`, also öffnet ein
Neuladen oder ein Lesezeichen ihn wieder. Eine Adresse mit einem Workspace, den es nicht mehr gibt,
zeigt die Startseite mit einem Hinweis.

In der Kopfzeile neben *grauwolf studio* sitzt der **Umschalter**: der Name des offenen Workspaces, ein Menü mit
den zuletzt geöffneten und *All workspaces…* (die Startseite). Die Statusleiste zeigt, ob
der Workspace eine Tool-Policy (`policy:`) und eine Lock-Datei (`lock:`) hat. Vor dem Umschalten fragt
es nach, wenn ein Spec- oder Datei-Reiter ungespeicherte Änderungen hat, und schließt sie.

Mit `--workspace <ordner>` hat das Studio nur diesen einen Workspace (nach dem Ordner benannt): Die
Startseite listet ihn, Anlegen, Registrieren, Umbenennen und Löschen werden nicht angeboten.

## Zwei Ebenen: Übersicht und Ablauf

Eine Spec öffnet sich in ihrer **Übersicht**: ein Knoten für den Agenten – Name, Modell mit seinen
Ausweichmodellen, die Art des Agenten (nächster Abschnitt), der Anfang des System-Prompts, Limits –
und je ein Knoten für jede seiner Ressourcen, mit ihm verbunden:

| Ressourcen-Knoten | Teil der Spec |
|---|---|
| Tool | ein Eintrag von `tools` |
| Tool-Datei, Tool-Ordner | ein Eintrag von `tool_files`, `tools_dir` |
| Tool-Paket | ein Eintrag von `tool_packages` |
| built-in: *Gruppe* | die Tools von `builtin_tools` einer Gruppe (Dateien, git, kv, todo, …) |
| MCP *Name* | ein Eintrag von `mcp_servers` |
| agent *Name* | ein Eintrag von `agent_tools` (eine lokale Spec oder ein A2A-Agent) |
| Wissen, Langzeitgedächtnis | `knowledge`, `long_term_memory` |
| Skills | ein Eintrag von `skills` |
| selbst gebaute Tools | `self_tools` |
| System One (RKEV) | aus dem Ablauf abgeleitet, kein Abschnitt der Spec: ein `decide`-Knoten mit `decider: system_one` (siehe unten) |

Wähl eine Ressource aus, um ihren Teil der Spec rechts als JSON zu bearbeiten. Wähl den Agenten
aus, um Name, Modell und System-Prompt zu bearbeiten.

Entscheidungen (`decide`-Knoten) sind Schritte des Ablaufs und stehen daher **nicht** in der Übersicht:
Du findest sie im Reiter *Flow*.

### Die Art des Agenten

Das Abzeichen oben am Agenten-Knoten sagt, welche Art von Agent die Spec beschreibt (in Großbuchstaben, wie die anderen Abzeichen):

| Angezeigt | Die Spec hat |
|---|---|
| **Agent** | eine fertige `strategy` (`single_run`, `react`, `chat`, `verified`) |
| **Graph-Agent** | einen eigenen `graph` ohne Planer-Knoten |
| **Planner-Agent** | einen eigenen `graph` mit einem `plan`- oder `goap`-Knoten, im Hauptgraphen oder in einem Subgraphen |

Ein Graph- oder Planner-Agent zeigt außerdem eine einzeilige Zusammenfassung seines Ablaufs, etwa
`Graph: 5 nodes, 1 decision`; `1 decision`, `2 subgraphs` und `1 planner` erscheinen nur, wenn es welche gibt. Gezählt
werden auch die Knoten der Subgraphen, nicht aber das implizite `start` und `finish`. Ein Klick auf die
Zusammenfassung öffnet den Reiter *Flow*. Ein Agent mit Strategie zeigt stattdessen seine Strategie mit *Show
flow* und *Edit as graph* (siehe [Der Ablauf](#der-ablauf)). Der Inspektor zeigt dasselbe, wenn der
Agent oder nichts ausgewählt ist.

### System One als Ressource

Hat ein `decide`-Knoten des Hauptgraphen oder eines Subgraphen `decider: system_one` (siehe
[Kapitel 4](04-ablaeufe.md)), zeigt die Übersicht eine Ressource, **System One (RKEV)**, in einer
eigenen Farbe (pink), verbunden mit dem Port **Resources** des Agenten – ein externer Dienst wie ein
MCP-Server. Sie wird aus dem Ablauf abgeleitet und ist kein Abschnitt der Spec; deshalb lässt sie sich
nicht trennen, parken oder löschen: Es gibt diese Aktionen nicht, der Hover sagt, warum, und sie
verschwindet, sobald kein `decide`-Knoten mehr System One nutzt. Ihr Inspektor listet die
`decide`-Knoten, die sie nutzen (ein Klick öffnet den Ablauf und wählt den Knoten aus), und nennt die
Variablen `RKEV_BASE_URL` (erforderlich) und `RKEV_API_KEY` (optional), jeweils als *set* oder *not
set* in der Umgebung des Studio-Prozesses – nie den Wert.

### Ports

Der Agenten-Knoten hat an seinem linken Rand für jede Gruppe von Ressourcen einen benannten Port:

| Port | Nimmt |
|---|---|
| Tools | Tools, Tool-Dateien, den Tool-Ordner (`tools_dir`), Gruppen eingebauter Tools, Tool-Pakete (`tool_packages`), selbst gebaute Tools |
| Agents | `agent_tools`: andere Agenten, lokale Specs oder entfernte A2A-Agenten |
| Resources | MCP-Server (`mcp_servers`), `knowledge`, `long_term_memory` |
| Skills | `skills` |

Darüber zeigt **Input** den Typ des `input` der Spec, rechts **Output** den Typ ihres `output`.
Jede Ressource hat einen Ausgang, **Agent**. Jede Port-Gruppe hat eine Farbe für ihren Socket und
die Verbindungen (Tools grün, Agents orange, Resources lila, Skills rosé); ein Ressourcen-Knoten
behält die Farbe seiner Art (ein Tool-Paket hellgrün, Wissen türkis, Gedächtnis bernstein). Eine
Ressource verbindet sich nur mit dem Port ihrer Gruppe: Ein Skill auf *Resources* fallen gelassen
verbindet nichts, und das Studio sagt, welchen Port er braucht („A skill does not connect to
Resources: its port is Skills.“). Während du
eine Verbindung ziehst, ist der passende Port hervorgehoben. Entscheidungen (`decide`-Knoten)
gehören zum Ablauf und haben keinen Knoten in der Übersicht.

### Verbinden, trennen, löschen

Die Verbindungen **sind** die Spec: Was mit dem Agenten verbunden ist, steht in der Spec-Datei.

- **Verbinde** eine Ressource, indem du von ihrem Ausgang *Agent* zu ihrem Port des Agenten ziehst
  (oder im Inspektor **Connect** klickst). Sie wird an ihrer Stelle in die Spec geschrieben – ein
  Tool in `tools`, ein MCP-Server unter seinem Namen in `mcp_servers` (`server_2`, wenn der Name
  vergeben ist) und so weiter.
- **Trenne** sie, indem du die Verbindung anklickst und Entf drückst, mit dem Kontextmenü der
  Verbindung oder der Ressource oder mit **Disconnect** im Inspektor. Die Ressource verlässt die
  Spec, bleibt aber gestrichelt und grau auf der Zeichenfläche, markiert als *not connected*. Ihre
  Konfiguration wird in der Layout-Datei aufbewahrt (*geparkt*), sodass erneutes Verbinden genau
  das zurückschreibt, was sie war. Eine geparkte Ressource wird nicht validiert: Sie gehört nicht
  zur Spec. Wähl sie aus, um ihre Konfiguration zu bearbeiten.
- **Lösche** eine Ressource (Entf auf dem ausgewählten Knoten, das Kontextmenü, **Delete** im
  Inspektor), um sie aus der Spec und von der Zeichenfläche zu entfernen.

Die Palette der Übersicht legt ein neues Tool, einen MCP-Server, einen Agenten als Tool, ein
Tool-Paket, Wissen, Langzeitgedächtnis, Skills oder ein eingebautes Tool **unverbunden** dort, wo du
es sehen kannst – neben dem Agenten, wenn dieser Platz sichtbar ist, sonst auf einem freien Platz der
sichtbaren Zeichenfläche (oder dorthin, wo du es fallen lässt) –, mit einer Vorlage zum Ausfüllen;
das Verbinden
schreibt es in die Spec. Eine Spec hat nur ein `knowledge`, ein `long_term_memory` und ein
`tools_dir`: Ein zweites zu verbinden wird abgelehnt, bis das erste getrennt oder gelöscht ist.

### Agenten des Workspace

Unter den Ressourcen listet die Palette die anderen Specs des Workspace. Zieh eine auf die
Übersicht (oder klick sie an): Sie wird ein Agenten-Knoten, noch nicht verbunden, der Namen und
Modell des referenzierten Agenten zeigt. Mit **Agents** verbunden, wird sie ein Eintrag von
`agent_tools`, der diese Spec als Sub-Agenten ausführt – `{ "spec": "13-reviewer.json",
"description": "…" }`, der Pfad relativ zur offenen Spec, wie in
[Kapitel 8](08-externe-tools-und-agenten.md); die Beschreibung, die das Modell liest, bearbeitest du
im Inspektor. Vorhandene `agent_tools`-Einträge mit lokaler `spec` zeigen Namen und Modell der Spec,
die sie ausführen, sodass die Übersicht zeigt, wer wen aufruft; entfernte A2A-Agenten (`url`)
behalten eine eigene Knotenart (*a2a agent*).

### Der Ablauf

Der **Ablauf** ist die zweite Ebene: der Reiter *Flow* über der Zeichenfläche oder ein Doppelklick
auf den Agenten-Knoten. Er existiert nur für eine Spec mit eigenem `graph`, der dort gezeigt wird,
bereit zum Bearbeiten (nächste Abschnitte). Jeder Port eines Ablauf-Knotens zeigt den Typ, der dort
durchläuft (`string`, `message`, `tool_call`, `tool_results` oder ein Typ der Spec; `start` den
Eingabetyp der Spec, `finish` ihren Ausgabetyp), und während du eine Verbindung ziehst, sind die
Eingänge hervorgehoben, zu denen sie passt – derselbe Typ oder ein Typ, in den eine Bedingung ihn
überführt.

Eine Spec mit `strategy` (`single_run`, `react`, `chat`, `verified`) hat keinen Reiter *Flow*, und
ein Doppelklick auf den Agenten öffnet keinen Ablauf. Der Agenten-Knoten (Abzeichen **Agent**) und der Inspektor zeigen
die Strategie („Strategy: single_run“) und zwei Aktionen:

- **Show flow** öffnet einen Dialog mit dem Ablauf, den grauwolf aus der Strategie baut, nur zum
  Lesen: Das Studio fragt grauwolf danach, er ist also genau das, was ausgeführt wird. Der Dialog
  sagt in einem Satz, was die Strategie tut, und hat auch den Knopf **Edit as graph**.
- **Edit as graph** schreibt diesen Ablauf als gewöhnlichen `graph` in die Spec (mit den Typen, die
  die Strategie ergänzt, etwa `Verification`) – die Spec verhält sich danach gleich und lässt sich
  Knoten für Knoten bearbeiten. Sie gilt als ungespeichert, und der Reiter *Flow* erscheint. Alle
  vier Strategien lassen sich umwandeln.

### Planer im Ablauf

Die Aktionen eines Planer-Knotens (`goap`, `plan`, [Kapitel 6](06-subgraphen-und-planer.md)) sind
keine Kanten der Spec – der Planer wählt sie, während er läuft. Der Ablauf zeigt sie trotzdem:

- **Gestrichelte Verbindungen** führen vom Planer-Knoten zu dem Subgraphen, den jede Aktion
  ausführt (oder zu einem gestrichelten Kasten `agent <name>` für eine Aktion mit `agent`),
  beschriftet mit den Aktionsnamen. Gestrichelt heißt „kann laufen“, nicht „läuft immer“. Mehrere
  Aktionen mit demselben Subgraphen teilen sich eine Verbindung mit einer Beschriftung je Aktion
  (`06-goap`: `write_rough_draft` und `revise` führen beide `write` aus). Die Verbindungen sind nur
  gezeichnet: Man kann sie nicht auswählen, löschen oder speichern. Die Subgraphen, die ein Planer
  des Hauptgraphen ausführt, stehen nebeneinander in einer Reihe darunter, sodass man die
  Verbindungen von oben nach unten liest.
- **Der Planer-Knoten** zeigt den Startzustand (`state: draft=none, facts_checked=false`) und die
  Ziele; der volle Text steht im Hover.
- **Der erwartete Plan (nur GOAP).** Das Studio fragt grauwolfs eigenen GOAP-Planer, welchen Plan er
  für den Startzustand macht, und nummeriert die Aktionen in dieser Reihenfolge: ① write_rough_draft,
  ② revise, ③ check_facts. Ohne gültigen Plan (kein Ziel erreichbar oder ungültige Parameter) zeigt
  der Knoten „⚠ no valid plan“ und zählt das als Warnung; der Hover sagt, warum. Die Eingabe des
  Knotens ist vor einem Lauf unbekannt, ein `input_fact` beginnt also leer. Ein LLM-Planer (`plan`)
  hat keinen erwarteten Plan: Seine eine Verbindung zum Schritt-Subgraphen hat keine Nummer.
- **Der Hover über einer Aktion** zeigt ihre Vorbedingungen, Wirkungen und Kosten (`pre:
  draft=final`, `effects: facts_checked=true`, `cost: 3`) und ihre Beschreibung.
- **Der Inspektor** eines Planer-Knotens listet die Aktionen in einer Tabelle – Nummer im erwarteten
  Plan, Name, was sie ausführt, `pre`, `effects`, Kosten – mit dem erwarteten Plan oder dem Grund,
  warum es keinen gibt.

**In einem Lauf** leuchten die Aktionen in der Reihenfolge auf, die der Planer tatsächlich gewählt
hat: Die Verbindung der laufenden Aktion leuchtet, und jede gelaufene Aktion bekommt ein ✓ mit ihrer
Laufnummer (`✓1`, `✓2` …). Neu geplante Schritte werden in Laufreihenfolge weiter nummeriert; führt
der Planer dieselbe Aktion gleich noch einmal aus (ein neuer Versuch nach einem Fehlschlag), behält
die Wiederholung ihre Nummer – ①②②③, wenn `revise` zweimal lief. Die Knoten in einem
Aktions-Subgraphen melden keine eigenen Ereignisse; der Subgraph bekommt daher die Markierungen an
`start` und `finish`, nicht an seinen inneren Knoten.

## Der Bildschirm

Das Studio ist aufgebaut wie ein Code-Editor (ADR-0126 Entscheidung 16):

- **Kopfzeile:** der Workspace-Umschalter (der Name des offenen Workspaces; das Menü listet die
  zuletzt geöffneten und *All workspaces…*), **Commands** (die Befehlspalette), **Validate**,
  **Save**, **Diagram**, **Run** (zeigt das RUN-Panel und setzt den Cursor in seine Eingabe),
  **Agent** (einen Coding-Agenten verbinden, siehe [unten](#gemeinsam-mit-einem-coding-agenten-zeichnen))
  und das Schlüssel-Symbol für die API-Schlüssel.
- **Aktivitätsleiste** am linken Rand mit drei Ansichten für die Seitenleiste daneben:
  - **Explorer** – der Workspace-Ordner als Dateibaum (siehe *Die Dateien des Workspaces*). Specs
    erkennst du an einem Ablauf-Symbol und ihrem Namen; ein Klick öffnet eine Spec auf der
    Zeichenfläche, jede andere Datei im Editor.
  - **Palette** – in der Übersicht die Ressourcen und die anderen Specs des Workspaces, im Ablauf die
    Knotenarten. Diese stammen aus dem Knotenkatalog von grauwolf, die Palette zeigt also immer genau
    die Arten, die deine grauwolf-Version kennt, jede mit Beschreibung und ihren Typen
    (`string → message`); unter den Katalogarten stehen die Arten, die den Rest der Spec brauchen:
    `subgraph`, `plan`, `goap`, `parallel`, `for_each`, `code`.
  - **Search** – Dateinamen, Spec-Namen und Knoten-IDs des ganzen Workspaces. Eine Datei öffnet sich
    im Editor, eine Spec auf der Zeichenfläche, ein Knoten ausgewählt im Ablauf seiner Spec.

  Ein Klick auf die aktive Ansicht klappt die Seitenleiste ein, ein weiterer holt sie zurück.
- **Editorbereich mit Reitern.** Jede Spec und jede Datei, die du öffnest, bekommt einen Reiter, es
  können also mehrere gleichzeitig offen sein – etwa `08-coordinator.json` und `add.rs`. Ein Reiter
  mit ungespeicherten Änderungen trägt **●**; das **×** schließt ihn und fragt vorher nach, wenn er
  ungespeicherte Änderungen hat. Eine Spec behält ihren Zustand in ihrem Reiter (ungespeicherte
  Änderungen, die Ansicht, die Auswahl), während ein anderer Reiter gezeigt wird. Jeder Spec-Reiter
  hat den Schalter **Overview / Flow / JSON**:
  - *Overview* und *Flow* sind die beiden Ebenen der Zeichenfläche (siehe oben). *Flow* gibt es nur
    für eine Spec mit eigenem Graphen; eine Spec mit Strategie behält *Show flow* und *Edit as graph*.
    Im Ablauf sind `start` und `finish` immer da; Subgraphen erscheinen als umrahmte Gruppen unter dem
    Hauptablauf. Zieh den Hintergrund, um die Ansicht zu verschieben; das Mausrad zoomt. **Tidy up**,
    links vom Umschalter, ordnet die Ansicht neu an (siehe [Anordnung und Tidy up](#anordnung-und-tidy-up)).
  - *JSON* ist der Spec-Quelltext in Monaco, abgeglichen mit der Zeichenfläche: Eine Änderung
    erscheint kurz nach dem Tippen auf der Zeichenfläche, eine Änderung auf der Zeichenfläche oder im
    Inspektor im Text. Text, der kein gültiges JSON ist, wird im Editor markiert und unter PROBLEMS
    gelistet; die Zeichenfläche behält die letzte gültige Spec, bis der Text wieder passt. Strg+S
    (Cmd+S) speichert.
- **Inspektor** rechts, mit drei Reitern für das Ausgewählte (ohne Auswahl die Spec):
  - **Properties** – das Formular: Name, Modell, System-Prompt, Ein- und Ausgabetyp und die ganze
    Spec als JSON für alles, was die Zeichenfläche nicht zeichnet; bei einem Knoten seine ID und
    Parameter, bei einer Kante ihre Bedingung, bei einer Ressource ihr Teil der Spec.
  - **Files** – die Dateien, auf die die Auswahl verweist, mit **Open** oder **Create from template**.
  - **JSON** – das JSON der Auswahl, nur lesend, mit *Edit in the JSON view*.
- **Panel unten** mit vier Reitern, jeder mit einer Zahl:
  - **RUN** – das Lauf-Panel: Eingabe, Verlauf, Freigaben, Verbrauch, Budget, Stop und Chat sowie
    die Historie der Läufe der Spec (nächster Abschnitt). RUN, EVENTS und TOOLS zeigen immer den
    Lauf der Spec, deren Reiter aktiv ist.
  - **EVENTS** – eine Zeile je Ereignis des Laufs: die Eingabe (`›`), Knotenstarts (`▸ draft`) und
    -enden mit ihrer Zeit (`✓ draft 1.1 s`), Planschritte (`◆ planner → revise`), Tool-Aufrufe mit
    Ergebnis und Zeit (`⇢ translator ✓ 1.1 s`), das Modell jeder Anfrage, Freigaben, die Ausgabe und
    das Ende des Turns und des Laufs. Die Zeile eines Knotens wählt ihn im Ablauf aus. Auch eine
    Änderung, die ein Coding-Agent an der Spec gemacht hat, bekommt eine Zeile
    (`✎ claude-code · add_node ask (llm_request)`).
  - **PROBLEMS** – die Validierung der gezeigten Spec. Ein Klick auf ein Problem wählt seinen Knoten
    oder seine Kante im Ablauf aus oder öffnet die JSON-Ansicht an der Stelle des Problems.
  - **TOOLS** – die Tool-Aufrufe des Laufs, eingeklappt; aufgeklappt zeigen sie Argumente und Ergebnis.

  Zieh die obere Kante des Panels, um seine Höhe zu ändern (der Browser merkt sie sich); der Pfeil
  rechts – oder ein Klick auf den aktiven Reiter – klappt es auf seine Reiterzeile ein.
- **Statusleiste** ganz unten: *✓ valid* oder die Zahl der Probleme (ein Klick öffnet PROBLEMS), die
  Tool-Policy (`grauwolf.toml`) und die Lock-Datei (`grauwolf.lock`) des Workspaces, ob das
  Modell-Backend des Anbieters der Spec erreichbar ist (etwa *ollama reachable*, über die
  Modellsuche gefragt, wenn der Anbieter wechselt, und dann eine Minute behalten – ein Klick fragt
  erneut), der Lauf der gezeigten Spec und, solange Läufe laufen, eine Liste wie *2 running:
  04-decide, 07-chat* – ein Klick auf einen Namen wechselt zum Reiter dieser Spec; solange ein
  Coding-Agent verbunden ist, *MCP: connected (claude-code)*.

### Befehlspalette

**Strg+K** (⌘K auf dem Mac) oder **Commands** in der Kopfzeile öffnet die Befehlspalette über der
Zeichenfläche. Tipp ein paar Buchstaben – die Liste wird unscharf gefiltert, `add mcp` findet also
*Add MCP server to Resources* –, wähl mit ↑ und ↓, führ mit ↵ aus. Befehle mit *…* zeigen eine zweite
Liste zur Auswahl (*Go to node…* die Knoten der Ansicht, die du siehst, *Open file…* die Dateien des
Workspaces); Rücktaste in einer leeren zweiten Liste geht zurück. Esc schließt die Palette; solange
sie offen ist, bleibt die Tastatur in ihr.

| Befehl | Kürzel | Was er tut |
|---|---|---|
| Add *Ressource* to *Port* (z. B. *Add MCP server to Resources*) | | parkt eine neue Ressource aus ihrer Vorlage sichtbar und ausgewählt in der Übersicht |
| List tools of MCP server… | | wählt einen verbundenen MCP-Server aus und listet seine Tools im Inspektor |
| Go to node… | Strg+P / ⌘P | scrollt zu einem Knoten der Übersicht oder des Ablaufs und wählt ihn aus |
| Run with last input | Strg+↵ / ⌘↵ | führt die gezeigte Spec erneut mit der Eingabe des letzten Laufs aus (speichert vorher) |
| Undo | Strg+Z / ⌘Z | nimmt deine letzte Änderung der gezeigten Spec zurück (siehe [Rückgängig und Wiederholen](#rückgängig-und-wiederholen)) |
| Redo | Strg+Umschalt+Z / ⇧⌘Z, Strg+Y | führt sie erneut aus |
| Tidy up | Umschalt+Alt+F / ⇧⌥F | ordnet den gezeigten Ablauf oder die Übersicht neu an (siehe [Anordnung und Tidy up](#anordnung-und-tidy-up)) |
| Save | Strg+S / ⌘S | speichert die gezeigte Spec oder Datei |
| Validate | | validiert die Spec |
| Switch view: Overview / Flow / JSON | | der Schalter des Spec-Reiters |
| Open file… / Open workspace… | | öffnet eine Datei des Workspaces / wechselt den Workspace |
| Close the tab | | schließt den gezeigten Reiter (fragt bei ungespeicherten Änderungen) |
| Toggle the bottom panel, Show EVENTS / PROBLEMS / TOOLS | | das Panel unten |
| Toggle the side bar, Show Explorer / Palette / Search | | die Seitenleiste |
| Toggle the inspector | | der Inspektor |

Strg+P und Strg+K wirken auch, wenn der Cursor in einem Textfeld steht; die anderen Kürzel gehören
dort dem Feld (Monaco speichert mit Strg+S selbst).

## Eine Spec ausführen

Der Reiter **RUN** des Panels unten führt die gezeigte Spec im Studio aus – mit demselben Interpreter wie `grauwolf spec
run`, im Prozess des Studios. Tipp eine Eingabe und drück **Run** (oder Enter); *Run with last input*
(Strg+↵) wiederholt die letzte:

- **Die gespeicherte Datei läuft.** Mit ungespeicherten Änderungen heißt der Knopf **Save & run** und
  speichert zuerst.
- **Der Verlauf** zeigt deine Eingabe, die Antwort, die Tool-Aufrufe (eingeklappt: ein Klick zeigt die
  Argumente und das Ergebnis, auf 4000 Zeichen gekürzt) und Hinweise wie ein Ausweichmodell oder
  `say_to_user`. Text erscheint Stück für Stück, wo die Spec streamt (`llm_request_streaming`); eine
  Strategie wie `single_run` antwortet in einem Stück, wenn sie fertig ist. EVENTS listet den Lauf
  Ereignis für Ereignis, TOOLS seine Tool-Aufrufe.
- **Die Zeichenfläche folgt dem Lauf:** Der Knoten, der gerade läuft, hat einen leuchtenden Rahmen,
  Knoten, die schon gelaufen sind, bekommen einen kleinen Haken – im Ablauf, zugeordnet über Knoten-ID
  und Subgraph; in der Übersicht leuchtet der Agent, solange der Lauf dauert. Wenn ein Lauf beginnt und wenn er endet,
  wird der Ablauf in die Ansicht eingepasst, falls ein Knoten (etwa `finish`) außerhalb liegt;
  dazwischen bleibt die Ansicht, wie du sie eingestellt hast, ein Verschieben während des Laufs wird
  also nie rückgängig gemacht. Markiert werden nur die
  Knoten auf dem genommenen Weg: nach einem `decide` bleibt der nicht gewählte Zweig dunkel. Das
  implizite `start` bekommt seinen Haken, wenn der Graph beginnt, `finish`, wenn der Lauf seine Ausgabe
  hat; `start` und `finish` eines Subgraphen werden genauso markiert, und sein Knoten im Hauptablauf
  leuchtet, solange er läuft.
- **Verbrauch und Kosten** stehen im Kopf des Panels (Tokens ein und aus; die Kosten, wenn eine
  Preistabelle mit `GRAUWOLF_PRICES` gesetzt ist).
- **Freigaben.** Ein Tool, das eine Freigabe braucht – `run_command` im Shell-Modus mit
  `approve_each_call`, das Bauen eines Tools mit `self_tools.confirm` –, fragt im Verlauf mit
  **Approve** und **Refuse** nach; der Lauf wartet auf deine Antwort.
- **Stop** beendet den Lauf sofort; seine Tools und MCP-Server enden mit ihm.
- **Chat.** Eine Spec mit der Strategie `chat` behält ihre Unterhaltung: Nach einer Antwort wartet das
  Panel auf deine nächste Nachricht (**Send**), bis du es mit Stop beendest; die Markierungen auf der
  Zeichenfläche werden zu Beginn jedes Zuges gelöscht, sodass jeder Zug seinen eigenen Weg zeigt.
  Andere Specs antworten einmal; die nächste Eingabe startet einen neuen Lauf.
- `ask_user` ist in Studio-Läufen noch nicht verfügbar; der Agent wird angewiesen, ohne Rückfrage zu
  antworten.

Der Lauf nutzt den Workspace als Arbeitsverzeichnis: Relative Pfade der Spec werden wie immer darin
aufgelöst, Tool-Code wird nach `<workspace>/.grauwolf/tools` gebaut, die Tool-Policy ist die
`grauwolf.toml` des Workspace und die Lock-Datei die neben der Spec. Die Tools sind genau die der
Spec – `run_command` nur, wenn die Spec es einschaltet.

### Budget und Grenzen eines Laufs

Jeder Turn eines Laufs hat ein Budget; das **budget** im Panel zeigt es und ändert es für den nächsten
Lauf:

| Grenze | Standard | Maximum im Studio |
|---|---|---|
| Schritte (Knotenstarts, Subgraphen eingeschlossen) | 100 | 1000 |
| Tokens (Ein- und Ausgabe des Agenten; das niedrigere `limits.max_tokens` der Spec gewinnt) | 200000 | 2000000 |
| Echtzeit in Sekunden (Laden und Warten auf Freigaben eingeschlossen) | 300 | 3600 |

Ein Turn über seinem Budget endet mit einem Fehler, der es nennt. Höchstens **zwei Läufe** laufen
gleichzeitig; ein dritter wird mit „at most 2 runs at the same time“ abgelehnt, bis einer endet oder du
ihn stoppst (ein Chat-Lauf, der auf deine Nachricht wartet, zählt nicht).

### Läufe gehören zu ihrer Spec

Jede Spec hat ihre eigenen Läufe. **RUN**, **EVENTS** und **TOOLS** zeigen den Lauf der Spec, deren
Reiter aktiv ist – den laufenden, sonst den letzten. Eine Spec ohne Lauf zeigt *No run yet – type an
input below and press Run*; wer `04-tool-loop` ausgeführt und dann `01-hello` geöffnet hat, sieht
ein leeres Panel, bis `01-hello` läuft. Die Läufe werden nach Workspace und Pfad gehalten: Schließt
du einen Reiter und öffnest die Spec wieder, sind sie wieder da (bis die Studio-Seite neu geladen
wird; danach wird die Historie unten aus der Liste der letzten Läufe des Studios gelesen).

- **Läufe laufen im Hintergrund weiter.** Ein Wechsel des Reiters stoppt weder einen Lauf noch seine
  Ereignisse. Eine Spec mit laufendem Lauf hat einen blauen Punkt auf ihrem Reiter, die Statusleiste
  listet die laufenden Specs (*2 running: 04-decide, 07-chat*); ein Klick auf einen Namen zeigt
  diesen Reiter und das RUN-Panel. Die Grenze von zwei gleichzeitigen Läufen bleibt.
- **Probleme am Reiter.** Eine Spec mit Validierungsfehlern zeigt deren Zahl als kleine rote Zahl auf
  ihrem Reiter; PROBLEMS listet sie für die aktive Spec.
- **Historie.** Unter der Kopfzeile von RUN zeigt ein eingeklappter Streifen *History (3)* den
  angezeigten Lauf (`#3 · completed · 1.5 s`); aufgeklappt listet er die letzten Läufe der Spec in
  höchstens vier Zeilen über dem Verlauf, nie darüber, und die Auswahl klappt ihn wieder zu. Die Liste ist
  neueste zuerst, etwa `#3 · completed · 2.1 s · 'Review this draft…'` (Nummer, Status, Zeit,
  Anfang der Eingabe). Wer einen auswählt, sieht seinen Verlauf sowie seine Ereignisse und Tools in
  EVENTS und TOOLS, nur lesend; *Show the latest run* (oder der nächste Lauf) geht zurück. Das Studio
  merkt sich im Browser die letzten 10 Läufe je Spec; der Studio-Server selbst listet nur seine
  letzten 20 Läufe des ganzen Workspaces, ältere können dort schon fehlen.
- **Die Zeichenfläche folgt dem gezeigten Lauf** der aktiven Spec: Knotenmarkierungen und
  Planerschritte sind die des gewählten Laufs.

## API-Schlüssel

Das Schlüssel-Symbol in der Kopfzeile öffnet die **API keys**: Für OpenAI, Anthropic, Gemini,
Bedrock (ein API-Schlüssel, `AWS_BEARER_TOKEN_BEDROCK`) und die OpenAI-kompatiblen Anbieter zeigt es
*set*, *not set* oder *from env*. **Set** speichert einen Schlüssel, **Remove** löscht ihn (ein
Schlüssel aus der Umgebung bleibt). Die Schlüssel werden für dich auf diesem Rechner in
`~/.config/grauwolf/studio/keys.json` gespeichert (Modus 0600, Ordner 0700; `--config-dir <ordner>`
oder `GRAUWOLF_STUDIO_CONFIG_DIR` für einen anderen Ordner) und danach nie wieder angezeigt – nicht im
Editor und nicht über die API. Läufe und die Modelllisten nutzen sie vor der Umgebung; sie gelangen
nie in die Umgebung des Studio-Prozesses. Ollama braucht keinen Schlüssel.

## Modelle und MCP-Tools

Im Inspektor der Spec ist **model.provider** eine Liste und **model.id** ein Textfeld mit Vorschlägen:
Fokussiert man es oder klickt man auf **List models**, fragt das Studio den Anbieter – Ollama nach den
installierten Modellen (`/api/tags` unter `OLLAMA_HOST`), OpenAI und die OpenAI-kompatiblen Anbieter,
Anthropic und Gemini mit ihrem Schlüssel; ohne Schlüssel und für Bedrock gibt es eine kurze eingebaute
Liste. Du kannst weiterhin jede ID eintippen. Ist der Anbieter nicht erreichbar, sagt der Inspektor
es, z. B. „Ollama not reachable at http://localhost:11434“.

Ein MCP-Server in der Übersicht hat im Inspektor **List tools**: Das Studio verbindet sich mit dem
Server so, wie der Eintrag jetzt steht (ein stdio-Server wird dafür gestartet und danach wieder
beendet, höchstens 20 Sekunden), und listet jedes Tool, das er anbietet. Hak die an, die der Agent
nutzen soll; sie kommen in `tools` des Eintrags.

## Die Dateien des Workspaces

Der Ordner des offenen Workspaces ist das Dateisystem des Studios: Alles, worauf eine Spec verweist – Tool-Code
(`code_file`, `tool_files`, `tools_dir`), der Code der `code`-Knoten, `on_code`-Bedingungen und
Transformationen, Skills, Wissensdokumente –, liegt darin neben den Specs, und das Studio zeigt und
bearbeitet es dort.

- **Der Baum** zeigt jede Datei und jeden Ordner des Workspaces außer versteckten (`.git`,
  `.grauwolf`), `node_modules` und `target`, und ohne die Layout-Dateien (`*.layout.json`), die zu
  ihrer Spec gehören. Symbole unterscheiden Specs, Tool-Code, Skills und Wissen; Dateien, auf die eine Spec verweist, bekommen die Rolle des Verweises (eine
  Wissensquelle und die Dokumente darin, ein Skill-Ordner).
- **Neue Spec, neue Datei, neuer Ordner** – die drei Schaltflächen über dem Baum; der neue Eintrag
  landet im ausgewählten Ordner (oder neben der ausgewählten Datei). Ein Name, der unter *New spec*
  auf `.json` endet, bekommt eine Spec mit `start` und `finish`.
- **Rename** und **Delete** erscheinen, wenn du auf einen Eintrag zeigst. Die Layout-Datei einer Spec
  wird mit ihr umbenannt und gelöscht; ein Ordner wird nach Bestätigung samt Inhalt gelöscht.
- **Der Editor** ist Monaco, mit Syntaxhervorhebung für Rust, Markdown und JSON (sonst Klartext).
  Jede Datei öffnet sich in einem eigenen Reiter neben den Specs. Strg+S (Cmd+S) speichert; ● im
  Reiter und die Markierung „unsaved“ zeigen noch nicht gespeicherte Änderungen. Nach dem Speichern validiert das Studio die geöffnete Spec erneut, eine
  korrigierte Tool-Datei löst ihr Problem also sofort.
- **Aus dem Reiter Files des Inspektors:** eine ausgewählte Tool-, Tool-Datei, ein Skill, eine Wissensquelle, ein
  `code`-Knoten oder eine Kante mit Code listet ihre Dateien auf. **Open** öffnet eine Datei im
  Editor (einen Ordner im Baum). Eine Datei, die es nicht gibt, ist mit *missing* markiert und bietet
  **Create from template** an; ohne Auswahl listet der Reiter alle Dateien der Spec.
- **Aus PROBLEMS:** ein Problem mit einer fehlenden Datei (zum Beispiel
  `/tools/0/code_file: …/add.rs: No such file or directory`) bietet dasselbe **Create from
  template** an.

Die Vorlagen passen zu dem Ort, für den die Datei gedacht ist:

| Verweis | Vorlage |
|---|---|
| `code_file` eines Tools (auch in einer Tool-Datei) | der Rumpf von `fn tool(args)`: eine Variable pro deklariertem Argument, gelesen mit dem passenden Zugriff (`as_str`, `as_f64`, `as_i64`, `as_bool`; Pflichtargumente mit einem Fehler, wenn sie fehlen), und eine Antwort, die sie zeigt |
| `code_file` eines `code`-Knotens | der Rumpf von `fn run(input: In) -> Result<Out, String>`, der einen Wert des Ausgabetyps zurückgibt |
| `code_file` von `on_code` | der Rumpf von `fn check(value: &In) -> bool`, der `true` zurückgibt |
| `code_file` einer `transform` | der Rumpf von `fn transform(value: In) -> Out` |
| ein Skill-Ordner (`skills[].dir`) | `<ordner>/<name>/SKILL.md` mit `name` und `description` (der erste aus `names`, sonst `new-skill`) |
| eine Wissensquelle | ein Markdown-Dokument (die Quelle selbst oder `notes.md` in einem Quellordner) |

Jede Vorlage validiert und baut so, wie sie ist; die Spec ist also gleich danach gültig. Ersetze
dann das `TODO` durch das, was der Code tun soll.

## Einen Ablauf zeichnen

1. Zieh `llm_request` aus der Palette auf die Zeichenfläche (oder klick darauf).
2. Zieh vom Ausgang von `start` (der Punkt an seinem rechten Rand) zum Eingang des neuen Knotens
   (der Punkt an seinem linken Rand), dann von dessen Ausgang zu `finish`.
3. Die Kante zu `finish` bekommt von selbst die Bedingung `on_assistant_message`: Das Studio wählt
   die Bedingung, die den Typ der Quelle (`message`) in den des Ziels (`string`) verwandelt. Eine
   Kante zu `execute_tool` bekommt `on_tool_call`, eine zu `execute_multiple_tools`
   `on_multiple_tool_calls`, Kanten aus einem `decide`-Knoten `on_decision`.
4. Wähl einen Knoten aus, um seine ID oder seine Parameter (als JSON; das Parameterschema steht
   darunter) zu ändern. Wähl eine Kante aus, um ihre Bedingung, deren Parameter oder eine
   Transformation zu ändern.
5. Um eine Kante zu löschen, klick sie an und drück Entf, oder klick sie mit der rechten Maustaste an
   und wähl *Delete edge*; Knoten genauso.
6. Speichere mit **Save** oder Strg+S (Cmd+S).

Heraus kommt dieselbe Spec, die du von Hand schreiben würdest, zum Beispiel die Tool-Schleife aus
[Kapitel 4](04-ablaeufe.md).

## Validierung am Knoten

Das Studio validiert die Spec nach jeder Änderung, mit denselben Regeln wie `grauwolf spec validate`.
Ein Knoten mit Fehler bekommt einen roten Rahmen und eine Zahl, ein Knoten mit Warnung einen gelben,
eine Kante mit Fehler wird rot. PROBLEMS im Panel unten zeigt jedes Problem mit seinem JSON-Pointer;
ein Klick wählt den Knoten oder die Kante im Ablauf aus oder öffnet die JSON-Ansicht am Pointer; die
Statusleiste zählt sie. In der Übersicht markiert ein Problem die Ressource, zu
der es gehört (einen MCP-Server, ein Tool …), oder – bei Modell, Prompt oder Strategie – den
Agenten-Knoten.

**Diagram** zeigt den Ablauf als Mermaid-Text, so wie `grauwolf spec graph` ihn ausgibt, zum Kopieren
in ein Dokument.

## Gemeinsam mit einem Coding-Agenten zeichnen

Das Studio ist auch ein MCP-Server: Ein Coding-Agent – Claude Code oder ein anderer MCP-Client –
liest und ändert die Specs des offenen Workspaces, und du siehst jede Änderung auf der Zeichenfläche
in dem Moment, in dem sie geschieht. Du zeichnest, der Agent zeichnet, niemand lädt etwas neu.

### Claude Code verbinden

Klicke in der Kopfzeile auf **Agent** (oder führe *Connect a coding agent…* aus der Befehlspalette
aus). Der Dialog zeigt

- den fertigen Befehl für Claude Code, zum Beispiel

  ```bash
  claude mcp add --transport http grauwolf-studio http://127.0.0.1:8090/mcp --header "Authorization: Bearer <token>"
  ```

  mit **Copy** – führe ihn in dem Projekt aus, in dem du Claude Code benutzt;
- die URL des Endpunkts (`/mcp` unter der Adresse des Studios) und das **Token**, verborgen, bis du
  **Show** klickst, mit **Copy**;
- einen Hinweis für andere MCP-Clients: Streamable HTTP unter dieser URL mit dem Header
  `Authorization: Bearer <token>`.

Das Token entsteht bei jedem Start des Studios neu, und nur das Studio zeigt es; eine Anfrage ohne
es bekommt `401`. Mit `--mcp-token <token>` (oder `GRAUWOLF_STUDIO_MCP_TOKEN`) gestartet, behält das
Studio dieses Token, sodass du den Server nach einem Neustart nicht wieder hinzufügen musst.

Solange ein Agent verbunden ist, zeigt die Statusleiste **MCP: connected (claude-code)**.

### Was der Agent kann

Der Agent arbeitet auf dem Workspace, den du im Browser offen hast, und auf der Spec des aktiven
Reiters, wenn er keine andere nennt. Seine Tools:

| Tool | Was es tut |
|---|---|
| `list_specs`, `read_spec` | die Specs des Workspaces; eine Spec so, wie das Studio sie gerade hat – ungespeicherte Änderungen eingeschlossen – mit Layout und Validierungsproblemen |
| `list_node_kinds`, `get_schema`, `validate` | die Palette (Knotenarten, Bedingungen, Ressourcenarten), das Spec-Format, die Validierung |
| `add_node`, `remove_node`, `connect`, `disconnect`, `set_params`, `set_field` | der Ablauf: Knoten und Kanten, die Parameter eines Knotens, jedes Feld der Spec per JSON-Pointer |
| `add_resource`, `remove_resource`, `move_node` | die Übersicht: Tools, MCP-Server, Agenten, Wissen, Gedächtnis, Skills …, verbunden mit ihrem Port am Agenten; Positionen auf der Zeichenfläche |
| `create_spec`, `write_file` | eine neue Spec; eine Textdatei des Workspaces (Tool-Code, `SKILL.md` …) |
| `save_spec` | die Spec auf die Platte schreiben |
| `run_spec` | die Spec wie **Run** ausführen und auf Ausgabe, Status und Token-Verbrauch warten |

Passt etwas nicht, erfährt der Agent, warum: eine unbekannte Knotenart (mit den bekannten), zwei
Ports, deren Typen nicht passen und die keine Bedingung umwandelt (`when` angeben), ein JSON-Pointer,
der ins Leere führt, eine Spec mit `strategy`, die keinen Graphen hat, in den Knoten kommen könnten.

### Live, während ihr beide bearbeitet

Eine Änderung des Agenten erscheint sofort: im Ablauf und in der Übersicht, in der JSON-Ansicht und
im Inspektor. Was sie berührt hat, leuchtet kurz auf, und EVENTS bekommt eine Zeile pro Änderung,
etwa `claude-code · add_node ask (llm_request)`; ein Klick auf die Zeile wählt den Knoten aus. Deine
Auswahl bleibt, wo sie war.

Du kannst weiterarbeiten, während der Agent arbeitet. Beide arbeiten an derselben **Arbeitskopie**,
die das Studio für jede offene Spec hält: Änderungen an verschiedenen Stellen der Spec – du
verschiebst einen Knoten, der Agent setzt einen Prompt – bleiben beide erhalten. Ändert ihr beide
denselben Wert, gewinnt die spätere Änderung und das Studio sagt es dir; eine Änderung an etwas, das
der andere inzwischen gelöscht hat, entfällt, mit einem Hinweis.

Ein Lauf, den der Agent mit `run_spec` startet, erscheint in RUN und im Verlauf seiner Spec wie dein
eigener.

### Speichern

Nichts, was der Agent tut, wird von selbst gespeichert: Der Reiter zeigt ●, und **Save** schreibt die
Spec – deine und die Änderungen des Agenten zusammen. Ein Agent, den du ums Speichern bittest, ruft
`save_spec` auf. `run_spec` führt die gespeicherte Datei aus: Bei ungespeicherten Änderungen lehnt
es ab, außer der Agent gibt `save: true` mit.

Schließt du einen Reiter mit ungespeicherten Änderungen, werden sie verworfen, auch die des Agenten.
Ändert sich die Datei auf der Platte, während die Spec ungespeicherte Änderungen hat (ein
`git checkout`), sagt das Studio es dir, und **Save** fragt, bevor es die Datei überschreibt; eine
Spec ohne ungespeicherte Änderungen folgt einfach der Datei.

## Rückgängig und Wiederholen

**Strg+Z** (⌘Z auf dem Mac) nimmt deine letzte Änderung der gezeigten Spec zurück,
**Strg+Umschalt+Z** (⇧⌘Z) oder **Strg+Y** führt sie erneut aus. Die Pfeile im Kopf tun dasselbe; ihr
Tooltip nennt den Schritt, etwa *Undo Move node ask*. Auch die Befehlspalette hat *Undo* und *Redo*.

| Du hast | Ein Rückgängig nimmt zurück |
|---|---|
| einen Knoten oder eine Ressource gezogen | das ganze Ziehen (der Knoten kehrt an seinen Platz zurück) |
| eine Verbindung gezeichnet oder gelöscht | die Verbindung |
| eine Ressource aus der Palette hinzugefügt, verbunden, getrennt oder gelöscht | genau diesen Schritt (erst die Verbindung, dann die Ressource) |
| in ein Feld des Inspektors getippt | das Tippen bis zu einer Pause von etwa einer Sekunde oder bis du das Feld verlässt |
| in der JSON-Ansicht bearbeitet | alles, seit du in den Editor gegangen bist, sobald du ihn verlässt |
| *Edit as graph*, einen Knoten umbenannt, hinzugefügt oder gelöscht | die ganze Aktion |

In einem Textfeld und im JSON-Editor ist Strg+Z das eigene Rückgängig des Feldes; das Rückgängig der
Spec gilt, wenn der Cursor nicht im Feld steht.

**Nur deine eigenen Änderungen.** Strg+Z nimmt nie zurück, was ein Coding-Agent (oder ein anderer
Browser-Tab) geändert hat: Sein Knoten bleibt. Deine Schritte passen trotzdem nach seinen Änderungen –
du verschiebst einen Knoten, der Agent setzt den Prompt des Knotens, und das Rückgängigmachen deiner
Verschiebung setzt den Knoten zurück, während der Prompt bleibt. Hat der Agent genau das geändert
oder gelöscht, was ein Schritt ändern würde, lässt sich der Schritt nicht anwenden: Das Studio sagt
*Could not undo 'Move node ask': changed by claude-code* und verwirft den Schritt.

**Eine Änderung des Agenten aus EVENTS zurücknehmen.** Jede Zeile einer Änderung des Agenten hat
einen Knopf **Undo**, der genau diese Änderung zurücknimmt, etwa den Knoten von
`claude-code · add_node ask (llm_request)`. Die Zeile zeigt danach *undone*. Hat sich seither etwas
daran geändert, sagt die Zeile das stattdessen.

Die Schritte gelten pro Spec, auch über **Save** hinweg, bis zu 200. **Revert**, das Schließen des
Reiters und das erneute Laden der Spec von der Platte löschen sie. Eine neue eigene Änderung löscht,
was du wiederholen könntest.

## Reihenfolge der Schlüssel

Beim Speichern bleibt die Reihenfolge der Schlüssel in der Spec-Datei erhalten: Eine Spec, die du mit
`name` zuerst und `graph` zuletzt geschrieben hast, wird genauso zurückgeschrieben, und *Convert to
graph* setzt `graph` an die Stelle, an der `strategy` stand.

## Anordnung und Tidy up

Das Studio ordnet Abläufe und die Übersicht mit dem Eclipse Layout Kernel (ELK) an: Der Ablauf läuft
in Schichten von links nach rechts mit so wenigen Kreuzungen wie möglich, `finish` am Ende; Subgraphen
sind Gruppen unter dem Hauptgraphen – die Subgraphen, die ein Planer ausführt, nebeneinander in einer
Reihe darunter, die anderen untereinander. **Kanten sind orthogonal** – gerade Strecken mit
abgerundeten Ecken – und laufen um die Knoten herum, nie unter einem hindurch; eine
Bedingungsbeschriftung sitzt neben ihrer Kante, auf eigenem Hintergrund, wo sie keinen Knoten und
keine andere Beschriftung überdeckt. Die ausgewählte Kante ist heller, eine ungültige rot; die
Verbindungen der Planer sind wie bisher gestrichelt.

Von selbst geschieht das nur für Knoten **ohne gespeicherte Position**: eine Spec ohne Layout-Datei
oder ein Knoten, den du in der JSON-Ansicht oder ein Agent ergänzt hat. Ein neuer Knoten kommt neben
die Knoten, mit denen er verbunden ist, dorthin, wo er nichts überdeckt; Knoten mit Position bleiben,
wo sie sind. Ziehst du einen Knoten, werden seine Kanten um die Knoten geführt, wie sie jetzt stehen.

**Tidy up** ordnet die ganze Ansicht, die du siehst – den Ablauf oder die Übersicht –, neu an: der
Knopf *Tidy up* neben *Overview / Flow / JSON*, der Befehl *Tidy up* in der Befehlspalette oder
**Umschalt+Alt+F** (⇧⌥F auf dem Mac). Es ist ein Schritt: **Strg+Z** (⌘Z) stellt jeden Knoten
dorthin zurück, wo er war. Danach, und wenn ein Ablauf öffnet oder ein Lauf startet, wird die Ansicht
eingepasst – aber nicht unter 60 % Zoom, damit die Texte lesbar bleiben; ein breiterer Ablauf beginnt
an seinem linken Ende, den Rest erreichst du durch Verschieben.

## Wo die Positionen stehen

Die Positionen der Knoten gehören nicht zur Spec. Das Studio hält sie in einer zweiten Datei daneben:
Zu `writer.json` gehört `writer.layout.json`, mit den Positionen und der Ansicht des Ablaufs und
der Übersicht sowie der Konfiguration geparkter (nicht verbundener) Ressourcen. Löschst du diese
Datei, verlierst du nur die Positionen und die geparkten Ressourcen. Der aus einer Strategie
für eine Strategie gezeigte Ablauf wird jedes Mal neu angeordnet; seine Positionen werden nicht gespeichert.

Eine Spec ohne Layout-Datei – eine, die du von Hand geschrieben hast, eine aus `grauwolf spec create`,
eine, die Claude Code geschrieben hat – wird beim Öffnen automatisch angeordnet (siehe
[Anordnung und Tidy up](#anordnung-und-tidy-up)). Fehlen der Layout-Datei einzelne Knoten (weil du sie
im JSON ergänzt hast), werden nur diese platziert, neben ihren Nachbarn, dort, wo sie nichts
überdecken; die anderen behalten ihre Positionen. Beim nächsten Speichern werden alle Positionen
geschrieben. Die Wege der Kanten werden nicht gespeichert: Das Studio ermittelt sie bei jedem Zeichnen.

## Grenzen

- Sub-Agenten aus `agent_tools` öffnest du nicht über die Zeichenfläche; öffne ihre Spec-Datei im
  Baum (der Inspektor der Agenten-Ressource bietet ebenfalls **Open** an).
- Ein Workspace aus einem GitHub-Repository kommt mit Teil 2b.
- Ein Knoten, den du gezogen hast, behält seinen Platz, auch wenn er im Weg steht: Seine Kanten
  führen um ihn herum, aber nur *Tidy up* verschiebt die Knoten selbst.
- Rückgängig und Wiederholen leben im Browser-Tab: Nach einem Neuladen der Seite sind die Schritte leer.
- Läufe nutzen die gespeicherte Spec; `ask_user` funktioniert in ihnen noch nicht, und eine Spec in
  einem Unterordner nutzt die Lock-Datei neben ihr, nicht die oben im Workspace.
- Die Typprüfung von `agent`-Knoten gegen die Spec des Sub-Agenten und die Prüfung lokaler
  Skill-Ordner laufen bisher nur in `grauwolf spec validate`.

Die API, Wurzelordner und Registry, die Layout-Datei im Detail und den Bau des Studios findest du in
`studio/README.md` (englisch).
