# 7 Gespräche, Gedächtnis und Wissen

Ein Lauf mit `spec run` beginnt immer bei null: Der Agent kennt nur seinen System-Prompt und deine Eingabe. Für echte Assistenten reicht das nicht. Dieses Kapitel zeigt drei Arten, wie dein Agent mehr weiß:

| Was | Wie lange | Wozu |
|---|---|---|
| **Gespräch** (`spec chat`, `--conversation`) | ein Gespräch, auf Wunsch gespeichert | Rückfragen und Bezüge auf vorher Gesagtes |
| **Langzeitgedächtnis** (`long_term_memory`) | über alle Gespräche hinweg | Fakten über Nutzer: Name, Vorlieben, Pläne |
| **Wissen** (`knowledge`) | solange die Dokumente da sind | Fragen zu deinen eigenen Dokumenten beantworten |

Gedächtnis und Wissen arbeiten mit **Embeddings**: Ein Embedding-Modell macht aus einem Text eine Zahlenreihe, die seine Bedeutung beschreibt. Ähnliche Texte haben ähnliche Zahlen; so findet grauwolf passende Stellen auch dann, wenn andere Wörter benutzt werden. Die Beispiele nutzen `nomic-embed-text` über Ollama:

```bash
ollama pull nomic-embed-text
```

## Chatten: `spec chat`

`spec chat` startet ein Gespräch auf der Konsole. Jede Zeile, die du eingibst, ist ein eigener Lauf – aber alle Läufe teilen denselben Verlauf. Das funktioniert mit jeder Spec, ganz ohne Änderung. Für dieses Kapitel reicht ein einfacher Agent:

```json
{
  "spec_version": 1,
  "name": "conversation_partner",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du bist ein hilfsbereiter Gesprächspartner. Antworte kurz und auf Deutsch und beziehe dich auf das bisherige Gespräch.",
  "strategy": { "kind": "single_run" }
}
```

Die Datei: [`beispiele/07-gespraech.json`](beispiele/07-gespraech.json).

```bash
grauwolf spec chat docs/handbuch/beispiele/07-gespraech.json
```

Etwa so:

```text
> Ich plane eine Radtour an der Mosel.
Schöne Idee! Wie viele Tage hast du eingeplant?
> Drei. Wo soll ich übernachten?
Für drei Tage an der Mosel bieten sich Bernkastel-Kues und Cochem als Etappenorte an.
> /exit
```

- Die Eingabeaufforderung `> ` geht auf stderr, die Antworten auf stdout. Du kannst also Zeilen auch per Pipe hineinschicken und nur die Antworten weiterverarbeiten.
- Das Gespräch endet mit `/exit` oder Strg+D (Ende der Eingabe). Leere Zeilen werden übersprungen.
- Schlägt ein Lauf fehl, steht der Fehler auf stderr – der Chat geht weiter.
- Hat die Spec einen eigenen Typ als `input` ([Kapitel 5](05-strukturierte-daten.md)), ist jede Zeile JSON.
- `spec chat` versteht die meisten Optionen von `spec run` (z. B. `--events`, `--confirm-tools`), aber noch nicht `--stream` und `--otel`.

Wenn du nichts anderes angibst, lebt das Gespräch nur im Speicher und ist nach dem Beenden weg.

## Gespräche speichern und fortsetzen: `--conversation`

Mit `--conversation <id>` speichert grauwolf den Verlauf nach jedem erfolgreichen Lauf und lädt ihn beim nächsten Mal wieder. Das klappt mit `spec chat` und mit einzelnen `spec run`-Aufrufen – auch gemischt:

```bash
grauwolf spec run docs/handbuch/beispiele/07-gespraech.json --conversation mosel \
  "Ich plane eine Radtour an der Mosel, drei Tage."
grauwolf spec run docs/handbuch/beispiele/07-gespraech.json --conversation mosel \
  "Wie viele Kilometer pro Tag schlägst du vor?"
grauwolf spec chat docs/handbuch/beispiele/07-gespraech.json --conversation mosel
```

Die zweite Antwort kennt die erste Frage, etwa so: `Bei drei Tagen sind 40 bis 50 Kilometer pro Tag gut machbar.`

Die Gesprächs-ID darf nur Buchstaben, Ziffern, `-` und `_` enthalten. Der Verlauf wird ohne den System-Prompt gespeichert; grauwolf nimmt ihn bei jedem Lauf frisch aus der Spec. So kannst du den System-Prompt ändern, ohne alte Gespräche zu verlieren.

