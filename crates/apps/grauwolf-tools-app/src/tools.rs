//! The tool commands `pull|inspect|keygen|pack|push|lock|build|validate` (ADR-0107, ADR-0118): the
//! clap definition and the code behind `grauwolf-tools` and `grauwolf tools`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;

use crate::lock::{LockSummary, Locker};
use crate::options::{BuildOptions, CacheOptions, tool_builder};
use crate::outln;
use crate::output::{Io, report};

#[derive(Subcommand)]
pub enum ToolsCommand {
    /// Fetches a tool package into the cache and verifies it (digests of manifest and modules).
    ///
    /// The reference is `registry/repository[:tag][@sha256:…]`, e.g.
    /// `ghcr.io/acme/tools/math:1.0`. Registries on `localhost` and `127.0.0.1` are reached over
    /// plain http, all others over https. Credentials: `GRAUWOLF_REGISTRY_AUTH`, a JSON map
    /// registry → `"user:token"` or bearer token, else the Docker configuration
    /// (`$DOCKER_CONFIG/config.json` or `~/.docker/config.json`: `auths`, `identitytoken`,
    /// credential helpers).
    ///
    /// A git package is given as `git+<url>[//<folder>]@<commit>`, `…@tag:<tag>` or
    /// `…@branch:<branch>`, e.g. `git+https://github.com/acme/tools.git//text@tag:v1`. It is
    /// checked against the tool policy (`allowed_git_hosts`, `require_signatures`), fetched with
    /// `GIT_TOKEN` if set, built from its sources (needs the Rust toolchain) and cached; the
    /// output names the commit.
    Pull {
        /// Reference of the package.
        reference: String,
        #[command(flatten)]
        cache: CacheOptions,
        /// Prefix under which cargo runs when a git package is built (see `spec build`); give
        /// each part once.
        #[arg(long = "build-wrapper", allow_hyphen_values = true)]
        wrapper: Vec<String>,
    },
    /// Shows what a tool package contains: digest, ABI, tools with arguments and modules.
    ///
    /// For a git package (reference as for `tools pull`) it fetches the commit without building
    /// anything and shows the commit, the tools, whether the package is cached and the signature
    /// of the commit or tag; a missing signature is shown, not refused.
    Inspect {
        /// Reference of the package.
        reference: String,
        /// Git only: allowed-signers file to check the signature with (format of git's
        /// `gpg.ssh.allowedSignersFile`), instead of `git_allowed_signers` of the tool policy.
        #[arg(long)]
        allowed_signers: Option<PathBuf>,
        /// Public key (PEM, e.g. cosign.pub) to check the signature with, besides the keys of
        /// the tool policy; repeatable.
        #[arg(long = "key")]
        keys: Vec<PathBuf>,
        #[command(flatten)]
        cache: CacheOptions,
    },
    /// Creates a signing key pair for tool packages: `<name>.key` and `<name>.pub`.
    ///
    /// The private key is PKCS#8 PEM and **unencrypted**, written readable for the owner only
    /// (mode 0600): keep it secret. The public key is cosign-compatible (SPKI PEM); list it under
    /// `oci_keys` in the tool policy. Existing files are never overwritten.
    Keygen {
        /// Path without extension, e.g. `keys/acme`.
        name: PathBuf,
    },
    /// Builds the tools of a package folder and packs them as an OCI image layout.
    ///
    /// The folder holds `grauwolf-tools.json` (name, version, tools with definitions) and
    /// `src/<tool>.rs` (the code body of each tool). The output directory (default `<dir>/dist`)
    /// gets `oci-layout`, `index.json` and `blobs/sha256/…`, including the sources; the same
    /// folder always gives the same digests. Prints the manifest digest.
    Pack {
        /// The package folder.
        dir: PathBuf,
        /// Output directory of the image layout [default: <dir>/dist].
        #[arg(long)]
        out: Option<PathBuf>,
        #[command(flatten)]
        build: BuildOptions,
    },
    /// Pushes a tool package to an OCI registry and prints `<reference>@sha256:…`.
    ///
    /// The source is a package folder (packed first, see `tools pack`) or an image layout
    /// directory written by `tools pack`. The reference is `registry/repository[:tag]`
    /// (`latest` without a tag). Credentials: `GRAUWOLF_REGISTRY_AUTH`, a JSON map
    /// registry → `"user:token"` or bearer token, else the Docker configuration
    /// (`$DOCKER_CONFIG/config.json` or `~/.docker/config.json`: `auths`, `identitytoken`,
    /// credential helpers).
    Push {
        /// The package folder or the packed image layout directory.
        source: PathBuf,
        /// Where to push, `registry/repository[:tag]`.
        reference: String,
        /// Sign the pushed package with this private key (from `tools keygen`; unencrypted
        /// PKCS#8 PEM). The cosign-compatible signature is pushed next to the package; the
        /// updated tool catalog is signed with it as well.
        #[arg(long)]
        sign_key: Option<PathBuf>,
        /// The tool catalog the pushed version is added to (ADR-0124): a registry or a full
        /// reference [default: `GRAUWOLF_TOOLS_CATALOG`, else
        /// `<registry>/grauwolf-catalog:latest` of the pushed reference].
        #[arg(long)]
        catalog: Option<String>,
        /// Do not add the pushed version to the tool catalog.
        #[arg(long)]
        no_catalog: bool,
        #[command(flatten)]
        build: BuildOptions,
    },
    /// Builds the WASM modules of a crate generated with `spec generate --tool-sources`.
    ///
    /// Reads `<dir>/tools/tools.json`, checks the hash of every tool's guest crate
    /// (`tools/<name>/`; a changed source is an error naming the tool) and compiles it with the
    /// isolated tool build (ADR-0030; offline, so the dependencies must be available locally, as
    /// in the builder image) into `tools/<name>.wasm`. A module whose source hash matches is
    /// skipped unless `--force`. Exit code 1 if a tool fails; errors are reported per tool.
    Build {
        /// The generated crate.
        dir: PathBuf,
        /// Output format: `text`, or `json` for tools such as AgentWeaver.
        #[arg(long, value_enum, default_value = "text")]
        format: crate::build::Format,
        /// Builds modules that are up to date, too.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        build: BuildOptions,
    },
    /// Checks a package folder without building it: `grauwolf-tools.json` and the code of every
    /// tool in `src/<tool>.rs` (the rules of `tools pack`, including the forbidden constructs of
    /// tool code). Reports every problem; exit code 1 if there is one.
    Validate {
        /// The package folder.
        dir: PathBuf,
    },
    /// Scaffolds a tool package folder: `grauwolf-tools.json` with the metadata fields, one
    /// example tool in `src/<tool>.rs`, its test (`cargo test`), a README with the steps to build,
    /// publish and find it, and a GitHub Actions workflow that validates, tests, packs, and on a
    /// tag signs and pushes it (with the catalog entry) in the `grauwolf-tools` image.
    New {
        /// Name of the package (a-z, 0-9, `-`, `_`).
        name: String,
        /// Name of the example tool (snake_case).
        #[arg(long, default_value = "hello")]
        tool: String,
        /// Folder to create [default: ./<name>]; must not exist or be empty.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// The tool catalog of a registry (ADR-0124): list, search and show its packages with the
    /// verdict of your tool policy, add, remove or rebuild entries.
    Catalog {
        #[command(subcommand)]
        command: crate::catalog::CatalogCommand,
    },
    /// Resolves the `tool_packages` and the git sources of `skills` of a spec and of its local
    /// sub-agents and writes `grauwolf.lock` next to the spec.
    Lock {
        /// Path to the spec.
        file: PathBuf,
        #[command(flatten)]
        build: BuildOptions,
    },
}

/// Runs a tool command; `lock` reads the spec tree with `locker`.
pub fn run(io: &Io, command: ToolsCommand, locker: Locker<'_>) -> ExitCode {
    if let ToolsCommand::Build {
        dir,
        format,
        force,
        build,
    } = command
    {
        return match crate::build::run(io, &dir, format, force, &build.tool_builder()) {
            Ok(code) => code,
            Err(message) => report(io, Err(message)),
        };
    }
    if let ToolsCommand::Validate { dir } = command {
        return crate::validate::run(io, &dir);
    }
    if let ToolsCommand::New { name, tool, dir } = command {
        return report(io, crate::new::run(io, &name, &tool, dir.as_deref()));
    }
    if let ToolsCommand::Catalog { command } = command {
        return report(io, crate::catalog::run(io, command));
    }
    report(
        io,
        (|| {
            let runtime = crate::errors::runtime()?;
            match command {
                ToolsCommand::Pull {
                    reference,
                    cache,
                    wrapper,
                } if is_git(&reference) => {
                    let source = parse_git(&reference)?;
                    let builder = tool_builder(&cache.cache, &wrapper);
                    let (package, commit) =
                        runtime.block_on(pull_git(&source, &cache.cache, &builder))?;
                    outln!(io, "{reference} -> commit {commit}");
                    outln!(io, "tools:");
                    for tool in &package.manifest.tools {
                        outln!(io, "  {}: {}", tool.name, tool.description);
                    }
                }
                ToolsCommand::Pull {
                    reference, cache, ..
                } => {
                    let package = runtime.block_on(pull_package(&reference, &cache.cache))?;
                    outln!(
                        io,
                        "{reference}: pulled {} ({} tools)",
                        package.digest,
                        package.manifest.tools.len()
                    );
                }
                ToolsCommand::Inspect {
                    reference,
                    allowed_signers,
                    keys,
                    cache,
                } if is_git(&reference) => {
                    if !keys.is_empty() {
                        return Err(
                            "--key is for OCI packages; use --allowed-signers for git".to_owned()
                        );
                    }
                    let source = parse_git(&reference)?;
                    let signers = allowed_signers
                        .as_deref()
                        .map(grauwolf_toolpkg::setup::load_allowed_signers)
                        .transpose()?;
                    let inspection =
                        runtime.block_on(inspect_git(&source, &cache.cache, signers.as_ref()))?;
                    print_git_package(io, &reference, &inspection);
                }
                ToolsCommand::Inspect {
                    reference,
                    keys,
                    cache,
                    ..
                } => {
                    let keys = keys
                        .iter()
                        .map(|path| grauwolf_toolpkg::setup::load_public_key(path))
                        .collect::<Result<Vec<_>, _>>()?;
                    let (package, status) =
                        runtime.block_on(inspect_package(&reference, &cache.cache, &keys))?;
                    print_package(io, &reference, &package, &status);
                }
                ToolsCommand::Keygen { name } => {
                    let (private, public, fingerprint) =
                        grauwolf_toolpkg::setup::generate_key(&name)?;
                    outln!(
                        io,
                        "{}: private key (PKCS#8 PEM, unencrypted, mode 0600): keep it secret",
                        private.display()
                    );
                    outln!(
                        io,
                        "{}: public key; list it under `oci_keys` in the tool policy",
                        public.display()
                    );
                    outln!(io, "fingerprint: {fingerprint}");
                }
                ToolsCommand::Pack { dir, out, build } => {
                    let packed = grauwolf_toolpkg::setup::pack_folder(&dir, &build.tool_builder())?;
                    let out = out.unwrap_or_else(|| dir.join("dist"));
                    packed
                        .write_layout(&out)
                        .map_err(|error| error.to_string())?;
                    outln!(
                        io,
                        "{}: packed {} ({} blobs)",
                        out.display(),
                        packed.digest,
                        packed.blobs.len()
                    );
                }
                ToolsCommand::Push {
                    source,
                    reference,
                    sign_key,
                    catalog,
                    no_catalog,
                    build,
                } => {
                    let key = sign_key
                        .as_deref()
                        .map(grauwolf_toolpkg::setup::load_signing_key)
                        .transpose()?;
                    let packed =
                        grauwolf_toolpkg::setup::load_packed(&source, &build.tool_builder())?;
                    let pushed =
                        runtime.block_on(grauwolf_toolpkg::setup::push_package_signed_with(
                            &reference,
                            &packed,
                            &grauwolf_config::RegistryConfig::from_env(
                                &grauwolf_config::ProcessEnv,
                            ),
                            key.as_ref(),
                        ))?;
                    outln!(io, "{pushed}");
                    if key.is_some() {
                        outln!(io, "signed");
                    }
                    if !no_catalog {
                        let at = crate::catalog::CatalogOptions {
                            catalog,
                            keys: Vec::new(),
                        };
                        let added = runtime
                            .block_on(crate::catalog::add(&pushed, &at, key.as_ref()))
                            .map_err(|error| {
                                format!(
                                    "the package was pushed, but the tool catalog was not \
                                     updated: {error}; add it with `catalog add {pushed}` or push \
                                     with --no-catalog"
                                )
                            })?;
                        outln!(io, "{added}");
                    }
                }
                ToolsCommand::Build { .. } => unreachable!("handled above"),
                ToolsCommand::Validate { .. } => unreachable!("handled above"),
                ToolsCommand::Catalog { .. } => unreachable!("handled above"),
                ToolsCommand::New { .. } => unreachable!("handled above"),
                ToolsCommand::Lock { file, build } => {
                    let summary: LockSummary = locker(io, &file, &build)?;
                    outln!(io, "{}", summary.describe(&file)?);
                }
            }
            Ok(())
        })(),
    )
}

/// Does `reference` name a git package (`git+<url>@…`)? Everything else is an OCI reference.
fn is_git(reference: &str) -> bool {
    reference.starts_with("git+")
}

/// The reference is not echoed: it could carry credentials, which the parser rejects.
fn parse_git(reference: &str) -> Result<grauwolf_spec::GitSource, String> {
    grauwolf_spec::GitSource::parse_reference(reference)
        .map_err(|reason| format!("git reference: {reason}"))
}

fn git_config() -> grauwolf_config::RegistryConfig {
    grauwolf_config::RegistryConfig::from_env(&grauwolf_config::ProcessEnv)
}

async fn pull_git(
    source: &grauwolf_spec::GitSource,
    cache: &Path,
    builder: &grauwolf_toolbuild::ToolBuilder,
) -> Result<(grauwolf_toolpkg::Package, String), String> {
    grauwolf_toolpkg::setup::git_resolver(cache, &git_config(), builder)?
        .pull_git(source)
        .await
        .map_err(|error| error.to_string())
}

async fn inspect_git(
    source: &grauwolf_spec::GitSource,
    cache: &Path,
    signers: Option<&grauwolf_toolpkg::AllowedSigners>,
) -> Result<grauwolf_toolpkg::GitInspection, String> {
    // Inspecting never builds; the builder is only there to complete the resolver.
    grauwolf_toolpkg::setup::git_resolver(cache, &git_config(), &tool_builder(cache, &[]))?
        .inspect_git(source, signers)
        .await
        .map_err(|error| error.to_string())
}

async fn pull_package(reference: &str, cache: &Path) -> Result<grauwolf_toolpkg::Package, String> {
    let parsed = grauwolf_spec::OciReference::parse(reference)
        .map_err(|reason| format!("{reference}: {reason}"))?;
    grauwolf_toolpkg::setup::resolver_for(
        None,
        cache,
        false,
        &grauwolf_config::RegistryConfig::from_env(&grauwolf_config::ProcessEnv),
    )?
    .pull(&parsed)
    .await
    .map_err(|error| format!("{reference}: {error}"))
}

async fn inspect_package(
    reference: &str,
    cache: &Path,
    keys: &[grauwolf_toolpkg::cosign::PublicKey],
) -> Result<(grauwolf_toolpkg::Package, grauwolf_toolpkg::SignatureStatus), String> {
    let parsed = grauwolf_spec::OciReference::parse(reference)
        .map_err(|reason| format!("{reference}: {reason}"))?;
    grauwolf_toolpkg::setup::resolver_for(
        None,
        cache,
        false,
        &grauwolf_config::RegistryConfig::from_env(&grauwolf_config::ProcessEnv),
    )?
    .inspect(&parsed, keys)
    .await
    .map_err(|error| format!("{reference}: {error}"))
}

/// What `grauwolf tools inspect` prints for a git package.
fn print_git_package(io: &Io, reference: &str, inspection: &grauwolf_toolpkg::GitInspection) {
    use grauwolf_toolpkg::GitSignatureStatus;
    let manifest = &inspection.manifest;
    outln!(io, "{reference}");
    outln!(io, "commit:  {}", inspection.commit);
    match &inspection.signature {
        GitSignatureStatus::Signed {
            principal,
            fingerprint,
        } => match principal {
            Some(principal) => outln!(io, "signature: signed by {principal} ({fingerprint})"),
            None => outln!(io, "signature: signed by {fingerprint}"),
        },
        GitSignatureStatus::Unsigned => outln!(io, "signature: unsigned"),
        GitSignatureStatus::Invalid(reason) => outln!(io, "signature: {reason}"),
        GitSignatureStatus::Unchecked => outln!(
            io,
            "signature: signed, not checked (no signers: list `git_allowed_signers` in the tool \
             policy or pass --allowed-signers)"
        ),
    }
    outln!(io, "package: {} {}", manifest.name, manifest.version);
    outln!(io, "abi:     {}", manifest.abi);
    outln!(
        io,
        "cache:   {}",
        if inspection.cached.is_some() {
            "cached"
        } else {
            "not cached"
        }
    );
    outln!(io, "tools:");
    for tool in &manifest.tools {
        outln!(io, "  {}: {}", tool.name, tool.description);
        outln!(io, "    args:   {}", tool.args);
        let hash = inspection
            .cached
            .as_ref()
            .and_then(|package| package.modules.get(&tool.module))
            .map_or("", |module| module.digest.as_str());
        outln!(io, "    module: {} {hash}", tool.module);
    }
}

/// What `grauwolf tools inspect` prints.
fn print_package(
    io: &Io,
    reference: &str,
    package: &grauwolf_toolpkg::Package,
    status: &grauwolf_toolpkg::SignatureStatus,
) {
    use grauwolf_toolpkg::SignatureStatus;
    let manifest = &package.manifest;
    outln!(io, "{reference}");
    outln!(io, "digest:  {}", package.digest);
    match status {
        SignatureStatus::Signed { fingerprint } => {
            outln!(io, "signature: signed by {fingerprint}")
        }
        SignatureStatus::Unsigned => outln!(io, "signature: unsigned"),
        SignatureStatus::Invalid(reason) => outln!(io, "signature: signature invalid ({reason})"),
        SignatureStatus::Unchecked { signatures } => outln!(
            io,
            "signature: {signatures} signature(s) present, not checked (no key: list `oci_keys` \
             in the tool policy or pass --key)"
        ),
    }
    outln!(io, "package: {} {}", manifest.name, manifest.version);
    outln!(io, "abi:     {}", manifest.abi);
    print_metadata(io, package);
    outln!(
        io,
        "sources: {}",
        if package.source.is_some() {
            "included"
        } else {
            "none"
        }
    );
    outln!(io, "tools:");
    for tool in &manifest.tools {
        outln!(io, "  {}: {}", tool.name, tool.description);
        outln!(io, "    args:   {}", tool.args);
        outln!(
            io,
            "    module: {} {}",
            tool.module,
            package
                .modules
                .get(&tool.module)
                .map_or("", |module| module.digest.as_str())
        );
        if !tool.limits.is_empty() {
            outln!(io, "    limits: {}", describe_limits(&tool.limits));
        }
    }
}

/// The metadata of a package (ADR-0124): the annotations of its manifest, falling back to the
/// fields of `grauwolf-tools.json`; packages packed before both show nothing here.
fn print_metadata(io: &Io, package: &grauwolf_toolpkg::Package) {
    let manifest = &package.manifest;
    let annotation = |key: &str| package.annotations.get(key).cloned();
    let authors = (!manifest.authors.is_empty()).then(|| manifest.authors.join(", "));
    let rows = [
        (
            "description",
            annotation(grauwolf_toolpkg::DESCRIPTION_ANNOTATION).or(manifest.description.clone()),
        ),
        ("created", annotation(grauwolf_toolpkg::CREATED_ANNOTATION)),
        (
            "source",
            annotation(grauwolf_toolpkg::SOURCE_ANNOTATION).or(manifest.source.clone()),
        ),
        (
            "revision",
            annotation(grauwolf_toolpkg::REVISION_ANNOTATION),
        ),
        (
            "license",
            annotation(grauwolf_toolpkg::LICENSES_ANNOTATION).or(manifest.license.clone()),
        ),
        (
            "authors",
            annotation(grauwolf_toolpkg::AUTHORS_ANNOTATION).or(authors),
        ),
        (
            "built with",
            annotation(grauwolf_toolpkg::BUILT_WITH_ANNOTATION).map(|v| format!("grauwolf {v}")),
        ),
        ("rustc", annotation(grauwolf_toolpkg::RUSTC_ANNOTATION)),
    ];
    for (label, value) in rows {
        if let Some(value) = value {
            outln!(io, "{label}: {value}");
        }
    }
}

/// `memory 16 MiB, fuel 1000000, time 500 ms`.
pub(crate) fn describe_limits(limits: &grauwolf_tools::ToolLimits) -> String {
    [
        limits.memory_mib.map(|value| format!("memory {value} MiB")),
        limits.fuel.map(|value| format!("fuel {value}")),
        limits.time_ms.map(|value| format!("time {value} ms")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ")
}
