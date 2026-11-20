# Kapitel 2 – Entwicklungsumgebung und Arbeitsweise

Dieses Kapitel zeigt, wie du grauwolf baust und testest, und nach welchen Regeln das Projekt arbeitet: die Pflichtprüfungen vor jedem Commit, testgetriebene Entwicklung, ADRs, die `AGENTS.md` jeder Crate und der Feature-Katalog. Wenn du die Bibliothek nur nutzen willst, brauchst du vor allem die Abschnitte 2.1 und 2.2; wenn du am Framework arbeitest, brauchst du das ganze Kapitel.

## 2.1 Toolchain

Du brauchst:

- **Rust ≥ 1.85** (Edition 2024, in der `Cargo.toml` über `rust-version = "1.85"` festgelegt, [ADR-0011](../adr/0011-edition-msrv.md)).
- **Das WASM-Target `wasm32-unknown-unknown`.** Tool-Code aus Specs wird dafür kompiliert ([Kapitel 4](04-tools-intern.md)). Ohne das Target schlagen alle Tests fehl, die Tools bauen.
- **Einen C-Linker und Build-Werkzeuge** (`build-essential` unter Debian/Ubuntu), weil einige Abhängigkeiten nativen Code enthalten.

```sh
rustup toolchain install stable
rustup target add wasm32-unknown-unknown
sudo apt-get install -y build-essential   # Debian/Ubuntu
```

Für manuelle Läufe gegen ein echtes Modell ist ein lokales Ollama praktisch; für die Tests brauchst du es **nicht** – sie laufen gegen Mocks.

## 2.2 Bauen und testen

Der ganze Workspace:

```sh
cargo build --workspace
cargo test --workspace
```

Eine einzelne Crate, ein einzelnes Testprogramm oder ein einzelner Test:

```sh
cargo test -p grauwolf-interpreter                 # eine Crate
cargo test -p grauwolf-interpreter --test echo     # tests/echo.rs
cargo test -p grauwolf-interpreter --test echo runs_the_echo_spec   # ein Testfall
```

Die Schichtenregeln aus [Kapitel 1](01-architektur.md) prüfst du allein mit:

```sh
cargo xtask check-arch
```

Die CLI startest du aus dem Workspace mit `cargo run -p grauwolf-cli -- …`, z. B. `cargo run -p grauwolf-cli -- spec validate examples/specs/calculator.json`. Alle Befehle stehen in der [CLI-Referenz des Nutzerhandbuchs](../handbuch/referenz-cli.md).

`grauwolf-cli` ist eine Bibliothek plus ein dünnes Binary: `src/main.rs` parst nur die Kommandozeile und ruft `grauwolf_cli::dispatch` auf; die Befehle liegen in `src/commands/` (ein Modul je Befehl, etwa `commands/spec/validate.rs`, `commands/tools.rs`), die gemeinsamen Optionsstrukturen in `src/options.rs`. Befehle schreiben über ein `Io`-Handle (Standardströme oder erfasste Puffer), daher führt `crates/apps/grauwolf-cli/tests/commands.rs` sie im selben Prozess mit `grauwolf_cli::run` aus und prüft ihre Ausgabe; `tests/cli.rs` und `tests/tool_packages.rs` starten weiterhin das echte Binary.

Die Tool-Befehle (`tools build|pack|push|pull|inspect|lock|keygen|validate`) stehen nicht in dieser Crate: Sie liegen in `crates/apps/grauwolf-tools-app`, das auch das kleine Programm `grauwolf-tools` baut (`cargo run -p grauwolf-tools-app -- validate <package folder>`; ADR-0118). `commands/tools.rs` delegiert nur, und `crates/apps/grauwolf-tools-app/tests/commands.rs` startet für jeden Befehl das Binary `grauwolf-tools`. Halte diese Crate frei von Abhängigkeiten auf Host, Server und CLI: `cargo xtask check-arch` und `xtask/tests/workspace_architecture.rs` schlagen sonst fehl.

