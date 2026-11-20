//! The built-in tools of a spec (ADR-0046, ADR-0104); provisioning builds them (ADR-0114).

use std::path::Path;
#[cfg(feature = "git")]
use std::path::{Component, PathBuf};
use std::sync::Arc;

#[cfg(feature = "atlassian")]
pub use grauwolf_atlassian::AtlassianClient;
pub use grauwolf_builtin_tools::{ConsoleUserIo, FileTools, UserIo, UserTools};
#[cfg(feature = "atlassian")]
use grauwolf_spec::AtlassianSpec;
use grauwolf_spec::{BuiltinGroup, BuiltinToolsSpec};
#[cfg(feature = "git")]
use grauwolf_spec::{GitSpec, PushPolicy};
use grauwolf_tools::DynTool;

use crate::config::{AtlassianConfig, Env, GitEnvConfig, ProcessEnv};
#[cfg(feature = "git")]
pub use grauwolf_tools_git::{GitConfig, GitTools};

/// The configuration the built-in tools read from the environment.
pub(crate) struct Settings<'a> {
    pub atlassian: &'a AtlassianConfig,
    pub git_env: &'a GitEnvConfig,
    /// Where the command tool takes the values of `command.env` from.
    pub command_env: &'a dyn Env,
    /// Workload identity of the hosts of `fetch.auth` (ADR-0110), in the order of the spec.
    pub fetch_auth: Vec<(String, Arc<grauwolf_spiffe::ClientAuth>)>,
    /// Workload identity of the git repository (`git.auth`).
    pub git_auth: Option<Arc<grauwolf_spiffe::ClientAuth>>,
}

/// The client authentications that `builtin_tools.fetch.auth` and `builtin_tools.git.auth`
/// name; they need the Workload API (async), so provisioning resolves them before building
/// the tools. Errors name `builtin_tools.<tool>.auth.<host>`.
pub(crate) async fn resolve_auth(
    builtin: &BuiltinToolsSpec,
    env: &dyn Env,
) -> Result<
    (
        Vec<(String, Arc<grauwolf_spiffe::ClientAuth>)>,
        Option<Arc<grauwolf_spiffe::ClientAuth>>,
    ),
    String,
> {
    let mut fetch = Vec::new();
    if let Some(section) = &builtin.fetch {
        for (host, auth) in &section.auth {
            let what = format!("builtin_tools.fetch.auth.{host}");
            fetch.push((
                host.clone(),
                crate::spiffe::client_auth(env, &what, auth).await?,
            ));
        }
    }
    let mut git = None;
    if let Some(section) = &builtin.git {
        let repo_host = grauwolf_spec::origin_host(&section.repo_url).unwrap_or_default();
        if let Some((host, auth)) = section
            .auth
            .iter()
            .find(|(pattern, _)| grauwolf_spec::host_matches(pattern, &repo_host))
        {
            let what = format!("builtin_tools.git.auth.{host}");
            git = Some(crate::spiffe::client_auth(env, &what, auth).await?);
        }
    }
    Ok((fetch, git))
}

/// Which of the built-in tools to build.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    /// Everything but the key-value store (it belongs to a user).
    WithoutKv,
    KvOnly,
}

/// The built-in tools of a provisioned agent that do not depend on the user (ADR-0114).
pub(crate) fn agent_tools(
    builtin: &BuiltinToolsSpec,
    base_dir: &Path,
    io: Arc<dyn UserIo>,
    settings: &Settings<'_>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    build(builtin, base_dir, Some(io), None, settings, Part::WithoutKv)
}

/// The key-value store of `user` (without: the shared area, ADR-0098).
pub(crate) fn kv_tools(
    builtin: &BuiltinToolsSpec,
    base_dir: &Path,
    user: Option<&str>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    build(
        builtin,
        base_dir,
        None,
        user,
        &Settings {
            atlassian: &AtlassianConfig::default(),
            git_env: &GitEnvConfig::default(),
            command_env: &ProcessEnv,
            fetch_auth: Vec::new(),
            git_auth: None,
        },
        Part::KvOnly,
    )
}

