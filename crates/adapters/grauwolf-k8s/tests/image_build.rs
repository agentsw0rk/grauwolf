//! Running a Kaniko build Job against the fake API server (ADR-0129).

use std::time::Duration;

use grauwolf_k8s::testing::{FakeCluster, JobOutcome};
use grauwolf_k8s::{BuildFinished, BuildRequest, run_build, run_build_with};

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
        git_secret: None,
        registry_secret: None,
    }
}

#[tokio::test]
async fn a_build_runs_its_job_to_the_end_returns_the_log_and_deletes_the_job() {
    let fake = FakeCluster::new();
    fake.set_job_outcome(JobOutcome::Succeeds(
        "INFO Pushing image to ghcr.io/agentsw0rk/bot:0.1.0\nINFO Pushed\n".into(),
    ));

    let build = run_build(&fake.cluster(), &request(), Duration::from_secs(5))
        .await
        .unwrap();

    assert_eq!(build.finished, BuildFinished::Succeeded);
    assert_eq!(
        build.log,
        vec![
            "INFO Pushing image to ghcr.io/agentsw0rk/bot:0.1.0".to_owned(),
            "INFO Pushed".to_owned()
        ]
    );
    assert!(
        fake.deleted()
            .iter()
            .any(|path| path.ends_with("/namespaces/grauwolf-builds/jobs/bot-build-01234567")),
        "{:?}",
        fake.deleted()
    );
}

#[tokio::test]
async fn the_log_lines_of_a_running_build_are_handed_over_once_while_it_runs() {
    let fake = FakeCluster::new();
    fake.set_job_outcome(JobOutcome::Running(
        "INFO Cloning\n\nINFO Building\n".into(),
    ));
    let mut seen = Vec::new();

    let build = run_build_with(
        &fake.cluster(),
        &request(),
        Duration::from_millis(1200),
        |line| seen.push(line.to_owned()),
    )
    .await
    .unwrap();

    assert_eq!(build.finished, BuildFinished::TimedOut);
    assert_eq!(seen, vec!["INFO Cloning", "INFO Building"]);
    assert_eq!(build.log, seen);
}
