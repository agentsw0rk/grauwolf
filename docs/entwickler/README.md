# grauwolf-Handbuch für Entwickler

> **Deutsche Fassung, Stand 15.11.2026 – entspricht der englischen Fassung von diesem Tag.**
> Die gepflegte Fassung ist englisch: [User guide](../user-guide/README.md) (bzw. [Developer guide](../developer-guide/README.md)).
> grauwolf gibt seine Meldungen und die eingebauten Anweisungen an das Modell auf Englisch aus; die Zitate in diesem Handbuch zeigen sie englisch.


Dieses Handbuch richtet sich an alle, die grauwolf als Rust-Bibliothek nutzen, erweitern oder am Framework selbst mitarbeiten. Es setzt voraus, dass du die Grundbegriffe aus dem [Nutzerhandbuch](../handbuch/README.md) kennst (Spec, Node, Kante, Tool, Strategie). Die Kapitel bauen aufeinander auf: erst das Gesamtbild, dann die Arbeit am Code, dann die Interna.

## Lernpfad

| Kapitel | Thema | Du lernst … |
|---|---|---|
| [1](01-architektur.md) | Architektur | Schichten, Crates, wie ein Lauf von der Spec bis zur Antwort fließt |
| [2](02-entwicklungsumgebung.md) | Entwicklungsumgebung | Bauen, testen, prüfen; TDD, Commits, ADRs und `AGENTS.md` |
| [3](03-rust-api.md) | grauwolf als Bibliothek | Agenten aus Rust laden und starten, typisierte Tools mit `#[tool]`, Graphen im Code |
| [4](04-tools-intern.md) | Tools von innen | Tool-Vertrag, Registry, Build zu WASM, Sandbox, Laufzeit-Tools |
| [5](05-erweitern.md) | Erweitern | Eigene Node-Arten und Bedingungen, eigene LLM-Anbieter, Ports und Adapter |
| [6](06-generator.md) | Generator | Wie aus einer Spec eine Crate wird und warum Interpreter und Generator gleich laufen müssen |
| [7](07-ereignisse-und-beobachtung.md) | Ereignisse und Beobachtung | Events, Features, Interceptors, Freigaben, OpenTelemetry |
| [8](08-server-und-protokolle.md) | Server und Protokolle | HTTP-Dienst, A2A, MCP – Aufbau und Erweiterung |
| [9](09-identitaet-zugangsdaten-und-cluster-werkzeug.md) | Identität, Zugangsdaten und Cluster-Werkzeug | Workload-Identität über SPIFFE, Zugangsdaten aus OpenBao, Delegation mit Txn-Tokens und das Kubernetes-Programm `grauwolf-k8-ops` |
| [10](10-grauwolf-studio.md) | grauwolf studio | Das Studio-Backend und seine API, die Abbildung Spec ↔ Zeichenfläche und die automatische Anordnung, der erzeugte TypeScript-Client, die Arbeitskopie mit ihren Operationen und der MCP-Endpunkt |

## Weiterführend

- [Architektur-Übersicht](../architecture.md)
- [Architekturentscheidungen (ADRs)](../adr/README.md)
- [Feature-Katalog](../features/README.md)
