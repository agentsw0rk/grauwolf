# 10 Selbst gebaute Tools

In [Kapitel 3](03-tools.md) hast du Tools selbst geschrieben – als Code in der Spec, als Tool-Datei oder im Tool-Ordner. Manchmal weißt du aber vorher nicht, welches Tool ein Agent brauchen wird: eine bestimmte Datumsberechnung, eine Umrechnung, einen Textfilter. Mit `self_tools` darf der Agent solche Tools **während des Laufs selbst bauen** und sofort nutzen.

Das ist mächtig – und braucht Kontrolle. In diesem Kapitel lernst du

- wie du `self_tools` einschaltest und begrenzt,
- wie du jeden Bau vorher prüfst und freigibst,
- wie gebaute Tools für spätere Läufe erhalten bleiben,
- wie du den Bau auf einzelne Subgraphen beschränkst,
- was daran sicher ist und wo die Grenzen liegen.

## Der erste Werkzeugbauer

[`beispiele/10-werkzeugbauer.json`](beispiele/10-werkzeugbauer.json):

```json
{
  "spec_version": 1,
  "name": "tool_builder",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du löst Aufgaben genau. Wenn dir ein Tool fehlt, baue es mit create_tool und verwende es dann.",
  "self_tools": { "max_tools": 3 },
  "strategy": { "kind": "single_run" }
}
```

Schon `"self_tools": {}` genügt: Der Agent bekommt dann zusätzlich das Tool `create_tool`. Ohne das Feld bekommt kein Agent dieses Tool.

```bash
grauwolf spec run docs/handbuch/beispiele/10-werkzeugbauer.json \
  "Wie viele Vokale hat das Wort Donaudampfschifffahrt?"
```

Was jetzt passiert:

1. Das Modell ruft `create_tool` mit vier Argumenten auf: `name` (snake_case), `description`, `args` (JSON-Schema der Argumente) und `code` (Rust).
2. Weil `confirm` standardmäßig eingeschaltet ist, fragt grauwolf dich zuerst (siehe nächster Abschnitt).
3. grauwolf baut den Code wie die Tools aus Kapitel 3 zu einem WebAssembly-Modul.
4. Das Modell bekommt `Tool 'count_vowels' is available from now on.` – ab der nächsten Anfrage wird das neue Tool angeboten.
5. Das Modell ruft das neue Tool auf und antwortet.

Etwa so:

```text
Das Wort "Donaudampfschifffahrt" hat 6 Vokale.
```

### Was das Modell schreiben muss

Wie in Kapitel 3 ist der Code der **Rumpf** der Funktion

```rust
fn tool(args: serde_json::Value) -> Result<String, String>
```

zum Beispiel:

```rust
let word = args["word"].as_str().ok_or("word is missing")?;
Ok(word.chars().filter(|c| "aeiouAEIOU".contains(*c)).count().to_string())
```

Modelle schreiben oft die ganze Funktion inklusive Signatur. Auch das wird akzeptiert: Beginnt der Code mit `fn tool(`, ruft grauwolf die Funktion selbst auf. Erlaubt sind nur `std` und `serde_json`; es gibt kein Netzwerk, keine Dateien und kein `unsafe`. Fehlt `args`, gilt `{ "type": "object" }`.

### Fehler gehen zurück an das Modell

Geht etwas schief, bricht der Lauf **nicht** ab. Der Fehler kommt als Tool-Ergebnis zurück, und das Modell kann es beheben:

| Situation | Meldung an das Modell (Beispiel) |
|---|---|
| name ist nicht snake_case | `name 'CountVowels' is not a snake_case identifier` |
| name ist schon vergeben | `tool 'count_vowels' already exists` |
| zu viele Tools in diesem Lauf | `at most 3 new tools per run` |
| Code zu lang | `code is longer than 20000 bytes` |
| Fehler beim Kompilieren | die Meldungen des Compilers mit Zeile und Spalte im Tool-Code |
| Bau abgelehnt | `The user refused to build the tool 'count_vowels'.` |

## Die Einstellungen