fn build(
    builtin: &BuiltinToolsSpec,
    base_dir: &Path,
    io: Option<Arc<dyn UserIo>>,
    user: Option<&str>,
    settings: &Settings<'_>,
    part: Part,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    let (atlassian, git_env) = (settings.atlassian, settings.git_env);
    // Credentials first: a missing one is the first thing to fix.
    let root = base_dir.join(builtin.root.as_deref().unwrap_or("."));
    let mut tools = Vec::new();
    // The groups come in the order of the table (ADR-0117); the match must name every group.
    for group in builtin.groups() {
        if (group == BuiltinGroup::Kv) != (part == Part::KvOnly) {
            continue;
        }
        let names = builtin.tools_of(group);
        match group {
            BuiltinGroup::User => {
                if let Some(io) = &io {
                    tools.extend(UserTools::new(Arc::clone(io)).tools(names)?);
                }
            }
            BuiltinGroup::Think => tools.push(grauwolf_builtin_tools::think_tool()),
            BuiltinGroup::Fetch => tools.push(fetch_tool(builtin, &settings.fetch_auth)?),
            BuiltinGroup::Command => {
                tools.push(command_tool(builtin, base_dir, settings.command_env)?);
            }
            BuiltinGroup::Todo => tools.extend(grauwolf_builtin_tools::todo_tools(names)?),
            BuiltinGroup::Kv => {
                // Per user in their own file next to the chat histories' scheme (ADR-0060).
                let file = match user {
                    None => "kv".to_owned(),
                    Some(_) => grauwolf_agents::memory::conversation_key(user, "kv")?,
                };
                tools.extend(
                    grauwolf_builtin_tools::KvTools::new(root.join(format!("{file}.json")))
                        .tools(names)?,
                );
            }
            BuiltinGroup::Jira => tools.extend(jira_tools(builtin, names, atlassian)?),
            BuiltinGroup::Confluence => {
                tools.extend(confluence_tools(builtin, names, atlassian)?);
            }
            BuiltinGroup::Git => {
                tools.extend(git_tools(
                    builtin,
                    names,
                    &root,
                    git_env,
                    &settings.git_auth,
                )?);
            }
            BuiltinGroup::File => {
                let file_tools = FileTools::new(&root).map_err(|error| {
                    format!("root of the file tools {}: {error}", root.display())
                })?;
                tools.extend(file_tools.tools(names)?);
            }
        }
    }
    Ok(tools)
}

/// The error of a spec that uses a group of tools whose feature is not compiled in.
#[cfg(not(all(
    feature = "git",
    feature = "fetch",
    feature = "command",
    feature = "atlassian"
)))]
fn missing_feature(what: &str, feature: &str) -> String {
    format!("this build has no {what} (feature `{feature}`)")
}

#[cfg(feature = "fetch")]
fn fetch_tool(
    builtin: &BuiltinToolsSpec,
    auth: &[(String, Arc<grauwolf_spiffe::ClientAuth>)],
) -> Result<Arc<dyn DynTool>, String> {
    let section = builtin
        .fetch
        .as_ref()
        .ok_or("the fetch tool needs a `fetch` section in `builtin_tools`")?;
    Ok(grauwolf_tools_web::fetch_tool(
        grauwolf_tools_web::FetchConfig {
            allowed_hosts: section.allowed_hosts.clone(),
            allow_http: section.allow_http,
            auth: auth.to_vec(),
        },
    ))
}

#[cfg(not(feature = "fetch"))]
fn fetch_tool(
    _: &BuiltinToolsSpec,
    _: &[(String, Arc<grauwolf_spiffe::ClientAuth>)],
) -> Result<Arc<dyn DynTool>, String> {
    Err(missing_feature("fetch tool", "fetch"))
}