### Tipps für knapp ausgestattete Rechner

Ein vollständiger Build mit allen Tests ist groß (mehrere GB). Drei Einstellungen helfen:

| Einstellung | Wirkung |
|---|---|
| `CARGO_TARGET_DIR=~/grauwolf-target` | Build-Ausgabe außerhalb des Workspace, z. B. wenn der Workspace auf einem langsamen oder kleinen Volume liegt (Container mit eingebundenem Verzeichnis). |
| `cargo test -j 4 …` | Weniger parallele Compiler- und Linker-Prozesse; verhindert, dass der Linker bei wenig Speicher beendet wird (`signal 9`). |
| `CARGO_INCREMENTAL=0` | Kein inkrementeller Cache (`debug/incremental`); spart viel Platz bei kaum längeren Builds. |

Der Äquivalenztest (Abschnitt 2.5) legt generierte Crates unter `$CARGO_TARGET_DIR/generated-equivalence` ab (sonst `target/generated-equivalence`). Das Verzeichnis kann auf mehrere GB wachsen; du darfst es nach einem Testlauf jederzeit löschen, der nächste Lauf baut es neu.

### Teststufen

Die Tests laufen in drei Stufen:

| Stufe | Was | Wie es läuft |
|---|---|---|
| schnell | Unit-Tests, Adapter-Tests gegen Mocks, In-Process-Integrationstests (etwa 940 Tests, etwa 20 s Testlaufzeit) | `cargo test --workspace` |
| langsam | Tests, die generierte Crates oder WASM-Tool-Code mit cargo kompilieren oder das CLI-Binary starten, das dies tut (etwa 30 Tests, mehrere Minuten) | `cargo test --workspace -- --include-ignored` (nur die langsamen: `-- --ignored`) |
| live | gegen echte Modelle und Dienste, per Umgebungsvariablen zugeschaltet (Abschnitt 2.5) | wie dort beschrieben |

Einen Test markierst du als langsam, mit einem Grund, der sagt, was ihn langsam macht; sonst ändert sich nichts:

```rust
#[test]
#[ignore = "slow: builds tool code for WASM with cargo"]
fn builds_working_code() { … }
```

Markiere einen Test nur, wenn er mit cargo kompiliert, ein Programm baut oder startet, das dies tut, oder mehrere Sekunden braucht; schnelle Tests markierst du nicht, und ein Test darf sich nie auf Seiteneffekte eines langsamen Tests verlassen. Ein einfaches `cargo test --workspace` überspringt die langsame Stufe, deshalb ist der vollständige Lauf vor einem Push der mit `--include-ignored`.

### `cargo xtask check`

Die beiden Prüfungen dieses Abschnitts als ein Befehl, mit einer lesbaren Auswahl dessen, was lief ([ADR-0108](../adr/0108-restructuring-for-separation-of-concerns.md)):

```sh
cargo xtask check --full       # fmt --check, clippy, alle Tests inkl. langsamer Stufe, Feature-Matrix, Schichtenregeln
cargo xtask check --affected   # dasselbe für das Geänderte, nur schnelle Stufe
cargo xtask check --affected --base main   # seit main geändert (plus Arbeitsverzeichnis)
```

`--affected` entnimmt die geänderten Dateien aus `git diff --name-only <base>` und `git status`, ordnet sie Paketen zu, nimmt jedes Paket hinzu, das von einem geänderten abhängt, und gibt die Auswahl mit dem Grund aus. Änderungen an `Cargo.toml`, `Cargo.lock`, `.cargo/`, `xtask/` oder `rust-toolchain*` wählen den ganzen Workspace; die Beispiele der Handbücher wählen `grauwolf-host`; andere Dokumente führen nur die Formatprüfung aus. Der Befehl bricht beim ersten fehlgeschlagenen Schritt ab und gibt dessen Ausgabe aus.

Die Regel (Abschnitt 2.3): `--affected` vor jedem Commit, `--full` vor jedem Push und in der CI. Bei einem warmen Build dauert eine Änderung in einer Blatt-Crate Sekunden, eine in `grauwolf-prompt` (unter fast allem) etwa zwei Minuten.

