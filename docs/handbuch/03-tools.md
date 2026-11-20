# Kapitel 3: Tools

Ein Sprachmodell ist gut mit Worten, aber schlecht im Rechnen, und es sieht weder deine Dateien
noch die aktuelle Uhrzeit. **Tools** schließen diese Lücke: Sie sind Funktionen, die das Modell
während eines Laufs aufrufen kann. In diesem Kapitel schreibst du eigene Tools, legst sie in eigene
Dateien und Ordner und nutzt die eingebauten Tools für Dateien und Rückfragen an den Benutzer.

## Was ein Tool ist

Ein Tool hat drei Dinge, die das Modell sieht:

- einen **Namen** (snake_case, z. B. `multiply`),
- eine **Beschreibung**, aus der das Modell erkennt, wofür das Tool gut ist,
- ein **Argument-Schema** (JSON-Schema), das sagt, welche Werte das Tool erwartet.

Der Ablauf eines Tool-Aufrufs: Das Modell antwortet nicht mit Text, sondern mit einem
*Tool-Aufruf* („rufe `multiply` mit `a = 12.5`, `b = 8` auf“). grauwolf führt das Tool aus und gibt
das Ergebnis an das Modell zurück, das dann weiterarbeitet – bis es eine Textantwort gibt. Die
Strategie `single_run` aus Kapitel 1 macht genau das.

Tool-Namen gelten für den ganzen Agenten: Eigene, eingebaute und externe Tools dürfen nicht gleich
heißen, sonst meldet `spec validate` einen Fehler.

## Eigener Tool-Code

Ein eigenes Tool steht in der Liste `tools`. Hier ein Rechner, der multiplizieren kann
([`beispiele/03-rechner.json`](beispiele/03-rechner.json)):

```json
{
  "spec_version": 1,
  "name": "calculator",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du bist ein Rechenassistent. Nutze für jede Multiplikation das Tool multiply und antworte auf Deutsch.",
  "tools": [
    {
      "name": "multiply",
      "description": "Multipliziert zwei Zahlen a und b",
      "args": {
        "type": "object",
        "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
        "required": ["a", "b"]
      },
      "code": "let a = args[\"a\"].as_f64().ok_or(\"a is missing\")?;\nlet b = args[\"b\"].as_f64().ok_or(\"b is missing\")?;\nOk((a * b).to_string())"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

Alle vier Felder `name`, `description`, `args` und `code` sind Pflicht.

### Was in `code` steht

`code` ist der **Rumpf** einer Rust-Funktion mit dieser Signatur:

```rust
fn tool(args: serde_json::Value) -> Result<String, String>
```

Ausgeschrieben sieht der Code aus dem Beispiel so aus:

```rust
let a = args["a"].as_f64().ok_or("a is missing")?;
let b = args["b"].as_f64().ok_or("b is missing")?;
Ok((a * b).to_string())
```

Auch ohne Rust-Kenntnisse kommst du mit wenigen Bausteinen weit:

- `args["a"]` holt das Argument `a` aus dem JSON, das das Modell geschickt hat.
- `.as_f64()` liest es als Zahl, `.as_str()` als Text, `.as_bool()` als Wahrheitswert.
- `.ok_or("a is missing")?` bricht mit dieser Fehlermeldung ab, wenn der Wert fehlt oder den
  falschen Typ hat.
- `Ok(…)` gibt das Ergebnis als Text zurück, `Err(…)` einen Fehler. Beides landet beim Modell –
  einen Fehlertext kann es lesen und z. B. mit besseren Argumenten neu versuchen.
- `format!("{count} words")` baut Text aus Werten zusammen.

Der Typ `Value` aus `serde_json` ist schon eingebunden.

### Die Sandbox

Tool-Code läuft nicht direkt auf deinem Rechner, sondern als WebAssembly-Modul in einer
**Sandbox** – einer abgeschotteten Umgebung. Darin gilt:

- Erlaubt sind nur die Rust-Standardbibliothek und `serde_json`.
- Kein Netzwerk, kein Dateisystem, keine Umgebungsvariablen.
- Rechenzeit und Speicher sind begrenzt; jeder Aufruf startet frisch.
- Konstrukte, die beim Übersetzen auf deinen Rechner zugreifen oder Schutzmechanismen aufweichen
  könnten, lehnt grauwolf vorab ab: `include!`, `include_str!`, `include_bytes!`, `env!`,
  `option_env!`, `mod`, `extern`, `unsafe` und Attribute (`#[…]`).

Für Dateien gibt es eingebaute Tools (weiter unten), für Netzwerkdienste MCP-Server
([Kapitel 8](08-externe-tools-und-agenten.md)).

### Ausführen

```sh
grauwolf spec run docs/handbuch/beispiele/03-rechner.json "Was ist 12,5 mal 8?"
```

Etwa so:

```text
12,5 mal 8 ergibt 100.
```

Mit `--events` siehst du den Tool-Aufruf auf stderr, etwa so (gekürzt):

```text
{"event":"tool_call_starting","call":{"id":"call_1","tool":"multiply","args":{"a":12.5,"b":8}}}
{"event":"tool_call_completed","result":{"id":"call_1","tool":"multiply","content":"100"}}
```

## Bauen und Cache: `spec build`

Bevor ein Tool laufen kann, übersetzt grauwolf den Code mit `cargo` nach WebAssembly. Dafür
brauchst du die Rust-Toolchain und das Ziel `wasm32-unknown-unknown` (siehe
[Kapitel 1](01-erste-schritte.md)); beim allerersten Build lädt cargo außerdem `serde_json` aus dem
Netz.

`spec run` baut fehlende Tools automatisch. Der erste Build eines Tools dauert einige Sekunden bis
etwa eine Minute. Mit `spec build` baust du vorab, z. B. bevor du einen Agenten vorführst:

```sh
grauwolf spec build docs/handbuch/beispiele/03-rechner.json
```

Ausgabe, etwa so (eine Zeile je Tool mit Name und Prüfsumme):

```text
multiply: 3f9c2a…
```

Gebaute Tools landen in einem **Cache**, standardmäßig `.grauwolf/tools` im aktuellen Ordner;
`--cache <ordner>` wählt einen anderen. Neu gebaut wird nur, wenn sich Name, Code oder
Argument-Schema des Tools oder die Rust-Version ändern – sonst kommt das Tool sofort aus dem Cache.

`spec validate` übersetzt den Code **nicht**. Compile-Fehler zeigen sich erst bei `spec build` oder
`spec run` – mit JSON-Pointer und Zeile/Spalte im Tool-Code, z. B.:

```text
docs/handbuch/beispiele/03-rechner.json:/tools/0/code:3:5: error: …
```

Für Dienste, die fremden Code bauen, kannst du cargo zusätzlich in eine Betriebssystem-Sandbox
packen: `--build-wrapper` setzt einen Befehl davor (z. B. `bwrap`), je Teil einmal angegeben.

## Tool-Code in einer eigenen Datei: `code_file`

Code als JSON-Text mit `\n` und `\"` ist mühsam. Mit `code_file` steht er in einer normalen Datei –
mit Syntaxhervorhebung im Editor ([`beispiele/03-code-datei.json`](beispiele/03-code-datei.json)):