| Feld | Standard | Bedeutung |
|---|---|---|
| `enabled` | `true` | Darf der Agent außerhalb von Subgraphen bauen? (siehe [Subgraphen](#nur-in-bestimmten-subgraphen-bauen)) |
| `tool` | `"create_tool"` | Name des Bau-Tools; teilt sich den Namensraum mit allen anderen Tools. |
| `max_tools` | `5` | Höchstens so viele neue Tools pro Lauf. |
| `max_code_bytes` | `20000` | Höchstens so viele Bytes Code pro Tool. |
| `confirm` | `true` | Ein Mensch bestätigt jedes neue Tool, bevor es gebaut wird. |
| `save` | `false` | Gebaute Tools im `tools_dir` speichern, damit spätere Läufe sie haben. |

Jeder Bau braucht Zeit (Sekunden mit warmem Cache, beim ersten Mal Minuten), und jede Reparaturschleife kostet Tokens. `max_tools` und ein Budget (`--max-tokens`, `--max-cost`, siehe [Kapitel 2](02-modelle.md)) halten das im Rahmen.

## Jeden Bau freigeben

Mit `confirm: true` – dem Standard – zeigen `spec run` bzw. `spec chat` vor jedem Bau Name, Beschreibung und Code auf stderr und warten auf deine Antwort:

```text
The agent wants to build the tool 'count_vowels': Counts the vowels of a word
---
let word = args["word"].as_str().ok_or("word is missing")?;
Ok(word.chars().filter(|c| "aeiouAEIOU".contains(*c)).count().to_string())
---
Build? [y/N]
```

`y` oder `yes` baut das Tool (`j` und `ja` funktionieren auch); alles andere lehnt es ab, und das Modell wird informiert.

Wo kein Mensch antworten kann, wird der Bau **abgelehnt**:

- im Dienst über `/run`, A2A und MCP – über `/run/stream` kommt dagegen ein `approval`-Ereignis für das Tool `create_tool`, über das du mit `POST /approvals/{id}` entscheidest, wie in [Kapitel 9](09-betrieb.md#freigaben-ein-mensch-entscheidet); die Argumente im Ereignis enthalten den Code;
- bei `spec mcp`.

Checkpoint-Läufe (`spec run --run-id`, `spec resume`) fragen wie gewöhnliche Läufe auf der Konsole nach.

Nur wenn du dem Agenten wirklich vertraust – etwa in einer abgeschotteten Testumgebung – schaltest du die Abfrage mit `"confirm": false` ab.

## Gebaute Tools behalten

Normalerweise lebt ein selbst gebautes Tool nur bis zum Ende des Laufs. Mit `save: true` speichert grauwolf jedes erfolgreich gebaute Tool im Tool-Ordner der Spec ([`beispiele/10-werkzeugbauer-speichern.json`](beispiele/10-werkzeugbauer-speichern.json)):

```json
{
  "spec_version": 1,
  "name": "tool_builder_save",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du löst Aufgaben genau. Nutze vorhandene Tools; fehlt eines, baue es mit create_tool.",
  "tools_dir": "10-werkzeuge",
  "self_tools": { "max_tools": 2, "max_code_bytes": 5000, "save": true },
  "strategy": { "kind": "single_run" }
}
```

`save` braucht ein `tools_dir`; fehlt es, meldet `spec validate`:

```text
10-werkzeugbauer-speichern.json:/self_tools/save: `save` needs a `tools_dir` to store the tools in
```

Nach einem Bau enthält der Ordner `10-werkzeuge/` (relativ zur Spec, bei Bedarf angelegt) zwei Dateien – im selben Format wie die Tool-Dateien aus [Kapitel 3](03-tools.md):

```text
10-werkzeuge/count_vowels.json   name, description, args und "code_file": "count_vowels.rs"
10-werkzeuge/count_vowels.rs     der Code
```

Beim nächsten Lauf lädt grauwolf den Tool-Ordner wie gewohnt (bei `spec chat` vor jeder Zeile, im Dienst vor jeder Anfrage an `/run` und `/run/stream`), und `count_vowels` ist von Anfang an da. Weil der Name dann belegt ist, baut der Agent es nicht noch einmal. Sieh dir gespeicherte Tools an, bevor du sie dauerhaft behältst – du kannst sie wie jede Tool-Datei bearbeiten oder löschen.

## Nur in bestimmten Subgraphen bauen

Oft soll nur ein bestimmter Teil des Ablaufs bauen dürfen. Dafür gibt es zwei Schalter:

- `self_tools.enabled` steuert den Hauptgraphen. Mit `false` sieht er weder `create_tool` noch selbst gebaute Tools; die übrigen Einstellungen (Grenzen, Freigabe, `save`) gelten trotzdem.
- Jeder Subgraph ([Kapitel 6](06-subgraphen-und-planer.md)) kann `"self_tools": true` oder `false` setzen. Fehlt die Angabe, gilt, was außerhalb gilt.

[`beispiele/10-werkstatt.json`](beispiele/10-werkstatt.json) erlaubt das Bauen nur im Subgraphen `calculate`; der nachfolgende Node `summarize` sieht keine Tools:

```json
{
  "spec_version": 1,
  "name": "workshop",
  "model": { "provider": "ollama", "id": "qwen3.5:9b" },
  "system_prompt": "Du rechnest zuerst und fasst dann zusammen.",
  "self_tools": { "enabled": false, "max_tools": 2 },
  "subgraphs": {
    "calculate": {
      "tools": [],
      "self_tools": true,
      "graph": {
        "nodes": [
          { "id": "call_llm", "kind": "llm_request" },
          { "id": "execute_tool", "kind": "execute_tool" },
          { "id": "send_result", "kind": "llm_send_tool_result" },
          { "id": "execute_tools_in_order", "kind": "execute_multiple_tools", "params": { "sequential": true } },
          { "id": "send_results", "kind": "llm_send_multiple_tool_results" }
        ],
        "edges": [
          { "from": "start", "to": "call_llm" },
          { "from": "call_llm", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
          { "from": "call_llm", "to": "execute_tools_in_order", "when": { "kind": "on_multiple_tool_calls" } },
          { "from": "call_llm", "to": "finish", "when": { "kind": "on_assistant_message" } },
          { "from": "execute_tool", "to": "send_result" },
          { "from": "send_result", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
          { "from": "send_result", "to": "execute_tools_in_order", "when": { "kind": "on_multiple_tool_calls" } },
          { "from": "send_result", "to": "finish", "when": { "kind": "on_assistant_message" } },
          { "from": "execute_tools_in_order", "to": "send_results" },
          { "from": "send_results", "to": "execute_tool", "when": { "kind": "on_tool_call" } },
          { "from": "send_results", "to": "execute_tools_in_order", "when": { "kind": "on_multiple_tool_calls" } },
          { "from": "send_results", "to": "finish", "when": { "kind": "on_assistant_message" } }
        ]
      }
    }
  },
  "graph": {
    "nodes": [
      { "id": "calculate", "kind": "subgraph", "params": { "subgraph": "calculate" } },
      { "id": "summarize", "kind": "llm_request" }
    ],
    "edges": [
      { "from": "start", "to": "calculate" },
      { "from": "calculate", "to": "summarize" },
      { "from": "summarize", "to": "finish", "when": { "kind": "on_assistant_message" } }
    ]
  }
}
```

### Wer sieht was?

Für `create_tool` und alle selbst gebauten Tools gelten dieselben Regeln:

| Wo | sichtbar? |
|---|---|
| Hauptgraph | wenn `self_tools.enabled` (Standard `true`) |
| Subgraph mit `"self_tools": true` | ja – auch wenn seine `tools`-Liste sie nicht nennt (im Beispiel `"tools": []`) |
| Subgraph mit `"self_tools": false` | nein |
| Subgraph ohne `self_tools` und ohne `tools`-Liste | wie außerhalb |
| Subgraph ohne `self_tools`, aber mit `tools`-Liste | nein – die Liste blendet sie aus, auch wenn sie `create_tool` nennt |
| Node ohne Tools (z. B. `llm_reason`) | nein |

Die in einem Lauf gebauten Tools liegen an einem gemeinsamen Ort: Ein Tool, das im Subgraphen `calculate` gebaut wurde, kann auch in jedem anderen Subgraphen genutzt werden, in dem Bauen erlaubt ist.

`self_tools` in einem Subgraphen ohne `self_tools` oben in der Spec ist ein Fehler:

```text
spec.json:/subgraphs/calculate/self_tools: `self_tools` in a subgraph needs `self_tools` at the top of the spec
```

## Sicherheit

Selbst gebauter Code läuft genauso abgeschottet wie die Tools aus Kapitel 3: in einer **WebAssembly-Sandbox** ohne Netzwerk und ohne Dateisystem. Ein Tool kann nur Text aus seinen Argumenten berechnen.

Das eigentliche Risiko ist der **Bau**: Der Code wird mit `cargo` auf deiner Maschine kompiliert. grauwolf sichert ihn in mehreren Schichten ab:

1. Vor dem Bau werden verbotene Konstrukte abgelehnt: `include!` und Verwandte, `env!`, `option_env!`, `mod`, `extern`, `unsafe` und Attribute.
2. `cargo` läuft mit einer leeren Umgebung (nur die Pfade der Toolchain) und mit `--offline`.
3. Jeder Bau hat eine Zeitgrenze (300 s).
4. Optional läuft `cargo` in einem weiteren Wrapper, den du mit `--build-wrapper` angibst – je Teil einmal, z. B.:

   ```bash
   grauwolf spec serve docs/handbuch/beispiele/10-werkzeugbauer.json \
     --build-wrapper bwrap --build-wrapper --unshare-net
   ```

   Weitere Argumente des Wrappers hängst du auf dieselbe Weise an (bei bwrap etwa die eingebundenen Verzeichnisse), jeweils mit einem eigenen `--build-wrapper`.

   Für Dienste, in denen ein Modell Code schreibt, wird ein solcher Wrapper (Bubblewrap, Docker oder ähnlich) **dringend empfohlen**.

Dazu kommt die Freigabe mit `confirm`: Ein Mensch sieht den Code, bevor er gebaut wird.

Der Bau braucht die Rust-Toolchain samt Target `wasm32-unknown-unknown` auf der Maschine, auf der der Agent läuft – wie in Kapitel 3 beschrieben. Gebaute Module landen im Cache (`--cache`, Standard `.grauwolf/tools`); derselbe Code wird nicht zweimal gebaut.

## Grenzen

- **Checkpoints:** Selbst gebaute Tools stehen mit ihrer Definition im Checkpoint. `spec resume` baut sie vor dem Fortsetzen neu (meist aus dem Bau-Cache); scheitert das, bricht das Fortsetzen mit einer Meldung ab.
- **Codeerzeugung:** `spec generate` lehnt Specs mit `self_tools` ab, weil sie den Bau und die Sandbox zur Laufzeit bräuchten ([Kapitel 11](11-code-erzeugen.md)).
- **Unteragenten** (`agent_tools`) haben keinen Rückkanal für Freigaben; baut ein Unteragent mit `confirm`, wird der Bau abgelehnt.

## Das Wichtigste in Kürze

- `self_tools` in der Spec gibt dem Agenten das Tool `create_tool`; er schreibt Rust-Code, grauwolf baut ihn in die Sandbox, und das neue Tool steht ab der nächsten Anfrage zur Verfügung.
- Grenzen setzt du mit `max_tools` und `max_code_bytes`; Fehler gehen als Tool-Ergebnis an das Modell zurück.
- `confirm` (Standard) legt jeden Bau zur Freigabe vor – auf der Konsole oder über `/run/stream`; ohne Rückkanal wird er abgelehnt.
- `save` speichert gebaute Tools im `tools_dir`.
- `enabled` und `self_tools` je Subgraph bestimmen, wo Tools gebaut und genutzt werden dürfen.
- Der Bau läuft auf deiner Maschine – nutze für Dienste `--build-wrapper`.

Weiter mit [Kapitel 11: Code erzeugen](11-code-erzeugen.md).
