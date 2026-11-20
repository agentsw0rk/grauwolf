//! `catalog list|search|show|add|remove|sync` (ADR-0124): the tool catalog of a registry, read
//! with the reader's tool policy applied per entry, written with the registry's own rights.

use std::path::{Path, PathBuf};

use grauwolf_config::RegistryConfig;
use grauwolf_spec::OciReference;
use grauwolf_toolpkg::catalog::{
    CatalogStore, IndexSignature, ReadIndex, catalog_entry, catalog_name, catalog_reference,
    entry_verdict, ignores_conditions_note, other_than_tool_package,
};
use grauwolf_toolpkg::cosign::{PublicKey, SigningKey};
use grauwolf_toolpkg::{Policy, RegistryClient, rfc3339};
use grauwolf_tools::catalog::{CATALOG_REPOSITORY, CatalogEntry, CatalogIndex, entry_key};
use serde_json::{Value, json};

use crate::outln;
use crate::output::Io;

/// Where the catalog is and which keys check signatures.
#[derive(clap::Args)]
pub struct CatalogOptions {
    /// The catalog: a registry (`registry.example.com`, meaning
    /// `registry.example.com/grauwolf-catalog:latest`) or a full reference
    /// `registry/repository[:tag]`. Default: `GRAUWOLF_TOOLS_CATALOG`.
    #[arg(long)]
    pub catalog: Option<String>,
    /// Public key (PEM) to check signatures with, besides the `oci_keys` of the tool policy;
    /// repeatable.
    #[arg(long = "key")]
    pub keys: Vec<PathBuf>,
}