Du entscheidest, wo der Verlauf gespeichert wird:

| Option | Speicherort | Standard |
|---|---|---|
| `--conversations-dir <dir>` | eine JSON-Datei je Gespräch: `<dir>/<id>.json` | `.grauwolf/conversations` |
| `--conversations-db <url>` | eine Datenbank, z. B. `sqlite://chats.db` oder `postgres://user@host/db` | – |

grauwolf legt eine SQLite-Datei bei Bedarf selbst an. Eine Datenbank lohnt sich vor allem, wenn mehrere Prozesse dieselben Gespräche nutzen, etwa ein Dienst ([Kapitel 9](09-betrieb.md)).

### Gespräche je Benutzer: `--user`

Bei `spec run` kannst du zusätzlich einen Benutzer angeben. Das Gespräch gehört dann diesem Benutzer: Dieselbe Gesprächs-ID ist für Anna und für Ben ein eigenes Gespräch.

```bash
grauwolf spec run docs/handbuch/beispiele/07-gespraech.json \
  --user anna --conversation mosel "Worüber hatten wir gesprochen?"
```

Im Verzeichnis landet das unter `<dir>/users/<user-hex>/<id>.json`; der Benutzername wird hexadezimal geschrieben, damit jeder Name funktioniert (auch E-Mail-Adressen). Benutzer-IDs sind 1 bis 128 Byte lang; der Dienst antwortet bei anderen IDs mit `400`, und eine Benutzerdatei mit einem solchen Namen lädt nicht. `--user` braucht entweder `--conversation` oder ein Langzeitgedächtnis in der Spec (nächster Abschnitt). `spec chat` kennt `--user` nicht; dort gibt es nur das gemeinsame Gespräch.

## Lange Gespräche: die Strategie `chat`

Ein gespeichertes Gespräch wächst mit jedem Lauf. Irgendwann wird es für das Modell zu groß – die Antworten werden langsam und teuer, und schließlich passt der Verlauf nicht mehr in das Kontextfenster. Die Strategie `chat` kümmert sich darum ([`beispiele/07-chat.json`](beispiele/07-chat.json)):

```json
{
  "spec_version": 1,
  "name": "chatterbox",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du bist ein freundlicher Gesprächspartner. Antworte kurz und auf Deutsch.",
  "strategy": { "kind": "chat", "params": { "max_messages": 20, "keep": 6 } }
}
```

Sie arbeitet wie `single_run` (Tools inklusive, standardmäßig mit `parallel_tools`), prüft aber vor jeder Antwort die Länge des Verlaufs. Hat er mehr als `max_messages` Nachrichten, verdichtet das Modell alle außer den letzten `keep` zu einer Liste von Fakten. Die Überschrift dieser Liste ist eine eingebaute Anweisung an das Modell: `Facts from the conversation so far:`.

```text
Facts from the conversation so far:
- Der Name des Nutzers ist Anna, und sie wohnt in Leipzig.
- Anna plant im Mai eine Radtour entlang der Elbe.
- Offen: Anna sucht noch eine Unterkunft in Dresden.
```

Die letzten `keep` Nachrichten bleiben wörtlich erhalten, der System-Prompt ohnehin. Wird der Verlauf später erneut verdichtet, wird die alte Liste in die neue eingearbeitet – es gibt also nie mehr als eine.

| Parameter | Standard | Bedeutung |
|---|---|---|
| `max_messages` | 30 | Bei mehr Nachrichten als diesem Wert (der System-Prompt zählt mit, die neue Eingabe noch nicht) wird der Verlauf vor der Antwort verdichtet |
| `keep` | 10 | So viele der letzten Nachrichten bleiben wörtlich erhalten (muss kleiner als `max_messages` sein) |
| `parallel_tools` | `true` | Mehrere Tool-Aufrufe pro Antwort |

Probier es aus:

```bash
grauwolf spec chat docs/handbuch/beispiele/07-chat.json --conversation anna
```

Gut zu wissen:

- Jede Verdichtung kostet eine zusätzliche Anfrage. Je größer der Abstand zwischen `max_messages` und `keep`, desto seltener passiert das.
- Die Verdichtung ist endgültig: Ein gespeichertes Gespräch enthält danach die Faktenliste statt der alten Nachrichten.
- Fakten im Verlauf gelten nur für dieses Gespräch. Was der Agent über Gespräche hinweg wissen soll, gehört ins Langzeitgedächtnis (nächster Abschnitt) – mit `remember_compressed` landen die dauerhaften Fakten aus einer Verdichtung automatisch dort.

