//! `trust-domain restore <backup>` (ADR-0120 decision 4): puts the cluster back as the backup of a
//! `trust-domain set` (or of an earlier restore) has it. It reads the backup, prints the plan
//! (line-level diff of the ConfigMaps, the OpenBao roles, the server data, the restarts), writes a
//! backup of the current state first and then changes the cluster in the order of `set`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use grauwolf_k8s::{Kind, kinds};
use serde_json::{Map, Value, json};

use crate::agent;
use crate::hcl::{LineChange, has_federation};
use crate::io::Io;
use crate::services::Services;
use crate::spire::{self, OIDC_NAME};
use crate::trust_domain::{
    self, Backup, BundleRef, ConfigChange, KeyChange, Options, Recreate, Utc, Workload,
    describe_config, get,
};
use crate::x509;

const TOOL: &str = "grauwolf-k8-ops trust-domain restore";

pub(crate) async fn run(
    services: &Services,
    io: &mut Io<'_>,
    file: &Path,
    options: Options,
) -> Result<i32, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| format!("cannot read the backup {}: {error}", file.display()))?;
    let backup = parse(&text).map_err(|error| format!("{}: {error}", file.display()))?;
    let plan = match make_plan(services, &backup).await? {
        Outcome::Done => {
            io.line("already as in the backup, nothing to do");
            return Ok(0);
        }
        Outcome::Plan(plan) => plan,
    };
    print_plan(io, &plan);
    io.line("");
    if !options.apply {
        io.line("This was only the plan: nothing was changed. Add --apply to do it.");
        return Ok(0);
    }
    if !options.yes && !io.confirm() {
        return Err("cancelled, nothing was changed".to_owned());
    }
    let new_backup = options.backup.clone().unwrap_or_else(|| {
        PathBuf::from(format!("trust-domain-backup-{}.json", Utc::now().compact()))
    });
    io.line("step 4: backup of the current state");
    write_backup(&new_backup, &plan).map_err(|error| format!("{error} (nothing was changed)"))?;
    io.line(&format!("  written to {}", new_backup.display()));
    apply(services, io, &plan, options.timeout)
        .await
        .map_err(|error| {
            format!(
                "{error}\nrestoring from: {}\nthe objects as they were before this restore: {}\nthe cluster may be partly changed: fix the cause and run the same command again with another --backup file, it continues where this one stopped",
                file.display(),
                new_backup.display()
            )
        })?;
    Ok(0)
}

// ---- the backup file -----------------------------------------------------------------------

struct StoredConfigMap {
    namespace: String,
    name: String,
    data: Map<String, Value>,
}

struct StoredWorkload {
    kind: Kind,
    namespace: String,
    name: String,
}

struct StoredRole {
    mount: String,
    name: String,
    data: Map<String, Value>,
}

struct BackupFile {
    /// The trust domain the objects held: the one to restore.
    domain: String,
    config_maps: Vec<StoredConfigMap>,
    workloads: Vec<StoredWorkload>,
    roles: Vec<StoredRole>,
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

fn array<'a>(backup: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    backup[key]
        .as_array()
        .ok_or_else(|| format!("'{key}' is missing or not a list: not a backup of this tool"))
}

