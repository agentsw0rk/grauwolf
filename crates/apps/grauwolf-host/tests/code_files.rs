//! Code of graphs in their own files (ADR-0102, ADR-0073) and its build errors.

use std::path::{Path, PathBuf};

use grauwolf_host::loader::{build_tools, load_spec};
use grauwolf_spec::AgentSpecExt;
use grauwolf_toolbuild::ToolBuilder;
use serde_json::{Value, json};

/// Fresh directory in the temporary directory of the container.
fn dir(name: &str) -> grauwolf_test::TestDir {
    let dir = grauwolf_test::TestDir::new(name);
    std::fs::create_dir_all(dir.join("code")).unwrap();
    dir
}

/// start → shout (code, `code_file`) → finish with a condition and a transformation from files.
fn spec(extra_node_params: Value) -> Value {
    let mut params = json!({ "input": "string", "output": "string", "code_file": "code/shout.rs" });
    for (key, value) in extra_node_params.as_object().unwrap() {
        params[key] = value.clone();
    }
    json!({
        "spec_version": 1, "name": "shouter",
        "model": { "provider": "ollama", "id": "m" },
        "graph": {
            "nodes": [ { "id": "shout", "kind": "code", "params": params } ],
            "edges": [
                { "from": "start", "to": "shout" },
                { "from": "shout", "to": "finish",
                  "when": { "kind": "on_code", "params": { "input": "string", "code_file": "code/loud.rs" } },
                  "transform": { "output": "string", "code_file": "code/wrap.rs" } },
                { "from": "shout", "to": "finish" }
            ]
        },
        "subgraphs": {
            "inner": {
                "graph": {
                    "nodes": [ { "id": "echo", "kind": "code", "params": { "input": "string", "output": "string", "code_file": "code/shout.rs" } } ],
                    "edges": [ { "from": "start", "to": "echo" }, { "from": "echo", "to": "finish" } ]
                }
            }
        }
    })
}

fn write_files(dir: &Path) {
    std::fs::write(dir.join("code/shout.rs"), "Ok(input.to_uppercase())").unwrap();
    std::fs::write(dir.join("code/loud.rs"), "value.len() > 3").unwrap();
    std::fs::write(dir.join("code/wrap.rs"), "format!(\"<{value}>\")").unwrap();
}

#[test]
fn reads_the_code_of_nodes_conditions_transformations_and_subgraphs_from_files() {
    let dir = dir("graph-code-files");
    write_files(&dir);
    std::fs::write(dir.join("spec.json"), spec(json!({})).to_string()).unwrap();

    let loaded = load_spec(&dir.join("spec.json")).unwrap();

    let pieces = loaded.code_pieces();
    let code: Vec<_> = pieces
        .iter()
        .map(|piece| (piece.name.as_str(), piece.code.as_str()))
        .collect();
    assert_eq!(
        code,
        [
            ("code_main_shout", "Ok(input.to_uppercase())"),
            ("condition_main_shout_finish", "value.len() > 3"),
            ("transform_main_shout_finish", "format!(\"<{value}>\")"),
            ("code_sub_inner_echo", "Ok(input.to_uppercase())"),
        ]
    );
}

#[test]
fn reports_missing_files_and_code_given_twice_with_their_place() {
    let dir = dir("graph-code-file-errors");
    write_files(&dir);
    std::fs::write(
        dir.join("twice.json"),
        spec(json!({ "code": "Ok(input)" })).to_string(),
    )
    .unwrap();
    std::fs::remove_file(dir.join("code/wrap.rs")).unwrap();
    std::fs::write(dir.join("missing.json"), spec(json!({})).to_string()).unwrap();

    let twice = load_spec(&dir.join("twice.json")).unwrap_err();
    let missing = load_spec(&dir.join("missing.json")).unwrap_err();

    assert!(
        twice.contains("/graph/nodes/0/params") && twice.contains("exactly one"),
        "{twice}"
    );
    assert!(
        missing.contains("/graph/edges/1/transform/code_file") && missing.contains("wrap.rs"),
        "{missing}"
    );
}

#[test]
#[ignore = "slow: builds code for WASM with cargo"]
fn compile_errors_point_to_the_code_in_the_spec_like_validation_errors() {
    let dir = dir("graph-code-build-error");
    write_files(&dir);
    std::fs::write(dir.join("code/loud.rs"), "let n: u32 = \"x\";\nn > 3").unwrap();
    let file = dir.join("spec.json");
    std::fs::write(&file, spec(json!({})).to_string()).unwrap();
    let loaded = load_spec(&file).unwrap();
    let builder = ToolBuilder::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/host-code-tests"),
    );

    let error = build_tools(&file, &loaded, &builder).unwrap_err();

    assert!(
        error.contains("spec.json:/graph/edges/1/when/params/code:1:"),
        "{error}"
    );
}

#[tokio::test]
#[ignore = "slow: builds code for WASM with cargo"]
async fn the_loader_builds_the_code_of_the_graph_at_run_start() {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/user-guide/examples/04-code.json");
    let builder = ToolBuilder::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/host-code-tests"),
    );

    let (spec, _agent) = grauwolf_host::loader::AgentLoader::new(builder)
        .load(&file, None)
        .await
        .unwrap();

    assert_eq!(spec.code_pieces().len(), 4);
}
