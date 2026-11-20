//! Building a harness's image in the cluster with Kaniko (ADR-0129): the build Job, its context a
//! commit of the workspace's git repository, its destination the user's registry.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{Cluster, kinds};

/// The Kaniko image of the build Job (ADR-0129 B3).
pub const KANIKO_IMAGE: &str = "ghcr.io/osscontainertools/kaniko:v1.27.2";

/// The namespace builds run in (ADR-0129).
pub const BUILD_NAMESPACE: &str = "grauwolf-builds";

/// What a build needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    /// The harness's name: the Job is `<name>-build-<short commit>`.
    pub name: String,
    pub namespace: String,
    /// The Kaniko image (ADR-0129 B3).
    pub kaniko_image: String,
    /// The repository Kaniko clones, e.g. `github.com/agentsw0rk/agents.git` (no scheme).
    pub repository: String,
    /// The branch the commit is on, e.g. `refs/heads/main`.
    pub git_ref: String,
    /// The full commit.
    pub commit: String,
    /// The project folder in the repository, e.g. `build/bot`.
    pub sub_path: String,
    /// The image to push, e.g. `ghcr.io/agentsw0rk/bot:0.1.0`.
    pub destination: String,
    /// A Secret with the keys `username` and `password` for a private repository.
    pub git_secret: Option<String>,
    /// A Secret of type `kubernetes.io/dockerconfigjson` for the registry.
    pub registry_secret: Option<String>,
}

/// The build Job of `request`: one Kaniko container, no retry (a failed build is reported, not
/// repeated), the git token as `GIT_USERNAME`/`GIT_PASSWORD`, the registry's `config.json` from its
/// Secret at `/kaniko/.docker`.
pub fn kaniko_job(request: &BuildRequest) -> Value {
    let short = &request.commit[..request.commit.len().min(8)];
    let mut container = json!({
        "name": "kaniko",
        "image": request.kaniko_image,
        "args": [
            format!("--context=git://{}#{}#{}", request.repository, request.git_ref, request.commit),
            format!("--context-sub-path={}", request.sub_path),
            "--dockerfile=Dockerfile",
            format!("--destination={}", request.destination),
        ],
    });
    if let Some(secret) = &request.git_secret {
        let from = |key: &str| json!({ "secretKeyRef": { "name": secret, "key": key } });
        container["env"] = json!([
            { "name": "GIT_USERNAME", "valueFrom": from("username") },
            { "name": "GIT_PASSWORD", "valueFrom": from("password") },
        ]);
    }
    let mut pod = json!({ "restartPolicy": "Never", "containers": [] });
    if let Some(secret) = &request.registry_secret {
        container["volumeMounts"] = json!([{ "name": "registry", "mountPath": "/kaniko/.docker" }]);
        pod["volumes"] = json!([{ "name": "registry", "secret": { "secretName": secret,
            "items": [{ "key": ".dockerconfigjson", "path": "config.json" }] } }]);
    }
    pod["containers"] = json!([container]);
    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": { "name": format!("{}-build-{short}", request.name), "namespace": request.namespace },
        "spec": { "backoffLimit": 0, "template": { "spec": pod } },
    })
}

/// The Secret of type `kubernetes.io/dockerconfigjson` that lets Kaniko push to `server` as
/// `username` with `token` (written as `stringData`; the API server encodes it).
pub fn registry_secret(
    name: &str,
    namespace: &str,
    server: &str,
    username: &str,
    token: &str,
) -> Value {
    use base64::Engine;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{token}"));
    let config =
        json!({ "auths": { server: { "username": username, "password": token, "auth": auth } } });
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": name, "namespace": namespace },
        "type": "kubernetes.io/dockerconfigjson",
        "stringData": { ".dockerconfigjson": config.to_string() },
    })
}