/// Reads and validates a backup written by `trust-domain set` or `restore`.
fn parse(file_text: &str) -> Result<BackupFile, String> {
    let backup: Value = serde_json::from_str(file_text)
        .map_err(|error| format!("not a backup of this tool: not valid JSON ({error})"))?;
    let tool = backup["tool"].as_str().ok_or(
        "not a backup of this tool: no 'tool' field (expected one written by grauwolf-k8-ops trust-domain)",
    )?;
    if !matches!(
        tool,
        "grauwolf-k8-ops trust-domain set" | "grauwolf-k8-ops trust-domain restore"
    ) {
        return Err(format!(
            "not a backup of this tool: it was written by '{tool}'"
        ));
    }
    match backup.get("format") {
        None => {}
        Some(format) if format.as_u64() == Some(trust_domain::BACKUP_FORMAT) => {}
        Some(format) => {
            return Err(format!(
                "unknown backup format {format} (this version reads format {})",
                trust_domain::BACKUP_FORMAT
            ));
        }
    }
    let domain = backup["from"]
        .as_str()
        .ok_or("the backup has no 'from' (the trust domain to restore)")?;
    trust_domain::validate_domain(domain)
        .map_err(|error| format!("'from' of the backup: {error}"))?;

    let mut config_maps = Vec::new();
    for (index, object) in array(&backup, "configmaps")?.iter().enumerate() {
        let at = format!("configmaps[{index}]");
        let (Some(namespace), Some(name)) = (
            text(object, "/metadata/namespace"),
            text(object, "/metadata/name"),
        ) else {
            return Err(format!("{at} has no metadata.namespace and metadata.name"));
        };
        let mut data = Map::new();
        for (key, value) in object["data"].as_object().into_iter().flatten() {
            if !value.is_string() {
                return Err(format!(
                    "{at} ({namespace}/{name}): key {key} is not a text"
                ));
            }
            data.insert(key.clone(), value.clone());
        }
        config_maps.push(StoredConfigMap {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            data,
        });
    }
    let mut workloads = Vec::new();
    for (index, object) in array(&backup, "workloads")?.iter().enumerate() {
        let at = format!("workloads[{index}]");
        let kind = match text(object, "/kind") {
            Some("Deployment") => kinds::DEPLOYMENT,
            Some("StatefulSet") => kinds::STATEFUL_SET,
            Some("DaemonSet") => kinds::DAEMON_SET,
            other => {
                return Err(format!(
                    "{at} has kind {} (not Deployment, StatefulSet or DaemonSet)",
                    other.unwrap_or("(none)")
                ));
            }
        };
        let (Some(namespace), Some(name)) = (
            text(object, "/metadata/namespace"),
            text(object, "/metadata/name"),
        ) else {
            return Err(format!("{at} has no metadata.namespace and metadata.name"));
        };
        workloads.push(StoredWorkload {
            kind,
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        });
    }
    let mut roles = Vec::new();
    for (index, role) in array(&backup, "openbao_roles")?.iter().enumerate() {
        let (Some(mount), Some(name), Some(data)) = (
            text(role, "/mount"),
            text(role, "/name"),
            role["data"].as_object(),
        ) else {
            return Err(format!(
                "openbao_roles[{index}] has no mount, name and data"
            ));
        };
        roles.push(StoredRole {
            mount: mount.to_owned(),
            name: name.to_owned(),
            data: data.clone(),
        });
    }
    Ok(BackupFile {
        domain: domain.to_owned(),
        config_maps,
        workloads,
        roles,
    })
}

// ---- the line diff -------------------------------------------------------------------------

