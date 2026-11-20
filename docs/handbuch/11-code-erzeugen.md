# 11 Code erzeugen

Bis hierher hat grauwolf deine Specs **interpretiert**: `spec run`, `spec chat` und `spec serve`
lesen die JSON-Datei und führen sie aus. Zwei Befehle gehen einen Schritt weiter:

- `grauwolf spec generate` macht aus einer Spec eine eigenständige **Rust-Crate** (ein Rust-Paket),
  die du in eigene Programme einbauen kannst – typgeprüft vom Compiler, mit fertig gebauten Tools.
- `grauwolf spec create` geht den umgekehrten Weg: Ein LLM schreibt dir aus einer Beschreibung in
  Worten eine gültige Spec.

Beide Befehle brauchen die Rust-Toolchain (wie der Tool-Build in [Kapitel 3](03-tools.md)).

## Eine Spec als Rust-Crate

Als Beispiel dient ein Agent mit einem eigenen Tool
([`beispiele/11-wortzaehler.json`](beispiele/11-wortzaehler.json)):

```json
{
  "spec_version": 1,
  "name": "word_counter",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du zählst Wörter mit dem Tool count_words und nennst das Ergebnis in einem Satz.",
  "tools": [
    {
      "name": "count_words",
      "description": "Zählt die Wörter eines Textes.",
      "args": {
        "type": "object",
        "properties": { "text": { "type": "string" } },
        "required": ["text"]
      },
      "code": "let text = args[\"text\"].as_str().ok_or(\"text is missing\")?;\nOk(text.split_whitespace().count().to_string())"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

```sh
grauwolf spec generate docs/handbuch/beispiele/11-wortzaehler.json --out word_counter
```

Ausgabe etwa so:

```text
docs/handbuch/beispiele/11-wortzaehler.json: Crate geschrieben nach word_counter
```

`generate` prüft die Spec, baut zuerst alle Tools (wie `spec build`, mit demselben Cache) und
schreibt dann:

```text
word_counter/
├── Cargo.toml              Paket "word_counter" (der Name der Spec) mit den grauwolf-Crates
├── src/lib.rs              der Agent als Rust-Code
└── tools/count_words.wasm  das gebaute Tool, wird in die Crate eingebettet
```

Die Datei `src/lib.rs` enthält unter anderem:

| Funktion | Zweck |
|---|---|
| `graph()` | der Ablauf als typgeprüfter Graph |
| `model()`, `prompt()` | Modell und System-Prompt aus der Spec |
| `tools()` | die Tools der Spec, ausgeführt in der Sandbox |
| `run(executor, input)` | führt den Agenten in einem frischen Kontext aus |
| `run_observed(…, features)` | wie `run`; die Features sehen alle Events (wie `--events` bei `spec run`) |
| `run_app(base_dir, input)` | mit Feature `app`: richtet alles ein wie `spec run` (siehe unten) |
| `subgraph_<name>()` | eine Funktion je Subgraph |

Benannte Typen aus `types` ([Kapitel 5](05-strukturierte-daten.md)) werden zu Rust-Structs, und `run` nimmt
genau diese Typen entgegen und gibt sie zurück. Braucht die Spec Tools von außen – MCP-Server, Wissen,
Gedächtnis-Tools, Unteragenten oder eingebaute Tools –, enthält die Crate zusätzlich `EXTERNAL_TOOLS` (die
benötigten Namen) und `run_with_tools(executor, input, external)`; dein Programm stellt diese Tools bereit.
Wie du die Crate aus Rust benutzt, zeigt das
[Entwicklerhandbuch](../entwickler/06-generator.md).

### Alles an einem Ort: Feature `app`

Wenn du nichts selbst verdrahten willst, aktiviere das Cargo-Feature `app` der Crate. Dann bekommst du
`run_app(base_dir, input)`, das alles selbst einrichtet wie `grauwolf spec run`: Anbieter und Schlüssel aus
der Umgebung (`OLLAMA_HOST`, `ANTHROPIC_API_KEY`, …), MCP-Server, Wissen, eingebaute Tools, Unteragenten und
Langzeitgedächtnis. Pfade aus der Spec (z. B. `root`, `sources`, `store`) sind relativ zu `base_dir`.

```toml
[dependencies]
word_counter = { path = "word_counter", features = ["app"] }
```

```rust
let answer = word_counter::run_app(Path::new("."), "Count the words in note.txt".into()).await?;
```

Für einen bestimmten Benutzer – sein Bereich im Langzeitgedächtnis und sein Schlüssel-Wert-Speicher, wie
`spec run --user` – und um die Events zu verfolgen, gibt es `run_app_with`. Seine Optionen sind zweigeteilt:
`ProvisionOptions` gehören zum Agenten (die Umgebung, aus der er seine Einstellungen liest, ein eigener
Entscheider, Beobachter, eine Preistabelle), `RunOptions` gehören zu einem einzelnen Lauf (der Benutzer, die
Features, die die Events sehen, und alles andere, was ein Lauf von `spec run` haben kann, siehe unten):

```rust
use grauwolf::config::ProcessEnv;
use grauwolf::provisioning::ProvisionOptions;

