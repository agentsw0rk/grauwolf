//! The prepared agent tree, read phase (ADR-0115): one walk over a spec and its sub-agents.

use std::path::{Path, PathBuf};

use grauwolf_agent_tree::{AgentNode, AgentTree, Child, ReadOptions};

/// The read options of a program that knows the provider `ollama` only.
fn options() -> ReadOptions {
    ReadOptions {
        providers: Some(vec!["ollama".to_owned()]),
        ..ReadOptions::default()
    }
}

fn read(file: &Path) -> Result<AgentTree, grauwolf_agent_tree::ReadErrors> {
    AgentTree::read(file, &options())
}

fn temp(name: &str) -> grauwolf_test::TestDir {
    grauwolf_test::TestDir::new(&format!("tree-{name}"))
}

/// Writes the spec `name` in `dir` (path may have subdirectories) with the sub-agents `subs`
/// (agent name → spec path) and the extra fields `extra`.
fn spec(dir: &Path, name: &str, subs: &[(&str, &str)], extra: serde_json::Value) {
    let mut value = serde_json::json!({
        "spec_version": 1, "name": name.trim_end_matches(".json").replace('/', "_"),
        "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" }
    });
    if !subs.is_empty() {
        value["agent_tools"] = subs
            .iter()
            .map(|(agent, file)| {
                (
                    (*agent).to_owned(),
                    serde_json::json!({ "spec": file, "description": "d" }),
                )
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
    }
    for (key, field) in extra.as_object().into_iter().flatten() {
        value[key] = field.clone();
    }
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

fn none() -> serde_json::Value {
    serde_json::json!({})
}

#[test]
fn a_spec_without_sub_agents_is_a_tree_of_one_node() {
    let dir = temp("single");
    spec(&dir, "root.json", &[], none());

    let tree = read(&dir.join("root.json")).unwrap();

    assert_eq!(tree.nodes().len(), 1);
    assert!(tree.root().children().is_empty());
    assert_eq!(tree.root().relative(), Path::new("root.json"));
}

#[test]
fn a_sub_agent_under_two_parents_is_one_node() {
    let dir = temp("diamond");
    spec(
        &dir,
        "coordinator.json",
        &[("writer", "writer.json"), ("reviewer", "reviewer.json")],
        none(),
    );
    spec(
        &dir,
        "writer.json",
        &[("reviewer", "reviewer.json")],
        none(),
    );
    spec(&dir, "reviewer.json", &[], none());

    let tree = read(&dir.join("coordinator.json")).unwrap();

    assert_eq!(tree.nodes().len(), 3);
    let node_of = |parent: &AgentNode, name: &str| {
        parent.children().iter().find_map(|child| match child {
            Child::Local { name: n, node } if n == name => Some(*node),
            _ => None,
        })
    };
    let writer = node_of(tree.root(), "writer").unwrap();
    assert_eq!(
        node_of(tree.root(), "reviewer"),
        node_of(tree.node(writer), "reviewer")
    );
}

#[test]
fn a_cycle_is_an_error_that_names_the_chain() {
    let dir = temp("cycle");
    spec(&dir, "a.json", &[("b", "b.json")], none());
    spec(&dir, "b.json", &[("a", "a.json")], none());

    let error = read(&dir.join("a.json")).unwrap_err().to_string();

    assert!(
        error.contains(
            "b.json:/agent_tools/a/spec: sub-agents form a cycle: a.json → b.json → a.json"
        ),
        "{error}"
    );
}

#[test]
fn a_sub_spec_outside_the_roots_directory_is_an_error() {
    let dir = temp("outside");
    spec(&dir, "elsewhere/other.json", &[], none());
    spec(
        &dir,
        "app/root.json",
        &[("o", "../elsewhere/other.json")],
        none(),
    );

    let error = read(&dir.join("app/root.json")).unwrap_err().to_string();

    assert!(
        error.contains(
            "root.json:/agent_tools/o/spec: sub-agent spec outside the agent's directory"
        ),
        "{error}"
    );
}

#[test]
fn an_absolute_sub_spec_path_is_an_error_even_inside_the_directory() {
    let dir = temp("absolute");
    spec(&dir, "child.json", &[], none());
    let absolute = dir.join("child.json").display().to_string();
    spec(&dir, "root.json", &[("c", &absolute)], none());

    let error = read(&dir.join("root.json")).unwrap_err().to_string();

    assert!(
        error.contains("sub-agent spec outside the agent's directory"),
        "{error}"
    );
}

#[test]
fn a_dotdot_that_stays_inside_the_directory_is_fine_and_relative_to_the_root() {
    let dir = temp("inside");
    spec(&dir, "sib.json", &[], none());
    spec(&dir, "sub/mid.json", &[("s", "../sib.json")], none());
    spec(&dir, "root.json", &[("m", "sub/mid.json")], none());

    let tree = read(&dir.join("root.json")).unwrap();

    let relatives: Vec<_> = tree
        .nodes()
        .iter()
        .map(|n| n.relative().to_owned())
        .collect();
    assert_eq!(
        relatives,
        [
            PathBuf::from("root.json"),
            "sub/mid.json".into(),
            "sib.json".into()
        ]
    );
}

#[test]
fn a_remote_sub_agent_is_a_leaf() {
    let dir = temp("a2a");
    let mut root = serde_json::json!({
        "spec_version": 1, "name": "root", "model": { "provider": "ollama", "id": "m" },
        "strategy": { "kind": "single_run" },
        "agent_tools": { "far": { "url": "http://localhost:1/", "description": "d" } }
    });
    root["name"] = "root".into();
    std::fs::write(dir.join("root.json"), root.to_string()).unwrap();

    let tree = read(&dir.join("root.json")).unwrap();

    assert_eq!(tree.nodes().len(), 1);
    assert_eq!(
        tree.root().children(),
        [Child::Remote {
            name: "far".into(),
            url: "http://localhost:1/".into()
        }]
    );
}

#[test]
fn companions_are_the_paths_below_the_spec_directory() {
    let dir = temp("companions");
    spec(
        &dir,
        "root.json",
        &[],
        serde_json::json!({ "knowledge": {
            "sources": ["docs", "../outside.md"], "embedding_model": { "provider": "ollama", "id": "e" } } }),
    );

    let tree = read(&dir.join("root.json")).unwrap();

    assert_eq!(tree.root().companions(), [PathBuf::from("docs")]);
}

#[test]
fn errors_of_all_nodes_come_together_as_file_pointer_message() {
    let dir = temp("errors");
    spec(
        &dir,
        "root.json",
        &[("a", "a.json"), ("m", "missing.json")],
        none(),
    );
    std::fs::write(
        dir.join("a.json"),
        r#"{ "spec_version": 1, "name": "a", "model": { "provider": "nope", "id": "m" },
            "graph": { "edges": [ { "from": "start", "to": "nowhere" } ] } }"#,
    )
    .unwrap();

    let error = read(&dir.join("root.json")).unwrap_err().to_string();

    assert!(error.contains("a.json:/graph/edges/0/to"), "{error}");
    assert!(
        error.contains("a.json:/model/provider: unknown provider 'nope'"),
        "{error}"
    );
    assert!(error.contains("missing.json: not readable"), "{error}");
}

#[test]
fn agent_types_must_match_the_sub_agents_signature() {
    let dir = temp("signature");
    spec(&dir, "echo.json", &[], none());
    std::fs::write(
        dir.join("flow.json"),
        r#"{ "spec_version": 1, "name": "flow", "model": { "provider": "ollama", "id": "m" },
             "output": "Topic",
             "types": { "Topic": { "type": "object", "properties": { "title": { "type": "string" } } } },
             "agent_tools": { "worker": { "spec": "echo.json", "description": "Echoes" } },
             "graph": {
               "nodes": [ { "id": "work", "kind": "agent", "params": { "agent": "worker", "output": "Topic" } } ],
               "edges": [ { "from": "start", "to": "work" }, { "from": "work", "to": "finish" } ] } }"#,
    )
    .unwrap();

    let error = read(&dir.join("flow.json")).unwrap_err().to_string();

    assert!(error.contains("flow.json:"), "{error}");
    assert!(
        error.contains(
            "the agent 'worker' has the output type 'string', but the node declares 'Topic'"
        ),
        "{error}"
    );
}

#[test]
fn a_root_in_memory_reads_its_sub_agents_from_the_base_directory() {
    let dir = temp("memory");
    spec(&dir, "child.json", &[], none());
    spec(&dir, "root.json", &[("c", "child.json")], none());
    let root = grauwolf_agent_tree::read_spec(&dir.join("root.json")).unwrap();
    std::fs::remove_file(dir.join("root.json")).unwrap();

    let tree = AgentTree::read_spec(root, &dir, &options()).unwrap();

    assert_eq!(tree.nodes().len(), 2);
}

#[test]
fn providers_are_checked_only_when_the_program_names_them() {
    let dir = temp("providers");
    std::fs::write(
        dir.join("root.json"),
        r#"{ "spec_version": 1, "name": "root", "model": { "provider": "nope", "id": "m" },
             "strategy": { "kind": "single_run" } }"#,
    )
    .unwrap();

    assert!(read(&dir.join("root.json")).is_err());
    assert!(AgentTree::read(&dir.join("root.json"), &ReadOptions::default()).is_ok());
}