#[cfg(feature = "command")]
fn command_tool(
    builtin: &BuiltinToolsSpec,
    base_dir: &Path,
    env: &dyn Env,
) -> Result<Arc<dyn DynTool>, String> {
    use grauwolf_tools_command::{CommandConfig, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_TIMEOUT_SECS};
    let section = builtin
        .command
        .as_ref()
        .ok_or("the command tool needs a `command` section in `builtin_tools`")?;
    let mut config = CommandConfig::new(section.allowed.clone());
    config.shell = section.shell;
    config.timeout =
        std::time::Duration::from_secs(section.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    config.max_output_bytes = section
        .max_output_bytes
        .map_or(DEFAULT_MAX_OUTPUT_BYTES, |bytes| {
            usize::try_from(bytes).unwrap_or(usize::MAX)
        });
    // Only the listed variables that are set reach the program (ADR-0099).
    config.env = section
        .env_or_default()
        .into_iter()
        .filter_map(|name| env.var(&name).map(|value| (name, value)))
        .collect();
    if section.workload_api {
        // The allowlisted programs get the agent's identity (ADR-0110); never in shell mode
        // (the spec refuses that).
        config.env.extend(crate::spiffe::workload_env(
            env,
            "builtin_tools.command.workload_api",
        )?);
    }
    let root = base_dir.join(builtin.root.as_deref().unwrap_or("."));
    grauwolf_tools_command::command_tool(root, config)
}

#[cfg(not(feature = "command"))]
fn command_tool(_: &BuiltinToolsSpec, _: &Path, _: &dyn Env) -> Result<Arc<dyn DynTool>, String> {
    Err(missing_feature("command tool", "command"))
}

#[cfg(feature = "atlassian")]
fn jira_tools(
    builtin: &BuiltinToolsSpec,
    names: Vec<&str>,
    atlassian: &AtlassianConfig,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    atlassian_client("Jira", builtin.jira.as_ref(), atlassian)?.jira_tools(names)
}

#[cfg(feature = "atlassian")]
fn confluence_tools(
    builtin: &BuiltinToolsSpec,
    names: Vec<&str>,
    atlassian: &AtlassianConfig,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    atlassian_client("Confluence", builtin.confluence.as_ref(), atlassian)?.confluence_tools(names)
}

#[cfg(not(feature = "atlassian"))]
fn jira_tools(
    _: &BuiltinToolsSpec,
    _: Vec<&str>,
    _: &AtlassianConfig,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    Err(missing_feature("Jira tools", "atlassian"))
}

#[cfg(not(feature = "atlassian"))]
fn confluence_tools(
    _: &BuiltinToolsSpec,
    _: Vec<&str>,
    _: &AtlassianConfig,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    Err(missing_feature("Confluence tools", "atlassian"))
}

#[cfg(feature = "git")]
fn git_tools(
    builtin: &BuiltinToolsSpec,
    names: Vec<&str>,
    root: &Path,
    git_env: &GitEnvConfig,
    auth: &Option<Arc<grauwolf_spiffe::ClientAuth>>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    let section = builtin
        .git
        .as_ref()
        .ok_or("the git tools need a `git` section in `builtin_tools`")?;
    let mut config = git_config(section, root, git_env)?;
    config.auth = auth.clone();
    GitTools::new(config).tools(names)
}

#[cfg(not(feature = "git"))]
fn git_tools(
    _: &BuiltinToolsSpec,
    _: Vec<&str>,
    _: &Path,
    _: &GitEnvConfig,
    _: &Option<Arc<grauwolf_spiffe::ClientAuth>>,
) -> Result<Vec<Arc<dyn DynTool>>, String> {
    Err(missing_feature("git tools", "git"))
}

#[cfg(feature = "atlassian")]
fn atlassian_client(
    product: &str,
    section: Option<&AtlassianSpec>,
    credentials: &AtlassianConfig,
) -> Result<AtlassianClient, String> {
    let section = section.ok_or_else(|| {
        format!(
            "the {product} tools need a `{}` section in `builtin_tools`",
            product.to_lowercase()
        )
    })?;
    let get = |value: &Option<String>, name: &str| {
        value.clone().ok_or_else(|| format!("{name} is not set"))
    };
    let client = get(&credentials.email, "ATLASSIAN_EMAIL").and_then(|email| {
        let token = get(&credentials.api_token, "ATLASSIAN_API_TOKEN")?;
        AtlassianClient::new(&section.base_url, &email, &token)
    });
    client.map_err(|error| {
        format!(
            "the {product} tools need Atlassian Cloud credentials: {error} \
             (ATLASSIAN_EMAIL is the account's e-mail address, ATLASSIAN_API_TOKEN an API token)"
        )
    })
}

#[cfg(feature = "git")]
/// The settings of the git tools: the directory below `root`, the token from `GIT_TOKEN` when
/// `use_token` asks for it, the author from the environment.
fn git_config(section: &GitSpec, root: &Path, env: &GitEnvConfig) -> Result<GitConfig, String> {
    let mut config = GitConfig::new(&section.repo_url, git_directory(section, root)?);
    config.push_policy = match section.push_policy {
        PushPolicy::NewBranches => grauwolf_tools_git::PushPolicy::NewBranches,
        PushPolicy::Any => grauwolf_tools_git::PushPolicy::Any,
    };
    if section.use_token {
        config.token = Some(env.token.clone().ok_or(
            "the git tools use `use_token`, but GIT_TOKEN is not set \
             (an access token for the repository; it is used as the password of the user \
             x-access-token)",
        )?);
    }
    config.author_name = env.author_name.clone();
    config.author_email = env.author_email.clone();
    Ok(config)
}

#[cfg(feature = "git")]
/// `directory` below `root`; an existing directory must not lead out through a symlink.
fn git_directory(section: &GitSpec, root: &Path) -> Result<PathBuf, String> {
    let relative = Path::new(section.directory_or_default());
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "the git directory '{}' must be relative and inside the root",
            relative.display()
        ));
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let directory = root.join(relative);
    match directory.canonicalize() {
        Ok(resolved) if !resolved.starts_with(&root) => Err(format!(
            "the git directory '{}' leads outside the root",
            relative.display()
        )),
        _ => Ok(directory),
    }
}