## 2.3 Pflichtprüfungen vor jedem Commit und Push

Vor **jedem** Commit:

```sh
cargo fmt --all
cargo xtask check --affected
```

Vor **jedem** Push (und in der CI):

```sh
cargo xtask check --full
```

- `cargo fmt` formatiert; geänderte Dateien gehören in denselben Commit.
- `--affected` führt `clippy -D warnings` aus (Tests eingeschlossen, `--all-targets`) und die schnellen Tests für die geänderten Crates und jede Crate, die von ihnen abhängt, dazu den Architekturtest; reine Dokumente bekommen nur die Formatprüfung.
- `--full` führt die Formatprüfung, clippy und **alle** Tests des Workspace inklusive der langsamen Stufe (`-- --include-ignored`), den Architekturtest (`xtask/tests/workspace_architecture.rs`) und den Test der Beispiele aus dem Nutzerhandbuch (Abschnitt 2.8) aus. Danach führt er clippy und die schnellen Tests der **Feature-Matrix** aus (`xtask::check::FEATURE_MATRIX`): `grauwolf-host` ohne Features und nur mit `builtin`, `grauwolf-server` und die Fassade `grauwolf` ohne Features. Der Workspace-Build vereinheitlicht Features, daher kompilieren nur diese Builds den Code hinter `#[cfg(not(feature = …))]` (die Fehler „this build has no …“ und ihre Tests). Ein Integrationstest, der ein optionales Feature braucht, deklariert es als `required-features` in einem `[[test]]`-Abschnitt der `Cargo.toml` seiner Crate; sonst scheitert die Matrix beim Kompilieren.

Alle Tests müssen grün sein. Ein roter Test wird nicht „später“ behoben, sondern vor dem Commit – auch wenn er mit deiner Änderung nichts zu tun zu haben scheint.

## 2.4 Testgetriebene Entwicklung

Neue Funktionalität entsteht testzuerst (TDD) im Zyklus *Rot – Grün – Refactoring*:

1. **Rot:** Schreibe **einen** Testfall, der das gewünschte Verhalten beschreibt, und sieh zu, wie er fehlschlägt.
2. **Grün:** Schreibe den kleinsten Code, der den Test besteht.
3. **Refactoring:** Verbessere Namen und Struktur; die Tests bleiben grün.

Dann machst du einen Commit mit **Test + Implementierung + Dokumentation** zusammen. Dokumentation heißt: Doc-Kommentare, der Stand in der `AGENTS.md` der Crate und das README oder Nutzerhandbuch, falls nötig. Ein Commit pro Verhalten – lieber viele kleine Commits als wenige große.

Tests prüfen **Verhalten**, nicht Implementierungsdetails. Gut: „Der Agent gibt die Antwort des Modells zurück“, „ungültige Specs werden mit einem JSON-Pointer abgelehnt“. Schlecht: „Funktion X wird aufgerufen“, „die Datei enthält den Text Y“.

Ein typischer Testname beschreibt das Verhalten als Satz, und die Kopfzeile der Datei nennt die Feature-ID, z. B. `crates/domain/grauwolf-interpreter/tests/echo.rs` (die hier zitierte Kopfzeile stammt aus dieser Datei):

```rust
//! Interpreter runs specs from the example corpus (F13.10).

#[test]
fn refuses_specs_that_do_not_validate() {
    // …
}
```

## 2.5 Teststrategie

### Mock statt echtem LLM

Fast alle Tests laufen ohne Netzwerk gegen `MockLlmClient` aus dem Testkit `grauwolf-test`. Der Mock arbeitet mit Regeln: Enthält die letzte Eingabe einen bestimmten Text, antwortet er mit einer Nachricht oder einem Tool-Aufruf. Nach `crates/domain/grauwolf-interpreter/tests/echo.rs` (gekürzt):

