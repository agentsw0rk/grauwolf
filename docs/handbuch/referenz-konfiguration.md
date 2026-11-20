# Konfigurationsreferenz: `grauwolf.toml`

`grauwolf.toml` wird aus dem Verzeichnis der Spec gelesen. Die Datei enthält zwei Tabellen: die
**Tool-Richtlinie** (`[tool_packages]`), die festlegt, welche Tool-Pakete ([Kapitel 3](03-tools.md#vertrauen-die-tool-richtlinie-und-signaturen)) benutzt werden dürfen. Die Umgebungsvariable `GRAUWOLF_TOOL_POLICY` nennt eine andere Richtliniendatei im gleichen Format; sie hat Vorrang vor `grauwolf.toml`. Ohne beides ist alles erlaubt und nichts muss signiert sein. Die Richtlinie gilt für jeden Befehl, der Tool-Pakete auflöst (`spec run|chat|serve|mcp|resume|build|generate`, `tools lock|pull`; `tools inspect` wendet nur die Allowlists an). Eine Datei, die sich nicht lesen lässt, unbekannte Schlüssel und falsche Typen sind Fehler, die die Datei nennen. Ein vollständig kommentiertes Beispiel: [`beispiele/03-richtlinie/grauwolf.toml`](beispiele/03-richtlinie/grauwolf.toml).

## `[tool_packages]`

| Schlüssel | Typ | Standard | Bedeutung |
|---|---|---|---|
| `allowed_registries` | Liste von Text | beliebig | OCI-Registries (`ghcr.io`) oder Präfixe aus Registry und Repository (`ghcr.io/acme`; ganze Pfadteile), die benutzt werden dürfen. Eine leere Liste erlaubt keine |
| `allowed_git_hosts` | Liste von Text | beliebig | Git-Hosts (`github.com`) oder Präfixe aus Host und Pfad (`github.com/acme`), die benutzt werden dürfen. Eine leere Liste erlaubt keine |
| `require_signatures` | Wahrheitswert | `false` | Jedes Paket muss eine gültige Signatur tragen, geprüft mit `oci_keys` (OCI) oder `git_allowed_signers` (Git). Braucht mindestens einen der beiden Schlüssel |
| `oci_keys` | Liste von Pfaden | `[]` | PEM-Public-Keys (P-256), die die cosign-kompatiblen Signaturen der OCI-Pakete prüfen; eine Datei darf mehrere Schlüssel enthalten. Erzeugt mit `grauwolf tools keygen` oder cosign. Relativ zur Richtliniendatei |
| `git_allowed_signers` | Pfad | – | Datei im Format von `gpg.ssh.allowedSignersFile` aus git (`principals [namespaces="…"] keytype key`): wer Commits und Tags von Git-Paketen signieren darf; die Principals werden mit der E-Mail-Adresse des Committers oder Taggers abgeglichen. Relativ zur Richtliniendatei |

Details und Schritt für Schritt: [Vertrauen: Tool-Richtlinie und Signaturen](03-tools.md#vertrauen-die-tool-richtlinie-und-signaturen).

## `[credentials]`

Quellen für Zugangsdaten: Variablen, die aus einer Datei oder aus OpenBao kommen statt aus der Umgebung ([Kapitel 12](12-zugangsdaten-aus-openbao.md)). Gelesen wird aus derselben `grauwolf.toml` oder aus der Datei, die `GRAUWOLF_CREDENTIALS` nennt (diese hat Vorrang). Relative `file:`-Pfade sind relativ zu dieser Datei.

```toml
[credentials.openbao]
address = "https://openbao.internal:8200"
auth = { spiffe = { audience = "openbao", role = "grauwolf-agents", mount = "jwt" } }
refresh = "5m"

[credentials.variables]
ATLASSIAN_API_TOKEN = "openbao:kv/agents/reviewer#jira"
OPENAI_API_KEY      = "file:/run/secrets/openai/api-key"
```

| Schlüssel | Typ | Standard | Bedeutung |
|---|---|---|---|
| `openbao.address` | Text | – | Basisadresse von OpenBao oder Vault. Pflicht bei einer Quelle `openbao:` |
| `openbao.auth.spiffe.audience` | Text | – | Audience des JWT-SVID; die `bound_audiences` der Rolle |
| `openbao.auth.spiffe.role` | Text | – | Rolle der JWT-Auth-Methode |
| `openbao.auth.spiffe.mount` | Text | `jwt` | Mount der JWT-Auth-Methode |
| `openbao.refresh` | Dauer (`30s`, `5m`, `1h`) | `5m` | Intervall, nach dem alle zugeordneten Werte neu gelesen werden |
| `variables.<NAME>` | Text | – | `openbao:<mount>/<pfad>#<feld>` (Alias `vault:`) oder `file:<pfad>`. `<NAME>` muss eine Variable aus der [Tabelle der Umgebungsvariablen](referenz-cli.md#umgebungsvariablen) sein, nicht eine, die direkt aus der Prozessumgebung gelesen wird (`PATH`, `HOME`, `OTEL_EXPORTER_OTLP_*`, …). Eine Variable, die zugeordnet und zugleich gesetzt ist, ist ein Fehler |

Unbekannte Schlüssel und falsche Typen sind Fehler, die die Datei nennen. Die Anmeldung bei OpenBao braucht `SPIFFE_ENDPOINT_SOCKET`.

## Die Richtliniendatei des Identitätsdienstes

Nicht Teil von `grauwolf.toml`: die JSON-Datei, die `grauwolf identity serve` aus `GRAUWOLF_IDENTITY_POLICY`
liest ([Kapitel 14](14-im-auftrag-von-personen.md#die-richtlinie)). Sie nennt die Workloads, die Txn-Tokens im
Auftrag von Personen anfordern dürfen – die Agenten am Eingang – und in `grants`, welche Agenten auf Grants
von Personen laufen dürfen und welche Workloads solche Läufe starten dürfen.

```json
{ "callers": {
    "spiffe://agentswork.de/ns/coordinator/sa/coordinator": { "purposes": ["review"] },
    "spiffe://agentswork.de/ns/frontends/*": {}
} }
```

| Schlüssel | Typ | Standard | Bedeutung |
|---|---|---|---|
| `callers.<SPIFFE-ID>` | Objekt | – | Eine Workload, die Tokens anfordern darf. Ein Schlüssel, der auf `/*` endet, ist ein Präfix aus ganzen Pfadteilen; die genaue ID gewinnt vor einem Präfix, das längste Präfix vor kürzeren |
| `callers.<…>.purposes` | Liste von Text | jeder | Die Zwecke (`purp`), die sie anfordern darf; ohne den Schlüssel ist jeder Zweck erlaubt |
| `grants` | Objekt | keiner | Grants für unbeaufsichtigte Läufe (ADR-0111 Stufe 3); ohne ihn lässt sich kein Grant anlegen oder nutzen |
| `grants.agents.<SPIFFE-ID>` | Objekt | – | Ein Einstiegsagent, der auf Grants von Personen laufen darf; `/*`-Präfixe wie bei `callers` |
| `grants.agents.<…>.purposes` | Liste von Text | jeder | Die Zwecke, die seine Grants haben dürfen; jeder Zweck eines Grants muss in der Liste stehen |
| `grants.triggers` | Liste von SPIFFE-IDs | `[]` | Die Workloads, die Grant-Läufe starten dürfen (z. B. `spiffe://<td>/ns/schedules/sa/morning` oder `spiffe://<td>/ns/schedules/*`) |

```json
{ "callers": {},
  "grants": {
    "agents": { "spiffe://agentswork.de/ns/coordinator/sa/coordinator": { "purposes": ["review"] } },
    "triggers": ["spiffe://agentswork.de/ns/schedules/*"]
} }
```

Beide Abschnitte sind optional. `grants` wird beim Anlegen eines Grants und bei jedem seiner Läufe
erneut geprüft; entfernst du einen Agenten oder Auslöser, stoppt das seine bestehenden Grants sofort
([Kapitel 14](14-im-auftrag-von-personen.md#unbeaufsichtigte-läufe-grants-und-zeitpläne)).

Unbekannte Schlüssel, ungültige SPIFFE-IDs und falsche Typen sind Fehler: Beim Start halten sie den Dienst an,
später bleibt die alte Richtlinie und der Fehler wird gemeldet. Die Datei wird alle 2 Sekunden auf Änderungen
geprüft (auch der Symlink-Tausch einer ConfigMap). Für das Erneuern eines Txn-Tokens braucht es keinen Eintrag:
Jede Workload der Trust Domain des Tokens darf erneuern.
