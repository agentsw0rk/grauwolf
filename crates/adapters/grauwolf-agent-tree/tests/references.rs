//! `file_references`: every file a spec names, found by the same walk as the reader, whether the
//! file exists or not (grauwolf studio, ADR-0126).

use grauwolf_agent_tree::{ReferenceKind, file_references};
use grauwolf_generator::CompanionKind;
use grauwolf_test::TestDir;
use serde_json::json;

#[test]
fn lists_code_files_tool_files_and_companions_with_their_pointers() {
    let dir = TestDir::new("agent-tree-references");
    std::fs::create_dir_all(dir.join("tools")).unwrap();
    std::fs::write(
        dir.join("tools/count.tool.json"),
        json!({
            "name": "count", "description": "Counts",
            "args": { "type": "object", "properties": {} },
            "code_file": "count.rs"
        })
        .to_string(),
    )
    .unwrap();
    let spec = json!({
        "spec_version": 1, "name": "librarian",
        "model": { "provider": "ollama", "id": "m" },
        "tools": [{
            "name": "add", "description": "Adds",
            "args": { "type": "object", "properties": {} },
            "code_file": "tools/add.rs"
        }],
        "tool_files": ["tools/count.tool.json", "tools/missing.tool.json"],
        "tools_dir": "tools/folder",
        "agent_tools": { "writer": { "spec": "writer.json", "description": "Writes" } },
        "knowledge": {
            "sources": ["docs"],
            "embedding_model": { "provider": "ollama", "id": "e" }
        },
        "skills": [{ "dir": "skills" }],
        "graph": {
            "nodes": [{ "id": "score", "kind": "code",
                        "params": { "input": "string", "output": "string", "code_file": "code/score.rs" } }],
            "edges": [
                { "from": "start", "to": "score" },
                { "from": "score", "to": "finish",
                  "when": { "kind": "on_code", "params": { "input": "string", "code_file": "code/check.rs" } },
                  "transform": { "output": "string", "code_file": "code/shape.rs" } }
            ]
        }
    });

    let references: Vec<(String, String, std::path::PathBuf, ReferenceKind)> =
        file_references(&spec, dir.path())
            .into_iter()
            .map(|r| (r.pointer, r.value, r.path, r.kind))
            .collect();

    let expected = [
        (
            "/tool_files/0",
            "tools/count.tool.json",
            "tools/count.tool.json",
            ReferenceKind::ToolFile,
        ),
        (
            "/tool_files/0/code_file",
            "count.rs",
            "tools/count.rs",
            ReferenceKind::ToolCode,
        ),
        (
            "/tool_files/1",
            "tools/missing.tool.json",
            "tools/missing.tool.json",
            ReferenceKind::ToolFile,
        ),
        (
            "/tools/0/code_file",
            "tools/add.rs",
            "tools/add.rs",
            ReferenceKind::ToolCode,
        ),
        (
            "/graph/nodes/0/params/code_file",
            "code/score.rs",
            "code/score.rs",
            ReferenceKind::NodeCode,
        ),
        (
            "/graph/edges/1/when/params/code_file",
            "code/check.rs",
            "code/check.rs",
            ReferenceKind::ConditionCode,
        ),
        (
            "/graph/edges/1/transform/code_file",
            "code/shape.rs",
            "code/shape.rs",
            ReferenceKind::TransformCode,
        ),
        (
            "/tools_dir",
            "tools/folder",
            "tools/folder",
            ReferenceKind::ToolsDir,
        ),
        (
            "/agent_tools/writer/spec",
            "writer.json",
            "writer.json",
            ReferenceKind::SubAgent,
        ),
        (
            "/knowledge/sources/0",
            "docs",
            "docs",
            ReferenceKind::Companion(CompanionKind::KnowledgeSource),
        ),
        (
            "/skills/0/dir",
            "skills",
            "skills",
            ReferenceKind::Companion(CompanionKind::SkillDir),
        ),
    ]
    .map(|(pointer, value, path, kind)| {
        (pointer.to_owned(), value.to_owned(), dir.join(path), kind)
    });
    assert_eq!(references, expected);
}

#[test]
fn a_spec_that_does_not_load_still_lists_its_code_files() {
    let dir = TestDir::new("agent-tree-references-broken");
    let spec = json!({
        "spec_version": 1, "name": "broken",
        "tools": [{ "name": "add", "code_file": "add.rs" }],
        "skills": [{ "dir": "skills" }]
    });

    let pointers: Vec<String> = file_references(&spec, dir.path())
        .into_iter()
        .map(|r| r.pointer)
        .collect();

    assert_eq!(pointers, ["/tools/0/code_file"]);
}
