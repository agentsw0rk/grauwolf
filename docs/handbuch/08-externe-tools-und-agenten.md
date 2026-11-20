# Kapitel 8: Externe Tools und andere Agenten

In [Kapitel 3](03-tools.md) hast du Tools selbst geschrieben. Viele nützliche Tools gibt es aber
schon fertig – als **MCP-Server**. Und manchmal ist das beste Werkzeug für eine Teilaufgabe ein
ganzer anderer Agent. Dieses Kapitel zeigt beides:

- `mcp_servers` bindet Tools fremder MCP-Server ein, lokal gestartet oder im Netz.
- `agent_tools` macht andere Agenten zu Tools – eigene Specs auf deinem Rechner oder entfernte
  Agenten, die das A2A-Protokoll sprechen.

Für das Modell sieht beides aus wie jedes andere Tool. Dein Ablauf braucht also einen
Tool-Kreislauf; die Beispiele nehmen dafür die Vorlage `single_run` aus [Kapitel 4](04-ablaeufe.md).

## MCP-Server lokal starten

Das **Model Context Protocol** (MCP) ist ein offener Standard, über den Programme einem KI-Agenten
Tools anbieten – für Dateien, Git, Datenbanken, Webseiten und vieles mehr. Ein lokaler MCP-Server ist
ein Programm, das grauwolf beim Start des Agenten aufruft und über Standardein- und -ausgabe (stdio)
anspricht:

```json
{
  "spec_version": 1,
  "name": "file_guide",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du beantwortest Fragen zu Dateien im gemeinsamen Ordner. Schlage mit den Tools nach, statt zu raten.",
  "mcp_servers": {
    "files": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
      "tools": ["list_directory", "read_text_file"]
    }
  },
  "strategy": { "kind": "single_run" }
}
```

Die Datei: [`beispiele/08-mcp-dateien.json`](beispiele/08-mcp-dateien.json). Für dieses Beispiel
brauchst du Node.js (`npx`).

| Feld | Bedeutung |
|---|---|
| Schlüssel (`files`) | Name des Servers, klein geschrieben (`[a-z][a-z0-9_]*`) |
| `command` | Programm, das den Server startet |
| `args` | Argumente für das Programm (Standard: keine) |
| `tools` | die Tools des Servers, die dein Agent nutzen darf (Pflicht) |

Der Server läuft im aktuellen Arbeitsverzeichnis – `"."` in `args` meint hier also das Verzeichnis,
in dem du `grauwolf` aufrufst, nicht den Ordner der Spec.

```bash
grauwolf spec run docs/handbuch/beispiele/08-mcp-dateien.json "Welche Kapitel hat das Handbuch?"
```

Etwa so:

```text
Im Ordner docs/handbuch liegen die Kapitel 01-erste-schritte.md bis 11-code-erzeugen.md,
dazu README.md und der Ordner beispiele.
```

**Warum Tools deklarieren?** Ein Server bietet oft viel mehr Tools an, als dein Agent braucht.
grauwolf bietet dem Modell nur die aufgelisteten an. Außerdem kann `spec validate` so Tool-Namen
prüfen, ohne den Server zu starten. Welche Tools ein Server hat, steht in seiner Dokumentation.
Fehlt ein deklariertes Tool beim Start, bricht grauwolf ab und nennt die vorhandenen, etwa so:

```text
MCP server 'files' does not offer tool 'read_file' (available: read_text_file, list_directory, …)
```

**Sicherheit:** Eigene Tools aus [Kapitel 3](03-tools.md) laufen in einer abgeschotteten Sandbox.
MCP-Server dagegen laufen als normale Programme mit deinen Rechten. Nutze nur Server, denen du
vertraust, und gib ihnen nur das Nötige frei (hier: ein Verzeichnis).

### Einem lokalen Server die Identität des Agenten geben: `workload_api`

Ein stdio-Server ist ein Kindprozess des Agenten. Er bekommt die Workload-API-Einstellungen des Agenten
**nicht** von selbst: `SPIFFE_ENDPOINT_SOCKET` und `SPIFFE_FETCH_TIMEOUT` werden aus der Umgebung entfernt, die er
erbt. Ein Server, der sich per SPIFFE bei einem internen Dienst anmeldet, kann sie bekommen (ADR-0110):