/// Output of the reading commands.
#[derive(clap::Args)]
pub struct JsonOption {
    /// Machine-readable output (JSON) instead of text.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Subcommand)]
pub enum CatalogCommand {
    /// Lists the packages of the catalog with the verdict of your tool policy per entry and the
    /// signature status of the index.
    List {
        #[command(flatten)]
        at: CatalogOptions,
        #[command(flatten)]
        output: JsonOption,
    },
    /// Lists the entries whose name, description, or tool names or descriptions contain `text`
    /// (ignoring case).
    Search {
        /// The text to look for.
        text: String,
        #[command(flatten)]
        at: CatalogOptions,
        #[command(flatten)]
        output: JsonOption,
    },
    /// Shows one package: all metadata, the tools with their arguments, limits and modules, the
    /// signatures and the verdict of your tool policy.
    Show {
        /// Name of the package.
        name: String,
        /// The version [default: the newest].
        #[arg(long)]
        version: Option<String>,
        #[command(flatten)]
        at: CatalogOptions,
        #[command(flatten)]
        output: JsonOption,
    },
    /// Adds the package at `reference` to the catalog, or updates its entry: reads its manifest,
    /// metadata and signatures from the registry. The catalog defaults to the one of the
    /// package's registry.
    Add {
        /// Reference of the package, `registry/repository[:tag][@sha256:…]`.
        reference: String,
        #[command(flatten)]
        at: CatalogOptions,
        /// Sign the written index with this private key (from `keygen`).
        #[arg(long)]
        sign_key: Option<PathBuf>,
    },
    /// Removes the entry `<name>@<version>` from the catalog (the package stays in the
    /// registry).
    Remove {
        /// The entry, `<name>@<version>`.
        entry: String,
        #[command(flatten)]
        at: CatalogOptions,
        /// Sign the written index with this private key (from `keygen`).
        #[arg(long)]
        sign_key: Option<PathBuf>,
    },
    /// Rebuilds the index from the registry: every tag of every repository (from
    /// `/v2/_catalog`, or the repositories given with `--repository`) that holds a tool package
    /// becomes an entry; other artifacts (ordinary images, image indexes) are skipped and
    /// counted in the summary line.
    Sync {
        /// Repository to read (relative to the catalog's registry, e.g. `team/math`);
        /// repeatable. Needed where the registry does not list its repositories (ghcr.io).
        #[arg(long = "repository")]
        repositories: Vec<String>,
        #[command(flatten)]
        at: CatalogOptions,
        /// Sign the written index with this private key (from `keygen`).
        #[arg(long)]
        sign_key: Option<PathBuf>,
        /// Name every skipped tag that is not a tool package (with its media type), not only
        /// those that failed.
        #[arg(long)]
        verbose: bool,
    },
}

/// The catalog named by `--catalog`, else `GRAUWOLF_TOOLS_CATALOG`, else the default catalog of
/// `registry` (the registry of the package a command writes about).
pub fn resolve_catalog(
    option: Option<&str>,
    config: &RegistryConfig,
    registry: Option<&str>,
) -> Result<OciReference, String> {
    match option.or(config.tools_catalog.as_deref()).or(registry) {
        Some(value) => catalog_reference(value),
        None => Err(
            "which catalog? Name it with --catalog <registry or reference> or \
             GRAUWOLF_TOOLS_CATALOG"
                .to_owned(),
        ),
    }
}

fn config() -> RegistryConfig {
    RegistryConfig::from_env(&grauwolf_config::ProcessEnv)
}

fn client(config: &RegistryConfig) -> Result<RegistryClient, String> {
    Ok(RegistryClient::new(grauwolf_toolpkg::setup::registry_auth(
        config,
    )?))
}

fn load_key(path: Option<&Path>) -> Result<Option<SigningKey>, String> {
    path.map(grauwolf_toolpkg::setup::load_signing_key)
        .transpose()
}

/// The keys that check signatures: those of the policy plus `--key`.
fn keys(policy: Option<&Policy>, extra: &[PathBuf]) -> Result<Vec<PublicKey>, String> {
    let mut keys: Vec<PublicKey> = policy
        .map(|policy| policy.oci_keys().to_vec())
        .unwrap_or_default();
    for path in extra {
        keys.push(grauwolf_toolpkg::setup::load_public_key(path)?);
    }
    Ok(keys)
}

/// Environment variable with the wait in milliseconds before a catalog write on a registry that
/// ignores conditional writes is checked (default: random between 300 and 1000).
pub const SETTLE_ENV: &str = "GRAUWOLF_TOOLS_CATALOG_SETTLE_MS";

fn store<'a>(client: &'a RegistryClient, reference: OciReference) -> CatalogStore<'a> {
    let store = CatalogStore::new(client, reference);
    match std::env::var(SETTLE_ENV)
        .ok()
        .and_then(|ms| ms.trim().parse::<u64>().ok())
    {
        Some(ms) => store.with_settle(
            std::time::Duration::from_millis(ms),
            std::time::Duration::from_millis(ms),
        ),
        None => store,
    }
}

/// The time stamp of a written index.
fn now() -> String {
    rfc3339(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    )
}

/// What a reader sees: the index, its signature, the policy.
struct Reading {
    reference: OciReference,
    read: ReadIndex,
    signature: Option<IndexSignature>,
    policy: Option<Policy>,
}

impl Reading {
    fn verdict(&self, entry: &CatalogEntry) -> Result<(), String> {
        entry_verdict(self.policy.as_ref(), entry)
    }
}

async fn read(at: &CatalogOptions) -> Result<Reading, String> {
    let config = config();
    let reference = resolve_catalog(at.catalog.as_deref(), &config, None)?;
    let policy = grauwolf_toolpkg::setup::load_policy(None, &config)?;
    let keys = keys(policy.as_ref(), &at.keys)?;
    let client = client(&config)?;
    let store = store(&client, reference.clone());
    let name = catalog_name(&reference);
    let read = store
        .read()
        .await
        .map_err(|error| format!("catalog {name}: {error}"))?;
    let signature = match &read.digest {
        Some(digest) => Some(
            store
                .signature(digest, &keys)
                .await
                .map_err(|error| format!("catalog {name}: {error}"))?,
        ),
        None => None,
    };
    let require = policy
        .as_ref()
        .is_some_and(|policy| policy.rules().require_signatures);
    if require && !matches!(signature, None | Some(IndexSignature::Valid { .. })) {
        return Err(format!(
            "catalog {name}: the tool policy requires signatures (`require_signatures`) and the \
             index is {}",
            describe_signature(signature.as_ref())
        ));
    }
    Ok(Reading {
        reference,
        read,
        signature,
        policy,
    })
}