## Langzeitgedächtnis: `long_term_memory`

Ein Gespräch hält den ganzen Verlauf – der wird lang und gilt nur für dieses Gespräch. Das **Langzeitgedächtnis** dagegen hält einzelne **Fakten** („Anna ist Vegetarierin.“), über alle Gespräche und Neustarts hinweg. Dafür bekommt der Agent zwei Tools:

- `remember` mit dem Argument `fact` speichert einen Fakt als vollständigen Satz,
- `recall` mit dem Argument `query` liefert die am besten passenden gespeicherten Fakten.

```json
{
  "spec_version": 1,
  "name": "assistant_with_memory",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du bist ein persönlicher Assistent mit Langzeitgedächtnis. Wenn der Nutzer dir etwas Dauerhaftes über sich erzählt (Name, Vorlieben, Pläne), speichere es mit remember als vollständigen Satz. Bevor du eine Frage zum Nutzer beantwortest, schlage sie mit recall nach. Antworte kurz.",
  "long_term_memory": {
    "store": "../../../.grauwolf/memory/assistant.json",
    "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
    "top_k": 3
  },
  "strategy": { "kind": "single_run" }
}
```

Die Datei: [`beispiele/07-gedaechtnis.json`](beispiele/07-gedaechtnis.json). Sag im System-Prompt ruhig klar, wann der Agent die Tools nutzen soll – das hilft besonders kleinen Modellen.

| Feld | Bedeutung | Standard |
|---|---|---|
| `store` | JSON-Datei relativ zur Spec, in der die Fakten liegen (genau eines von `store` und `vector_store`, siehe unten) | – |
| `embedding_model` | Embedding-Modell `{ provider, id }` (Pflicht); Anbieter `ollama`, `openai`, `gemini` oder ein OpenAI-kompatibler mit Embeddings, z. B. `mistral` mit `mistral-embed` | – |
| `top_k` | So viele Fakten werden pro Abruf zurückgegeben | `3` |
| `remember_tool` | Name des Tools zum Merken | `remember` |
| `recall_tool` | Name des Tools zum Abrufen | `recall` |
| `auto_recall` | passende Fakten vor jedem Lauf automatisch hinzufügen | `false` |
| `auto_remember` | neue Fakten nach jedem Lauf automatisch speichern | `false` |
| `tools` | die zwei Tools dem Agenten anbieten | `true` |

Hier zeigt `store` drei Ebenen nach oben in den Ordner `.grauwolf` im Projektverzeichnis; fehlende Verzeichnisse legt grauwolf an. Identische Fakten werden in der Datei nur einmal gespeichert. Die Datei merkt sich das Embedding-Modell – wenn du es später änderst, brauchst du eine neue Datei.

```bash
grauwolf spec run docs/handbuch/beispiele/07-gedaechtnis.json "Ich heiße Anna und esse kein Fleisch."
grauwolf spec run docs/handbuch/beispiele/07-gedaechtnis.json "Was soll ich heute kochen?"
```

Die zweite Antwort lautet etwa: `Wie wäre es mit einem Gemüsecurry, Anna? Komplett fleischfrei.`

**Getrennt nach Benutzer:** Mit `spec run --user <id>` sieht und füllt der Agent nur den Bereich dieses Benutzers in derselben Datei. Ohne `--user` (und bei `spec chat`) nutzt er den gemeinsamen Bereich.

```bash
grauwolf spec run docs/handbuch/beispiele/07-gedaechtnis.json --user anna "Was esse ich nicht?"
```

### Automatisch merken und abrufen

Kleine Modelle vergessen oft, die Tools aufzurufen. Mit `auto_recall` und `auto_remember` erledigt grauwolf das selbst:

- **`auto_recall`**: Vor jedem Lauf werden die `top_k` am besten passenden Fakten zur Eingabe gesucht und als Systemnachricht `Known facts from long-term memory: …` (eingebaute Anweisung) hinzugefügt. Gibt es keine Treffer, wird keine Nachricht hinzugefügt.
- **`auto_remember`**: Nach jedem erfolgreichen Lauf fragt grauwolf das Modell in einer eigenen Anfrage nach dauerhaften, neuen Fakten aus Eingabe und Antwort und speichert sie. Das kostet pro Lauf eine zusätzliche Anfrage.
- **`tools: false`** blendet `remember` und `recall` aus; das Gedächtnis arbeitet dann nur automatisch. So entstehen keine doppelten Einträge in leicht anderer Formulierung.