/// Every group of the table has a dispatch (ADR-0117): a group's tools are built, or the build
/// reports why not (missing credentials or feature) – they are never dropped silently.
#[cfg(test)]
mod dispatch {
    use super::*;
    use grauwolf_spec::BUILTIN_GROUPS;

    struct Silent;

    impl UserIo for Silent {
        fn say(&self, _: &str) {}

        fn ask<'a>(
            &'a self,
            _: &'a str,
        ) -> futures_util::future::BoxFuture<'a, Result<String, String>> {
            Box::pin(async { Ok(String::new()) })
        }
    }

    #[test]
    fn every_group_of_the_table_is_built_or_reported() {
        for info in BUILTIN_GROUPS {
            let builtin: BuiltinToolsSpec = serde_json::from_value(serde_json::json!({
                "tools": info.tools,
                "root": ".",
                "jira": { "base_url": "https://example.atlassian.net" },
                "confluence": { "base_url": "https://example.atlassian.net" },
                "git": { "repo_url": "https://example.com/r.git" },
                "fetch": { "allowed_hosts": ["example.com"] },
                "command": { "allowed": ["ls"] }
            }))
            .unwrap();
            let built = if info.group == BuiltinGroup::Kv {
                kv_tools(&builtin, Path::new("."), None)
            } else {
                let settings = Settings {
                    atlassian: &AtlassianConfig::default(),
                    git_env: &GitEnvConfig::default(),
                    command_env: &ProcessEnv,
                    fetch_auth: Vec::new(),
                    git_auth: None,
                };
                agent_tools(&builtin, Path::new("."), Arc::new(Silent), &settings)
            };
            if let Ok(tools) = built {
                let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
                for tool in info.tools {
                    assert!(
                        names.contains(tool),
                        "{:?}: {tool} was dropped, built {names:?}",
                        info.group
                    );
                }
            }
        }
    }
}

/// A build without a feature reports the tools of its group as an error before the run. These
/// tests only exist (and run) in such builds, e.g. `--no-default-features --features builtin`.
#[cfg(all(
    test,
    not(all(
        feature = "git",
        feature = "fetch",
        feature = "command",
        feature = "atlassian"
    ))
))]
mod missing_features {
    use super::*;

    fn error(tool: &str) -> String {
        use crate::provisioning::{ProvisionOptions, ProvisionedAgent};
        use grauwolf_spec::AgentSpecExt as _;
        let spec = grauwolf_spec::AgentSpec::from_json(
            &serde_json::json!({
                "spec_version": 1, "name": "missing-features",
                "model": { "provider": "ollama", "id": "m" },
                "builtin_tools": { "tools": [tool] },
                "strategy": { "kind": "single_run" }
            })
            .to_string(),
        )
        .unwrap();
        let options = ProvisionOptions::new(Arc::new(crate::config::MapEnv::new([])))
            .with_executor(Arc::new(grauwolf_llm::PromptExecutor::builder().build()));
        match futures_executor::block_on(ProvisionedAgent::new(&spec, Path::new("."), options)) {
            Ok(_) => panic!("{tool} must be refused"),
            Err(error) => error,
        }
    }

    #[cfg(not(feature = "git"))]
    #[test]
    fn git_tools_need_the_git_feature() {
        assert_eq!(
            error("git_commit"),
            "this build has no git tools (feature `git`)"
        );
    }

    #[cfg(not(feature = "command"))]
    #[test]
    fn command_tool_needs_the_command_feature() {
        assert_eq!(
            error("run_command"),
            "this build has no command tool (feature `command`)"
        );
    }

    #[cfg(not(feature = "fetch"))]
    #[test]
    fn fetch_tool_needs_the_fetch_feature() {
        assert_eq!(
            error("fetch"),
            "this build has no fetch tool (feature `fetch`)"
        );
    }

    #[cfg(not(feature = "atlassian"))]
    #[test]
    fn jira_and_confluence_tools_need_the_atlassian_feature() {
        assert_eq!(
            error("jira_search_issues"),
            "this build has no Jira tools (feature `atlassian`)"
        );
        assert_eq!(
            error("confluence_search_pages"),
            "this build has no Confluence tools (feature `atlassian`)"
        );
    }
}