fn describe_signature(signature: Option<&IndexSignature>) -> String {
    match signature {
        None => "not there yet".to_owned(),
        Some(IndexSignature::Valid { fingerprint }) => format!("signed by {fingerprint}: valid"),
        Some(IndexSignature::Invalid(reason)) => format!("signed: invalid ({reason})"),
        Some(IndexSignature::Unsigned) => "unsigned".to_owned(),
        Some(IndexSignature::Unchecked { signatures }) => format!(
            "signed ({signatures} signature(s)), not checked: no key – list `oci_keys` in the \
             tool policy or pass --key"
        ),
    }
}

fn signature_json(signature: Option<&IndexSignature>) -> Value {
    match signature {
        None => json!({ "status": "none" }),
        Some(IndexSignature::Valid { fingerprint }) => {
            json!({ "status": "valid", "fingerprint": fingerprint })
        }
        Some(IndexSignature::Invalid(reason)) => json!({ "status": "invalid", "reason": reason }),
        Some(IndexSignature::Unsigned) => json!({ "status": "unsigned" }),
        Some(IndexSignature::Unchecked { signatures }) => {
            json!({ "status": "unchecked", "signatures": signatures })
        }
    }
}

fn verdict_text(verdict: &Result<(), String>) -> String {
    match verdict {
        Ok(()) => "allowed".to_owned(),
        Err(reason) => format!("refused: {reason}"),
    }
}

fn verdict_json(verdict: &Result<(), String>) -> Value {
    match verdict {
        Ok(()) => json!({ "allowed": true }),
        Err(reason) => json!({ "allowed": false, "reason": reason }),
    }
}

fn entry_json(reading: &Reading, entry: &CatalogEntry) -> Value {
    let mut value = serde_json::to_value(entry).expect("an entry serializes");
    value["policy"] = verdict_json(&reading.verdict(entry));
    value
}

fn listing_json(reading: &Reading, entries: &[&CatalogEntry]) -> String {
    serde_json::to_string_pretty(&json!({
        "catalog": catalog_name(&reading.reference),
        "digest": reading.read.digest,
        "updated": reading.read.index.updated,
        "signature": signature_json(reading.signature.as_ref()),
        "entries": entries.iter().map(|entry| entry_json(reading, entry)).collect::<Vec<_>>(),
    }))
    .expect("JSON")
}

fn print_header(io: &Io, reading: &Reading) {
    let index = &reading.read.index;
    match &reading.read.digest {
        Some(_) => outln!(
            io,
            "catalog {} (updated {}, {} entries)",
            catalog_name(&reading.reference),
            index.updated,
            index.entries.len()
        ),
        None => outln!(
            io,
            "catalog {}: empty (no index yet; `catalog add`, `push` or `catalog sync` write one)",
            catalog_name(&reading.reference)
        ),
    }
    if reading.read.digest.is_some() {
        outln!(
            io,
            "index: {}",
            describe_signature(reading.signature.as_ref())
        );
    }
}