```json
{
  "spec_version": 1,
  "name": "adder",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du bist ein Rechenassistent. Nutze für jede Addition das Tool add und antworte auf Deutsch.",
  "tools": [
    {
      "name": "add",
      "description": "Addiert zwei Zahlen a und b",
      "args": {
        "type": "object",
        "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
        "required": ["a", "b"]
      },
      "code_file": "03-werkzeuge/addiere.rs"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

Die Datei [`beispiele/03-werkzeuge/addiere.rs`](beispiele/03-werkzeuge/addiere.rs) enthält nur den
Rumpf:

```rust
let a = args["a"].as_f64().ok_or("a is missing")?;
let b = args["b"].as_f64().ok_or("b is missing")?;
Ok((a + b).to_string())
```

```sh
grauwolf spec run docs/handbuch/beispiele/03-code-datei.json "Was ist 19 plus 23?"
```

Etwa so:

```text
19 plus 23 ergibt 42.
```

Regeln:

- Der Pfad ist **relativ zur Spec-Datei**, nicht zu dem Ordner, aus dem du den Befehl startest.
- Ein Tool hat genau eines von `code` und `code_file`; beides zusammen meldet
  `` /tools/0: give exactly one of `code` and `code_file` ``.
- Fehlt die Datei, nennt die Meldung Pointer und Pfad, z. B.
  `/tools/0/code_file: …/missing.rs: No such file or directory (os error 2)`.
- Compile-Fehler zeigen weiterhin auf `/tools/<i>/code`; Zeilen zählen ab dem Anfang der Datei.

## Ganze Tools in eigenen Dateien: `tool_files`

Willst du ein Tool in mehreren Specs nutzen, legst du die **ganze Definition** in eine eigene
JSON-Datei – eine kleine Tool-Bibliothek. Die Datei
[`beispiele/03-werkzeuge/zaehle_woerter.tool.json`](beispiele/03-werkzeuge/zaehle_woerter.tool.json)
hat dasselbe Format wie ein Eintrag in `tools`:

```json
{
  "name": "count_words",
  "description": "Zählt die Wörter eines Textes",
  "args": {
    "type": "object",
    "properties": { "text": { "type": "string" } },
    "required": ["text"]
  },
  "code_file": "zaehle_woerter.rs"
}
```

Mit dem Code in [`zaehle_woerter.rs`](beispiele/03-werkzeuge/zaehle_woerter.rs):

```rust
let text = args["text"].as_str().ok_or("text is missing")?;
let count = text.split_whitespace().count();
Ok(format!("{count} words"))
```

Die Spec bindet sie über `tool_files` ein und kann daneben eigene `tools` haben
([`beispiele/03-tool-dateien.json`](beispiele/03-tool-dateien.json)):

```json
{
  "spec_version": 1,
  "name": "text_tools",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du hilfst bei Texten. Nutze die Tools, statt selbst zu zählen oder zu rechnen.",
  "tool_files": ["03-werkzeuge/zaehle_woerter.tool.json"],
  "tools": [
    {
      "name": "add",
      "description": "Addiert zwei Zahlen a und b",
      "args": {
        "type": "object",
        "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
        "required": ["a", "b"]
      },
      "code_file": "03-werkzeuge/addiere.rs"
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

```sh
grauwolf spec run docs/handbuch/beispiele/03-tool-dateien.json "Wie viele Wörter hat der Satz: Der Igel schläft im Laub?"
```

Etwa so:

```text
Der Satz hat 5 Wörter.
```

Die Pfade in `tool_files` sind relativ zur Spec, ein `code_file` in einer Tool-Datei relativ zur
**Tool-Datei**. Beim Laden hängt grauwolf die Tools hinten an `tools` an; danach ist es eine
gewöhnliche Spec. Das Schema von `spec schema` kennt beide Felder: Bei einem Tool genügt `code`
oder `code_file`.

## Tools zur Laufzeit nachladen: `tools_dir`

Mit `tools_dir` nennst du einen Ordner, aus dem grauwolf **vor jedem Lauf** alle `*.json`-Dateien
liest (nicht in Unterordnern). Ein neues Tool ist dann nur eine neue Datei – ohne Änderung an der
Spec und ohne Neustart ([`beispiele/03-tool-ordner.json`](beispiele/03-tool-ordner.json)):

```json
{
  "spec_version": 1,
  "name": "toolbox",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du hilfst bei Texten. Nutze passende Tools, sofern vorhanden.",
  "tools_dir": "03-werkzeuge/ordner",
  "strategy": { "kind": "single_run" }
}
```

Im Ordner liegt [`umdrehen.json`](beispiele/03-werkzeuge/ordner/umdrehen.json) im selben Format wie
bei `tool_files` (mit `code` oder `code_file`):

```json
{
  "name": "reverse",
  "description": "Schreibt einen Text rückwärts",
  "args": {
    "type": "object",
    "properties": { "text": { "type": "string" } },
    "required": ["text"]
  },
  "code": "let text = args[\"text\"].as_str().ok_or(\"text is missing\")?;\nOk(text.chars().rev().collect::<String>())"
}
```

```sh
grauwolf spec run docs/handbuch/beispiele/03-tool-ordner.json "Schreib 'grauwolf' rückwärts."
```

Etwa so:

```text
Rückwärts geschrieben: flowuarg
```

So verhält sich der Ordner:

- `spec run` liest ihn einmal, `spec chat` vor jeder Zeile, `spec serve` vor jeder Anfrage.
- Gebaut wird wie oben mit Cache: Nur neue oder geänderte Tools kosten Zeit.
- Unbrauchbare Dateien (kein JSON, fehlende Felder, ungültiger Name, Compile-Fehler) werden
  übersprungen und auf stderr mit `tool folder: …` gemeldet; der Lauf geht weiter.
- Ein Tool aus dem Ordner darf kein Tool der Spec und kein eingebautes oder externes Tool
  überdecken – auch das wird übersprungen.
- Ein fehlender Ordner gilt als leer.
- `spec validate` kann die Tools im Ordner nicht prüfen, und ein eigener Graph kann sie nicht beim
  Namen nennen. Die Strategien `single_run` und `react` bieten sie an; Subgraphen mit eigener
  Tool-Liste sehen sie nicht.

## Tool-Pakete aus einer Registry: `tool_packages`

Tools können auch fertig gebaut aus einer OCI-Registry kommen – demselben Server-Typ, der
Container-Images hält (ghcr.io, Harbor, GitLab, eine lokale `registry:2`). Ein **Tool-Paket** ist ein
versioniertes Bündel aus einem oder mehreren Tools: deren Definitionen (Name, Beschreibung,
Argument-Schema) und die übersetzten WASM-Module. Du brauchst dafür keine Rust-Toolchain, und das
Modul, das läuft, ist genau das, das veröffentlicht wurde
([`beispiele/03-tool-pakete.json`](beispiele/03-tool-pakete.json)):

```json
{
  "spec_version": 1,
  "name": "pack_user",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du rechnest. Nutze für jede Rechnung die Tools des Pakets.",
  "tool_packages": [
    {
      "oci": "ghcr.io/example/grauwolf-tools/math:1.0",
      "tools": ["add", "multiply"],
      "as": { "multiply": "times" }
    }
  ],
  "strategy": { "kind": "single_run" }
}
```

- `oci` ist `registry/repository[:tag][@sha256:<digest>]`. Den Registry-Host schreibst du immer
  aus; ohne Tag und Digest ist der Tag `latest`. Nimm lieber einen Versions-Tag oder einen Digest.
- `tools` wählt Tools des Pakets aus (Standard: alle), `as` benennt sie um – nützlich, wenn zwei
  Pakete oder dein eigenes Tool denselben Namen nutzen. Namen müssen über alle Tools der Spec
  eindeutig sein.
- Die Tools laufen in derselben Sandbox wie deine eigenen: keine Dateien, kein Netz.

Schau dir ein Paket an, bevor du es nutzt, und hol es vorab:

```sh
grauwolf tools inspect ghcr.io/example/grauwolf-tools/math:1.0
grauwolf tools pull ghcr.io/example/grauwolf-tools/math:1.0
```

`inspect` zeigt den Digest, die Tool-ABI-Version und pro Tool Beschreibung, Argument-Schema und
Modul-Hash; `pull` holt das Paket, prüft es und legt es im Cache ab (`.grauwolf/tools/packages/`).

### Das Lockfile `grauwolf.lock`

Beim ersten Lauf einer Spec mit `tool_packages` (`spec run`, `chat`, `serve`, `build`, `generate`)
oder beim Sperren mit `grauwolf tools lock spec.json` löst grauwolf jede Referenz zum **Digest** des
Pakets auf und schreibt `grauwolf.lock` neben die Spec: je Referenz, wie geschrieben, den
Manifest-Digest und die SHA-256 jedes Moduls. Committe sie ins Repository. Von da an wird die
Referenz über den Digest geholt, nicht über den Tag – ein verschobener Tag ändert also nichts.
Zum Aktualisieren löschst du den Eintrag (oder die Datei).

Ein Paket, dessen Digest im Cache liegt, wird nicht erneut geholt, und jedes Modul wird bei jedem
Laden gegen seinen Hash geprüft. In CI und beim Bauen von Images nutzt du `--locked`: Der Befehl
schlägt dann fehl, statt die Lockdatei zu ändern (ein fehlender oder veralteter Eintrag ist der
Fehler; bei einem Baum von Agenten ist ein Eintrag veraltet, den kein Agent des Baums mehr nutzt).

```sh
grauwolf tools lock docs/handbuch/beispiele/03-tool-pakete.json
grauwolf spec run spec.json "Was ist 17 mal 3?" --locked
```

Etwa so:

```text
docs/handbuch/beispiele/grauwolf.lock: 1 locked (2 tools)
17 mal 3 ergibt 51.
```

### Tool-Pakete aus einem Git-Repository

Ein Paketordner (siehe [unten](#ein-eigenes-paket-veröffentlichen-tools-pack-und-tools-push)) kann
auch direkt aus einem Git-Repository genutzt werden, ohne Registry
([`beispiele/03-tool-pakete-git.json`](beispiele/03-tool-pakete-git.json)):

```json
"tool_packages": [
  {
    "git": "https://github.com/example/grauwolf-tools.git",
    "path": "text-and-math",
    "tag": "v1.0.0",
    "tools": ["add", "count_words"],
    "as": { "count_words": "word_count" }
  }
]
```

- `git` ist die Repository-URL: `https://`, oder `http://` nur für `localhost` und `127.0.0.1`.
  Keine Benutzerkennung und kein Passwort in der URL; für ein privates Repository setzt du
  `GIT_TOKEN` (siehe unten).
- `path` ist der Ordner des Pakets im Repository (relativ, ohne `..`); ohne Angabe ist die Wurzel
  des Repositorys der Paketordner.
- Genau eines von `rev` (ein Commit, vollständig oder auf mindestens 7 Hex-Ziffern gekürzt), `tag`
  oder `branch`. Ein Tag oder Branch wird einmal zu einem Commit aufgelöst und dann gesperrt;
  nimm am besten `rev` oder einen Tag.
- `tools` und `as` funktionieren wie bei `oci`.

grauwolf holt diesen Commit (flach, wenn der Server das erlaubt), liest `grauwolf-tools.json` und
`src/<tool>.rs` aus dem Ordner und **baut die Module selbst** mit dem isolierten Tool-Build. Deshalb
braucht es beim ersten Mal die Rust-Toolchain (siehe [die Build-Optionen](referenz-cli.md)).
Vorgefertigte `.wasm`-Dateien im Ordner werden ignoriert.

Das Lockfile hält den Commit und den Hash jedes Moduls unter einem Schlüssel, der die Angabe so
wiedergibt, wie sie geschrieben ist: `git+<url>[//<path>]@<rev>`, `…@tag:<tag>` oder
`…@branch:<branch>`, zum Beispiel
`git+https://github.com/example/grauwolf-tools.git//text-and-math@tag:v1.0.0`. Ein gesperrter
Eintrag wird immer über seinen Commit geholt, nie über Tag oder Branch. Das gebaute Paket wird pro
Commit und Pfad in `.grauwolf/tools/packages/` im Cache abgelegt; mit Cache und Lockfile braucht ein
späterer Lauf weder Netz noch Toolchain. Ist der Cache weg und baut eine andere Toolchain andere
Bytes, passt der Modul-Hash nicht mehr zur Sperre, und das Auflösen schlägt fehl: Git-Pakete sind
reproduzierbar über den Commit, nicht bitgenau. `--locked` verhält sich wie bei OCI.

`GIT_TOKEN` (dieselbe Variable wie für die [Git-Tools](03-tools.md)) wird nur an den Host der URL
geschickt, als HTTP-Basisauthentifizierung (Benutzer `x-access-token`), nie in Meldungen oder Logs;
ohne sie ist der Zugriff anonym. Weiterleitungen werden nicht verfolgt.

`tools pull` und `tools inspect` nehmen auch Git-Pakete an. Die Referenz ist der Schlüssel des
Lockfiles, mit immer angegebener Revision (`git+<url>[//<path>]@<commit>`, `…@tag:<tag>` oder
`…@branch:<branch>`); alles, was nicht mit `git+` beginnt, ist eine OCI-Referenz. Es gelten dieselben
Regeln wie in einer Spec (`https`, `http` nur für `localhost` und `127.0.0.1`, keine Zugangsdaten in
der URL, ein relativer Ordner):

```sh
grauwolf tools inspect git+https://github.com/example/grauwolf-tools.git//text-and-math@tag:v1.0.0
grauwolf tools pull    git+https://github.com/example/grauwolf-tools.git//text-and-math@tag:v1.0.0
```

`inspect` holt den Commit, **baut aber nichts** (keine Rust-Toolchain nötig). Es zeigt den Commit,
Paketnamen, Version und ABI, ob das Paket schon im Cache liegt (dann mit den Hashes seiner Module),
die Tools mit Beschreibung und Argumenten und die Signatur: `signed by <principal> (<key fingerprint>)`,
`unsigned` oder `signature invalid: …`. Die Signatur wird gegen `git_allowed_signers` der
[Tool-Richtlinie](#vertrauen-die-tool-richtlinie-und-signaturen) geprüft oder gegen die Datei, die
mit `--allowed-signers <datei>` angegeben wird. Ohne beides wird ein signierter Commit als
`signed, not checked` gemeldet. Wie bei OCI wendet `inspect` die Allowlist (`allowed_git_hosts`) an,
zeigt aber eine fehlende Signatur an, statt das Paket abzulehnen.

`pull` tut, was das Auflösen einer Spec für den Eintrag tut: Es prüft die Richtlinie (Host-Allowlist
und, mit `require_signatures`, die SSH-Signatur, bevor etwas gebaut wird), holt das Paket, baut es
aus den Quellen (Rust-Toolchain, `--build-wrapper` wie bei `spec build`) und legt es im Cache ab.
Die Ausgabe ist `<reference> -> commit <40 hex digits>` und die Tools. Eine Spec, die diesen Commit
(`rev`) nennt, wird dann aus dem Cache aufgelöst, ohne Netz und Toolchain; `tools lock` schreibt die
Sperre von dort.

### Registries, Anmeldungen und Klartext-http

Öffentliche Pakete werden anonym geholt. Für eine Registry, die eine Anmeldung verlangt, setzt du
`GRAUWOLF_REGISTRY_AUTH` auf eine JSON-Zuordnung von Registry zu `"user:token"` (oder zu einem nackten
Bearer-Token):

```sh
export GRAUWOLF_REGISTRY_AUTH='{"ghcr.io": "my-user:ghp_xxxxxxxx"}'
```

Bist du schon mit Docker angemeldet (`docker login`), ist nichts weiter nötig: Verlangt eine Registry
Zugangsdaten und hat `GRAUWOLF_REGISTRY_AUTH` dafür keinen Eintrag, liest grauwolf die
Docker-Konfiguration (`$DOCKER_CONFIG/config.json`, sonst `~/.docker/config.json`). Die Reihenfolge
ist `GRAUWOLF_REGISTRY_AUTH`, dann die Docker-Konfiguration; dort gewinnt ein Credential-Helper vor
gespeicherten `auths`:

```json
{
  "auths": { "registry.example.com": { "auth": "bXktdXNlcjpteS10b2tlbg==" } },
  "credHelpers": { "ghcr.io": "pass" },
  "credsStore": "desktop"
}
```

- `auths.<registry>.auth` ist Base64 von `user:password`; `identitytoken` wird als Bearer-Token
  geschickt.
- `credHelpers.<registry>` und `credsStore` nennen einen Helfer: grauwolf startet
  `docker-credential-<name> get` (in `PATH` gefunden) mit der Registry auf stdin, wie Docker es tut.
  Das ist ein Programm, das du selbst eingerichtet hast; seine Ausgabe wird nie ausgegeben oder
  protokolliert. Ein fehlender Helfer wird mit Namen gemeldet.
- Zugangsdaten werden nur abgefragt, wenn die Registry mit einer Anmeldeaufforderung antwortet;
  anonyme Pulls starten nie einen Helfer. `docker.io` wird als `https://index.docker.io/v1/`
  verstanden.

Registries werden über https angesprochen. Nur `localhost`- und `127.0.0.1`-Registries (mit beliebigem
Port) nutzen Klartext-http, wie Dockers unsicherer Standard für lokale Registries – praktisch für
eine lokale `registry:2`.

### Ein eigenes Paket veröffentlichen: `tools pack` und `tools push`

Ein **Paketordner** hat eine Manifestdatei `grauwolf-tools.json` und pro Tool eine Datei mit dem
Rumpf des Codes in `src/` – dasselbe Format wie [`code_file`](#tool-code-in-einer-eigenen-datei-code_file).
Der Ordner [`beispiele/03-paket`](beispiele/03-paket/grauwolf-tools.json) enthält zwei Tools:

```text
03-paket/
  grauwolf-tools.json      Name, Version und pro Tool: name, description, args
  src/add.rs               Rumpf des Codes des Tools add
  src/count_words.rs       Rumpf des Codes des Tools count_words
```

```json
{
  "name": "text-and-math",
  "version": "1.0.0",
  "tools": [
    { "name": "add", "description": "Addiert zwei Zahlen",
      "args": { "type": "object",
                "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
                "required": ["a", "b"] } }
  ]
}
```

(`format`, `abi` und der Modulname `<tool>.wasm` sind optional und werden abgeleitet.) `tools pack`
baut jedes Tool im isolierten Tool-Build, deshalb braucht es hier die Rust-Toolchain, und schreibt
ein OCI-Image-Layout: einen Ordner mit `oci-layout`, `index.json` und `blobs/sha256/…`, standardmäßig
`<ordner>/dist`. Die Quellen sind als letzte Schicht enthalten, und derselbe Ordner ergibt immer
dieselben Digests. `tools push` lädt das Layout hoch (oder einen Paketordner, der vorher gepackt
wird):

```sh
grauwolf tools pack docs/handbuch/beispiele/03-paket
grauwolf tools push docs/handbuch/beispiele/03-paket/dist registry.example.com/team/text-and-math:1.0
```

Vor dem Packen kannst du einen Ordner prüfen, ohne etwas zu bauen: `grauwolf tools validate
docs/handbuch/beispiele/03-paket` liest `grauwolf-tools.json` und den Code jedes Tools in `src/` mit
den Regeln von `tools pack` (einschließlich der verbotenen Konstrukte im Tool-Code), meldet jedes
Problem und endet mit 1, wenn es eines gibt.

#### Das schlanke Programm `grauwolf-tools`

Alles unter `grauwolf tools …` gibt es auch als eigenes Programm, `grauwolf-tools`, für Tool-Autoren
und CI-Pipelines, die Tools nur bauen, prüfen, packen und veröffentlichen. Es hat keine
Modellanbieter, keine Sandbox, kein MCP und keinen HTTP-Server; dadurch ist es etwa 8 MB groß und
baut in unter einer Minute. Die Befehle und Optionen sind dieselben, nur ohne das Wort `tools`:

```sh
grauwolf-tools validate docs/handbuch/beispiele/03-paket
grauwolf-tools pack docs/handbuch/beispiele/03-paket
grauwolf-tools push docs/handbuch/beispiele/03-paket registry.example.com/team/text-and-math:1.0
```

Installieren kannst du es mit `cargo install --git https://github.com/agentsw0rk/grauwolf grauwolf-tools-app`,
auf einem Mac mit `scripts/build-macos.sh --tools` bauen, oder das Image
`ghcr.io/agentsw0rk/grauwolf-tools` nutzen (`docker/tools.Dockerfile`; es enthält die Rust-Toolchain
mit dem WASM-Target, das zum Bauen von Tools nötig ist). `grauwolf-tools lock <spec>` sperrt die
Tool-Pakete und Git-Skills einer Spec und ihrer lokalen Unteragenten wie `grauwolf tools lock`, prüft
die Spec aber sonst nicht (Anbieter, Skill-Ordner); dafür nutze `grauwolf spec validate`.

Etwa so:

```text
docs/handbuch/beispiele/03-paket/dist: packed sha256:a025e887… (4 blobs)
registry.example.com/team/text-and-math:1.0@sha256:a025e887…
```

Die letzte Zeile ist die vollständige Referenz über den Digest; trag sie in eine Spec ein, um das Paket
genau festzulegen. Blobs, die die Registry schon hat, werden nicht erneut hochgeladen, eine Referenz
ohne Tag wird als `latest` gepusht, und eine Registry, die eine Anmeldung verlangt, wird mit den oben
beschriebenen Zugangsdaten beantwortet (mit Schreibrecht). Prüfe das Ergebnis mit
`grauwolf tools inspect <ref>`.

Ein Paket kann sich außerdem selbst beschreiben (`description`, `license`, `authors`, `source`) und
pro Tool strengere Sandbox-`limits` verlangen; `push` trägt jede hochgeladene Version in den
**Tool-Katalog** der Registry ein, und `grauwolf-tools new` legt einen Paketordner mit Tests und
einem CI-Workflow an – alles in [Kapitel 15](15-der-tool-katalog.md).

### Vertrauen: die Tool-Richtlinie und Signaturen

Ein Tool-Paket ist fremder Code. Die Sandbox begrenzt, was es tun kann, und eine **Tool-Richtlinie**
begrenzt, woher es kommen darf und wer dafür gebürgt haben muss. Ohne Richtlinie ist alles erlaubt
und nichts muss signiert sein. Die Richtlinie ist die Tabelle `[tool_packages]` einer `grauwolf.toml`
neben der Spec oder der Datei, die `GRAUWOLF_TOOL_POLICY` nennt (sie gewinnt). Pfade darin sind
relativ zur Richtliniendatei ([`beispiele/03-richtlinie/grauwolf.toml`](beispiele/03-richtlinie/grauwolf.toml);
alle Einstellungen in der [Konfigurationsreferenz](referenz-konfiguration.md)):

```toml
[tool_packages]
allowed_registries = ["registry.example.com/team", "ghcr.io/example"]
allowed_git_hosts = ["github.com/example"]
require_signatures = true
oci_keys = ["keys/team.pub"]
git_allowed_signers = "keys/allowed_signers"
```

- `allowed_registries` und `allowed_git_hosts` nehmen Hosts oder Präfixe (`github.com`,
  `github.com/example`; ein Präfix trifft ganze Pfadteile). Fehlt die Angabe, ist alles erlaubt,
  eine leere Liste erlaubt nichts. Ein Paket aus einer anderen Quelle wird abgelehnt, bevor eine
  Anfrage geht, **auch wenn es im Cache oder im Lockfile steht** – eine verschärfte Richtlinie wirkt
  also sofort.
- `require_signatures` verlangt auf jedem Paket eine gültige Signatur, OCI wie Git.

**Signieren von OCI-Paketen.** Erzeuge einmal ein Schlüsselpaar, signiere beim Pushen und gib den
Nutzern den öffentlichen Schlüssel:

```sh
grauwolf tools keygen team                 # schreibt team.key (geheim halten) und team.pub
grauwolf tools push docs/handbuch/beispiele/03-paket registry.example.com/team/text-and-math:1.0 \
  --sign-key team.key
mkdir keys && cp team.pub keys/            # wird von oci_keys in grauwolf.toml referenziert
```

Die Signatur ist cosign-kompatibel (ECDSA P-256 über die Simple-Signing-Nutzlast des Paket-Digests,
in der Registry neben dem Paket als `sha256-<digest>.sig` abgelegt). Deshalb akzeptiert `oci_keys`
auch öffentliche Schlüssel, die mit cosign erzeugt wurden. Keyless-Signieren wird noch nicht
unterstützt. Wird ein Paket mit `require_signatures` geholt, sucht grauwolf nach einer Signatur eines
der `oci_keys` für genau diese Registry, dieses Repository und diesen Digest. Die Signaturen werden
mit dem Cache-Eintrag gespeichert, damit die Prüfung auch offline funktioniert; eine geänderte
Schlüsselliste gilt für gecachte Pakete ohne Netz; und mit `--locked` wird nichts geholt, um ein
früher gecachtes Paket zu prüfen. Ein unsigniertes Paket wird mit einer Meldung abgelehnt, die es
nennt:

```text
error: `registry.example.com/team/text-and-math@sha256:a025e887…` does not satisfy the tool policy
(`require_signatures`): unsigned
```

`grauwolf tools inspect <ref> [--key team.pub]` zeigt den Status, ohne abzulehnen:
`signed by SHA256:…`, `unsigned` oder `signature invalid`. Für Git-Pakete siehe
[oben](#tool-pakete-aus-einem-git-repository): `tools inspect git+… [--allowed-signers <datei>]`.

**Signieren von Git-Paketen.** Der Commit (`rev`, `branch`) oder der Tag muss eine SSH-Signatur
tragen, wie sie `git commit -S` bzw. `git tag -s` mit `gpg.format = ssh` erzeugt. `git_allowed_signers`
ist eine Datei im Format von gits `gpg.ssh.allowedSignersFile`:

```text
alice@example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI… alice
*@example.com namespaces="git" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI… team
```

Die Principals werden mit der E-Mail-Adresse des Committers (oder Taggers) abgeglichen; `*` und `?`
funktionieren. Die einzige unterstützte Option ist `namespaces` (muss `git` enthalten). GPG- und
X.509-Signaturen werden nicht unterstützt. Ein Eintrag mit `rev` oder `branch` braucht einen
signierten Commit, ein Eintrag mit `tag` einen signierten **annotierten** Tag oder sonst einen
signierten Commit (ein Lightweight-Tag auf einem unsignierten Commit wird abgelehnt). Die Prüfung
findet **bevor etwas gebaut wird** statt. Die rohen Commit- und Tag-Objekte werden mit dem
Cache-Eintrag gespeichert, sodass ein gecachtes Paket ohne Netz erneut geprüft wird. Erfüllen sie
eine geänderte Richtlinie nicht (oder wurden sie vor der Richtlinie gecacht), holt grauwolf sie erneut.
Das verbietet `--locked` (die Meldung sagt es; einmal ohne `--locked` auflösen). Ein Git-Eintrag unter
`require_signatures` braucht `git_allowed_signers`, sonst wird er mit diesem Hinweis abgelehnt.

### Generierte Crates und Images

`spec generate` holt die Pakete und bettet ihre Module wie lokale Tools ein. Das generierte Crate und
das Image-Bauen (`--dockerfile`) brauchen weder Registry noch Netz. Nutze `--locked`, wenn du in CI
generierst.

### Was wann geprüft wird

- `spec validate` prüft die Syntax der Referenzen und die Namen, die es ohne Netz kennt: die Tools,
  die du in `tools` nennst, nach dem Umbenennen, untereinander und gegenüber deinen anderen Tools.
  Es holt nie etwas. Nenne die gewünschten Tools in `tools`, wenn die Tool-Liste eines Subgraphen
  auf sie verweist.
- Beim Auflösen des Pakets: die Digests von Manifest und Modulen, dass es ein grauwolf-Tool-Paket ist,
  die Tool-ABI-Version (es gibt nur `1`), dass die ausgewählten Tools existieren und dass keine Namen
  kollidieren.
- Mit einer [Tool-Richtlinie](#vertrauen-die-tool-richtlinie-und-signaturen): zuerst die erlaubten
  Registries und Git-Hosts (vor Cache, Lockfile oder Netz), dann mit `require_signatures` die
  Signatur.

## Skills: Anleitungen, die der Agent bei Bedarf lädt: `skills`

Ein *Skill* ist ein Ordner mit Anleitungen: eine `SKILL.md` und, wenn du magst, Dateien daneben
(Referenzen, Vorlagen, Beispiele). Lange Anleitungen gehören nicht in den System-Prompt, wo sie den
Kontext jeder Anfrage füllen würden. Mit Skills sieht der Agent nur eine Liste aus Namen und
Beschreibungen und lädt die Anleitung eines Skills, wenn eine Aufgabe sie braucht.

```
skills/
  haiku-writer/
    SKILL.md
    references/signature.md
```

```markdown
---
name: haiku-writer
description: Schreibt ein Haiku und signiert es. Nutze es, wenn jemand ein Gedicht oder ein Haiku verlangt.
---
Antworte mit genau einem Haiku im Muster 5-7-5 Silben, eine Zeile pro Vers.
Füge nach dem Gedicht eine letzte Zeile mit der Signatur aus `references/signature.md` hinzu.
```

Die Front-Matter braucht `name` (gleich dem Namen des Ordners) und `description`; der Rest der Datei
sind die Anleitungen. Die Spec nennt die *Quellen* von Skills:

```json
{
  "system_prompt": "Du bist ein hilfreicher Assistent. Prüfe deine Fähigkeiten, bevor du antwortest.",
  "skills": [{ "dir": "03-skills" }]
}
```

(`beispiele/03-skills.json`, mit dem Skill-Ordner `beispiele/03-skills/`.) Jeder Unterordner von
`dir`, der eine `SKILL.md` enthält, ist ein Skill; `"names": ["haiku-writer"]` nimmt nur einige davon.
`dir` ist relativ zur Spec-Datei.

Was der Agent bekommt, sobald `skills` gesetzt ist:

- Einen Block am Ende seines System-Prompts: eine Zeile `- <name>: <description>` je Skill und der
  Hinweis, `load_skill` aufzurufen.
- Das Tool `load_skill { name }`: liefert die Anleitung ohne Front-Matter und die Liste der übrigen
  Dateien des Skills. Ein unbekannter Name ergibt einen Fehler, der die bekannten Skills auflistet.
- Das Tool `read_skill_file { name, path }`: liefert eine Textdatei des Skills (höchstens 1 MiB; der
  Pfad bleibt im Skill-Ordner; Binärdateien werden mit einer Meldung abgelehnt).
- Das Ereignis `skill_loaded { name }` nach jedem erfolgreichen `load_skill` (bei `--events`, im
  JSONL-Log und als Span-Ereignis in den OpenTelemetry-Spans).

Die beiden Tools lesen nur. Skripte in einem Skill-Ordner führen sie nie aus. Ihre Namen sind
reserviert (ein eigenes Tool namens `load_skill` ist ein Validierungsfehler), und der Agent darf
daneben Dateitools haben.

### Skills aus einem Git-Repository

```json
"skills": [
  { "dir": "03-skills" },
  { "git": "https://github.com/example/grauwolf-skills.git", "path": "writing",
    "tag": "v1.0.0", "names": ["pirate-translator"] }
]
```

(`beispiele/03-skills-git.json`.) Eine Git-Quelle funktioniert wie ein Git-[Tool-Paket](#tool-pakete-aus-einem-git-repository):
`https://` (`http://` nur für `localhost`), keine Benutzerangabe in der URL (private Repositorys nutzen
`GIT_TOKEN`), `path` ist der Ordner mit den Skill-Ordnern (Standard: Wurzel des Repositorys), und
genau eines von `rev`, `tag` und `branch`. Der Commit wird im Abschnitt `skills` der `grauwolf.lock`
neben der Spec festgehalten, zusammen mit einem Hash der Dateien. `grauwolf tools lock <spec>` schreibt
ihn (derselbe Befehl sperrt auch `tool_packages`), und `spec run`, `spec serve` und die anderen lösen
einen fehlenden Eintrag beim Start auf. Mit `--locked` wird die Lockdatei nie geändert, und eine
gesperrte Quelle wird aus dem Cache gelesen (`.grauwolf/tools/packages/skills-<commit>…`), ohne
Netzzugriff; die Dateien im Cache werden bei jeder Nutzung geprüft. Die
[Tool-Richtlinie](#vertrauen-die-tool-richtlinie-und-signaturen) gilt auch: `allowed_git_hosts` und mit
`require_signatures` ein Commit oder Tag, der von einem der `git_allowed_signers` signiert ist.

### Was wann geprüft wird

- `spec validate`: die Form jeder Quelle (genau eines von `dir` und `git`, eine brauchbare URL oder
  ein Ordner, gültige Namen) und bei lokalen Ordnern die Ordner selbst: eine `SKILL.md` mit `name` und
  `description`, der Front-Matter-`name` gleich dem Ordnernamen, jede Datei höchstens 1 MiB und kein
  symbolischer Link, der aus seinem Skill-Ordner hinausführt. Skill-Namen müssen über alle Quellen
  eindeutig sein. Git-Quellen werden beim Holen geprüft.
- Ein Lauf, `spec serve` und `spec build` machen dieselben Prüfungen beim Start; ein kaputter Skill
  stoppt sie vor der ersten Anfrage, mit dem Dateinamen.

### Generierte Crates und Images

Skills werden nicht in das Programm eingebaut. `spec generate --dockerfile` kopiert lokale
Skill-Ordner nach `app/` (neben die Wissensquellen, gleicher relativer Pfad), sodass das Image sie nur
lesend enthält. Git-Quellen werden zur Generierungszeit geholt (in CI `--locked` verwenden) und nach
`app/skills-git/<n>/` kopiert; die Spec im Crate nennt diese Kopien. Image-Build und laufender Dienst
brauchen daher weder Git noch Netz. Dasselbe gilt für die Skills von Unteragenten: Der gesamte
[Agentenbaum](08-externe-tools-und-agenten.md#agenten-als-tools-agent_tools) wird kopiert. Ohne
`--dockerfile` liest ein generiertes Crate lokale Skill-Ordner neben seiner Spec wie der Interpreter,
und Git-Quellen werden abgelehnt.

## Eingebaute Tools: `builtin_tools`

Manche Tools bringt grauwolf fertig mit. Sie laufen nicht in der Sandbox, sondern mit den Rechten des
grauwolf-Prozesses – deshalb wählst du jedes einzeln aus.

| Tool | Argumente | Wirkung |
|---|---|---|
| `read_file` | `path` | Datei lesen |
| `list_directory` | `path` | Ordnerinhalt auflisten (Ordner enden mit `/`) |
| `write_file` | `path`, `content` | Datei schreiben, fehlende Ordner anlegen |
| `edit_file` | `path`, `old`, `new` | `old` durch `new` ersetzen; `old` muss genau einmal vorkommen |
| `search_files` | `pattern`, optional `path`, `max_results` | Textdateien mit einem regulären Ausdruck durchsuchen |
| `say_to_user` | `message` | dem Benutzer etwas mitteilen |
| `ask_user` | `question` | den Benutzer etwas fragen und die Antwort zurückgeben |

Die ersten fünf sind **Dateitools** und brauchen `root`: den Ordner (relativ zur Spec), in dem sie
arbeiten dürfen. Pfade sind relativ zu diesem Ordner; absolute Pfade, `..` und Verknüpfungen
(Symlinks) nach draußen werden abgelehnt. `search_files` liefert Treffer als `pfad:zeile: text`,
standardmäßig höchstens 50 (`max_results` bis 200), und überspringt versteckte Einträge, Symlinks,
Binärdateien und Dateien über 1 MiB.

`say_to_user` und `ask_user` sind **Benutzer-Tools**: In der CLI schreiben sie auf stderr und lesen
die Antwort als Zeile von stdin.

Ein Agent, der Notizen liest und bei Unklarheiten nachfragt
([`beispiele/03-dateien.json`](beispiele/03-dateien.json)):

```json
{
  "spec_version": 1,
  "name": "notes_helper",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du hilfst bei den Notizen im Arbeitsordner. Schlage nach, bevor du antwortest, und frage nach, wenn etwas unklar ist. Antworte auf Deutsch.",
  "builtin_tools": {
    "tools": ["list_directory", "read_file", "search_files", "ask_user"],
    "root": "03-werkzeuge/arbeitsordner"
  },
  "strategy": { "kind": "single_run" }
}
```

```sh
grauwolf spec run docs/handbuch/beispiele/03-dateien.json "Wann ist mein Zahnarzttermin?"
```

Etwa so:

```text
Dein Zahnarzttermin ist am Dienstag um 9 Uhr.
```

Die Spec gibt dem Agenten bewusst nur lesende Dateitools. Schreibende Tools (`write_file`,
`edit_file`) solltest du nur auswählen, wenn der Agent wirklich schreiben soll – und am besten
zusammen mit einer Freigabe (nächster Abschnitt).

`spec validate` prüft die Auswahl, etwa so:

```text
bt.json:/builtin_tools/tools/1: unknown tool 'shell'
bt.json:/builtin_tools/root: built-in file and key-value tools need `root` (directory relative to the spec)
```

## Denken, Webabrufe, Notizen und Aufgaben

Weitere eingebaute Tools helfen Agenten, die über mehrere Schritte arbeiten. Sie werden wie die
anderen in `builtin_tools.tools` aufgeführt ([`beispiele/03-abruf-notizen.json`](beispiele/03-abruf-notizen.json)).

| Tool | Argumente | Wirkung |
|---|---|---|
| `think` | `thought` | gibt dem Modell Raum zum Nachdenken zwischen Tool-Aufrufen; antwortet `ok`, ändert nichts |
| `fetch` | `url`, optional `method`, `headers`, `body` | HTTP-Anfrage; braucht `builtin_tools.fetch` |
| `store_value` | `key`, `value` | Textwert im Schlüssel-Wert-Speicher ablegen (überschreibt) |
| `retrieve_value` | `key` | Wert lesen (`Key not found: <key>`, wenn es keinen gibt) |
| `list_keys` | – | alle Schlüssel auflisten (`No keys stored`, wenn leer) |
| `add_task` | `title` | Aufgabe zur Aufgabenliste hinzufügen (`Added task #1: <title>`) |
| `complete_task` | `id` | Aufgabe als erledigt markieren |
| `remove_task` | `id` | Aufgabe entfernen |
| `list_tasks` | – | Aufgaben als `#1 [x] <title>` auflisten (`No tasks`, wenn leer) |

**`fetch`** ist standardmäßig sicher, weil die Spec die Hosts nennen muss:

```json
"fetch": { "allowed_hosts": ["example.com", "*.wikipedia.org"], "allow_http": false }
```

- Ein Host ist erlaubt, wenn er genau aufgeführt ist oder auf `*.domain` passt (alle Subdomains, nicht
  die Domain selbst). Alles andere wird mit einer Meldung abgelehnt, die den Host nennt.
- Nur `https://` funktioniert, außer `allow_http` ist `true`.
- Weiterleitungen werden verfolgt (höchstens fünf), aber jedes Ziel wird erneut gegen die Liste
  geprüft.
- Der Hostname wird zuerst aufgelöst, und die Verbindung geht zur geprüften Adresse. Löst er zu einer
  privaten, Loopback- oder Link-Local-Adresse auf (z. B. `127.0.0.1`, `10.x.x.x`, `169.254.169.254`),
  wird die Anfrage abgelehnt – es sei denn, genau dieser Host ist aufgeführt (`"localhost"`,
  `"10.0.0.5"`). So kommt ein Agent nicht in deinen Cluster.
- Die Anfrage läuft nach 30 Sekunden ab. Die Antwort ist die Statuszeile (`HTTP 200 OK`), die
  Kopfzeilen `content-type` und `content-length` und der Body als Text, nach 1 MiB abgeschnitten.
  HTML wird auf seinen Text reduziert (Skripte, Stile und Tags entfernt).
- `spec validate` meldet einen fehlenden Abschnitt (`MissingBuiltinSection`), eine leere Liste
  (`EmptyAllowedHosts`) und Einträge, die keine Hosts sind (`InvalidAllowedHost`).

### `fetch` mit SPIFFE: `fetch.auth`

In einem Cluster mit SPIRE können interne Dienste prüfen, *welcher Agent* sie aufruft, statt ein gemeinsames
Token zu verlangen. Gib so einem Host eine Identität (ADR-0110):

```json
"fetch": {
  "allowed_hosts": ["reviewer.agents.svc", "*.tools.svc", "example.com"],
  "auth": {
    "reviewer.agents.svc": { "spiffe": { "id": "spiffe://agentswork.de/ns/agents/sa/reviewer" } },
    "*.tools.svc": { "spiffe": { "mode": "jwt", "audience": "tools" } }
  }
}
```

- Jeder Schlüssel von `auth` muss ein Eintrag von `allowed_hosts` sein (gleich geschrieben, auch Muster wie
  `*.tools.svc`); der Wert hat die Form von [`auth.spiffe`](referenz-spec.md#authspiffe-in-agent_toolsid-und-mcp_serversid)
  (`mtls` als Standard oder `jwt` mit `audience`; bei `mtls` genau eines von `id`, `ids`, `trust_domain`).
- Eine Anfrage an so einen Host zeigt das X.509-SVID des Agenten und akzeptiert den Server nur, wenn seine
  SPIFFE-ID passt (`mtls`), oder sendet ein frisches JWT-SVID als `Authorization: Bearer` (`jwt`). Andere Hosts,
  wie `example.com` oben, werden ohne Identität abgerufen; ebenso der nächste Sprung einer Weiterleitung, die
  einen Identitäts-Host verlässt.
- **Identitäts-Hosts werden nur über `https://` aufgerufen.** Eine URL mit `http://` wird abgelehnt
  (`'<host>' needs a workload identity (`auth`), which only works over https://`), auch mit `allow_http: true`:
  es gibt kein stilles Zurückfallen auf unverschlüsseltes HTTP. (`allow_http` gilt für alle Hosts, deshalb prüft
  das der einzelne Aufruf und nicht `spec validate`.)
- Ein abgelehnter Handshake nennt den Grund, wie bei MCP-Servern und A2A: `TLS handshake with <host> failed: the
  server is <id>, expected <id>; the request was not sent`.
- Das Modell sieht nie ein Zertifikat oder ein Token: Den Aufruf macht der Host-Prozess. Ohne
  `SPIFFE_ENDPOINT_SOCKET` startet der Agent nicht; der Fehler nennt `builtin_tools.fetch.auth.<host>`.

**Schlüssel-Wert-Speicher:** eine JSON-Datei `kv.json` in `root` (deshalb ist `root` Pflicht). Mit
einem Benutzer (`--user` oder die Benutzer des Dienstes, wie beim Gedächtnis in
[Kapitel 7](07-gespraeche-und-wissen.md)) bekommt jeder Benutzer seine eigene Datei
`users/<hex>/kv.json`. Schlüssel haben höchstens 256 Byte, Werte 64 KiB; Schreibvorgänge sind atomar.
Der Speicher hält genaue Werte unter genauen Schlüsseln und ist nicht das semantische
Langzeitgedächtnis (`remember`/`recall`). Die Datei ist für die Dateitools oben unter demselben
`root` sichtbar.

**Aufgabenliste:** Die Liste gehört zum Lauf und liegt im Agenten-Speicher. Sie ist also Teil der
Checkpoints und bleibt nach einem `--resume` erhalten. IDs sind ganze Zahlen, die nie wiederverwendet
werden.

## Jira, Confluence und Git

Drei weitere Gruppen eingebauter Tools arbeiten mit Systemen außerhalb des Rechners. Jede Gruppe
braucht einen eigenen Abschnitt in `builtin_tools`; ein Tool aufzuführen, ohne den Abschnitt seiner
Gruppe, ist ein Spec-Fehler (`MissingBuiltinSection`). Wie bei den übrigen eingebauten Tools werden nur
die aufgeführten Tools angeboten.

| Gruppe | Tools | Abschnitt |
|---|---|---|
| Jira | `jira_search_issues { jql, max_results }`, `jira_get_issue { issue_key }`, `jira_create_issue { project_key, issue_type, summary, description }`, `jira_add_comment { issue_key, comment }`, `jira_transition_issue { issue_key, status_name }` | `"jira": { "base_url": "https://<site>.atlassian.net" }` |
| Confluence | `confluence_search_pages { cql, limit }`, `confluence_get_page { page_id }`, `confluence_create_page { space_key, title, body, parent_id }`, `confluence_update_page { page_id, title, body }` | `"confluence": { "base_url": "https://<site>.atlassian.net" }` |
| Git | `git_checkout { ref }`, `git_list_branches`, `git_create_branch { name }`, `git_commit { message }`, `git_push` | `"git": { "repo_url": "https://…", "directory": "repo", "push_policy": "new_branches", "use_token": true }` |

**Jira und Confluence (Atlassian Cloud).** `base_url` muss `https://` sein; gezählt werden nur Schema
und Host, sodass auch eine eingefügte Adresse mit `…/wiki` funktioniert. Die Zugangsdaten kommen aus
der Umgebung und werden vor dem Lauf gelesen: `ATLASSIAN_EMAIL` (die Adresse des Kontos) und
`ATLASSIAN_API_TOKEN` (ein API-Token). Jedes Ergebnis ist `HTTP <status>` und ein Zeilenumbruch,
gefolgt vom Body der Antwort – auch bei Fehlern, damit das Modell sieht, was schiefging. Issue-Schlüssel
(`ABC-123`) und Seiten-IDs (Ziffern) werden vor der Anfrage geprüft. Jira-Texte gehen als Atlassian
Document Format hinaus: ein Absatz pro Zeile. Jira Server und Data Center werden nicht abgedeckt.

**Git.** Ein festes Repository pro Spec: `repo_url` (`https://`, ohne Benutzername und Passwort),
`directory` (unterhalb von `root`, oder das Verzeichnis der Spec ohne `root`; Standard `repo`). Die
Tools sind Teil von grauwolf (libgit2 mit eigenem HTTP-Transport). Es braucht kein `git`-Programm,
auch nicht im Container-Image. `git_checkout` klont beim ersten Aufruf und holt danach nach. Dann
wechselt es auf einen Branch (vorgespult auf den Remote), einen Tag oder einen Commit-Hash (losgelöst).
Es verweigert den Lauf bei uncommitteten Änderungen, beim Verlassen eines Branches mit nicht
gepushten Commits und wenn ein Branch vom Remote abweicht. Die Verweigerungen beginnen mit `Refused:`
und sagen dem Modell, was zu tun ist. `git_create_branch` legt einen neuen Branch am aktuellen Commit
an; `git_commit` stagt alles und committet auf dem aktuellen Branch; `git_push` pusht den aktuellen
Branch unter demselben Namen, nie mit force.

- **Nur-Lese-Repository:** führe nur `git_checkout` und `git_list_branches` auf.
- **`push_policy`:** `new_branches` (Standard) lässt `git_push` nur Branches pushen, die der Agent in
  diesem Prozess mit `git_create_branch` angelegt hat; `any` erlaubt jeden Branch.
- **`use_token`:** mit `true` wird das Zugangstoken aus `GIT_TOKEN` genutzt (vor dem Lauf gelesen). Es
  wird im Speicher als `Authorization`-Kopfzeile geschickt (Benutzer `x-access-token`), nur für
  Netzbefehle und nur an Schema und Host von `repo_url`. Es erscheint nie in einer URL, einem Ergebnis
  oder einem Log. Commits verwenden `GIT_AUTHOR_NAME` und `GIT_AUTHOR_EMAIL` (Standard
  `grauwolf agent`, `agent@grauwolf.local`).
- **Wache:** Bevor es holt, auflistet oder pusht, prüft grauwolf, dass `.git/config` weiterhin genau
  `repo_url` als `origin` nennt, ohne `pushurl` und ohne `url.*.insteadOf`. So kann ein Dateitool, das
  in denselben Ordner schreibt, das Token nicht zu einem anderen Server umleiten; der Aufruf wird
  abgelehnt.
- **Weiterleitungen werden abgelehnt:** Die Tools folgen keiner HTTP-Weiterleitung (auch nicht auf
  denselben Host). Ein Server, der mit einer antwortet – etwa bei einem umgezogenen Repository –,
  liefert `Error: git <command> failed: the server answered with a redirect (HTTP 301); redirects are not
  followed`. Nimm die endgültige URL als `repo_url`. Nur Server, die das Smart-HTTP-Protokoll sprechen,
  funktionieren.
- Aufrufe auf einem Ordner laufen nacheinander, Netzaufrufe laufen nach 120 Sekunden ab, und Fehler
  kommen als Text zum Modell zurück (`Error: git <command> failed: …`).

### git mit SPIFFE: `git.auth`

Steht dein Git-Server hinter einem Proxy, der die SPIFFE-ID des Aufrufers prüft und auf einen Git-Benutzer
abbildet, können die Git-Tools statt `GIT_TOKEN` die Identität des Agenten verwenden (ADR-0110):

```json
"git": {
  "repo_url": "https://git.tools.svc:8443/team/service.git",
  "auth": { "git.tools.svc": { "spiffe": { "id": "spiffe://agentswork.de/ns/git/sa/git-proxy" } } }
}
```

- Der Schlüssel muss der Host von `repo_url` sein (oder ein Muster wie `*.tools.svc`, das ihn trifft;
  sonst `AuthHostNotRepository`); der Wert ist ein [`auth.spiffe`](referenz-spec.md#authspiffe-in-agent_toolsid-und-mcp_serversid)-Block.
- Clone, Fetch und Push zeigen das X.509-SVID (`mtls`, die ID des Servers wird geprüft) oder senden ein
  frisches JWT-SVID als `Authorization: Bearer` (`jwt`). Der Server antwortet mit `403` auf eine Identität,
  die er nicht abbildet, und auf eine Nur-Lese-Identität, die pusht (`access denied (HTTP 403)`).
- `auth` und `use_token` schließen sich aus (`GitTokenWithAuth`): Die Identität ersetzt `GIT_TOKEN`.
- Abgedeckt ist nur das Repository der Tools selbst. Die Git-Quellen von Tool-Paketen und Skills werden beim
  Bauen aufgelöst, meist außerhalb des Clusters, und verwenden `auth` noch nicht.
- Zum Ausprobieren ohne eigenen Proxy dient der Test-Git-Server in
  [`examples/test-git-server/`](../../examples/test-git-server/README.md): Er stellt ein Repository über mTLS
  bereit, mit einer Mapping-Datei von SPIFFE-IDs auf Benutzer (nur für Tests).

Ein Agent, der an einem Ticket arbeitet ([`beispiele/03-jira-git.json`](beispiele/03-jira-git.json)):

```json
{
  "spec_version": 1,
  "name": "ticket_worker",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du arbeitest an Jira-Tickets. Lies das Ticket, hole das Repository, nimm die Änderung auf einem neuen Branch vor, committe und pushe sie und kommentiere dann das Ticket mit dem Branch-Namen. Antworte auf Deutsch.",
  "builtin_tools": {
    "tools": [
      "jira_get_issue", "jira_add_comment",
      "git_checkout", "git_list_branches", "git_create_branch", "git_commit", "git_push",
      "read_file", "write_file"
    ],
    "root": "work",
    "jira": { "base_url": "https://example.atlassian.net" },
    "git": {
      "repo_url": "https://github.com/example/service.git",
      "directory": "service",
      "push_policy": "new_branches",
      "use_token": true
    }
  },
  "strategy": { "kind": "react" }
}
```

```sh
export ATLASSIAN_EMAIL=me@example.com ATLASSIAN_API_TOKEN=… GIT_TOKEN=…
grauwolf spec run docs/handbuch/beispiele/03-jira-git.json "Arbeite an Ticket ABC-42"
```

Ohne eine dieser Variablen startet der Lauf nicht:

```text
03-jira-git.json: the Jira tools need Atlassian Cloud credentials: ATLASSIAN_EMAIL is not set (ATLASSIAN_EMAIL is the account's e-mail address, ATLASSIAN_API_TOKEN an API token)
```

Diese Tools verändern Systeme Dritter. Gib dem Agenten nur die Tools, die er braucht, bevorzuge
`push_policy: new_branches` und erwäge `--confirm-tools` (nächster Abschnitt). Dienste, die aus einer
solchen Spec entstehen, laufen im selben kleinen Image wie jeder andere Dienst.

## Befehle ausführen

Das eingebaute Tool `run_command` lässt den Agenten Programme starten, zum Beispiel `git`, `ls` oder
`python3`. Es ist das einzige eingebaute Tool mit Seiteneffekten auf Prozessebene. Deshalb ist es aus,
außer die Spec schaltet es ausdrücklich ein: Du führst `run_command` auf, setzt ein `root` und fügst
einen Abschnitt `command` hinzu ([`beispiele/03-befehl.json`](beispiele/03-befehl.json)):

```json
{
  "spec_version": 1,
  "name": "file_counter",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du beantwortest Fragen zu den Dateien im Arbeitsordner. Nutze run_command mit den Programmen ls und wc; rate nicht. Antworte auf Deutsch.",
  "builtin_tools": {
    "tools": ["run_command"],
    "root": "03-werkzeuge/arbeitsordner",
    "command": {
      "allowed": ["ls", "wc"],
      "timeout_secs": 20,
      "max_output_bytes": 16384,
      "env": ["PATH", "LANG"]
    }
  },
  "strategy": { "kind": "single_run" }
}
```

**Allowlist-Modus (Standard).** Das Modell ruft `run_command { program, args?, cwd? }` auf, zum
Beispiel `{ "program": "wc", "args": ["-l", "README.md"] }`.

- Nur Programme aus `allowed` starten: ein nackter Name (im `PATH` der geleerten Umgebung gesucht,
  siehe unten) oder ein absoluter Pfad, der genau aufgeführt ist. Alles andere wird mit einer Meldung
  abgelehnt, die die erlaubten Programme auflistet.
- Das Programm startet **direkt**, ohne Shell: `;`, `|`, `*` und `$HOME` in einem Argument bleiben
  wörtlicher Text.
- `cwd` ist relativ zu `root` und muss darin bleiben (kein absoluter Pfad, kein `..`, kein Symlink nach
  draußen) – dieselben Regeln wie bei den Dateitools. Standard ist `root` selbst.
- Die Umgebung wird **geleert**: Das Programm sieht nur die Variablen, die in `env` genannt sind
  (Standard `["PATH"]`), mit ihren Werten aus dem Prozess. API-Schlüssel und Tokens bleiben unsichtbar,
  solange du sie nicht auflistest. Liste `PATH` auf, wenn nackte Programmnamen gefunden werden sollen.
- Die Standardeingabe ist geschlossen. stdout und stderr werden erfasst, jeweils nach
  `max_output_bytes` abgeschnitten (Standard 65536, höchstens 10 MiB) mit einem Hinweis.
- Nach `timeout_secs` (Standard 60, höchstens 3600) wird die ganze Prozessgruppe beendet, einschließlich
  allem, was das Programm gestartet hat.
- Das Ergebnis beginnt mit `exit code: N`, `terminated by signal N` oder `timed out after N s and was
  killed`, gefolgt von den Abschnitten `stdout:` und `stderr:`.
- Keine Freigabe nötig: Die Liste ist die Freigabe.

`spec validate` prüft den Abschnitt: Er ist für das Tool Pflicht (`MissingBuiltinSection`), `root`
ebenso (`MissingBuiltinRoot`). `allowed` darf nicht leer sein (`EmptyAllowedCommands`), ein Programm ist
ein nackter Name oder ein absoluter Pfad (`InvalidAllowedProgram`), `*` gibt es nur im Shell-Modus
(`WildcardWithoutShell`), und die Grenzen müssen im erlaubten Bereich liegen (`CommandLimitOutOfRange`).

**Shell-Modus (nur ausdrücklich, und riskant).** Wenn das Modell Pipes, Globbing oder Variablen braucht:

```json
"command": { "allowed": ["*"], "shell": true, "i_understand_shell": true }
```

Das Tool nimmt dann `{ command }` und führt es mit `sh -c` in `root` aus, mit derselben geleerten
Umgebung, Zeitgrenze und Ausgabegrenze. Es ist eine Shell mit den Rechten des grauwolf-Prozesses und
ohne Betriebssystem-Sandbox: Das Modell kann Dateien löschen, die dein Benutzer erreicht, lesen, was die
Umgebung der `PATH`-Programme lesen kann, und jeden Dienst erreichen, den dein Rechner erreicht. Deshalb
sind drei bewusste Einstellungen nötig (`shell`, `allowed: ["*"]`, `i_understand_shell`; jede andere
Kombination ist ein Validierungsfehler), und jedes `spec validate` sowie jeder Befehl, der die Spec
lädt, gibt eine Warnung aus.

- **Freigabe je Aufruf.** Im Shell-Modus braucht jeder Aufruf eine Freigabe (Standard
  `"approve_each_call": true`). `spec run` und `spec chat` fragen vor jedem Aufruf auf der Konsole nach
  (`The agent wants to run the shell command … Run? [y/N]`). `spec serve` fragt über `/approvals`, wie
  bei [`--confirm-tools`](#vor-tools-nachfragen---confirm-tools) beschrieben. Ein Lauf ohne jemanden, der
  freigeben kann, lehnt den Aufruf ab und sagt es dem Modell. Unbeaufsichtigte Dienste können die
  Freigabe mit `"approve_each_call": false` abschalten.
- **Generierte Crates** sperren die Shell wie den Interpreter (ADR-0119): Der Aufruf wird ohne
  Freigeber in `RunOptions` abgelehnt, und ein generierter Dienst fragt über `/approvals` auf
  `/run/stream`.
- **Container.** Wenn du einen Agenten mit diesem Tool als Dienst betreibst, härte den Container: ein
  Nicht-root-Benutzer, ein Root-Dateisystem nur zum Lesen mit einem beschreibbaren Volume nur unter der
  Wurzel, kein Service-Account-Token (`automountServiceAccountToken: false`), keine Rechteausweitung,
  abgelegte Capabilities und eine Netzwerkrichtlinie, die nur den Modell-Endpunkt und die Hosts erlaubt,
  die der Agent braucht. AgentWeaver setzt das für jeden Harness automatisch, dessen Spec `run_command`
  nutzt.
- Bevorzuge die Allowlist. `python3` oder `sh` in die Liste aufzunehmen ist dasselbe wie der Shell-Modus.

**Die Identität des Agenten für ein Programm: `workload_api`.** Ein Programm, das sich selbst per SPIFFE anmeldet
(etwa ein CLI für einen internen Dienst), braucht den Socket der Workload API, den die geleerte Umgebung aber
versteckt. Nenne die Programme und setze (ADR-0110):

```json
"command": { "allowed": ["internal-cli"], "workload_api": true }
```

`SPIFFE_ENDPOINT_SOCKET` (und `SPIFFE_FETCH_TIMEOUT`, falls gesetzt) kommt dann in die Umgebung der erlaubten
Programme. Sie handeln **als der Agent**: SPIRE unterscheidet die Prozesse eines Pods nicht. Fehlt der Socket zur
Laufzeit, startet der Agent nicht (`builtin_tools.command.workload_api: … SPIFFE_ENDPOINT_SOCKET is not set`). Im
Shell-Modus ist die Kombination ein Validierungsfehler (`WorkloadApiWithShell`): Eine Shell, die das Modell
steuert, darf nicht mit der Identität des Agenten gegenüber internen Diensten handeln.

Das Tool braucht ein Unix-System (Prozessgruppen, `sh`); anderswo startet der Lauf nicht. Es gehört zum
Facade-Feature `command` (standardmäßig an); ein Build ohne es lehnt eine Spec mit `run_command` vor dem
Lauf ab.

## Vor Tools nachfragen: `--confirm-tools`

Mit `--confirm-tools` fragt grauwolf auf der Konsole vor jedem Aufruf der genannten Tools (kommagetrennt):

```sh
grauwolf spec run docs/handbuch/beispiele/03-dateien.json "Was steht in notizen.md?" --confirm-tools read_file
```

Etwa so:

```text
Run tool read_file with {"path":"notizen.md"}? [y/N] y
Deine Notizen enthalten eine Einkaufsliste (Mehl, Eier, Milch), einen Zahnarzttermin am Dienstag um 9 Uhr und eine Idee für einen Rezept-Agenten.
```

`y` oder `yes` erlauben den Aufruf (`j` und `ja` funktionieren ebenfalls). Alles andere lehnt ihn ab.
Das Modell erhält dann als Ergebnis `The user refused the call of read_file.` und kann darauf
reagieren. Die Option wirkt auf jedes Tool, auch auf eigene. Wie Freigaben im Dienstbetrieb funktionieren,
zeigt [Kapitel 9](09-betrieb.md).

## Selbst gebaute Tools

Ein Agent kann sich während eines Laufs auch selbst neue Tools schreiben: Mit `self_tools` bekommt er ein
Tool `create_tool`, grauwolf baut den Code in die Sandbox, und standardmäßig bestätigt ein Mensch jedes
neue Tool. Wie du das sicher und kontrolliert machst, zeigt [Kapitel 10](10-selbst-gebaute-tools.md).

## Das Wichtigste in Kürze

- Ein Tool hat `name`, `description`, `args` (JSON-Schema) und Code; das Modell entscheidet, wann es
  aufgerufen wird.
- Der Code ist der Rumpf von `fn tool(args: serde_json::Value) -> Result<String, String>` – nur std und
  `serde_json`, in einer Sandbox ohne Netz und Dateien.
- `spec run` baut Tools automatisch, `spec build` vorab; der Cache liegt in `.grauwolf/tools`.
  `spec validate` kompiliert nicht.
- `code_file` lagert den Code aus, `tool_files` ganze Tool-Definitionen; Pfade sind relativ zur Datei,
  die sie nennt.
- `tools_dir` lädt Tools vor jedem Lauf aus einem Ordner; Unbrauchbares wird übersprungen und gemeldet.
- `tool_packages` importiert fertig gebaute Tools aus einer OCI-Registry (`oci`, `tools`, `as`);
  `grauwolf.lock` pinnt die Digests, `--locked` verbietet das Ändern, `GRAUWOLF_REGISTRY_AUTH` oder die
  Docker-Konfiguration halten die Zugangsdaten. `tools pack` und `tools push` veröffentlichen deinen
  eigenen Paketordner. Eine Tool-Richtlinie (`grauwolf.toml`) kann Registries und Git-Hosts einschränken
  und Signaturen verlangen (`tools keygen`, `tools push --sign-key`, signierte Git-Commits und Tags).
- `skills` nennt Skill-Ordner (`dir`, oder `git` mit `rev`/`tag`/`branch`); der Agent bekommt eine
  Skill-Liste in seinem System-Prompt und die lesenden Tools `load_skill` und `read_skill_file`. Git-Quellen
  werden in `grauwolf.lock` gesperrt (`tools lock`), generierte Images enthalten Kopien in `app/`.
- `builtin_tools` bringt Dateitools (nur unter `root`) und die Benutzer-Tools `say_to_user`/`ask_user`.
- `think`, `fetch` (Hosts aus `builtin_tools.fetch`, nur HTTPS, keine privaten Adressen außer wenn
  aufgeführt), `store_value`/`retrieve_value`/`list_keys` (`kv.json` unter `root`) und die Aufgabentools
  (Liste pro Lauf, Teil der Checkpoints) runden die eingebauten Tools ab.
- Jira-, Confluence- und Git-Tools brauchen ihren Abschnitt in `builtin_tools` und Zugangsdaten aus der
  Umgebung (`ATLASSIAN_EMAIL`, `ATLASSIAN_API_TOKEN`, `GIT_TOKEN`); fehlt eine, startet der Lauf nicht.
- `--confirm-tools a,b` fragt vor diesen Tools nach; die Ablehnung erhält das Modell als Ergebnis.

Weiter mit [Kapitel 4: Abläufe](04-ablaeufe.md).
