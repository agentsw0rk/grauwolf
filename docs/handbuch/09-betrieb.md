# 9 Betrieb

Bisher hast du deine Agenten auf der Kommandozeile gestartet: eine Eingabe, eine Antwort, fertig. In diesem Kapitel machst du aus einem Agenten einen **Dienst**, den andere Programme über HTTP erreichen. Du lernst

- wie `grauwolf spec serve` einen Agenten bereitstellt und welche Endpunkte es gibt,
- wie du den Dienst mit Tokens absicherst und Läufe einzelnen Benutzern zuordnest,
- wie ein Mensch heikle Tool-Aufrufe freigibt – auf der Konsole und im Dienst,
- wie du den Agenten in Claude Desktop einbindest,
- wie du Läufe beobachtest (Ereignisse, OpenTelemetry) und unterbrochene Läufe fortsetzt.

Das Beispiel ist ein kleines Notizbuch, das Markdown-Dateien liest und schreibt ([`beispiele/09-notizbuch.json`](beispiele/09-notizbuch.json)):

```json
{
  "spec_version": 1,
  "name": "notebook",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du verwaltest Notizen als Markdown-Dateien. Lies bestehende Notizen, bevor du sie änderst. Antworte kurz und auf Deutsch.",
  "builtin_tools": {
    "tools": ["list_directory", "read_file", "write_file"],
    "root": "09-betrieb/ablage"
  },
  "strategy": { "kind": "single_run" }
}
```

Die eingebauten Datei-Tools kennst du aus [Kapitel 3](03-tools.md); sie arbeiten nur unterhalb von `root`, hier im Ordner `docs/handbuch/beispiele/09-betrieb/ablage/`.

## Der Agent als HTTP-Dienst

```bash
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json
```

Auf stderr siehst du etwa:

```text
grauwolf service for notebook on http://127.0.0.1:8080
```

Der Dienst lädt die Spec **einmal** beim Start – samt Tools, MCP-Servern und Wissen – und bedient danach beliebig viele Anfragen nebeneinander; jeder Lauf bekommt einen frischen Kontext. Mit Strg+C beendest du ihn.

Die Adresse wählst du mit `--addr`; Standard ist `127.0.0.1:8080`. Ohne Absicherung (siehe unten) startet der Dienst nur auf einer lokalen Adresse.

### `GET /health` – läuft der Dienst?

```bash
curl http://127.0.0.1:8080/health
```

Antwort: `ok`. Der Endpunkt ist immer offen und eignet sich für Load-Balancer und Container-Checks.

### `POST /run` – ein Lauf, eine Antwort

Der Body ist JSON mit dem Feld `input` – Text bei `input: string`, sonst ein JSON-Wert des Eingabetyps der Spec ([Kapitel 5](05-strukturierte-daten.md)). Vergiss nicht den Header `Content-Type: application/json`.

```bash
curl -s http://127.0.0.1:8080/run \
  -H 'Content-Type: application/json' \
  -d '{ "input": "Was steht auf meiner Einkaufsliste?" }'
```

Antwort etwa:

```json
{ "output": "Deine Einkaufsliste hat Brot und Äpfel." }
```

Schlägt der Lauf fehl, bekommst du den Status `500` mit `{ "error": "…" }`.

Drei optionale Felder erweitern die Anfrage:

| Feld | Bedeutung |
|---|---|
| `conversation_id` | Führt dieses Gespräch fort ([Kapitel 7](07-gespraeche-und-wissen.md)). Erlaubt sind Buchstaben, Ziffern, `-` und `_`; sonst Status `400`. |
| `user_id` | Benutzer des Laufs: wählt sein Gespräch und – mit `long_term_memory` – seinen Bereich im Gedächtnis. |
| `attachments` | Anhänge der Eingabe: `[{ "media_type": "image/png", "data": "<base64>" }]` oder mit `"url"` statt `"data"`; fehlen beide oder sind beide vorhanden, Status `400`. Welche Arten ein Anbieter versteht, steht in [Kapitel 2](02-modelle.md#bilder-pdfs-und-audio); der Lauf lehnt alles andere vor der Anfrage ab. |

Wie bei `spec run` liegen Gespräche in `.grauwolf/conversations` (Option `--conversations-dir`) oder in einer Datenbank (`--conversations-db sqlite://chats.db` oder `postgres://…`).

### `POST /run/stream` – live mitverfolgen

Derselbe Body wie bei `/run`, aber die Antwort ist ein Strom von **Server-Sent Events** (SSE, ein einfaches Format, in dem der Server benannte Ereignisse nacheinander sendet). Mit `curl -N` siehst du sie sofort:

```bash
curl -N http://127.0.0.1:8080/run/stream \
  -H 'Content-Type: application/json' \
  -d '{ "input": "Lege eine Notiz vacation.md mit drei Packtipps an." }'
```

Etwa so:

```text
event: agent
data: {"event":"agent_starting","input":"Lege eine Notiz vacation.md mit drei Packtipps an."}

event: agent
data: {"event":"node_starting","path":["call_llm"],"input":"Lege eine Notiz …"}

event: agent
data: {"event":"tool_call_starting","call":{"id":"call_1","tool":"write_file","args":{"path":"vacation.md","content":"…"}}}

event: output
data: "Ich habe vacation.md mit drei Packtipps angelegt."
```

| Ereignis | Inhalt |
|---|---|
| `agent` | Jedes Ereignis des Laufs als JSON mit dem Feld `event` (`agent_starting`, `node_starting`, `llm_call_completed`, `tool_call_starting`, `agent_completed` …) – dieselben Ereignisse wie bei `--events`. |
| `text` | Textstücke, sobald ein Node `llm_request_streaming` sie liefert. |
| `approval` | Ein Tool wartet auf Freigabe (siehe [Freigaben](#freigaben-ein-mensch-entscheidet)). |
| `output` | Am Ende: das Ergebnis als JSON. |
| `error` | Am Ende: die Fehlermeldung als JSON-Text. |

### A2A und MCP im selben Dienst

Derselbe Dienst spricht zwei weitere Protokolle, über die andere Agenten und Tools ihn erreichen können ([Kapitel 8](08-externe-tools-und-agenten.md) zeigt die Gegenseite):

- **A2A** (Agent-to-Agent): Die *Agent Card* unter `GET /.well-known/agent-card.json` beschreibt den Agenten (Name, Beschreibung aus dem System-Prompt, Adresse des Endpunkts). Aufgaben werden als JSON-RPC an `POST /a2a` geschickt (Methoden `SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask`).
- **MCP** (Model Context Protocol): `POST /mcp` bietet den Agenten als ein Tool an, das nach der Spec benannt ist; bei `input: string` heißt sein Argument `request`. Dazu kommen **Ressourcen** – die Spec selbst (`grauwolf://<name>/spec`) und die Dokumente aus `knowledge.sources` (`grauwolf://<name>/knowledge/<path>`) – sowie ein **Prompt** `<name>` mit dem Argument `request`: der System-Prompt des Agenten zusammen mit der Aufgabe, für Programme, die Prompts direkt nutzen.

A2A und MCP laufen wie `/run`: mit den Tools und dem Gedächtnis des Benutzers aus dem Token und mit dem frisch geladenen Tool-Ordner.

```bash
curl http://127.0.0.1:8080/.well-known/agent-card.json

curl -s http://127.0.0.1:8080/a2a -H 'Content-Type: application/json' -d '{
  "jsonrpc": "2.0", "id": 1, "method": "SendMessage",
  "params": { "message": { "role": "ROLE_USER", "messageId": "m1",
                           "parts": [ { "text": "Welche Notizen gibt es?" } ] } } }'

curl -s http://127.0.0.1:8080/mcp -H 'Content-Type: application/json' -d '{
  "jsonrpc": "2.0", "id": 1, "method": "tools/call",
  "params": { "name": "notebook", "arguments": { "request": "Welche Notizen gibt es?" } } }'
```

Die A2A-Antwort enthält einen `task` mit `status.state` `TASK_STATE_COMPLETED` und der Antwort im Artefakt `answer`. Die `contextId` einer Nachricht wird zum Gespräch.

Die Agent Card nennt die gebundene Adresse, z. B. `http://127.0.0.1:8080/a2a`. Läuft der Dienst hinter einem Reverse-Proxy, gib die öffentliche Adresse an:

```bash
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json --public-url https://agents.example.com
```

### Die Chat-API: `/api/chat` und `/api/signature`

Manche Plattformen – AgentWeaver etwa – sprechen einen bereitgestellten Agenten über zwei feste Routen an. Sie sind standardmäßig aus; `spec serve --chat-api` (erzeugte Dienste: `GRAUWOLF_CHAT_API=1`) schaltet sie ein.

- `POST /api/chat` führt den Agenten einmal aus, wie `/run`, aber ohne Gespräch:
  - `input: string` – die Anfrage ist `{"message": "…"}`, die Antwort `{"response": "…"}`.
  - Ein benannter Ein- oder Ausgabetyp – der Anfrage-Body *ist* das Eingabe-Objekt, und die Antwort *ist* das Ausgabe-Objekt.
  - Fehler sind `{"error": "…"}`: `400` bei einem nicht lesbaren Body oder einem Body mit falscher Form, `500`, wenn der Lauf fehlschlug. Budgets und Grenzen gelten wie bei `/run`; Tools, die eine Freigabe brauchen, werden abgelehnt, weil diese Route niemanden fragen kann.
- `GET /api/signature` antwortet `{"signature": …, "observations": …}` und braucht **kein Token**, weil sie keine Geheimnisse enthält. `signature` ist der Inhalt von `agent-signature.json` (Pfad aus `AGENT_SIGNATURE_PATH`, sonst die Datei im Verzeichnis der Spec – in einem Image `/app`), `null`, wenn sie fehlt oder nicht lesbar ist. `observations` ist, was der laufende Agent seit seinem Start gesehen hat: `startedAt`, `models` (je Agent die Modell-IDs, die der Anbieter gemeldet hat, mit `firstSeen` und `lastSeen`) und `mcpTools` (je MCP-Server die angebotenen Tools, jeweils mit Name, Beschreibung und `schemaHash`). Ein neuer Eintrag erscheint nur, wenn sich der Wert ändert. Bei Ollama wird ein Modell als `<model>@<erste 12 Hex-Zeichen seines Digests>` gemeldet (beim Ollama-Server nachgeschlagen, zehn Minuten zwischengespeichert).

Die Authentifizierung von `/api/chat` ist die von `/run`. Eine Plattform, deren Cron-Jobs kein Token schicken, braucht `GRAUWOLF_ALLOW_ANONYMOUS=true` in einem vertrauenswürdigen Netz. In diesem Modus wird `OLLAMA_BASE_URL` benutzt, wenn `OLLAMA_HOST` nicht gesetzt ist. Bei `spec serve` werden Unteragenten unter ihrem eigenen Namen aufgeführt (ein erzeugter Dienst führt nur seinen Hauptagenten auf); eine Datei `agent-names.json` neben der Spec, `{"agents": {"<spec name>": "<agent name>"}}`, benennt Agenten in der Signatur um.

```bash
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json --chat-api
curl -s http://127.0.0.1:8080/api/chat -d '{"message": "Welche Notizen gibt es?"}'
curl -s http://127.0.0.1:8080/api/signature
```

## Den Dienst absichern

### Ein gemeinsames Token

Setze das Token in der Umgebungsvariable `GRAUWOLF_SERVER_TOKEN` – bewusst nicht als Argument, damit es nicht in der Shell-Historie oder der Prozessliste landet:

```bash
export GRAUWOLF_SERVER_TOKEN="$(openssl rand -hex 32)"
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json --addr 0.0.0.0:8080
```

Jetzt verlangen `/run`, `/run/stream`, `/approvals/…`, `/a2a` und `/mcp` den Header `Authorization: Bearer <token>`; sonst antwortet der Dienst mit `401` und `{ "error": "token missing or wrong" }`. Nur `/health` und die Agent Card unter `/.well-known/…` bleiben offen.

```bash
curl -s http://server:8080/run \
  -H "Authorization: Bearer $GRAUWOLF_SERVER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{ "input": "Hallo", "user_id": "anna" }'
```

Ohne Token und ohne Benutzerdatei startet der Dienst nicht auf einer Adresse, die von außen erreichbar ist:

```text
0.0.0.0:8080 is reachable from outside: set a token in GRAUWOLF_SERVER_TOKEN, pass --users or bind to 127.0.0.1
```

Der Dienst spricht nur HTTP. Für HTTPS setzt du einen Reverse-Proxy (z. B. nginx, Caddy) davor.

### Benutzer-Tokens

Das gemeinsame Token ist für ein vertrauenswürdiges Backend gedacht: Wer es hat, darf eine beliebige `user_id` angeben. Sollen Endnutzer direkt auf den Dienst zugreifen, gib jedem von ihnen ein **eigenes Token**:

```bash
grauwolf token anna
```

Ausgabe etwa:

```text
gw_5d1c0e7a9b3f4a2e8c6d0b1a2f3e4d5c
"anna": "sha256:9f2b…"
```

Die erste Zeile ist das Token – gib es Anna; es wird nur jetzt angezeigt. Die zweite Zeile kommt in die **Benutzerdatei**, die nur die SHA-256-Hashes enthält (Vorlage: [`beispiele/09-betrieb/benutzer.json`](beispiele/09-betrieb/benutzer.json)):

```json
{
  "anna": "sha256:9f2b…",
  "ben": "sha256:41c7…"
}
```

```bash
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json --addr 0.0.0.0:8080 \
  --users docs/handbuch/beispiele/09-betrieb/benutzer.json
```

Eine Anfrage mit Annas Token läuft automatisch als `anna`: Die `user_id` wird aus dem Token gesetzt. Nennt der Body eine andere `user_id`, antwortet der Dienst mit `403` (`user_id does not match the token`). Gemeinsames Token und Benutzerdatei kannst du kombinieren.

### Wem gehört was?

- **Gespräche** gehören dem Benutzer: `conversation_id` `trip` von Anna und `trip` von Ben sind zwei verschiedene Gespräche (im Verzeichnis unter `users/<hex>/trip.json`). Ohne `user_id` ist es ein gemeinsames Gespräch.
- **Gedächtnis**: Mit `long_term_memory` in der Spec arbeiten `remember`/`recall` und das automatische Gedächtnis im Bereich des Benutzers ([Kapitel 7](07-gespraeche-und-wissen.md)).
- Ohne Token vertraut der Dienst der `user_id` im Body. Dann betreibe ihn nur lokal oder hinter einem Backend, das die `user_id` setzt.

Bei A2A gehört eine Aufgabe mit Benutzer-Token dem Benutzer des Tokens: Die `contextId` führt in dessen eigenes Gespräch, auch wenn ein anderer Benutzer dieselbe `contextId` wählt, und `GetTask` und `CancelTask` finden nur die eigenen Aufgaben – fremde Aufgaben gelten als unbekannt (`-32001`). Aufgaben- und Kontext-IDs sind zufällig (128 Bit) und nicht zu erraten; höchstens die letzten 1000 Aufgaben werden behalten. Mit dem gemeinsamen Token oder ohne Schutz teilen sich alle Aufrufer einen Bereich.

Da A2A und MCP wie `/run` laufen, sieht ein Benutzer über alle drei Wege dieselben Tools und dasselbe Gedächtnis: bei A2A ist der Benutzer der Eigentümer der Aufgabe, bei MCP der Benutzer aus dem Token.

### Agenten rufen Agenten mit SPIFFE

Wo im Cluster SPIRE läuft, brauchen Agenten kein gemeinsames Geheimnis, um sich gegenseitig aufzurufen (ADR-0109): Jeder Agent hat eine SPIFFE-Identität (`spiffe://agentswork.de/ns/reviewer/sa/reviewer`), die die Workload API unter `SPIFFE_ENDPOINT_SOCKET` als kurzlebiges, automatisch erneuertes Zertifikat ausgibt. Benutzer-Tokens bleiben für Menschen; SPIFFE ist ein zusätzlicher Zugang.

**Aufrufen.** In der Spec des Aufrufers `auth.spiffe` an einem `agent_tools`- oder `mcp_servers`-Eintrag mit `url`:

```json
"agent_tools": {
  "reviewer": {
    "url": "https://reviewer.reviewer.svc:8443",
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
  }
},
"mcp_servers": {
  "tickets": {
    "url": "https://tickets-mcp.tools.svc/mcp", "tools": ["search"],
    "auth": { "spiffe": { "mode": "jwt", "audience": "tickets-mcp" } }
  }
}
```

Mit dem Standard `mtls` legt der Agent sein Zertifikat vor und akzeptiert den Server nur, wenn dessen SPIFFE-ID die angegebene `id`, eine der `ids` oder irgendeine ID aus `trust_domain` ist; die URL muss `https://` sein. Mit `"mode": "jwt"` sendet er ein frisches JWT-SVID für `audience` als `Authorization: Bearer`, für Ziele hinter einem TLS-terminierenden Ingress. Dasselbe `auth` gilt für `agent`-Knoten und `for_each` über entfernte Agenten, im Interpreter wie im erzeugten Code. Ohne `SPIFFE_ENDPOINT_SOCKET` schlägt das Laden der Spec fehl und nennt den Unteragenten bzw. Server. Referenz: [`auth.spiffe`](referenz-spec.md#authspiffe-in-agent_toolsid-und-mcp_serversid).

**Aufgerufen werden.** `GRAUWOLF_SPIFFE=1` (`spec serve --spiffe`) fügt einen zweiten Listener hinzu:

| Port | Wer darf kommen | Woher man den Aufrufer kennt |
|---|---|---|
| HTTP, `8080` | Menschen mit Tokens, der Ingress, Probes und Workloads mit JWT-SVID | `Authorization: Bearer <JWT-SVID>` prüft die Workload API gegen `GRAUWOLF_SPIFFE_AUDIENCE` (Standard: die eigene SPIFFE-ID des Dienstes) |
| mTLS, `GRAUWOLF_SPIFFE_PORT` (Standard `8443`) | Workloads mit Zertifikat einer akzeptierten Trust-Domain; Clients **ohne** gültiges Zertifikat werden schon im Handshake abgewiesen | die SPIFFE-ID des Zertifikats |

Der mTLS-Port bedient dieselben Routen wie der HTTP-Port, auch A2A (`/a2a`) und MCP; ein anderer Agent ruft diesen also per mTLS unter `https://<dienst>:8443` auf. Seine A2A-Agent-Card nennt die mTLS-URL als Endpunkt (`GRAUWOLF_SPIFFE_PUBLIC_URL`, Standard `https://localhost:<port>`; setze sie auf die Adresse, unter der Aufrufer ihn erreichen, etwa den Dienstnamen im Cluster), weil ein A2A-Client seine Nachrichten an den Endpunkt der Card schickt. Der zusätzliche A2A-Port eines generierten Dienstes (`A2A_PORT`) bleibt ein einfacher Port: JWT-SVIDs gehen dort, mTLS nicht – dafür nimmst du den mTLS-Port.

`/health` ist an beiden Ports offen. Ohne `GRAUWOLF_SPIFFE` ändert sich nichts; mit, aber ohne Socket schlägt der Start nach `SPIFFE_FETCH_TIMEOUT` fehl und nennt `SPIFFE_ENDPOINT_SOCKET`. Akzeptiert wird nur die eigene Trust-Domain; `GRAUWOLF_SPIFFE_TRUST_DOMAINS` ist für die Föderation reserviert.

**MCP ist in beiden Richtungen gleich geschützt.** Einen MCP-Server über HTTP rufst du mit `mcp_servers.<name>.auth.spiffe` auf (`mtls` mit `id`, `ids` oder `trust_domain`, oder `jwt` mit `audience`), genau wie bei `agent_tools`, für Streamable HTTP und für `"transport": "sse"` gleichermaßen. Als MCP-Server aufgerufen wirst du über die Route `/mcp`: Sie liegt am mTLS-Port hinter derselben Aufruferprüfung (Zertifikat, dann JWT-SVID, dann die Zuordnungsdatei); ein Desktop-Assistent oder ein anderer Agent, der deinen Agenten als Tool nutzt, wird also authentifiziert, einem Benutzer zugeordnet und wie jeder A2A-Aufrufer als `caller` gezeigt. Ein Agent, der einen anderen Agenten als MCP-Tool aufruft:

```json
"mcp_servers": {
  "reviewer": {
    "url": "https://reviewer.reviewer.svc:8443/mcp",
    "tools": ["reviewer"],
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
  }
}
```

Meldet sich der Server mit einer anderen Identität als `id`, scheitert der Aufruf im Handshake, und die Meldung nennt beide: `TLS handshake with https://reviewer.reviewer.svc:8443/mcp failed: the server is spiffe://…/sa/reviewer, expected spiffe://…/sa/other; the call was not executed`. `no connection to … (refused or given up after 30 s)` ist ein anderer Fall: Am Port antwortet nichts. Siehe [Kapitel 8](08-externe-tools-und-agenten.md#die-verbindung-mit-spiffe-absichern).

**Wer ist das?** Eine authentifizierte ID ist noch nicht autorisiert. `GRAUWOLF_SPIFFE_USERS` (`--spiffe-users`) nennt eine Datei, die IDs auf Benutzer abbildet:

```json
{
  "spiffe://agentswork.de/ns/coordinator/sa/coordinator": "coordinator",
  "spiffe://agentswork.de/ns/batch/*": "batch-jobs"
}
```

Ein Eintrag ist eine genaue ID oder ein Präfix mit `/*` am Ende, das nur ganze Pfadsegmente trifft (`…/ns/batch/*` deckt `…/ns/batch/sa/x`, aber nicht `…/ns/batchy/sa/x`); der genaueste Eintrag gewinnt. Eine authentifizierte ID ohne Eintrag bekommt `403`, ebenso eine `user_id` im Body, die vom zugeordneten Benutzer abweicht. Der zugeordnete Benutzer ist der Benutzer der Anfrage wie bei einem Benutzer-Token: Gespräche, Gedächtnis, Freigaben und Tool-Speicher je Benutzer bleiben getrennt ([Wem gehört was?](#wem-gehört-was)). Die Datei wird beim Start gelesen und danach alle zwei Sekunden neu, wenn sie sich ändert, auch wenn ein ConfigMap-Update in Kubernetes den Symlink `..data` austauscht; eine ungültige neue Datei wird gemeldet, die alte bleibt. Ohne Datei wird jeder Aufrufer mit ID abgewiesen.

**In Ereignissen und Spans.** Ein Aufruf, der durch eine SPIFFE-ID authentifiziert wurde (Zertifikat oder JWT-SVID), setzt das Feld `caller` – die SPIFFE-ID des Aufrufers, nie ein Token – in das Ereignis `agent_starting`. So zeigen `--events`, die `agent`-Ereignisse von `/run/stream`, Logs und der OpenTelemetry-Span `invoke_agent` (Attribut `grauwolf.caller`), wer den Agenten aufgerufen hat. Aufrufe mit einem Benutzer-Token oder dem gemeinsamen Token haben kein `caller`. Über A2A und MCP wird dasselbe Feld gesetzt.

```json
{"event":"agent_starting","input":"Review this.","caller":"spiffe://agentswork.de/ns/coordinator/sa/coordinator"}
```

Reihenfolge am HTTP-Port: ein Client-Zertifikat (nur am mTLS-Port), dann ein Bearer, der ein JWT ist (drei Teile, nicht `gw_…`, nicht das gemeinsame Token), dann gemeinsames Token und Benutzer-Tokens wie bisher.

## Container-Images

Das Repository enthält Dockerfiles in `docker/` und einen GitHub-Workflow (`.github/workflows/images.yml`), der sie baut und prüft. Baue aus dem Wurzelverzeichnis des Repositories.

### Das CLI-Image

```bash
docker build -f docker/cli.Dockerfile -t grauwolf .
docker run --rm grauwolf --version
docker run --rm -v "$PWD:/work" -w /work grauwolf spec validate examples/specs/echo.json
docker run --rm -p 8080:8080 -v "$PWD:/work" -w /work -e GRAUWOLF_SERVER_TOKEN=secret \
  grauwolf spec serve examples/specs/echo.json --addr 0.0.0.0:8080
```

Das Standardziel `runtime` ist `debian:bookworm-slim` mit `ca-certificates`, läuft als Nicht-Root-Benutzer `nonroot` (uid 65532) und hat den Einstiegspunkt `grauwolf`. Es enthält **keine Rust-Toolchain**: Das reicht für `spec validate`, `serve`, `run`, `chat` und `mcp` bei Specs ohne eigene `tools`. Tool-Code wird beim Laden einer Spec kompiliert; für solche Specs (und für `spec generate`) nimmst du daher das Ziel `toolchain`, das Rust und das Target `wasm32-unknown-unknown` ergänzt (`--target toolchain`; das Image ist groß).

### Ein Agent-Image aus einer Spec

`docker/agent.Dockerfile` erzeugt das Crate mit `--service` ([Kapitel 11](11-code-erzeugen.md)), baut es im Release-Modus und kopiert nur das Binary `agent` in ein kleines Image:

```bash
docker build -f docker/agent.Dockerfile --build-arg SPEC=examples/specs/echo.json \
  --target runtime -t echo-agent .
docker run --rm -p 8080:8080 -e GRAUWOLF_SERVER_TOKEN=secret echo-agent
curl localhost:8080/health
```

| Build-Argument / Ziel | Bedeutung |
|---|---|
| `--build-arg SPEC=<path>` | Spec im Build-Kontext (Standard `examples/specs/echo.json`) |
| `--target runtime` | Standard: `gcr.io/distroless/cc-debian12:nonroot` – keine Shell, Benutzer 65532; auch für Specs mit Git-Tools (libgit2 ist Teil des Binarys) |

Das ganze Verzeichnis der Spec wird nach `/app` kopiert, das Arbeitsverzeichnis und `GRAUWOLF_DATA_DIR`: Dateien, die die Spec relativ zu sich nennt (Wissensquellen, der `store` des Gedächtnisses, die Wurzel der Datei-Tools), müssen daneben liegen. `/app` ist für den Dienstbenutzer beschreibbar; hänge dort ein Volume ein (`-v agent-data:/app`), wenn das Langzeitgedächtnis überleben soll; dasselbe gilt für Gespräche mit `GRAUWOLF_CONVERSATIONS_DIR=conversations` (standardmäßig bleiben sie im Speicher des Prozesses; `GRAUWOLF_CONVERSATIONS_DB` nimmt stattdessen eine Datenbank-URL). Tools werden beim Bauen nach WASM kompiliert und in das Binary eingebettet.

Der Dienst lauscht auf Port 8080 (`ENV SERVER_PORT=8080`, `EXPOSE 8080`) und wird über die Umgebungsvariablen aus [Kapitel 11](11-code-erzeugen.md#als-http-dienst---service) konfiguriert: `GRAUWOLF_SERVER_TOKEN` (oder `GRAUWOLF_USERS_FILE`), Anbieter-Schlüssel wie `OPENAI_API_KEY` oder `OLLAMA_HOST`, `A2A_PORT` und so weiter. Weil er an `0.0.0.0` bindet, startet er ohne Token nicht, außer du setzt `GRAUWOLF_ALLOW_ANONYMOUS=true`. SIGTERM (`docker stop`, Kubernetes) beendet ihn, nachdem laufende Anfragen fertig sind.

### Das Builder-Image: Agent-Bau offline in Minuten

`docker/agent.Dockerfile` kompiliert bei jedem Bau alle Abhängigkeiten und braucht crates.io. Für eine Plattform, die viele Agenten baut (etwa mit kaniko, mit Zeitlimit und ohne Netz außer der Registry), gibt es das **Builder-Image** `ghcr.io/agentsw0rk/grauwolf-builder:<version>`:

```bash
docker build -f docker/builder.Dockerfile --build-arg GRAUWOLF_VERSION=0.1.0 \
  -t grauwolf-builder:0.1.0 .
```

Es enthält die Rust-Toolchain mit `wasm32-unknown-unknown`, die CLI `grauwolf`, die grauwolf-Quellen genau dieser Version in `/opt/grauwolf/crates`, **alle Abhängigkeiten vendored** in `/opt/grauwolf/vendor` (`$CARGO_HOME/config.toml` ersetzt crates.io durch sie und setzt `net.offline`) und ein **vorgebautes Target-Verzeichnis** `/opt/grauwolf/target`: Die Abhängigkeiten eines erzeugten Dienstes sind schon kompiliert. Das Image ist groß (mehrere GiB) und wird einmal pro Knoten gezogen.

Ein Agent wird in zwei Schritten gebaut, keiner braucht ein Netz:

```bash
# 1. Crate und sein Dockerfile im Builder-Image erzeugen (Tool-Code wird hier kompiliert).
docker run --rm -v "$PWD:/work" -w /work grauwolf-builder:0.1.0 \
  grauwolf spec generate spec.json --dockerfile --out agent --builder-image grauwolf-builder:0.1.0
# 2. Agent-Image bauen; nur das Agent-Crate wird kompiliert.
docker build --network none agent
```

`--dockerfile` ([Kapitel 11](11-code-erzeugen.md#dockerfile---dockerfile)) schreibt ein `Dockerfile`, das `FROM` das Builder-Image startet, mit `cargo build --release --offline --features service` baut und das Binary in ein distroless-Runtime-Image kopiert (Debian slim mit `git` für Specs mit Git-Tools). Die Dateien, die die Spec zur Laufzeit liest, werden nach `app/` kopiert und werden zu `/app`. Ohne `--builder-image` heißt das Image `ghcr.io/agentsw0rk/grauwolf-builder:<Version der CLI>`; nimm also eine CLI und ein Builder-Image derselben Version: Die vorgebauten Abhängigkeiten passen nur zu den grauwolf-Quellen dieser Version. Das `Dockerfile` erzwingt das (der Builder setzt `GRAUWOLF_VERSION`; eine Abweichung lässt den ersten Bau-Schritt mit einer Meldung scheitern, die beide Versionen nennt; `--allow-builder-mismatch` lässt die Prüfung weg). Ein Tool, das den Bau steuert, kann beides mit `grauwolf version --json` lesen: `{"version": "0.1.0", "builder_image": "ghcr.io/agentsw0rk/grauwolf-builder:0.1.0"}`. Halte das erzeugte Verzeichnis aus jedem anderen Build-Kontext heraus: Es enthält das Crate, das `Dockerfile` und `app/`.

### Was der Workflow veröffentlicht

Bei jedem Push auf `main` und jedem Pull Request baut der Workflow das CLI-Image und das Agent-Image für die Spec `echo` (beide Ziele) für `linux/amd64`, führt `grauwolf --version` aus, startet den Agenten und prüft `/health` und die Agent Card. Ein zweiter Job baut das Builder-Image **einmal pro Architektur auf einem nativen Runner** (`ubuntu-24.04` für amd64, `ubuntu-24.04-arm` für arm64, ohne Emulation), erzeugt darin den Agenten `echo` und den Agenten `calculator` (Tool-Code als Quellen), baut sie mit `docker build --network none` (das belegt, dass der Bau offline läuft, die [Versionsprüfung](11-code-erzeugen.md#dockerfile---dockerfile) besteht und die Dauer ausgibt), startet sie und prüft `/health`.

Bei einem Tag `v<version>` (der Tag muss der Version in `Cargo.toml` entsprechen) schiebt der Workflow zusätzlich nach `ghcr.io/agentsw0rk`, authentifiziert mit dem `GITHUB_TOKEN` (`packages: write`): das CLI-Image `grauwolf`, das Tool-Programm `grauwolf-tools` und das Builder-Image `grauwolf-builder`, jeweils nativ für `linux/amd64` und `linux/arm64` gebaut. Jede Architektur wird für sich geschoben und über ihren Digest identifiziert; ein letzter Job führt die beiden Digests zu einem Multi-Arch-Tag zusammen (`docker buildx imagetools create`): `:<version>`, und `:latest` für das CLI- und das Tools-Image (nicht für den Builder, der immer zur CLI passen muss, die erzeugt). Alle Images tragen die Labels `org.opencontainers.image.source`, `.version` und `.revision`. Agent-Images hängen von deiner eigenen Spec ab und werden nicht veröffentlicht; baue sie mit den Dockerfiles oben.

Die Pakete sind **privat**. Zum Abrufen meldest du dich mit einem persönlichen Access-Token mit `read:packages` an:

```bash
echo "$TOKEN" | docker login ghcr.io -u <github-user> --password-stdin
docker pull ghcr.io/agentsw0rk/grauwolf-builder:<version>
```

Ein Kubernetes-Cluster, der diese Images baut oder betreibt (etwa der von AgentWeaver), braucht ein **Image-Pull-Secret**: `kubectl create secret docker-registry ghcr --docker-server=ghcr.io --docker-username=<user> --docker-password=<token>`, das im Pod oder im Service Account referenziert wird. Native arm64-Runner sind für öffentliche Repositories kostenlos; für ein privates Repository brauchen sie womöglich einen GitHub-Plan mit arm64-Runnern (größere Runner), sonst warten die `arm64`-Jobs auf einen Runner, der nie kommt; deshalb laufen sie nur bei Tags und manuellen Läufen, Pushes und Pull Requests bauen amd64. Wie ein Release geschnitten wird, steht im [Entwicklerhandbuch](../entwickler/02-entwicklungsumgebung.md#210-ein-release-schneiden).

## Freigaben: ein Mensch entscheidet

Manche Tools sollten nicht ohne Zustimmung laufen – `write_file` etwa oder alles, was Geld kostet. Mit `--confirm-tools` machst du Tools **freigabepflichtig** (mehrere durch Kommas getrennt).

### Auf der Konsole

```bash
grauwolf spec run docs/handbuch/beispiele/09-notizbuch.json "Lösche alles aus einkauf.md außer Brot." \
  --confirm-tools write_file
```

Vor jedem Aufruf fragt grauwolf auf stderr:

```text
Run tool write_file with {"content":"# Einkauf\n\n- Brot\n","path":"einkauf.md"}? [y/N]
```

`y` oder `yes` führt das Tool aus (`j` und `ja` funktionieren auch); alles andere lehnt ab, und das Modell bekommt `The user refused the call of write_file.` als Tool-Ergebnis. Das funktioniert mit `spec run`, `spec chat` und `spec resume`. Bei `spec mcp` ist die Option verboten, weil stdin dort das MCP-Protokoll trägt.

### Im Dienst

```bash
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json --confirm-tools write_file
```

Freigaben brauchen einen Rückkanal, deshalb gibt es sie nur über `/run/stream`. Vor dem Aufruf kommt ein `approval`-Ereignis, und der Lauf wartet:

```text
event: approval
data: {"id":"8c1f0a…","tool":"write_file","args":{"path":"vacation.md","content":"…"}}
```

Du entscheidest mit `POST /approvals/{id}`:

```bash
# freigeben
curl -X POST http://127.0.0.1:8080/approvals/8c1f0a… \
  -H 'Content-Type: application/json' -d '{ "approved": true }'

# ablehnen, mit einem Grund für das Modell
curl -X POST http://127.0.0.1:8080/approvals/8c1f0a… \
  -H 'Content-Type: application/json' \
  -d '{ "approved": false, "reason": "Bitte keine neuen Dateien anlegen." }'
```

Antwort: `204 No Content`. Danach läuft der Strom weiter.

- Ohne `reason` bekommt das Modell `Denied by the user`.
- Kommt innerhalb von 5 Minuten keine Entscheidung, wird der Aufruf abgelehnt (`No approval given within 300 s`).
- Lief die Anfrage mit einer `user_id`, muss die Entscheidung dieselbe `user_id` tragen; sonst – und bei unbekannten IDs – antwortet der Dienst mit `404`. Mit einem Benutzer-Token nimmt der Dienst die `user_id` aus dem Token.
- `/run`, A2A und MCP haben keinen Rückkanal: Freigabepflichtige Tools werden dort immer abgelehnt, und das Modell erfährt `tool 'write_file' needs an approval that nobody can give here`.

Offene Freigaben liegen im Arbeitsspeicher des Dienstes. Betreibst du mehrere Instanzen hinter einem Load-Balancer, muss die Entscheidung bei derselben Instanz ankommen (Sticky Sessions).

## Einbindung in Claude Desktop

`grauwolf spec mcp` stellt den Agenten als MCP-Server über stdin/stdout bereit – genau das, was Desktop-Programme wie Claude Desktop erwarten. Alle Meldungen gehen auf stderr; stdout gehört dem Protokoll. Ein Eintrag in der Konfiguration von Claude Desktop sieht etwa so aus:

```json
{
  "mcpServers": {
    "notebook": {
      "command": "/home/you/.cargo/bin/grauwolf",
      "args": [
        "spec", "mcp", "/home/you/agents/09-notizbuch.json",
        "--cache", "/home/you/.grauwolf/tools"
      ]
    }
  }
}
```

Auch hier gibt es neben dem Tool die Ressourcen des Agenten (Spec, Wissen) und seinen Prompt; der Tool-Ordner wird vor jedem Aufruf neu gelesen, und das automatische Gedächtnis gilt im gemeinsamen Bereich.

Verwende absolute Pfade: Claude Desktop bestimmt das Arbeitsverzeichnis, und Standardordner wie `.grauwolf/tools` sind relativ dazu. Pfade *in* der Spec (`root`, `sources`, …) sind wie immer relativ zur Spec-Datei. Zugangsdaten wie `ANTHROPIC_API_KEY` müssen in der Umgebung des Prozesses stehen.

## Beobachten

### Ereignisse als JSON-Zeilen

`--events` schreibt jedes Ereignis des Laufs als JSON-Zeile auf stderr – ideal für `jq` oder ein Logging-System. Die Antwort bleibt auf stdout:

```bash
grauwolf spec run docs/handbuch/beispiele/09-notizbuch.json "Welche Notizen gibt es?" --events 2> events.jsonl
```

`events.jsonl` etwa:

```text
{"event":"agent_starting","input":"Welche Notizen gibt es?"}
{"event":"node_starting","path":["call_llm"],"input":"Welche Notizen gibt es?"}
{"event":"tool_call_starting","call":{"id":"call_1","tool":"list_directory","args":{"path":"."}}}
{"event":"tool_call_completed","result":{"id":"call_1","tool":"list_directory","content":"einkauf.md"}}
{"event":"agent_completed","output":"Es gibt eine Notiz: einkauf.md.","usage":{"input_tokens":812,"output_tokens":46}}
```

`usage` erscheint, wenn der Anbieter Token-Zahlen meldet; `cost` zusätzlich mit `--prices`.

Ein Dienst aus `spec generate --service` hat keine Flags; er schreibt dieselben Zeilen (seine eigenen und die seiner
Unteragenten) auf stderr, wenn die Umgebung `GRAUWOLF_EVENTS=1` (oder `true`) sagt, und in einem Cluster setzt
`grauwolf-k8-ops agent deploy --events` das ([Kapitel 13](13-cluster-einrichten.md#events-im-pod-log)). Jeder
andere Wert ist ein Startfehler, der die Variable nennt.

### OpenTelemetry

Mit `--otel` exportiert grauwolf jeden Lauf als **Spans** (Zeitabschnitte eines Traces) über OTLP/HTTP – an einen OpenTelemetry Collector, Jaeger, Langfuse und andere. Ziel und Header kommen aus den Standardvariablen:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
export OTEL_SERVICE_NAME=notebook        # Standard: grauwolf
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json --otel
```

Pro Lauf siehst du einen Span `invoke_agent <name>`, darunter `node <path>` je Node, `chat <model>` je LLM-Aufruf und `execute_tool <name>` je Tool-Aufruf. Unteragenten melden eigene `invoke_agent`-Spans, als Kinder des Aufrufs, der sie startet (für Agenten auf einem anderen Dienst siehe „Ein Trace über Agenten hinweg“ unten). Inhalte (Antworten, Tool-Argumente und -Ergebnisse) können personenbezogene Daten enthalten und werden deshalb nur mit `--otel-content` aufgezeichnet.

**Ein Trace über Agenten hinweg.** Ruft ein Agent mit `--otel` einen anderen Agenten über A2A oder MCP (HTTP) auf,
schickt er den W3C-Header `traceparent` (und `tracestate`, falls vorhanden) mit der Anfrage. Der aufgerufene Agent
setzt diesen Trace fort: Sein Span `invoke_agent` ist ein Kind des Spans, der den Aufruf gemacht hat – des Spans
`execute_tool <name>` des Tool-Aufrufs oder des Spans `node <path>` eines `agent`-Knotens. Aufrufer und
Aufgerufener sind dann **ein Trace** in Jaeger oder Tempo, sofern beide an denselben Collector exportieren. Für
einen Agenten ohne `--otel` ändert sich nichts: Er schickt keinen Header, und einen empfangenen Header ignoriert
er. Ein fehlender oder fehlerhafter `traceparent` beginnt wie bisher einen neuen Trace. Der Header verknüpft nur
Spans und gewährt nichts, deshalb akzeptiert ihn der Dienst von jedem Aufrufer, der die Authentifizierung
bestanden hat. `grauwolf.caller` bleibt am Span, du siehst also weiterhin, wer aufgerufen hat.

`--otel` gibt es bei `spec run`, `spec resume`, `spec serve` und `spec mcp`; `spec chat` lehnt es ab.

**Erzeugte Dienste.** Ein Dienst aus `spec generate --service` hat keine Flags; er exportiert dieselben Spans, wenn die Umgebung es sagt: `GRAUWOLF_OTEL=1` (oder `true`) und `GRAUWOLF_OTEL_CONTENT=1` für Inhalte. Ziel und Dienstname kommen aus den Variablen oben. Der Dienst setzt einen eingehenden `traceparent` fort, zeichnet `grauwolf.caller` und die Spans seiner Unteragenten auf und sendet bei SIGTERM und Strg+C die noch offenen Spans. Jeder andere Wert von `GRAUWOLF_OTEL` ist ein Startfehler, der die Variable nennt.

## Checkpoints: abgebrochene Läufe fortsetzen

Lange Läufe können abbrechen – Netz weg, Rechner neu gestartet. Mit `--run-id` speichert grauwolf einen Checkpoint **vor jedem Node** (Verlauf, Position im Graphen, Speicher). Das Beispiel ist ein Ablauf aus zwei Schritten ([`beispiele/09-entwurf.json`](beispiele/09-entwurf.json)):

```json
{
  "spec_version": 1,
  "name": "draft",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du schreibst sachliche Texte auf Deutsch.",
  "graph": {
    "nodes": [
      { "id": "draft", "kind": "llm_request" },
      { "id": "shorten", "kind": "llm_request" }
    ],
    "edges": [
      { "from": "start", "to": "draft" },
      { "from": "draft", "to": "shorten", "when": { "kind": "on_assistant_message" } },
      { "from": "shorten", "to": "finish", "when": { "kind": "on_assistant_message" } }
    ]
  }
}
```

```bash
grauwolf spec run docs/handbuch/beispiele/09-entwurf.json "Schreib einen Absatz über Wölfe." --run-id woelfe-1
# … Unterbrechung, z. B. mit Strg+C während "shorten" …
grauwolf spec resume docs/handbuch/beispiele/09-entwurf.json --run-id woelfe-1
```

`spec resume` setzt am letzten Checkpoint fort; was davor lag, läuft nicht noch einmal.

- Checkpoints liegen in `.grauwolf/runs/<lauf-id>.json` (`--runs-dir`) oder in einer Datenbank (`--runs-db sqlite://runs.db`). Lauf-IDs bestehen aus Buchstaben, Ziffern, `-` und `_`.
- Tool-Aufrufe zwischen zwei Checkpoints können nach einem Abbruch ein zweites Mal laufen.
- `--run-id` lässt sich nicht mit `--conversation` kombinieren. Mit `--user` nutzt der Lauf das Gedächtnis dieses Benutzers; gib beim Fortsetzen denselben `--user` an (`spec resume … --run-id lauf-1 --user anna`) – der Benutzer steht nicht im Checkpoint; ohne `--user` setzt `spec resume` im gemeinsamen Bereich fort.
- Ansonsten verhalten sich Checkpoint-Läufe wie gewöhnliche Läufe: `spec run --run-id` und `spec resume` laden den Tool-Ordner (`tools_dir`), nutzen das automatische Gedächtnis (bei `spec resume` den gemeinsamen Bereich) und fragen bei Freigaben auf der Konsole nach. Selbst gebaute Tools ([Kapitel 10](10-selbst-gebaute-tools.md)) stehen im Checkpoint und werden beim Fortsetzen neu gebaut. `spec resume` lädt den Tool-Ordner neu – es gelten die Tools zum Zeitpunkt des Fortsetzens.

## Kosten und Budget

[Kapitel 2](02-modelle.md) erklärt im Detail, was Läufe kosten und wie du das begrenzt. Im Betrieb wichtig:

- `--prices <datei>` liest eine Preistabelle (pro Million Tokens, Vorlage: [`beispiele/09-betrieb/preise.json`](beispiele/09-betrieb/preise.json)); Ereignisse und Spans melden dann die Kosten.
- `--max-tokens <n>` und `--max-cost <betrag>` beenden einen Lauf, sobald die Grenze erreicht ist – im Dienst **je Anfrage**, bei `spec chat` je Zeile, bei `spec mcp` je Aufruf. `--max-cost` braucht `--prices`.

```bash
grauwolf spec serve docs/handbuch/beispiele/09-notizbuch.json \
  --prices docs/handbuch/beispiele/09-betrieb/preise.json --max-cost 0.05 --max-tokens 20000
```

## Das Wichtigste in Kürze

- `grauwolf spec serve <spec>` startet einen HTTP-Dienst mit `/run`, `/run/stream` (SSE), `/health`, A2A unter `/a2a` samt Agent Card und MCP unter `/mcp`.
- Ohne Absicherung nur lokal. Für Zugriff von außen setzt du `GRAUWOLF_SERVER_TOKEN` oder erzeugst Benutzer-Tokens für `--users` mit `grauwolf token`; Gespräche und Gedächtnis gehören dann dem Benutzer.
- `--confirm-tools` macht Tools freigabepflichtig: auf der Konsole mit `y` oder `yes`, im Dienst über das SSE-Ereignis `approval` und `POST /approvals/{id}`.
- `docker/cli.Dockerfile` und `docker/agent.Dockerfile` bauen die CLI und einen erzeugten Agenten-Dienst als Container-Images (distroless oder mit git, Nicht-Root, Port 8080); `docker/builder.Dockerfile` baut das Offline-Builder-Image, und `spec generate --dockerfile` schreibt das `Dockerfile`, das darin einen Agenten ohne Netz baut; siehe „Container-Images“.
- `grauwolf spec mcp` bindet den Agenten in Claude Desktop ein.
- `--events`, `--otel` und `--otel-content` machen Läufe sichtbar.
- `--run-id` und `spec resume` setzen unterbrochene Läufe fort.

Weiter mit [Kapitel 10: Selbst gebaute Tools](10-selbst-gebaute-tools.md).
