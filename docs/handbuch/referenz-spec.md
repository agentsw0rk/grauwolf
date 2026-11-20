# Spec-Referenz

Alle Felder einer Spec (Version 1) mit Typ, Standardwert und Verweis auf das erklärende Kapitel,
dazu alle Node-Arten, Bedingungen und Validierungsfehler. Das vollständige JSON-Schema gibt
`grauwolf spec schema` aus.

Allgemeine Regeln:

- Unbekannte Felder sind überall ein Fehler (`…:/<pointer>: unknown field (allowed: …)`).
- Pfade in der Spec (`root`, `sources`, `store`, `index`, `tools_dir`, `spec`, `code_file`,
  `tool_files`) gelten relativ zur Spec-Datei.
- **IDs** (Name der Spec, Namen von Tools, Nodes, Subgraphen und MCP-Servern) sind snake_case:
  `[a-z][a-z0-9_]*`. **Typnamen** sind PascalCase: `[A-Z][A-Za-z0-9]*`.
- Alle Tools teilen sich **einen Namensraum**: eigene Tools, MCP-Tools, das Such-Tool, die Gedächtnis-Tools,
  Unteragenten, eingebaute Tools und das Bau-Tool von `self_tools`.

## Oberste Ebene

| Feld | Typ | Standard | Bedeutung | Kapitel |
|---|---|---|---|---|
| `spec_version` | Zahl | **Pflicht** | Version des Formats; nur `1` wird unterstützt | [1](01-erste-schritte.md) |
| `name` | ID | **Pflicht** | Name des Agenten (bei `generate` auch der Crate-Name, bei MCP der Tool-Name) | [1](01-erste-schritte.md) |
| `model` | [Modell](#modell) | **Pflicht** | LLM des Agenten | [2](02-modelle.md) |
| `system_prompt` | Text | – | System-Prompt; dient auch als Beschreibung für A2A und MCP | [1](01-erste-schritte.md) |
| `types` | Objekt `name → JSON-Schema` | `{}` | Benannte Typen für Ein- und Ausgaben | [5](05-strukturierte-daten.md) |
| `input` | Text | `"string"` | Eingabetyp: `string` oder ein Name aus `types` | [5](05-strukturierte-daten.md) |
| `output` | Text | `"string"` | Ausgabetyp: `string` oder ein Name aus `types` | [5](05-strukturierte-daten.md) |
| `tools` | Liste von [Tools](#tools) | `[]` | Eigene Tools mit Code, laufen in der Sandbox | [3](03-tools.md) |
| `tool_files` | Liste von Pfaden | `[]` | Tool-Definitionen aus eigenen JSON-Dateien; werden an `tools` angehängt | [3](03-tools.md) |
| `tools_dir` | Pfad | – | Ordner, dessen Tool-Dateien vor jedem Lauf geladen werden | [3](03-tools.md) |
| `tool_packages` | Liste von [Tool-Paketen](#tool_packages) | `[]` | Fertig gebaute Tools aus OCI-Registries | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `skills` | Liste von [Skill-Quellen](#skills) | `[]` | Anleitungsordner, die der Agent bei Bedarf lädt (`load_skill`, `read_skill_file`) | [3](03-tools.md#skills-anleitungen-die-der-agent-bei-bedarf-lädt-skills) |
| `self_tools` | [Selbst gebaute Tools](#self_tools) | – | Der Agent darf während des Laufs Tools bauen | [10](10-selbst-gebaute-tools.md) |
| `limits` | [Grenzen](#limits) | – | Grenzen je Lauf: Schritte, Tool-Aufrufe, Tokens, Kosten | [2](02-modelle.md) |
| `builtin_tools` | [Eingebaute Tools](#builtin_tools) | – | Dateitools und Ein- und Ausgabe mit dem Benutzer | [3](03-tools.md) |
| `mcp_servers` | Objekt `ID → MCP-Server` | `{}` | MCP-Server, deren Tools der Agent benutzt | [8](08-externe-tools-und-agenten.md) |
| `agent_tools` | Objekt `ID → Unteragent` | `{}` | Andere Agenten als Tools | [8](08-externe-tools-und-agenten.md) |
| `knowledge` | [Wissen](#knowledge) | – | Dokumente, über ein Such-Tool durchsuchbar (RAG) | [7](07-gespraeche-und-wissen.md) |
| `long_term_memory` | [Gedächtnis](#long_term_memory) | – | Langzeitgedächtnis | [7](07-gespraeche-und-wissen.md) |
| `subgraphs` | Objekt `ID → Subgraph` | `{}` | Benannte Teilabläufe | [6](06-subgraphen-und-planer.md) |
| `strategy` | [Vorlage](#strategy) | – | Fertiger Ablauf statt `graph`; schließt `graph` aus | [4](04-ablaeufe.md) |
| `graph` | [Graph](#graph) | leer | Ablauf aus Nodes und Kanten | [4](04-ablaeufe.md) |

## Verschachtelte Felder

### Modell

Gilt für `model`, `subgraphs.*.model`, `knowledge.embedding_model` und
`long_term_memory.embedding_model`.

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `provider` | Text | **Pflicht** | Anbieter: `ollama`, `openai`, `anthropic`, `gemini`, `bedrock` ([Kapitel 2](02-modelle.md#amazon-bedrock)) oder ein OpenAI-kompatibler (`openrouter`, `deepseek`, `mistral`, `groq`, `dashscope`, `xai`; [Kapitel 2](02-modelle.md#openai-und-kompatible-dienste)) |
| `id` | Text | **Pflicht** | Modell-ID beim Anbieter, z. B. `qwen3.5:9b` oder `claude-haiku-5-5` |
| `fallbacks` | Liste von `{ provider, id }` | leer | Ausweichmodelle, nacheinander gefragt, wenn eine Anfrage scheitert; nicht bei Embedding-Modellen ([Kapitel 2](02-modelle.md#ausweichmodelle-fallbacks), ADR-0088) |

| `temperature` | Zahl 0–2 | Standard des Anbieters | Sampling-Temperatur ([Kapitel 2](02-modelle.md#temperatur-und-denken-temperature-think), ADR-0097); nicht bei Embedding-Modellen |
| `think` | bool | nicht gesetzt | Denken der Ollama-Modelle; hat Vorrang vor `OLLAMA_THINK`; nur für `ollama`; nicht bei Embedding-Modellen |

Die Einträge in `fallbacks` nehmen auch `temperature` und `think` an.

### `tools`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `name` | ID | **Pflicht** | Name, unter dem das LLM das Tool aufruft |
| `description` | Text | **Pflicht** | Beschreibung für das LLM |
| `args` | JSON-Schema | **Pflicht** | Schema der Argumente |
| `code` | Text | Pflicht (oder `code_file`) | Rumpf von `fn tool(args: serde_json::Value) -> Result<String, String>` |
| `code_file` | Pfad | – | Code aus einer Datei statt `code`; genau eines von beiden |

Eine Datei aus `tool_files` oder `tools_dir` enthält dieselben Felder; ihr `code_file` gilt relativ
zur Tool-Datei.

### `tool_packages`

Jeder Eintrag nennt genau eine Quelle.

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `oci` | Text | – | Referenz `registry/repository[:tag][@sha256:<digest>]`, z. B. `ghcr.io/acme/tools/math:1.0`; der Registry-Host wird immer ausgeschrieben; ohne Tag und Digest ist der Tag `latest` |
| `tools` | Liste von Namen | alle | Die Tools des Pakets, die benutzt werden |
| `as` | Objekt `name → name` | `{}` | Benennt übernommene Tools um: Name im Paket → Name in der Spec |
| `git` | Text | – | Git-Quelle statt `oci`: Repository-URL (`https://`; `http://` nur für `localhost` und `127.0.0.1`; keine Benutzerangaben, das Token ist `GIT_TOKEN`) |
| `path` | Text | Wurzel des Repositorys | Nur Git: Ordner des Pakets im Repository (relativ, ohne `..`) |
| `rev`, `tag`, `branch` | Text | – | Nur Git, genau eines: ein Commit (7 bis 40 Hex-Zeichen in Kleinbuchstaben), oder ein Tag oder Branch, der zu einem Commit aufgelöst und gesperrt wird |

Die Tools treten dem Namensraum aller anderen Tools bei. Namen, die sich ohne Abruf des Pakets
kennen lassen (die in `tools` aufgelisteten, nach dem Umbenennen), prüft `spec validate`; dafür
braucht es nie das Netz. Die Tools eines Eintrags ohne `tools`, unbekannte Tool-Namen und die
ABI-Version prüft erst das Auflösen des Pakets (`spec run`, `spec build`, `tools lock`). Beim Auflösen
wird `grauwolf.lock` neben der Spec geschrieben.

### `skills`

Jeder Eintrag nennt genau eine Quelle; ihr Ordner enthält je Skill ein Unterverzeichnis mit einer `SKILL.md`
(Front Matter `name` gleich dem Ordnernamen, `description`; danach die Anleitung).

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `dir` | Text | – | Lokales Verzeichnis relativ zur Spec (ohne `..`, nicht absolut) |
| `git` | Text | – | Git-Quelle statt `dir`: Repository-URL (`https://`; `http://` nur für `localhost` und `127.0.0.1`; keine Benutzerangaben, das Token ist `GIT_TOKEN`) |
| `path` | Text | Wurzel des Repositorys | Nur Git: Ordner mit den Skills im Repository (relativ, ohne `..`) |
| `rev`, `tag`, `branch` | Text | – | Nur Git, genau eines: ein Commit (7 bis 40 Hex-Zeichen in Kleinbuchstaben), oder ein Tag oder Branch, der zu einem Commit aufgelöst und in `grauwolf.lock` (Abschnitt `skills`) gesperrt wird |
| `names` | Liste von Namen | alle | Die Skills der Quelle, die benutzt werden (Kleinbuchstaben, Ziffern, `-` und `_`) |

Mit `skills` bekommt der Agent die Tools `load_skill` und `read_skill_file`; ihre Namen sind im
gemeinsamen Namensraum der Tools reserviert. Skill-Namen müssen über alle Quellen hinweg eindeutig sein,
jede Datei höchstens 1 MiB groß, und kein symbolischer Link darf aus seinem Skill-Ordner hinausführen.
`spec validate` prüft lokale Verzeichnisse; Git-Quellen werden beim Abruf geprüft.

### `self_tools`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `enabled` | bool | `true` | Bauen außerhalb von Subgraphen erlaubt |
| `tool` | ID | `"create_tool"` | Name des Bau-Tools |
| `max_tools` | Zahl | `5` | Höchstens so viele neue Tools je Lauf |
| `max_code_bytes` | Zahl | `20000` | Höchstens so viele Bytes Code je Tool |
| `confirm` | bool | `true` | Ein Mensch bestätigt jeden Bau |
| `save` | bool | `false` | Gebaute Tools im `tools_dir` speichern; braucht `tools_dir` |

### `limits`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `max_steps` | ganze Zahl ≥ 1 | 50 | Node-Ausführungen je Durchlauf des Graphen (Subgraphen zählen getrennt) |
| `max_tool_calls` | ganze Zahl ≥ 0 | unbegrenzt | Tool-Aufrufe je Lauf einschließlich Forks; darüber hinaus ein Tool-Ergebnis statt einer Ausführung |
| `max_prompt_tokens` | ganze Zahl ≥ 1 | unbegrenzt | Tokens je Anfrage, vorab geschätzt; größere Anfragen werden nicht gesendet (ADR-0087) |
| `max_tokens` | ganze Zahl ≥ 1 | unbegrenzt | Eingabe- und Ausgabe-Tokens je Lauf (wie `--max-tokens`) |
| `max_cost` | Zahl > 0 | unbegrenzt | Kosten je Lauf (wie `--max-cost`); ohne Preistabelle ein Fehler beim Laden (erzeugte Agenten: `GRAUWOLF_PRICES`) |

CLI-Optionen haben je Feld Vorrang. Unmögliche Werte → `InvalidLimit` bei `/limits/<feld>`.

### `builtin_tools`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `tools` | Liste | **Pflicht** | Dateitools `read_file`, `list_directory`, `write_file`, `edit_file`, `search_files`; Benutzer-Tools `say_to_user`, `ask_user`; `think`; `fetch`; Schlüssel-Wert-Tools `store_value`, `retrieve_value`, `list_keys`; Aufgaben-Tools `add_task`, `complete_task`, `remove_task`, `list_tasks`; `run_command` (braucht `command` und `root`) |
| `root` | Pfad | – | Verzeichnis, in dem die Dateitools und das Git-Verzeichnis arbeiten; Pflicht für Dateitools und die Schlüssel-Wert-Tools (`kv.json`) |
| `fetch` | `{ allowed_hosts, allow_http?, auth? }` | – | Hosts des Tools `fetch`: genauer Host oder IP oder `*.domain`; `allow_http` (Standard `false`) erlaubt zusätzlich `http://`; private, Loopback- und Link-local-Adressen werden abgelehnt, außer der Host steht genau in der Liste (ADR-0098); `auth` bildet Hosts auf eine Workload-Identität ab (`{ "<erlaubter Host>": { "spiffe": … } }`, die Form von `auth.spiffe` unten): Solche Hosts werden nur über `https://` aufgerufen, mit mTLS oder einem JWT-SVID, andere Hosts ohne (ADR-0110; `AuthHostNotAllowed`) |
| `command` | `{ allowed, shell?, i_understand_shell?, approve_each_call?, timeout_secs?, max_output_bytes?, env?, workload_api? }` | – | Das Tool `run_command` (ADR-0099): `allowed` sind erlaubte Programme (bloße Namen oder absolute Pfade; im Shell-Modus genau `["*"]`); `shell` (Standard `false`) führt `{ command }` mit `sh -c` aus und braucht `allowed: ["*"]` und `i_understand_shell: true`; `approve_each_call` (Standard `true`, nur im Shell-Modus) fragt vor jedem Aufruf nach; `timeout_secs` 1–3600 (Standard 60); `max_output_bytes` 1–10485760 je Strom (Standard 65536); `env` sind die Variablen, die das Programm erhält (Standard `["PATH"]`, alles andere wird gelöscht); `workload_api` (Standard `false`) fügt `SPIFFE_ENDPOINT_SOCKET` (und `SPIFFE_FETCH_TIMEOUT`) für die erlaubten Programme hinzu, die dann als der Agent handeln; im Shell-Modus ein Fehler (`WorkloadApiWithShell`, ADR-0110) |
| `jira` | `{ base_url }` | – | Atlassian-Cloud-Site der Tools `jira_*`; `base_url` ist `https://` (ein Pfad wie `/wiki` wird ignoriert) |
| `confluence` | `{ base_url }` | – | Atlassian-Cloud-Site der Tools `confluence_*` |
| `git` | `{ repo_url, directory?, push_policy?, use_token?, auth? }` | – | Das eine Repository der Tools `git_*`: `repo_url` `https://` ohne Benutzerangaben, `directory` relativ zu `root` (oder zum Verzeichnis der Spec), Standard `repo`; `push_policy` `new_branches` (Standard) oder `any`; `use_token` (Standard `false`) meldet mit `GIT_TOKEN` an; `auth` (`{ "<Host von repo_url oder ein Muster>": { "spiffe": … } }`) nimmt für Clone, Fetch und Push mTLS oder ein JWT-SVID statt des Tokens und schließt `use_token` aus (ADR-0110; `AuthHostNotRepository`, `GitTokenWithAuth`) |

Die Tools `jira_search_issues`, `jira_get_issue`, `jira_create_issue`, `jira_add_comment`, `jira_transition_issue`, `confluence_search_pages`, `confluence_get_page`, `confluence_create_page`, `confluence_update_page`, `git_checkout`, `git_list_branches`, `git_create_branch`, `git_commit` und `git_push` (ADR-0104) brauchen je den Abschnitt ihrer Gruppe. Ein Git-Tool, das nicht aufgelistet ist, wird nicht angeboten: Stehen nur `git_checkout` und `git_list_branches` darin, ist das Repository schreibgeschützt.

### `mcp_servers.<id>`

Genau eines von `command` (lokal, stdio) und `url` (remote, Streamable HTTP oder, mit `transport: "sse"`, das ältere HTTP+SSE-Transportverfahren).

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `command` | Text | – | Programm, das den Server startet, z. B. `npx` |
| `args` | Liste von Texten | `[]` | Argumente des Programms |
| `url` | Text | – | Endpunkt eines entfernten Servers |
| `transport` | `"streamable_http"` oder `"sse"` | `"streamable_http"` | Transport des entfernten Servers; `sse` ist das HTTP+SSE-Transportverfahren des Protokolls 2024-11-05 (ADR-0112); ein Fehler ohne `url` |
| `headers` | Objekt `name → Text` | `{}` | Kopfzeilen für `url`; `${VAR}` wird aus der Umgebung ersetzt |
| `tools` | Liste von Tool-Namen | **Pflicht** | Tools des Servers, die der Agent benutzt; nur diese werden angeboten |
| `resources` | bool | `false` | Ressourcen als Tools `<server>_list_resources` und `<server>_read_resource` anbieten |
| `auth` | `{ spiffe: … }` | – | Workload-Identität gegenüber `url` statt eines Geheimnisses in `headers`; siehe `auth.spiffe` unten; ein Fehler bei einem `command`-Server (bzw. lokalem `spec`-Agenten) und zusammen mit einer `Authorization`-Kopfzeile (ADR-0109) |
| `workload_api` | bool | `false` | Gibt `SPIFFE_ENDPOINT_SOCKET` (und `SPIFFE_FETCH_TIMEOUT`, falls gesetzt) an den Kindprozess weiter; nur mit `command`, sonst `WorkloadApiWithoutStdio`. Das Kind bekommt die Identität des Agenten (ADR-0110) |

### `agent_tools.<id>`

Genau eines von `spec` (lokal) und `url` (remote, A2A). Der Schlüssel ist der Tool-Name.

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `spec` | Pfad | – | Spec-Datei des Unteragenten |
| `url` | Text | – | Agent-Karte oder Basis-URL eines A2A-Agenten |
| `headers` | Objekt `name → Text` | `{}` | Kopfzeilen für `url`; `${VAR}` aus der Umgebung |
| `description` | Text | bei `url`: aus der Agent-Karte | Beschreibung für das LLM; bei `spec` Pflicht |
| `auth` | `{ spiffe: … }` | – | Workload-Identität gegenüber `url` statt eines Geheimnisses in `headers`; siehe `auth.spiffe` unten; ein Fehler bei einem `command`-Server (bzw. lokalem `spec`-Agenten) und zusammen mit einer `Authorization`-Kopfzeile (ADR-0109) |

### `auth.spiffe` (in `agent_tools.<id>` und `mcp_servers.<id>`)

Weist diesen Agenten gegenüber einem entfernten Agenten (A2A) oder MCP-Server mit seiner SPIFFE-Identität aus (ADR-0109, Benutzerhandbuch Kapitel 9). Nur mit `url`; nicht zusammen mit einer `Authorization`-Kopfzeile.

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `mode` | `"mtls"` oder `"jwt"` | `"mtls"` | `mtls`: der Client legt sein X.509-SVID vor und prüft die SPIFFE-ID des Servers gegen das Bundle; `jwt`: ein frisches JWT-SVID für `audience` als `Authorization: Bearer`, für Ziele hinter einem TLS-terminierenden Ingress |
| `id` | SPIFFE-ID | – | Der Server muss genau diese ID haben, z. B. `spiffe://agentswork.de/ns/reviewer/sa/reviewer` |
| `ids` | Liste von SPIFFE-IDs | – | Der Server muss eine dieser IDs haben |
| `trust_domain` | Text | – | Der Server darf jede ID dieser Trust-Domain haben |
| `audience` | Text | – | Audience des JWT-SVIDs; bei `mode: "jwt"` Pflicht, bei `mtls` ein Fehler |

Bei `mtls` ist genau eines von `id`, `ids` und `trust_domain` Pflicht; bei `jwt` ist eines davon optional (dann wird die TLS-Verbindung gegen das SPIFFE-Bundle statt gegen die Web-PKI geprüft). Die Ziel-URL muss `https://` sein. Ohne `SPIFFE_ENDPOINT_SOCKET` schlägt das Laden der Spec fehl und nennt das Tool bzw. den Server.

### `knowledge`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `sources` | Liste von Pfaden | `[]` | Dateien oder Verzeichnisse (rekursiv); mindestens eines von `sources` und `urls` |
| `urls` | Liste von URLs | `[]` | Webseiten, die bei jedem Start mit bedingten Anfragen geholt werden (ADR-0113): `https://` (`http://` nur für `localhost`/`127.0.0.1`), keine Zugangsdaten, kein Fragment, keine Doppelten; Text, Markdown, HTML |
| `min_similarity` | Zahl | – | Mindestähnlichkeit 0 bis 1 eines Treffers |
| `inject` | bool | `false` | Fügt die besten `top_k` Abschnitte jeder neuen Benutzereingabe als Systemnachricht hinzu |
| `tool_enabled` | bool | `true` | Bietet das Such-Tool an; `false` braucht `inject: true` |
| `embedding_model` | Modell | **Pflicht** | Embedding-Modell, z. B. `ollama`/`nomic-embed-text` |
| `tool` | ID | `"search_knowledge"` | Name des Such-Tools |
| `top_k` | Zahl | `3` | Abschnitte je Suche |
| `chunk_size` | Zahl | `1000` | Höchstlänge eines Abschnitts in Zeichen |
| `index` | Pfad | – | Datei, in der Abschnitte und Embeddings zwischen den Läufen erhalten bleiben (nur Speicher `file`) |
| `store` | Objekt | `{ "kind": "file" }` | `{ "kind": "file" }` oder `{ "kind": "postgres", "url_env": "PGVECTOR_URL" }`: PostgreSQL mit pgvector (ADR-0106); `url_env` nennt die Umgebungsvariable mit der Verbindungs-URL; die URL steht nie in der Spec |

### `long_term_memory`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `store` | Pfad | – | JSON-Datei mit Fakten und Embeddings; genau eines von `store` und `vector_store` |
| `vector_store` | Objekt | – | PostgreSQL mit pgvector (ADR-0106): `{ "kind": "postgres", "url_env": "PGVECTOR_URL", "ttl_days"?: n, "min_similarity"?: 0..1 }`; `url_env` nennt die Umgebungsvariable mit der Verbindungs-URL; `ttl_days` (mindestens 1) lässt Fakten ablaufen, `min_similarity` wirft schwache Treffer beim Erinnern weg; ohne sie laufen Fakten nie ab und `top_k` Fakten kommen immer zurück |
| `embedding_model` | Modell | **Pflicht** | Embedding-Modell |
| `remember_tool` | ID | `"remember"` | Name des Tools zum Merken |
| `recall_tool` | ID | `"recall"` | Name des Tools zum Erinnern |
| `top_k` | Zahl | `3` | Fakten je Erinnern |
| `auto_recall` | bool | `false` | Vor jedem Lauf passende Fakten erinnern und als Systemnachricht anhängen |
| `auto_remember` | bool | `false` | Nach jedem erfolgreichen Lauf Fakten speichern |
| `remember_compressed` | bool | `false` | Dauerhafte Fakten aus der Verlaufs-Kompression als Fakten speichern (`chat`, `compress_history` mit `facts`) (ADR-0079) |
| `tools` | bool | `true` | Bietet die zwei Tools an; mit `false` braucht es `auto_recall`, `auto_remember` oder `remember_compressed` |

### `subgraphs.<id>`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `input` | Text | `"string"` | Eingabetyp |
| `output` | Text | `"string"` | Ausgabetyp |
| `tools` | Liste von Tool-Namen | alle Tools | Im Subgraphen verfügbare Tools |
| `self_tools` | bool | wie außerhalb | Bauen und Benutzen selbst gebauter Tools erlauben (`true`) oder verbieten (`false`); braucht `self_tools` oben |
| `model` | Modell | äußeres Modell | Eigenes Modell im Subgraphen |
| `graph` | Graph | **Pflicht** | Ablauf; `start` liefert die Eingabe, `finish` nimmt die Ausgabe |

### `strategy`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `kind` | Text | **Pflicht** | `single_run`, `react`, `verified` oder `chat` |
| `params` | Objekt | – | Parameter, abhängig von `kind` |

| `kind` | Parameter | Ablauf |
|---|---|---|
| `single_run` | `parallel_tools` (bool), `moderate` (bool) | Tool-Kreislauf `call_llm` → `execute_tool` → `send_result`; eine Antwort mit mehreren Tool-Aufrufen nimmt `execute_tools_in_order` → `send_results` und führt sie nacheinander aus (alle Strategien außer `parallel_tools`); mit `parallel_tools` laufen die Aufrufe einer Antwort gleichzeitig (`execute_tools`); mit `moderate` läuft vorher eine Moderation (legt den Typ `Moderation` an) |
| `react` | `reasoning_prompt` (Text) | vor jeder Aktion nachdenken (`think`, `reflect`; `reflect_all` nach mehreren Aufrufen in einer Antwort) |
| `verified` | `verify_prompt` (Text) | die Antwort prüfen lassen und mit dem Feedback überarbeiten (legt den Typ `Verification` an) |
| `chat` | `max_messages` (≥ 1, Standard 30), `keep` (≥ 0, Standard 10, kleiner als `max_messages`), `parallel_tools` (bool, Standard `true`) | vor der Antwort `compress` (`compress_history` mit `facts`), wenn der Verlauf länger als `max_messages` ist; danach ein Tool-Kreislauf wie bei `single_run` |

Die angelegten Typnamen sind reserviert; ein eigener Typ mit demselben Namen ist ein Fehler.

### `graph`

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `nodes` | Liste von Nodes | `[]` | `start` und `finish` gibt es implizit |
| `edges` | Liste von Kanten | `[]` | Gerichtete Kanten; die erste passende gewinnt |

**Node:** `id` (ID, Pflicht, nicht `start`/`finish`), `kind` (Pflicht, siehe
[Node-Arten](#node-arten)), `params` (abhängig von der Art).

**Kante:** `from` und `to` (Pflicht; Node-ID, `start` oder `finish`), `when` (optional: `kind` und
`params` einer [Bedingung](#bedingungen)), `transform` (optional, siehe unten). Ohne `when` wird der
Wert unverändert weitergegeben.

**`transform`** (ADR-0102): Code, der den Wert der Kante abbildet, bevor der Ziel-Node ihn erhält.

| Feld | Typ | Standard | Bedeutung |
|---|---|---|---|
| `input` | Typname | was die Kante liefert | Typ des Werts; wenn angegeben, muss er genau dem entsprechen, was die Kante liefert (Ausgabe des Quell-Nodes, nach der Bedingung) |
| `output` | Typname | Pflicht | Typ des Ergebnisses; der Ziel-Node erhält ihn und er muss der Eingabetyp des Ziels sein |
| `code` | Text | Pflicht (oder `code_file`) | Rumpf von `fn transform(value: In) -> Out` |
| `code_file` | Pfad | – | Code aus einer Datei relativ zur Spec statt `code`; genau eines von beiden |

Er läuft, nachdem die Bedingung gepasst hat, und bevor das Ziel läuft; die Umwandlung ist eine reine
Funktion in der Sandbox wie der Tool-Code. Typen: `string`, ein Typ aus `types` oder `array<T>` von
diesen. Siehe [Kapitel 4](04-ablaeufe.md#code-in-graphen-code-on_code-und-transform).

## Datentypen auf Kanten

| Typ | Bedeutung |
|---|---|
| `string` | Text |
| `message` | Antwort eines LLM (Text und/oder Tool-Aufrufe) |
| `tool_call` / `tool_calls` | ein oder alle Tool-Aufrufe einer Antwort |
| `tool_result` / `tool_results` | Ergebnis eines oder mehrerer Tool-Aufrufe |
| `<Name>` | benannter Typ aus `types`; wird über den Namen verglichen |
| `array<T>` | Liste von Werten des Typs `T` (`string` oder ein Typ aus `types`); nur in den Parametern von Nodes des Typs `agent` (`for_each`) und in Subgraph-Signaturen, nie als Typ des Agenten selbst (ADR-0101) |

## Node-Arten

„Eingabe → Ausgabe“ ist die Signatur, gegen die die Kanten geprüft werden. Parameter ohne Markierung
sind optional; unbekannte Parameter sind ein Fehler.

| Art | Parameter | Eingabe → Ausgabe | Bedeutung |
|---|---|---|---|
| `llm_request` | – | `string` → `message` | das LLM fragen, Tools werden angeboten |
| `llm_request_streaming` | – | `string` → `message` | wie `llm_request`, der Text wird live ausgegeben |
| `llm_request_only_tools` | – | `string` → `message` | ein Tool-Aufruf ist Pflicht |
| `llm_request_force_tool` | `tool` (Pflicht) | `string` → `message` | genau dieses Tool muss aufgerufen werden |
| `llm_request_best_of` | `choices` (Pflicht, ≥ 1), `criteria` | `string` → `message` | mehrere Antworten, die beste geht in den Verlauf |
| `llm_request_structured` | `type` (Pflicht), `retries` (≥ 0) | `string` → `type` | Antwort als benannter Typ |
| `llm_continue` | – | `string` → `message` | noch einmal fragen, ohne die Eingabe anzuhängen |
| `llm_reason` | `value` (Pflicht: `string`, `tool_result` oder `tool_results`), `prompt` | `value` → `string` | ohne Tools nachdenken |
| `llm_verify` | `type` (Pflicht), `prompt` | `string` → `type` | die Antwort prüfen; Urteil mit `correct`, `feedback`, `answer` |
| `llm_moderate` | `type` (Pflicht), `provider` + `id` (nur zusammen), `temperature`, `think`, `fallbacks` | `string` → `type` | Text moderieren; Urteil mit `harmful`, `categories`, `text`, `message` |
| `execute_tool` | – | `tool_call` → `tool_result` | ein Tool ausführen |
| `llm_send_tool_result` | – | `tool_result` → `message` | das Ergebnis an das LLM schicken |
| `execute_multiple_tools` | `sequential` (bool, Standard `false`) | `tool_calls` → `tool_results` | mehrere Tools gleichzeitig ausführen; mit `sequential: true` nacheinander in der Reihenfolge der Aufrufe |
| `llm_send_multiple_tool_results` | – | `tool_results` → `message` | mehrere Ergebnisse schicken |
| `get_field` | `type`, `field` (beide Pflicht) | `type` → `string` | ein Feld eines benannten Typs als Text |
| `compress_history` | `value`, `strategy` (Pflicht: `summarize`, `keep_last` oder `facts`), `keep` (≥ 0, Pflicht bei `keep_last` und `facts`) | `value` → `value` | den Verlauf komprimieren (`facts`: ältere Nachrichten als Liste von Fakten), den Wert durchreichen |
| `agent` | `agent` (Pflicht: Name eines Eintrags aus `agent_tools`), `input`, `output` (Typnamen, Standard `string`; keine Listen; sie müssen die Typen der Spec des Unteragenten sein) | `input` → `output` | den Unteragenten als Schritt ausführen: eigener Verlauf; Zähler für Nutzung, Kosten und Tool-Aufrufe, Grenzen und Freigaben werden mit dem Aufrufer geteilt (ADR-0101) |
| `decide` | `value`, `kind` (`choice`, `yes_no` oder `score`), `question` (alle Pflicht); `options` (Pflicht bei `choice`: 1–255 `{ name, description? }`, eindeutige Namen), `levels` (Pflicht bei `score`: 1–255 eindeutige Beschreibungen, niedrigste zuerst), `threshold` (`yes_no`, 0–1, Standard 0.5), `yes`, `no` (`yes_no`: was sie bedeuten), `decider` (`system_one` oder `llm`, Standard `llm`), `model` (ein System-One-Checkpoint, oder `provider/id` bei `llm`), `temperature` (0–2), `think` (bool; nur `ollama`) – beides nur beim Entscheider `llm`, nur für die Entscheidungsanfrage | `value` → `value` | einen Entscheider zu dem Wert befragen; der Wert wird durchgereicht, das Urteil landet im Speicher unter `grauwolf.decision` (ADR-0100) |
| `set_model` | `provider`, `id`, `value` (alle Pflicht), `temperature`, `think`, `fallbacks` | `value` → `value` | das Modell für den Rest des Laufs wechseln; `fallbacks` wie bei `model` (ADR-0088) |
| `store` | `key`, `value` (Pflicht) | `value` → `value` | einen Wert im Speicher des Agenten ablegen |
| `load` | `key`, `value`, `type` (Pflicht) | `value` → `type` | einen Wert aus dem Speicher holen; die Eingabe wird ignoriert |

Diese Arten stehen nicht im Katalog, gehören aber zu den Subgraphen ([Kapitel 6](06-subgraphen-und-planer.md)):

| Art | Parameter | Eingabe → Ausgabe | Bedeutung |
|---|---|---|---|
| `subgraph` | `subgraph` (Pflicht) | wie der Subgraph | einen Subgraphen ausführen |
| `plan` | `subgraph` (Pflicht, `string` → `string`), `critic` (bool), `max_iterations` (≥ 1, Standard 20) | `string` → `string` | LLM-Planer; jeder Schritt läuft im Subgraphen |
| `parallel` | `subgraphs` (Pflicht, ≥ 2 mit gleicher Signatur), `merge` (Pflicht: `concat` – nur mit Ausgabe `string` –, `index` mit `index`, `llm_select` mit optionalen `criteria`) | wie die Zweige | Zweige gleichzeitig ausführen, dann zusammenführen |
| `for_each` | `body` (Pflicht: `{ "agent": "<name>" }` oder `{ "subgraph": "<name>" }`), `input` (`array<T>` oder `string`, Standard `array<string>`), `output` (`array<U>` oder `string`, Standard `array<string>`), `max_parallel` (1–16, Standard 1), `on_error` (`fail` oder `skip`, Standard `fail`) | `input` → `output` | den Rumpf einmal für jedes Element einer JSON-Liste auf einem eigenen Fork ausführen, jeweils `max_parallel` gleichzeitig; Ergebnisse in der Reihenfolge der Eingabe; bei `skip` fehlen gescheiterte Elemente (Ereignis `for_each_item_failed`); Fortschritt je Element unter `grauwolf.for_each.<id>`; eine Text-`input` wird als JSON-Array gelesen (ein Code-Block wird ignoriert), eine Text-`output` ist die Liste als JSON-Text (ADR-0101) |
| `code` | `input`, `output` (Pflicht: `string`, ein Typ aus `types` oder `array<T>` davon), `code` (Pflicht, oder `code_file`: ein Pfad relativ zur Spec; genau eines von beiden) | `input` → `output` | den Rumpf von `fn run(input: In) -> Result<Out, String>` in der Sandbox ausführen (Tool-ABI v1, Fuel- und Speichergrenzen) auf den Wert; `Err(message)` lässt den Node scheitern; eine reine Funktion: kein Verlauf, kein Speicher, keine Tools, kein Modell, keine Uhr und kein Zufall (ADR-0102) |
| `goap` | `state` (Pflicht), `values`, `actions` (Pflicht, ≥ 1), `goals` (Pflicht, ≥ 1), `input_fact`, `output_fact`, `max_iterations` (≥ 1, Standard 20), `max_search_nodes` (≥ 1, Standard 10000), `max_plan_depth` (≥ 1, Standard 50) | `string` → `string` | Planung über benannte Fakten |

Für `goap`: `state` ist `{ "fact": true|false|integer|number|"text" }`; der Typ folgt dem Anfangswert
(`0` ganze Zahl, `0.0` Zahl); ein Text-Fakt mit einer Liste in `values` (`{ "fact": ["text", …] }`) ist
eine Aufzählung, ohne Liste ein freier Text. Jede Aktion hat `name` (eindeutig) und genau einen
Ausführer: `subgraph` (Eingabe `string`) oder `agent` (ein Eintrag aus `agent_tools`). Optional: `pre`
(Bedingungen: ein Wert oder ein Operator `eq`/`ne`/`gt`/`lt`/`ge`/`le`; Vergleiche nur bei ganzen Zahlen
und Zahlen), `effects`, `on_success` (Standard `effects`) und `on_failure` (Werte oder `{ "add": n }` bei
ganzen Zahlen und Zahlen), `cost` (≥ 0, Standard 1), `description`, `input` (Vorlage mit `{{fact}}` und
`{{task}}`), `output_fact` (freier Textfakt für die Antwort) und `success_contains` (Text). Jedes Ziel hat
`name`, `facts` (Bedingungen) und optional `priority` (ganze Zahl ≥ 0); verfolgt wird das Ziel mit dem
kleinsten `cost / (priority + 1)`. `input_fact` und `output_fact` benennen freie Textfakten für die
Eingabe und das Ergebnis des Nodes.

## Bedingungen

| Art | Parameter | Eingabe → weitergegeben | Trifft zu, wenn … |
|---|---|---|---|
| `on_assistant_message` | – | `message` → `string` | die Antwort Text ist |
| `on_tool_call` | – | `message` → `tool_call` | die Antwort ein Tool aufruft |
| `on_multiple_tool_calls` | – | `message` → `tool_calls` | die Antwort Tools aufruft (alle Aufrufe) |
| `on_field` | `field`, `equals` (beide Pflicht) | `<Typ>` → `<Typ>` | das Feld eines benannten Typs gleich `equals` ist |
| `on_decision` | `answers` (Pflicht: nicht leere Liste eindeutiger Antworten) | beliebig → unverändert | die Antwort der letzten Entscheidung eine aus `answers` ist; nur nach `decide` (ADR-0100) |
| `on_history_longer` | `messages` (Pflicht, ≥ 0) | beliebig → unverändert | der Verlauf mehr Nachrichten hat |
| `on_history_tokens_above` | `tokens` (Pflicht, ≥ 0) | beliebig → unverändert | der Verlauf mehr Tokens hat (geschätzt, ADR-0087) |
| `on_code` | `input` (Pflicht: der Ausgabetyp des Quell-Nodes), `code` (Pflicht, oder `code_file`) | `input` → unverändert | der Rumpf von `fn check(value: &In) -> bool` `true` liefert; ein Panic, Trap oder aufgebrauchtes Fuel ist ein Fehler des Laufs, nicht `false` (ADR-0102) |

## Fehler beim Laden

| Meldung | Bedeutung |
|---|---|
| `invalid spec JSON: …` | kein gültiges JSON (mit Zeile und Spalte) |
| `<pointer>: unknown field (allowed: …)` | Tippfehler in einem Feldnamen |
| `<pointer>: missing required field` | ein Pflichtfeld fehlt |
| `<pointer>: wrong type: …, expected …` | z. B. eine Zahl statt Text |
| `<pointer>: unknown value …, allowed: …` | ein Wert außerhalb einer festen Auswahl |
| `spec_version <n> is not supported (supported: 1)` | eine andere Version |
| `/strategy: …` | unbekannte Vorlage, ungültige Parameter, neben `graph` oder ein reservierter Typname |
| `` /tools/<i>: give exactly one of `code` and `code_file` `` | beides angegeben (ohne eines: `/tools/<i>/code: missing required field`) |
| `/tool_files/<i>: …`, `/tools/<i>/code_file: …` | Datei fehlt, ist nicht lesbar oder kein JSON |
| `` /graph/nodes/<i>/params: give exactly one of `code` and `code_file` ``, `…/params/code_file: …` | beides angegeben oder die Datei fehlt; ebenso bei `…/edges/<i>/when/params` und `…/edges/<i>/transform` sowie bei `/subgraphs/<name>/graph` (ADR-0102) |
| `<spec>: <registry> …`, `<spec>: invalid tool package: …`, `<lockfile> is out of date: …` | Fehler von Tool-Paketen beim Laden einer Spec: Registry nicht erreichbar oder abweisend, Digest stimmt nicht, kein grauwolf-Paket, unbekannte ABI-Version, ein Tool-Name, der nicht existiert oder kollidiert, eine Lock-Datei, die nicht passt (siehe [Kapitel 3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages)) |

## Validierungsfehler

`spec validate` meldet alle Fehler auf einmal, je mit einem JSON-Pointer:
`<file>:<pointer>: <message>`.

| Fehlerart | Meldung | Bedeutung und typische Stelle |
|---|---|---|
| `UnknownNode` | `unknown node '<id>'` | eine Kante zeigt auf einen Node, den es nicht gibt (`…/edges/<i>/from` oder `/to`) |
| `ReservedNodeId` | `'<id>' is reserved …` | ein Node heißt `start` oder `finish` |
| `InvalidId` | `'<id>' is not a valid ID …` | Name der Spec, eines Tools, Nodes, Subgraphen, Servers o. Ä. ist nicht snake_case |
| `DuplicateNodeId` | `node ID '<id>' is used more than once` | zwei Nodes mit derselben ID in einem Graphen |
| `DuplicateFallback` | `fallback model '<provider>:<id>' is already listed` | ein Ausweichmodell ist das Modell selbst oder ein früheres Ausweichmodell (`…/fallbacks/<i>`) |
| `TemperatureOutOfRange` | `temperature must be a number from 0 to 2` | `…/temperature` eines Modells, Ausweichmodells, Nodes `set_model`/`llm_moderate` oder `decide` |
| `ThinkNotSupported` | `'think' is only supported by the provider 'ollama', not '<provider>'` | `…/think` (bei einem Node `decide`: der Anbieter seines `model`, sonst des Hauptmodells) |
| `SamplingWithSystemOne` | `the 'system_one' decider is a decision model: it has no temperature or think` | `…/params/temperature` oder `…/params/think` eines Nodes `decide` mit `decider: system_one` |
| `EmbeddingSampling` | `embedding models have no temperature or think` | `temperature`/`think` bei einem Embedding-Modell |
| `EmbeddingFallbacks` | `embedding models have no fallback models: a different model would produce different vectors` | `fallbacks` bei `knowledge.embedding_model` oder `long_term_memory.embedding_model` |
| `DuplicateToolName` | `tool name '<name>' is used more than once (the other one is the <origin> at <pointer>)` | ein Name, den im gemeinsamen Namensraum schon eine frühere Deklaration hat – gemeldet bei der späteren Deklaration, der Text nennt die frühere (Herkunft: eigenes Tool, MCP-Tool, MCP-Ressourcen-Tool, Such-Tool des Wissens, Gedächtnis-Tool, eingebautes Tool, Skill-Tool, Unteragent, Paket-Tool nach dem Umbenennen, Bau-Tool, Code-Stück). Die Reihenfolge der Deklarationen ist: `tools`, `mcp_servers` (Tools, dann Ressourcen-Tools), `knowledge`, `long_term_memory`, `builtin_tools`, `skills` (`load_skill`, `read_skill_file`, bei `/skills`), `agent_tools`, `tool_packages` (`/tool_packages/<i>/tools/<j>` oder `/as/<name>`), `self_tools`, dann die Code-Stücke des Graphen (`code_main_<node>`, …, bei `…/code` des Stücks) |
| `SkillSource` | `` give exactly one of `dir` and `git` `` | ein Eintrag von `skills` mit keinem oder beiden (`/skills/<i>`); andere Probleme eines Eintrags sind `InvalidGitSource` (`/skills/<i>/dir`, `/git`, `/path`, `/rev`, …) oder `InvalidId` (`/skills/<i>/names/<j>`) |
| `ToolPackageSource` | `` give exactly one of `oci` and `git` `` | ein Eintrag von `tool_packages` mit keinem oder beiden (`/tool_packages/<i>`) |
| `UnsupportedToolSource` | `git sources for tool packages are not supported yet` | `/tool_packages/<i>/git` |
| `InvalidOciReference` | `invalid OCI reference: …` | `/tool_packages/<i>/oci` ist nicht `registry/repository[:tag][@sha256:…]` (die Meldung wiederholt die Referenz nie) |
| `UnknownNodeKind` | `unknown node kind '<kind>'` | `kind` steht nicht im Katalog |
| `InvalidParams` | `invalid parameters for '<kind>': <reason>` | Parameter eines Nodes oder einer Bedingung passen nicht (z. B. `/extra: unknown parameter`) |
| `UnknownConditionKind` | `unknown condition '<kind>'` | `when.kind` ist unbekannt |
| `InvalidTypeName` | `'<name>' is not a valid type name …` | Typname in `types` ist nicht PascalCase |
| `UnknownType` | `unknown type '<name>' …` | `input`, `output` oder ein Parameter eines Nodes nennt einen nicht deklarierten Typ |
| `FinishUnreachable` | `'finish' is not reachable from 'start'` | der Graph kann nie ein Ergebnis liefern |
| `UnreachableNode` | `node '<id>' is not reachable from 'start'` | der Node würde nie laufen |
| `DeadEnd` | `no path leads from node '<id>' to 'finish' …` | der Lauf würde dort hängen bleiben; eine ausgehende Kante fehlt |
| `UnknownSubgraph` | `unknown subgraph '<name>'` | ein Node `subgraph`/`plan`/`parallel` oder der Rumpf eines `for_each` nennt einen fehlenden Subgraphen (beim Rumpf bei `…/params/body/subgraph`) |
| `UnknownAgent` | `unknown agent '<name>': it is not in agent_tools` | ein Node `agent` oder der Rumpf eines `for_each` nennt einen Agenten, den `agent_tools` nicht hat (bei `…/params/agent` oder `…/params/body/agent`) |
| `UnknownTool` | `unknown tool '<name>'` | `tools` eines Subgraphen nennt ein unbekanntes Tool, oder `builtin_tools.tools` ein unbekanntes eingebautes, oder `as` benennt ein Tool um, das `tools` nicht auswählt (`/tool_packages/<i>/as/<name>`) |
| `SubgraphCycle` | `subgraph '<name>' includes itself` | direkte oder indirekte Selbsteinbindung |
| `McpTransport` | `` MCP server needs exactly one of `command` (local) and `url` (remote) `` | keines oder beides angegeben |
| `AgentToolSource` | `` sub-agent needs exactly one of `spec` (local) and `url` (remote, A2A) `` | keines oder beides angegeben |
| `AuthWithoutUrl` | `` `auth` only applies to a remote server or agent with `url` `` | `…/auth` eines `command`-MCP-Servers oder eines lokalen `spec`-Unteragenten |
| `AuthWithAuthorizationHeader` | `` `auth` and an `Authorization` header exclude each other `` | `…/auth` neben einem `Authorization`-Eintrag (beliebige Schreibweise) in `headers` |
| `SpiffeMatch` | `` `auth.spiffe` needs exactly one of `id`, `ids` and `trust_domain` `` | `…/auth/spiffe` im Modus `mtls` ohne oder mit mehr als einem; im Modus `jwt` mit mehr als einem |
| `SpiffeAudience` | `` `auth.spiffe` with mode `jwt` needs an `audience` `` | `…/auth/spiffe` |
| `SpiffeAudienceWithMtls` | `` `audience` only applies to mode `jwt` `` | `…/auth/spiffe/audience` |
| `AuthHostNotAllowed` | ``'<host>' is not in `allowed_hosts`: `auth` can only name an allowed host`` | ein Schlüssel von `builtin_tools.fetch.auth`, der kein Eintrag von `allowed_hosts` ist (bei `…/fetch/auth/<host>`) |
| `AuthHostNotRepository` | ``'<host>' is not the host of `repo_url`: `auth` can only name the repository's host`` | ein Schlüssel von `builtin_tools.git.auth`, der nicht zum Host von `repo_url` passt |
| `GitTokenWithAuth` | `` `use_token` and `auth` exclude each other: the identity replaces `GIT_TOKEN` `` | `builtin_tools.git` mit beidem |
| `WorkloadApiWithoutStdio` | `` `workload_api` only applies to a local MCP server with `command` (stdio) `` | `mcp_servers.<id>.workload_api` bei einem Server mit `url` |
| `WorkloadApiWithShell` | `` `workload_api` is refused in shell mode: … `` | `builtin_tools.command.workload_api` mit `shell: true` |
| `InvalidSpiffe` | `invalid SPIFFE value: …` | `…/auth/spiffe/id`, `…/ids/<i>`, `…/trust_domain`, das keine SPIFFE-ID (`spiffe://<trust domain>/<path>`, Pfadsegmente ohne `.`/`..`/leer) oder Trust-Domain (`a-z 0-9 . - _`) ist |
| `MissingAgentDescription` | `` local sub-agent needs a `description` `` | `agent_tools.<id>` mit `spec`, aber ohne `description` |
| `UnusedMemory` | `` memory without tools needs `auto_recall`, `auto_remember` or `remember_compressed` `` | `long_term_memory.tools: false` ohne automatisches Gedächtnis |
| `InvalidLimit` | `invalid limit: …` | `limits.max_steps`, `max_tokens` oder `max_prompt_tokens` gleich 0, oder `max_cost` ≤ 0 |
| `UnsupportedSchema` | `unsupported schema: …` | Typ-Schema außerhalb der Untermenge (Objekt mit Feldern `string`, `integer`, `number`, `boolean`, `array` mit `items`) |
| `SaveWithoutToolsDir` | `` `save` needs a `tools_dir` to store the tools in `` | `self_tools.save` ohne `tools_dir` |
| `SelfToolsNotConfigured` | `` `self_tools` in a subgraph needs `self_tools` at the top of the spec `` | ein Subgraph steuert `self_tools`, die Spec erlaubt es aber nicht |
| `MissingBuiltinRoot` | `` built-in file and key-value tools need `root` (directory relative to the spec) `` | Dateitools oder Schlüssel-Wert-Tools ohne `builtin_tools.root` |
| `MissingBuiltinSection` | `` the tools of '<group>' need a `<group>` section in `builtin_tools` `` | ein Tool `jira_*`, `confluence_*`, `git_*`, `fetch` oder `run_command` ohne `builtin_tools.jira`, `.confluence`, `.git`, `.fetch` oder `.command` (am jeweiligen Pfad) |
| `EmptyAllowedCommands` | `allowed is empty: the command tool could not start any program` | `builtin_tools.command.allowed` ist `[]` |
| `WildcardWithoutShell` | `'*' is only allowed together with "shell": true` | `*` in `allowed` (`…/allowed/<i>`) ohne Shell-Modus |
| `InvalidAllowedProgram` | `'<program>' is not a program: …` | ein Eintrag, der leer ist, Leerzeichen enthält oder ein relativer Pfad mit `/` ist |
| `ShellNeedsWildcard` | `shell mode needs exactly "allowed": ["*"]` | `shell: true` mit einem anderen `allowed` |
| `ShellNotAcknowledged` | `shell mode needs "i_understand_shell": true` | `shell: true` ohne die Bestätigung |
| `CommandLimitOutOfRange` | `<field> is out of range` | `timeout_secs` oder `max_output_bytes` ist 0 oder zu groß |
| `InvalidUrl` | `invalid URL: …` | `base_url` oder `repo_url` ist nicht `https://`, hat keinen Host oder enthält Benutzerangaben (die Meldung wiederholt die URL nie) |
| `EmptyAllowedHosts` | `allowed_hosts is empty: the fetch tool could not call any host` | `builtin_tools.fetch.allowed_hosts` ist `[]` |
| `InvalidAllowedHost` | `'<host>' is not a host: …` | ein Eintrag mit Schema, Pfad, Port oder einem überzähligen `*` (`…/allowed_hosts/<i>`) |
| `DirectoryOutsideRoot` | ``the directory must be relative and inside the root, without `..` `` | `builtin_tools.git.directory` ist absolut oder enthält `..` |
| `NoKnowledgeSources` | `knowledge needs at least one source` | `knowledge.sources` und `knowledge.urls` sind beide leer |
| `UnusedKnowledge` | `knowledge without the tool needs `inject: true`` | `knowledge.tool_enabled: false` ohne `inject` |
| `InvalidUrl` | `invalid URL: <reason>` | ein Eintrag von `knowledge.urls`, der nicht `https://` ist (oder `http://` für localhost), Zugangsdaten oder ein Fragment hat oder doppelt steht (auch von `fetch` und Git-Quellen genutzt) |
| `InvalidLimit` | `invalid limit: <reason>` | auch `knowledge.min_similarity` außerhalb von 0 bis 1 |
| `InvalidStore` | `invalid store: <reason>` | `knowledge.store` der Art `postgres` ohne `url_env` oder mit etwas, das kein Variablenname ist (der Grund wiederholt es nie), `url_env` bei der Art `file`, `knowledge.index` zusammen mit `postgres`; `long_term_memory` mit keinem oder beiden von `store` und `vector_store`, ein `vector_store.url_env`, das kein Variablenname ist, `ttl_days` von 0, `min_similarity` außerhalb von 0 bis 1 |
| `MissingCode` | `the code is missing or empty` | ein Node `code`, eine Bedingung `on_code` oder ein `transform` ohne Code (`…/params/code`, `…/transform/code`) |
| `TransformInputMismatch` | `the transformation declares the input <a>, the edge delivers <b>` | `transform.input` weicht von dem ab, was die Kante liefert (`…/edges/<i>/transform/input`) |
| `TypeMismatch` | `type error: target expects <a>, the edge delivers <b>` | eine Kante verbindet unverträgliche Typen |
| `ConditionInputMismatch` | `condition '<kind>' expects <a>, the source delivers <b>` | die Bedingung passt nicht zum Wert der Kante (bei `…/when`) |
| `SubAgentTypeMismatch` | `the agent '<name>' has the <input\|output> type '<a>', but the node declares '<b>'` | von `spec validate` und beim Laden gefunden, nicht von der Prüfung der einzelnen Spec: ein Node `agent` oder der Rumpf eines `for_each`-Agenten deklariert andere Typen als die eigene Spec des Unteragenten (bei `…/params/input` oder `…/params/output`) |
| `DecisionEdgeSource` | `` `on_decision` may only leave a `decide` node `` | eine Kante `on_decision` beginnt bei einem anderen Node (bei `…/edges/<i>/when`) |
| `DecisionEdgeCondition` | `` edges leaving the `decide` node '<node>' must use `on_decision` `` | eine Kante eines Nodes `decide` hat keine oder eine andere Bedingung (bei `…/edges/<i>`) |
| `UnknownDecisionAnswer` | `the decision '<node>' has no answer '<answer>'` | `answers` nennt etwas, das keine Option, kein `yes`/`no` und keine Stufe ist (bei `…/answers/<j>`) |
| `DecisionAnswerUncovered` | `no edge handles the answer '<answer>' of the decision '<node>'` | eine Antwort ohne Kante (am Node) |
| `DecisionAnswerDuplicated` | `the answer '<answer>' of the decision '<node>' is handled by more than one edge` | zwei Kanten für eine Antwort (an der zweiten Kante) |

## Validierungswarnungen

Warnungen weisen auf Specs hin, die gültig sind, aber vermutlich nicht gewollt. Sie lassen
`spec validate` nie scheitern (Exit-Code `0`); die Befehle, die eine Spec laden, geben sie auf stderr
aus, als `warning: <file>:<pointer>: <message>`.

| Warnung | Bedeutung und Stelle |
|---|---|
| `shell mode gives the model a shell with the permissions of this process; …` | `builtin_tools.command.shell` ist `true` (bei `/builtin_tools/command/shell`) |
| ``answers with several tool calls have no edge; add an `on_multiple_tool_calls` edge …`` | ein Node, dessen Antwort Tool-Aufrufe enthalten kann (`llm_request`, `llm_send_tool_result`, `llm_continue`, `llm_request_streaming`, …), hat eine Kante `on_tool_call`, die genau einen Aufruf braucht, aber keine Kante `on_multiple_tool_calls`; schickt das Modell zwei Aufrufe in einer Antwort, bricht der Lauf mit `no matching edge` ab. Am Node (`…/nodes/<i>`), auch in Subgraphen. Die [Strategien](04-ablaeufe.md) zeigen das Muster. |