/// The repository of a git remote as Kaniko's git context names it (`<host>/<path>.git`, cloned over
/// https): from `https://…`, `ssh://git@…` and `git@host:path`, credentials in the URL dropped – the
/// build gets its token from a Secret, never from the context.
pub fn kaniko_repository(remote: &str) -> Result<String, String> {
    let rest = if let Some(rest) = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("ssh://"))
    {
        // Drop `user[:password]@` in front of the host.
        let (authority, path) = rest
            .split_once('/')
            .ok_or_else(|| format!("no repository path in git remote '{remote}'"))?;
        let host = authority.rsplit('@').next().unwrap_or(authority);
        format!("{host}/{path}")
    } else if let Some((user_host, path)) = remote
        .split_once(':')
        .filter(|(user_host, _)| user_host.contains('@') && !user_host.contains('/'))
    {
        let host = user_host.rsplit('@').next().unwrap_or(user_host);
        format!("{host}/{path}")
    } else {
        return Err(format!(
            "git remote '{remote}' is no https or ssh URL a cluster could clone"
        ));
    };
    Ok(if rest.ends_with(".git") {
        rest
    } else {
        format!("{rest}.git")
    })
}

/// How a build ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildFinished {
    /// Kaniko built and pushed the image.
    Succeeded,
    Failed,
    /// Not finished within the time given; the Job is deleted anyway.
    TimedOut,
}

/// A build that ran: how it ended and the non-empty lines of Kaniko's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub finished: BuildFinished,
    pub log: Vec<String>,
}

/// How often the Job is looked at while it runs.
const POLL: Duration = Duration::from_millis(500);

/// Runs the build Job of `request` (the namespace and the Secrets it names must exist): creates
/// it, waits up to `timeout` for its end, reads the log of its pod and deletes it – whatever
/// happened while waiting.
pub async fn run_build(
    cluster: &Cluster,
    request: &BuildRequest,
    timeout: Duration,
) -> Result<Build, String> {
    run_build_with(cluster, request, timeout, |_| ()).await
}

