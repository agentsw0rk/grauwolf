use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Sets up SPIRE and OpenBao for grauwolf agents on Kubernetes (ADR-0120).
#[derive(Debug, Parser)]
#[command(name = "grauwolf-k8-ops", version)]
pub struct Cli {
    #[command(flatten)]
    pub settings: Settings,
    #[command(subcommand)]
    pub command: Command,
}

/// Where things are. The defaults fit a cluster set up as in `examples/spire-openbao/`.
#[derive(Debug, Clone, Args)]
pub struct Settings {
    /// Kubeconfig file (default: `KUBECONFIG`, then `~/.kube/config`).
    #[arg(long, global = true, value_name = "FILE")]
    pub kubeconfig: Option<PathBuf>,
    /// Kubeconfig context to use (default: the current context of the kubeconfig).
    #[arg(long, global = true, value_name = "NAME")]
    pub context: Option<String>,
    /// Namespace of the SPIRE server and agents.
    #[arg(
        long,
        global = true,
        default_value = "spire-system",
        value_name = "NAMESPACE"
    )]
    pub spire_namespace: String,
    /// Namespace of the SPIRE OIDC discovery provider.
    #[arg(
        long,
        global = true,
        default_value = "spire-oidc",
        value_name = "NAMESPACE"
    )]
    pub oidc_namespace: String,
    /// Namespace of OpenBao (for the API service proxy).
    #[arg(
        long,
        global = true,
        default_value = "openbao",
        value_name = "NAMESPACE"
    )]
    pub openbao_namespace: String,
    /// `name:port` of the OpenBao service (for the API service proxy).
    #[arg(
        long,
        global = true,
        default_value = "openbao:8200",
        value_name = "NAME:PORT"
    )]
    pub openbao_service: String,
    /// Reach OpenBao directly at this address instead of through the Kubernetes API.
    #[arg(long, global = true, value_name = "URL")]
    pub openbao_addr: Option<String>,
    /// File with the OpenBao token (default: the environment variable `OPENBAO_TOKEN`).
    #[arg(long, global = true, value_name = "FILE")]
    pub token_file: Option<PathBuf>,
    /// Mount path of the JWT auth method.
    #[arg(long, global = true, default_value = "jwt", value_name = "MOUNT")]
    pub jwt_mount: String,
    /// Mount path of the KV v2 secrets engine.
    #[arg(long, global = true, default_value = "kv", value_name = "MOUNT")]
    pub kv_mount: String,
    /// Audience of the JWT-SVIDs the agents log in with.
    #[arg(
        long,
        global = true,
        default_value = "openbao",
        value_name = "AUDIENCE"
    )]
    pub audience: String,
    /// Address of OpenBao as the agents see it, written into their configuration
    /// (default: `http://<service>.<namespace>.svc:<port>`).
    #[arg(long, global = true, value_name = "URL")]
    pub agent_openbao_address: Option<String>,
}

impl Settings {
    /// `(name, port)` of the OpenBao service.
    pub fn openbao_service_parts(&self) -> (&str, &str) {
        self.openbao_service
            .split_once(':')
            .unwrap_or((&self.openbao_service, "8200"))
    }