```json
"long_term_memory": {
  "store": "../../../.grauwolf/memory/assistant-auto.json",
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "auto_recall": true,
  "auto_remember": true,
  "tools": false
}
```

Die vollständige Datei: [`beispiele/07-gedaechtnis-auto.json`](beispiele/07-gedaechtnis-auto.json).

### Fakten aus langen Gesprächen behalten: `remember_compressed`

Wenn die Strategie `chat` (oder `compress_history` mit `facts`) ein langes Gespräch verdichtet, entsteht eine Faktenliste – die gilt aber nur für dieses Gespräch. Mit `"remember_compressed": true` fragt grauwolf in derselben Verdichtungsanfrage zusätzlich, welche dieser Fakten für den Nutzer **dauerhaft** gelten (Name, Vorlieben, Pläne, Beziehungen – keine offenen Fragen, keine Ereignisse des Tages), und speichert genau diese im Langzeitgedächtnis. Das kostet keine zusätzliche Anfrage ([`beispiele/07-chat-gedaechtnis.json`](beispiele/07-chat-gedaechtnis.json)):

```json
"long_term_memory": {
  "store": "../../../.grauwolf/memory/companion.json",
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "auto_recall": true,
  "remember_compressed": true,
  "tools": false
},
"strategy": { "kind": "chat", "params": { "max_messages": 20, "keep": 6 } }
```

Zusammen mit `auto_recall` weiß der Agent so in einem neuen Gespräch, was er in einem früheren gelernt hat. Fakten landen im Bereich des Benutzers (`--user` bzw. der Benutzer im Dienst), sonst im gemeinsamen Bereich; `--events` zeigt das als `memory_remembered`. Fehler beim automatischen Merken oder Abrufen brechen den Lauf nicht ab; mit `--events` siehst du sie als `memory_failed`, erfolgreiche Schritte als `memory_recalled` und `memory_remembered`.

Typische Meldungen von `spec validate`:

```text
…:/long_term_memory/tools: memory without tools needs `auto_recall`, `auto_remember` or `remember_compressed`
…:/long_term_memory/recall_tool: tool name 'recall' is used more than once (the other one is the … at …)
```

## Eigene Dokumente durchsuchen: `knowledge`

Mit `knowledge` beantwortet dein Agent Fragen zu deinen eigenen Dokumenten. Diese Technik heißt **RAG** (Retrieval-Augmented Generation, also „Generieren mit Nachschlagen“): Beim Start teilt grauwolf die Dokumente in Abschnitte, bettet sie ein (Embedding) und bietet dem Agenten ein **Suchtool** an. Das Modell entscheidet selbst, wann es etwas nachschlägt, und bekommt die am besten passenden Abschnitte samt Quelle zurück.

```json
{
  "spec_version": 1,
  "name": "caretaker",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du beantwortest Fragen zur Hausordnung. Schlage vor jeder Antwort mit search_house_rules nach und nenne die Quelle. Steht etwas nicht in den Dokumenten, sag das.",
  "knowledge": {
    "sources": ["07-wissen"],
    "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
    "tool": "search_house_rules",
    "chunk_size": 500,
    "top_k": 2,
    "index": "../../../.grauwolf/knowledge/house-rules.json"
  },
  "strategy": { "kind": "single_run" }
}
```

Die Datei: [`beispiele/07-wissen.json`](beispiele/07-wissen.json); die Dokumente liegen in [`beispiele/07-wissen/`](beispiele/07-wissen/).