fn print_table(io: &Io, reading: &Reading, entries: &[&CatalogEntry]) {
    if entries.is_empty() {
        return;
    }
    let rows: Vec<[String; 5]> = entries
        .iter()
        .map(|entry| {
            [
                entry.name.clone(),
                entry.version.clone(),
                entry.tools.len().to_string(),
                entry.description.clone().unwrap_or_default(),
                verdict_text(&reading.verdict(entry)),
            ]
        })
        .collect();
    let header = ["NAME", "VERSION", "TOOLS", "DESCRIPTION", "POLICY"];
    let width = |column: usize| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .chain(std::iter::once(header[column].len()))
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..4).map(width).collect();
    let line = |cells: [&str; 5]| {
        let mut text = String::new();
        for (column, cell) in cells.iter().enumerate().take(4) {
            text.push_str(&format!("{cell:<width$}  ", width = widths[column]));
        }
        text.push_str(cells[4]);
        text
    };
    outln!(io, "{}", line(header));
    for row in &rows {
        outln!(
            io,
            "{}",
            line([&row[0], &row[1], &row[2], &row[3], &row[4]])
        );
    }
}

fn print_entry(io: &Io, reading: &Reading, entry: &CatalogEntry, versions: &[&str]) {
    outln!(io, "{} {}", entry.name, entry.version);
    outln!(io, "reference: {}", entry.reference);
    let optional = [
        ("description", entry.description.clone()),
        ("created", entry.created.clone()),
        ("source", entry.source.clone()),
        ("revision", entry.revision.clone()),
        ("license", entry.license.clone()),
        (
            "authors",
            (!entry.authors.is_empty()).then(|| entry.authors.join(", ")),
        ),
        ("abi", Some(entry.abi.to_string())),
        (
            "built with",
            entry
                .built_with
                .as_ref()
                .map(|version| format!("grauwolf {version}")),
        ),
        ("rustc", entry.rustc.clone()),
        ("versions", Some(versions.join(", "))),
    ];
    for (label, value) in optional {
        if let Some(value) = value {
            outln!(io, "{label}: {value}");
        }
    }
    let signature = &entry.signature;
    if !signature.signed {
        outln!(io, "signature: unsigned");
    } else if signature.fingerprints.is_empty() {
        outln!(
            io,
            "signature: {} signature(s), by no key the writer knew",
            signature.signatures.len()
        );
    } else {
        outln!(
            io,
            "signature: signed by {}",
            signature.fingerprints.join(", ")
        );
    }
    outln!(io, "policy: {}", verdict_text(&reading.verdict(entry)));
    outln!(io, "tools:");
    for tool in &entry.tools {
        outln!(io, "  {}: {}", tool.name, tool.description);
        outln!(io, "    args:   {}", tool.args);
        outln!(
            io,
            "    module: {} {} ({} bytes)",
            tool.module,
            tool.digest,
            tool.size
        );
        if !tool.limits.is_empty() {
            outln!(
                io,
                "    limits: {}",
                crate::tools::describe_limits(&tool.limits)
            );
        }
        if tool.capabilities.is_empty() {
            outln!(io, "    capabilities: none (no network, no files)");
        } else {
            outln!(
                io,
                "    capabilities: {}",
                Value::Array(tool.capabilities.clone())
            );
        }
    }
}