```rust
use grauwolf_interpreter::Interpreter;
use grauwolf_llm::PromptExecutor;
use grauwolf_test::MockLlmClient;

let executor = PromptExecutor::builder()
    .client(
        "ollama",
        Arc::new(MockLlmClient::new("?").answer_when_contains("Hello world", "Hello world")),
    )
    .build();
let agent = Interpreter::new(Arc::new(executor))
    .load(&example("echo.json"))
    .unwrap();

let output = block_on(agent.run("Hello world")).unwrap();
assert_eq!(output, "Hello world");
```

Ohne passende Regel antwortet `MockLlmClient::new(default)` mit `default`. Weitere Regeln: `answer_when_contains(needle, answer)`, `call_tool_when_contains(needle, tool, args)` und `call_tools_when_contains(needle, [(tool, args), …])` für mehrere Tool-Aufrufe in einer Antwort. Der Name, unter dem du den Mock beim `PromptExecutor` registrierst, muss mit dem `model.provider` der Spec übereinstimmen.

Domain-Tests brauchen keine tokio-Laufzeit: `futures_executor::block_on` genügt. Tests in Adaptern und Apps verwenden `#[tokio::test]`.

### HTTP-Adapter gegen wiremock

Anbieter-Adapter testest du gegen einen lokalen HTTP-Mock mit aufgezeichneten Antworten. Nach `crates/adapters/grauwolf-ollama/tests/chat.rs` (gekürzt):

```rust
let server = MockServer::start().await;
Mock::given(method("POST"))
    .and(path("/api/chat"))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "model": "qwen3.5:9b",
        "message": { "role": "assistant", "content": "Hi" },
        "done": true
    })))
    .expect(1)
    .mount(&server)
    .await;
let client = OllamaClient::new(server.uri());
let prompt = Prompt::builder("p").system("Be brief.").user("Hello").build();

let answer = client.execute(&prompt, &model(), &[]).await.unwrap();
assert_eq!(answer, Message::assistant("Hi"));
```

### Konformitäts-Suite und Live-Tests

`grauwolf_test::conformance::check_llm_client(&client, &model)` prüft, ob sich ein `LlmClient` wie erwartet verhält (Text, Tool-Aufrufe, …), und gibt eine Liste von `Failure`s zurück. Sie läuft nur gegen echte Modelle, wenn die passende Umgebungsvariable gesetzt ist; sonst überspringt sich der Test selbst (`crates/adapters/grauwolf-ollama/tests/live.rs`):

```sh
GRAUWOLF_OLLAMA_URL=http://localhost:11434 GRAUWOLF_OLLAMA_MODEL=qwen3.5:9b \
  cargo test -p grauwolf-ollama --test live
GRAUWOLF_OPENAI_API_KEY=… cargo test -p grauwolf-openai --test live
GRAUWOLF_ANTHROPIC_API_KEY=… cargo test -p grauwolf-anthropic --test live
GRAUWOLF_GEMINI_API_KEY=… cargo test -p grauwolf-gemini --test live
AWS_BEARER_TOKEN_BEDROCK=… AWS_REGION=us-west-2 cargo test -p grauwolf-bedrock --test live
```

Optional wählt `GRAUWOLF_<PROVIDER>_MODEL` das Modell, und `GRAUWOLF_OPENAI_BASE_URL` richtet den OpenAI-Test auf einen kompatiblen Dienst. Der Bedrock-Test nimmt die üblichen AWS-Zugangsdaten (`AWS_BEARER_TOKEN_BEDROCK` oder `AWS_ACCESS_KEY_ID` und `AWS_SECRET_ACCESS_KEY`) und `GRAUWOLF_BEDROCK_MODEL`; ohne Zugangsdaten wird er übersprungen.

Live-Tests sind optional und wirken in `cargo test --workspace` nicht – ohne die Variable bestehen sie sofort.

### Äquivalenztests

