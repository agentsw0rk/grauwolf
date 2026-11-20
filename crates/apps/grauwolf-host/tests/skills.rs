//! The skill tools of a spec (ADR-0103).

use std::path::Path;

mod support;
use grauwolf_spec::{AgentSpec, AgentSpecExt};
use serde_json::json;

fn spec(skills: serde_json::Value) -> AgentSpec {
    AgentSpec::from_json(
        &json!({
            "spec_version": 1, "name": "writer",
            "model": { "provider": "ollama", "id": "q" },
            "skills": skills,
            "strategy": { "kind": "single_run" }
        })
        .to_string(),
    )
    .unwrap()
}

fn base(name: &str) -> std::path::PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("host-skills")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("skills/haiku-writer")).unwrap();
    std::fs::write(
        dir.join("skills/haiku-writer/SKILL.md"),
        "---\nname: haiku-writer\ndescription: Writes haiku\n---\nAnswer in 5-7-5.\n",
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn provides_the_skill_tools_for_local_directories_relative_to_the_spec() {
    let base = base("local");
    let spec = spec(json!([{ "dir": "skills" }]));
    let tools = support::tools(&spec, &base).await.unwrap();
    let names: Vec<_> = tools.iter().map(|tool| tool.name().to_owned()).collect();
    assert_eq!(names, ["load_skill", "read_skill_file"]);
    let text = tools[0]
        .call_json(json!({ "name": "haiku-writer" }))
        .await
        .unwrap();
    assert!(text.starts_with("Answer in 5-7-5."), "{text}");
    assert!(
        tools[0]
            .prompt_addition()
            .unwrap()
            .contains("- haiku-writer: Writes haiku")
    );
}

#[tokio::test]
async fn a_broken_skill_fails_before_the_run_with_the_file_name() {
    let base = base("broken");
    std::fs::write(base.join("skills/haiku-writer/SKILL.md"), "no front matter").unwrap();
    let spec = spec(json!([{ "dir": "skills" }]));
    let error = support::tools(&spec, &base).await.err().unwrap();
    assert!(
        error.contains("SKILL.md") && error.contains("front matter"),
        "{error}"
    );
}

#[tokio::test]
async fn an_unresolved_git_source_is_refused() {
    let spec = spec(json!([{ "git": "https://example.com/s.git", "rev": "4e1a9c2" }]));
    let error = support::tools(&spec, Path::new(".")).await.err().unwrap();
    assert!(error.contains("/skills/0"), "{error}");
}

#[test]
fn the_skill_folders_of_the_handbook_and_the_example_specs_load() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for dir in [
        "docs/user-guide/examples/03-skills",
        "examples/specs/skills",
    ] {
        let library =
            grauwolf_builtin_tools::SkillLibrary::load(&[grauwolf_builtin_tools::SkillRoot {
                dir: root.join(dir),
                names: None,
            }])
            .unwrap_or_else(|error| panic!("{dir}: {error}"));
        assert_eq!(library.list().len(), 1, "{dir}");
        assert_eq!(library.list()[0].0, "haiku-writer");
    }
}
