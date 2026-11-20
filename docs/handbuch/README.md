# grauwolf-Handbuch für Nutzer

> **Deutsche Fassung, Stand 15.11.2026 – entspricht der englischen Fassung von diesem Tag.**
> Die gepflegte Fassung ist englisch: [User guide](../user-guide/README.md).
> grauwolf gibt seine Meldungen und die eingebauten Anweisungen an das Modell auf Englisch aus; die Zitate in diesem Handbuch zeigen sie englisch.


Dieses Handbuch zeigt Schritt für Schritt, wie du mit grauwolf Agenten beschreibst, ausführst und
betreibst – ohne eine Zeile Rust schreiben zu müssen. Jedes Kapitel baut auf den vorherigen auf und
endet mit einem Beispiel, das du direkt ausprobieren kannst. Alle Beispiel-Specs liegen in
[`beispiele/`](beispiele/) und werden bei jedem Testlauf automatisch geprüft.

## Lernpfad

| Kapitel | Thema | Du lernst … |
|---|---|---|
| [1](01-erste-schritte.md) | Erste Schritte | grauwolf einrichten, den ersten Agenten prüfen, ausführen und mit ihm chatten |
| [2](02-modelle.md) | Modelle und Anbieter | Ollama, OpenAI (und kompatible Anbieter), Anthropic und Gemini nutzen, mehrere Modelle mischen, Kosten und Budget |
| [3](03-tools.md) | Tools | Dem Agenten Fähigkeiten geben: eigener Tool-Code, Tool-Dateien, Tool-Ordner, eingebaute Tools (Dateien, Jira, Confluence, git) |
| [4](04-ablaeufe.md) | Abläufe | Vorlagen (`single_run`, `react`, `verified`, Moderation) und eigene Graphen aus Nodes und Kanten |
| [5](05-strukturierte-daten.md) | Strukturierte Daten | Eigene Typen für Ein- und Ausgabe statt reinem Text |
| [6](06-subgraphen-und-planer.md) | Subgraphen und Planer | Abläufe zerlegen, parallel arbeiten, planen lassen (`plan`, GOAP) |
| [7](07-gespraeche-und-wissen.md) | Gespräche, Gedächtnis und Wissen | Gespräche fortsetzen, Langzeitgedächtnis, eigene Dokumente durchsuchen (RAG) |
| [8](08-externe-tools-und-agenten.md) | Externe Tools und andere Agenten | MCP-Server anbinden, Agenten als Tools anderer Agenten (lokal und über A2A) |
| [9](09-betrieb.md) | Betrieb | Agent als Dienst, Benutzer und Tokens, Freigaben, Beobachtung, Checkpoints |
| [10](10-selbst-gebaute-tools.md) | Selbst gebaute Tools | Den Agenten während des Laufs eigene Tools bauen lassen – sicher und kontrolliert |
| [11](11-code-erzeugen.md) | Code erzeugen | Aus einer Spec eine eigenständige Rust-Crate machen und Specs per LLM erzeugen |
| [12](12-zugangsdaten-aus-openbao.md) | Zugangsdaten aus OpenBao mit SPIFFE | Agenten holen Tokens mit ihrer SPIFFE-Identität von einem Secret-Broker statt aus der Umgebung |
| [13](13-cluster-einrichten.md) | Cluster einrichten mit grauwolf-k8-ops | `doctor`, ein Namespace und eine OpenBao-Rolle pro Agent, Secrets, ein Prüf-Job – die Schritte aus Kapitel 12 als Befehle |
| [14](14-im-auftrag-von-personen.md) | Im Auftrag von Personen handeln | Delegation mit Entra ID und Txn-Tokens: Begriffe, Voraussetzungen, Einrichtung Schritt für Schritt, der Ablauf zur Laufzeit als Diagramme, der Identitätsdienst, Betrieb (Richtlinie, Schlüsselrotation, Deinstallation), Fehlersuche aus der Live-Prüfung, Entra Local |
| [15](15-der-tool-katalog.md) | Der Tool-Katalog | Paket-Metadaten und Sandbox-Grenzen, der Katalog einer Registry (`grauwolf-tools catalog list/search/show/add/remove/sync`), beurteilt mit deiner Tool-Richtlinie, ein Tool-Repository mit CI aus `grauwolf-tools new` |
| [16](16-grauwolf-studio.md) | grauwolf studio | Specs auf einer Zeichenfläche zeichnen: die Palette aus dem Knotenkatalog, Validierung am Knoten, die Layout-Datei neben der Spec, gemeinsam mit einem Coding-Agenten über MCP zeichnen |

## Nachschlagen

- [CLI-Referenz](referenz-cli.md) – alle Befehle und Optionen von `grauwolf`
- [Spec-Referenz](referenz-spec.md) – alle Felder einer Spec mit Standardwerten
- [Konfigurationsreferenz](referenz-konfiguration.md) – `grauwolf.toml`: die Tool-Richtlinie und die Quellen für Zugangsdaten; die Richtliniendatei des Identitätsdienstes

Wer grauwolf selbst erweitern oder als Rust-Bibliothek nutzen möchte, findet alles Weitere im
[Entwicklerhandbuch](../entwickler/README.md).