/// Runs a catalog command.
pub fn run(io: &Io, command: CatalogCommand) -> Result<(), String> {
    let runtime = crate::errors::runtime()?;
    match command {
        CatalogCommand::List { at, output } => {
            let reading = runtime.block_on(read(&at))?;
            let entries = reading.read.index.list();
            if output.json {
                outln!(io, "{}", listing_json(&reading, &entries));
            } else {
                print_header(io, &reading);
                print_table(io, &reading, &entries);
            }
        }
        CatalogCommand::Search { text, at, output } => {
            let reading = runtime.block_on(read(&at))?;
            let entries = reading.read.index.search(&text);
            if output.json {
                outln!(io, "{}", listing_json(&reading, &entries));
            } else if entries.is_empty() {
                outln!(
                    io,
                    "nothing in {} matches `{text}`",
                    catalog_name(&reading.reference)
                );
            } else {
                print_table(io, &reading, &entries);
            }
        }
        CatalogCommand::Show {
            name,
            version,
            at,
            output,
        } => {
            let reading = runtime.block_on(read(&at))?;
            let index = &reading.read.index;
            let entry = index.find(&name, version.as_deref()).ok_or_else(|| {
                let versions = index.versions(&name);
                match (&version, versions.is_empty()) {
                    (_, true) => format!(
                        "`{name}` is not in the catalog {}",
                        catalog_name(&reading.reference)
                    ),
                    (Some(version), false) => format!(
                        "`{name}` has no version {version} in the catalog (versions: {})",
                        versions.join(", ")
                    ),
                    (None, false) => unreachable!("a package with versions has a newest one"),
                }
            })?;
            if output.json {
                outln!(
                    io,
                    "{}",
                    serde_json::to_string_pretty(&entry_json(&reading, entry)).expect("JSON")
                );
            } else {
                print_entry(io, &reading, entry, &index.versions(&name));
            }
        }
        CatalogCommand::Add {
            reference,
            at,
            sign_key,
        } => {
            let key = load_key(sign_key.as_deref())?;
            let added = runtime.block_on(add(&reference, &at, key.as_ref()))?;
            outln!(io, "{added}");
        }
        CatalogCommand::Remove {
            entry,
            at,
            sign_key,
        } => {
            let key = load_key(sign_key.as_deref())?;
            let (name, version) = entry
                .rsplit_once('@')
                .filter(|(name, version)| !name.is_empty() && !version.is_empty())
                .ok_or_else(|| format!("`{entry}`: name the entry as <name>@<version>"))?;
            let config = config();
            let reference = resolve_catalog(at.catalog.as_deref(), &config, None)?;
            let client = client(&config)?;
            let store = store(&client, reference.clone());
            let mut found = false;
            let gone = entry_key(name, version);
            let updated = runtime
                .block_on(store.update(
                    &mut |index: &mut CatalogIndex| {
                        found |= index.remove(name, version).is_some();
                        Ok(found)
                    },
                    &|index: &CatalogIndex| !index.entries.contains_key(&gone),
                    &now,
                    key.as_ref(),
                ))
                .map_err(|error| format!("catalog {}: {error}", catalog_name(&reference)))?;
            if !found {
                return Err(format!(
                    "`{name}@{version}` is not in the catalog {}",
                    catalog_name(&reference)
                ));
            }
            outln!(
                io,
                "catalog {}: removed {name}@{version}",
                catalog_name(&reference)
            );
            if updated.ignores_conditions {
                outln!(io, "{}", ignores_conditions_note(&reference.registry));
            }
        }
        CatalogCommand::Sync {
            repositories,
            at,
            sign_key,
            verbose,
        } => {
            let key = load_key(sign_key.as_deref())?;
            runtime.block_on(sync(io, &repositories, &at, key.as_ref(), verbose))?;
        }
    }
    Ok(())
}

/// Adds the package at `reference` to the catalog (`at`, else the default catalog of the
/// package's registry); returns the line to print. Also used by `push`.
pub async fn add(
    reference: &str,
    at: &CatalogOptions,
    key: Option<&SigningKey>,
) -> Result<String, String> {
    let parsed =
        OciReference::parse(reference).map_err(|reason| format!("{reference}: {reason}"))?;
    let config = config();
    let catalog = resolve_catalog(at.catalog.as_deref(), &config, Some(&parsed.registry))?;
    let policy = grauwolf_toolpkg::setup::load_policy(None, &config)?;
    let mut keys = keys(policy.as_ref(), &at.keys)?;
    // The writer's own key names itself in the entry.
    keys.extend(key.map(SigningKey::public_key));
    let client = client(&config)?;
    let entry = catalog_entry(&client, &parsed, &keys)
        .await
        .map_err(|error| format!("{reference}: {error}"))?;
    let key_name = entry.key();
    let store = store(&client, catalog.clone());
    let updated = store
        .update(
            &mut |index: &mut CatalogIndex| Ok(index.upsert(entry.clone())),
            &|index: &CatalogIndex| index.entries.get(&key_name) == Some(&entry),
            &now,
            key,
        )
        .await
        .map_err(|error| format!("catalog {}: {error}", catalog_name(&catalog)))?;
    let mut line = if updated.written {
        format!("catalog {}: added {key_name}", catalog_name(&catalog))
    } else {
        format!(
            "catalog {}: {key_name} is up to date",
            catalog_name(&catalog)
        )
    };
    if updated.ignores_conditions {
        line.push('\n');
        line.push_str(&ignores_conditions_note(&catalog.registry));
    }
    Ok(line)
}

