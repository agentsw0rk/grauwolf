# 13 Cluster einrichten mit grauwolf-k8-ops

[Kapitel 12](12-zugangsdaten-aus-openbao.md) erklärt, wie ein Agent mit seiner SPIFFE-Identität seine
Zugangsdaten aus OpenBao bekommt, und das [Beispiel](../../examples/spire-openbao/README.md) listet die
manuellen Schritte auf. `grauwolf-k8-ops` erledigt diese Schritte für dich: Es ist ein kleines Programm
(getrennt von `grauwolf`, damit der Kubernetes-Client nicht in die Agent-Laufzeit gelangt), das mit deinem
Cluster über deine kubeconfig spricht und mit OpenBao über ein Admin-Token. Jeder Befehl lässt sich erneut
ausführen; er ändert nur, was abweicht, und meldet für jedes Objekt `created`, `updated` oder `unchanged`.

Was es **nicht** tut: SPIRE, spire-controller-manager, den SPIFFE-CSI-Treiber oder OpenBao installieren.
`doctor` prüft, dass sie vorhanden sind.

## Das Modell: ein Agent, ein Namespace

Ein Agent namens `reviewer` bekommt

- einen Namespace `reviewer` (überschreibbar mit `--namespace`) mit den Labels `grauwolf.io/agent=reviewer`
  und `app.kubernetes.io/managed-by=grauwolf-k8-ops` und dem erzwungenen Pod Security Standard `restricted`;
- einen Service-Account `reviewer`, sodass seine Pods die SPIFFE-ID
  `spiffe://<trust domain>/ns/reviewer/sa/reviewer` bekommen (eine `ClusterSPIFFEID` namens `grauwolf-agents`
  wählt alle Namespaces mit dem Agenten-Label aus);
- eine OpenBao-Rolle und Policy `agent-reviewer`: Die Rolle nimmt genau diese SPIFFE-ID an, die Policy
  liest nur `kv/data/agents/reviewer/*` – der Agent kann die Geheimnisse anderer Agenten nicht lesen;
- eine ConfigMap `grauwolf-config` mit dem Abschnitt `[credentials]` seiner `grauwolf.toml`
  (siehe [Kapitel 12](12-zugangsdaten-aus-openbao.md) und die [Konfigurationsreferenz](referenz-konfiguration.md)).

Das Werkzeug fasst nie einen Namespace ohne diese Labels an: `agent add` verweigert einen bestehenden Namespace,
der nicht dem Agenten gehört, und `agent remove` löscht nur die eigenen des Agenten.

## Was du brauchst

- Eine kubeconfig, die Namespaces, ConfigMaps, Jobs und `ClusterSPIFFEID`s anlegen darf (`doctor` prüft das
  mit Access Reviews). `--kubeconfig <datei>`, sonst `KUBECONFIG`, sonst `~/.kube/config`; `--context
  <name>` wählt einen anderen Kontext als den aktuellen (ein nicht vorhandener Kontext ist ein Fehler, der die
  verfügbaren auflistet). `doctor` beginnt mit der Zeile `Cluster <Server-URL> (context <name>)`, damit du siehst,
  um welchen Cluster es geht, bevor es etwas prüft; die anderen Befehle wiederholen sie nicht (ihre Ausgabe wird
  oft kopiert oder abgefangen).
- Ein laufendes SPIRE (Server, Agenten, spire-controller-manager, SPIFFE-CSI-Treiber) und OpenBao, initialisiert
  und entsiegelt.
