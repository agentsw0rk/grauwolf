# CLI-Referenz

Alle Befehle und Optionen von `grauwolf`. Erklärungen mit Beispielen stehen in den Kapiteln; die
Spalte „Kapitel“ verweist dorthin. `grauwolf <befehl> --help` zeigt jederzeit die aktuelle Hilfe.

Alle Befehle schreiben Ergebnisse auf stdout und Meldungen, Rückfragen und Fehler auf stderr. Der
Exit-Code ist `0` bei Erfolg und `1` bei einem Fehler (`2` bei falschen Argumenten). Fehler in Specs nennen Datei und
JSON-Pointer, z. B. `spec.json:/graph/nodes/0/kind: unknown node kind 'llm_answer'`. Warnungen zu gültigen, aber verdächtigen Specs gibt jeder Befehl, der eine Spec lädt, einmal auf stderr aus, als `warning: <datei>:<pointer>: <meldung>`; sie ändern nie den Exit-Code.

## Überblick

| Befehl | Zweck | Kapitel |
|---|---|---|
| `grauwolf spec validate <datei>` | Spec prüfen, einschließlich Anbieternamen, lokaler Skill-Verzeichnisse und des ganzen Baums lokaler Unteragenten (Zyklen und Unteragenten-Specs außerhalb des Verzeichnisses der Spec sind Fehler); meldet alle Fehler mit JSON-Pointer, sonst `<datei>: valid`; Warnungen (`warning: <datei>:<pointer>: …` auf stderr, siehe [Spec-Referenz](referenz-spec.md#validierungswarnungen)) ändern den Exit-Code nicht | [1](01-erste-schritte.md) |
| `grauwolf spec graph <datei>` | Ablauf als Mermaid-Diagramm ausgeben (`flowchart TD`) | [4](04-ablaeufe.md) |
| `grauwolf spec egress <datei> [--format text\|json] [--otel]` | Alle Ziele auflisten, die der Agent und seine lokalen Unteragenten erreichen dürfen, abgeleitet aus den Specs: Modell-Hosts, MCP-Server über HTTP, entfernte Agenten, `fetch`-Hosts, Jira/Confluence, Git, System One, Wissens-URLs, pgvector, OpenBao (aus einer `grauwolf.toml` neben der Spec) und mit `--otel` den OTLP-Endpunkt. Eine Zeile je Ziel oder ein JSON-Array mit `scheme`, `host`, `port`, `any_port` (ein `fetch`-Host im Cluster hat `port: null` und `any_port: true`), `purpose`, `pointer`, `phase` (`run` oder `build`) und `from_env` | [13](13-cluster-einrichten.md#begrenzen-was-ein-agent-erreichen-darf-egress) |
| `grauwolf spec schema` | JSON-Schema des Spec-Formats ausgeben | [Spec-Referenz](referenz-spec.md) |
| `grauwolf spec build <datei>` | Tool-Code zu WASM bauen, für die Spec und ihre Unteragenten; eine Zeile `<name>: <hash>` je Tool | [3](03-tools.md) |
| `grauwolf spec run <datei> <eingabe>` | Agent einmal ausführen | [1](01-erste-schritte.md) |
| `grauwolf spec chat <datei>` | Chatten: jede Zeile von stdin ist ein Lauf im selben Gespräch | [1](01-erste-schritte.md), [7](07-gespraeche-und-wissen.md) |
| `grauwolf spec serve <datei>` | Agent als HTTP-Dienst (mit A2A und MCP) | [9](09-betrieb.md) |
| `grauwolf spec mcp <datei>` | Agent als MCP-Server über stdin/stdout | [9](09-betrieb.md) |
| `grauwolf spec resume <datei> --run-id <id>` | Abgebrochenen Lauf am letzten Checkpoint fortsetzen | [9](09-betrieb.md) |
| `grauwolf spec generate <datei> --out <verz>` | Rust-Crate aus der Spec erzeugen | [11](11-code-erzeugen.md) |
| `grauwolf version [--json]` | `grauwolf <version>` ausgeben (wie `--version`). Mit `--json` ein JSON-Objekt `{"version": "<version>", "builder_image": "ghcr.io/agentsw0rk/grauwolf-builder:<version>"}`: die Version der CLI und das Builder-Image, das `spec generate --dockerfile` standardmäßig benutzt, für Werkzeuge, die beides prüfen, ohne Text zu parsen | [9](09-betrieb.md#das-builder-image-agent-bau-offline-in-minuten) |
| `grauwolf spec create <anforderung> --model … --out …` | Ein LLM eine Spec schreiben lassen | [11](11-code-erzeugen.md) |
| `grauwolf tools pull <ref>` | Ein Tool-Paket in den Cache holen und prüfen. OCI-Referenz: Digests und die Tool-Richtlinie (erlaubte Registries, vorgeschriebene Signatur). Git-Referenz `git+<url>[//<pfad>]@<commit>\|@tag:<tag>\|@branch:<branch>`: Host-Allowlist, SSH-Signatur, Abruf (`GIT_TOKEN`), Bau aus den Quellen (Rust-Toolchain, `--build-wrapper`); gibt `<ref> -> commit <40 hex>` und die Tools aus | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `grauwolf tools push <verz\|layout> <ref> [--sign-key <schlüssel>] [--catalog <k>] [--no-catalog]` | Einen Paketordner (vorher gepackt) oder ein mit `tools pack` geschriebenes Layout in eine OCI-Registry hochladen; gibt `<ref>@sha256:…` aus. Mit `--sign-key` (ein privater Schlüssel aus `tools keygen`) wird das Paket auch signiert: eine cosign-kompatible Signatur wird daneben hochgeladen. Zugangsdaten: `GRAUWOLF_REGISTRY_AUTH`, sonst die Docker-Konfiguration. Trägt die hochgeladene Version in den Tool-Katalog ein (`--catalog`, sonst `GRAUWOLF_TOOLS_CATALOG`, sonst `<registry>/grauwolf-catalog:latest`; der Signaturschlüssel signiert auch den Index); `--no-catalog` lässt das aus | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `grauwolf tools pack <verz> [--out <verz>]` | Die Tools eines Paketordners bauen und als OCI-Image-Layout packen (Standard `<verz>/dist`, mit Quellen; reproduzierbare Digests) | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `grauwolf tools inspect <git-ref> [--allowed-signers <datei>]` | Git-Paket (Referenz wie bei `tools pull`): den Commit holen, **ohne zu bauen**; zeigt Commit, Name, Version, ABI, Tools mit Argumenten, ob es im Cache liegt (dann mit Modul-Hashes) und die Signatur (`signed by <principal> (<fingerprint>)`, `unsigned`, `signature invalid: …` oder `signed, not checked` ohne Signierer), geprüft gegen `git_allowed_signers` der Tool-Richtlinie oder `--allowed-signers`. Die Host-Allowlist gilt; eine fehlende Signatur wird angezeigt, nicht abgelehnt | [3](03-tools.md#tool-pakete-aus-einem-git-repository) |
| `grauwolf tools inspect <ref> [--key <pub>]…`; außerdem die Metadaten des Pakets aus den Manifest-Annotationen (Beschreibung, Build-Datum, Quelle, Revision, Lizenz, Autoren, grauwolf- und rustc-Version) und pro Tool seine `limits` | Digest, Signaturstatus (`signed by <fingerprint>`, `unsigned`, `signature invalid`; geprüft mit den Schlüsseln der Tool-Richtlinie und jedem `--key`, einem PEM-Public-Key wie cosigns `cosign.pub`), ABI, Tools mit Argumenten und Module eines Pakets anzeigen. Die Allowlist der Tool-Richtlinie gilt; eine fehlende Signatur wird angezeigt, nicht abgelehnt |
| `grauwolf tools validate <verz>` | Einen Tool-Paketordner prüfen, ohne ihn zu bauen: `grauwolf-tools.json` und `src/<tool>.rs` jedes Tools (die Regeln von `tools pack`, einschließlich verbotener Konstrukte); meldet jedes Problem, Exit-Code 1, wenn es eines gibt | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `grauwolf tools keygen <name>` | Ein Signaturschlüsselpaar anlegen: `<name>.key` (PKCS#8-PEM, unverschlüsselt, Modus 0600: geheim halten) und `<name>.pub` (öffentlicher Schlüssel für `oci_keys` in der Tool-Richtlinie); legt einen fehlenden Ordner an (neue Ordner mit Modus 0700); überschreibt nie etwas | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `grauwolf tools build <verz> [--format text\|json] [--force]` | Die WASM-Module einer mit `spec generate --tool-sources` erzeugten Crate bauen, aus `tools/tools.json` und den Gast-Crates (Hash-Prüfung, isolierter Bau, Fehler je Tool; JSON: `{ "tools": [ { "name", "kind", "ok", "skipped", "errors": [ { "message", "line"?, "column"? } ] } ] }`; `kind` ist `tool`, oder `code`, `condition`, `transform` für den Code von Graphen) | [11](11-code-erzeugen.md#tool-code-als-quellen---tool-sources-und-grauwolf-tools-build) |
| `grauwolf tools lock <spec>` | Die `tool_packages` einer Spec (OCI und Git) und die Git-Quellen ihrer `skills` auflösen und `grauwolf.lock` daneben schreiben; deckt auch die Unteragenten der Spec ab (eine Lock-Datei je Baum, Schlüssel der Unteragenten tragen deren Specpfad) | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `grauwolf tools new <name> [--tool <tool>] [--dir <verz>]` | Einen Tool-Paketordner anlegen: `grauwolf-tools.json` mit Beschreibung, Lizenz, Autoren, Grenzen; `src/<tool>.rs` (Standard-Tool `hello`); sein `cargo test`; eine README; ein GitHub-Actions-Workflow, der im Image `grauwolf-tools` prüft, testet, packt und bei einem Tag `v*` signiert und pusht. Lehnt einen Ordner ab, der nicht leer ist | [15](15-der-tool-katalog.md#ein-repository-für-deine-tools-grauwolf-tools-new) |
| `grauwolf tools catalog list\|search <text>\|show <name> [--version <v>] [--catalog <k>] [--key <pub>]… [--json]` | Den Tool-Katalog lesen (`--catalog <registry\|referenz>`, sonst `GRAUWOLF_TOOLS_CATALOG`): Tabelle oder Detailansicht der Einträge, jeder mit dem Urteil deiner Tool-Richtlinie (`allowed`, `refused: <grund>`), und die Signatur des Index (`signed by <fingerprint>: valid`, `invalid`, `unsigned`; unter `require_signatures` abgelehnt, wenn nicht gültig) | [15](15-der-tool-katalog.md#tools-finden-catalog-list-search-show) |
| `grauwolf tools catalog add <ref>\|remove <name>@<version>\|sync [--repository <repo>]… [--verbose] [--catalog <k>] [--sign-key <schlüssel>]` | Den Tool-Katalog schreiben: den Eintrag eines Pakets hinzufügen oder auffrischen, eine Version entfernen oder den Index aus der Registry neu aufbauen (`/v2/_catalog` oder die genannten Repositories; andere Artefakte werden übersprungen und in der Zusammenfassung gezählt, `--verbose` nennt sie). Bedingtes Schreiben mit bis zu fünf Wiederholungen; `--sign-key` signiert den geschriebenen Index | [15](15-der-tool-katalog.md#schreiben-push-catalog-add-remove-sync) |
| `grauwolf token <benutzer>` | Benutzer-Token für `spec serve --users` erzeugen | [9](09-betrieb.md) |
| `grauwolf login [--tenant <id>] [--authority <url>] [--client-id <id>] [--scope <scope>] [--config-dir <verz>]` | Eine Person über den Device-Code-Flow bei Entra (oder Entra Local) anmelden und die Tokens zwischenspeichern (Modus 0600); `--status` zeigt, wer angemeldet ist, `--print-token` gibt das Access-Token aus (bei Bedarf erneuert) | [14](14-im-auftrag-von-personen.md#anmelden-auf-der-kommandozeile-grauwolf-login) |
| `grauwolf logout [--config-dir <verz>]` | Die zwischengespeicherten Tokens löschen | [14](14-im-auftrag-von-personen.md#anmelden-auf-der-kommandozeile-grauwolf-login) |
| `grauwolf identity serve [--port <n>] [--mtls-port <n>]` | Der Identitätsdienst einer Trust Domain (ADR-0111): tauscht Entra-Token von Personen für Aufrufer mit SPIFFE-Identität gegen Txn-Token, erneuert sie, veröffentlicht die Schlüssel | [14](14-im-auftrag-von-personen.md) |
| `grauwolf identity grant create\|list\|revoke …` | Die Grants einer Person für unbeaufsichtigte Läufe: anlegen (mit einer eigenen Anmeldung), auflisten, widerrufen | [14](14-im-auftrag-von-personen.md#unbeaufsichtigte-läufe-grants-und-zeitpläne) |
| `grauwolf call <url> [--spiffe-id <id>] [--grant <grt_…>] [--input <text>] [--json] [--token <t>] [--timeout <s>]` | Das `/run` eines Agenten einmal mit der Workload-Identität (oder einem Token) aufrufen und die Ausgabe zeigen; das Kommando eines Zeitplans | [14](14-im-auftrag-von-personen.md#unbeaufsichtigte-läufe-grants-und-zeitpläne) |
| `grauwolf credentials check <VAR>… [--all] [--config <datei>]` | Variablen über die Quellen für Zugangsdaten auflösen und je Variable eine Zeile `resolved <VAR> from <file\|openbao\|environment> (N bytes)` ausgeben, nie den Wert | [12](12-zugangsdaten-aus-openbao.md) |

## Argumente der Befehle

| Befehl | Argumente und eigene Optionen |
|---|---|
| `spec validate`, `spec graph` | `<datei>` – Pfad zur Spec |
| `spec egress` | `<datei>`; `--format text\|json` (Standard `text`), `--otel` |
| `spec schema` | keine |
| `spec build` | `<datei>`; [Build-Optionen](#build-optionen) |
| `spec run` | `<datei>`, `<eingabe>` – Text bei `input: string`, sonst JSON passend zum Eingabetyp; [Lauf-Optionen](#lauf-optionen); dazu `--attach`, `--run-id`, `--user` (unten) |
| `spec chat` | `<datei>`; [Lauf-Optionen](#lauf-optionen). Endet mit EOF (Strg+D) oder `/exit`; leere Zeilen werden übersprungen; `/attach <datei>` hängt eine Datei an die nächste Eingabe |
| `spec serve` | `<datei>`; [Lauf-Optionen](#lauf-optionen); dazu `--addr`, `--public-url`, `--users`, `--chat-api` (unten) |
| `spec mcp` | `<datei>`; [Lauf-Optionen](#lauf-optionen) |
| `spec resume` | `<datei>`, `--run-id <id>` (Pflicht), `--user <id>` (wie beim Start); [Lauf-Optionen](#lauf-optionen) |
| `spec generate` | `<datei>`, `--out <verz>` (Pflicht), `--grauwolf-crates`, `--check`, `--service`, `--full-facade`, `--dockerfile`, `--builder-image`, `--allow-builder-mismatch`, `--tool-sources`; [Build-Optionen](#build-optionen) |
| `spec create` | `<anforderung>`, `--model` und `--out` (Pflicht), `--max-repairs`; [Build-Optionen](#build-optionen) |
| `token` | `<benutzer>` |

### Eigene Optionen einzelner Befehle

| Option | Befehl | Standard | Bedeutung |
|---|---|---|---|
| `--attach <datei>` | `spec run` | – | Datei als Anhang der Eingabe (mehrfach möglich, je höchstens 20 MB): Bilder `.png`, `.jpg`/`.jpeg`, `.gif`, `.webp`, PDF `.pdf`, Audio `.wav`, `.mp3`. Was der Anbieter nicht versteht, lehnt der Lauf vor der Anfrage ab (ADR-0081). |
| `--run-id <id>` | `spec run` | – | Speichert vor jedem Node einen Checkpoint unter dieser ID (Buchstaben, Ziffern, `-`, `_`). Nicht mit `--conversation`; mit `--user` nutzt der Lauf das Gedächtnis dieses Benutzers. |
| `--run-id <id>` | `spec resume` | – (Pflicht) | Lauf, der fortgesetzt wird |
| `--user <id>` | `spec run`, `spec resume` | – | Benutzer des Laufs: sein Bereich im Langzeitgedächtnis und – mit `--conversation` – sein Gespräch. Braucht `long_term_memory` in der Spec oder `--conversation`. |
| `--addr <adresse>` | `spec serve` | `127.0.0.1:8080` | Adresse des Dienstes. Nicht-lokale Adressen nur mit `GRAUWOLF_SERVER_TOKEN` oder `--users`. |
| `--public-url <url>` | `spec serve` | gebundene Adresse | Öffentliche Basis-URL für die A2A-Agent-Karte, z. B. hinter einem Reverse-Proxy |
| `--users <datei>` | `spec serve` | – | Benutzerdatei `{ "<benutzer>": "sha256:<hex>" }` mit Einträgen aus `grauwolf token` |
| `--spiffe` | `spec serve` | aus | Zweiter Port über mTLS mit dem SVID der Workload API (Client-Zertifikate Pflicht) und JWT-SVIDs am HTTP-Port (`GRAUWOLF_SPIFFE`, [Kapitel 9](09-betrieb.md#agenten-rufen-agenten-mit-spiffe)) |
| `--spiffe-port <port>` | `spec serve` | `8443` | Port des mTLS-Listeners (`GRAUWOLF_SPIFFE_PORT`) |
| `--spiffe-users <datei>` | `spec serve` | – | Datei `{ "<spiffe-id>" \| "<präfix>/*": "<benutzer>" }`, die Aufrufer auf Benutzer abbildet; wird bei Änderung neu gelesen (`GRAUWOLF_SPIFFE_USERS`) |
| `--chat-api` | `spec serve` | aus | Bietet außerdem `POST /api/chat` und `GET /api/signature` an ([Kapitel 9](09-betrieb.md#die-chat-api-apichat-und-apisignature)); liest `agent-signature.json` und `agent-names.json` neben der Spec; `OLLAMA_BASE_URL` ersetzt `OLLAMA_HOST` |
| `--out <verz>` | `spec generate` | – (Pflicht) | Zielverzeichnis der Crate |
| `--grauwolf-crates <verz>` | `spec generate` | Checkout dieser CLI | Verzeichnis `crates/` eines grauwolf-Checkouts, auf das die Crate verweist |
| `--check` | `spec generate` | aus | Erzeugte Crate mit `cargo check` prüfen (mit `--service`: mit Feature `service`) |
| `--service` | `spec generate` | aus | Schreibt zusätzlich `src/main.rs`, das HTTP-Dienst-Binary `agent` hinter dem Cargo-Feature `service` ([Kapitel 11](11-code-erzeugen.md#als-http-dienst---service)) |
| `--full-facade` | `spec generate` | aus | Hängt von der Facade mit allen ihren Features ab, unabhängig davon, was die Spec braucht, damit der Abhängigkeitsgraph für jede Spec gleich ist und vorgebaute Abhängigkeiten passen ([Kapitel 11](11-code-erzeugen.md#gleiche-dependencies---full-facade)) |
| `--tool-sources` | `spec generate` | aus | Schreibt den Tool-Code als Quellen (`tools/<name>/`, `tools/tools.json`, `build.rs`), statt ihn zu kompilieren; kein Rust-Toolchain nötig; `grauwolf tools build <verz>` baut die Module ([Kapitel 11](11-code-erzeugen.md#tool-code-als-quellen---tool-sources-und-grauwolf-tools-build)) |
| `--dockerfile` | `spec generate` | aus | Schreibt zusätzlich `Dockerfile` und `.dockerignore`, die den Dienst offline im Builder-Image bauen, und kopiert die Dateien, die die Spec liest, nach `app/`; impliziert `--service`, `--full-facade` und `--grauwolf-crates /opt/grauwolf/crates` ([Kapitel 11](11-code-erzeugen.md#dockerfile---dockerfile)) |
| `--builder-image <image>` | `spec generate` | `ghcr.io/agentsw0rk/grauwolf-builder:<version der CLI>` | Builder-Image im `Dockerfile`; nur mit `--dockerfile` |
| `--allow-builder-mismatch` | `spec generate` | aus | Lässt die Versionsprüfung des `Dockerfile` weg, die den Bau scheitern lässt, wenn das Builder-Image nicht zur Version dieser CLI gehört; nur mit `--dockerfile` |
| `--model <anbieter>:<modell>` | `spec create` | – (Pflicht) | LLM des Builders und des erzeugten Agenten, z. B. `ollama:qwen3.5:9b` |
| `--out <datei>` | `spec create` | – (Pflicht) | Zieldatei der Spec |
| `--max-repairs <n>` | `spec create` | `3` | Höchstzahl der Reparaturen |

## `grauwolf-tools`

Das Programm `grauwolf-tools` (ADR-0118) führt die Tool-Befehle ohne die Agent-Laufzeit aus: `build`,
`pack`, `push`, `pull`, `inspect`, `lock`, `keygen`, `validate`, `new` und `catalog`, mit denselben Argumenten, Optionen,
Ausgaben und Exit-Codes wie `grauwolf tools <befehl>` (siehe die Tabelle oben und die Build-Optionen
unten; `--cache`, `--locked` und `--build-wrapper` wirken genauso). Es liest dieselben Umgebungsvariablen
(`GRAUWOLF_REGISTRY_AUTH`, `GIT_TOKEN`, `GRAUWOLF_TOOL_POLICY`, `GRAUWOLF_TOOLS_CATALOG`, `SOURCE_DATE_EPOCH`, `DOCKER_CONFIG`, `HOME`, `PATH`).
`grauwolf-tools --version` gibt `grauwolf-tools <version>` aus. Der einzige Unterschied: `grauwolf-tools
lock <spec>` liest die Spec und ihre lokalen Unteragenten nur mit der Spec-Crate und sperrt sie; es prüft
keine Anbieter, Skill-Verzeichnisse und nicht den Rest der Spec (das tut `grauwolf spec validate`). Das
Programm wird als Container-Image ausgeliefert (`ghcr.io/agentsw0rk/grauwolf-tools`), im Builder-Image
und mit `scripts/build-macos.sh --tools`.

## `grauwolf-k8-ops`

Das Programm `grauwolf-k8-ops` (ADR-0120) richtet SPIRE und OpenBao für Agenten auf Kubernetes ein; siehe
[Kapitel 13](13-cluster-einrichten.md). Exit-Codes: 0 ok, 1 ein Fehlschlag (bei `doctor` ein fehlgeschlagener
Check, bei `agent check` ein fehlgeschlagener oder abgelaufener Check, bei `agent deploy` ein Pod, der nicht
rechtzeitig bereit ist), 2 ein Bedienfehler.

| Befehl | Was er tut |
|---|---|
| `doctor [--json]` | Nur lesend: Voraussetzungen und Einrichtung, eine Zeile je Check (`ok`, `warn`, `fail`, Grund, nächster Schritt); JSON: `[ { "section", "check", "status", "reason", "hint" } ]`. Die Zeile `identity service`: installiert und bereit, nicht installiert (in Ordnung, solange kein Agent mit `--identity` ausgerollt ist) oder nicht bereit. Je Agent außerdem: `callers` (die ConfigMap lässt sich lesen, hängende Einträge), `NetworkPolicy` (passt zu den Aufrufern), `caller spec` (die Aufrufe einer ausgerollten Spec an andere Agenten: `auth.spiffe`, mTLS-Port, Erlaubnis) und eine Warnung, dass sich die Durchsetzung von NetworkPolicies über die API nicht erkennen lässt |
| `oidc install` | Der SPIRE-OIDC-Discovery-Provider in seinem eigenen Namespace, mit der Image-Version des laufenden SPIRE-Servers und seiner `ClusterSPIFFEID` |
| `openbao setup [--jwks-url <url>] [--bound-issuer <issuer>]` | Die JWT-Auth-Methode mit der `jwks_url` des Providers, der KV-v2-Mount und die Policy `grauwolf-k8-ops` (genau das, was das Werkzeug braucht, auch der Transit-Mount und die Schlüssel sowie Rolle und Policy `grauwolf-identity` von `identity install`; nach einem Update erneut ausführen) |
| `openbao token [--ttl <dauer>] [--renewable]` | Ein Token mit nur der Policy `grauwolf-k8-ops` erzeugen (Standard `--ttl 8h`) und auf stdout ausgeben, sonst nichts; braucht Root oder ein Admin-Token. Siehe [Ohne das Root-Token arbeiten](13-cluster-einrichten.md#ohne-das-root-token-arbeiten) |
| `agent add <name> [--namespace <ns>] [--pull-secret <ns>/<name>]` | Namespace (Pod Security Standard `restricted`), Service-Account, `ClusterSPIFFEID grauwolf-agents`, ConfigMap `grauwolf-config`, OpenBao-Rolle und Policy `agent-<name>` |
| `agent list [--json]` | Die Agenten mit Namespace und SPIFFE-ID |
| `agent remove <name> [--purge-secrets] [--yes]` | Namespace, Rolle und Policy löschen (und die Geheimnisse); nimmt den Agenten außerdem aus den Aufrufern (`grauwolf-callers`) und NetworkPolicies der Agenten, die ihn erlaubt hatten |
| `agent check <name> --image <image> [--variable <NAME>]... [--timeout <s>]` | `grauwolf credentials check` für jede in der Konfiguration des Agenten zugeordnete Variable (`--variable` schränkt ein) als einen Job ausführen, dessen Log ausgeben (eine Zeile je Variable; bei `permission denied` ein Hinweis mit der Policy und dem versuchten Pfad; für einen mit `--identity` ausgerollten Agenten ein Hinweis, wenn der Identitätsdienst fehlt oder nicht bereit ist), den Job löschen; Exit-Code 1, wenn eine Variable fehlschlägt |
| `agent deploy <name> --image <image> [--spec <datei>] [--env NAME=VALUE]... [--host <host>] [--port <n>] [--mtls-port <n>] [--data-size <menge>] [--cpu <menge>] [--memory <menge>] [--pull-secret <ns>/<name>] [--otel [--otel-endpoint <url>] [--otel-content]] [--events] [--identity] [--egress [--egress-strict] [--cluster-cidr <cidr>]... [--egress-dns-namespace <ns>] [--egress-dns-selector <key=value>]] [--timeout <s>]` | Den Agenten ausrollen: Deployment (ein Replikat), Service; mit `--host` ein Ingress, mit `--spec` eine ConfigMap mit dem Agenten-Baum und, für jedes relative `builtin_tools.root` und `tools_dir` des Baums, ein Verzeichnis des Datenvolumes, das an dieser Stelle unter `/app/spec` eingehängt wird (von einem Init-Container angelegt; ein absoluter Pfad bleibt, `..` wird abgelehnt), mit `--data-size` eine PVC; immer mit eingeschaltetem SPIFFE: `GRAUWOLF_SPIFFE=1`, der mTLS-Port `--mtls-port` (Standard 8443) im Pod und im Service, die ConfigMap `grauwolf-callers` als Verzeichnis eingehängt (leer angelegt, falls sie fehlt, sonst behalten, nicht Teil des Neustart-Hashes) und die NetworkPolicy `grauwolf-mtls-callers`; legt das Server-Token an, falls es fehlt; mit `--otel` `spec serve --otel` (ein Image ohne `--spec`: `GRAUWOLF_OTEL=1`, mit `--otel-content` auch `GRAUWOLF_OTEL_CONTENT=1`) und `OTEL_EXPORTER_OTLP_ENDPOINT` (`--otel-endpoint`, Standard `http://lgtm.lgtm.svc:4318`) und `OTEL_SERVICE_NAME` (der Name des Agenten), `--otel-content` zeichnet Inhalte auf, ein `deploy` ohne `--otel` schaltet es wieder aus; mit `--events` `spec serve --events` (ein Image: `GRAUWOLF_EVENTS=1`): jedes Event als JSON-Zeile auf stderr, also im Pod-Log, ein `deploy` ohne `--events` schaltet es wieder aus; mit `--identity` setzt `GRAUWOLF_IDENTITY_URL` und `GRAUWOLF_IDENTITY_SPIFFE_ID` auf den Identitätsdienst von `identity install` (ohne ihn abgelehnt; mit `--egress` ein erlaubtes Ziel; siehe [Kapitel 14](14-im-auftrag-von-personen.md#im-cluster-mit-grauwolf-k8-ops)); `--egress` (braucht `--spec`) die NetworkPolicy `grauwolf-egress`, abgeleitet aus `spec egress` des Baums (DNS, Cluster-Services nach Namespace und Pod-Port, IPs als /32, externe Namen nur nach Port; `--egress-strict` lässt sie weg; ein Deploy ohne `--egress` löscht die Policy; siehe [Egress begrenzen](13-cluster-einrichten.md#begrenzen-was-ein-agent-erreichen-darf-egress)); wartet auf den Rollout und auf `/health` (Standard 300 s); schlägt das fehl, gibt er die letzten 20 Logzeilen aus und endet mit 1. Wiederholbar; startet den Pod nur neu, wenn sich etwas geändert hat, das er liest. Siehe [Kapitel 13](13-cluster-einrichten.md#einen-agenten-ausrollen) und [Traces in Tempo](13-cluster-einrichten.md#traces-in-tempo) |
| `agent status <name> [--json]` | Ausgerollt oder nicht, Pod (Phase, bereit, Neustarts, Wartegrund), Image, Endpunkt im Cluster, Host, `tracing: on (<endpoint>)` oder `off`, `events: on` oder `off`, `identity: on (<url>)` oder `off` (`"identity"` in JSON), `egress: <n> destinations, <m> only port-restricted` oder `egress: off` (in JSON `"egress"`), die beschreibbaren Ordner (`writable /app/spec/work …`; in JSON `"writable"`), ob das Server-Token existiert (nie dessen Wert) |
| `agent logs <name> [--follow] [--tail <n>] [--events-only]` | Das Log des Pods (Standard die letzten 100 Zeilen); `--follow` streamt; `--events-only` gibt nur die JSON-Event-Zeilen aus (siehe [Events im Pod-Log](13-cluster-einrichten.md#events-im-pod-log)) |
| `agent allow <target> --from <caller> [--as <user>]` | `<caller>` den Aufruf von `<target>` über den mTLS-Port erlauben: schreibt `spiffe://<trust domain>/ns/<caller namespace>/sa/<caller>` in die ConfigMap `grauwolf-callers` (Schlüssel `callers.json`) des Ziels, zugeordnet dem Benutzer `--as` (Standard: der Name des Aufrufers), und lässt die NetworkPolicy `grauwolf-mtls-callers` den Namespace des Aufrufers auf dem mTLS-Port zu (der HTTP-Port bleibt offen). Beide Agenten müssen existieren. Wiederholbar: meldet `created`, `updated` oder `unchanged`. Siehe [Kapitel 13](13-cluster-einrichten.md#agenten-sich-gegenseitig-aufrufen-lassen) |
| `agent deny <target> --from <caller>` | Zurücknehmen: entfernt den Eintrag und den Namespace aus der Policy (ohne Aufrufer gibt es keine Regel für den mTLS-Port); wiederholbar |
| `agent callers <target> [--json]` | Die Aufrufer-IDs mit ihren Benutzern, dem zugehörigen Agenten (oder `no such agent`), ob die NetworkPolicy ihren Namespace zulässt und ob die Policy `matches`, `differs` oder `missing` ist |
| `agent undeploy <name> [--purge-data] [--yes]` | Deployment, Service, Ingress, die ConfigMap der Spec und die NetworkPolicy `grauwolf-egress` löschen; Identität, Rolle, Geheimnisse, die Aufrufer (ConfigMap und NetworkPolicy) und die PVC bleiben; `--purge-data` löscht zusätzlich die PVC (fragt nach, außer mit `--yes`) |
| `identity install --tenant <id> --issuer <url> --audience <aud>... --image <image> [--pull-secret <ns>/<name>] [--jwks-url <url>] [--scope <scope>] [--public-url <url>] [--policy <datei>] [--transit-key <name>] [--port <n>] [--mtls-port <n>] [--cluster-cidr <cidr>]... [--egress-dns-namespace <ns>] [--egress-dns-selector <key=value>] [--grant-client-id <id>] [--timeout <s>]` | Der Identitätsdienst von ADR-0111 im Namespace `identity` (eingeschränkt): `ClusterSPIFFEID grauwolf-identity`, Service-Account, Deployment und Service `grauwolf-identity` (`grauwolf identity serve`, HTTP 8080, mTLS 8443), ConfigMap `grauwolf-identity-policy` (von `--policy` ersetzt, sonst behalten), NetworkPolicies `grauwolf-identity-ingress` (mTLS nur aus Agenten-Namespaces) und `grauwolf-egress` (DNS, OpenBao, der Schlüssel-Host); in OpenBao der Mount `transit/`, der Schlüssel (`ecdsa-p256`, Standard `grauwolf-identity`), Policy und JWT-Rolle `grauwolf-identity`. `--audience` darf mehrfach angegeben werden (zusammengefügt zu `GRAUWOLF_IDENTITY_AUDIENCE`). `--pull-secret` kopiert dieses Image-Pull-Secret in den Namespace `identity` und hängt es an den Service-Account, wie `agent deploy --pull-secret` es für einen Agenten tut (für ein Image aus einer Registry, die Zugangsdaten braucht). `--grant-client-id` (der öffentliche Client "grauwolf CLI") schaltet Grants ein: der Dienst bekommt `GRAUWOLF_IDENTITY_GRANT_CLIENT_ID` und den KV-Mount, seine OpenBao-Policy zusätzlich die Ordner `<kv>/identity/grants/` und `<kv>/identity/grant-tokens/`. Wartet auf das Rollout und `/health`. Siehe [Kapitel 14](14-im-auftrag-von-personen.md#im-cluster-mit-grauwolf-k8-ops) |
| `identity status [--json]` | Installiert oder nicht, bereit, Pod, Image, SPIFFE-ID, mTLS-Endpunkt, Schlüssel-URL, die Einstellungen, die Aufrufer der Richtlinie mit ihren Zwecken, den Grants-Abschnitt (Agenten und Auslöser) sowie Rolle, Policy und Transit-Schlüssel in OpenBao |
| `identity uninstall [--delete-key] [--yes]` | Namespace `identity`, die ClusterSPIFFEID sowie Rolle und Policy in OpenBao löschen; der Transit-Schlüssel bleibt ohne `--delete-key`; warnt vor Agenten, die mit `--identity` ausgerollt sind |
| `identity allow <agent> [--grants] [--purpose <zweck>]...` | Den Agenten Txn-Tokens im Auftrag von Personen anfordern lassen (ein Eintrag in `callers`; mit `--grants` in `grants.agents`: er darf auf Grants von Personen laufen; ohne `--purpose` jeder Zweck); wiederholbar |
| `identity allow --trigger <zeitplan oder spiffe id>` | Eine Workload Grant-Läufe starten lassen (`grants.triggers`; ein Name ist der Zeitplan `spiffe://<td>/ns/schedules/sa/<name>`, eine ID darf auf `/*` enden) |
| `identity grants [--revoke <grt_…>] [--json]` | Die Grants aller Personen aus OpenBao (Besitzer, Agent, Auslöser, Zwecke, Gültigkeit, letzte Nutzung; nie ein Refresh-Token), oder ein Grant widerrufen (Metadaten und Refresh-Token gelöscht). Braucht die Policy von `openbao setup` aus dieser Version (sie liest `<kv>/identity/grants/`) |
| `identity deny <agent> [--grants]`, `identity deny --trigger <zeitplan oder spiffe id>` | Den Agenten (aus `callers`, mit `--grants` aus `grants.agents`) oder den Auslöser aus der Richtlinie nehmen |
| `schedule add <name> --agent <agent> --grant <grt_…> --cron "<cron>" --input <text> --image <image> [--pull-secret <ns>/<name>]` | Ein Zeitplan (ADR-0111 Stufe 3): Namespace `schedules` (eingeschränkt), Service-Account `<name>` und `ClusterSPIFFEID grauwolf-schedule-<name>` (`spiffe://<td>/ns/schedules/sa/<name>`), NetworkPolicy `grauwolf-schedule-<name>` (Egress: nur DNS und der mTLS-Port des Agenten), der CronJob `<name>` (`grauwolf call https://<agent>.<ns>.svc:<mtls>/run --spiffe-id <agent-id> --grant … --input …`, das SPIFFE-CSI-Volume, ein Lauf zur Zeit) und der Zeitplan als Aufrufer des Agenten (Benutzer `schedule-<name>`, Aufruferdatei und NetworkPolicy). Sagt, was der Richtlinie des Identitätsdienstes noch fehlt (`identity allow <agent> --grants`, `identity allow --trigger <name>`). Wiederholbar. Siehe [Kapitel 14](14-im-auftrag-von-personen.md#unbeaufsichtigte-läufe-grants-und-zeitpläne) |
| `schedule list [--json]` | Die Zeitpläne: Cron-Zeile, Agent, Grant, letzter Lauf, letzter Erfolg |
| `schedule run-now <name>` | Ein Lauf sofort: ein Job `<name>-manual-<zeit>` aus dem CronJob |
| `schedule remove <name>` | CronJob, NetworkPolicy, ClusterSPIFFEID und Service-Account löschen und den Eintrag des Zeitplans aus den Aufrufern des Agenten nehmen; der Grant bleibt |
| `secret set <agent> <key> [--variable <NAME>]` | Einen Wert von stdin oder aus einer verdeckten Eingabe unter `kv/agents/<agent>/<key>` speichern |
| `secret get <agent> <key> --reveal` | Den Wert auf stdout ausgeben und sonst nichts (ein Zeilenumbruch folgt); ohne `--reveal` verweigert er. Der einzige Befehl, der einen Wert ausgibt |
| `secret list <agent> [--json]` | Schlüssel und Versionen, nie Werte |
| `secret delete <agent> <key>` | Ein Geheimnis mit allen seinen Versionen löschen |
| `trust-domain set <domain> [--apply] [--yes] [--backup <datei>] [--consumers <datei>] [--from <alte domain>] [--timeout <s>]` | Die Trust Domain von SPIRE und allem, was sie enthält, ändern. Ohne `--apply`: nur der Plan. Mit `--apply`: Bestätigung (`--yes` überspringt sie), Backup (Standard `trust-domain-backup-<UTC-Zeitstempel>.json`), Neuschreiben der Konfiguration, Neuanlegen der Daten des Servers, Warten auf das neue Bundle, Neustart der Agenten und des OIDC-Providers, OpenBao-Rollen, Verbraucher (`--consumers`: TOML mit `[[configmap]]` und `[[restart]]`), Prüfung; `--timeout` je Wartevorgang, Standard 300 s; `--from` beendet einen unterbrochenen Lauf. `<aktuelle domain>` gibt `already <domain>, nothing to do` aus. Siehe [Kapitel 13](13-cluster-einrichten.md#die-trust-domain-umstellen) |
| `trust-domain restore <backup> [--apply] [--yes] [--backup <datei>] [--timeout <s>]` | Den Cluster so zurücksetzen, wie ihn ein Backup von `trust-domain set` (oder eines früheren Restore) festhält: die `from`-Domain des Backups. Ohne `--apply`: nur der Plan (zeilenweiser Diff der ConfigMap-`data`, Rollen, Neustarts; andere geänderte Zeilen werden getrennt aufgeführt). Mit `--apply`: Bestätigung (`--yes` überspringt sie), Backup des aktuellen Zustands (Standard `trust-domain-backup-<UTC-Zeitstempel>.json`), ConfigMap-`data`, Neuanlegen der Daten des Servers, falls seine CA die Domain nicht enthält, Warten auf das Bundle, Neustarts, OpenBao-Rollen der Agenten, die es noch gibt, Prüfung. Ist er schon gleich, meldet er `already as in the backup, nothing to do` und legt kein Backup an. Siehe [Kapitel 13](13-cluster-einrichten.md#aus-einem-backup-wiederherstellen) |

Globale Optionen (alle Befehle):

| Option | Standard | Bedeutung |
|---|---|---|
| `--kubeconfig <datei>` | `KUBECONFIG`, `~/.kube/config` | Kubeconfig |
| `--context <name>` | der aktuelle Kontext der kubeconfig | Kontext der kubeconfig; ein nicht vorhandener ist ein Fehler, der die verfügbaren auflistet. `doctor` gibt die Server-URL des Clusters und den benutzten Kontext als erste Zeile aus |
| `--spire-namespace <ns>` | `spire-system` | SPIRE-Server und -Agenten |
| `--oidc-namespace <ns>` | `spire-oidc` | OIDC-Discovery-Provider |
| `--openbao-namespace <ns>`, `--openbao-service <name:port>` | `openbao`, `openbao:8200` | OpenBao, erreicht über den Service-Proxy des API-Servers |
| `--openbao-addr <url>` | – | OpenBao stattdessen direkt erreichen |
| `--token-file <datei>` | Umgebungsvariable `OPENBAO_TOKEN` | Das OpenBao-Token (nie als Argument) |
| `--jwt-mount <mount>`, `--kv-mount <mount>` | `jwt`, `kv` | OpenBao-Mounts |
| `--audience <audience>` | `openbao` | Audience der JWT-SVIDs der Agenten |
| `--agent-openbao-address <url>` | `http://<service>.<namespace>.svc:<port>` | OpenBao, so wie die Agenten es sehen (wird in ihre ConfigMap geschrieben) |

## Build-Optionen

Für alle Befehle, die Tool-Code bauen: `build`, `run`, `chat`, `serve`, `mcp`, `resume`,
`generate`, `create`.

| Option | Standard | Bedeutung |
|---|---|---|
| `--cache <verz>` | `.grauwolf/tools` | Cache-Verzeichnis für Gast-Crates und gebaute Artefakte (relativ zum Arbeitsverzeichnis) |
| `--locked` | aus | Tool-Pakete müssen in `grauwolf.lock` stehen; schlägt fehl, statt die Lock-Datei zu ändern, auch bei einem Eintrag, den kein Agent des Baums benutzt (CI, Images). `tools lock` hat dieselbe Option |
| `--build-wrapper <teil>` | – | Präfix, unter dem `cargo` läuft (auch `tools lock` und `tools pull` eines Git-Pakets: sie bauen) (z. B. bwrap oder docker); mehrfach möglich, je Teil einmal: `--build-wrapper bwrap --build-wrapper --unshare-net` |

`tools pull`, `tools inspect`, `tools pack`, `tools push` und `tools lock` nehmen `--cache <verz>` (Standard `.grauwolf/tools`); die
Pakete werden in dessen Ordner `packages/` zwischengespeichert, geschlüsselt nach Digest.

## Lauf-Optionen

Für `run`, `chat`, `serve`, `mcp` und `resume`; immer zusammen mit den Build-Optionen.

| Option | Standard | Bedeutung | Kapitel |
|---|---|---|---|
| `--stream` | aus | Text von Streaming-Nodes (`llm_request_streaming`) sofort ausgeben | [4](04-ablaeufe.md) |
| `--events` | aus | Alle Events des Laufs als JSON-Zeilen nach stderr | [9](09-betrieb.md) |
| `--prices <datei>` | – | Preistabelle `{ "currency", "models": { "<anbieter>:<id>": { "input", "output", … } } }` je Million Tokens; Events und Spans nennen die Kosten | [2](02-modelle.md) |
| `--max-tokens <n>` | – | Lauf beenden, sobald er so viele Eingabe- und Ausgabe-Tokens verbraucht hat | [2](02-modelle.md) |
| `--max-cost <betrag>` | – | Lauf beenden, sobald er diese Kosten erreicht hat; braucht `--prices` | [2](02-modelle.md) |
| `--conversation <id>` | – | Gespräch mit dieser ID fortsetzen und speichern | [7](07-gespraeche-und-wissen.md) |
| `--conversations-dir <verz>` | `.grauwolf/conversations` | Verzeichnis der Gesprächsverläufe | [7](07-gespraeche-und-wissen.md) |
| `--conversations-db <url>` | – | Datenbank statt Verzeichnis, z. B. `sqlite://chats.db` oder `postgres://nutzer@host/db` | [7](07-gespraeche-und-wissen.md) |
| `--llm-cache <verz>` | – | Gleiche LLM-Anfragen aus Dateien in diesem Verzeichnis beantworten statt beim Anbieter (0 Tokens) | [2](02-modelle.md) |
| `--runs-dir <verz>` | `.grauwolf/runs` | Verzeichnis der Checkpoints | [9](09-betrieb.md) |
| `--runs-db <url>` | – | Datenbank der Checkpoints statt des Verzeichnisses, z. B. `sqlite://runs.db` | [9](09-betrieb.md) |
| `--otel` | aus | Spans über OTLP/HTTP exportieren (Ziel aus `OTEL_EXPORTER_OTLP_*`) | [9](09-betrieb.md) |
| `--otel-content` | aus | Auch Inhalte in den Spans aufzeichnen; nur zusammen mit `--otel` | [9](09-betrieb.md) |
| `--confirm-tools <a,b>` | – | Vor jedem Aufruf dieser Tools nachfragen (kommagetrennt); im Dienst über `/run/stream` | [9](09-betrieb.md) |

### Welche Lauf-Option wirkt wo?

Nicht jede Lauf-Option wirkt bei jedem Befehl. Setzt du eine, die keine Wirkung hätte, bricht der
Befehl ab, statt sie still zu übergehen:

| Option | `run` | `chat` | `serve` | `mcp` | `resume` |
|---|---|---|---|---|---|
| `--stream` | ja | Fehler | Fehler | Fehler | ja |
| `--events`, `--prices`, `--max-tokens`, `--max-cost`, `--llm-cache` | ja | ja (je Zeile) | ja (je Anfrage) | ja (je Aufruf) | ja |
| `--conversation` | ja | ja (sonst Gespräch nur im Arbeitsspeicher) | Fehler (je Anfrage `conversation_id`) | Fehler | Fehler |
| `--conversations-dir`, `--conversations-db` | mit `--conversation` | mit `--conversation` | ja | Fehler | Fehler |
| `--runs-dir`, `--runs-db` | mit `--run-id` | Fehler | Fehler | Fehler | ja |
| `--otel`, `--otel-content` | ja | Fehler | ja | ja | ja |
| `--confirm-tools` | ja (Konsole) | ja (Konsole) | ja (`/run/stream`) | Fehler | ja (Konsole) |

„mit …“ heißt: nur zusammen mit dieser Option, sonst Fehler. Die Meldung nennt alle wirkungslosen
Optionen auf einmal, z. B.

```text
keine Wirkung bei spec serve: --stream, --conversation
keine Wirkung bei spec run: --runs-dir (nur mit --run-id)
```

Dazu kommen eigene Meldungen wie `--stream und --otel sind in spec chat noch nicht verfügbar` oder
`--confirm-tools liest stdin und passt nicht zum stdio-Transport von MCP`.

### Unverträgliche Kombinationen bei `spec run`

| Kombination | Meldung |
|---|---|
| `--run-id` + `--conversation` | `--run-id and --conversation cannot be combined` |
| `--user` ohne `long_term_memory` und ohne `--conversation` | `--user needs a long_term_memory in the spec or --conversation` (bei `spec resume`: `--user needs a long_term_memory in the spec`) |
| `--max-cost` ohne `--prices` | `--max-cost needs --prices: without a price table the cost is unknown` |

## Endpunkte von `spec serve`

| Methode und Pfad | Zweck | Token nötig? |
|---|---|---|
| `GET /health` | Antwortet `ok` | nein |
| `POST /run` | `{ input, conversation_id?, user_id? }` → `{ output }` bzw. Status 500 mit `{ error }` | ja* |
| `POST /run/stream` | wie `/run`, Antwort als SSE: `agent`, `text`, `approval`, zum Schluss `output` oder `error` | ja* |
| `POST /approvals/{id}` | `{ approved, reason?, user_id? }` → 204; unbekannt oder fremder Benutzer → 404 | ja* |
| `GET /.well-known/agent-card.json` | A2A-Agent-Karte | nein |
| `POST /a2a` | A2A 1.0 über JSON-RPC: `SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask` | ja* |
| `POST /mcp` | MCP über Streamable HTTP; der Agent als ein Tool mit dem Namen der Spec | ja* |

\* nur wenn `GRAUWOLF_SERVER_TOKEN` gesetzt oder `--users` angegeben ist. Dann `401` ohne gültiges
Token und `403`, wenn die `user_id` im Körper nicht zum Benutzer-Token passt.

## `grauwolf token`

```sh
grauwolf token <benutzer>
```

Erste Zeile: das neue Token (`gw_` und 32 Hex-Zeichen) – nur jetzt sichtbar. Zweite Zeile:
`"<benutzer>": "sha256:<hex>"` für die Benutzerdatei von `spec serve --users`.

## `grauwolf login`

```sh
grauwolf login [--tenant <id>] [--authority <url>] [--client-id <id>] [--scope <scope>] [--config-dir <verz>]
grauwolf login --status [--config-dir <verz>]
grauwolf login --print-token [--config-dir <verz>]
grauwolf logout [--config-dir <verz>]
```

Meldet eine Person über den Device-Code-Flow (RFC 8628) an `<authority>/<tenant>/oauth2/v2.0/devicecode`
und `…/token` an (Entra ID und Entra Local) und speichert Access- und Refresh-Token in
`<config dir>/login.json` (Modus 0600). `offline_access` wird an `--scope` angehängt.

| Option | Standard | Bedeutung |
|---|---|---|
| `--tenant <id>` | `GRAUWOLF_LOGIN_TENANT`, sonst die letzte Anmeldung | Der Tenant |
| `--authority <url>` | `GRAUWOLF_LOGIN_AUTHORITY`, die letzte Anmeldung, `https://login.microsoftonline.com` | Entra ID oder die Adresse von Entra Local |
| `--client-id <id>` | `GRAUWOLF_LOGIN_CLIENT_ID`, sonst die letzte Anmeldung | Der öffentliche Client ("grauwolf CLI") |
| `--scope <scope>` | `GRAUWOLF_LOGIN_SCOPE`, sonst die letzte Anmeldung | Der API-Scope, z. B. `api://grauwolf-identity/access_as_user` |
| `--identity-url <url>` | `GRAUWOLF_LOGIN_IDENTITY_URL`, sonst die letzte Anmeldung | Die öffentliche URL des Identitätsdienstes, aufbewahrt für `grauwolf identity grant` |
| `--config-dir <verz>` | `GRAUWOLF_CONFIG_DIR`, `$XDG_CONFIG_HOME/grauwolf`, `~/.config/grauwolf` | Wo `login.json` liegt |
| `--status` | – | Wer angemeldet ist und bis wann; Exit-Code 1, wenn niemand angemeldet ist oder die Anmeldung ohne Refresh-Token abgelaufen ist |
| `--print-token` | – | Nur das Access-Token auf stdout (vorher erneuert, wenn weniger als 60 s übrig sind), für `Authorization: Bearer` |

Keine andere Ausgabe zeigt ein Token. Siehe [Kapitel 14](14-im-auftrag-von-personen.md#anmelden-auf-der-kommandozeile-grauwolf-login).

## `grauwolf credentials check`

```
grauwolf credentials check <VAR>… [--config <datei>]
grauwolf credentials check --all [--config <datei>]
```

Löst die Variablen `<VAR>…` (mit `--all` jede im Abschnitt `[credentials]` zugeordnete Variable) so auf, wie es ein Lauf täte: über den Abschnitt `[credentials]` von `--config`, sonst von `GRAUWOLF_CREDENTIALS`, sonst von `./grauwolf.toml`, und gibt je Variable eine Zeile aus: `resolved <VAR> from <file|openbao|environment> (N bytes)` auf stdout oder den Fehler, der die Variable nennt, auf stderr. Eine fehlschlagende Variable verdeckt die anderen nicht. Der Wert wird nie ausgegeben. Exit-Code 1, wenn eine Variable fehlschlägt (ihre Quelle kann nicht gelesen werden oder sie ist weder zugeordnet noch gesetzt). Siehe [Zugangsdaten aus OpenBao mit SPIFFE](12-zugangsdaten-aus-openbao.md).

## `grauwolf identity serve`

```
grauwolf identity serve [--port <n>] [--mtls-port <n>]
```

Startet den Identitätsdienst (ADR-0111) am HTTP-Port (`--port`, sonst `SERVER_PORT`, Standard
8080) und am mTLS-Port (`--mtls-port`, sonst `GRAUWOLF_SPIFFE_PORT`, Standard 8443):
`POST /api/identity/token` (Aufrufer mit Client-Zertifikat oder einem JWT-SVID für seine eigene
SPIFFE-ID oder `GRAUWOLF_SPIFFE_AUDIENCE`), `GET /api/identity/jwks.json`,
`GET /api/identity/config` und `GET /health` (öffentlich). Braucht `SPIFFE_ENDPOINT_SOCKET`, die
`GRAUWOLF_IDENTITY_*`-Variablen unten und einen Transit-Schlüssel in OpenBao. Mit
`GRAUWOLF_IDENTITY_GRANT_CLIENT_ID` bedient er auch Grants (Stufe 3): `POST /api/identity/grants`,
`GET /api/identity/grants` und `DELETE /api/identity/grants/{id}` (Aufrufer: Personen mit ihrem
Entra-Access-Token als Bearer), abgelegt in OpenBao KV. Siehe
[Im Auftrag von Personen handeln](14-im-auftrag-von-personen.md).

## `grauwolf identity grant`

```
grauwolf identity grant create --agent <spiffe id> --trigger <spiffe id> [--purpose <p>]… [--valid 30d] [--max-run 4h] [--identity-url <url>] [--config-dir <verz>]
grauwolf identity grant list [--json] [--identity-url <url>] [--config-dir <verz>]
grauwolf identity grant revoke <grt_…> [--identity-url <url>] [--config-dir <verz>]
```

Die Grants einer Person für unbeaufsichtigte Läufe (ADR-0111 Stufe 3), autorisiert durch die
zwischengespeicherte Anmeldung von `grauwolf login`. `create` führt eine eigene Device-Code-Anmeldung
mit `offline_access` durch (derselbe Client und Scope wie beim Login, der Login-Cache bleibt
unberührt), schickt das Refresh-Token dieser Anmeldung einmal an den Identitätsdienst und behält keine
Kopie; der Dienst löst es einmal ein, um zu prüfen, dass es deines ist.

| Option | Standard | Bedeutung |
|---|---|---|
| `--agent <spiffe id>` | – | Der Einstiegsagent, für den der Grant gilt |
| `--trigger <spiffe id>` | – | Die Workload, die Läufe starten darf, z. B. `spiffe://<td>/ns/schedules/sa/morning` |
| `--purpose <p>` | keiner | Ein Zweck der Läufe (wiederholbar); wird zu `purp` des Txn-Token |
| `--valid <dauer>` | `30d` | Wie lange der Grant gilt, höchstens `90d` (`d`, `h`, `m`, `s`) |
| `--max-run <dauer>` | `4h` | Wie lange ein Lauf dauern darf (die maximale Lebensdauer seiner Kette), höchstens die 8 h des Dienstes |
| `--identity-url <url>` | `GRAUWOLF_LOGIN_IDENTITY_URL`, sonst `grauwolf login --identity-url` | Die öffentliche URL des Identitätsdienstes |
| `--json` (`list`) | – | JSON ausgeben |

`list` zeigt deine Grants (nie ein Token), `revoke` löscht Metadaten und Refresh-Token eines Grants
auf einmal. Exit-Code 1, wenn der Dienst ablehnt (der Grund wird ausgegeben). Siehe
[Unbeaufsichtigte Läufe](14-im-auftrag-von-personen.md#unbeaufsichtigte-läufe-grants-und-zeitpläne).

## `grauwolf call`

```
grauwolf call <url> [--spiffe-id <id>] [--grant <grt_…>] [--input <text> | stdin] [--json] [--token <t>] [--timeout <s>]
```

Schickt `{"input": …}` an das `/run` eines Agenten (an eine bloße Adresse wird `/run` angehängt) und
gibt `output` aus (Text unverändert, alles andere als JSON). Mit `SPIFFE_ENDPOINT_SOCKET` meldet es
sich mit der Workload-Identität an: über `https://` mit mTLS (der Server muss `--spiffe-id` sein,
sonst eine beliebige ID der eigenen Trust Domain), über `http://` mit einem JWT-SVID für die Audience
`--spiffe-id` (dort Pflicht). `--token` schickt stattdessen ein Bearer-Token. `--grant` schickt
`Grauwolf-Grant`. `--json` schickt die Eingabe als JSON statt als Text; `--timeout` (Standard 600 s)
begrenzt das Warten. Exit-Codes: 0 der Lauf hat geantwortet, 1 der Aufruf ist fehlgeschlagen
(unerreichbar, Fehler des Agenten, jeder andere HTTP-Status), 3 abgelehnt (401 oder 403).

## Umgebungsvariablen

grauwolf liest jede dieser Variablen an einer einzigen Stelle (`config::VARIABLES`, siehe Entwicklerhandbuch,
Kapitel 3); ein Dienst, der aus einer erzeugten Crate startet, und die CLI nutzen dieselben Namen.

| Variable | Wirkung | Kapitel |
|---|---|---|
| `OLLAMA_HOST` | Adresse von Ollama; Standard `http://localhost:11434` | [2](02-modelle.md) |
| `OLLAMA_THINK` | `true`/`1` oder `false`/`0`: Denkmodus von Ollama-Modellen ein- oder ausschalten | [2](02-modelle.md) |
| `OPENAI_API_KEY` | Schlüssel für den Anbieter `openai` (Pflicht für ihn) | [2](02-modelle.md) |
| `OPENAI_BASE_URL` | Andere Basis-URL für `openai`, z. B. ein kompatibler Dienst | [2](02-modelle.md) |
| `ANTHROPIC_API_KEY` | Schlüssel für den Anbieter `anthropic` (Pflicht für ihn) | [2](02-modelle.md) |
| `GEMINI_API_KEY` | Schlüssel für den Anbieter `gemini` (Pflicht für ihn) | [2](02-modelle.md#google-gemini) |
| `GEMINI_BASE_URL` | Andere Basis-URL für `gemini`; Standard `https://generativelanguage.googleapis.com` | [2](02-modelle.md#google-gemini) |
| `AWS_ACCESS_KEY_ID` | Zugriffsschlüssel des Anbieters `bedrock`; braucht `AWS_SECRET_ACCESS_KEY` | [2](02-modelle.md#amazon-bedrock) |
| `AWS_SECRET_ACCESS_KEY` | Geheimer Schlüssel des Anbieters `bedrock` | [2](02-modelle.md#amazon-bedrock) |
| `AWS_SESSION_TOKEN` | Session-Token für temporäre AWS-Zugangsdaten (optional) | [2](02-modelle.md#amazon-bedrock) |
| `AWS_REGION` | Region des Anbieters `bedrock`; Standard `us-west-2` | [2](02-modelle.md#amazon-bedrock) |
| `AWS_DEFAULT_REGION` | Region von `bedrock`, wenn `AWS_REGION` nicht gesetzt ist | [2](02-modelle.md#amazon-bedrock) |
| `AWS_BEARER_TOKEN_BEDROCK` | Bedrock-API-Schlüssel; ersetzt die Zugriffsschlüssel und gewinnt, wenn beides gesetzt ist | [2](02-modelle.md#amazon-bedrock) |
| `BEDROCK_BASE_URL` | Andere Basis-URL für `bedrock` (Tests, VPC-Endpunkte, Proxies); Standard `https://bedrock-runtime.<region>.amazonaws.com` | [2](02-modelle.md#amazon-bedrock) |
| `<NAME>_API_KEY` | Schlüssel für einen OpenAI-kompatiblen Anbieter (Pflicht für ihn): `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`, `MISTRAL_API_KEY`, `GROQ_API_KEY`, `DASHSCOPE_API_KEY`, `XAI_API_KEY` | [2](02-modelle.md#openai-und-kompatible-dienste) |
| `<NAME>_BASE_URL` | Andere Basis-URL für diesen Anbieter: `OPENROUTER_BASE_URL`, `DEEPSEEK_BASE_URL`, `MISTRAL_BASE_URL`, `GROQ_BASE_URL`, `DASHSCOPE_BASE_URL` (z. B. für eine andere Region), `XAI_BASE_URL` | [2](02-modelle.md#openai-und-kompatible-dienste) |
| `RKEV_BASE_URL` | Adresse des System-One-Dienstes (z. B. RKEV) für Entscheidungen mit `decider: system_one`; ohne sie startet eine solche Spec nicht | [4](04-ablaeufe.md#entscheidungen-verzweigen-nach-einer-frage) |
| `RKEV_API_KEY` | Optionaler Schlüssel für diesen Dienst (wird als `Authorization: Bearer …` gesendet) | [4](04-ablaeufe.md#entscheidungen-verzweigen-nach-einer-frage) |
| `ATLASSIAN_EMAIL`, `ATLASSIAN_API_TOKEN` | E-Mail-Adresse und API-Token für die eingebauten Tools `jira_*` und `confluence_*`; ohne sie startet eine solche Spec nicht | [3](03-tools.md#jira-confluence-und-git) |
| `GIT_TOKEN` | Zugriffstoken für die `git_*`-Tools mit `git.use_token: true` (Benutzer `x-access-token`; ohne es startet eine solche Spec nicht) und für `tool_packages` mit Git-Quellen (optional, ohne es anonym) | [3](03-tools.md#jira-confluence-und-git) |
| `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` | Autor der Commits von `git_commit`; Standard `grauwolf agent`, `agent@grauwolf.local` | [3](03-tools.md#jira-confluence-und-git) |
| `GRAUWOLF_REGISTRY_AUTH` | Zugangsdaten für Tool-Paket-Registries, eine JSON-Map Registry → `"user:token"` (Basis-Zugangsdaten, auch für die Token-Anfrage) oder ein nacktes Bearer-Token; ohne Eintrag wird die Docker-Konfiguration befragt (siehe `DOCKER_CONFIG`), danach werden Pakete anonym geholt | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `GRAUWOLF_TOOL_POLICY` | Pfad zur Richtliniendatei für Tool-Pakete (erlaubte Registries und Git-Hosts, vorgeschriebene Signaturen; gleiches Format wie `grauwolf.toml`, Tabelle `[tool_packages]`). Sie hat Vorrang vor einer `grauwolf.toml` neben der Spec; ohne beides ist alles erlaubt und keine Signatur nötig. Gilt für jeden Befehl, der Tool-Pakete auflöst, und für `tools pull` | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `GRAUWOLF_TOOLS_CATALOG` | Der Tool-Katalog von `grauwolf-tools catalog …` und von `push`: eine Registry (`registry.example.com` = `registry.example.com/grauwolf-catalog:latest`) oder eine volle Referenz `registry/repository[:tag]`. `--catalog` hat Vorrang; ohne beides nehmen `catalog add` und `push` den Katalog der Registry des Pakets | [15](15-der-tool-katalog.md#welcher-katalog) |
| `SOURCE_DATE_EPOCH` | Build-Datum eines gepackten Tool-Pakets (`tools pack`, `tools push` eines Ordners) in Sekunden seit 1970, für reproduzierbare Builds; Standard: jetzt | [15](15-der-tool-katalog.md#was-ein-paket-über-sich-sagt) |
| `GRAUWOLF_CREDENTIALS` | Pfad zu einer Datei mit dem Abschnitt `[credentials]`, der Variablen auf Quellen für Zugangsdaten abbildet (`file:`, später `openbao:`); gleiches Format wie `grauwolf.toml`. Sie hat Vorrang vor einer `grauwolf.toml` neben der Spec. Eine zugeordnete Variable darf nicht zusätzlich in der Umgebung gesetzt sein | – |
| `SPIFFE_ENDPOINT_SOCKET` | Socket der SPIFFE-Workload-API des SPIRE-Agenten (`unix:///run/spire/sockets/agent.sock`). In Kubernetes setzt ihn das Volume des SPIFFE-CSI-Treibers. Die JWT-SVID des Agenten, mit der sich Quellen für Zugangsdaten bei OpenBao anmelden, kommt von dort; ohne sie ändert sich nichts | – |
| `SPIFFE_FETCH_TIMEOUT` | Wie lange beim Start auf die SPIFFE-Identität des Pods gewartet wird (`250ms`, `30s`, `1m`; Standard `30s`; `0s` versucht es einmal). Ein neuer Pod startet oft, bevor SPIRE seinen Registrierungseintrag synchronisiert hat; die Workload-API antwortet dann `no identity issued`, und grauwolf versucht es mit wachsender Pause, bis die Frist abläuft | [12](12-zugangsdaten-aus-openbao.md) |
| `DOCKER_CONFIG` | Verzeichnis der Docker-`config.json`, auf die Tool-Paket-Registries hinter `GRAUWOLF_REGISTRY_AUTH` zurückgreifen (Standard `~/.docker`): `auths` (`auth`, `identitytoken`), `credHelpers` und `credsStore`; Helfer laufen als `docker-credential-<name> get` und werden in `PATH` gesucht; nur befragt, wenn eine Registry Zugangsdaten verlangt | [3](03-tools.md#tool-pakete-aus-einer-registry-tool_packages) |
| `GRAUWOLF_SERVER_TOKEN` | Gemeinsames Token für `spec serve` und für erzeugte Dienste | [9](09-betrieb.md) |
| `SERVER_PORT`, `A2A_PORT`, `A2A_BASE_PATH` | Port, zusätzlicher A2A-Port und dessen Pfadpräfix eines erzeugten Dienstes (`--service`) | [11](11-code-erzeugen.md#als-http-dienst---service) |
| `GRAUWOLF_USERS_FILE`, `GRAUWOLF_ALLOW_ANONYMOUS`, `GRAUWOLF_DATA_DIR`, `GRAUWOLF_PUBLIC_URL`, `GRAUWOLF_A2A_PUBLIC_URL` | Benutzerdatei, anonymer Zugriff, Basisverzeichnis und öffentliche URLs eines erzeugten Dienstes | [11](11-code-erzeugen.md#als-http-dienst---service) |
| `GRAUWOLF_SPIFFE`, `GRAUWOLF_SPIFFE_PORT`, `GRAUWOLF_SPIFFE_USERS`, `GRAUWOLF_SPIFFE_AUDIENCE`, `GRAUWOLF_SPIFFE_TRUST_DOMAINS`, `GRAUWOLF_SPIFFE_PUBLIC_URL` | SPIFFE-Aufrufer (ADR-0109, [Kapitel 9](09-betrieb.md#agenten-rufen-agenten-mit-spiffe)): `1`/`true` schaltet den mTLS-Port (`GRAUWOLF_SPIFFE_PORT`, Standard `8443`) und JWT-SVIDs am HTTP-Port ein; `GRAUWOLF_SPIFFE_USERS` ist die Datei, die SPIFFE-IDs auf Benutzer abbildet; `GRAUWOLF_SPIFFE_AUDIENCE` die Audience der JWT-SVIDs (Standard die eigene SPIFFE-ID); `GRAUWOLF_SPIFFE_TRUST_DOMAINS` weitere akzeptierte Trust-Domains (für die Föderation reserviert); `GRAUWOLF_SPIFFE_PUBLIC_URL` die URL des mTLS-Ports, die seine A2A-Agent-Card nennt (Standard `https://localhost:<port>`). `spec serve` hat die Optionen `--spiffe`, `--spiffe-port`, `--spiffe-users` |
| `GRAUWOLF_IDENTITY_URL`, `GRAUWOLF_IDENTITY_SPIFFE_ID` | Delegation in einem Agenten (ADR-0111, [Kapitel 14](14-im-auftrag-von-personen.md)): der Identitätsdienst, bei dem ein Entra-Bearer-Token gegen ein Txn-Token getauscht wird und dessen Schlüssel den `Txn-Token`-Header von Aufrufern prüfen; braucht `GRAUWOLF_SPIFFE`. `GRAUWOLF_IDENTITY_SPIFFE_ID` ist die SPIFFE-ID des Dienstes: über `https://` geprüft (mTLS; Standard jede ID der eigenen Trust Domain), über `http://` die Audience des JWT-SVID (dort Pflicht) |
| `GRAUWOLF_IDENTITY_TENANT`, `GRAUWOLF_IDENTITY_ISSUER`, `GRAUWOLF_IDENTITY_AUDIENCE`, `GRAUWOLF_IDENTITY_SCOPE`, `GRAUWOLF_IDENTITY_JWKS_URL` | `grauwolf identity serve`: was Entra-Token von Personen nennen müssen – Tenant (`tid`), Issuer (`iss`), Audience (`aud`, die grauwolf-Identitäts-API: eine kommagetrennte Liste, z. B. ihre Application ID und ihre `api://…`-URI; ein Token gilt, wenn sein `aud` eines davon ist), Scope in `scp` (Standard `access_as_user`); die JWKS-URL (Standard aus der OpenID-Discovery des Issuers) | [14](14-im-auftrag-von-personen.md) |
| `GRAUWOLF_IDENTITY_PUBLIC_URL`, `GRAUWOLF_IDENTITY_TRUST_DOMAIN`, `GRAUWOLF_IDENTITY_POLICY` | `grauwolf identity serve`: die eigene URL (`iss` der Txn-Token), die Trust Domain (`aud`; Standard die seiner SPIFFE-ID), die Policy-Datei (welche Aufrufer Token im Auftrag von Personen anfordern dürfen, für welche Zwecke; wird bei Änderung neu gelesen) | [14](14-im-auftrag-von-personen.md) |
| `GRAUWOLF_IDENTITY_TRANSIT_KEY`, `GRAUWOLF_IDENTITY_TRANSIT_MOUNT`, `GRAUWOLF_IDENTITY_OPENBAO_ADDR`, `GRAUWOLF_IDENTITY_OPENBAO_ROLE`, `GRAUWOLF_IDENTITY_OPENBAO_AUDIENCE`, `GRAUWOLF_IDENTITY_OPENBAO_MOUNT` | `grauwolf identity serve`: der Transit-Schlüssel in OpenBao (`ecdsa-p256`), der die Txn-Token signiert, und sein Mount (Standard `transit`); die Adresse von OpenBao und die JWT-Anmeldung mit dem JWT-SVID des Dienstes (Rolle; Audience, Standard `openbao`; Mount, Standard `jwt`) | [14](14-im-auftrag-von-personen.md) |
| `GRAUWOLF_IDENTITY_GRANT_CLIENT_ID`, `GRAUWOLF_IDENTITY_GRANT_TOKEN_URL`, `GRAUWOLF_IDENTITY_GRANT_SCOPE`, `GRAUWOLF_IDENTITY_GRANT_KV_MOUNT` | `grauwolf identity serve`, Grants: der öffentliche Client, dessen Refresh-Token die Grants halten (ohne ihn hat der Dienst keine Grants), der Token-Endpunkt von Entra (Standard: aus der OpenID-Discovery des Issuers), der Scope der Einlösung (Standard `<Audience-URI>/<Scope> offline_access`) und der KV-v2-Mount der Grants (Standard `kv`) | [14](14-im-auftrag-von-personen.md) |
| `GRAUWOLF_LOGIN_TENANT`, `GRAUWOLF_LOGIN_AUTHORITY`, `GRAUWOLF_LOGIN_CLIENT_ID`, `GRAUWOLF_LOGIN_SCOPE`, `GRAUWOLF_LOGIN_IDENTITY_URL` | Standardwerte von `grauwolf login` (die Flags gewinnen, die letzte Anmeldung ist der Rückfall); die Identitäts-URL liest auch `grauwolf identity grant` | [14](14-im-auftrag-von-personen.md#anmelden-auf-der-kommandozeile-grauwolf-login) |
| `GRAUWOLF_CONFIG_DIR` | Verzeichnis des Token-Caches von `grauwolf login` (Standard `$XDG_CONFIG_HOME/grauwolf`, `~/.config/grauwolf`) | [14](14-im-auftrag-von-personen.md#anmelden-auf-der-kommandozeile-grauwolf-login) |
| `GRAUWOLF_CONVERSATIONS_DIR`, `GRAUWOLF_CONVERSATIONS_DB` | Wo ein erzeugter Dienst seine Gespräche (`conversation_id`) hält: ein Verzeichnis (relativ zu `GRAUWOLF_DATA_DIR`) oder eine Datenbank-URL (`sqlite://…`, `postgres://…`; wird nie ausgegeben, braucht das Facade-Feature `sql`) statt des Gedächtnisses des Prozesses; beide zu setzen ist ein Fehler beim Start (wie `spec serve --conversations-dir` / `--conversations-db`) | [11](11-code-erzeugen.md#als-http-dienst---service) |
| `GRAUWOLF_PRICES` | Pfad der Preistabelle eines erzeugten Agenten (die Datei von `--prices`), relativ zu `GRAUWOLF_DATA_DIR` bzw. zum Basisverzeichnis; Events tragen dann `cost`, und `limits.max_cost` wirkt | [11](11-code-erzeugen.md) |
| `GRAUWOLF_CHAT_API` | `1` oder `true` schaltet `/api/chat` und `/api/signature` in einem erzeugten Dienst ein (wie `spec serve --chat-api`) | [9](09-betrieb.md#die-chat-api-apichat-und-apisignature) |
| `GRAUWOLF_OTEL`, `GRAUWOLF_OTEL_CONTENT` | `1` oder `true` lässt einen erzeugten Dienst seine Spans per OTLP/HTTP exportieren (wie `spec serve --otel`; Ziel und Dienstname aus `OTEL_EXPORTER_OTLP_*` und `OTEL_SERVICE_NAME`); `GRAUWOLF_OTEL_CONTENT=1` zeichnet auch Inhalte auf (wie `--otel-content`). `0`/`false`/leer ist aus, jeder andere Wert ist ein Startfehler, der die Variable nennt; ein Build ohne das Facade-Feature `otel` lehnt `1` mit `this build has no OpenTelemetry (feature `otel`)` ab. Ein eingehender `traceparent` wird fortgesetzt (ADR-0123) | [9](09-betrieb.md#opentelemetry), [13](13-cluster-einrichten.md#traces-in-tempo) |
| `GRAUWOLF_EVENTS` | `1` oder `true` lässt einen erzeugten Dienst jedes Event seiner Läufe (und seiner Unteragenten) als eine JSON-Zeile auf stderr schreiben, wie `spec serve --events`, sodass die Events im Pod-Log stehen. `0`/`false`/leer ist aus, jeder andere Wert ist ein Startfehler, der die Variable nennt | [9](09-betrieb.md#ereignisse-als-json-zeilen), [13](13-cluster-einrichten.md#events-im-pod-log) |
| `AGENT_SIGNATURE_PATH` | Signaturdatei für `/api/signature`; Standard `agent-signature.json` im Basisverzeichnis | [9](09-betrieb.md#die-chat-api-apichat-und-apisignature) |
| `OLLAMA_BASE_URL` | Im Chat-API-Modus: Adresse des Ollama-Servers, wenn `OLLAMA_HOST` nicht gesetzt ist | [9](09-betrieb.md#die-chat-api-apichat-und-apisignature) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Ziel des OpenTelemetry-Exports (`--otel`) | [9](09-betrieb.md) |
| `OTEL_EXPORTER_OTLP_HEADERS` | Kopfzeilen des Exports, z. B. für Zugangsdaten | [9](09-betrieb.md) |
| `OTEL_SERVICE_NAME` | Dienstname in den Spans; Standard `grauwolf` | [9](09-betrieb.md) |
| `PGVECTOR_URL` | Übliche Namensgebung für die Variable mit der PostgreSQL-URL (`postgres://user:password@host:5432/db`) von `knowledge.store` und `long_term_memory.vector_store`; die Spec nennt die Variable in `url_env`, daher funktioniert jeder Name. Braucht die Erweiterung pgvector | [7](07-gespraeche-und-wissen.md#wissen-und-gedächtnis-in-postgresql-mit-pgvector) |
| `PGVECTOR_TEST_URL` | PostgreSQL mit pgvector für die optionalen Datenbanktests von `grauwolf-store-pgvector` (`cargo test -p grauwolf-store-pgvector --test postgres`); ohne sie kehren sie sofort zurück | [Entwicklerhandbuch](../entwickler/README.md) |
| `CARGO`, `RUSTC` | Programme für den Tool-Build und `generate --check`; Standard `cargo` und `rustc` | [3](03-tools.md) |
| `PATH`, `HOME`, `CARGO_HOME`, `RUSTUP_HOME`, `RUSTUP_TOOLCHAIN` | Die einzigen Variablen, die der Tool-Build an `cargo` weitergibt | [10](10-selbst-gebaute-tools.md#sicherheit) |
| beliebige, als `${VAR}` | In `headers` von `mcp_servers` und `agent_tools` ersetzt; fehlt die Variable, ist das ein Fehler | [8](08-externe-tools-und-agenten.md) |

## Standardverzeichnisse

Alle relativ zum Arbeitsverzeichnis, in dem du `grauwolf` startest:

| Verzeichnis | Inhalt | Option |
|---|---|---|
| `.grauwolf/tools` | Cache gebauter Tools und geholter Tool-Pakete (`packages/`) | `--cache` |
| `.grauwolf/conversations` | Gespräche (`<id>.json`, je Benutzer unter `users/<hex>/<id>.json`) | `--conversations-dir` |
| `.grauwolf/runs` | Checkpoints (`<run-id>.json`) | `--runs-dir` |

Pfade *in* der Spec (`builtin_tools.root`, `knowledge.sources`, `tools_dir`, `code_file` …) gelten
dagegen relativ zur Spec-Datei.