`crates/domain/grauwolf-generator/tests/equivalence.rs` führt jede Spec des Korpus `examples/specs/` einmal im Interpreter und einmal als generierte Crate gegen denselben Mock aus und vergleicht die Ausgaben. Wenn du eine Node-Art, eine Bedingung oder ein Spec-Feld hinzufügst, ergänze dort einen Fall (`Case { spec, input, expect, mock }`). Mehr dazu in [Kapitel 6](06-generator.md).

### Temporäre Verzeichnisse

Tests bauen nie von Hand Verzeichnisse im System-Temp-Ordner (`std::env::temp_dir().join(…)`): Solche Verzeichnisse werden nie entfernt und häufen sich zu Tausenden. Nimm stattdessen `grauwolf_test::TestDir` (Dev-Dependency `grauwolf-test`):

```rust
let dir = grauwolf_test::TestDir::new("chat-history"); // /tmp/grauwolf-chat-history-<pid>-<n>
std::fs::write(dir.join("spec.json"), "{}").unwrap();
// `dir` derefs to `Path`; the directory and its content are removed when `dir` is dropped.
```

Der Name ist pro Test eindeutig (Prozess-ID und Zähler), sodass parallele Tests und parallele Testprozesse nie ein Verzeichnis teilen. Behalte den Wächter für den ganzen Test am Leben, auch solange eine Hintergrundaufgabe (ein gestarteter Agent, ein Mock-Server) den Pfad nutzt, und verwende denselben Wächter, wenn ein Test prüft, dass Daten von einer Instanz zur nächsten erhalten bleiben. Eine Hilfsfunktion, die nur einen Pfad zurückgibt, der von einem lokalen Wächter abgeleitet ist, würde das Verzeichnis bei ihrer Rückkehr entfernen: Gib den Wächter selbst zurück.

### Andere Testarten

- **Snapshot-Tests** mit `insta` für generierten Code (`crates/domain/grauwolf-generator/tests/snapshots`).
- **Katalog-Konformität**: Für jede Node-Art und jede Bedingung im Katalog gibt es eine Implementierung (`crates/domain/grauwolf-agents/tests/catalog_conformance.rs`) und eine Generator-Vorlage (`crates/domain/grauwolf-generator/tests/catalog_conformance.rs`).
- **Dokumenttests**, z. B. der `compile_fail`-Test an `grauwolf_graph::TypedGraphBuilder::edge`, der zeigt, dass falsch typisierte Kanten nicht kompilieren.

## 2.6 Architekturentscheidungen festhalten

Jede Entwurfsentscheidung mit Wirkung über eine einzelne Crate hinaus wird als **ADR** (Architecture Decision Record) unter `docs/adr/` festgehalten. Lies die betroffenen ADRs, bevor du etwas umstrukturierst. Eine neue Entscheidung bekommt die nächste freie Nummer und das folgende Format. ADRs werden auf **Englisch** geschrieben ([ADR-0091](../adr/0091-english-everywhere.md)) mit den Überschriften `Status`, `Date`, `Context`, `Decision` und `Consequences`; der Status ist `Proposed` oder `Accepted`.

```markdown
# ADR-0092: Short title

- **Status:** Proposed
- **Date:** 2026-11-10

## Context
Which problem, which constraints?

## Decision
What is done – concretely, with names of types, fields, crates.

## Consequences
What follows, what gets harder, what remains open.
```

Ergänze den ADR in der Tabelle in [docs/adr/README.md](../adr/README.md). Ersetzte ADRs werden nicht gelöscht, sondern auf `Superseded by ADR-XXXX` gesetzt.

## 2.7 `AGENTS.md` je Crate

Jede Crate hat eine `AGENTS.md` (mit Symlink `CLAUDE.md`). Sie ist der kompakte Vertrag der Crate für Menschen und KI-Agenten ([ADR-0019](../adr/0019-ai-guardrails.md)):

- **Schicht und Stand** – was die Crate heute kann, mit den wichtigsten Typen und Funktionen.
- **Zweck** – ein Satz.
- **Features / ADRs** – zugehörige Feature-IDs und Entscheidungen.
- **Erlaubte Abhängigkeiten** – grauwolf-Crates und externe Crates.
- **Invarianten** – was nie verletzt werden darf; noch nicht umgesetzte werden als „Ziel“ markiert.
- **Tests** und **Arbeit an dieser Crate**.