- Ein OpenBao-Token, das Auth-Methoden, Mounts, Policies und Rollen konfigurieren darf: `export
  OPENBAO_TOKEN=...` oder `--token-file <datei>` (nie als Argument). Das Root-Token funktioniert, aber nach dem
  ersten `openbao setup` genügt ein eingeschränktes ([unten](#ohne-das-root-token-arbeiten)). Standardmäßig wird OpenBao über den
  Service-Proxy der Kubernetes-API erreicht (`/api/v1/namespaces/openbao/services/http:openbao:8200/proxy/`),
  daher ist kein Ingress nötig; `--openbao-addr http://host:8200` erreicht es direkt.

Die Standardwerte passen zum [Beispiel](../../examples/spire-openbao/README.md): SPIRE in `spire-system`, der
OIDC-Discovery-Provider in `spire-oidc`, OpenBao in `openbao` (Service `openbao:8200`), die Mounts `jwt` und `kv`,
Audience `openbao`. Jeder Wert lässt sich mit einer globalen Option ändern (`--spire-namespace`,
`--oidc-namespace`, `--openbao-namespace`, `--openbao-service`, `--jwt-mount`, `--kv-mount`, `--audience`;
`--agent-openbao-address` ist die Adresse, die in die Konfiguration der Agenten geschrieben wird).

## Durchgang

```console
$ grauwolf-k8-ops doctor
Prerequisites (not installed by grauwolf-k8-ops)
  ok    cluster: reachable, Kubernetes v1.34.1+k3s1
  ok    rights on namespaces: get, list, create, delete allowed (agent add/remove)
  ...
  ok    SPIRE server: StatefulSet spire-system/spire-server ready (1/1)
  ok    OpenBao: reachable, initialized, unsealed (version 2.7.2)
  ok    OpenBao token: valid; may configure the auth method, mount, policies, roles and agent secrets
  warn  OpenBao token policy: working with the root token
        next: create a restricted one with `grauwolf-k8-ops openbao token`
Setup (installed by grauwolf-k8-ops)
  fail  OIDC discovery provider: Deployment spire-oidc/spire-oidc-discovery-provider not found
        next: run `grauwolf-k8-ops oidc install`
  fail  JWT auth method: auth/jwt/ is not enabled
        next: run `grauwolf-k8-ops openbao setup`
```

`doctor` ändert nichts. Es gibt je Prüfung eine Zeile aus – `ok`, `warn` oder `fail` mit dem Grund und, wo es
einen gibt, dem nächsten Schritt – in zwei Abschnitten: den **Voraussetzungen** (was das Werkzeug nicht
installiert: Cluster, Rechte, SPIRE, CSI-Treiber, CRD, Controller-Manager, OpenBao und dessen Zustand, die
Rechte des Tokens) und der **Einrichtung** (was es installiert: der Discovery-Provider und seine Schlüssel, die
JWT-Auth-Methode und ihre `jwks_url`, der KV-Mount, die `ClusterSPIFFEID`, die Trust Domain an jeder Stelle, die
sie enthält, und für jeden Agenten Namespace, Service-Account, Rolle, Policy und ConfigMap). Der Exit-Code ist 1,
wenn eine Prüfung fehlschlägt; Warnungen zählen nicht. `--json` gibt ein Array von
`{section, check, status, reason, hint}` aus.

```console
$ grauwolf-k8-ops oidc install
trust domain agentswork.de, SPIRE 1.15.5; discovery provider in namespace spire-oidc
Namespace spire-oidc: created
...
$ grauwolf-k8-ops openbao setup
auth method jwt/ (jwks_url http://spire-oidc-discovery-provider.spire-oidc.svc/keys): created
KV v2 mount kv/: created
OpenBao policy grauwolf-k8-ops (what this tool needs, for `openbao token`): created
$ grauwolf-k8-ops agent add reviewer
trust domain agentswork.de; identity spiffe://agentswork.de/ns/reviewer/sa/reviewer
namespace reviewer: created
service account reviewer/reviewer: created
ClusterSPIFFEID grauwolf-agents: created
ConfigMap reviewer/grauwolf-config: created
OpenBao policy agent-reviewer (reads kv/data/agents/reviewer/*): created
OpenBao role auth/jwt/role/agent-reviewer (subject spiffe://agentswork.de/ns/reviewer/sa/reviewer): created
$ grauwolf-k8-ops secret set reviewer jira --variable ATLASSIAN_API_TOKEN
Value for reviewer/jira (hidden):
secret kv/agents/reviewer/jira#value: stored (version 1, 32 bytes)
variable ATLASSIAN_API_TOKEN -> openbao:kv/agents/reviewer/jira#value in reviewer/grauwolf-config: updated
$ grauwolf-k8-ops agent check reviewer --image registry.example.com/grauwolf:1.0
running Job reviewer/grauwolf-check-3fa9c1 (check of ATLASSIAN_API_TOKEN, image registry.example.com/grauwolf:1.0)
Job grauwolf-check-3fa9c1: deleted
agent reviewer: resolved ATLASSIAN_API_TOKEN from openbao (32 bytes)
```

1. **`oidc install`** installiert (oder übernimmt, wenn du es aus dem Beispiel angelegt hast) den SPIRE-OIDC-
   Discovery-Provider in seinem eigenen Namespace (spire-controller-manager ignoriert `spire-system`), mit der
   Image-Version des laufenden SPIRE-Servers und seiner eigenen `ClusterSPIFFEID`. Die Trust Domain kommt aus der
   Konfiguration des SPIRE-Servers, der einzigen Quelle der Wahrheit.
2. **`openbao setup`** aktiviert die JWT-Auth-Methode mit der `jwks_url` des Providers (`--jwks-url`,
   `--bound-issuer`, wenn der SPIRE-Server `jwt_issuer` setzt) und den KV-v2-Mount und schreibt die Policy
   `grauwolf-k8-ops` (siehe [Ohne das Root-Token arbeiten](#ohne-das-root-token-arbeiten)).
3. **`agent add <name>`** legt an, was das Modell oben auflistet. `--pull-secret <namespace>/<name>` kopiert ein
   bestehendes Image-Pull-Secret in den Namespace des Agenten und hängt es an den Service-Account. `agent list
   [--json]` zeigt die Agenten und ihre SPIFFE-IDs.
4. **`secret set <agent> <key>`** speichert einen Wert unter `kv/agents/<agent>/<key>` (Feld `value`). Der Wert
   wird von **stdin** gelesen (`printf %s "$TOKEN" | grauwolf-k8-ops secret set ...`) oder, am Terminal, aus einer
   verdeckten Eingabe – nie aus einem Argument oder der Umgebung, und er wird nie ausgegeben. `--variable NAME`
   ordnet außerdem die Konfigurationsvariable `NAME` dem Geheimnis in der ConfigMap des Agenten zu. `secret list
   <agent> [--json]` zeigt Schlüssel und Versionen, nie Werte; `secret delete <agent> <key>` entfernt alle
   Versionen.
5. **`agent check <name> --image <image>`** führt `grauwolf credentials check` als Job im Namespace des Agenten
   nach dem restriktiven Standard aus (numerischer Benutzer 65532, seccomp `RuntimeDefault`, alle Capabilities
   entfernt), wartet (`--timeout`, Standard 120 s), gibt das Log aus und löscht den Job – auch wenn er scheitert
   oder die Zeit abläuft. Er prüft jede in der Konfiguration des Agenten zugeordnete Variable in einem Job und
   gibt je Variable eine Zeile aus; `--variable <NAME>` (wiederholbar) schränkt ein. Schlägt eine Variable mit
   `permission denied` fehl, nennt ein Hinweis die Policy des Agenten (sie liest nur `kv/data/agents/<name>/*`)
   und den versuchten Pfad. Der Exit-Code ist 1, wenn eine Variable fehlschlägt.
6. **`agent remove <name>`** löscht den Namespace sowie die OpenBao-Rolle und -Policy, und mit `--purge-secrets`
   die Geheimnisse unter `kv/agents/<name>/`. Es nennt, was es entfernt, und fragt nach einer Bestätigung, außer
   `--yes` ist angegeben.

## Ohne das Root-Token arbeiten

Alles oben funktioniert mit dem Root-Token, aber das Werkzeug braucht viel weniger. `openbao setup` schreibt
auch die Policy `grauwolf-k8-ops`, die genau das erlaubt, was die Befehle tun, und sonst nichts: die
JWT-Auth-Methode und ihre Konfiguration (`sys/auth/jwt`, `auth/jwt/config`), den KV-Mount (`sys/mounts/kv`), die
Policies und Rollen `agent-*` (und das Lesen der eigenen Policy), die Geheimnisse unter `kv/data/agents/*` und
`kv/metadata/agents/*` und die wenigen Lesezugriffe von `doctor` (`sys/health`, `sys/capabilities-self`,
`auth/token/lookup-self`, die Altrolle `grauwolf-agents`). Es gibt keinen `sys/*`-Platzhalter, keinen Zugriff auf
andere Geheimnisse und kein Recht, Tokens zu erzeugen. Die Pfade folgen `--jwt-mount` und `--kv-mount`.

So gehst du vor: Führe `openbao setup` **einmal** mit dem Root-Token (oder einem Admin-Token) aus, erzeuge ein
eingeschränktes Token und arbeite ab dann damit:

```console
$ export OPENBAO_TOKEN=<root token>
$ grauwolf-k8-ops openbao setup
...
OpenBao policy grauwolf-k8-ops (what this tool needs, for `openbao token`): created
$ export OPENBAO_TOKEN=$(grauwolf-k8-ops openbao token --ttl 8h)
token with the policy grauwolf-k8-ops, valid for 8h; use it with `export OPENBAO_TOKEN=...` or --token-file
$ grauwolf-k8-ops doctor
...
  ok    OpenBao token policy: the token has the policy grauwolf-k8-ops
```

`openbao token [--ttl <dauer>] [--renewable]` (Standard `--ttl 8h`) erzeugt ein Token, das nur diese Policy hat
(nicht einmal `default`), und gibt es auf stdout aus, sonst nichts, sodass eine Befehlssubstitution es
einfängt; der Hinweis geht nach stderr. Es braucht ein Token, das Tokens erzeugen darf (Root oder ein
Admin-Token), und die Policy muss existieren (`openbao setup`). Das Token hat keinen Parent, wenn das benutzte
Token solche Tokens erzeugen darf (Root), und überlebt es daher; sonst ist es ein Kind davon. `doctor` warnt
`working with the root token` (nächster Schritt: create a restricted one with `grauwolf-k8-ops openbao token`),
solange das benutzte Token die Policy `root` hat, und meldet `ok` für ein Token mit der Policy
`grauwolf-k8-ops`. Ändert sich in einer späteren Version, was das Werkzeug braucht, führe `openbao setup` erneut
mit dem Root-Token aus: Es aktualisiert die Policy (das eingeschränkte Token kann sie nicht schreiben).

## Einen Agenten ausrollen

`agent deploy` bringt einen Agenten, der eine Identität hat (`agent add`), in einen laufenden Dienst
([ADR-0121](../adr/0121-grauwolf-k8-ops-agent-deploy.md)). Das Beispiel ist
[`01-hallo.json`](beispiele/01-hallo.json), ein Agent, der mit einem Ollama-Server im Cluster spricht:

```console
$ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
    --spec beispiele/01-hallo.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434 \
    --host reviewer.example.com
agent reviewer (namespace reviewer): image registry.example.com/grauwolf:1.0, grauwolf spec serve beispiele/01-hallo.json
server token kv/agents/reviewer/server-token: created (read it with `grauwolf-k8-ops secret get reviewer server-token --reveal`)
ConfigMap reviewer/grauwolf-config (GRAUWOLF_SERVER_TOKEN -> the server token): updated
ConfigMap reviewer/grauwolf-callers (no callers yet): created
ConfigMap reviewer/grauwolf-agent (1 file(s), 1 KiB): created
Deployment reviewer/reviewer: created
Service reviewer/reviewer (ports 8080, 8443 mTLS): created
Ingress reviewer/reviewer (host reviewer.example.com): created
NetworkPolicy reviewer/grauwolf-mtls-callers (port 8443 from nobody; port 8080 from anywhere): created
waiting for the rollout and /health (at most 300 s)
agent reviewer: ready, /health answers ok
endpoint http://reviewer.reviewer.svc:8080
mTLS endpoint https://reviewer.reviewer.svc:8443
host reviewer.example.com
$ grauwolf-k8-ops agent status reviewer
agent reviewer (namespace reviewer): deployed, ready
pod reviewer-7d9f-x1: Running, ready, 0 restart(s)
image registry.example.com/grauwolf:1.0
endpoint http://reviewer.reviewer.svc:8080
mTLS endpoint https://reviewer.reviewer.svc:8443
host reviewer.example.com
server token: exists
$ grauwolf-k8-ops secret get reviewer server-token --reveal   # the bearer token of the service
$ grauwolf-k8-ops agent logs reviewer --follow
$ grauwolf-k8-ops agent undeploy reviewer
```

Rufe den Dienst mit `Authorization: Bearer <server token>` auf, von innerhalb des Clusters (der Endpunkt) oder
über den Ingress von `--host`. Der Service-Proxy der Kubernetes-API (`kubectl proxy`, der Pfad
`…/services/http:<name>:8080/proxy/`) ist kein Zugang für Aufrufe, die das Token brauchen: Der API-Server
entfernt die Kopfzeile `Authorization`, daher antwortet der Dienst mit 401. `/health` braucht kein Token und
funktioniert darüber.

Was es im Namespace des Agenten anlegt:

- **Ein Deployment** `<name>` mit einem Replikat (Strategie `Recreate`, damit ein Volume nie von zwei Pods
  benutzt wird). Der Pod folgt dem Pod Security Standard `restricted` (numerischer Benutzer 65532, seccomp
  `RuntimeDefault`, alle Capabilities entfernt, keine Rechteausweitung, kein Service-Account-Token), hängt das
  SPIFFE-CSI-Volume ein (`SPIFFE_ENDPOINT_SOCKET`, `SPIFFE_FETCH_TIMEOUT=2m` für das Startrennen) und die
  ConfigMap `grauwolf-config` als `grauwolf.toml` (`GRAUWOLF_CREDENTIALS` zeigt darauf), und hat eine Startup-,
  Readiness- und Liveness-Probe auf `/health`. `--cpu` und `--memory` setzen Request und Limit; ohne sie wird
  nichts gesetzt.
- **Was läuft.** Mit `--spec <datei>` ist das Image die grauwolf-CLI, und der Pod führt `grauwolf spec serve` auf
  der Spec und ihrem ganzen Agenten-Baum aus (Prompts, Skills, Unteragenten, Wissen und andere Dateien), die
  `deploy` in der ConfigMap `grauwolf-agent` ablegt und schreibgeschützt unter `/app/spec` einhängt, eine
  einfache Datei je Eintrag. Ohne `--spec` ist das Image ein erzeugter Dienst (`spec generate --service
  --dockerfile`), der seine Spec enthält und seinen eigenen Einstiegspunkt behält; `--port` wird sein
  `SERVER_PORT`. Das Arbeitsverzeichnis des Containers ist `/data`.
- **Aufrufer per SPIFFE-ID** ([ADR-0109](../adr/0109-workload-identity-with-spiffe.md)). Der Pod läuft immer mit
  `GRAUWOLF_SPIFFE=1`: Neben dem HTTP-Port hat er einen zweiten Container-Port `mtls` (`--mtls-port`, Standard
  8443, `GRAUWOLF_SPIFFE_PORT`), der ein Client-Zertifikat verlangt, und
  `GRAUWOLF_SPIFFE_PUBLIC_URL=https://<name>.<namespace>.svc:<mtls-Port>`, die seine A2A-Agent-Card nennt. Die
  ConfigMap `grauwolf-callers` (leer, `{}`, angelegt, wenn sie fehlt, und von einem späteren Deploy nie
  ersetzt) wird als **Verzeichnis** unter `/etc/grauwolf/callers` eingehängt, und `GRAUWOLF_SPIFFE_USERS` zeigt
  auf `callers.json` darin. Ein Verzeichnis und kein `subPath` einer einzelnen Datei, denn nur ein Verzeichnis
  folgt einer Änderung der ConfigMap, und der Dienst liest die Datei neu, wenn sie sich ändert. Die Probes
  bleiben auf dem HTTP-Port. Die ConfigMap gehört nicht zum Hash weiter unten: `agent allow` und `deny` wirken
  ohne Neustart. `deploy` hält außerdem die NetworkPolicy `grauwolf-mtls-callers` bei den Ports dieses Deploys;
  siehe [Agenten sich gegenseitig aufrufen lassen](#agenten-sich-gegenseitig-aufrufen-lassen).
- **Der Service** `<name>` (ClusterIP, Port `--port`, Standard 8080, und der Port `mtls`) und, mit `--host`, ein **Ingress** für diesen
  Host. Er hat keinen TLS-Block und keine Klasse: Die Standards des Clusters ergänzen sie (auf dem Cluster des
  Eigentümers ergänzen Kyverno und cert-manager sie). Ohne `--host` ist der Agent nur innerhalb des Clusters
  erreichbar, unter `http://<name>.<namespace>.svc:<port>`. Ein `deploy` ohne `--host` entfernt einen Ingress aus
  einem früheren Lauf.
- **Daten.** `/data` ist standardmäßig ein `emptyDir` (geht mit dem Pod verloren); `--data-size 1Gi` macht daraus
  eine PersistentVolumeClaim `<name>-data`, und ein erzeugter Dienst bewahrt seine Gespräche dann dort auf
  (`GRAUWOLF_CONVERSATIONS_DIR`).
- **Beschreibbare Ordner (`--spec`).** Der Spec-Ordner ist schreibgeschützt, aber ein Agent schreibt in manche
  seiner Ordner. Für jeden **relativen** Ordner des Baums (die Root-Spec und ihre Unteragenten, jeweils relativ zur
  eigenen Spec), den der Agent beschreibt, hängt `deploy` ein Verzeichnis des Datenvolumes genau an dieser Stelle
  unter `/app/spec` ein, sodass die Spec unverändert bleibt: `builtin_tools.root` (`"root": "work"` ergibt
  `/app/spec/work`), `tools_dir` (für `self_tools` mit `save`) und der Ordner von `long_term_memory.store` und von
  `knowledge.index` (`"store": "state/memory.json"` ergibt `/app/spec/state`; eine Kopie der Datei, die ein lokaler
  Lauf in deinem Spec-Ordner hinterlassen hat, wird nicht mitgeschickt, der Agent startet mit leerem Speicher und
  baut den Index neu auf). Das Verzeichnis ist `files/<pfad>` des
  Datenvolumes; es überlebt mit `--data-size` einen Neustart und ist ohne ein `emptyDir`. Ein Init-Container legt
  die Verzeichnisse an, bevor der Agent startet (als Benutzer des Agenten; ein Verzeichnis, das kubelet für ein
  `subPath` anlegt, gehörte root und wäre nicht beschreibbar), mit denselben restriktiven Einstellungen und dem
  Image des Agenten, das `mkdir` braucht; das grauwolf-Image hat es. Die Ausgabe zeigt je Ordner eine Zeile
  `writable: …`, und `agent status` listet sie. Regeln: Ein **absoluter** Pfad bleibt, wie er ist, und muss im
  Image existieren oder unter `/data` liegen, sonst warnt `deploy`; ein Pfad, der den **Spec-Ordner verlässt**
  (`..`), wird abgelehnt; ein fehlendes `root` oder `"."` ist der Spec-Ordner selbst, der nicht beschreibbar sein
  kann (nimm einen Unterordner). Ein Mount verdeckt nie Spec-Dateien: Ein `root`, das Dateien der Spec enthält,
  bleibt ein schreibgeschützter Ordner der Spec (mit einer Warnung; nimm für das, was der Agent schreibt, einen
  anderen Ort), ein `tools_dir` mit Tool-Dateien wird abgelehnt (sie müssen gebaut werden), und ein `store` oder
  `index` wird abgelehnt, wenn die Datei im eigenen Ordner der Spec oder in einem Ordner mit anderen Dateien der
  Spec liegt (leg sie in einen eigenen Unterordner, etwa `state/memory.json`).
- **`--env NAME=VALUE`** (wiederholbar) setzt Werte, die **nicht geheim** sind, etwa die Ollama-URL. Sie landen im
  Deployment, nicht in einem Secret, und jeder, der das Deployment lesen darf, sieht sie. Ein Name, den die
  `grauwolf.toml` des Agenten auf OpenBao abbildet, wird abgelehnt; ein Name, der keine Konfigurationsvariable
  von grauwolf ist, erzeugt nur eine Warnung. Geheimnisse laufen weiterhin über `secret set --variable`.
- **Das Server-Token.** Das erste `deploy` erzeugt 32 Zufallsbytes (hex) unter `kv/agents/<name>/server-token` und
  ordnet ihnen in `grauwolf.toml` `GRAUWOLF_SERVER_TOKEN` zu, damit der Dienst `Authorization: Bearer <token>`
  verlangt (`/health` bleibt offen). Ein erneutes `deploy` und `undeploy` behalten es; `secret get <name>
  server-token --reveal` gibt es aus.
- **`--pull-secret <namespace>/<name>`** kopiert ein Pull-Secret und hängt es an den Service-Account (wie es
  `agent add` tut).
- **`--otel`** schickt die Traces des Agenten an einen Collector; siehe [Traces in Tempo](#traces-in-tempo).

`deploy` lässt sich erneut ausführen: Es wendet nur an, was abweicht. Ein Hash aus Pod, Konfiguration und Dateien
des Agenten steht als Annotation an der Pod-Vorlage, daher startet der Pod genau dann neu, wenn sich Spec, Image,
Umgebung, Port oder Konfiguration geändert haben (auch nach `secret set --variable`), sonst nicht. Danach wartet es
(`--timeout`, Standard 300 s), bis der Rollout abgeschlossen ist und `/health` über den Service-Proxy der
API-Server antwortet. Stürzt der Pod ab, kann er sein Image nicht ziehen oder wird nicht rechtzeitig bereit, gibt
er den Grund und die letzten 20 Zeilen des Logs aus und endet mit 1; `agent logs` und `agent status` schauen
genauer hin. `agent undeploy` entfernt Deployment, Service, Ingress und die ConfigMap der Spec und behält
Identität, Rolle, Geheimnisse, die Aufrufer (ConfigMap und NetworkPolicy) und das Datenvolume (`--purge-data` löscht das Volume nach einer Bestätigung);
`agent remove` entfernt weiterhin alles, ob ausgerollt oder nicht. `doctor` hat je ausgerolltem Agenten eine
Zeile (bereit oder nicht, mit seinem Image) und, für einen Agenten mit eingeschaltetem Tracing, eine weitere, die
sagt, ob der Service des Collectors existiert.

**Was `--spec` ablehnt** (die Fehlermeldung sagt: ein Image mit `spec generate --service --dockerfile` bauen und
ohne `--spec` ausrollen): Tools mit Code, Code im Graphen, Tool-Pakete und Git-Skills (sie müssen gebaut oder
geholt werden), ein `tools_dir` mit Tool-Dateien, ein Pfad der Spec, der den Spec-Ordner verlässt (`..`), ein Baum,
der größer als etwa 1 MiB ist (die Grenze einer ConfigMap), und ein `long_term_memory.store` oder
`knowledge.index`, der im eigenen Ordner der Spec oder in einem Ordner mit anderen Spec-Dateien liegt (die
Fehlermeldung schlägt einen eigenen Unterordner vor).

### Events im Pod-Log

`agent deploy --events` schreibt jedes Event der Läufe des Agenten als eine JSON-Zeile auf stderr, sodass die Events
im Pod-Log stehen, wo ein Log-Sammler (zum Beispiel Loki) sie aufnimmt. Es ist die Cluster-Seite von `--events` bei
`spec serve` ([Kapitel 9](09-betrieb.md#ereignisse-als-json-zeilen)): Mit `--spec` führt der Pod
`grauwolf spec serve … --events` aus; bei einem Image ohne `--spec` bekommt der erzeugte Dienst
`GRAUWOLF_EVENTS=1` ([Kapitel 11](11-code-erzeugen.md#als-http-dienst---service)), das auch seine Unteragenten
abdeckt. Events tragen Ein- und Ausgabe des Agenten, Prompts und Tool-Argumente: Sie können personenbezogene Daten
enthalten, und das Pod-Log ist kein Ort für Geheimnisse.

```console
$ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 --events
…
events: on, JSON lines on stderr (the pod log)
$ grauwolf-k8-ops agent logs reviewer --events-only | jq -c '{event, path}'
{"event":"agent_starting","path":null}
…
```

- **Wie jede andere Option gilt sie nur, solange sie angegeben ist.** Ein Deploy ohne `--events` entfernt das Flag
  bzw. die Variable wieder; die Einstellung ist Teil des Pod-Templates, der Pod startet also genau dann neu, wenn du
  sie ein- oder ausschaltest.
- **`--env GRAUWOLF_EVENTS=…`** wird zusammen mit `--events` abgelehnt; ohne `--events` geht es wie jedes andere
  `--env` durch.
- **`agent logs --events-only`** gibt nur die Zeilen aus, die JSON-Events sind (sie beginnen mit `{"event":`), ohne
  die übrigen Logzeilen; `agent logs` selbst ist unverändert.
- **`agent status`** zeigt `events: on` oder `events: off`; `status --json` hat `events` (`true`/`false`).
- Mit einem älteren grauwolf gebaute Images kennen `GRAUWOLF_EVENTS` nicht und ignorieren es; baue das Image neu.

### Traces in Tempo

Mit `--otel` exportiert der Pod seine Traces ([Kapitel 9](09-betrieb.md#opentelemetry), ADR-0049) über
OTLP/HTTP. So siehst du in einem Grafana Tempo, welcher Agent welchen aufgerufen hat und wer der Aufrufer war:

```console
$ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
    --spec examples/01-hello.json --otel
agent reviewer (namespace reviewer): image registry.example.com/grauwolf:1.0, grauwolf spec serve examples/01-hello.json
tracing: on, OTLP/HTTP to http://lgtm.lgtm.svc:4318
…
$ grauwolf-k8-ops agent status reviewer
…
tracing: on (http://lgtm.lgtm.svc:4318)
```

| Option | Bedeutung |
|---|---|
| `--otel` | Der Pod führt `grauwolf spec serve … --otel` aus, mit `OTEL_EXPORTER_OTLP_ENDPOINT=<endpoint>` und `OTEL_SERVICE_NAME=<Name des Agenten>`. Bei einem Image ohne `--spec` bekommt der Dienst stattdessen `GRAUWOLF_OTEL=1` in seiner Umgebung. |
| `--otel-endpoint <url>` | Die OTLP/HTTP-Basis-URL des Collectors; das SDK hängt `/v1/traces` an. Standard `http://lgtm.lgtm.svc:4318`: der Port `otel-http` eines Service `lgtm` im Namespace `lgtm`, wie ein Grafana-LGTM-Stack (Loki, Grafana, Tempo, Mimir) oft installiert wird. Gib deine eigene URL an, wenn der Collector woanders läuft; `--help` zeigt den Standard. |
| `--otel-content` | Zeichnet in den Spans auch Prompts, Antworten sowie Argumente und Ergebnisse von Tools auf (bei einem Image: `GRAUWOLF_OTEL_CONTENT=1`). Sie können personenbezogene Daten enthalten. |

- **Wie jede andere Option ist das Tracing nur an, solange die Option angegeben wird.** Ein `deploy` ohne `--otel`
  entfernt das Flag und beide Variablen wieder. Die Einstellungen gehören zur Pod-Vorlage, daher startet der Pod
  genau dann neu, wenn du das Tracing ein- oder ausschaltest oder den Endpunkt bzw. `--otel-content` änderst, und
  bei einer Wiederholung nicht.
- **`--env` und die reservierten Variablen.** Mit `--otel` werden `--env OTEL_EXPORTER_OTLP_ENDPOINT=…`, `--env
  OTEL_SERVICE_NAME=…`, `--env GRAUWOLF_OTEL=…` und `--env GRAUWOLF_OTEL_CONTENT=…` abgelehnt (nimm `--otel-endpoint`; der Service-Name ist der Name des Agenten). Ohne `--otel`
  gehen sie wie jedes andere `--env` durch, und `OTEL_EXPORTER_OTLP_HEADERS` immer: Es steht wie jeder
  `--env`-Wert im Deployment und ist daher kein Ort für ein Geheimnis.
- **Ein Image ohne `--spec`** geht auch: Ein generierter Dienst (`spec generate --service`) hat keine Flags,
  deshalb setzt `--otel` `GRAUWOLF_OTEL=1` (und mit `--otel-content` `GRAUWOLF_OTEL_CONTENT=1`) in seiner
  Umgebung ([Kapitel 11](11-code-erzeugen.md#als-http-dienst---service)). Images, die mit einem älteren grauwolf
  gebaut wurden, kennen die Variable nicht und ignorieren sie, exportieren also nichts; baue das Image neu.
- **Das Netz.** Die NetworkPolicy `grauwolf-mtls-callers` beschränkt nur den *Eingang* (ihr `policyTypes` ist
  `Ingress`), der Weg des Pods hinaus zum Collector wird also nicht berührt. Hat dein Cluster ein eigenes Default
  Deny für ausgehenden Verkehr, erlaube dort den Port des Collectors.
- **`status`** zeigt `tracing: on (<endpoint>)` (mit `, with contents`, wenn `--otel-content` an ist) oder
  `tracing: off`; `status --json` hat `tracing` mit `endpoint` und `content`, oder `null`.
  **`doctor`** hat für einen Agenten mit eingeschaltetem Tracing die Zeile `agent <name>: tracing`. Ist der Endpunkt
  `http://<service>.<namespace>.svc[…][:port]`, ist sie `ok`, wenn dieser Service existiert, und eine Warnung, wenn
  nicht (die Traces gingen verloren). Er kontaktiert den Collector nie; eine andere URL wird nicht geprüft.

**Was du in Tempo siehst.** Jede Anfrage an den Agenten ist ein Trace mit dem Span `invoke_agent reviewer` (der
Dienst heißt wie der Agent), darunter die Spans `node …`, `chat <model>` und `execute_tool …`. Hat ein
anderer Agent über SPIFFE aufgerufen ([nächster Abschnitt](#agenten-sich-gegenseitig-aufrufen-lassen)), trägt der
Span `invoke_agent` das Attribut `grauwolf.caller` mit der SPIFFE-ID des Aufrufers. In Grafanas Explore mit der
Tempo-Datenquelle findet TraceQL diese Aufrufe:

```
{ span.grauwolf.caller = "spiffe://agentswork.de/ns/coordinator/sa/coordinator" }
```

Ohne Filter listet `{ resource.service.name = "reviewer" && span.grauwolf.caller != nil }` jeden Aufruf, der von
einem anderen Agenten kam.

**Ein Trace vom Aufrufer zum Aufgerufenen.** Ein Aufruf von einem Agenten an einen anderen ist ein Trace, nicht
zwei: Der aufrufende Agent schickt den W3C-Header `traceparent` mit seiner A2A- oder MCP-Anfrage
([Kapitel 9](09-betrieb.md#opentelemetry)), und der Span `invoke_agent` des aufgerufenen Agenten setzt den Trace
fort. Rolle beide Agenten mit `--otel` aus (zum selben Collector). Im Trace hat `invoke_agent coordinator` den Span
`execute_tool reviewer` unter sich, und `invoke_agent reviewer` ist ein Kind dieses Spans. Die Dienste bleiben
getrennt (`coordinator`, `reviewer`); eine strukturelle TraceQL-Abfrage findet daher die Traces, die vom einen zum
anderen reichen:

```
{ resource.service.name = "coordinator" } >> { resource.service.name = "reviewer" }
```

`grauwolf.caller` steht weiterhin an `invoke_agent reviewer`, die Suche oben funktioniert also wie zuvor. Ein
Agent ohne `--otel` schickt und setzt keinen Trace fort; seine Aufgerufenen beginnen dann eigene Traces.

## Begrenzen, was ein Agent erreichen darf (Egress)

Alles, was ein Agent erreichen darf, folgt aus seiner Spec: die Modell-Hosts, die MCP-Server und entfernten
Agenten, die `fetch`-Hosts, Jira, Git, OpenBao. `grauwolf spec egress` listet diese Ziele auf, und
`agent deploy --egress` macht daraus eine NetworkPolicy `grauwolf-egress`, sodass ein `fetch` zu einem Host,
der nicht in der Spec steht, oder ein direkter Verbindungsversuch vom Cluster blockiert wird.

```sh
grauwolf spec egress reviewer.json
```

```text
run   http://localhost:11434  model (ollama)  [/model/provider]  (from OLLAMA_HOST)
run   https://acme.atlassian.net:443  Jira  [/builtin_tools/jira/base_url]
run   http://peer.team.svc:8080  agent peer (A2A)  [/agent_tools/peer/url]
```

`--format json` gibt ein Array mit `scheme`, `host`, `port`, `any_port`, `purpose`, `pointer` (bei einem
Unteragenten mit der Spec-Datei), `phase` und `from_env` aus; `spec generate --dockerfile` schreibt dasselbe nach `app/egress.json`.
Die Regeln der Liste:

- **Nie geraten.** Eine Adresse, die nur eine Variable kennt (`RKEV_BASE_URL`, ein pgvector-`url_env`, der
  OTLP-Endpunkt mit `--otel`), hat keinen Host, nur `from_env`. Eine Adresse mit Vorgabe (die eines
  Modell-Providers: `api.openai.com`, `localhost:11434` für Ollama, die Standardregion von `bedrock`) hat die
  Vorgabe, und `from_env` nennt die Variable, die sie ersetzen kann (`OPENAI_BASE_URL`, `OLLAMA_HOST`,
  `AWS_REGION`, …).
- **Ein `fetch`-Host im Cluster hat keinen festen Port.** `fetch.allowed_hosts` nennt Hosts ohne Port, daher
  nimmt die Liste für einen externen Host 443 an (und 80 mit `allow_http`). Einen Cluster-Host
  (`<service>.<namespace>.svc`, `*.<namespace>.svc`) darf der Agent auf jedem Port erreichen, den sein Service
  hat, zum Beispiel auf dem mTLS-Port eines anderen Agenten: Sein Eintrag hat `"port": null` und
  `"any_port": true`, und das Textformat zeigt `https://target.target.svc:*`.
- `phase: build` kennzeichnet, was nur beim Bau des Images gebraucht wird: Registries der Tool-Pakete und
  Git-Quellen von Paketen und Skills. Zur Laufzeit werden sie nicht gebraucht, und `deploy` erlaubt sie nicht.
- Doppelte Einträge werden zusammengefasst (die Zwecke werden verbunden), Host-Namen sind klein geschrieben, die
  Reihenfolge ist fest.

Mit `agent deploy --egress` (braucht `--spec`) löst `deploy` `from_env` mit dem auf, was es kennt: den
`--env`-Werten, der OpenBao-Adresse aus der Konfiguration des Agenten und dem `--otel-endpoint`. Dann schreibt es
die NetworkPolicy `grauwolf-egress` für die Pods des Agenten:

| Ziel | Regel |
|---|---|
| DNS | UDP und TCP 53 zu den Pods mit dem Label `k8s-app=kube-dns` in `kube-system` (k3s; `--egress-dns-namespace` und `--egress-dns-selector` ändern das); immer |
| Im Cluster (`<service>.<namespace>.svc`) | Der Namespace (Label `kubernetes.io/metadata.name`) und der Port. Eine NetworkPolicy gilt für den **Pod**-Port, daher liest `deploy` den Service und erlaubt dessen `targetPort`; ist der Service nicht da, erlaubt es den Service-Port und warnt. Ein `fetch`-Host im Cluster hat keinen festen Port („jeder Port“): `deploy` erlaubt die Pod-Ports von **allen** Ports seines Service; bei einem Muster (`*.<namespace>.svc`) oder einem Service, den es noch nicht gibt (mit Warnung), erlaubt es den ganzen Namespace ohne Port-Einschränkung |
| IP-Adresse (z. B. `OLLAMA_HOST=http://10.1.2.3:11434`) | `ipBlock` `/32` (`/128`) und der Port |
| `localhost` | Keine Regel: es bleibt im Pod |
| Externer Host-Name | Nur nach Port, siehe unten |

**Die Grenze: keine Namen.** Eine einfache NetworkPolicy vergleicht Adressen, keine Namen. Ein externer Host wie
`acme.atlassian.net` lässt sich daher nicht für sich allein erlauben. Standardmäßig erlaubt die Policy seinen
Port (443) zu jeder Adresse außerhalb der Pod- und Service-Bereiche des Clusters (`--cluster-cidr`, Standard
die k3s-Bereiche `10.42.0.0/16` und `10.43.0.0/16`), und `deploy` warnt mit den Namen, die nur nach Port begrenzt
sind; `agent status` zeigt `egress: <n> destinations, <m> only port-restricted`, und `doctor` listet sie auf.
Mit `--egress-strict` werden sie weggelassen und **blockiert**; `deploy` sagt, welche. Namen zu vergleichen
braucht eine Policy-Engine, die FQDNs kennt, oder ein Egress-Gateway.

Weitere Grenzen:

- **Ports externer `fetch`-Hosts.** Die Liste und die Policy nehmen für einen externen `fetch`-Host 443 an (und
  80 mit `allow_http`), weil `allowed_hosts` keine Ports hat. Ein `fetch` zu einem externen Host auf einem
  anderen Port ist nicht abgedeckt und wird blockiert.
- **Die ersten Sekunden eines Pods.** NetworkPolicies werden vom Netzwerk-Plugin oder Policy-Controller
  asynchron angewendet. Auf k3s war ein neuer Pod nach dem Start einige Sekunden lang unbeschränkt, bis der
  Controller die Regeln angewendet hatte: Eine sofort gestartete Probe erreichte `example.com:80` und andere
  Hosts, die die Policy blockiert, dieselbe Probe nach 20 Sekunden nicht mehr. Die ersten Sekunden eines Pods
  sind also womöglich nicht abgedeckt. Das ist eine Eigenschaft des Policy-Controllers des Clusters, nicht von
  grauwolf; starte in diesem Fenster keine Arbeit, die den Cluster nicht verlassen darf.

```sh
grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
  --spec reviewer.json --env OLLAMA_HOST=http://10.1.2.3:11434 --egress
```

```text
NetworkPolicy reviewer/grauwolf-egress (4 destinations, 1 only port-restricted): created
note: egress: peer.team port 8080 is port 9090 of its pods: the policy allows 9090
warning: egress: 1 external destination(s) can only be restricted by port, not by name (NetworkPolicies match addresses): acme.atlassian.net:443; `--egress-strict` leaves them out
```

Die Policy gehört zu dem, was `deploy` abgleicht: ein erneutes Ausrollen aktualisiert sie, eines ohne `--egress`
löscht sie, `agent undeploy` entfernt sie und `agent remove` entfernt sie mit dem Namespace. Sie steckt nicht im
Pod-Template, eine Änderung startet also nichts neu. Sie gilt nur für die Pods des Agenten; die Policy
`grauwolf-mtls-callers` (wer den Agenten aufrufen darf) bleibt getrennt. Ein Ziel, dessen Adresse aus OpenBao
kommt (eine zugeordnete Variable) oder das kein `--env` nennt, bleibt unbekannt: `deploy` warnt, und keine Regel
erlaubt es. Ein Cluster, der NetworkPolicies nicht durchsetzt, ignoriert die Policy; `doctor` prüft das.

## Agenten sich gegenseitig aufrufen lassen

Ein Agent kann einen anderen Agenten des Clusters über A2A aufrufen oder als MCP-Server nutzen, und das
Ziel weiß dann, **wer** aufruft: Der Aufrufer weist seine SPIFFE-ID mit einem Zertifikat nach (mTLS,
[Kapitel 9](09-betrieb.md)), und das Ziel ordnet diese ID einem Benutzer zu
([ADR-0109](../adr/0109-workload-identity-with-spiffe.md), Entscheidungen Q7-Q9). Niemand darf einen
Agenten aufrufen, bis sein Besitzer es erlaubt.

### Durchgang: coordinator ruft reviewer auf

Zwei Agenten: `reviewer` (das Ziel, [`13-pruefer.json`](beispiele/13-pruefer.json)) und `coordinator` (der
Aufrufer, [`13-koordinator.json`](beispiele/13-koordinator.json)), der Entwürfe an den Reviewer weitergibt.
Die Agenten laufen in den Namespaces `reviewer` und `coordinator`, und die Trust Domain ist `agentswork.de`,
wie überall in diesem Kapitel.

1. **Beide Agenten anlegen** (Identität, Namespace, OpenBao-Rolle; siehe den Durchgang weiter oben):

   ```console
   $ grauwolf-k8-ops agent add reviewer
   $ grauwolf-k8-ops agent add coordinator
   ```

2. **Dem Koordinator den Aufruf des Reviewers erlauben:**

   ```console
   $ grauwolf-k8-ops agent allow reviewer --from coordinator
   agent reviewer (namespace reviewer) allows spiffe://agentswork.de/ns/coordinator/sa/coordinator as user coordinator: created
   ConfigMap reviewer/grauwolf-callers: created
   NetworkPolicy reviewer/grauwolf-mtls-callers (port 8443 from namespace coordinator; port 8080 from anywhere): created
   ```

3. **Die Spec des Koordinators schreiben.** Der Eintrag in `agent_tools` hat die Service-Adresse des
   Reviewers auf dem **mTLS-Port über https** und die SPIFFE-ID des Reviewers,
   `spiffe://<trust domain>/ns/reviewer/sa/reviewer` (`agent list` gibt sie aus):

   ```json
   "agent_tools": {
     "reviewer": {
       "url": "https://reviewer.reviewer.svc:8443",
       "description": "Prüft einen kurzen Text und nennt seine Hauptprobleme",
       "auth": { "spiffe": { "id": "spiffe://agentswork.de/ns/reviewer/sa/reviewer" } }
     }
   }
   ```

   Der Koordinator zeigt sein Zertifikat vor und akzeptiert den Server nur, wenn er genau diese ID hat.

4. **Beide ausrollen**, zuerst den Reviewer (ein Modellserver wie Ollama muss erreichbar sein, siehe
   [Einen Agenten ausrollen](#einen-agenten-ausrollen)):

   ```console
   $ grauwolf-k8-ops agent deploy reviewer --image registry.example.com/grauwolf:1.0 \
       --spec beispiele/13-pruefer.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434
   $ grauwolf-k8-ops agent deploy coordinator --image registry.example.com/grauwolf:1.0 \
       --spec beispiele/13-koordinator.json --env OLLAMA_HOST=http://ollama.ollama.svc:11434
   ```

   Passte die Spec des Koordinators nicht (kein `auth`, eine falsche ID, `http://` oder Port 8080 oder kein
   `allow`), gäbe das Ausrollen des Koordinators für jedes Problem eine Warnung aus, zum Beispiel
   `warning: coordinator: agent_tools.reviewer (agent reviewer) calls agent reviewer, which has not allowed coordinator (…)
   (run `grauwolf-k8-ops agent allow reviewer --from coordinator`)`; das Ausrollen läuft weiter.

5. **Den Koordinator aufrufen**, von innerhalb des Clusters mit seinem Server-Token (`secret get coordinator
   server-token --reveal`), wie in [Einen Agenten ausrollen](#einen-agenten-ausrollen):

   ```console
   $ curl -s http://coordinator.coordinator.svc:8080/run -H "Authorization: Bearer $TOKEN" \
       -H 'Content-Type: application/json' -d '{ "input": "Review: our new login page is fast but ugly." }'
   ```

   Der Koordinator schickt den Text über A2A durch den mTLS-Port an den Reviewer; die Antwort ist dessen
   Prüfung in zwei Sätzen.

6. **Sehen, wer aufgerufen hat.** Der Reviewer ordnet die ID des Koordinators dem Benutzer `coordinator` zu,
   der Aufruf hat dort also seine eigenen Gespräche, sein Gedächtnis und seine Freigaben, und das Ereignis
   `agent_starting` des Reviewers hat das Feld `caller` mit
   `spiffe://agentswork.de/ns/coordinator/sa/coordinator`
   ([Kapitel 9](09-betrieb.md#agenten-rufen-agenten-mit-spiffe)): in den `agent`-Ereignissen von
   `/run/stream`, mit `spec serve --events` und als Attribut `grauwolf.caller` des Spans `invoke_agent` mit
   OpenTelemetry. Rolle beide Agenten mit `--otel` aus, um den Aufruf in Tempo als einen Trace zu sehen
   ([Traces in Tempo](#traces-in-tempo)): `{ span.grauwolf.caller = "spiffe://agentswork.de/ns/coordinator/sa/coordinator" }`.
   `agent deploy` startet `spec serve` ohne `--events`, deshalb zeigt `agent logs` die Ereignisse nicht.

7. **Gegenproben.** `agent deny reviewer --from coordinator`, und der nächste Aufruf scheitert: Wird die
   NetworkPolicy durchgesetzt, läuft die Verbindung in ein Timeout, sonst antwortet der Reviewer mit `403`.
   Ein Aufrufer, der nie erlaubt wurde, bekommt dasselbe. `grauwolf-k8-ops doctor` listet die Aufrufer, die
   Policy und die Aufrufe des Koordinators (unten).

### Die Befehle

```console
$ grauwolf-k8-ops agent allow reviewer --from coordinator
agent reviewer (namespace reviewer) allows spiffe://agentswork.de/ns/coordinator/sa/coordinator as user coordinator: created
ConfigMap reviewer/grauwolf-callers: created
NetworkPolicy reviewer/grauwolf-mtls-callers (port 8443 from namespace coordinator; port 8080 from anywhere): created
$ grauwolf-k8-ops agent callers reviewer
agent reviewer (namespace reviewer): 1 caller(s)
  spiffe://agentswork.de/ns/coordinator/sa/coordinator  user coordinator  agent coordinator  network policy: admitted
NetworkPolicy reviewer/grauwolf-mtls-callers: matches
$ grauwolf-k8-ops agent deny reviewer --from coordinator
```

- **`agent allow <target> --from <caller> [--as <user>]`** schreibt die SPIFFE-ID des Aufrufers
  (`spiffe://<trust domain>/ns/<Namespace des Aufrufers>/sa/<caller>`; die Trust Domain kommt wie überall
  im Werkzeug vom SPIRE-Server) in die ConfigMap `grauwolf-callers` im Namespace des Ziels, Schlüssel
  `callers.json`: `{ "<SPIFFE-ID>": "<user>" }`. Der Benutzer ist der Name des Aufrufers, wenn `--as` nichts
  anderes sagt; es ist der Benutzer der Aufrufe in den Gesprächen, im Gedächtnis und bei den Freigaben des
  Ziels. Beide Agenten müssen existieren (`agent add`). Der Befehl ist wiederholbar und meldet `created`,
  `updated` oder `unchanged`.
- **Die NetworkPolicy `grauwolf-mtls-callers`** wird mit der Zuordnung abgeglichen. Sie wählt die Pods mit
  dem Label `grauwolf.io/agent=<target>` und hat zwei Ingress-Regeln: den HTTP-Port (8080) von überall
  (Ingress, Probes, Menschen mit Tokens) und den mTLS-Port (8443) nur aus den Namespaces der Aufrufer
  (`kubernetes.io/metadata.name`). Ohne Aufrufer gibt es gar keine Regel für den mTLS-Port: Eine Regel ohne
  `from` ließe alle herein. Eine Policy, die von Hand geändert wurde, bringt das nächste `allow` oder `deny`
  wieder in Ordnung.
- **`agent deny <target> --from <caller>`** entfernt den Eintrag und den Namespace aus der Policy, auch für
  einen Aufrufer, den es nicht mehr gibt. Auch das ist wiederholbar.
- **`agent remove <name>`** nimmt den Agenten außerdem aus den Aufrufern und NetworkPolicies der anderen
  Agenten, die ihn erlaubt hatten, damit kein Eintrag hängen bleibt: Die Rückfrage listet sie auf, und die
  Ausgabe sagt `ConfigMap <ns>/grauwolf-callers: <name> removed from the callers of <other>`.
- **`agent callers <target> [--json]`** listet die IDs mit ihren Benutzern, den zugehörigen Agenten
  (`no such agent` bei einem hängenden Eintrag), ob die Policy ihren Namespace zulässt und ob die Policy
  `matches`, von dem `differs`, was die Aufrufer verlangen, oder `missing` ist.

### Die Prüfung der aufrufenden Spec

Hat eine Spec, die `agent deploy --spec` in den Cluster bringt, einen
Eintrag in `agent_tools` oder `mcp_servers`, dessen URL `<name>.<namespace>.svc` (oder
`….svc.cluster.local`) eines anderen, mit diesem Werkzeug eingerichteten Agenten ist, gibt `deploy` eine
Warnung aus, nie einen Fehler, wenn

- der Eintrag kein `auth.spiffe` hat oder seine `id` (`ids`, `trust_domain`) nicht die SPIFFE-ID dieses Agenten ist;
- die URL nicht `https` auf dem mTLS-Port des Ziels ist (bei `"mode": "jwt"`, das an den HTTP-Port geht, entfällt die Prüfung);
- das Ziel diesen Aufrufer nicht erlaubt hat (`agent allow`; nur für Einträge mit `auth.spiffe`).

`doctor` zeigt dasselbe je ausgerolltem Agenten als Zeile `agent <name>: caller spec`, gelesen aus der
ConfigMap `grauwolf-agent` des Agenten; ein Agent, der aus einem Image ohne `--spec` ausgerollt wurde, hat
keine solche ConfigMap, und die Zeile sagt, dass seine Aufrufe nicht geprüft werden. Weitere Zeilen je
Agent: `callers` (die ConfigMap existiert und lässt sich lesen; eine Warnung für jeden Eintrag, der einen
Agenten nennt, den es nicht mehr gibt, oder eine andere Trust Domain) und `NetworkPolicy` (sie passt zu den
Aufrufern). **`doctor` kann nicht erkennen, ob der Cluster NetworkPolicies durchsetzt**: Die API nimmt sie
in jedem Fall an, und ein Netzwerk-Plugin, das sie ignoriert, lässt jeden Pod an den mTLS-Port (der Dienst
selbst antwortet einem Aufrufer, den er nicht kennt, weiter mit `403` und weist einen Pod ohne Zertifikat
ab). Deshalb endet er mit der Warnung `NetworkPolicy enforcement`; Gewissheit gibt ein Pod in einem
Namespace, den kein Agent erlaubt, und der Versuch, den Port zu erreichen, `nc -zv -w 3
<agent>.<namespace>.svc 8443` muss in ein Timeout laufen, oder der Live-Check.

**Wie schnell `allow` und `deny` wirken.** Kubernetes kopiert eine geänderte ConfigMap mit
Verzögerung in einen laufenden Pod, meist 30–90 Sekunden, und der Dienst liest die Datei danach
innerhalb von 2 Sekunden neu ein. Im Live-Test kam nach einem `deny` das erste `403` nach etwa
40 Sekunden, ohne Neustart. Eine Änderung der NetworkPolicy trennt bereits offene Verbindungen nicht,
etwa den Verbindungspool eines aufrufenden Agenten, deshalb erreicht ein entfernter Aufrufer den Port
noch eine Weile. Abgewiesen wird er dann über die Datei mit den Aufrufern.

## Im Auftrag von Personen handeln

`grauwolf-k8-ops identity install` betreibt den Identitätsdienst von
[ADR-0111](../adr/0111-delegation-with-transaction-tokens.md) im Namespace `identity`, mit eigener
SPIFFE-Identität und einem Transit-Schlüssel in OpenBao, der die Txn-Tokens signiert; `agent deploy
--identity` verbindet einen Agenten mit ihm, `identity allow <agent>` lässt einen Agenten am Eingang Tokens
für Personen anfordern. [Kapitel 14](14-im-auftrag-von-personen.md#im-cluster-mit-grauwolf-k8-ops) erklärt
es, und [`examples/delegation/`](../../examples/delegation/README.md) geht es mit Entra Local und
`grauwolf login` durch. Führe nach einem Update von `grauwolf-k8-ops` `openbao setup` erneut aus: Seine
Policy deckt jetzt den Identitätsdienst ab.

## Die Trust Domain umstellen

`trust-domain set <domain>` ändert die Trust Domain eines laufenden SPIRE samt allem, was sie enthält. Es ist die
einzige Änderung in diesem Kapitel, die sich nicht durch erneutes Ausführen eines Befehls rückgängig machen lässt:
**jede SPIFFE-ID ändert sich** (`spiffe://old/ns/reviewer/sa/reviewer` wird zu
`spiffe://new/ns/reviewer/sa/reviewer`), jeder Workload bekommt eine neue SVID, und der SPIRE-Server startet mit
einer leeren Datenbank und einer neuen CA, denn die CA einer Trust Domain lässt sich nicht umbenennen. Alles, was
noch den alten Bereich nennt und nicht Teil der Änderung ist, **hört auf zu funktionieren**.

```console
$ grauwolf-k8-ops trust-domain set agentswork.de --consumers trust-domain-consumers.toml
```

Ohne `--apply` gibt dies nur den Plan aus und ändert nichts:

- wo die Domain heute steht (Konfiguration von SPIRE-Server, -Agent und -Controller-Manager, der OIDC-Provider, die
  CA im Bundle des Servers, das gebundene Subject jeder `agent-*`-Rolle in OpenBao);
- jede ConfigMap mit den genauen Zeilen, die sich ändern (`- trust_domain = "old"` / `+ trust_domain =
  "new"`): in der Konfiguration von SPIRE-Server und -Agent nur die Zuweisung `trust_domain`, im Controller-Manager
  `trustDomain:`, im OIDC-Provider das `trust_domain` von `workload_api`; Zeilen, die danach noch die alte Domain
  nennen (etwa ein `jwt_issuer`), werden als Warnung aufgeführt, sie werden nicht angefasst;
- das Volume, das neu angelegt wird (StatefulSet auf 0 skalieren, PVC löschen, wieder hochskalieren), und das
  Warten auf das neue Bundle;
- die Neustarts (Agenten, OIDC-Provider, spire-controller-manager, falls es ein Deployment ist) und die
  OpenBao-Rollen mit altem und neuem Subject;
- die Verbraucher (unten), was unberührt bleibt und was die Änderung bedeutet.

Mit `--apply` fragt es nach einer Bestätigung (`--yes` überspringt sie; ohne Terminal und ohne `--yes` bricht es
ab) und führt dann diese Schritte in dieser Reihenfolge aus:

1. **Backup**: `--backup <datei>` (Standard `trust-domain-backup-<UTC-Zeitstempel>.json` im Arbeitsverzeichnis; eine
   bestehende Datei wird nie überschrieben, und kann sie nicht geschrieben werden, ändert sich nichts). Es enthält die
   vollständigen Objekte jeder ConfigMap, die neu geschrieben wird, des StatefulSet, der DaemonSets und Deployments,
   die neu gestartet werden, sowie die OpenBao-Rollen. Kein Secret wird gelesen, und kein Geheimniswert steht darin.
   Die Datei wird nur für ihren Besitzer lesbar angelegt.
2. **Konfiguration** wird wie im Plan neu geschrieben.
3. **Die Daten des Servers werden neu angelegt**: Der StatefulSet wird auf 0 skaliert (die Replikazahl wird
   inzwischen in der Annotation `grauwolf.io/trust-domain-replicas` gehalten), das Werkzeug wartet, bis sein Pod
   verschwunden ist, löscht die PVC des Datenvolumes, skaliert zurück und wartet, bis er bereit ist.
4. **Es wartet**, bis die Bundle-ConfigMap (die des `k8sbundle`-Notifiers; Name und Namespace aus der
   Konfiguration des Servers, Standard `spire-bundle`) eine CA mit der URI-SAN `spiffe://<new>` hat. Erst dann
   werden die SPIRE-Agenten (Neustart-Annotation am DaemonSet), spire-controller-manager (falls ein Deployment) und
   der OIDC-Provider neu gestartet, jeweils mit Warten bis sie bereit sind.
5. **OpenBao**: Das `bound_subject` jeder Agenten-Rolle wird auf die neue SPIFFE-ID gesetzt (lesen, ändern,
   schreiben: alle anderen Felder bleiben).
6. **Verbraucher** werden neu geschrieben und neu gestartet.
7. **Prüfung**: Jede Stelle hat die neue Domain (die Übereinstimmung, die `doctor` für die Trust Domain prüft). Der
   Exit-Code ist 1, wenn nicht, und die Fehlermeldung sagt, welcher Schritt fehlschlug und wo das Backup liegt.

`--timeout <sekunden>` (Standard 300) gilt für jedes Warten; eine Zeitüberschreitung ist ein Fehler, der nennt, worauf
gewartet wurde.

Um zum Zustand vor einer Änderung zurückzukehren, siehe [Aus einem Backup wiederherstellen](#aus-einem-backup-wiederherstellen).

**Es verweigert** (mit Grund, ohne etwas zu ändern), wenn sich die Daten des SPIRE-Servers nicht einfach neu anlegen
lassen – der `DataStore "sql"` ist nicht `sqlite3`, oder seine Datei bzw. die Schlüssel des `KeyManager "disk"`
liegen nicht auf einem Volume Claim Template des StatefulSet – und wenn die Konfiguration eine Föderation enthält
(`federation { ... }` oder `federates_with`): Das Ändern föderierter Trust Domains liegt außerhalb des Umfangs. Es
verweigert auch, wenn der Cluster neben der neuen noch mehr als eine Domain enthält, bei einem Verbraucher, den es
nicht gibt, und bei einem StatefulSet, das auf 0 skaliert ist, ohne die Notiz.

**Ein Lauf wiederholen.** `trust-domain set <aktuelle domain>` gibt `already <domain>, nothing to do` aus. Ist ein
Lauf mittendrin gescheitert (eine Zeitüberschreitung, eine verlorene Verbindung), führe denselben Befehl erneut aus:
Der Plan zeigt den gemischten Zustand (Stellen, die schon die neue Domain haben, bleiben unberührt), und `--apply`
bringt die Arbeit zu Ende. Das Volume des Servers wird nur neu angelegt, solange seine Bundle-CA noch die alte
Domain hat. Behalte das erste Backup: Ein zweiter Lauf schreibt ein neues, das schon einen Teil der Änderung
enthält. Nennt im Cluster nichts mehr die alte Domain, hängt aber ein Verbraucher noch unfertig, übergib
`--from <alte domain>` zusammen mit der Verbraucherdatei.

### Verbraucher

Auch andere Systeme halten die Trust Domain (eine ACL, die Agenten über ihre SPIFFE-ID nennt, die Konfiguration
eines Servers). Das Werkzeug kann sie nicht kennen, daher listest du sie in einer TOML-Datei auf
([Beispiel für scion](../../examples/k8-ops/trust-domain-consumers.toml)):

```toml
[[configmap]]            # rewritten
namespace = "scion"
name = "scion-server"
keys = ["acl.toml", "scion-server.toml"]   # optional: all keys of the ConfigMap by default

[[restart]]              # restarted after the new bundle is in place
kind = "Deployment"      # Deployment | StatefulSet | DaemonSet
namespace = "scion"
name = "scion-server"
```

Im Text eines Verbrauchers werden nur `spiffe://<old>` gefolgt von `/`, einem Anführungszeichen, Leerraum oder dem
Textende ersetzt sowie die genauen Zeichenketten `"<old>"` und `'<old>'`. Ein bloßes Vorkommen (`foo.<old>`,
`https://<old>/`) bleibt stehen, und der Plan führt solche Zeilen als Warnung auf, denn sie brauchen vielleicht eine
Entscheidung von Hand. Die ConfigMaps werden neu geschrieben, nachdem das neue Bundle steht, und die Workloads
danach neu gestartet.

**Fehlt ein Verbraucher in der Datei**, behält er die alte Domain: Seine ACLs passen nicht zu den neuen
SPIFFE-IDs, und seine Workloads können die neuen SVIDs nicht prüfen, er bricht also, bis du ihn änderst. Suche im
Cluster nach der alten Domain, bevor du anwendest (`kubectl get configmaps -A -o yaml | grep old.example`).

### Aus einem Backup wiederherstellen

`trust-domain restore <backup>` bringt den Cluster in den Zustand zurück, den ein Backup von `trust-domain set`
festhält: die Trust Domain, die der Cluster vor diesem `set` hatte (das `from` des Backups).

```
$ grauwolf-k8-ops trust-domain restore trust-domain-backup-20261114T153000Z.json
$ grauwolf-k8-ops trust-domain restore trust-domain-backup-20261114T153000Z.json --apply
```

Optionen: `--apply`, `--yes`, `--backup <datei>` (wohin das Backup des aktuellen Zustands geht) und `--timeout
<sekunden>`, wie bei `set`. Ohne `--apply` gibt es nur den Plan aus; mit ihm fragt es nach (`--yes` überspringt die
Frage; ohne Terminal und ohne `--yes` bricht es ab). Eine Datei, die kein Backup dieses Werkzeugs ist (kein JSON, von
einem anderen Programm geschrieben, ein unbekanntes Format, kein gültiges `from`), wird mit Grund abgelehnt.

**Der Plan** zeigt je ConfigMap des Backups und je Schlüssel die Zeilen, die sich vom Backup unterscheiden (`-` ist,
was der Cluster jetzt hat, `+` das, was die Wiederherstellung schreibt), die Daten des SPIRE-Servers, falls sie neu
angelegt werden müssen, die Neustarts und die OpenBao-Rollen mit den Feldern, die sich ändern. Eine Wiederherstellung
schreibt ganze Schlüssel zurück, macht also auch jede andere Änderung an diesen Zeilen seit dem Backup rückgängig.
Solche Zeilen (die keine der beiden Trust Domains nennen) werden gesondert als Warnung aufgeführt; sieh sie dir an,
bevor du anwendest.

**Was es tut**, in dieser Reihenfolge (wie `set`, nur in die andere Richtung):

1. **Backup des aktuellen Zustands** (`--backup`, Standard `trust-domain-backup-<UTC-Zeitstempel>.json`; dasselbe
   Format, überschreibt nie eine Datei, ändert nichts, wenn es nicht geschrieben werden kann). Eine Wiederherstellung
   lässt sich rückgängig machen, indem man *diese* Datei wiederherstellt.
2. **ConfigMaps**: Nur die `data` der ConfigMaps aus dem Backup werden geschrieben (ein Merge-Patch mit den Schlüsseln
   des Backups; Schlüssel, die im Cluster, aber nicht im Backup stehen, bleiben). Eine ConfigMap, die nicht mehr
   existiert, wird namentlich abgelehnt.
3. **Die Daten des Servers werden neu angelegt**, wenn seine CA die wiederherzustellende Domain nicht hat (wie `set`:
   auf 0 skalieren, die Volume Claim löschen, zurück skalieren).
4. **Wartet** auf eine CA der wiederhergestellten Domain im Bundle und startet dann die SPIRE-Agenten,
   spire-controller-manager (falls vorhanden), den OIDC-Provider und die im Backup aufgeführten Verbraucher-Workloads neu.
5. **OpenBao-Rollen**: Jedes Feld der Rollen aus dem Backup wird zurückgeschrieben (lesen, ändern, schreiben), aber nur
   für Agenten, die es noch gibt. Die Rolle eines Agenten, der seitdem entfernt wurde, wird übersprungen (der Plan sagt
   es) und nicht zurückgegeben. Rollen von Agenten, die seitdem hinzugekommen sind, bleiben unverändert; die Prüfung
   meldet sie als Warnungen, und `agent add <name>` bindet sie an die aktuelle Domain.
6. **Prüfung** wie bei `set`: die wiederhergestellte Domain an jeder Stelle, jede Rolle an das richtige Subject
   gebunden. Der Exit-Code ist sonst 1, und die Meldung nennt den fehlgeschlagenen Schritt und beide Dateien: das
   Backup, aus dem wiederhergestellt wird, und das Backup des Zustands davor.

Es verweigert, was `set` verweigert, wo es zutrifft: Föderation und (wenn die Daten des Servers neu angelegt werden
müssen) einen Datenspeicher, der nicht SQLite auf einem Volume Claim Template ist.

**Was es nicht wiederherstellt:** Metadaten, Labels und Annotationen der ConfigMaps (nur `data`); Workloads außer durch
einen Neustart; Secrets (keines steckt in einem Backup); seit dem Backup hinzugekommene oder entfernte Agenten; und
die alte Datenbank und CA des SPIRE-Servers: Die Daten werden neu angelegt, daher bekommt jeder Workload wieder eine
neue SVID.

**Ein Lauf wiederholen.** Entspricht der aktuelle Zustand schon dem Backup (seine ConfigMap-Daten, seine Rollen und
eine CA seiner Domain im Bundle), gibt es `already as in the backup, nothing to do` aus, endet mit 0 und schreibt kein
Backup. Eine unterbrochene Wiederherstellung (eine Zeitüberschreitung, eine verlorene Verbindung) wird beendet, indem
man sie mit derselben Backup-Datei und einer neuen `--backup`-Datei erneut ausführt; der Plan zeigt, was übrig ist.

Von Hand (etwa bei einem Backup, dessen ConfigMaps nicht mehr existieren): Das Backup ist einfaches JSON mit den
Objekten, wie sie waren. Hole ein Objekt heraus (`jq '.configmaps[] | select(.metadata.name=="spire-server")' backup.json`),
lege es an oder patche es (`kubectl patch configmap spire-server -n spire-system --type merge -p '{"data": {...}}'`),
und führe `restore` erneut aus, oder starte die Workloads neu und schreibe die Rollen (`openbao_roles`: `mount`,
`name`, `data`) mit `bao write auth/<mount>/role/<name> @-` zurück.

## Wenn etwas nicht stimmt

Führe `doctor` aus: Seine Hinweise nennen den Befehl, der die Zeile behebt. Typische Fälle: *die Trust Domain ist
nicht überall gleich* (maßgeblich ist der Wert des SPIRE-Servers; wurde ein `trust-domain set` unterbrochen, führe es
erneut aus, um es zu beenden), *die Rolle eines Agenten ist an ein anderes Subject gebunden* (`agent add <name>`
erneut ausführen), *die alte gemeinsame Rolle `grauwolf-agents`* (aus dem Beispiel: Sie erlaubt ihren Agenten, die
Geheimnisse aller Agenten zu lesen; Rollen pro Agent ersetzen sie).

Ein Aufruf eines anderen Agenten oder MCP-Servers mit falschem `auth.spiffe` scheitert so:

```text
A2A over HTTP: TLS handshake with https://reviewer.reviewer.svc:8443/.well-known/agent-card.json failed: the server is spiffe://agentswork.de/ns/reviewer/sa/reviewer, expected spiffe://agentswork.de/ns/intruder/sa/intruder; the call was not executed
```

Setze `auth.spiffe.id` auf die Identität, die die Meldung nennt (`grauwolf-k8-ops agent list` zeigt sie).
„no connection … (refused or given up …)“ heißt dagegen, dass der Port nicht erreichbar ist: Der Agent
ist nicht ausgerollt, oder eine NetworkPolicy lässt den Aufrufer nicht durch
(`grauwolf-k8-ops agent callers <ziel>`).

## Grenzen dieser Stufe

- `trust-domain restore` stellt nur wieder her, was ein Backup von `trust-domain set` enthält (ConfigMap-Daten,
  Rollen); es kann die alte Datenbank und CA des SPIRE-Servers nicht zurückbringen.
- Das Programm ist noch nicht Teil der Container-Images.
- `trust-domain set` und `restore` schreiben die Rollen der Agenten um, nicht die Rolle `grauwolf-identity` des Identitätsdienstes: Führe danach `identity install` erneut aus (es bindet die Rolle an die aktuelle Trust Domain).
- Wofür ein Agent seine Identität außer für Aufrufe von Agenten und MCP-Servern nutzen darf (`fetch.auth`, `git.auth`, `workload_api`), steht in der Spec ([Kapitel 3](03-tools.md)); `agent deploy` ändert sich dafür nicht, aber der Pod braucht den SPIFFE-Socket wie bei jeder Nutzung der Identität. [`examples/test-git-server/`](../../examples/test-git-server/README.md) hat einen Test-Git-Server, um `git.auth` im eigenen Cluster auszuprobieren.
- Nur ein Replikat; `agent deploy` skaliert nicht, richtet keine Autoskalierung ein und verwaltet kein TLS.
- Ein wiederholter Lauf vergleicht mit dem, was das Werkzeug zuletzt angewendet hat (die Annotation
  `grauwolf.io/applied-hash` an jedem Objekt); er bemerkt also keine Handbearbeitung an einem Objekt, das es verwaltet.
  `doctor` prüft die Teile, die für den Zugriff wichtig sind (Rolle, Policy, Labels).

Zu den Interna (die Crates, `FakeCluster`, wie ein Befehl hinzukommt, die Fallen aus den Live-Tests) siehe
[Entwicklerhandbuch, Kapitel 9](../entwickler/09-identitaet-zugangsdaten-und-cluster-werkzeug.md).