async fn sync(
    io: &Io,
    repositories: &[String],
    at: &CatalogOptions,
    key: Option<&SigningKey>,
    verbose: bool,
) -> Result<(), String> {
    let config = config();
    let catalog = resolve_catalog(at.catalog.as_deref(), &config, None)?;
    let name = catalog_name(&catalog);
    let policy = grauwolf_toolpkg::setup::load_policy(None, &config)?;
    let keys = keys(policy.as_ref(), &at.keys)?;
    let client = client(&config)?;
    let repositories = if repositories.is_empty() {
        client
            .repositories(&catalog.registry)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "the registry {} does not list its repositories (`/v2/_catalog`); name them \
                     with --repository",
                    catalog.registry
                )
            })?
    } else {
        repositories.to_vec()
    };
    let mut index = CatalogIndex::new(String::new());
    let (mut other, mut failed) = (0, 0);
    for repository in &repositories {
        if *repository == catalog.repository || *repository == CATALOG_REPOSITORY {
            continue;
        }
        let at_repository = OciReference {
            registry: catalog.registry.clone(),
            repository: repository.clone(),
            tag: None,
            digest: None,
        };
        let tags = client
            .tags(&at_repository)
            .await
            .map_err(|error| format!("{repository}: {error}"))?;
        for tag in tags {
            // Cosign signatures and attachments live next to the packages.
            if tag.starts_with("sha256-") {
                continue;
            }
            let reference = OciReference {
                tag: Some(tag.clone()),
                ..at_repository.clone()
            };
            let label = format!("{}/{repository}:{tag}", catalog.registry);
            match other_than_tool_package(&client, &reference).await {
                Ok(None) => {}
                Ok(Some(media_type)) => {
                    if verbose {
                        outln!(io, "skipped {label}: not a tool package ({media_type})");
                    }
                    other += 1;
                    continue;
                }
                Err(error) => {
                    outln!(io, "skipped {label}: {error}");
                    failed += 1;
                    continue;
                }
            }
            match catalog_entry(&client, &reference, &keys).await {
                Ok(entry) => {
                    index.upsert(entry);
                }
                Err(error) => {
                    outln!(io, "skipped {label}: {error}");
                    failed += 1;
                }
            }
        }
    }
    let entries = index.entries.clone();
    let store = store(&client, catalog.clone());
    let updated = store
        .update(
            &mut |current: &mut CatalogIndex| {
                let changed = current.entries != entries;
                current.entries = entries.clone();
                Ok(changed)
            },
            &|current: &CatalogIndex| {
                entries
                    .iter()
                    .all(|(key, entry)| current.entries.get(key) == Some(entry))
            },
            &now,
            key,
        )
        .await
        .map_err(|error| format!("catalog {name}: {error}"))?;
    outln!(
        io,
        "catalog {name}: {} entries from {} repositories ({} skipped: {other} not tool packages \
         (--verbose names them), {failed} failed)",
        entries.len(),
        repositories.len(),
        other + failed
    );
    if updated.ignores_conditions {
        outln!(io, "{}", ignores_conditions_note(&catalog.registry));
    }
    Ok(())
}