| Feld | Bedeutung | Standard |
|---|---|---|
| `sources` | Dateien oder Verzeichnisse relativ zur Spec; Verzeichnisse werden mit allen Unterordnern gelesen (mindestens eines von `sources` und `urls`) | – |
| `urls` | Webseiten, die bei jedem Start abgerufen werden, siehe [unten](#webseiten-als-quellen-urls) | – |
| `min_similarity` | Mindestähnlichkeit (0 bis 1) eines Treffers; schwächere Abschnitte fallen weg | – |
| `inject` | die besten Abschnitte automatisch zu jeder Frage hinzufügen, siehe [unten](#wissen-als-kontext-inject) | `false` |
| `tool_enabled` | das Suchtool dem Modell anbieten; mit `false` brauchst du `inject: true` | `true` |
| `embedding_model` | Embedding-Modell `{ provider, id }` (Pflicht); Anbieter `ollama`, `openai`, `gemini` oder ein OpenAI-kompatibler mit Embeddings, z. B. `mistral` mit `mistral-embed` | – |
| `tool` | Name des Suchtools | `search_knowledge` |
| `top_k` | So viele Abschnitte werden pro Suche zurückgegeben | `3` |
| `chunk_size` | maximale Länge eines Abschnitts in Zeichen | `1000` |
| `index` | Datei relativ zur Spec, in der Abschnitte und Embeddings zwischen den Läufen liegen | – |

```bash
grauwolf spec run docs/handbuch/beispiele/07-wissen.json "Wann darf ich Sperrmüll rausstellen?"
```

Etwa so:

```text
Sperrmüll bei der Hausverwaltung melden und frühestens am Abend vor der Abholung auf die Straße stellen (Quelle: 07-wissen/hausordnung.md).
```

Gut zu wissen:

- Alle Dateien unter einer Quelle werden als UTF-8-Text gelesen. Lege dort also nur Textdateien ab (Markdown, Klartext); eine Datei, die nicht UTF-8 ist, bricht den Start mit einem Fehler und ihrem Pfad ab.
- Ohne `index` teilt und bettet grauwolf **alle** Dokumente bei **jedem Start** neu ein. Mit `index` merkt es sich eine Prüfsumme je Datei: Beim nächsten Start werden nur neue oder geänderte Dateien neu eingebettet, gelöschte fallen heraus. Wenn du das Embedding-Modell oder `chunk_size` änderst, wird alles neu eingebettet.
- Ein kleineres `chunk_size` liefert genauere, aber kürzere Treffer; mit `top_k` steuerst du, wie viel Text pro Suche ins Gespräch kommt.
- Das Suchtool ist ein Tool wie jedes andere: Sein Name darf zu keinem anderen Tool passen, und du kannst es in der `tools`-Liste eines Subgraphen nennen ([Kapitel 6](06-subgraphen-und-planer.md)).

### Webseiten als Quellen: `urls`

`urls` listet Webseiten (`https://`; einfaches `http://` nur für `localhost` und `127.0.0.1`), die wie Dateien indexiert werden. Bei jedem Start holt grauwolf jede URL mit einer **bedingten Anfrage** (`ETag` und `Last-Modified` aus dem letzten Abruf, neben dem Index gespeichert): Ein `304 Not Modified` bedeutet, dass nichts neu eingebettet wird; eine geänderte Seite wird neu aufgeteilt – mit der Datei `index` wird die ganze Seite neu eingebettet, mit dem PostgreSQL-Speicher nur ihre geänderten Abschnitte. Eine URL, die du aus der Spec entfernst, fällt aus dem Index. Der Quellenname in Suchergebnissen ist die URL.

```json
"knowledge": {
  "sources": ["07-wissen"],
  "urls": ["https://docs.example.com/guide/getting-started.html", "https://docs.example.com/faq.md"],
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "min_similarity": 0.5,
  "inject": true,
  "tool_enabled": false,
  "index": "../../../.grauwolf/knowledge/docs-helper.json"
}
```

Die Datei: [`beispiele/07-wissen-urls.json`](beispiele/07-wissen-urls.json).

- **Formate:** `text/plain`, `text/markdown`, `text/html` und `application/xhtml+xml` (anhand von `Content-Type`, sonst anhand der Dateiendung). HTML wird zu lesbarem Text: Skripte, Stile, Navigation, Kopf- und Fußzeile, Seitenleisten und Formulare fallen weg; Überschriften, Absätze, Listenpunkte und Tabellenzellen bleiben. PDF und andere Binärtypen werden mit einer Meldung abgelehnt. Der Zeichensatz kommt aus dem Header (standardmäßig UTF-8, Latin-1 funktioniert auch).
- **Regeln:** höchstens 5 MiB pro Seite, 30 Sekunden, keine Cookies oder Zugangsdaten. Weiterleitungen werden nur innerhalb desselben Ursprungs gefolgt; eine Weiterleitung auf einen anderen Host ist ein Fehler – trage die endgültige URL in `urls` ein.
- **Fehler:** Lässt sich eine URL nicht abrufen und enthält der Index schon ihren Inhalt, wird dieser Inhalt verwendet und eine Warnung geht auf stderr. Ohne indexierten Inhalt scheitert der Start und nennt URL und Grund. Beim Datei-Speicher bleibt der Inhalt nur zwischen den Läufen erhalten, wenn du `index` setzt.
- Prüfung: keine Zugangsdaten in der URL, kein `#fragment`, keine Duplikate.

### Wissen als Kontext: `inject`

Mit `"inject": true` sucht grauwolf die Frage selbst heraus: Vor jeder Anfrage, die von einer neuen Benutzereingabe ausgeht, werden die besten `top_k` Abschnitte (unter Beachtung von `min_similarity`) als Systemnachricht mit ihren Quellen dem Gespräch hinzugefügt. Das Modell muss kein Tool aufrufen, deshalb funktioniert das auch mit kleinen Modellen, die selten Tools nutzen. Das Ereignis `knowledge_injected` (`sources`, `count`, kein Text) meldet es, `knowledge_failed` eine fehlgeschlagene Suche (der Lauf geht weiter).

Standardmäßig bleibt das Suchtool außerdem verfügbar. Mit `"tool_enabled": false` wird nur die Einspeisung genutzt; beides auszuschalten ist ein Validierungsfehler (`knowledge without the tool needs inject: true`).

### Felder anderer Tools, anders benannt

Wenn du die RAG-Einstellungen von AgentWeaver kennst: `urlSources` ist `urls`, `similarityThreshold` ist `min_similarity`, `maxDocuments` ist `top_k`, `injectAsContext` ist `inject` und `provideAsTool` ist `tool_enabled`.

Typische Meldungen von `spec validate`:

```text
…:/knowledge/sources: knowledge needs at least one source
…:/knowledge/tool: tool name 'recall' is used more than once (the other one is the … at …)
```

## Wissen und Gedächtnis in PostgreSQL mit pgvector

Standardmäßig liegt Wissen im Speicher (mit einer `index`-Datei als Cache) und das Langzeitgedächtnis in einer JSON-Datei – ideal für einen Container und ein paar tausend Einträge. Für **große Dokumentenbestände**, für **mehrere Replicas, die einen Index teilen**, und für einen **zustandslosen Container** können beide stattdessen in PostgreSQL mit der Erweiterung [pgvector](https://github.com/pgvector/pgvector) liegen.

```bash
docker run -d -p 5432:5432 -e POSTGRES_PASSWORD=postgres pgvector/pgvector:pg17
export PGVECTOR_URL=postgres://postgres:postgres@localhost:5432/postgres
```

Die URL ist **nie** Teil der Spec: Die Spec nennt die Umgebungsvariable (`url_env`), und der Wert kommt aus der Umgebung (ein Kubernetes-Secret, `docker run -e`, …). Fehler wie eine fehlende Variable, eine abgelehnte Verbindung oder eine fehlende Erweiterung nennen die Variable und geben nie die URL oder das Passwort aus. grauwolf legt beim Start an, was es braucht (`CREATE EXTENSION IF NOT EXISTS vector`, seine Tabellen und HNSW-Indizes mit Cosinus-Distanz); darf der Datenbankbenutzer keine Erweiterungen anlegen, führt ein Administrator einmal `CREATE EXTENSION vector` aus.

### Wissen: `knowledge.store`

```json
"knowledge": {
  "sources": ["07-wissen"],
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" },
  "store": { "kind": "postgres", "url_env": "PGVECTOR_URL" }
}
```

Die Datei: [`beispiele/07-wissen-postgres.json`](beispiele/07-wissen-postgres.json). `kind` ist `file` (der Standard, mit `index`) oder `postgres`; `index` gilt nur für `file` und ist zusammen mit `postgres` ein Fehler.

- Die Abschnitte werden in der Tabelle `grauwolf_chunks` gespeichert, in einer Sammlung namens `<spec name>/<tool name>` – mehrere Agenten können sich eine Datenbank teilen.
- Die Indizierung ist inkrementell wie bei `index`: ein Hash je Abschnitt; beim Start werden nur neue oder geänderte Abschnitte eingebettet, Abschnitte gelöschter Dateien entfernt. Ändert sich das Embedding-Modell oder seine Dimension, wird die Sammlung neu aufgebaut.
- Mehrere Replicas, die gleichzeitig starten, indizieren nacheinander (ein Advisory Lock je Sammlung), sodass jeder Abschnitt nur einmal eingebettet wird.

### Gedächtnis: `long_term_memory.vector_store`

```json
"long_term_memory": {
  "vector_store": { "kind": "postgres", "url_env": "PGVECTOR_URL", "ttl_days": 90, "min_similarity": 0.4 },
  "embedding_model": { "provider": "ollama", "id": "nomic-embed-text" }
}
```

Die Datei: [`beispiele/07-gedaechtnis-postgres.json`](beispiele/07-gedaechtnis-postgres.json). Gib **genau eines** von `store` (JSON-Datei) und `vector_store` an.

| Feld | Bedeutung | Standard |
|---|---|---|
| `kind` | `postgres` | Pflicht |
| `url_env` | Name der Umgebungsvariable, die die URL enthält | Pflicht |
| `ttl_days` | Fakten verfallen so viele Tage nach dem Speichern (mindestens 1); erneutes Speichern desselben Fakts verlängert ihn | nie |
| `min_similarity` | Abruf liefert nur Fakten, die mindestens so ähnlich zur Anfrage sind (0 bis 1, Cosinus) | keine |

Fakten werden je Agent (der Spec-Name) und je Benutzer genau wie in der Datei gehalten: `--user` und die `user_id` des Dienstes sehen nur ihren eigenen Bereich, alles andere den gemeinsamen. Identische Fakten werden je Bereich einmal gespeichert. Der Abruf liefert höchstens `top_k` Fakten, die `min_similarity` erreichen, die besten zuerst; abgelaufene Fakten werden nie zurückgegeben und gelöscht, wenn derselbe Bereich seinen nächsten Fakt speichert (es gibt keinen Hintergrundjob). Die Tabelle `grauwolf_memories` zählt außerdem, wie oft ein Fakt abgerufen wurde (`access_count`). `min_similarity` gilt nur für den PostgreSQL-Speicher. Wie die Datei merkt sich die Datenbank das Embedding-Modell je Agent – ein anderes Modell wird abgelehnt.

Gut zu wissen: pgvector indiziert Vektoren mit bis zu 2000 Werten; eine Tabelle hat genau eine Dimension, deshalb brauchen Embedding-Modelle mit unterschiedlichen Dimensionen getrennte Datenbanken (oder Schemata). Erzeugte Crates (`spec generate`) nehmen dieselben Variablen; das Facade-Feature `pgvector` wird für sie automatisch aktiviert.

## Was nehme ich wann?

- **Rückfragen innerhalb einer Sitzung** → `spec chat`, zum Fortsetzen später mit `--conversation`.
- **Gespräche, die lange laufen** → die Strategie `chat`; ältere Teile bleiben als Fakten erhalten.
- **Der Agent soll Menschen über Wochen kennen** → `long_term_memory`, bei kleinen Modellen mit `auto_recall`/`auto_remember`, bei mehreren Nutzern mit `--user`.
- **Fragen zu Handbüchern, Regeln, Notizen** → `knowledge` mit `index`.
- **Große Bestände, mehrere Replicas, Fakten, die verfallen** → die PostgreSQL-Speicher oben.

Alles lässt sich kombinieren: Ein Agent kann in einem gespeicherten Gespräch chatten, Fakten merken und gleichzeitig in Dokumenten nachschlagen.

## Das Wichtigste in Kürze

- `spec chat <spec>` führt jede Zeile als Lauf im selben Gespräch aus; Ende mit `/exit` oder EOF.
- `--conversation <id>` speichert Gespräche (Standard `.grauwolf/conversations`, alternativ `--conversations-db`); bei `spec run` bindet `--user` sie an einen Benutzer.
- Die Strategie `chat` verdichtet lange Gespräche vor der Antwort zu Fakten (`max_messages`, `keep`).
- `long_term_memory` gibt dem Agenten `remember`/`recall`; `auto_recall`, `auto_remember` und `tools: false` machen es automatisch.
- `knowledge` macht Dokumente und Webseiten (`urls`) über ein Suchtool oder als automatischen Kontext (`inject`) durchsuchbar; `index` spart erneutes Einbetten.

Weiter mit [Kapitel 8: Externe Tools und andere Agenten](08-externe-tools-und-agenten.md).