```json
"mcp_servers": {
  "internal": { "command": "internal-mcp", "tools": ["search"], "workload_api": true }
}
```

- Nur für `command`-Server; mit `url` ist es ein Validierungsfehler (`WorkloadApiWithoutStdio`).
- Ohne `SPIFFE_ENDPOINT_SOCKET` in der Umgebung des Agenten startet der Agent nicht; der Fehler nennt den Server
  (`MCP server 'internal': `workload_api` needs a workload identity, but SPIFFE_ENDPOINT_SOCKET is not set`).
- **Das Kind bekommt die Identität des Agenten.** Der Kubernetes-Attestor von SPIRE unterscheidet die Prozesse
  eines Pods nicht; alles, was der Agent mit seinen SVIDs darf, darf das Kind auch. Eine eigene Identität für das
  Kind braucht den Unix-Attestor von SPIRE (Selektoren für Benutzer-ID oder Binärpfad): Cluster-Konfiguration,
  die grauwolf nicht übernimmt. Setze `workload_api` nur für Server, denen du so weit vertraust wie dem Agenten.

## MCP-Server im Netz

Viele MCP-Server laufen als Dienst im Internet. Statt `command` gibst du dann `url` an; grauwolf
spricht sie über den Transport „Streamable HTTP“ an:

```json
"mcp_servers": {
  "deepwiki": {
    "url": "https://mcp.deepwiki.com/mcp",
    "tools": ["read_wiki_structure", "ask_wiki_question"]
  }
}
```

Die vollständige Datei: [`beispiele/08-mcp-http.json`](beispiele/08-mcp-http.json).

```bash
grauwolf spec run docs/handbuch/beispiele/08-mcp-http.json \
  "Wie ist das Repository tokio-rs/axum aufgebaut?"
```

Die Antwort fasst etwa die Wiki-Struktur des Repositorys zusammen.

**Ältere Server (HTTP+SSE):** Manche Server sprechen nur den älteren Transport der Protokollversion
2024-11-05, bei dem der Client mit `GET` einen Ereignisstrom öffnet und seine Nachrichten per `POST`
an eine Adresse schickt, die der Server ankündigt. Wähle ihn ausdrücklich mit `transport` (Standard:
`streamable_http`; `transport` funktioniert nur zusammen mit `url`):

```json
"mcp_servers": {
  "legacy": {
    "url": "http://127.0.0.1:8931/sse",
    "transport": "sse",
    "tools": ["reverse"]
  }
}
```

Die vollständige Datei: [`beispiele/08-mcp-sse.json`](beispiele/08-mcp-sse.json). grauwolf rät den
Transport nicht: Wenn du `streamable_http` auf einen alten Server richtest (oder `sse` auf einen
neuen), schlägt die Verbindung mit dem HTTP-Status des Servers fehl. Um es ohne eigenen Server
auszuprobieren, startest du den Beispielserver des Repositorys (ein Tool, `reverse`):
`cargo run -p grauwolf-mcp --features test-sse-server --example serve_sse_mcp`. Hat ein `mcp-server` von AgentWeaver den Transport `sse`, nutze `"transport": "sse"`.

Braucht ein Server Zugangsdaten, gibst du **Kopfzeilen** unter `headers` an. Schreib das Geheimnis
nicht in die Spec, sondern als `${VARIABLE}`; grauwolf setzt beim Start den Wert der
Umgebungsvariablen ein:

```json
"mcp_servers": {
  "doku": {
    "url": "https://mcp.example.com/mcp",
    "headers": { "Authorization": "Bearer ${DOKU_TOKEN}" },
    "tools": ["search"],
    "resources": true
  }
}
```

```bash
DOKU_TOKEN=geheim grauwolf spec run meine-spec.json "…"
```

Fehlt die Variable, bricht der Start mit `missing environment variable DOKU_TOKEN` ab.

**Ressourcen:** Manche Server bieten neben Tools auch **Ressourcen** an – lesbare Inhalte mit einer
Adresse (URI), etwa Dokumente oder Datensätze. Mit `"resources": true` bekommt der Agent dafür zwei
zusätzliche Tools, benannt nach dem Server:

- `doku_list_resources` listet die Ressourcen mit URI, Name, Beschreibung und Typ,
- `doku_read_resource` liest eine Ressource (Argument `uri`).

`resources` gibt es für lokale und entfernte Server.

### Die Verbindung mit SPIFFE absichern

In einem Cluster, in dem SPIRE läuft, brauchst du für einen eigenen Server kein Token. Gib dem Eintrag statt `headers` ein `auth.spiffe`
(beide schließen sich für `Authorization` aus); grauwolf weist sich dann mit einem kurzlebigen Zertifikat aus:

```json
"mcp_servers": {
  "tickets": {
    "url": "https://tickets-mcp.tools.svc:8443/mcp",
    "tools": ["search"],
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/tickets/sa/tickets-mcp" } }
  },
  "wiki": {
    "url": "https://mcp.example.com/mcp",
    "tools": ["search"],
    "auth": { "spiffe": { "mode": "jwt", "audience": "wiki-mcp" } }
  }
}
```

`mtls` (der Standard) legt das Zertifikat vor und akzeptiert den Server nur, wenn dessen SPIFFE-ID die angegebene `id` ist (oder eine der `ids` oder irgendeine ID aus `trust_domain`); die URL muss `https` sein. `"mode": "jwt"` sendet ein JWT-SVID für `audience` als `Authorization: Bearer`, für Server hinter einem Ingress, der TLS beendet. `auth` braucht `SPIFFE_ENDPOINT_SOCKET` und geht nicht mit `command`.

Auch die aufgerufene Seite muss Aufrufer prüfen: Ein grauwolf-Agent braucht `GRAUWOLF_SPIFFE=1` und eine Aufruferdatei (`GRAUWOLF_SPIFFE_USERS`), die deinen Agenten nennt; in einem mit `grauwolf-k8-ops` eingerichteten Cluster `agent allow <ziel> --from <dein agent>`.

Zwei Fehlermeldungen unterscheiden die Ursachen:

```text
MCP over HTTP: TLS handshake with https://tickets-mcp.tools.svc:8443/mcp failed: the server is spiffe://agentswork.de/ns/tickets/sa/other, expected spiffe://agentswork.de/ns/tickets/sa/tickets-mcp; the call was not executed
MCP over HTTP: no connection to https://tickets-mcp.tools.svc:8443/mcp (refused or given up after 30 s); the call was not executed
```

