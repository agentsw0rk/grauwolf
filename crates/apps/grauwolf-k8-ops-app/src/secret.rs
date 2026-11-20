//! `secret set | get | list | delete`: the values of an agent at `<kv>/agents/<agent>/<key>`, field
//! `value` (ADR-0120 decision 5). A value is read from stdin or a hidden prompt and never
//! printed; errors name agent and key only.

use grauwolf_k8s::kinds;
use serde_json::{Value, json};

use crate::agent::{self, CONFIG_MAP};
use crate::cli::SecretCommand;
use crate::io::Io;
use crate::report;
use crate::services::Services;

const FIELD: &str = "value";

pub async fn run(
    services: &Services,
    io: &mut Io<'_>,
    command: SecretCommand,
) -> Result<(), String> {
    match command {
        SecretCommand::Set {
            agent,
            key,
            variable,
        } => set(services, io, &agent, &key, variable).await,
        SecretCommand::List { agent, json } => list(services, io, &agent, json).await,
        SecretCommand::Get { agent, key, reveal } => get(services, io, &agent, &key, reveal).await,
        SecretCommand::Delete { agent, key } => delete(services, io, &agent, &key).await,
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    let valid = !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{key}' is not a valid key: use letters, digits, '-', '_' and '.' (at most 128)"
        ))
    }
}

fn path_of(agent: &str, key: &str) -> String {
    format!("{}/{key}", agent::kv_prefix(agent))
}

async fn set(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    key: &str,
    variable: Option<String>,
) -> Result<(), String> {
    agent::validate_name(agent)?;
    validate_key(key)?;
    let settings = &services.settings;
    let bao = services.admin()?;
    // Everything that can be checked is checked before the secret is written.
    let namespace = match &variable {
        Some(name) => {
            if !grauwolf_config::VARIABLES
                .iter()
                .any(|known| known.name == name)
            {
                return Err(format!(
                    "{name} is not a configuration variable of grauwolf"
                ));
            }
            Some(agent::find_namespace(&services.cluster, agent).await?.ok_or_else(|| {
                format!("agent {agent} not found: --variable needs its ConfigMap (`grauwolf-k8-ops agent add {agent}`)")
            })?)
        }
        None => None,
    };
    let value = io.read_secret(&format!("Value for {agent}/{key} (hidden): "))?;
    let path = path_of(agent, key);
    let version = bao
        .kv_put_field(&settings.kv_mount, &path, FIELD, &value)
        .await
        .map_err(|error| format!("secret {agent}/{key}: {error}"))?;
    io.line(&format!(
        "secret {}/{path}#{FIELD}: stored (version {version}, {} bytes)",
        settings.kv_mount,
        value.len()
    ));
    if let (Some(name), Some(namespace)) = (variable, namespace) {
        let cluster = &services.cluster;
        let mut variables = agent::current_config(cluster, &namespace)
            .await?
            .map(|text| agent::variables_of(&text))
            .unwrap_or_default();
        let source = format!("openbao:{}/{path}#{FIELD}", settings.kv_mount);
        variables.insert(name.clone(), source.clone());
        let config = agent::config_map_object(
            &namespace,
            &agent::render_config(settings, agent, &variables),
        );
        let outcome = cluster
            .ensure(&kinds::CONFIG_MAP, Some(&namespace), &config)
            .await
            .map_err(|error| format!("ConfigMap {namespace}/{CONFIG_MAP}: {error}"))?;
        io.line(&format!(
            "variable {name} -> {source} in {namespace}/{CONFIG_MAP}: {}",
            report::k8s(outcome)
        ));
    }
    Ok(())
}

async fn list(services: &Services, io: &mut Io<'_>, agent: &str, json: bool) -> Result<(), String> {
    agent::validate_name(agent)?;
    let settings = &services.settings;
    let bao = services.admin()?;
    let mut rows = Vec::new();
    for name in bao
        .kv_list(&settings.kv_mount, &agent::kv_prefix(agent))
        .await
        .map_err(|error| format!("secrets of {agent}: {error}"))?
    {
        if name.ends_with('/') {
            continue;
        }
        let version = bao
            .kv_current_version(&settings.kv_mount, &path_of(agent, &name))
            .await
            .map_err(|error| format!("secret {agent}/{name}: {error}"))?;
        rows.push(json!({"key": name, "version": version}));
    }
    if json {
        io.line(&Value::Array(rows).to_string());
    } else if rows.is_empty() {
        io.line(&format!("no secrets for agent {agent}"));
    } else {
        for row in rows {
            io.line(&format!(
                "{}  version {}",
                row["key"].as_str().unwrap_or(""),
                row["version"].as_u64().unwrap_or(0)
            ));
        }
    }
    Ok(())
}

/// The value of a secret on stdout and nothing else; the one place where a value is printed
/// (ADR-0121 decision 4), so it needs `--reveal`. Errors name agent and key only.
async fn get(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    key: &str,
    reveal: bool,
) -> Result<(), String> {
    agent::validate_name(agent)?;
    validate_key(key)?;
    if !reveal {
        return Err(format!(
            "this prints the value of secret {agent}/{key}: add --reveal if you want that"
        ));
    }
    let settings = &services.settings;
    let bao = services.admin()?;
    let path = path_of(agent, key);
    if bao
        .kv_current_version(&settings.kv_mount, &path)
        .await
        .map_err(|error| format!("secret {agent}/{key}: {error}"))?
        .is_none()
    {
        return Err(format!("secret {agent}/{key} not found"));
    }
    let value = bao
        .kv_read_field(&settings.kv_mount, &path, FIELD)
        .await
        .map_err(|error| format!("secret {agent}/{key}: {error}"))?;
    io.value(&value);
    Ok(())
}

async fn delete(
    services: &Services,
    io: &mut Io<'_>,
    agent: &str,
    key: &str,
) -> Result<(), String> {
    agent::validate_name(agent)?;
    validate_key(key)?;
    let settings = &services.settings;
    let bao = services.admin()?;
    let path = path_of(agent, key);
    if bao
        .kv_current_version(&settings.kv_mount, &path)
        .await
        .map_err(|error| format!("secret {agent}/{key}: {error}"))?
        .is_none()
    {
        return Err(format!("secret {agent}/{key} not found"));
    }
    bao.kv_delete_all(&settings.kv_mount, &path)
        .await
        .map_err(|error| format!("secret {agent}/{key}: {error}"))?;
    io.line(&format!(
        "secret {}/{path}: deleted (all versions)",
        settings.kv_mount
    ));
    Ok(())
}