Ändern sich Zweck, API, Abhängigkeiten oder Invarianten, aktualisierst du die `AGENTS.md` im selben Commit. Eine neue Abhängigkeit fügst du erst hinzu, nachdem du sie gegen [ADR-0016](../adr/0016-layered-architecture.md) geprüft hast; der Architekturtest prüft die Schichten, aber die feinere Liste in der `AGENTS.md` prüft niemand automatisch.

## 2.8 Feature-Katalog und Beispiele

Der [Feature-Katalog](../features/README.md) listet alle Funktionen in Epics (`E03` Tools, `E13` generative Agenten, …) mit IDs wie `F03.5` (`#[tool]`) oder `F13.10` (Interpreter). Jede ID gehört zu genau einer Crate. Nenne sie in Doc-Kommentaren und in der Kopfzeile von Testdateien, damit Funktion, Code und Tests zusammen zu finden sind.

Die Beispiel-Specs des Nutzerhandbuchs unter `docs/user-guide/examples/*.json` werden bei jedem `cargo test --workspace` geprüft (`crates/apps/grauwolf-host/tests/docs_examples.rs`): Jede Datei muss sich mit `grauwolf::loader::load_spec` laden und validieren lassen. Wenn du das Spec-Format änderst, merkst du sofort, wenn das Nutzerhandbuch veraltet.

## 2.9 Sprache

Die Sprachregeln folgen [ADR-0091](../adr/0091-english-everywhere.md):

- **Englisch** – alles im Repository: `README.md`, das Nutzer- und das Entwicklerhandbuch samt ihren Beispielen, ADRs, `docs/architecture.md`, der Feature-Katalog, Code- und Doc-Kommentare, die `AGENTS.md` der Crates, Testdaten, jede Meldung der CLI und der Bibliothek (Fehlermeldungen, Validierungsfehler, CLI-Ausgabe, Tool-Ergebnisse an das Modell) und die eingebauten Anweisungen an das Modell (Komprimierung, Fakten, Moderation, Verifikation, Planer, Builder, …). Ihr Wortlaut beeinflusst die Ergebnisse kleiner Modelle; prüfe Änderungen daran mit einem Live-Lauf (etwa gegen ein lokales Ollama).
- **Deutsch** – nur `docs/handbuch/` und `docs/entwickler/`, übersetzt aus dem englischen Stand vom 15.11.2026 und nur bei Bedarf aktualisiert.
- **Bezeichner im Code auf Englisch** – Typen, Funktionen, Felder, Testnamen.

```rust
/// Limits every run of every loaded agent to `budget` (ADR-0072).
pub fn with_budget(mut self, budget: grauwolf_agents::Budget) -> Self { … }
```

Öffentliche Elemente brauchen Doc-Kommentare. Bisher wird das nur in `grauwolf-codegen` erzwungen (`#![deny(missing_docs)]`); in allen anderen Crates ist es eine Konvention.

## 2.10 Ein Release schneiden

Nur der Eigentümer schneidet ein Release und setzt den Tag; ein Release ist ein Tag `v<version>` auf `main`. Images (`.github/workflows/images.yml`, ADR-0096) werden nur aus einem Tag veröffentlicht.