    /// The OpenBao address written into the configuration of agents.
    pub fn agent_address(&self) -> String {
        self.agent_openbao_address.clone().unwrap_or_else(|| {
            let (name, port) = self.openbao_service_parts();
            format!("http://{name}.{}.svc:{port}", self.openbao_namespace)
        })
    }
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once, see `AgentCommand`
pub enum Command {
    /// Check the prerequisites and the setup; exit code 1 if a check fails (read-only).
    Doctor {
        /// Print the checks as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The SPIRE OIDC discovery provider.
    Oidc {
        #[command(subcommand)]
        command: OidcCommand,
    },
    /// The OpenBao side of the setup.
    Openbao {
        #[command(subcommand)]
        command: OpenbaoCommand,
    },
    /// Agents: a namespace, an identity and an OpenBao role each.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Building harness images in the cluster (ADR-0129): the credentials of the build Jobs.
    Build {
        #[command(subcommand)]
        command: BuildCommand,
    },
    /// The secrets of an agent in OpenBao.
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    /// The identity service of the trust domain (ADR-0111): `grauwolf identity serve` in
    /// namespace `identity`, the Transit key that signs the Txn-Tokens and the policy of the agents
    /// that may act on behalf of people.
    Identity {
        #[command(subcommand)]
        command: IdentityCommand,
    },
    /// Schedules (ADR-0111 stage 3): CronJobs in namespace `schedules` that start runs on a
    /// person's grant with their own SPIFFE identity.
    Schedule {
        #[command(subcommand)]
        command: ScheduleCommand,
    },
    /// The trust domain of SPIRE and everything that holds it.
    TrustDomain {
        #[command(subcommand)]
        command: TrustDomainCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TrustDomainCommand {
    /// Change the trust domain. Without `--apply` it only prints the plan.
    Set {
        /// The new trust domain (lowercase letters, digits, `.`, `-`, `_`).
        domain: String,
        /// The domain to replace, for finishing an interrupted run when nothing in the cluster
        /// names it any more (default: the one the cluster holds).
        #[arg(long, value_name = "DOMAIN")]
        from: Option<String>,
        /// TOML file with more ConfigMaps to rewrite and workloads to restart
        /// (`[[configmap]]`, `[[restart]]`).
        #[arg(long, value_name = "FILE")]
        consumers: Option<PathBuf>,
        /// Do it (after the plan, a confirmation and a backup).
        #[arg(long)]
        apply: bool,
        /// Do not ask for confirmation.
        #[arg(long, requires = "apply")]
        yes: bool,
        /// Where the backup of the touched objects is written first
        /// (default: `trust-domain-backup-<UTC timestamp>.json` in the working directory).
        #[arg(long, value_name = "FILE", requires = "apply")]
        backup: Option<PathBuf>,
        /// Give up after this many seconds on each wait.
        #[arg(long, default_value_t = 300, value_name = "SECONDS")]
        timeout: u64,
    },
    /// Put the cluster back as a backup of `set` (or of an earlier restore) has it. Without
    /// `--apply` it only prints the plan.
    Restore {
        /// The backup file written by `trust-domain set` or `restore`.
        #[arg(value_name = "BACKUP")]
        file: PathBuf,
        /// Do it (after the plan, a confirmation and a backup of the current state).
        #[arg(long)]
        apply: bool,
        /// Do not ask for confirmation.
        #[arg(long, requires = "apply")]
        yes: bool,
        /// Where the backup of the current state is written first
        /// (default: `trust-domain-backup-<UTC timestamp>.json` in the working directory).
        #[arg(long, value_name = "FILE", requires = "apply")]
        backup: Option<PathBuf>,
        /// Give up after this many seconds on each wait.
        #[arg(long, default_value_t = 300, value_name = "SECONDS")]
        timeout: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum OidcCommand {
    /// Install (or adopt) the discovery provider in its own namespace.
    Install,
}

#[derive(Debug, Subcommand)]
pub enum OpenbaoCommand {
    /// Enable the JWT auth method with the provider's keys and the KV v2 mount.
    Setup {
        /// Where OpenBao fetches the signing keys
        /// (default: `http://spire-oidc-discovery-provider.<oidc namespace>.svc/keys`).
        #[arg(long, value_name = "URL")]
        jwks_url: Option<String>,
        /// `bound_issuer` of the auth method, if the SPIRE server sets `jwt_issuer`.
        #[arg(long, value_name = "ISSUER")]
        bound_issuer: Option<String>,
    },
    /// Create a token with only the policy `grauwolf-k8-ops` (what this tool needs, written by
    /// `setup`) and print it on stdout, nothing else: work without the root token. It needs a
    /// token that may create tokens (root or an admin token).
    Token {
        /// How long the token is valid (`30m`, `8h`, `7d`).
        #[arg(long, default_value = crate::openbao_setup::DEFAULT_TTL, value_name = "DURATION")]
        ttl: String,
        /// The token can be renewed until its maximum lifetime.
        #[arg(long)]
        renewable: bool,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once; boxing every field of `Deploy` would only add noise
pub enum AgentCommand {
    /// Create or update an agent: namespace, service account, identity, OpenBao role and policy.
    Add {
        /// The agent's name (a DNS-1123 label).
        name: String,
        /// Namespace of the agent (default: its name).
        #[arg(long, value_name = "NAMESPACE")]
        namespace: Option<String>,
        /// Copy this image pull secret (`namespace/name`) into the agent's namespace and attach
        /// it to the service account.
        #[arg(long, value_name = "NAMESPACE/NAME")]
        pull_secret: Option<String>,
    },
    /// List the agents.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Remove an agent: its namespace and its OpenBao role and policy.
    Remove {
        name: String,
        /// Also delete the agent's secrets under `<kv>/agents/<name>/`.
        #[arg(long)]
        purge_secrets: bool,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Let another agent call this one over the mTLS port: its SPIFFE ID goes into the ConfigMap
    /// `grauwolf-callers` and its namespace into the NetworkPolicy of the target (ADR-0109).
    /// Repeatable: it says `created`, `updated` or `unchanged`.
    Allow {
        /// The agent that is called.
        target: String,
        /// The agent that may call it.
        #[arg(long, value_name = "AGENT")]
        from: String,
        /// The user the caller is mapped to (default: the caller's name).
        #[arg(long = "as", value_name = "USER")]
        user: Option<String>,
    },
    /// Take the permission back: remove the caller from `grauwolf-callers` and the NetworkPolicy.
    Deny {
        target: String,
        #[arg(long, value_name = "AGENT")]
        from: String,
    },
    /// List the agents that may call this one, their users and whether the NetworkPolicy admits
    /// them.
    Callers {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Run the credentials check of ADR-0110 as a Job in the agent's namespace.
    Check {
        name: String,
        /// The grauwolf image to run.
        #[arg(long)]
        image: String,
        /// A configuration variable to resolve (repeatable; default: every variable mapped in
        /// the agent's configuration), all in one Job.
        #[arg(long = "variable", value_name = "NAME")]
        variables: Vec<String>,
        /// Give up after this many seconds.
        #[arg(long, default_value_t = 120, value_name = "SECONDS")]
        timeout: u64,
    },
    /// Build the image of a harness in the cluster with Kaniko (ADR-0129): the project folder of
    /// the harness (committed and pushed) is cloned by a Job in namespace `grauwolf-builds` and the
    /// image pushed to the harness's `image`. Exit code 0 when the image is pushed, 1 otherwise.
    Build {
        /// The harness file (`<name>.harness.json`, ADR-0128).
        #[arg(long, value_name = "FILE")]
        harness: PathBuf,
        /// The generated project folder (default: `build/<name>` of the working directory).
        #[arg(long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// The namespace of the build Job.
        #[arg(long, default_value = grauwolf_k8s::BUILD_NAMESPACE, value_name = "NAMESPACE")]
        namespace: String,
        /// The Kaniko image.
        #[arg(long, default_value = grauwolf_k8s::KANIKO_IMAGE, value_name = "IMAGE")]
        kaniko_image: String,
        /// An existing Secret of the build namespace with the registry credentials (type
        /// `kubernetes.io/dockerconfigjson`; `build secret registry` makes one).
        #[arg(long, value_name = "NAME")]
        registry_secret: Option<String>,
        /// An existing Secret of the build namespace with the keys `username` and `password` for
        /// a private git repository (`build secret git` makes one).
        #[arg(long, value_name = "NAME")]
        git_secret: Option<String>,
        /// Give up after this many seconds (the Job is deleted).
        #[arg(long, default_value_t = 1800, value_name = "SECONDS")]
        timeout: u64,
    },
    /// Run the agent in its namespace: Deployment, Service, optionally an Ingress (ADR-0121).
    /// Repeatable: it applies only what differs and restarts the pod only when the spec, the
    /// image, the environment or the configuration changed.
    Deploy {
        /// The agent; with `--harness` its `name`.
        #[arg(required_unless_present = "harness", conflicts_with = "harness")]
        name: Option<String>,
        /// Take the settings from this harness file (`<name>.harness.json`, ADR-0128): the agent's
        /// name, its `image` (else `<registry>/<name>:<version>` with `[harness] registry` of the
        /// nearest `grauwolf.toml`), `port` and `env`.
        #[arg(long, value_name = "FILE")]
        harness: Option<PathBuf>,
        /// The image: the grauwolf CLI with `--spec`, else a generated service
        /// (`spec generate --service --dockerfile`). With `--harness` its `image`.
        #[arg(long, required_unless_present = "harness")]
        image: Option<String>,
        /// Run `grauwolf spec serve` on this spec and its agent tree (prompts, skills,
        /// sub-agents, knowledge files), stored in a ConfigMap of about 1 MiB at most.
        #[arg(long, value_name = "FILE")]
        spec: Option<PathBuf>,
        /// A value that is not secret for the pod's environment (repeatable), e.g.
        /// `OLLAMA_HOST=http://ollama.ollama.svc:11434`. It is stored in the Deployment, not in a
        /// Secret; secrets use `secret set --variable`.
        #[arg(long = "env", value_name = "NAME=VALUE")]
        env: Vec<String>,
        /// Also an Ingress for this host (TLS and the entry point are left to the cluster).
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        /// The port of the service in the container and of the Kubernetes Service [default: 8080;
        /// with `--harness` its `port`].
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        /// The port of the mTLS listener (`GRAUWOLF_SPIFFE_PORT`) in the container and of the
        /// Kubernetes Service; callers come in by their SPIFFE ID (`agent allow`).
        #[arg(long, default_value_t = 8443, value_name = "PORT")]
        mtls_port: u16,
        /// Keep the data on a PersistentVolumeClaim of this size (e.g. `1Gi`) instead of an
        /// `emptyDir`.
        #[arg(long, value_name = "QUANTITY")]
        data_size: Option<String>,
        /// CPU request and limit (e.g. `500m`); without it none is set.
        #[arg(long, value_name = "QUANTITY")]
        cpu: Option<String>,
        /// Memory request and limit (e.g. `512Mi`); without it none is set.
        #[arg(long, value_name = "QUANTITY")]
        memory: Option<String>,
        /// Copy this image pull secret (`namespace/name`) into the agent's namespace and attach
        /// it to the service account.
        #[arg(long, value_name = "NAMESPACE/NAME")]
        pull_secret: Option<String>,
        /// Send the agent's traces (OpenTelemetry, OTLP/HTTP) to `--otel-endpoint`: runs
        /// `spec serve --otel` (an image without `--spec`: `GRAUWOLF_OTEL=1`) with
        /// `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME` (the agent's name). Like every
        /// option it is only on while it is given: a deploy without it switches tracing off again.
        #[arg(long)]
        otel: bool,
        /// The OTLP/HTTP base URL of the collector (the SDK adds `/v1/traces`).
        #[arg(long, default_value = crate::deploy::DEFAULT_OTEL_ENDPOINT, value_name = "URL", requires = "otel")]
        otel_endpoint: String,
        /// Also record the contents of prompts and answers in the spans (`spec serve --otel-content`,
        /// an image: `GRAUWOLF_OTEL_CONTENT=1`).
        #[arg(long, requires = "otel")]
        otel_content: bool,
        /// Write every event of the agent's runs as a JSON line to stderr, so it is in the pod
        /// log (`agent logs --events-only`): runs `spec serve --events` (an image without
        /// `--spec`: `GRAUWOLF_EVENTS=1`). Like every option it is only on while it is given.
        #[arg(long)]
        events: bool,
        /// Let the agent act on behalf of people (ADR-0111): sets `GRAUWOLF_IDENTITY_URL` (the mTLS
        /// URL of the identity service of `identity install`) and `GRAUWOLF_IDENTITY_SPIFFE_ID`, so
        /// the agent exchanges people's tokens and verifies Txn-Tokens; with `--egress` the
        /// identity service is an allowed destination. Like every option it is only on while it is
        /// given. An agent that requests tokens also needs `identity allow <agent>`.
        #[arg(long)]
        identity: bool,
        /// Limit what the agent may reach (needs `--spec`): a NetworkPolicy `grauwolf-egress` made
        /// from `grauwolf spec egress` of the spec tree, with the addresses `deploy` knows (`--env`,
        /// the OpenBao address, the `--otel` endpoint). DNS is always allowed; cluster services by
        /// namespace and pod port; IP addresses as /32; external host names only by port (a
        /// NetworkPolicy cannot match names; they are listed as a warning). Like every option it is
        /// only on while it is given: a deploy without it deletes the policy.
        #[arg(long)]
        egress: bool,
        /// With `--egress`: leave the external host names out, so they are blocked.
        #[arg(long, requires = "egress")]
        egress_strict: bool,
        /// With `--egress`: the pod and service ranges of the cluster, which "all addresses" for
        /// the external names excludes (repeatable; default: the k3s ranges 10.42.0.0/16 and
        /// 10.43.0.0/16).
        #[arg(long, requires = "egress", value_name = "CIDR")]
        cluster_cidr: Vec<String>,
        /// With `--egress`: the namespace of the cluster's DNS pods.
        #[arg(long, requires = "egress", default_value = crate::egress::DEFAULT_DNS_NAMESPACE, value_name = "NAMESPACE")]
        egress_dns_namespace: String,
        /// With `--egress`: the label of the cluster's DNS pods (`key=value`).
        #[arg(long, requires = "egress", default_value = crate::egress::DEFAULT_DNS_SELECTOR, value_name = "KEY=VALUE")]
        egress_dns_selector: String,
        /// Give up waiting for the rollout and `/health` after this many seconds.
        #[arg(long, default_value_t = 300, value_name = "SECONDS")]
        timeout: u64,
    },
    /// Show whether the agent runs: pod, restarts, image, endpoint, host, server token.
    Status {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the log of the agent's pod.
    Logs {
        name: String,
        /// Keep streaming new lines.
        #[arg(long, short)]
        follow: bool,
        /// Start with the last N lines.
        #[arg(long, default_value_t = 100, value_name = "N")]
        tail: u32,
        /// Print only the JSON event lines (`agent deploy --events`), not the other log lines.
        #[arg(long)]
        events_only: bool,
    },
    /// Remove what `deploy` made (Deployment, Service, Ingress, spec ConfigMap); keeps the
    /// identity, the role and the secrets, and the data volume unless `--purge-data`.
    Undeploy {
        name: String,
        /// Also delete the PersistentVolumeClaim of `--data-size`.
        #[arg(long)]
        purge_data: bool,
        /// Do not ask for confirmation of `--purge-data`.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum BuildCommand {
    /// Secrets of the build namespace; the token is read from stdin or a hidden prompt, never from
    /// an argument.
    Secret {
        #[command(subcommand)]
        command: BuildSecretCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum BuildSecretCommand {
    /// Create or update the Secret with the credentials of a container registry
    /// (`agent build --registry-secret <name>`).
    Registry {
        name: String,
        /// The registry's server, e.g. `ghcr.io`.
        #[arg(long, value_name = "SERVER")]
        server: String,
        #[arg(long, value_name = "USER")]
        username: String,
        /// The namespace of the Secret (the build namespace of `agent build`).
        #[arg(long, default_value = grauwolf_k8s::BUILD_NAMESPACE, value_name = "NAMESPACE")]
        namespace: String,
    },
    /// Create or update the Secret with the git credentials for a private repository
    /// (`agent build --git-secret <name>`): the keys `username` and `password`.
    Git {
        name: String,
        #[arg(long, value_name = "USER")]
        username: String,
        /// The namespace of the Secret (the build namespace of `agent build`).
        #[arg(long, default_value = grauwolf_k8s::BUILD_NAMESPACE, value_name = "NAMESPACE")]
        namespace: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecretCommand {
    /// Store a value read from stdin (or a hidden prompt) at `<kv>/agents/<agent>/<key>`.
    Set {
        agent: String,
        key: String,
        /// Also map this configuration variable to the secret in the agent's `grauwolf.toml`.
        #[arg(long, value_name = "NAME")]
        variable: Option<String>,
    },
    /// List the keys and versions of an agent's secrets (never the values).
    List {
        agent: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the value of a secret (stdout, nothing else). Refuses without `--reveal`.
    Get {
        agent: String,
        key: String,
        /// Really print the value.
        #[arg(long)]
        reveal: bool,
    },
    /// Delete a secret with all its versions.
    Delete { agent: String, key: String },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)] // parsed once, see `AgentCommand`
pub enum IdentityCommand {
    /// Install or update the identity service: namespace `identity` (restricted), its SPIFFE
    /// identity, Deployment, Service (HTTP and mTLS port), policy ConfigMap and NetworkPolicies, and
    /// in OpenBao the Transit key, a policy that may only sign with it and a JWT role bound to the
    /// service's SPIFFE ID. Repeatable: it says `created`, `updated` or `unchanged`.
    Install {
        /// The Entra tenant ID (`tid` of people's tokens).
        #[arg(long, value_name = "ID")]
        tenant: String,
        /// The issuer of people's tokens (`iss`, e.g. `https://login.microsoftonline.com/<tenant>/v2.0`).
        #[arg(long, value_name = "URL")]
        issuer: String,
        /// The audiences of people's tokens: the application ID of the grauwolf identity API
        /// (v2.0 tokens) and its application ID URI (v1.0 tokens, Entra Local's token generator).
        /// Repeatable; a token is accepted when its `aud` is any of them.
        #[arg(long, value_name = "AUDIENCE", required = true)]
        audience: Vec<String>,
        /// Where the keys of people's tokens are (default: the OpenID discovery of the issuer).
        #[arg(long, value_name = "URL")]
        jwks_url: Option<String>,
        /// The scope people's tokens must carry (`scp`).
        #[arg(long, default_value = crate::identity::DEFAULT_SCOPE, value_name = "SCOPE")]
        scope: String,
        /// `iss` of the Txn-Tokens (default: the mTLS URL of the service in the cluster).
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// The policy file (`{"callers": {"<spiffe id>": {"purposes": [...]}}}`); replaces the
        /// policy in the cluster. Without it the policy is kept (`identity allow` edits it).
        #[arg(long, value_name = "FILE")]
        policy: Option<PathBuf>,
        /// The grauwolf CLI image (it runs `grauwolf identity serve`).
        #[arg(long)]
        image: String,
        /// Copy this image pull secret (`namespace/name`) into namespace `identity` and attach it
        /// to the service account.
        #[arg(long, value_name = "NAMESPACE/NAME")]
        pull_secret: Option<String>,
        /// The OpenBao Transit key that signs the Txn-Tokens (type ecdsa-p256, mount `transit/`).
        #[arg(long, default_value = crate::identity::DEFAULT_TRANSIT_KEY, value_name = "NAME")]
        transit_key: String,
        /// The HTTP port (health, keys, configuration).
        #[arg(long, default_value_t = crate::identity::DEFAULT_PORT, value_name = "PORT")]
        port: u16,
        /// The mTLS port the agents call.
        #[arg(long, default_value_t = crate::identity::DEFAULT_MTLS_PORT, value_name = "PORT")]
        mtls_port: u16,
        /// The pod and service ranges of the cluster, which "all addresses" for an external keys
        /// host excludes (repeatable; default: the k3s ranges 10.42.0.0/16 and 10.43.0.0/16).
        #[arg(long, value_name = "CIDR")]
        cluster_cidr: Vec<String>,
        /// The namespace of the cluster's DNS pods (egress).
        #[arg(long, default_value = crate::egress::DEFAULT_DNS_NAMESPACE, value_name = "NAMESPACE")]
        egress_dns_namespace: String,
        /// The label of the cluster's DNS pods (`key=value`, egress).
        #[arg(long, default_value = crate::egress::DEFAULT_DNS_SELECTOR, value_name = "KEY=VALUE")]
        egress_dns_selector: String,
        /// Give up waiting for the rollout and `/health` after this many seconds.
        #[arg(long, default_value_t = 300, value_name = "SECONDS")]
        timeout: u64,
        /// The application ID of the public client "grauwolf CLI": enables grants (ADR-0111
        /// stage 3); the service redeems the grants' refresh tokens as this client and keeps them
        /// in `<kv mount>/identity/`.
        #[arg(long, value_name = "ID")]
        grant_client_id: Option<String>,
    },
    /// Every person's grants (ADR-0111 stage 3) from OpenBao – metadata only, never a refresh
    /// token –, or with `--revoke` one grant deleted (metadata and refresh token).
    Grants {
        /// Revoke this grant (`grt_…`).
        #[arg(long, value_name = "GRANT")]
        revoke: Option<String>,
        #[arg(long, conflicts_with = "revoke")]
        json: bool,
    },
    /// Show whether the service runs, its URLs, configuration, policy and OpenBao objects.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Remove the service: namespace `identity`, ClusterSPIFFEID, OpenBao role and policy; the
    /// Transit key stays unless `--delete-key`.
    Uninstall {
        /// Also delete the Transit key (Txn-Tokens it signed can then no longer be verified).
        #[arg(long)]
        delete_key: bool,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Let an agent request Txn-Tokens on behalf of people (an entry in the service's policy);
    /// with `--grants`, let it run on people's grants; with `--trigger`, let a workload start grant
    /// runs. Repeatable: it says `created`, `updated` or `unchanged`.
    Allow {
        /// The agent (not with `--trigger`).
        #[arg(required_unless_present = "trigger")]
        agent: Option<String>,
        /// The `grants` section: the agent may run on grants (ADR-0111 stage 3).
        #[arg(long)]
        grants: bool,
        /// A trigger of grant runs: a schedule's name or a SPIFFE ID (`…/*` for a prefix).
        #[arg(long, value_name = "NAME_OR_ID", conflicts_with_all = ["agent", "grants", "purposes"])]
        trigger: Option<String>,
        /// A purpose the agent may name (repeatable); without any, every purpose.
        #[arg(long = "purpose", value_name = "PURPOSE")]
        purposes: Vec<String>,
    },
    /// Take the permission back: remove the agent (with `--grants` from the grants section) or
    /// the trigger from the service's policy.
    Deny {
        #[arg(required_unless_present = "trigger")]
        agent: Option<String>,
        #[arg(long)]
        grants: bool,
        #[arg(long, value_name = "NAME_OR_ID", conflicts_with_all = ["agent", "grants"])]
        trigger: Option<String>,
    },
}

/// `schedule …`.
#[derive(Debug, Subcommand)]
pub enum ScheduleCommand {
    /// Create or update a schedule: namespace `schedules` (restricted), a service account and a
    /// ClusterSPIFFEID for `spiffe://<td>/ns/schedules/sa/<name>`, a NetworkPolicy (egress: DNS and
    /// the agent's mTLS port), the CronJob running `grauwolf call <agent> --grant …` with the SPIFFE
    /// CSI volume, and the schedule as a caller of the agent (callers file and NetworkPolicy).
    /// Repeatable: it says `created`, `updated` or `unchanged`.
    Add {
        /// The schedule's name (also its service account).
        name: String,
        /// The entry agent to call.
        #[arg(long)]
        agent: String,
        /// The person's grant (`grt_…`, from `grauwolf identity grant create`).
        #[arg(long, value_name = "GRANT")]
        grant: String,
        /// When to run, as a cron line (`"0 7 * * 1-5"`, `@hourly`), in the cluster's time zone.
        #[arg(long, value_name = "CRON")]
        cron: String,
        /// The input of every run.
        #[arg(long)]
        input: String,
        /// The grauwolf CLI image (it runs `grauwolf call`).
        #[arg(long)]
        image: String,
        /// Copy this image pull secret (`namespace/name`) into namespace `schedules` and attach it
        /// to the schedule's service account.
        #[arg(long, value_name = "NAMESPACE/NAME")]
        pull_secret: Option<String>,
    },
    /// The schedules: cron line, agent, grant, last run and last success.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Remove a schedule: CronJob, NetworkPolicy, ClusterSPIFFEID, service account, and its entry
    /// in the agent's callers. The grant stays (`grauwolf identity grant revoke`).
    Remove { name: String },
    /// Start one run now: a Job from the schedule's CronJob.
    RunNow { name: String },
}