let provision = ProvisionOptions::new(Arc::new(ProcessEnv));
let run = grauwolf::app::RunOptions {
    user: Some("anna".into()),
    features: vec![Arc::new(grauwolf::features::JsonlEventLog::new(std::io::stderr()))],
    ..Default::default()
};
let answer = word_counter::run_app_with(Path::new("."), "What do you know about me?".into(), provision, run).await?;
```

`run_app_with` richtet den Agenten (Anbieter, MCP-Server, Wissen, Unteragenten) bei jedem Aufruf neu ein. Wenn
du den Agenten oft laufen lässt, richte ihn einmal ein und führe ihn je Anfrage aus:

```rust
let agent = word_counter::provision(Path::new("."), provision).await?;
let answer = word_counter::run_provisioned(&agent, "What do you know about me?".into(), run).await?;
```

Erzeugter Code läuft wie der Interpreter (ADR-0119). Die Felder von `RunOptions` sind:

| Feld | Bedeutung |
|---|---|
| `user` | Der Bereich des Benutzers im Langzeitgedächtnis und im Schlüssel-Wert-Speicher |
| `features` | Features, die alle Events des Laufs sehen |
| `budget` | Grenzen dieses Laufs (`max_tokens`, `max_cost`, `max_tool_calls`, `max_prompt_tokens`); jedes hier gesetzte Feld gewinnt gegen die `limits` der Spec, wie `--max-tokens`/`--max-cost` |
| `conversation` | `(store, id)`: setzt das Gespräch fort und speichert es (`ChatHistoryStore`) |
| `attachments` | Bilder, PDFs oder Audio für die Eingabe des Laufs |
| `approver` | Entscheidet über Tools, die eine Freigabe brauchen (`ToolApprover`); ohne ihn wird ein solcher Aufruf abgelehnt mit `tool '<name>' needs an approval that nobody can give here` |
| `frame_sink` | Empfängt die Frames der Streaming-Nodes, also die Text-Deltas |
| `checkpoints` | `(store, run id)`: speichert den Zustand vor jedem Node |

Nach einem Absturz setzt `word_counter::resume_provisioned(&agent, store, "run-1", run).await?` den Lauf ab
dem letzten Checkpoint fort (die Eingabe ist die des Checkpoints; `RunOptions::resuming` tut dasselbe für deine
eigenen Einstiegspunkte). Tools, die eine Freigabe brauchen, sind `self_tools.tool` mit `confirm` und
`run_command` im Shell-Modus mit `approve_each_call`: `spec generate` nimmt solche Specs an, und die Crate
gibt die Aufrufe frei wie `spec run`.

Kosten kommen aus einer Preistabelle, derselben Datei wie `--prices` ([Kapitel 2](02-modelle.md)): Setze
`GRAUWOLF_PRICES` auf ihren Pfad (relativ zum Basisverzeichnis) oder übergib sie mit
`ProvisionOptions::with_prices`. Dann trägt `agent_completed` den `cost`, und `limits.max_cost` wirkt; eine
Spec mit `limits.max_cost` und ohne Preistabelle scheitert schon in `provision`, vor dem ersten Lauf.

So arbeitet auch ein erzeugter Dienst. `AppOptions` gibt es nicht mehr: Seine `user` und `features` sind jetzt
`RunOptions`, sein `decider`, `mcp_tools_observer` und `agent_features` gehören zu `ProvisionOptions`
(`with_decider`, `with_mcp_tools_observer`, `with_agent_features`).

Ohne `app` bleibt die Crate schlank und hängt nicht von der Facade von grauwolf ab.

Die Datei ist erzeugt – ändere besser die Spec und erzeuge neu.

### Als HTTP-Dienst: `--service`

Mit `--service` bekommt die Crate zusätzlich ein Cargo-Feature `service` und das Binary `agent`
(`src/main.rs`), das den Agenten wie `grauwolf spec serve` bereitstellt: `/run`, `/run/stream`,
`/health`, A2A (Agent-Karte und `/a2a`) und MCP (`/mcp`). Das Binary wird nur mit dem
Feature gebaut:

```sh
grauwolf spec generate examples/specs/echo.json --out echo --service --check
cargo build --release --features service --manifest-path echo/Cargo.toml
OLLAMA_HOST=http://localhost:11434 GRAUWOLF_ALLOW_ANONYMOUS=true echo/target/release/agent
```

Das Binary heißt für jede Spec `agent`, damit Images den Namen der Spec nicht kennen müssen.
Alles wird über die Umgebung konfiguriert:

| Variable | Bedeutung | Standard |
|---|---|---|
| `SERVER_PORT` | Port des Dienstes (gebunden an `0.0.0.0`) | `8080` |
| `GRAUWOLF_SERVER_TOKEN` | Gemeinsames Token (`Authorization: Bearer …`) | – |
| `GRAUWOLF_USERS_FILE` | Benutzer-Tokens wie bei `spec serve --users` | – |
| `GRAUWOLF_ALLOW_ANONYMOUS` | `true` erlaubt den Zugriff ohne jedes Token; sonst startet der Dienst ohne Token oder Benutzerdatei nicht | – |
| `GRAUWOLF_DATA_DIR` | Verzeichnis, gegen das die Pfade der Spec aufgelöst werden (`knowledge`, `store` des Gedächtnisses, `builtin_tools.root`, Tool-Cache) | Arbeitsverzeichnis |
| `GRAUWOLF_CONVERSATIONS_DIR` | Verzeichnis (relativ zu `GRAUWOLF_DATA_DIR`), in dem die Gespräche (`conversation_id`) liegen, eine Datei je Gespräch, damit sie einen Neustart überstehen; wie `spec serve --conversations-dir` | im Speicher |
| `GRAUWOLF_CONVERSATIONS_DB` | Datenbank-URL für die Gespräche (`sqlite://chats.db`, `postgres://…`), wie `spec serve --conversations-db`; die URL wird nie ausgegeben. Braucht das Facade-Feature `sql` (bei `--full-facade` enthalten); ohne es startet der Dienst nicht. Beide Gesprächsvariablen zu setzen ist ein Fehler beim Start | im Speicher |
| `A2A_PORT` | A2A auch auf diesem Port (er ist immer auch am Hauptport aktiv) | – |
| `A2A_BASE_PATH` | Pfadpräfix der A2A-Routen auf diesem Port, z. B. `/agents`; die Karte liegt dann unter `/agents/.well-known/agent-card.json` | keiner |
| `GRAUWOLF_CHAT_API` | `1` oder `true` bietet außerdem `POST /api/chat` und `GET /api/signature` an ([Kapitel 9](09-betrieb.md#die-chat-api-apichat-und-apisignature)); mit `AGENT_SIGNATURE_PATH` oder `agent-signature.json` und `agent-names.json` in `GRAUWOLF_DATA_DIR`; `--dockerfile` kopiert diese beiden Dateien neben der Spec nach `app/` | – |
| `GRAUWOLF_OTEL`, `GRAUWOLF_OTEL_CONTENT` | `1` oder `true` exportiert die Spans jedes Laufs per OTLP/HTTP, wie `spec serve --otel` (Ziel aus `OTEL_EXPORTER_OTLP_ENDPOINT` und `OTEL_EXPORTER_OTLP_HEADERS`, Dienstname aus `OTEL_SERVICE_NAME`, Standard `grauwolf`), und setzt einen eingehenden `traceparent` fort ([Kapitel 9](09-betrieb.md#opentelemetry)); `GRAUWOLF_OTEL_CONTENT` zeichnet auch Inhalte auf. Jeder andere Wert ist ein Startfehler, der die Variable nennt. `--service`-Crates enthalten das Facade-Feature `otel` (etwa 19 Crates mehr, die das Builder-Image schon kompiliert) | aus |
| `GRAUWOLF_EVENTS` | `1` oder `true` schreibt jedes Event jedes Laufs, auch das der Unteragenten, als eine JSON-Zeile auf stderr, wie `spec serve --events` ([Kapitel 9](09-betrieb.md#ereignisse-als-json-zeilen)); `agent deploy --events` setzt es in einem Cluster ([Kapitel 13](13-cluster-einrichten.md#events-im-pod-log)). Jeder andere Wert als `0`/`false`/leer ist ein Startfehler, der die Variable nennt | aus |
| `GRAUWOLF_PUBLIC_URL`, `GRAUWOLF_A2A_PUBLIC_URL` | Öffentliche URLs für die Agent-Karten | `http://localhost:<port>` |

Anbieter und ihre Schlüssel kommen aus den üblichen Variablen (`OLLAMA_HOST`, `OPENAI_API_KEY`, …). Bei
SIGTERM oder SIGINT nimmt der Dienst keine Verbindungen mehr an und beendet laufende Anfragen.

Ein erzeugter Dienst verhält sich wie `spec serve`: Der Benutzer (`user_id` oder Benutzer-Token → das
Langzeitgedächtnis und der Schlüssel-Wert-Speicher dieses Benutzers; jeder Benutzer hat seine eigenen), Gespräche
(`conversation_id`, im Speicher des Prozesses, sofern nicht `GRAUWOLF_CONVERSATIONS_DIR` oder
`GRAUWOLF_CONVERSATIONS_DB` gesetzt ist), Anhänge, die Events und Text-Deltas eines Laufs auf `/run/stream` und
Freigaben (`event: approval` und `POST /approvals/{id}`, [Kapitel 9](09-betrieb.md)) erreichen alle den Lauf.
`/run`, A2A und MCP haben keinen Rückkanal, daher werden Tools, die eine Freigabe brauchen, dort abgelehnt.
Kosten brauchen `GRAUWOLF_PRICES`. Wie du den Dienst in einem Container betreibst, beschreibt
[Kapitel 9](09-betrieb.md#container-images).

### Gleiche Dependencies: `--full-facade`

Normalerweise hängt die Crate nur mit den Features von der Facade ab, die die Spec braucht (Anbieter, MCP,
eingebaute Tools, ...). Mit `--full-facade` hängt sie von der Facade mit allen ihren Features ab, unabhängig
davon, was die Spec braucht. Der Abhängigkeitsgraph und die Feature-Mengen sind dann für jede Spec gleich, so
dass eine Build-Umgebung alle Abhängigkeiten einmal kompilieren und für jeden Agenten wiederverwenden kann
(das Builder-Image, [Kapitel 9](09-betrieb.md#container-images)). Der Preis ist ein längerer erster Build.

### Dockerfile: `--dockerfile`

```sh
grauwolf spec generate spec.json --service --dockerfile --out agent
docker build --network none agent          # oder kaniko
```

`--dockerfile` impliziert `--service` und `--full-facade` und nimmt die grauwolf-Crates aus
`/opt/grauwolf/crates`, wo sie das [Builder-Image](09-betrieb.md#container-images) ablegt (gib
`--grauwolf-crates` an, um ein anderes Verzeichnis zu nutzen). Es schreibt neben die Crate:

- `Dockerfile`: baut `FROM ghcr.io/agentsw0rk/grauwolf-builder:<version der CLI>` (ändern mit
  `--builder-image`). Der erste Build-Schritt ist eine **Versionsprüfung**: Die Variable `GRAUWOLF_VERSION` des
  Builder-Images muss der Version der CLI entsprechen, die die Crate erzeugt hat, sonst scheitert der Build mit
  `crate generated by grauwolf X, builder is Y - regenerate with grauwolf Y or pass --builder-image with
  --allow-builder-mismatch`. Für ein bewusst anderes Builder-Image gib `--allow-builder-mismatch` an: Die
  Prüfung entfällt. Der Build führt dann `cargo build --release --offline --features service` aus, kompiliert
  also nur die Agenten-Crate; die Laufzeitstufe ist distroless (`gcr.io/distroless/cc-debian12:nonroot`), oder
  Debian slim mit `git` und `ca-certificates` (ohne Root), wenn die Spec Git-Tools benutzt. Das Image führt das
  Binary `agent` in `/app` als Benutzer 65532 aus und hört auf Port 8080 (`SERVER_PORT`).
- `.dockerignore`.
- `app/`: die Dateien, die der Dienst zur Laufzeit liest, kopiert aus dem Verzeichnis der Spec mit ihren
  relativen Pfaden: Wissensquellen und Index, der Speicher des Gedächtnisses, das Wurzelverzeichnis der
  Dateitools und die Specs lokaler Unteragenten mit ihren eigenen Dateien. Das wird `/app` im Image, das
  Arbeitsverzeichnis und `GRAUWOLF_DATA_DIR`. Pfade außerhalb des Verzeichnisses der Spec werden nicht kopiert.
  Unteragenten laufen **offline**: `spec generate` löst den ganzen [Agenten-Baum](08-externe-tools-und-agenten.md#agenten-als-tools-agent_tools)
  auf, und `app/` enthält auch, was sie brauchen – den gebauten Code ihrer Tools und die Tool-Pakete, die sie
  nutzen, in `app/.grauwolf/tools/` sowie die `grauwolf.lock` des Baums. Der Dienst lädt sie mit `--locked` aus
  diesem Cache, ohne Netz, ohne Toolchain und ohne Git. Die Tool-Richtlinie wird beim Bau des Images angewendet.
- `app/egress.json`: alle Ziele des Agenten-Baums, so wie `grauwolf spec egress --format json` sie ausgibt
  (Lauf- und Build-Phase, unterschieden durch `phase`; `any_port` kennzeichnet einen `fetch`-Host im Cluster ohne festen Port; der OTLP-Endpunkt steht nicht darin). Ein Deployment
  leitet daraus seine Netzregeln ab, siehe
  [Egress begrenzen](13-cluster-einrichten.md#begrenzen-was-ein-agent-erreichen-darf-egress).

Ohne `--tool-sources` kompiliert `spec generate` den Tool-Code selbst zu WASM, bevor die Crate geschrieben
wird; die Build-Stufe braucht dafür nichts. Führe `spec generate` im Builder-Image aus (es enthält die CLI, die
Toolchain und die vendorten Abhängigkeiten), damit dieser Schritt ebenfalls offline funktioniert:

```sh
docker run --rm -v "$PWD:/work" -w /work ghcr.io/agentsw0rk/grauwolf-builder:<version> \
  grauwolf spec generate spec.json --dockerfile --out agent
```

### Tool-Code als Quellen: `--tool-sources` und `grauwolf tools build`

Mit `--tool-sources` kompiliert `spec generate` nichts und braucht keine Rust-Toolchain. Eine Crate, deren
Spec Tools hat, bekommt neben der Bibliothek:

- `tools/<name>/Cargo.toml` und `tools/<name>/src/lib.rs`: die Gast-Crate jedes Tools, genau das, was
  `spec build` kompiliert;
- `tools/tools.json`: die Formatversion und je Tool den Namen, die Modul-Datei (`tools/<name>.wasm`) und den
  SHA-256 der beiden Dateien der Gast-Crate;
- dasselbe für den Code des Graphen ([Kapitel 4](04-ablaeufe.md#code-in-graphen-code-on_code-und-transform)):
  jeder `code`-Node, jede Bedingung `on_code` und jedes `transform` einer Kante ist eine eigene Gast-Crate, benannt
  nach dem Node oder der Kante (`code_main_score`, `condition_main_score_finish`,
  `transform_main_score_finish`), mit der Art `code`, `condition` oder `transform` im Manifest (Tools haben
  keine); erzeugte Crates führen diese Module wie Tools in der Sandbox aus, so verhalten sie sich genau wie der
  Interpreter;
- `build.rs` (nur die Standardbibliothek): bricht `cargo build` mit der Meldung
  `tool modules missing: tools/add.wasm - run \`grauwolf tools build .\` in this directory first` ab, solange
  ein Modul fehlt.

Die erzeugte Bibliothek bettet weiterhin `tools/<name>.wasm` ein; die Dateien gibt es noch nicht. Baue sie:

```sh
grauwolf spec generate spec.json --tool-sources --service --out agent
grauwolf tools build agent
cargo build --manifest-path agent/Cargo.toml --features service
```

`tools build <verz>` prüft den Hash jeder Gast-Crate gegen `tools/tools.json` (ein Tool, dessen Quellen geändert
wurden, ist ein Fehler, der das Tool nennt: erzeuge die Crate dann neu) und baut sie mit dem isolierten
Tool-Build ([Kapitel 10](10-selbst-gebaute-tools.md); eigenes Zielverzeichnis, nur die erlaubten Crates,
offline). Dafür braucht es die Rust-Toolchain mit dem Target `wasm32-unknown-unknown` und `serde_json` in den
lokalen Cargo-Quellen – das Builder-Image hat beides. Ein Modul, dessen Quellhash dem entspricht, aus dem es
gebaut wurde (Stempel `tools/<name>.wasm.src-sha256`), wird übersprungen; `--force` baut neu. Optionen:
`--cache`, `--build-wrapper` wie bei `spec build`.

Fehler werden je Tool gemeldet, mit Zeile und Spalte im Tool-Code der Spec:

```text
add: line 1, column 14: mismatched types
```

`--format json` gibt stattdessen einen Bericht auf der Standardausgabe aus (Exit-Code 1, wenn ein Tool
gescheitert ist), den Werkzeuge wie AgentWeaver dem Tool-Node zuordnen können:

```json
{ "tools": [ { "name": "add", "ok": false, "skipped": false,
               "errors": [ { "message": "mismatched types", "line": 1, "column": 14 } ] } ] }
```

`ok` ist `true` für gebaute und übersprungene Tools; `line` und `column` (ab 1) fehlen dort, wo der Compiler im
Tool-Code keine Position angibt.

Mit `--dockerfile` kopiert das `Dockerfile` zuerst `tools/` und führt
`grauwolf tools build . --cache /tmp/tool-cache` in einer eigenen Schicht aus, dann kopiert es den Rest der Crate
und führt den Offline-`cargo build` aus. Solange sich die Tools nicht ändern, wird die Schicht wiederverwendet:

```sh
grauwolf spec generate spec.json --tool-sources --dockerfile --out agent
docker build --network none agent          # oder kaniko
```

Grenzen: `--tool-sources` funktioniert nicht mit `--check` (die Prüfung braucht die Module), und die
`tool_packages` der Spec werden abgelehnt: OCI-Pakete tragen gebaute Module, also erzeuge ohne `--tool-sources`
oder nimm eine Git-Quelle; Git-Pakete werden auch noch nicht als Quellen geschrieben.

### Compiler-Check: `--check`

Mit `--check` führt grauwolf danach `cargo check` für die Crate aus – mit Feature `app`, damit `run_app` ebenfalls
geprüft wird (mit `--service`: mit Feature `service`, das `app` enthält, damit auch `src/main.rs` geprüft wird):

```sh
grauwolf spec generate docs/handbuch/beispiele/11-wortzaehler.json --out word_counter --check
```

Etwa so:

```text
docs/handbuch/beispiele/11-wortzaehler.json: Crate geschrieben nach word_counter
docs/handbuch/beispiele/11-wortzaehler.json: cargo check (mit Feature app): keine Fehler
```

Findet der Compiler Fehler, listet grauwolf jeden mit Datei, Zeile und Spalte in der erzeugten Crate auf, z. B.
`docs/handbuch/beispiele/11-wortzaehler.json: word_counter/src/lib.rs:12:5: error: …`, und endet mit dem
Exit-Code 1. Welches `cargo` aufgerufen wird, bestimmt die Umgebungsvariable `CARGO` (Standard: `cargo`).

### Woher kommen die grauwolf-Crates?

Die erzeugte `Cargo.toml` verweist per Pfad auf die Crates eines grauwolf-Checkouts:

```toml
[dependencies]
grauwolf-agents = { path = "/home/you/grauwolf/crates/domain/grauwolf-agents" }
grauwolf-graph = { path = "/home/you/grauwolf/crates/domain/grauwolf-graph" }
# …
```

Standard ist der Checkout, aus dem deine `grauwolf`-CLI gebaut wurde. Ein anderes wählst du mit
`--grauwolf-crates` – du übergibst dessen Verzeichnis `crates/`:

```sh
grauwolf spec generate docs/handbuch/beispiele/11-wortzaehler.json --out word_counter \
  --grauwolf-crates /opt/grauwolf/crates
```

So funktioniert die Crate überall, wo dieser Checkout unter demselben Pfad liegt.

### Tools bauen: `--cache` und `--build-wrapper`

Wie bei allen Befehlen, die Tool-Code bauen, setzt du das Cache-Verzeichnis mit `--cache` (Standard
`.grauwolf/tools`) und einen Wrapper um `cargo` mit `--build-wrapper` (siehe
[Kapitel 10](10-selbst-gebaute-tools.md#sicherheit)). Ein Compilerfehler im Tool-Code bricht ab und zeigt auf die
Stelle in der Spec, etwa so:

```text
docs/handbuch/beispiele/11-wortzaehler.json:/tools/0/code:1:42: error: …
```

## Automatisches Gedächtnis

Hat die Spec ein `long_term_memory` mit `auto_recall`, `auto_remember` oder `remember_compressed`, bekommt die
Crate zusätzlich `run_with_memory(executor, input, [external,] memory)`. Mit einem Gedächtnis (`Some(…)`) ruft
der Agent vor dem Lauf passende Fakten ab und speichert danach neue – genau wie `spec run`. `run` und
`run_with_tools` laufen ohne Gedächtnis.

## Was sich (noch) nicht erzeugen lässt

Manche Fähigkeiten brauchen die Laufzeit des Interpreters. Dafür lehnt `generate` die Spec ab:

| Feld der Spec | Grund |
|---|---|
| `tools_dir` | Das Tool-Verzeichnis wird nur zur Laufzeit gelesen. |
| `self_tools` | Selbst gebaute Tools brauchen Bau und Sandbox zur Laufzeit. |

Die Meldung sieht so aus:

```text
docs/handbuch/beispiele/10-werkzeugbauer.json: 'self_tools' is not yet supported by the generator – run the spec with `grauwolf spec run`
```

Außerdem müssen sich benannte Typen nach Rust abbilden lassen: Jeder Typ ist ein Objekt (`"type": "object"`),
seine Felder haben die Typen `string`, `integer`, `number`, `boolean` oder `array` (mit `items`), und Feldnamen
bestehen aus Kleinbuchstaben, Ziffern und `_`. Sonst meldet `generate` die Stelle, z. B.:

```text
ticket.json: /types/Ticket/properties/meta: type not supported: supported are "string", "integer", "number", "boolean" and "array"
```

Felder, die nicht unter `required` stehen, werden optionale Felder.

Was der Interpreter zur Laufzeit mitbringt, nimmt die Crate als Optionen eines Laufs entgegen (`RunOptions`, siehe
oben): Budget, Gespräch, Anhänge, Freigaben, Checkpoints und Kosten. OpenTelemetry und die Gesprächsspeicher der
CLI richtest du in deinem eigenen Programm ein (oder nutzt `spec run`/`spec serve`).

## Ein LLM Specs schreiben lassen

`grauwolf spec create` nutzt den **Builder-Agenten**: Er bekommt deine Anforderung, das JSON-Schema des
Spec-Formats und alle Node-Arten und Bedingungen, schreibt eine Spec und prüft sie selbst.

```sh
grauwolf spec create "Ein Agent, der Temperaturen von Celsius in Fahrenheit umrechnet und dafür ein Tool nutzt." \
  --model ollama:qwen3.5:9b --out temperatur.json
```

Ausgabe etwa so:

```text
temperatur.json: spec 'temperature_converter' created (1 repair)
```

So arbeitet der Builder:

1. Das LLM schreibt einen Entwurf – über strukturierte Ausgabe, daher immer syntaktisch gültiges JSON.
2. grauwolf lädt und prüft ihn. Ist die Spec gültig, wird ihr Tool-Code ebenfalls kompiliert (mit `--cache` und
   `--build-wrapper` wie bei `spec build`).
3. Jedes Problem geht mit einem JSON-Pointer zurück an das LLM, z. B.
   `/tools/0/code: line 2, column 9: error: …` oder `/graph/edges/3/to: unknown node 'answer'`. Das LLM
   repariert es, und es geht zurück zu Schritt 2.
4. Ist alles in Ordnung, schreibt grauwolf die Spec nach `--out` (fehlende Verzeichnisse werden angelegt).

| Option | Bedeutung |
|---|---|
| `--model <anbieter>:<modell>` | LLM für den Builder, z. B. `ollama:qwen3.5:9b` oder `anthropic:claude-haiku-5-5`. Der erzeugte Agent läuft ebenfalls genau mit diesem Modell – es steht in seiner Spec. |
| `--out <datei>` | Zieldatei der Spec |
| `--max-repairs <n>` | Höchstzahl der Reparaturen, Standard `3` |
| `--cache`, `--build-wrapper` | wie beim Tool-Build |

Die Zugangsdaten des Anbieters kommen wie immer aus der Umgebung ([Kapitel 2](02-modelle.md)). Fehlt der
Doppelpunkt in `--model`, meldet grauwolf:

```text
--model expects <provider>:<model>, e.g. ollama:qwen3.5:9b, got: qwen3.5
```

Reichen die Reparaturen nicht, bricht `create` ab und listet die verbleibenden Probleme auf:

```text
no valid spec after 3 repairs:
/graph/nodes/1/kind: unknown node kind 'llm_answer'
```

Eine erzeugte Spec ist ein Ausgangspunkt, kein fertiges Produkt: Lies sie durch, prüfe sie mit `spec graph`
([Kapitel 4](04-ablaeufe.md)) und probiere sie mit `spec run` aus, bevor du sie benutzt.

## Die wichtigsten Punkte

- `grauwolf spec generate <spec> --out <verzeichnis>` erzeugt eine Rust-Crate mit `graph()`, `run()` und den
  fertig gebauten Tools; `--check` prüft sie mit `cargo check`.
- `--service` ergänzt das HTTP-Dienst-Binary `agent` hinter dem Cargo-Feature `service`, konfiguriert über
  Umgebungsvariablen (`SERVER_PORT`, `GRAUWOLF_SERVER_TOKEN`, `A2A_PORT`, …).
- `--grauwolf-crates` wählt den grauwolf-Checkout, auf den die Crate verweist.
- `tools_dir` und `self_tools` lassen sich nicht erzeugen; das automatische Gedächtnis schon
  (`run_with_memory`). Benannte Typen müssen sich nach Rust abbilden lassen; ihre Felder sind `string`, `integer`,
  `number`, `boolean` oder `array`.
- `grauwolf spec create "<anforderung>" --model <anbieter>:<modell> --out <datei>` lässt ein LLM eine gültige Spec
  schreiben und reparieren (`--max-repairs`, Standard 3).

Weiter mit der [CLI-Referenz](referenz-cli.md) und der [Spec-Referenz](referenz-spec.md).