Die erste heißt: Der Server hat sich mit einer anderen Identität gemeldet als `id`; korrigiere `id`. Die zweite heißt: Am Port lauscht nichts, oder eine NetworkPolicy hält dich draußen. Einzelheiten: [Kapitel 9](09-betrieb.md#agenten-rufen-agenten-mit-spiffe), [Kapitel 13](13-cluster-einrichten.md#agenten-sich-gegenseitig-aufrufen-lassen) und die Referenz ([`auth.spiffe`](referenz-spec.md#authspiffe-in-agent_toolsid-und-mcp_serversid)).

### Typische Fehler

```text
…:/mcp_servers/files: MCP server needs exactly one of `command` (local) and `url` (remote)
…:/mcp_servers/files/transport: `transport` only applies to a remote MCP server with `url`
…:/mcp_servers/files/tools/2: tool name 'read_text_file' is used more than once (the other one is the … at …)
```

Alle Tool-Namen einer Spec teilen sich einen Namensraum: Eigene Tools, MCP-Tools, Unteragenten,
Such- und Gedächtnis-Tools dürfen nicht gleich heißen.

## Agenten als Tools: `agent_tools`

Statt einen Agenten alles können zu lassen, baust du ein Team: kleine, spezialisierte Agenten, jeder
mit eigener Spec, eigenem Modell und eigenem System-Prompt. Ein **Koordinator** verteilt die Arbeit,
indem er die anderen wie Tools aufruft.

```json
{
  "spec_version": 1,
  "name": "coordinator",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du koordinierst Anfragen an den Kundenservice. Fehlermeldungen gibst du an create_ticket weiter, Übersetzungen ins Deutsche an translator. Danach fasst du die Ergebnisse kurz für den Nutzer zusammen.",
  "agent_tools": {
    "translator": {
      "spec": "08-team/uebersetzer.json",
      "description": "Übersetzt einen englischen Text ins Deutsche"
    },
    "create_ticket": {
      "spec": "08-team/ticket_erfassen.json",
      "description": "Macht aus einer Fehlermeldung ein Ticket mit Titel, Priorität und Tags"
    }
  },
  "strategy": { "kind": "single_run" }
}
```

Die Datei: [`beispiele/08-koordinator.json`](beispiele/08-koordinator.json). Die beiden Unteragenten
liegen in [`beispiele/08-team/`](beispiele/08-team/):
[`uebersetzer.json`](beispiele/08-team/uebersetzer.json) ist ein einfacher Text-Agent,
[`ticket_erfassen.json`](beispiele/08-team/ticket_erfassen.json) liefert ein `Ticket` wie in
[Kapitel 5](05-strukturierte-daten.md).

| Feld | Bedeutung |
|---|---|
| Schlüssel (`translator`) | Name des Tools, klein geschrieben wie jeder Tool-Name |
| `spec` | Spec-Datei des Unteragenten, relativ zu dieser Spec |
| `description` | wofür der Unteragent da ist; bei `spec` Pflicht, denn danach entscheidet das Modell |

So verhält sich ein Unteragent:

- **Argumente:** Hat er als `input` Text, bekommt das Tool ein Argument `request` mit dem Auftrag.
  Hat er einen eigenen Typ als `input`, sind dessen Felder die Argumente.
- **Ergebnis:** Text kommt unverändert zurück, ein eigener Typ als JSON-Text.
- **Eigener Verlauf:** Jeder Aufruf beginnt frisch, mit Modell und System-Prompt seiner Spec. Er sieht
  nicht, was der Koordinator bisher besprochen hat – schreib also vollständige Aufträge.
- **Fehler** des Unteragenten bekommt das Modell als Tool-Fehler zu sehen und kann darauf reagieren.
- Unteragenten dürfen selbst Tools, MCP-Server, Wissen und weitere Unteragenten haben; grauwolf lädt
  alles beim Start mit.

Die Specs eines Agenten und seiner lokalen Unteragenten (`agent_tools` mit `spec`) bilden einen Baum,
und für den ganzen Baum gilt in jedem Befehl (`spec validate`, `spec build`, `tools lock`, `spec run`,
`spec serve`, `spec generate`) eine Regel-Gruppe:

- **Die Pfadregel.** Die Spec eines Unteragenten muss im Ordner der Wurzel-Spec liegen (der, die du dem
  Befehl gibst): Ein Pfad, der mit `..` aus ihm hinausführt, oder ein absoluter Pfad, ist ein Fehler
  (`sub-agent spec outside the agent's directory`). So lässt sich der ganze Baum in ein Image kopieren.
  Pfade mit `..`, die innerhalb des Ordners bleiben, sind in Ordnung.
- **Zyklen** sind Fehler und nennen die Kette: `sub-agents form a cycle: a.json → b.json → a.json`.
  `spec validate` meldet sie wie ein Lauf.
- **Rauten.** Nennen zwei Agenten dieselbe Unteragenten-Spec, ist es ein einziger Unteragent: einmal
  gelesen, zur Laufzeit einmal geladen und von beiden Eltern geteilt – ein Satz MCP-Server, ein
  Wissensindex.
- **Eine Lockdatei und eine Richtlinie.** Die `grauwolf.lock` und die Tool-Richtlinie (`grauwolf.toml`
  oder `GRAUWOLF_TOOL_POLICY`) neben der Wurzel-Spec gelten für jeden Agenten des Baums, sodass kein
  Unteragent eine laxere Richtlinie mitbringt. Die Einträge der Tool-Pakete und Git-Skills eines
  Unteragenten stehen in der Wurzel-Lockdatei unter `<Pfad der Spec relativ zum Ordner der Wurzel>#<Referenz>`,
  zum Beispiel `agents/writer.json#ghcr.io/acme/tools/math:1.0`; Einträge der Wurzel-Spec behalten die
  einfache Referenz. Eine `grauwolf.lock` neben der Spec eines Unteragenten wird nicht genutzt (eine
  Warnung sagt es). `spec build` und `tools lock` erfassen die Unteragenten ebenfalls. Ein Eintrag, den
  kein Agent des Baums mehr nutzt (etwa nachdem du einen Unteragenten entfernt hast), ist veraltet:
  `--locked` schlägt fehl und nennt ihn; ohne `--locked` wird der Eintrag aus der Lockdatei entfernt.
- Ein entfernter Unteragent (`url`) ist ein Blatt: Er hat keine Spec-Dateien und keine Signaturprüfung.

`spec validate` prüft den ganzen Baum. Fehler darin werden mit dem Dateinamen des Unteragenten
angezeigt:

```bash
grauwolf spec validate docs/handbuch/beispiele/08-koordinator.json
```

Ein Lauf, etwa so:

```bash
grauwolf spec run docs/handbuch/beispiele/08-koordinator.json \
  "Beim Bezahlen mit PayPal erscheint nur eine weiße Seite. Bitte auch auf Deutsch für unseren Dienstleister."
```

```text
I created a ticket: "Blank white page when paying with PayPal" (priority 1, tags: payment,
paypal, checkout). In German for the service provider: "Beim Bezahlen mit PayPal erscheint nur
eine weiße Seite."
```

Mit `--events` siehst du dabei, wann der Koordinator welchen Unteragenten aufruft.

## Entfernte Agenten über A2A

**A2A** (Agent-to-Agent) ist ein offenes Protokoll, über das Agenten verschiedener Frameworks einander
Aufträge geben. Jeder grauwolf-Agent, den du mit `spec serve` startest, ist darüber erreichbar
([Kapitel 9](09-betrieb.md)) – und umgekehrt kann dein Agent entfernte A2A-Agenten als Unteragenten
nutzen. Statt `spec` gibst du dann `url` an:

```json
{
  "spec_version": 1,
  "name": "requester",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du leitest Fragen zur Hausordnung an caretaker weiter und gibst dessen Antwort wieder.",
  "agent_tools": {
    "caretaker": {
      "url": "http://127.0.0.1:8080",
      "headers": { "Authorization": "Bearer ${CARETAKER_TOKEN}" }
    }
  },
  "strategy": { "kind": "single_run" }
}
```

Die Datei: [`beispiele/08-a2a.json`](beispiele/08-a2a.json).

- `url` ist die Basisadresse des Agenten (grauwolf lädt dann `/.well-known/agent-card.json`) oder direkt
  die Adresse seiner **Agent-Karte** – der Selbstbeschreibung des Agenten.
- `headers` funktioniert wie bei MCP, `${VAR}` inklusive.
- `description` ist hier optional; ohne Angabe nimmt grauwolf die Beschreibung aus der Agent-Karte.
- Jeder Aufruf ist eine neue, eigene Nachricht ohne gemerkten Gesprächsverlauf. Stellt der entfernte
  Agent eine Rückfrage, kommt sie als Ergebnis zurück, und das Modell kann darauf eingehen.
- Bietet die Agent-Karte Streaming an (`capabilities.streaming`, bei grauwolf-Diensten immer), holt
  grauwolf die Antwort Stück für Stück. Lange Aufträge brechen dann nicht nach zehn Minuten am
  Lese-Timeout ab: Er gilt nur zwischen zwei Lebenszeichen des Agenten.

Zum Ausprobieren startest du den Wissens-Agenten aus [Kapitel 7](07-gespraeche-und-wissen.md) als
Dienst und rufst ihn in einem zweiten Terminal auf:

```bash
GRAUWOLF_SERVER_TOKEN=geheim grauwolf spec serve docs/handbuch/beispiele/07-wissen.json
CARETAKER_TOKEN=geheim grauwolf spec run docs/handbuch/beispiele/08-a2a.json \
  "Darf mein Fahrrad im Treppenhaus stehen?"
```

Die Antwort ist etwa: `Nein, Fahrräder gehören in den Fahrradkeller – das Treppenhaus ist Fluchtweg.`

### Die Verbindung mit SPIFFE absichern

Für einen Agenten im eigenen Cluster ersetzt `auth.spiffe` das Bearer-Token. Die URL ist der mTLS-Port des Ziels über `https`; bei einem Ziel hinter einem Ingress, der TLS beendet, nimmst du `jwt`:

```json
"agent_tools": {
  "reviewer": {
    "url": "https://reviewer.reviewer.svc:8443",
    "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
  },
  "archivist": {
    "url": "https://archivist.example.com",
    "auth": { "spiffe": { "mode": "jwt", "audience": "archivist" } }
  }
}
```

Der aufgerufene Agent braucht `GRAUWOLF_SPIFFE=1` und eine Aufruferdatei, die deinen Agenten nennt (`GRAUWOLF_SPIFFE_USERS`); in einem Cluster erledigt `grauwolf-k8-ops agent allow reviewer --from <dein agent>` die Datei und die NetworkPolicy. Beachte, dass die Agent-Card des Ziels beim Start deines Agenten geholt wird; ein Ziel, das nicht erreichbar ist, stoppt deinen Agenten also schon beim Start.

Eine falsche `id` und ein unerreichbarer Port sehen verschieden aus:

```text
A2A over HTTP: TLS handshake with https://reviewer.reviewer.svc:8443/.well-known/agent-card.json failed: the server is spiffe://agentswork.de/ns/reviewer/sa/reviewer, expected spiffe://agentswork.de/ns/intruder/sa/intruder; the call was not executed
A2A over HTTP: no connection to https://reviewer.reviewer.svc:8443/.well-known/agent-card.json (refused or given up after 30 s); the call was not executed
```

Die erste nennt die Identität, die der Server wirklich hat: trage sie als `id` ein. Die zweite heißt, dass der Port nicht erreichbar ist (Agent nicht deployt, oder eine NetworkPolicy hält den Aufrufer draußen). Siehe [Kapitel 9](09-betrieb.md#agenten-rufen-agenten-mit-spiffe), [Kapitel 13](13-cluster-einrichten.md#agenten-sich-gegenseitig-aufrufen-lassen) und die Referenz ([`auth.spiffe`](referenz-spec.md#authspiffe-in-agent_toolsid-und-mcp_serversid)).

### Typische Fehler

```text
…:/agent_tools/caretaker: sub-agent needs exactly one of `spec` (local) and `url` (remote, A2A)
…:/agent_tools/translator/description: local sub-agent needs a `description`
…:/agent_tools/Checker: 'Checker' is not a valid ID: allowed are lowercase letters, digits and '_', starting with a letter
```

## Was nehme ich wann?

| Du willst … | Nimm |
|---|---|
| ein fertiges Werkzeug (Dateien, Git, Web, Datenbank) | einen MCP-Server, lokal per `command` |
| einen Dienst im Netz mit MCP-Schnittstelle | einen MCP-Server per `url` mit `headers` |
| denselben Dienst oder einen Agenten im eigenen Cluster, ohne gemeinsames Geheimnis | `auth.spiffe` am `url`-Eintrag |
| eine Teilaufgabe mit eigenem Prompt, Modell oder eigenen Tools | einen lokalen Unteragenten per `spec` |
| einen Agenten eines anderen Teams oder Frameworks | einen entfernten Unteragenten per `url` (A2A) |
| nur einen Teil des Ablaufs mit anderem Modell oder anderen Tools, aber im selben Verlauf | einen Subgraphen ([Kapitel 6](06-subgraphen-und-planer.md)) |

## Das Wichtigste in Kürze

- `mcp_servers` nennt je Server entweder `command` (+ `args`) oder `url` (+ `headers` mit `${VAR}`, und
  `transport`: `streamable_http` oder `sse`) sowie immer die genutzten `tools`; `resources: true` ergänzt
  `<server>_list_resources` und `<server>_read_resource`.
- MCP-Server laufen außerhalb der Sandbox mit deinen Rechten.
- `agent_tools` macht andere Agenten zu Tools: lokal mit `spec` und Pflicht-`description`, entfernt mit
  `url` (A2A) und optional `headers`.
- Unteragenten arbeiten mit eigenem, frischem Verlauf; Argument `request` bei Text-Eingabe.
- Im Cluster mit SPIRE ersetzt `auth.spiffe` an einem `url`-Eintrag das Token: `mtls` mit `id` oder `jwt` mit `audience`.
- Alle Tool-Namen teilen sich einen Namensraum.

Weiter mit [Kapitel 9: Betrieb](09-betrieb.md).