1. **Version erhöhen** in `[workspace.package]` der `Cargo.toml` (jede Crate erbt sie) und `cargo check --workspace` ausführen, damit `Cargo.lock` nachzieht. Die Version ist der Vertrag zwischen CLI, Builder-Image und generierten Crates: Das `GRAUWOLF_VERSION` des Builders muss der Version der CLI entsprechen, sonst weigert sich das generierte `Dockerfile`, zu bauen.
2. **Vollständige Prüfung:** `cargo xtask check --full` (der ganze Workspace inklusive der langsamen Tests). Den Versionssprung committen, nach `main` mergen.
3. **Tag:** `git tag v<version>` auf diesem Commit und `git push origin v<version>`. Der Workflow bricht sofort ab, wenn Tag und Workspace-Version nicht übereinstimmen.
4. **Was die CI veröffentlicht** (privat, `ghcr.io/agentsw0rk`): nach den Smoke-Tests das CLI-Image `grauwolf`, `grauwolf-tools` und das Builder-Image `grauwolf-builder`, jeweils nativ gebaut für `linux/amd64` (`ubuntu-24.04`) und `linux/arm64` (`ubuntu-24.04-arm`), je Architektur über den Digest gepusht und zu einem Multi-Arch-Tag `:<version>` zusammengeführt (`:latest` für das CLI- und das Tools-Image, nicht für den Builder). Der Builder-Job führt außerdem die Offline-Agenten-Builds aus, sodass die Versionsprüfung auf beiden Architekturen positiv nachgewiesen ist, bevor etwas gepusht wird. Der Builder-Job pusht sein getestetes Image unter dem Hilfs-Tag `:<version>-<arch>` (der klassische Image-Speicher kann ohne Namen nicht pushen); verwendet werden nur die Digests.
5. **Prüfen:** `docker buildx imagetools inspect ghcr.io/agentsw0rk/grauwolf-builder:<version>` listet beide Plattformen auf.

Das Token des Repositorys braucht `packages: write` (im Workflow gesetzt); das Label `org.opencontainers.image.source` verbindet jedes Paket mit dem Repository, sodass das Paket dessen Zugriffsrechte erbt. Ein Paket, das schon existiert, braucht unter seiner Einstellung „Manage Actions access“ unter Umständen das Repository als Eintrag.

**Private Images abrufen:** `docker login ghcr.io` mit einem persönlichen Access-Token, das `read:packages` hat. Ein Cluster (der von AgentWeaver, das das Builder-Image in seinen kaniko-Builds und das CLI-Image für sein Backend abruft) braucht ein Image-Pull-Secret für `ghcr.io` mit einem solchen Token. Native arm64-Runner sind nur für öffentliche Repositories kostenlos; für ein privates kann ein GitHub-Plan mit arm64-Runnern (größere Runner) nötig sein. Die arm64-Jobs laufen deshalb nur bei Tags und manuellen Läufen (`workflow_dispatch`) – starte einen von Hand vor dem ersten Release-Tag, um zu sehen, ob der Plan solche Runner hat. Der Workflow läuft bei `pull_request` und bei Pushes auf `main` (nur amd64), ohne zu veröffentlichen; lokal lässt er sich nicht ausführen (kein Docker im Entwicklungscontainer), daher ist der erste Lauf nach einer Änderung der eigentliche Test.

## Das Wichtigste in Kürze

- Rust ≥ 1.85, `rustup target add wasm32-unknown-unknown`, Build-Werkzeuge.
- Vor jedem Commit: `cargo fmt --all` und `cargo xtask check --affected`; vor jedem Push: `cargo xtask check --full` (Abschnitt 2.3).
- TDD: ein Testfall je Zyklus; Commit = Test + Code + Dokumentation.
- Tests laufen gegen `MockLlmClient` und wiremock; Live-Tests nur mit einer Umgebungsvariable; Äquivalenztests halten Interpreter und Generator gleich.
- Entscheidungen als ADRs, Crate-Verträge in `AGENTS.md`, Feature-IDs in Doc-Kommentaren und Tests.
- Schnelle Stufe `cargo test --workspace`, langsame Stufe mit `-- --include-ignored`; `cargo xtask check --affected` vor Commits, `--full` vor Pushes (Abschnitt 2.3).
- Wenig Platz oder Speicher: `CARGO_TARGET_DIR` außerhalb des Workspace, `-j 4`, `CARGO_INCREMENTAL=0`, `generated-equivalence` nach dem Lauf löschen.

Weiter mit [Kapitel 3 – grauwolf als Bibliothek](03-rust-api.md).
