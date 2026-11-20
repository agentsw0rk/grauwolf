//! `skills` with git sources through the host's loader (ADR-0103); the resolving itself is tested
//! in `grauwolf-agent-tree`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use grauwolf_git::testing::{TestGitServer, commit_files};
use grauwolf_host::config::MapEnv;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("git-skills-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spec(url: &str, commit: &str) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "agent",
            "model": { "provider": "ollama", "id": "m" },
            "strategy": { "kind": "single_run" },
            "skills": [{ "git": url, "path": "skills", "rev": commit, "names": ["haiku-writer"] }]
        })
        .to_string(),
    )
    .unwrap()
}

#[tokio::test]
async fn the_loader_resolves_git_skills_and_reports_broken_ones_with_their_file() {
    let root = temp("loader");
    let repository = root.join("repo.git");
    let skill = |body: &str| -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([(
            "skills/haiku-writer/SKILL.md".to_owned(),
            body.as_bytes().to_vec(),
        )])
    };
    let good = commit_files(
        &repository,
        "main",
        "good",
        &skill("---\nname: haiku-writer\ndescription: Writes haiku\n---\nFive."),
    )
    .to_string();
    let bad = commit_files(&repository, "main", "bad", &skill("no front matter")).to_string();
    let server = TestGitServer::start(&repository, None).await;
    let loader = grauwolf_host::loader::AgentLoader::new(grauwolf_toolbuild::ToolBuilder::new(
        root.join("cache"),
    ))
    .with_env(Arc::new(MapEnv::default()));
    for (commit, ok) in [(&good, true), (&bad, false)] {
        let file = root.join(format!("{}.json", &commit[..7]));
        let mut value: serde_json::Value =
            serde_json::to_value(spec(&server.url("r"), commit)).unwrap();
        value.as_object_mut().unwrap().remove("graph");
        value["strategy"] = json!({ "kind": "single_run" });
        std::fs::write(&file, value.to_string()).unwrap();
        let result = loader.load(&file, None).await;
        if ok {
            result.unwrap();
        } else {
            let error = result.err().unwrap();
            assert!(
                error.contains("SKILL.md") && error.contains("front matter"),
                "{error}"
            );
        }
    }
}