/// [`run_build`] that hands every line of the log to `on_line` as it appears (the log is read
/// with every look at the Job), each line once and in order.
pub async fn run_build_with<F: FnMut(&str)>(
    cluster: &Cluster,
    request: &BuildRequest,
    timeout: Duration,
    mut on_line: F,
) -> Result<Build, String> {
    let job = kaniko_job(request);
    let name = job["metadata"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let namespace = request.namespace.as_str();
    cluster
        .ensure(&kinds::JOB, Some(namespace), &job)
        .await
        .map_err(|error| format!("Job {name}: {error}"))?;
    let mut sent = 0;
    let finished = wait(cluster, namespace, &name, timeout, &mut sent, &mut on_line).await;
    let log = job_log(cluster, namespace, &name).await;
    for line in log.iter().skip(sent) {
        on_line(line);
    }
    cluster
        .delete(&kinds::JOB, Some(namespace), &name)
        .await
        .map_err(|error| format!("cannot delete Job {name}: {error}"))?;
    Ok(Build {
        finished: finished?,
        log,
    })
}

async fn wait<F: FnMut(&str)>(
    cluster: &Cluster,
    namespace: &str,
    name: &str,
    timeout: Duration,
    sent: &mut usize,
    on_line: &mut F,
) -> Result<BuildFinished, String> {
    let started = Instant::now();
    loop {
        let job = cluster
            .get(&kinds::JOB, Some(namespace), name)
            .await
            .map_err(|error| format!("Job {name}: {error}"))?
            .ok_or_else(|| format!("Job {name} disappeared"))?;
        if job["status"]["succeeded"].as_u64().unwrap_or(0) > 0 {
            return Ok(BuildFinished::Succeeded);
        }
        if job["status"]["failed"].as_u64().unwrap_or(0) > 0 {
            return Ok(BuildFinished::Failed);
        }
        if started.elapsed() >= timeout {
            return Ok(BuildFinished::TimedOut);
        }
        let log = job_log(cluster, namespace, name).await;
        for line in log.iter().skip(*sent) {
            on_line(line);
        }
        *sent = (*sent).max(log.len());
        tokio::time::sleep(POLL).await;
    }
}

/// The non-empty lines of the log of the Job's pod (empty when there is no pod or log).
async fn job_log(cluster: &Cluster, namespace: &str, job: &str) -> Vec<String> {
    let Ok(pods) = cluster
        .list(
            &kinds::POD,
            Some(namespace),
            Some(&format!("job-name={job}")),
        )
        .await
    else {
        return Vec::new();
    };
    let Some(pod) = pods
        .first()
        .and_then(|pod| pod["metadata"]["name"].as_str())
    else {
        return Vec::new();
    };
    let Ok(log) = cluster.pod_log(namespace, pod, 2000).await else {
        return Vec::new();
    };
    log.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> BuildRequest {
        BuildRequest {
            name: "bot".into(),
            namespace: "grauwolf-builds".into(),
            kaniko_image: "ghcr.io/osscontainertools/kaniko:v1.27.2".into(),
            repository: "github.com/agentsw0rk/agents.git".into(),
            git_ref: "refs/heads/main".into(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            sub_path: "build/bot".into(),
            destination: "ghcr.io/agentsw0rk/bot:0.1.0".into(),
            git_secret: Some("bot-git".into()),
            registry_secret: Some("bot-registry".into()),
        }
    }

    #[test]
    fn the_repository_kaniko_clones_comes_from_the_remote_without_credentials() {
        for remote in [
            "https://github.com/agentsw0rk/agents.git",
            "https://github.com/agentsw0rk/agents",
            "https://x-access-token:secret@github.com/agentsw0rk/agents.git",
            "git@github.com:agentsw0rk/agents.git",
            "ssh://git@github.com/agentsw0rk/agents.git",
        ] {
            assert_eq!(
                kaniko_repository(remote),
                Ok("github.com/agentsw0rk/agents.git".to_owned()),
                "{remote}"
            );
        }
        assert!(kaniko_repository("/home/me/agents").is_err());
    }

    #[test]
    fn the_registry_secret_holds_a_docker_config_for_the_server() {
        let secret = registry_secret(
            "bot-registry",
            "grauwolf-builds",
            "ghcr.io",
            "arek",
            "token-1",
        );

        assert_eq!(secret["kind"], "Secret");
        assert_eq!(secret["type"], "kubernetes.io/dockerconfigjson");
        assert_eq!(
            secret["metadata"],
            json!({ "name": "bot-registry", "namespace": "grauwolf-builds" })
        );
        let config: Value =
            serde_json::from_str(secret["stringData"][".dockerconfigjson"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            config,
            json!({ "auths": { "ghcr.io": { "username": "arek", "password": "token-1", "auth": "YXJlazp0b2tlbi0x" } } })
        );
    }

    #[test]
    fn the_job_builds_the_project_folder_of_a_commit_and_pushes_it_with_the_secrets() {
        let job = kaniko_job(&request());

        assert_eq!(job["kind"], "Job");
        assert_eq!(job["metadata"]["name"], "bot-build-01234567");
        assert_eq!(job["metadata"]["namespace"], "grauwolf-builds");
        assert_eq!(job["spec"]["backoffLimit"], 0);
        let pod = &job["spec"]["template"]["spec"];
        assert_eq!(pod["restartPolicy"], "Never");
        let kaniko = &pod["containers"][0];
        assert_eq!(kaniko["image"], "ghcr.io/osscontainertools/kaniko:v1.27.2");
        assert_eq!(
            kaniko["args"],
            json!([
                "--context=git://github.com/agentsw0rk/agents.git#refs/heads/main#0123456789abcdef0123456789abcdef01234567",
                "--context-sub-path=build/bot",
                "--dockerfile=Dockerfile",
                "--destination=ghcr.io/agentsw0rk/bot:0.1.0"
            ])
        );
        assert_eq!(
            kaniko["env"],
            json!([
                { "name": "GIT_USERNAME", "valueFrom": { "secretKeyRef": { "name": "bot-git", "key": "username" } } },
                { "name": "GIT_PASSWORD", "valueFrom": { "secretKeyRef": { "name": "bot-git", "key": "password" } } }
            ])
        );
        assert_eq!(
            kaniko["volumeMounts"],
            json!([{ "name": "registry", "mountPath": "/kaniko/.docker" }])
        );
        assert_eq!(
            pod["volumes"],
            json!([{ "name": "registry", "secret": { "secretName": "bot-registry",
                "items": [{ "key": ".dockerconfigjson", "path": "config.json" }] } }])
        );
    }
}