/// The line-level difference from `current` to `wanted`: a line that matches after `normalize`
/// (the trust domain put back) but differs as written is a change of its own; the other changed
/// runs of lines have their removed and added lines paired by position. `line` is the 1-based
/// line of `current` (for an addition the line it is inserted before).
fn line_diff(current: &str, wanted: &str, normalize: &dyn Fn(&str) -> String) -> Vec<LineChange> {
    enum Op {
        Match(usize, usize),
        Delete(usize),
        Insert(usize),
    }
    let before: Vec<&str> = current.split('\n').collect();
    let after: Vec<&str> = wanted.split('\n').collect();
    let same: Vec<String> = before.iter().map(|line| normalize(line)).collect();
    let equal = |i: usize, j: usize| same[i] == after[j];
    let prefix = (0..before.len().min(after.len()))
        .take_while(|k| equal(*k, *k))
        .count();
    let suffix = (0..before.len().min(after.len()) - prefix)
        .take_while(|k| equal(before.len() - 1 - k, after.len() - 1 - k))
        .count();
    let (a_end, b_end) = (before.len() - suffix, after.len() - suffix);
    let mut ops: Vec<Op> = (0..prefix).map(|k| Op::Match(k, k)).collect();
    let (rows, columns) = (a_end - prefix, b_end - prefix);
    if rows * columns > 4_000_000 {
        // Too big to align: one run of changes.
        ops.extend((prefix..a_end).map(Op::Delete));
        ops.extend((prefix..b_end).map(Op::Insert));
    } else {
        // Longest common subsequence of the middle parts.
        let mut table = vec![vec![0u32; columns + 1]; rows + 1];
        for i in (0..rows).rev() {
            for j in (0..columns).rev() {
                table[i][j] = if equal(prefix + i, prefix + j) {
                    table[i + 1][j + 1] + 1
                } else {
                    table[i + 1][j].max(table[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < rows || j < columns {
            if i < rows && j < columns && equal(prefix + i, prefix + j) {
                ops.push(Op::Match(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if j == columns || (i < rows && table[i + 1][j] >= table[i][j + 1]) {
                ops.push(Op::Delete(prefix + i));
                i += 1;
            } else {
                ops.push(Op::Insert(prefix + j));
                j += 1;
            }
        }
    }
    ops.extend((0..suffix).map(|k| Op::Match(a_end + k, b_end + k)));

    let mut changes = Vec::new();
    // The current run of unmatched lines: where it starts, what is removed and what is added.
    let (mut start, mut removed, mut added) = (0, Vec::new(), Vec::new());
    let flush = |changes: &mut Vec<LineChange>,
                 start: usize,
                 removed: &mut Vec<usize>,
                 added: &mut Vec<usize>| {
        for index in 0..removed.len().max(added.len()) {
            changes.push(LineChange {
                line: start + index + 1,
                old: removed
                    .get(index)
                    .map(|i| before[*i].trim().to_owned())
                    .unwrap_or_default(),
                new: added
                    .get(index)
                    .map(|j| after[*j].trim().to_owned())
                    .unwrap_or_default(),
            });
        }
        removed.clear();
        added.clear();
    };
    let mut cursor = 0;
    for op in &ops {
        match op {
            Op::Delete(i) => {
                if removed.is_empty() && added.is_empty() {
                    start = cursor;
                }
                removed.push(*i);
                cursor = i + 1;
            }
            Op::Insert(j) => {
                if removed.is_empty() && added.is_empty() {
                    start = cursor;
                }
                added.push(*j);
            }
            Op::Match(i, j) => {
                flush(&mut changes, start, &mut removed, &mut added);
                if before[*i] != after[*j] {
                    changes.push(LineChange {
                        line: i + 1,
                        old: before[*i].trim().to_owned(),
                        new: after[*j].trim().to_owned(),
                    });
                }
                cursor = i + 1;
            }
        }
    }
    flush(&mut changes, start, &mut removed, &mut added);
    changes
}

// ---- the plan ------------------------------------------------------------------------------

struct RoleRestore {
    mount: String,
    name: String,
    /// As OpenBao returns it now (`None`: the role does not exist).
    current: Option<Value>,
    /// The current fields with those of the backup on top.
    wanted: Value,
}

impl RoleRestore {
    /// `(field, now, wanted)` of every field that differs.
    fn differences(&self) -> Vec<(String, String, String)> {
        let show = |value: Option<&Value>| value.map_or("(not set)".to_owned(), Value::to_string);
        self.wanted
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, value)| {
                self.current
                    .as_ref()
                    .is_none_or(|now| now.get(*key) != Some(*value))
            })
            .map(|(key, value)| {
                (
                    key.clone(),
                    show(self.current.as_ref().and_then(|now| now.get(key))),
                    show(Some(value)),
                )
            })
            .collect()
    }
}

struct Plan {
    /// The trust domain restored.
    domain: String,
    /// The trust domain of the cluster now.
    current: String,
    configs: Vec<ConfigChange>,
    /// Changed lines that do not mention either trust domain: a restore undoes them, too.
    others: Vec<String>,
    /// Every ConfigMap of the backup as it is now (for the backup of this restore).
    config_objects: Vec<Value>,
    bundle: BundleRef,
    recreate: Option<Recreate>,
    restarts: Vec<Workload>,
    consumer_restarts: Vec<Workload>,
    roles: Vec<RoleRestore>,
    /// Roles of the backup that stay as they are, and why.
    skipped: Vec<String>,
    /// What is already as in the backup, and other remarks.
    settled: Vec<String>,
    /// Roles of agents that are not in the backup: their binding is not touched.
    unlisted_roles: Vec<String>,
}

enum Outcome {
    Done,
    Plan(Box<Plan>),
}

fn is_spire_own(workload: &StoredWorkload, services: &Services) -> bool {
    let settings = &services.settings;
    let own = [
        (
            &kinds::STATEFUL_SET,
            &settings.spire_namespace,
            "spire-server",
        ),
        (&kinds::DAEMON_SET, &settings.spire_namespace, "spire-agent"),
        (
            &kinds::DEPLOYMENT,
            &settings.spire_namespace,
            "spire-controller-manager",
        ),
        (&kinds::DEPLOYMENT, &settings.oidc_namespace, OIDC_NAME),
    ];
    own.iter().any(|(kind, namespace, name)| {
        **kind == workload.kind && **namespace == workload.namespace && *name == workload.name
    })
}

async fn make_plan(services: &Services, backup: &BackupFile) -> Result<Outcome, String> {
    let settings = &services.settings;
    let cluster = &services.cluster;
    let spire_ns = &settings.spire_namespace;
    let domain = backup.domain.as_str();
    let current = spire::trust_domain(cluster, settings).await?;

    // The ConfigMaps: only `data`, key by key.
    let mut configs = Vec::new();
    let mut others = Vec::new();
    let mut config_objects = Vec::new();
    let mut settled = Vec::new();
    let mut server_texts: Vec<String> = Vec::new();
    let mut wanted_server: Option<String> = None;
    let domains = [domain, current.as_str()];
    for stored in &backup.config_maps {
        let (namespace, name) = (&stored.namespace, &stored.name);
        let object = get(cluster, &kinds::CONFIG_MAP, namespace, name)
            .await?
            .ok_or_else(|| {
                format!(
                    "ConfigMap {namespace}/{name} of the backup does not exist any more: restore it by hand (the object is in the backup file) or remove it from the file; nothing was changed"
                )
            })?;
        let now = object["data"].as_object().cloned().unwrap_or_default();
        let mut keys = Vec::new();
        for (key, wanted) in &stored.data {
            let wanted = wanted.as_str().unwrap_or("");
            let before = now.get(key).and_then(Value::as_str).unwrap_or("");
            if (namespace, name.as_str()) == (spire_ns, spire::SERVER_CONFIG.0)
                && key == spire::SERVER_CONFIG.1
            {
                server_texts.push(before.to_owned());
                wanted_server = Some(wanted.to_owned());
            }
            if before == wanted && now.contains_key(key) {
                continue;
            }
            let changes = line_diff(before, wanted, &|line| line.replace(&current, domain));
            for change in &changes {
                if !domains
                    .iter()
                    .any(|d| change.old.contains(d) || change.new.contains(d))
                {
                    others.push(format!(
                        "{}, line {}: {} -> {}",
                        describe_config(namespace, name, key),
                        change.line,
                        shown(&change.old),
                        shown(&change.new)
                    ));
                }
            }
            keys.push(KeyChange {
                key: key.clone(),
                new_text: wanted.to_owned(),
                changes,
            });
        }
        if keys.is_empty() {
            settled.push(format!(
                "ConfigMap {namespace}/{name} is already as in the backup"
            ));
        } else {
            configs.push(ConfigChange {
                namespace: namespace.clone(),
                name: name.clone(),
                object: object.clone(),
                keys,
            });
        }
        config_objects.push(object);
    }

    // The server configuration that is in effect decides about federation and the data.
    let server_conf = match &wanted_server {
        Some(text) => text.clone(),
        None => {
            let object = get(cluster, &kinds::CONFIG_MAP, spire_ns, spire::SERVER_CONFIG.0)
                .await?
                .ok_or_else(|| {
                    format!(
                        "ConfigMap {spire_ns}/{} not found: SPIRE is not installed in namespace {spire_ns}",
                        spire::SERVER_CONFIG.0
                    )
                })?;
            let text = object["data"][spire::SERVER_CONFIG.1]
                .as_str()
                .unwrap_or("")
                .to_owned();
            server_texts.push(text.clone());
            text
        }
    };
    if server_texts
        .iter()
        .chain([&server_conf])
        .any(|text| has_federation(text))
    {
        return Err("the SPIRE server configuration contains federation (federation { ... } or federates_with): changing a federated trust domain is out of scope".to_owned());
    }
    let current_server = server_texts.first().cloned().unwrap_or_default();
    let bundle = trust_domain::bundle_ref(&current_server, spire_ns);

    // The server's data.
    let bundle_object = get(cluster, &kinds::CONFIG_MAP, &bundle.namespace, &bundle.name).await?;
    let has_domain = bundle_object
        .as_ref()
        .and_then(|object| object["data"][bundle.key.as_str()].as_str())
        .map(x509::spiffe_trust_domains)
        .unwrap_or_default()
        .iter()
        .any(|found| found == domain);
    let server_set = get(cluster, &kinds::STATEFUL_SET, spire_ns, "spire-server")
        .await?
        .ok_or_else(|| format!("StatefulSet {spire_ns}/spire-server not found"))?;
    let recreate = if has_domain {
        settled.push(format!(
            "the server's CA already has the trust domain {domain}: its data is not recreated"
        ));
        None
    } else {
        let mut texts: Vec<&str> = server_texts.iter().map(String::as_str).collect();
        texts.push(&server_conf);
        Some(
            trust_domain::plan_recreate(
                services,
                &texts,
                &bundle,
                bundle_object.is_some(),
                domain,
                server_set,
            )
            .await?,
        )
    };

    // The restarts: SPIRE's own, then the consumers of the backup.
    let restarts = trust_domain::spire_restarts(services).await?;
    let mut consumer_restarts = Vec::new();
    for stored in backup
        .workloads
        .iter()
        .filter(|stored| !is_spire_own(stored, services))
    {
        match get(cluster, &stored.kind, &stored.namespace, &stored.name).await? {
            Some(object) => consumer_restarts.push(Workload {
                kind: stored.kind,
                namespace: stored.namespace.clone(),
                name: stored.name.clone(),
                object,
            }),
            None => settled.push(format!(
                "{} {}/{} of the backup does not exist any more, not restarted",
                stored.kind.kind, stored.namespace, stored.name
            )),
        }
    }

    // The OpenBao roles of the agents that still exist.
    let agents = agent::agents(cluster).await?;
    let mut roles = Vec::new();
    let mut skipped = Vec::new();
    for stored in &backup.roles {
        if !agents
            .iter()
            .any(|(agent, _)| agent::role_name(agent) == stored.name)
        {
            skipped.push(format!(
                "OpenBao role auth/{}/role/{}: its agent no longer exists (no managed namespace, or it is being deleted), the role is not given back",
                stored.mount, stored.name
            ));
            continue;
        }
        let bao = services.admin()?;
        let now = bao
            .read_jwt_role(&stored.mount, &stored.name)
            .await
            .map_err(|error| format!("OpenBao role {}: {error}", stored.name))?;
        let mut wanted = now.clone().unwrap_or_else(|| json!({}));
        for (field, value) in &stored.data {
            wanted[field] = value.clone();
        }
        roles.push(RoleRestore {
            mount: stored.mount.clone(),
            name: stored.name.clone(),
            current: now,
            wanted,
        });
    }
    let mut unlisted_roles = Vec::new();
    for (agent, _) in &agents {
        let role = agent::role_name(agent);
        if backup.roles.iter().all(|stored| stored.name != role) {
            unlisted_roles.push(role);
        }
    }
    roles.retain(|role| {
        let unchanged = role.differences().is_empty();
        if unchanged {
            settled.push(format!(
                "OpenBao role auth/{}/role/{} is already as in the backup",
                role.mount, role.name
            ));
        }
        !unchanged
    });

    if configs.is_empty() && roles.is_empty() && recreate.is_none() {
        return Ok(Outcome::Done);
    }
    Ok(Outcome::Plan(Box::new(Plan {
        domain: domain.to_owned(),
        current,
        configs,
        others,
        config_objects,
        bundle,
        recreate,
        restarts,
        consumer_restarts,
        roles,
        skipped,
        settled,
        unlisted_roles,
    })))
}

fn shown(line: &str) -> String {
    if line.is_empty() {
        "(no line)".to_owned()
    } else {
        format!("`{line}`")
    }
}

fn print_plan(io: &mut Io<'_>, plan: &Plan) {
    let domain = &plan.domain;
    if plan.current == *domain {
        io.line(&format!(
            "trust domain restore: {domain} (the cluster already holds it in part)"
        ));
    } else {
        io.line(&format!(
            "trust domain restore: {} -> {domain}",
            plan.current
        ));
    }
    io.line("");
    io.line(if plan.configs.is_empty() {
        "ConfigMaps to restore (only `data`, key by key): nothing"
    } else {
        "ConfigMaps to restore (only `data`, key by key; line by line, `-` is now, `+` is restored):"
    });
    for config in &plan.configs {
        trust_domain::print_changes(io, config);
    }
    if !plan.others.is_empty() {
        io.line("");
        io.line("Warning: these lines changed since the backup and are not about the trust domain; the restore undoes them too:");
        for other in &plan.others {
            io.line(&format!("  {other}"));
        }
    }
    io.line("");
    match &plan.recreate {
        Some(recreate) => {
            io.line(&format!(
                "SPIRE server data (its database and signing keys) is recreated, because its CA does not have the domain {domain}:"
            ));
            trust_domain::print_recreate(io, recreate);
        }
        None => io.line(&format!(
            "SPIRE server data: kept (the server's CA already has the domain {domain})"
        )),
    }
    io.line(&format!(
        "  then wait until ConfigMap {}/{}, key {} carries a CA with the URI SAN spiffe://{domain}",
        plan.bundle.namespace, plan.bundle.name, plan.bundle.key
    ));
    io.line("");
    io.line("Restarted after that, one after the other (each waits until it is ready):");
    for workload in plan.restarts.iter().chain(&plan.consumer_restarts) {
        io.line(&format!("  {}", workload.label()));
    }
    io.line("");
    if plan.roles.is_empty() {
        io.line("OpenBao roles: nothing to restore");
    } else {
        io.line("OpenBao roles (the fields of the backup; the others stay):");
        for role in &plan.roles {
            io.line(&format!("  auth/{}/role/{}:", role.mount, role.name));
            for (field, now, wanted) in role.differences() {
                io.line(&format!("    {field}: {now} -> {wanted}"));
            }
        }
    }
    for skipped in &plan.skipped {
        io.line(&format!("  skipped: {skipped}"));
    }
    if !plan.unlisted_roles.is_empty() {
        io.line(&format!(
            "  not in the backup, left as they are: {}",
            plan.unlisted_roles.join(", ")
        ));
    }
    if !plan.settled.is_empty() {
        io.line("");
        io.line("Left alone:");
        for line in &plan.settled {
            io.line(&format!("  {line}"));
        }
    }
    io.line("");
    io.line("What this means:");
    io.line(&format!(
        "  - Every SPIFFE ID changes back (to spiffe://{domain}/...); every workload gets a new SVID."
    ));
    io.line("  - The SPIRE server starts with an empty database and a new CA, unless its CA has the domain already.");
    io.line("  - Not restored: labels, annotations and everything else outside the `data` of the ConfigMaps, workloads other than by a restart, Secrets, agents added or removed since the backup.");
    io.line("  - A backup of the current state is written first; restoring that file undoes this restore.");
}

// ---- the change ----------------------------------------------------------------------------

fn write_backup(file: &Path, plan: &Plan) -> Result<(), String> {
    let mut workloads: Vec<&Value> = Vec::new();
    if let Some(recreate) = &plan.recreate {
        workloads.push(&recreate.server.object);
    }
    for workload in plan.restarts.iter().chain(&plan.consumer_restarts) {
        workloads.push(&workload.object);
    }
    let roles: Vec<Value> = plan
        .roles
        .iter()
        .filter_map(|role| {
            Some(json!({"mount": role.mount, "name": role.name, "data": role.current.as_ref()?}))
        })
        .collect();
    let config_maps: Vec<&Value> = plan.config_objects.iter().collect();
    trust_domain::write_backup_file(
        file,
        &Backup {
            tool: TOOL,
            from: &plan.current,
            to: &plan.domain,
            config_maps: &config_maps,
            workloads: &workloads,
            roles,
        },
    )
}

async fn apply(
    services: &Services,
    io: &mut Io<'_>,
    plan: &Plan,
    timeout: Duration,
) -> Result<(), String> {
    let cluster = &services.cluster;
    let domain = plan.domain.as_str();
    let step =
        |number: u32, what: &str, error: String| format!("step {number} ({what}) failed: {error}");

    io.line("step 5: restore the ConfigMaps");
    trust_domain::rewrite_configs(cluster, io, &plan.configs)
        .await
        .map_err(|error| step(5, "restore the ConfigMaps", error))?;
    for other in &plan.others {
        io.line(&format!(
            "  warning: undone, not about the trust domain: {other}"
        ));
    }

    io.line("step 6: recreate the SPIRE server's data");
    match &plan.recreate {
        Some(recreate) => trust_domain::recreate_server(cluster, io, recreate, timeout)
            .await
            .map_err(|error| step(6, "recreate the SPIRE server's data", error))?,
        None => io.line("  skipped: the server's CA already has the domain"),
    }

    io.line("step 7: wait for the bundle, then restart");
    trust_domain::wait_until(
        timeout,
        &format!(
            "ConfigMap {}/{} to carry a CA of spiffe://{domain}",
            plan.bundle.namespace, plan.bundle.name
        ),
        || trust_domain::bundle_has(cluster, &plan.bundle, domain),
    )
    .await
    .map_err(|error| step(7, "wait for the bundle", error))?;
    io.line(&format!(
        "  ConfigMap {}/{} carries a CA of spiffe://{domain}",
        plan.bundle.namespace, plan.bundle.name
    ));
    for workload in plan.restarts.iter().chain(&plan.consumer_restarts) {
        trust_domain::restart(cluster, io, workload, timeout)
            .await
            .map_err(|error| step(7, "restart", error))?;
    }

    io.line("step 8: OpenBao roles");
    if plan.roles.is_empty() {
        io.line("  nothing to restore");
    }
    for role in &plan.roles {
        services
            .admin()
            .map_err(|error| step(8, "OpenBao roles", error))?
            .ensure_jwt_role(&role.mount, &role.name, &role.wanted)
            .await
            .map_err(|error| step(8, "OpenBao roles", format!("role {}: {error}", role.name)))?;
        io.line(&format!(
            "  auth/{}/role/{}: restored (bound_subject {})",
            role.mount,
            role.name,
            role.wanted["bound_subject"].as_str().unwrap_or("(none)")
        ));
    }

    io.line("step 9: verify");
    let tolerated: BTreeSet<String> = plan.unlisted_roles.iter().cloned().collect();
    let tolerated: Vec<String> = tolerated.into_iter().collect();
    let (places, warnings) = trust_domain::verify(services, domain, &plan.bundle, &tolerated)
        .await
        .map_err(|error| step(9, "verify", error))?;
    for warning in warnings {
        io.line(&format!(
            "  warning: {warning} (not in the backup, left as it is)"
        ));
    }
    io.line(&format!(
        "done: the trust domain is {domain} in all {places} place(s) read"
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_of_equal_texts_is_empty() {
        assert!(line_diff("a\nb\n", "a\nb\n", &|l| l.to_owned()).is_empty());
    }

    #[test]
    fn diff_pairs_changed_lines_and_finds_insertions() {
        let changes = line_diff("a\nx = 1\nb\nc\n", "a\nx = 2\nb\nnew\nc\n", &|l| {
            l.to_owned()
        });
        let shown: Vec<(usize, &str, &str)> = changes
            .iter()
            .map(|c| (c.line, c.old.as_str(), c.new.as_str()))
            .collect();
        assert_eq!(shown, vec![(2, "x = 1", "x = 2"), (4, "", "new")]);
    }

    #[test]
    fn diff_aligns_domain_lines_so_that_other_lines_stand_alone() {
        let changes = line_diff("a\nextra\nd = \"B\"\n", "a\nd = \"A\"\n", &|l| {
            l.replace("B", "A")
        });
        let shown: Vec<(usize, &str, &str)> = changes
            .iter()
            .map(|c| (c.line, c.old.as_str(), c.new.as_str()))
            .collect();
        assert_eq!(shown, vec![(2, "extra", ""), (3, "d = \"B\"", "d = \"A\"")]);
    }

    #[test]
    fn diff_finds_deletions() {
        let changes = line_diff("a\nb\nc", "a\nc", &|l| l.to_owned());
        assert_eq!(changes.len(), 1);
        assert_eq!(
            (
                changes[0].line,
                changes[0].old.as_str(),
                changes[0].new.as_str()
            ),
            (2, "b", "")
        );
    }
}
